//! Library analysis in the background: one track at a time on a low-priority
//! thread, out of the way of playback. Files already in the playback cache are
//! read from there; others are fetched into a small separate cache so a
//! library-wide run doesn't push recently played tracks out.
//!
//! Results go into the library db (keyed by track id, with a stamp of the
//! file) and to the UI. A pause, or quitting, stops the track in progress;
//! it goes back to the front of the queue.

use analysis::{Record, TrackAnalysis};
use audio::Status;
use library::{AnalysisStore, Track};
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use streamcache::{CacheKey, StreamCache};

/// Audio seconds analysed per wall-clock second, assumed until measured.
const ASSUMED_SPEED: f64 = 20.0;

/// One track to analyse. Its stream URL is made only when it's fetched (see
/// [`Runner::set_server`]): making one per track for a whole library on the UI
/// thread is a noticeable stall.
#[derive(Debug, Clone)]
pub struct Job {
    pub track_id: String,
    pub suffix: Option<String>,
    pub stamp: String,
    pub secs: f64,
    /// A downloaded copy (offline store), read directly instead of fetching.
    pub local: Option<std::path::PathBuf>,
}

/// The stamp a track's stored analysis must carry to still be current.
pub fn stamp_of(t: &Track) -> String {
    analysis::stamp(t.size, t.suffix.as_deref(), t.bit_depth, t.sample_rate, t.duration_secs)
}

