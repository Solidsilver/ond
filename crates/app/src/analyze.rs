//! Library analysis in the background: one track at a time on a low-priority
//! thread, out of the way of playback. Files already in the playback cache are
//! read from there; others are fetched into a small separate cache so a
//! library-wide run doesn't push recently played tracks out.
//!
//! Results go into the library db (keyed by track id, with a stamp of the
//! file) and to the UI. A pause, or quitting, stops the track in progress;
//! it goes back to the front of the queue.

use analysis::{Record, TrackAnalysis};
use audio::{PlayState, Status};
use library::{Library, Track};
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use streamcache::{CacheKey, StreamCache};

/// Audio seconds analysed per wall-clock second, assumed until measured.
const ASSUMED_SPEED: f64 = 20.0;

/// One track to analyse.
#[derive(Debug, Clone)]
pub struct Job {
    pub track_id: String,
    pub url: String,
    pub suffix: Option<String>,
    pub stamp: String,
    pub secs: f64,
}

/// The stamp a track's stored analysis must carry to still be current.
pub fn stamp_of(t: &Track) -> String {
    analysis::stamp(t.size, t.suffix.as_deref(), t.bit_depth, t.sample_rate)
}

pub fn job_for(t: &Track, url: String) -> Job {
    Job {
        track_id: t.id.clone(),
        url,
        suffix: t.suffix.clone(),
        stamp: stamp_of(t),
        secs: f64::from(t.duration_secs.unwrap_or(0)),
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

#[derive(Default)]
struct State {
    queue: VecDeque<Job>,
    queued: HashSet<String>,
    current: Option<Job>,
    paused: bool,
    waiting: bool,
    done: usize,
    failed: usize,
    /// Throughput so far: audio seconds over wall seconds.
    audio_secs: f64,
    wall_secs: f64,
}

impl State {
    fn progress(&self) -> Progress {
        let waiting = self.queue.len() + usize::from(self.current.is_some());
        let remaining_secs = self.queue.iter().chain(&self.current).map(|j| j.secs).sum::<f64>();
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
    pub library: Arc<Library>,
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
            let running = st.current.as_ref().is_some_and(|c| c.track_id == job.track_id);
            if !running && st.queued.insert(job.track_id.clone()) {
                st.queue.push_back(job);
                added += 1;
            }
        }
        drop(st);
        self.shared.wake.notify_all();
        added
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

/// "about 3 h", "about 25 min".
pub fn about(secs: f64) -> String {
    let mins = (secs / 60.0).round();
    if mins < 1.0 {
        "under a minute".into()
    } else if mins < 90.0 {
        format!("about {mins} min")
    } else {
        format!("about {} h", (mins / 60.0).round())
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

/// Background QoS: the scheduler keeps it to spare capacity (and an
/// efficiency core where there is one), and throttles its disk and network.
fn lower_priority() {
    #[cfg(target_os = "macos")]
    // SAFETY: only changes the calling thread's QoS class.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_BACKGROUND, 0);
    }
}

/// Playback is fetching or filling its buffer: leave the network and disk to it.
fn playback_busy(status: &Status) -> bool {
    match status.state() {
        PlayState::Loading | PlayState::Buffering => true,
        PlayState::Playing | PlayState::Paused => {
            let total = status.total_bytes.load(Ordering::Relaxed);
            total > 0 && status.fetched_bytes.load(Ordering::Relaxed) < total
        }
        PlayState::Stopped => false,
    }
}

fn run(shared: &Shared, cfg: &Config) {
    loop {
        // Next job, once not paused and playback doesn't need the bandwidth.
        let job = {
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
            let job = st.queue.pop_front().expect("checked non-empty");
            st.queued.remove(&job.track_id);
            st.current = Some(job.clone());
            job
        };
        (cfg.on_progress)();

        let started = Instant::now();
        let stop = || {
            shared.quit.load(Ordering::Relaxed) || {
                let st = shared.lock();
                st.paused || st.current.as_ref().is_none_or(|c| c.stamp.is_empty())
            }
        };
        let result = analyze_job(&job, cfg, &stop);

        let mut st = shared.lock();
        let cleared = st.current.take().is_none_or(|c| c.stamp.is_empty());
        match result {
            Ok(analysis) => {
                st.done += 1;
                st.audio_secs += analysis.duration_secs;
                st.wall_secs += started.elapsed().as_secs_f64();
                drop(st);
                let record = Record { stamp: job.stamp.clone(), analysis };
                if let Err(e) = cfg.library.put_analysis(&job.track_id, &record.encode()) {
                    tracing::warn!("storing analysis: {e}");
                }
                (cfg.on_result)(job.track_id, record);
            }
            Err(analysis::AnalyzeError::Cancelled) => {
                // Paused (or quitting): try again first when resumed, unless cleared.
                if !cleared && !shared.quit.load(Ordering::Relaxed) && st.queued.insert(job.track_id.clone()) {
                    st.queue.push_front(job);
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

fn analyze_job(job: &Job, cfg: &Config, stop: &(dyn Fn() -> bool + Sync)) -> Result<TrackAnalysis, analysis::AnalyzeError> {
    let key = CacheKey::new(job.track_id.clone(), job.suffix.clone());
    let cached = cfg.playback.progress(&key).is_some_and(|p| p.complete);
    let cache = if cached { &cfg.playback } else { &cfg.scratch };
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
    // a watcher turns a stop into the flag both of them poll.
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Ordering::Relaxed) {
                if check() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        let out = (|| {
            let source = cache.open(&key, &job.url, abort.clone()).map_err(|e| {
                if halt.load(Ordering::Relaxed) {
                    analysis::AnalyzeError::Cancelled
                } else {
                    analysis::AnalyzeError::Failed(format!("fetch: {e}"))
                }
            })?;
            let len = source.byte_len();
            analysis::analyze_reader(source, len, job.suffix.as_deref(), &|| halt.load(Ordering::Relaxed))
        })();
        done.store(true, Ordering::Relaxed);
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str, secs: f64) -> Job {
        Job { track_id: id.into(), url: String::new(), suffix: None, stamp: "s".into(), secs }
    }

    #[test]
    fn progress_counts_queue_and_estimates_time() {
        let mut st = State::default();
        st.queue.extend([job("a", 100.0), job("b", 300.0)]);
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
        st.queue.clear();
        assert!(!st.progress().active());
    }
}