pub fn job_for(t: &Track, local: Option<std::path::PathBuf>) -> Job {
    Job {
        track_id: t.id.clone(),
        suffix: t.suffix.clone(),
        stamp: stamp_of(t),
        secs: f64::from(t.duration_secs.unwrap_or(0)),
        local,
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Progress {
    /// Finished (or failed) since the queue was last empty.
    pub done: usize,
    pub failed: usize,
    /// `done` plus what's still queued or running.
    pub total: usize,
    pub paused: bool,
    pub running: bool,
    /// Held back while playback needs the network.
    pub waiting: bool,
    /// Audio still to analyse, and the estimated wall-clock time for it.
    pub remaining_secs: f64,
    pub eta_secs: f64,
}

impl Progress {
    pub fn active(&self) -> bool {
        self.total > self.done
    }
}

/// A track's stream URL on the signed-in server.
pub type StreamUrl = Arc<dyn Fn(&str) -> String + Send + Sync>;

#[derive(Default)]
struct State {
    queue: VecDeque<Job>,
    queued: HashSet<String>,
    /// Audio seconds in `queue`, kept as jobs come and go so progress (read
    /// every frame during a run) doesn't walk the whole queue.
    queued_secs: f64,
    current: Option<Job>,
    /// Where tracks without a downloaded copy are fetched from; `None` once
    /// signed out.
    server: Option<StreamUrl>,
    paused: bool,
    waiting: bool,
    done: usize,
    failed: usize,
    /// Throughput so far: audio seconds over wall seconds.
    audio_secs: f64,
    wall_secs: f64,
}

impl State {
    fn push_back(&mut self, job: Job) {
        self.queued_secs += job.secs;
        self.queue.push_back(job);
    }

    fn push_front(&mut self, job: Job) {
        self.queued_secs += job.secs;
        self.queue.push_front(job);
    }

    fn pop_front(&mut self) -> Option<Job> {
        let job = self.queue.pop_front()?;
        self.queued.remove(&job.track_id);
        self.queued_secs = if self.queue.is_empty() { 0.0 } else { self.queued_secs - job.secs };
        Some(job)
    }

    fn progress(&self) -> Progress {
        let waiting = self.queue.len() + usize::from(self.current.is_some());
        let remaining_secs = self.queued_secs.max(0.0) + self.current.as_ref().map_or(0.0, |j| j.secs);
        let speed = if self.wall_secs > 5.0 { self.audio_secs / self.wall_secs } else { ASSUMED_SPEED };
        Progress {
            done: self.done,
            failed: self.failed,
            total: self.done + waiting,
            paused: self.paused,
            running: self.current.is_some(),
            waiting: self.waiting,
            remaining_secs,
            eta_secs: remaining_secs / speed.max(0.1),
        }
    }
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    quit: AtomicBool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub struct Config {
    /// Where results are kept.
    pub store: Arc<AnalysisStore>,
    /// The playback cache: complete files there are read without a download.
    pub playback: StreamCache,
    /// Where files that aren't cached are fetched to.
    pub scratch: StreamCache,
    pub status: Arc<Status>,
    /// Called with each finished analysis (on the worker thread).
    pub on_result: Arc<dyn Fn(String, Record) + Send + Sync>,
    /// Called whenever progress changes (on the worker thread).
    pub on_progress: Arc<dyn Fn() + Send + Sync>,
}

pub struct Runner {
    shared: Arc<Shared>,
}

impl Runner {
    pub fn start(cfg: Config) -> Self {
        let shared = Arc::new(Shared { state: Mutex::default(), wake: Condvar::new(), quit: AtomicBool::new(false) });
        let worker = shared.clone();
        let spawned = std::thread::Builder::new().name("analysis".into()).spawn(move || {
            lower_priority();
            run(&worker, &cfg);
        });
        if let Err(e) = spawned {
            tracing::warn!("analysis thread: {e}");
        }
        Self { shared }
    }

    /// Queues jobs not already queued or running; returns how many were new.
    pub fn enqueue(&self, jobs: impl IntoIterator<Item = Job>) -> usize {
        let mut st = self.shared.lock();
        if st.queue.is_empty() && st.current.is_none() {
            // A fresh run: count from zero.
            st.done = 0;
            st.failed = 0;
        }
        let mut added = 0;
        for job in jobs {
            // A cleared job still stopping counts as gone: it isn't re-queued when it stops.
            let running = st.current.as_ref().is_some_and(|c| c.track_id == job.track_id && !c.stamp.is_empty());
            if !running && st.queued.insert(job.track_id.clone()) {
                st.push_back(job);
                added += 1;
            }
        }
        drop(st);
        self.shared.wake.notify_all();
        added
    }

    /// Where tracks without a downloaded copy are fetched from (`None`: signed
    /// out; such tracks fail).
    pub fn set_server(&self, server: Option<StreamUrl>) {
        self.shared.lock().server = server;
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.lock().paused = paused;
        self.shared.wake.notify_all();
    }

    /// Drops everything queued; the track in progress is stopped.
    pub fn clear(&self) {
        let mut st = self.shared.lock();
        st.queue.clear();
        st.queued.clear();
        st.queued_secs = 0.0;
        st.done = 0;
        st.failed = 0;
        if let Some(job) = st.current.as_mut() {
            // Marks it as not to be re-queued when it stops.
            job.stamp.clear();
        }
        drop(st);
        self.shared.wake.notify_all();
    }

    pub fn progress(&self) -> Progress {
        self.shared.lock().progress()
    }

    /// How many of `ids` are queued or being analysed.
    pub fn pending<'a>(&self, ids: impl IntoIterator<Item = &'a str>) -> usize {
        let st = self.shared.lock();
        ids.into_iter().filter(|id| st.queued.contains(*id) || st.current.as_ref().is_some_and(|c| c.track_id == *id)).count()
    }
}

/// Audio seconds analysed per second, before any have been measured.
pub fn assumed_eta(audio_secs: f64) -> f64 {
    audio_secs / ASSUMED_SPEED
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.shared.quit.store(true, Ordering::Relaxed);
        self.shared.wake.notify_all();
    }
}

/// Background priority. macOS: the background QoS class, which keeps the
/// thread to spare capacity (and an efficiency core where there is one) and
/// throttles its disk and network. Linux: nice 19 (the lowest priority), for this
/// thread only. Windows: background mode, which lowers its CPU, disk and
/// memory priority.
pub(crate) fn lower_priority() {
    #[cfg(target_os = "macos")]
    // SAFETY: only changes the calling thread's QoS class.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_BACKGROUND, 0);
    }
    #[cfg(target_os = "linux")]
    // SAFETY: on Linux a thread id given to setpriority names just that thread.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t, 19);
    }
    #[cfg(windows)]
    // SAFETY: only changes the calling thread's priority.
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN};
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
}

/// Playback is fetching (the current track, of known size or not, or the next
/// one's prefetch) or filling its buffer: leave the network to it.
fn playback_busy(status: &Status) -> bool {
    status.needs_network()
}

fn run(shared: &Shared, cfg: &Config) {
    loop {
        // Next job, once not paused and playback doesn't need the bandwidth.
        let (job, server) = {
            let mut st = shared.lock();
            loop {
                if shared.quit.load(Ordering::Relaxed) {
                    return;
                }
                let ready = !st.paused && !st.queue.is_empty();
                let busy = ready && playback_busy(&cfg.status);
                if st.waiting != busy {
                    st.waiting = busy;
                    (cfg.on_progress)();
                }
                if ready && !busy {
                    break;
                }
                st = shared.wake.wait_timeout(st, Duration::from_millis(500)).unwrap_or_else(|e| e.into_inner()).0;
            }
            let job = st.pop_front().expect("checked non-empty");
            st.current = Some(job.clone());
            let server = st.server.clone();
            (job, server)
        };
        (cfg.on_progress)();

        let started = Instant::now();
        let stop = || {
            shared.quit.load(Ordering::Relaxed) || {
                let st = shared.lock();
                st.paused || st.current.as_ref().is_none_or(|c| c.stamp.is_empty())
            }
        };
        // A panic on one odd file fails that job; the worker carries on.
        let url = || server.as_ref().map(|f| f(&job.track_id));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| analyze_job(&job, url, cfg, &stop)))
            .unwrap_or_else(|p| Err(analysis::AnalyzeError::Failed(format!("crashed: {}", panic_text(&*p)))));

        let mut st = shared.lock();
        let cleared = st.current.take().is_none_or(|c| c.stamp.is_empty());
        match result {
            Ok(analysis) => {
                st.done += 1;
                st.audio_secs += analysis.duration_secs;
                st.wall_secs += started.elapsed().as_secs_f64();
                drop(st);
                let record = Record { stamp: job.stamp.clone(), analysis };
                if let Err(e) = cfg.store.put(&job.track_id, &record.encode()) {
                    tracing::warn!("storing analysis: {e}");
                }
                (cfg.on_result)(job.track_id, record);
            }
            Err(analysis::AnalyzeError::Cancelled) => {
                // Paused (or quitting): try again first when resumed, unless cleared.
                if !cleared && !shared.quit.load(Ordering::Relaxed) && st.queued.insert(job.track_id.clone()) {
                    st.push_front(job);
                }
            }
            Err(analysis::AnalyzeError::Failed(e)) => {
                tracing::warn!("analysing {}: {e}", job.track_id);
                st.done += 1;
                st.failed += 1;
            }
        }
        (cfg.on_progress)();
    }
}

/// The message a panic carried.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

/// Reads the job's downloaded copy, if it has one that opens.
fn analyze_local(job: &Job, stop: &(dyn Fn() -> bool + Sync)) -> Option<Result<TrackAnalysis, analysis::AnalyzeError>> {
    let file = std::fs::File::open(job.local.as_ref()?).ok()?;
    let len = file.metadata().ok().map(|m| m.len());
    Some(analysis::analyze_reader(file, len, job.suffix.as_deref(), stop))
}

/// Analyses `job`: its downloaded copy if it has one, else fetched from
/// `url()` (`None`: signed out).
fn analyze_job(job: &Job, url: impl FnOnce() -> Option<String>, cfg: &Config, stop: &(dyn Fn() -> bool + Sync)) -> Result<TrackAnalysis, analysis::AnalyzeError> {
    #[cfg(test)]
    if job.track_id == "test-panic" {
        panic!("odd file");
    }
    // A downloaded copy first: no network, no cache churn.
    if let Some(out) = analyze_local(job, stop) {
        return out;
    }
    let Some(url) = url() else { return Err(analysis::AnalyzeError::Failed("not signed in".into())) };
    let key = CacheKey::original(job.track_id.clone(), job.suffix.clone());
    let cached = cfg.playback.progress(&key).is_some_and(|p| p.complete);
    let cache = if cached { &cfg.playback } else { &cfg.scratch };
    // A download of our own gives way as soon as playback wants the network;
    // the job goes back to the front of the queue and resumes after.
    let stop = || stop() || (!cached && playback_busy(&cfg.status));
    // The reader polls this while waiting on the download; `stop` borrows, so
    // hand it a flag that the loop below keeps current.
    let halt = Arc::new(AtomicBool::new(false));
    let abort: streamcache::Abort = {
        let halt = halt.clone();
        Arc::new(move || halt.load(Ordering::Relaxed))
    };
    let check = || {
        let s = stop();
        if s {
            halt.store(true, Ordering::Relaxed);
        }
        s
    };
    // Opening blocks until the server answers, and reads block on the download:
    // a watcher turns a stop into the flag both of them poll. It looks every
    // 100 ms (playback needing the network has no signal of its own) and is
    // woken at once when the job ends, so the next one needn't wait for it.
    let done = (Mutex::new(false), Condvar::new());
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut finished = done.0.lock().unwrap_or_else(|e| e.into_inner());
            while !*finished && !check() {
                finished = done.1.wait_timeout(finished, Duration::from_millis(100)).unwrap_or_else(|e| e.into_inner()).0;
            }
        });
        let out = (|| {
            let source = cache.open(&key, &url, abort.clone()).map_err(|e| {
                if halt.load(Ordering::Relaxed) {
                    analysis::AnalyzeError::Cancelled
                } else {
                    analysis::AnalyzeError::Failed(format!("fetch: {e}"))
                }
            })?;
            let len = source.byte_len();
            analysis::analyze_reader(source, len, job.suffix.as_deref(), &|| halt.load(Ordering::Relaxed))
        })();
        *done.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
        done.1.notify_all();
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str, secs: f64) -> Job {
        Job { track_id: id.into(), suffix: None, stamp: "s".into(), secs, local: None }
    }

    #[test]
    fn downloaded_copies_are_read_from_disk() {
        // One second of a 1 kHz sine, 16-bit mono WAV at 44.1 kHz.
        let rate = 44_100u32;
        let samples: Vec<i16> = (0..rate).map(|i| ((i as f32 * std::f32::consts::TAU * 1000.0 / rate as f32).sin() * 8000.0) as i16).collect();
        let mut wav = Vec::new();
        let data_len = (samples.len() * 2) as u32;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for s in &samples {
            wav.extend_from_slice(&s.to_le_bytes());
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.wav");
        std::fs::write(&path, wav).unwrap();
        let mut j = job("t", 1.0);
        j.suffix = Some("wav".into());
        // No URL at all: only the local file can be used.
        assert!(analyze_local(&j, &|| false).is_none(), "no local copy: falls through to fetching");
        j.local = Some(path);
        let out = analyze_local(&j, &|| false).expect("local copy is used").expect("decodes");
        assert_eq!(out.sample_rate, 44_100);
        assert!(out.true_peak_dbtp.is_some_and(f64::is_finite));
    }

    #[test]
    fn progress_counts_queue_and_estimates_time() {
        let mut st = State::default();
        st.push_back(job("a", 100.0));
        st.push_back(job("b", 300.0));
        st.done = 2;
        let p = st.progress();
        assert_eq!((p.done, p.total), (2, 4));
        assert!(p.active());
        assert_eq!(p.remaining_secs, 400.0);
        assert!((p.eta_secs - 400.0 / ASSUMED_SPEED).abs() < 1e-9);
        // Measured throughput replaces the assumption.
        st.audio_secs = 600.0;
        st.wall_secs = 10.0;
        assert!((st.progress().eta_secs - 400.0 / 60.0).abs() < 1e-9);
        // Kept as jobs come and go: the running one still counts.
        st.current = st.pop_front();
        assert_eq!(st.progress().remaining_secs, 400.0);
        st.current = None;
        assert_eq!(st.progress().remaining_secs, 300.0);
        st.push_front(job("a", 100.0));
        assert_eq!(st.progress().remaining_secs, 400.0);
        while st.pop_front().is_some() {}
        assert_eq!(st.progress().remaining_secs, 0.0);
        assert!(!st.progress().active());
    }

    /// A runner with no worker: the queue only changes when the test changes it.
    fn idle_runner() -> Runner {
        Runner { shared: Arc::new(Shared { state: Mutex::default(), wake: Condvar::new(), quit: AtomicBool::new(false) }) }
    }

    #[test]
    fn a_cleared_job_can_be_queued_again_while_it_stops() {
        let runner = idle_runner();
        runner.enqueue([job("a", 10.0), job("b", 20.0)]);
        // The worker takes a.
        {
            let mut st = runner.shared.lock();
            st.current = st.pop_front();
        }
        assert_eq!(runner.enqueue([job("a", 10.0)]), 0, "already running");
        runner.clear();
        assert_eq!(runner.progress().remaining_secs, 10.0, "only the one still stopping");
        // Queued again before the worker noticed: it isn't lost.
        assert_eq!(runner.enqueue([job("a", 10.0), job("b", 20.0)]), 2);
        assert_eq!(runner.pending(["a", "b"]), 2);
        assert_eq!(runner.progress().remaining_secs, 40.0);
    }

    #[test]
    fn a_download_of_our_own_gives_way_to_playback() {
        use std::io::{BufRead, BufReader, Write};
        // A slow server: 2 MB at 16 KB every 20 ms, about 2.5 s.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/rest/stream?id=t", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { break };
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(conn.try_clone().unwrap());
                    let mut line = String::new();
                    while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
                        line.clear();
                    }
                    let body = vec![1u8; 2_000_000];
                    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    if conn.write_all(head.as_bytes()).is_err() {
                        return;
                    }
                    for chunk in body.chunks(16 * 1024) {
                        if conn.write_all(chunk).is_err() {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                });
            }
        });
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cache = |name: &str| StreamCache::new(streamcache::CacheConfig::new(dir.path().join(name)), rt.handle().clone()).unwrap();
        let status = Arc::new(Status::default());
        let cfg = Config {
            store: Arc::new(AnalysisStore::in_memory().unwrap()),
            playback: cache("play"),
            scratch: cache("scratch"),
            status: status.clone(),
            on_result: Arc::new(|_, _| {}),
            on_progress: Arc::new(|| {}),
        };
        let j = job("t", 10.0);
        // Playback starts buffering while our download runs.
        let flip = status.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flip.preview(audio::PlayState::Buffering, 0.0, 100.0, 0.0);
        });
        let t = Instant::now();
        let out = analyze_job(&j, || Some(url), &cfg, &|| false);
        assert!(matches!(out, Err(analysis::AnalyzeError::Cancelled)), "{out:?}");
        assert!(t.elapsed() < Duration::from_secs(2), "gave way after {:?}", t.elapsed());
    }

    #[test]
    fn a_panicking_job_fails_and_the_worker_carries_on() {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cache = |name: &str| StreamCache::new(streamcache::CacheConfig::new(dir.path().join(name)), rt.handle().clone()).unwrap();
        let runner = Runner::start(Config {
            store: Arc::new(AnalysisStore::in_memory().unwrap()),
            playback: cache("play"),
            scratch: cache("scratch"),
            status: Arc::new(Status::default()),
            on_result: Arc::new(|_, _| {}),
            on_progress: Arc::new(|| {}),
        });
        // The second job has nothing to read: it fails normally, which only
        // happens if the worker survived the first.
        runner.enqueue([job("test-panic", 1.0), job("after", 1.0)]);
        // The second job's failed fetch takes ~4 s on its own; leave room for a
        // busy machine running the whole suite.
        let deadline = Instant::now() + Duration::from_secs(20);
        while runner.progress().done < 2 {
            assert!(Instant::now() < deadline, "worker stalled: {:?}", runner.progress());
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(runner.progress().failed, 2);
    }
}
