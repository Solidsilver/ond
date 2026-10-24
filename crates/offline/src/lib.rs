//! Offline downloads: albums, playlists and tracks the user pinned, kept as
//! original files in a directory that's never evicted.
//!
//! [`OfflineStore`] keeps the pins (see [`pins`]) in `offline.json` next to the
//! files and runs a small download queue on the tokio runtime: two at a time,
//! resumed with `Range` after an interruption or a restart, retried with
//! backoff, and checked against the size the server reported. A track that's
//! already complete in the stream cache is hard-linked (or copied) instead of
//! downloaded again.
//!
//! Files are named by [`streamcache::file_name`], exactly as in the stream
//! cache, with `.part` appended while downloading. The engine plays a finished one as a
//! `file://` source; see [`OfflineStore::local_path`].

pub mod pins;

pub use pins::{Pin, PinRecord, Pins, TrackRef};

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};
use streamcache::fetch::{self, FetchError};
use tokio::sync::Notify;

const STATE_FILE: &str = "offline.json";
/// Downloads running at once.
const CONCURRENCY: usize = 2;
/// Failed attempts before a track waits for a manual retry or a reconnect.
const MAX_ATTEMPTS: u32 = 6;
/// Progress repaints at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(150);

/// Makes the stream URL for a track id. The store holds one only while the
/// server is reachable (see [`OfflineStore::set_source`]).
pub type UrlFn = Arc<dyn Fn(&str) -> String + Send + Sync>;
/// Finds a complete copy of a track in the stream cache.
pub type CacheLookup = Arc<dyn Fn(&TrackRef) -> Option<PathBuf> + Send + Sync>;
/// Called (from any thread) whenever the snapshot changed.
pub type OnChange = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone)]
pub struct OfflineConfig {
    pub dir: PathBuf,
    /// Cap on finished downloads; `None` is unlimited.
    pub limit: Option<u64>,
}

/// One track's download, as the UI shows it.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackState {
    Queued,
    Downloading { done: u64, total: Option<u64> },
    Done { size: u64 },
    /// Gave up after several attempts; retried on reconnect or by hand.
    Failed { error: String },
}

/// Progress of one pin.
#[derive(Debug, Clone, PartialEq)]
pub struct PinSummary {
    pub pin: Pin,
    pub label: String,
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    /// Bytes on disk, including partial downloads.
    pub bytes_done: u64,
    /// Expected bytes, where the server said.
    pub bytes_total: u64,
    pub downloading: bool,
}

impl PinSummary {
    pub fn complete(&self) -> bool {
        self.done == self.total
    }

    /// 0..=1, by bytes when sizes are known, else by tracks.
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            return 1.0;
        }
        if self.bytes_total > 0 {
            (self.bytes_done as f64 / self.bytes_total as f64).clamp(0.0, 1.0) as f32
        } else {
            self.done as f32 / self.total as f32
        }
    }
}

/// Everything the UI needs, built once per change.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub version: u64,
    pub tracks: HashMap<String, TrackState>,
    pub pins: Vec<PinSummary>,
    /// Bytes of finished downloads.
    pub used_bytes: u64,
    /// Free space on the volume holding the downloads.
    pub free_bytes: Option<u64>,
    pub limit: Option<u64>,
    /// Some downloads wait because the limit is reached.
    pub limit_reached: bool,
    pub paused: bool,
    /// A stream URL can be made (the server is reachable).
    pub online: bool,
    pub queued: usize,
    pub active: usize,
    pub failed: usize,
}

impl Snapshot {
    pub fn pin(&self, pin: &Pin) -> Option<&PinSummary> {
        self.pins.iter().find(|p| &p.pin == pin)
    }

    pub fn track(&self, id: &str) -> Option<&TrackState> {
        self.tracks.get(id)
    }

    pub fn is_done(&self, id: &str) -> bool {
        matches!(self.tracks.get(id), Some(TrackState::Done { .. }))
    }

    /// Work left: queued, downloading or failed.
    pub fn busy(&self) -> bool {
        self.queued + self.active + self.failed > 0
    }
}

#[derive(Clone)]
pub struct OfflineStore {
    inner: Arc<Inner>,
}

impl Drop for OfflineStore {
    /// Waits for queued saves to reach the disk, so reopening (or moving) the
    /// store right after sees them.
    fn drop(&mut self) {
        self.inner.flush.wait_for(self.inner.flush.requested.load(Ordering::Acquire), Duration::from_secs(5));
    }
}

impl std::fmt::Debug for OfflineStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OfflineStore").field("dir", &self.inner.dir).finish_non_exhaustive()
    }
}

struct Inner {
    dir: PathBuf,
    http: reqwest::Client,
    state: Mutex<State>,
    wake: Notify,
    on_change: OnChange,
    /// Track ids whose files should go, deleted on a thread of their own so an
    /// unpin never waits on the disk (or holds the state lock while it does).
    doomed: Mutex<Sender<Vec<String>>>,
    /// Pins to write, saved on a thread of their own for the same reason.
    saves: Mutex<Sender<Save>>,
    flush: Arc<Flush>,
}

/// A copy of the pins to write, taken under the state lock and written after it.
struct Save {
    revision: u64,
    bytes: Vec<u8>,
}

/// How far the saver thread got, for waiting on it.
#[derive(Default)]
struct Flush {
    /// The newest revision handed to the saver.
    requested: AtomicU64,
    /// The newest revision written (or given up on).
    done: Mutex<u64>,
    cond: Condvar,
}

impl Flush {
    fn wait_for(&self, revision: u64, timeout: Duration) {
        let done = self.done.lock().unwrap_or_else(|e| e.into_inner());
        let _ = self.cond.wait_timeout_while(done, timeout, |d| *d < revision);
    }

    fn mark(&self, revision: u64) {
        let mut done = self.done.lock().unwrap_or_else(|e| e.into_inner());
        *done = (*done).max(revision);
        self.cond.notify_all();
    }
}

struct State {
    pins: Pins,
    url: Option<UrlFn>,
    cache: Option<CacheLookup>,
    limit: Option<u64>,
    active: HashMap<String, Active>,
    failures: HashMap<String, Failure>,
    limit_reached: bool,
    version: u64,
    snapshot: Arc<Snapshot>,
    free: Option<(Instant, Option<u64>)>,
    /// Bumped by every change to `pins` that needs saving.
    pins_revision: u64,
}

struct Active {
    cancel: Arc<AtomicBool>,
    done: u64,
    total: Option<u64>,
}

struct Failure {
    attempts: u32,
    retry_at: Instant,
    error: String,
}

struct Job {
    r: TrackRef,
    url: Option<String>,
    from_cache: Option<PathBuf>,
    part: PathBuf,
    dest: PathBuf,
    cancel: Arc<AtomicBool>,
}

#[derive(Debug)]
enum JobError {
    Cancelled,
    Failed(String),
}

impl From<io::Error> for JobError {
    fn from(e: io::Error) -> Self {
        Self::Failed(e.to_string())
    }
}

impl OfflineStore {
    /// Loads the pins, checks the files are still there, removes strays and
    /// starts the download workers on `rt`.
    pub fn open(cfg: OfflineConfig, rt: tokio::runtime::Handle, on_change: OnChange) -> io::Result<Self> {
        std::fs::create_dir_all(&cfg.dir)?;
        let mut pins: Pins = match std::fs::read(cfg.dir.join(STATE_FILE)) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!("offline state unreadable, starting empty: {e}");
                Pins::default()
            }),
            Err(_) => Pins::default(),
        };
        // A finished file that's gone (deleted by hand) downloads again.
        for s in pins.tracks.values_mut() {
            if s.done.is_some() && !cfg.dir.join(file_name(&s.r)).is_file() {
                s.done = None;
            }
        }
        remove_strays(&cfg.dir, &pins);
        let (doomed, doomed_rx) = std::sync::mpsc::channel();
        let (saves, saves_rx) = std::sync::mpsc::channel();
        let flush = Arc::new(Flush::default());
        let inner = Arc::new(Inner {
            doomed: Mutex::new(doomed),
            saves: Mutex::new(saves),
            flush: flush.clone(),
            // Offline downloads aren't waited on by playback: a slow server gets longer.
            http: fetch::client(Duration::from_secs(60)).map_err(io::Error::other)?,
            dir: cfg.dir,
            wake: Notify::new(),
            on_change,
            state: Mutex::new(State {
                pins,
                url: None,
                cache: None,
                limit: cfg.limit,
                active: HashMap::new(),
                failures: HashMap::new(),
                limit_reached: false,
                version: 1,
                snapshot: Arc::new(Snapshot::default()),
                free: None,
                pins_revision: 0,
            }),
        });
        {
            let mut st = inner.lock();
            let snap = inner.build_snapshot(&mut st);
            st.snapshot = Arc::new(snap);
        }
        for _ in 0..CONCURRENCY {
            let inner = inner.clone();
            rt.spawn(async move { worker(inner).await });
        }
        let (weak, dir) = (Arc::downgrade(&inner), inner.dir.clone());
        std::thread::Builder::new()
            .name("offline-delete".into())
            .spawn(move || deleter(weak, dir, doomed_rx))
            .map_err(io::Error::other)?;
        let dir = inner.dir.clone();
        std::thread::Builder::new()
            .name("offline-save".into())
            .spawn(move || saver(dir, saves_rx, flush))
            .map_err(io::Error::other)?;
        Ok(Self { inner })
    }

    pub fn dir(&self) -> &Path {
        &self.inner.dir
    }

    /// Where stream URLs come from. `None` while the server is unreachable;
    /// setting one (a reconnect) also retries failed downloads.
    pub fn set_source(&self, url: Option<UrlFn>) {
        let mut st = self.inner.lock();
        if url.is_some() {
            st.failures.clear();
        }
        st.url = url;
        self.inner.changed(st);
    }

    /// How to find complete copies in the stream cache.
    pub fn set_cache_lookup(&self, lookup: CacheLookup) {
        let mut st = self.inner.lock();
        st.cache = Some(lookup);
        self.inner.changed(st);
    }

    /// Pins (or re-pins with a new track list) and queues what's missing.
    pub fn pin(&self, pin: Pin, label: impl Into<String>, tracks: Vec<TrackRef>) {
        self.pin_many(vec![(pin, label.into(), tracks)]);
    }

    /// [`pin`](Self::pin) for many at once (say, every song in a selection),
    /// with one save instead of one per pin.
    pub fn pin_many(&self, pins: Vec<(Pin, String, Vec<TrackRef>)>) {
        if pins.is_empty() {
            return;
        }
        let mut st = self.inner.lock();
        let gone = st.pins.set_many(pins);
        self.inner.commit(st, gone);
    }

    /// Refreshes a pin's track list (the album or playlist changed on the
    /// server). Does nothing if it isn't pinned.
    pub fn update(&self, pin: &Pin, label: impl Into<String>, tracks: Vec<TrackRef>) {
        let mut st = self.inner.lock();
        let Some(rec) = st.pins.record(pin) else { return };
        let same = rec.tracks.len() == tracks.len() && rec.tracks.iter().zip(&tracks).all(|(a, b)| a == &b.id);
        let label = label.into();
        if same && rec.label == label {
            return;
        }
        let gone = st.pins.set(pin.clone(), label, tracks);
        self.inner.commit(st, gone);
    }

    /// Adds tracks to a pin without dropping any: for track lists that may be
    /// partial (the local library mid-sync), which can grow a pin but must never
    /// shrink it and delete downloads. Does nothing if it isn't pinned.
    pub fn extend(&self, pin: &Pin, tracks: Vec<TrackRef>) {
        let mut st = self.inner.lock();
        let Some(rec) = st.pins.record(pin) else { return };
        let have: HashSet<&String> = rec.tracks.iter().collect();
        let added: Vec<TrackRef> = tracks.into_iter().filter(|t| !have.contains(&t.id)).collect();
        if added.is_empty() {
            return;
        }
        let label = rec.label.clone();
        let mut all: Vec<TrackRef> = rec.tracks.iter().filter_map(|id| st.pins.tracks.get(id).map(|s| s.r.clone())).collect();
        all.extend(added);
        let gone = st.pins.set(pin.clone(), label, all);
        debug_assert!(gone.is_empty());
        self.inner.commit(st, gone);
    }

    /// Unpins; files no other pin needs are deleted (in the background).
    pub fn unpin(&self, pin: &Pin) {
        self.unpin_many(std::slice::from_ref(pin));
    }

    /// [`unpin`](Self::unpin) for many at once, with one save.
    pub fn unpin_many(&self, pins: &[Pin]) {
        if pins.is_empty() {
            return;
        }
        let mut st = self.inner.lock();
        let gone = st.pins.remove_many(pins);
        self.inner.commit(st, gone);
    }

    pub fn is_pinned(&self, pin: &Pin) -> bool {
        self.inner.lock().pins.is_pinned(pin)
    }

    /// The pin's label and track ids.
    pub fn pin_tracks(&self, pin: &Pin) -> Option<(String, Vec<String>)> {
        self.inner.lock().pins.record(pin).map(|r| (r.label.clone(), r.tracks.clone()))
    }

    /// The finished file for track `id`, if there is one.
    pub fn local_path(&self, id: &str) -> Option<PathBuf> {
        let st = self.inner.lock();
        let s = st.pins.tracks.get(id)?;
        s.done?;
        Some(self.inner.dir.join(file_name(&s.r)))
    }

    /// Stops downloading (partial files are kept and resumed later), or resumes.
    pub fn set_paused(&self, paused: bool) {
        let mut st = self.inner.lock();
        if st.pins.paused == paused {
            return;
        }
        st.pins.paused = paused;
        if paused {
            for a in st.active.values() {
                a.cancel.store(true, Ordering::Relaxed);
            }
        }
        self.inner.commit(st, Vec::new());
    }

    /// Tries failed downloads again now.
    pub fn retry_failed(&self) {
        let mut st = self.inner.lock();
        st.failures.clear();
        self.inner.changed(st);
    }

    pub fn set_limit(&self, limit: Option<u64>) {
        let mut st = self.inner.lock();
        st.limit = limit;
        self.inner.changed(st);
    }

    /// The current state; cheap to call every frame.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        let mut st = self.inner.lock();
        if st.snapshot.version != st.version {
            let snap = self.inner.build_snapshot(&mut st);
            st.snapshot = Arc::new(snap);
        }
        st.snapshot.clone()
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Bumps the version, wakes the workers and tells the UI.
    fn changed(&self, mut st: MutexGuard<'_, State>) {
        st.version += 1;
        drop(st);
        self.wake.notify_waiters();
        (self.on_change)();
    }

    /// After a change to the pins: forgets `gone` tracks, then (with the state
    /// lock released) hands the pins to the saver and the files to the deleter.
    /// Neither touches the disk on the calling thread.
    fn commit(&self, mut st: MutexGuard<'_, State>, gone: Vec<String>) {
        let doomed = self.forget(&mut st, gone);
        let save = self.save(&mut st);
        if let Some(save) = &save {
            // Recorded before the change is visible, so whoever sees it and then
            // waits on the file (dropping the store, see `Drop`) waits for this.
            self.flush.requested.fetch_max(save.revision, Ordering::AcqRel);
        }
        self.changed(st);
        if let Some(save) = save {
            let revision = save.revision;
            if self.saves.lock().unwrap_or_else(|e| e.into_inner()).send(save).is_err() {
                self.flush.mark(revision);
            }
        }
        self.delete(doomed);
    }

    /// Tracks no pin needs: returns the ids whose files should go. A running
    /// download of one is cancelled instead and cleans up after itself.
    fn forget(&self, st: &mut State, gone: Vec<String>) -> Vec<String> {
        let mut doomed = Vec::new();
        for id in gone {
            st.failures.remove(&id);
            if let Some(a) = st.active.get(&id) {
                a.cancel.store(true, Ordering::Relaxed);
                continue;
            }
            doomed.push(id);
        }
        doomed
    }

    fn delete(&self, ids: Vec<String>) {
        if !ids.is_empty() {
            let _ = self.doomed.lock().unwrap_or_else(|e| e.into_inner()).send(ids);
        }
    }

    /// The pins as they are now, to [`write`](Self::write) once the lock is released.
    fn save(&self, st: &mut State) -> Option<Save> {
        st.pins_revision += 1;
        match serde_json::to_vec(&st.pins) {
            Ok(bytes) => Some(Save { revision: st.pins_revision, bytes }),
            Err(e) => {
                tracing::warn!("saving offline state: {e}");
                None
            }
        }
    }

    /// Picks the next track to fetch and marks it active. Also returns when the
    /// earliest backoff ends, for the idle wait.
    fn claim(&self) -> (Option<Job>, Option<Instant>) {
        let mut st = self.lock();
        if st.pins.paused {
            return (None, None);
        }
        let now = Instant::now();
        let mut due: Option<Instant> = None;
        let mut blocked_by_limit = false;
        let used = st.pins.used() + st.active.values().map(|a| a.total.unwrap_or(0)).sum::<u64>();
        let mut pick = None;
        for id in st.pins.pending() {
            if st.active.contains_key(id) {
                continue;
            }
            if let Some(f) = st.failures.get(id) {
                if f.attempts >= MAX_ATTEMPTS {
                    continue;
                }
                if f.retry_at > now {
                    due = Some(due.map_or(f.retry_at, |d| d.min(f.retry_at)));
                    continue;
                }
            }
            let r = st.pins.tracks[id].r.clone();
            if let Some(limit) = st.limit
                && used + r.size.unwrap_or(0) > limit
            {
                blocked_by_limit = true;
                continue;
            }
            let from_cache = st.cache.as_ref().and_then(|c| c(&r)).filter(|p| p.is_file());
            let url = st.url.as_ref().map(|u| u(id));
            if from_cache.is_none() && url.is_none() {
                continue;
            }
            pick = Some((r, url, from_cache));
            break;
        }
        if st.limit_reached != blocked_by_limit {
            st.limit_reached = blocked_by_limit;
            st.version += 1;
        }
        let Some((r, url, from_cache)) = pick else { return (None, due) };
        let cancel = Arc::new(AtomicBool::new(false));
        st.active.insert(r.id.clone(), Active { cancel: cancel.clone(), done: 0, total: r.size });
        st.version += 1;
        let name = file_name(&r);
        let job = Job { part: self.dir.join(format!("{name}.part")), dest: self.dir.join(name), r, url, from_cache, cancel };
        drop(st);
        (self.on_change)();
        (Some(job), due)
    }

    fn progress(&self, id: &str, done: u64, total: Option<u64>, note: bool) {
        let mut st = self.lock();
        if let Some(a) = st.active.get_mut(id) {
            a.done = done;
            a.total = total.or(a.total);
        }
        if note {
            st.version += 1;
            drop(st);
            (self.on_change)();
        }
    }

    fn complete(&self, job: Job, result: Result<u64, JobError>) {
        let mut st = self.lock();
        st.active.remove(&job.r.id);
        if !st.pins.tracks.contains_key(&job.r.id) {
            // Unpinned while downloading.
            self.changed(st);
            return self.delete(vec![job.r.id]);
        }
        match result {
            Ok(size) => {
                st.failures.remove(&job.r.id);
                if let Some(s) = st.pins.tracks.get_mut(&job.r.id) {
                    s.done = Some(size);
                }
                return self.commit(st, Vec::new());
            }
            Err(JobError::Cancelled) => {}
            Err(JobError::Failed(error)) => {
                tracing::warn!("offline download {}: {error}", job.r.id);
                let attempts = st.failures.get(&job.r.id).map_or(0, |f| f.attempts) + 1;
                let retry_at = Instant::now() + backoff(attempts);
                st.failures.insert(job.r.id.clone(), Failure { attempts, retry_at, error });
            }
        }
        self.changed(st);
    }

    fn build_snapshot(&self, st: &mut State) -> Snapshot {
        let mut tracks = HashMap::with_capacity(st.pins.tracks.len());
        let (mut queued, mut failed) = (0, 0);
        for (id, s) in &st.pins.tracks {
            let state = if let Some(size) = s.done {
                TrackState::Done { size }
            } else if let Some(a) = st.active.get(id) {
                TrackState::Downloading { done: a.done, total: a.total }
            } else if let Some(f) = st.failures.get(id).filter(|f| f.attempts >= MAX_ATTEMPTS) {
                failed += 1;
                TrackState::Failed { error: f.error.clone() }
            } else {
                queued += 1;
                TrackState::Queued
            };
            tracks.insert(id.clone(), state);
        }
        let pins = st
            .pins
            .pins
            .iter()
            .map(|p| {
                let mut sum = PinSummary {
                    pin: p.pin.clone(),
                    label: p.label.clone(),
                    total: p.tracks.len(),
                    done: 0,
                    failed: 0,
                    bytes_done: 0,
                    bytes_total: 0,
                    downloading: false,
                };
                for id in &p.tracks {
                    let Some(s) = st.pins.tracks.get(id) else { continue };
                    sum.bytes_total += s.done.or(s.r.size).unwrap_or(0);
                    match tracks.get(id) {
                        Some(TrackState::Done { size }) => {
                            sum.done += 1;
                            sum.bytes_done += size;
                        }
                        Some(TrackState::Downloading { done, .. }) => {
                            sum.downloading = true;
                            sum.bytes_done += done;
                        }
                        Some(TrackState::Failed { .. }) => sum.failed += 1,
                        _ => {}
                    }
                }
                sum
            })
            .collect();
        let now = Instant::now();
        let free = match st.free {
            Some((at, free)) if now.duration_since(at) < Duration::from_secs(5) => free,
            _ => {
                let free = free_space(&self.dir);
                st.free = Some((now, free));
                free
            }
        };
        Snapshot {
            version: st.version,
            tracks,
            pins,
            used_bytes: st.pins.used(),
            free_bytes: free,
            limit: st.limit,
            limit_reached: st.limit_reached,
            paused: st.pins.paused,
            online: st.url.is_some(),
            queued,
            active: st.active.len(),
            failed,
        }
    }
}

async fn worker(inner: Arc<Inner>) {
    loop {
        // Registered before looking, so a change in between still wakes us.
        let notified = inner.wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let (job, due) = inner.claim();
        if let Some(job) = job {
            let result = run(&inner, &job).await;
            inner.complete(job, result);
            continue;
        }
        let wait = due.map_or(Duration::from_secs(60), |d| d.saturating_duration_since(Instant::now()));
        tokio::select! {
            _ = &mut notified => {}
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

async fn run(inner: &Inner, job: &Job) -> Result<u64, JobError> {
    if let Some(src) = &job.from_cache {
        // A copy across volumes can take a while: not on the async workers.
        let (src, size, part, dest) = (src.clone(), job.r.size, job.part.clone(), job.dest.clone());
        match blocking(move || promote(&src, size, &part, &dest)).await {
            Ok(size) => return Ok(size),
            Err(e) => tracing::debug!("promote {} from cache: {e}", job.r.id),
        }
    }
    let Some(url) = &job.url else { return Err(JobError::Failed("server unreachable".into())) };
    download(inner, job, url).await
}

/// Runs disk work that may block (copies, fsync, renames) on tokio's blocking pool.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> io::Result<T> + Send + 'static) -> io::Result<T> {
    tokio::task::spawn_blocking(f).await.unwrap_or_else(|e| Err(io::Error::other(e)))
}

/// Takes a complete stream-cache file: a hard link where possible (same
/// volume; eviction there only unlinks its own name), else a copy.
fn promote(src: &Path, size: Option<u64>, part: &Path, dest: &Path) -> io::Result<u64> {
    let len = std::fs::metadata(src)?.len();
    if size.is_some_and(|s| s != len) {
        return Err(io::Error::other("cached file has the wrong size"));
    }
    let _ = std::fs::remove_file(dest);
    if std::fs::hard_link(src, dest).is_err() {
        std::fs::copy(src, part)?;
        std::fs::rename(part, dest)?;
    }
    Ok(len)
}

async fn download(inner: &Inner, job: &Job, url: &str) -> Result<u64, JobError> {
    let mut have = std::fs::metadata(&job.part).map_or(0, |m| m.len());
    if job.r.size.is_some_and(|s| have > s) {
        have = 0;
    }
    let mut body = match fetch::get_from(&inner.http, url, (have > 0).then_some(have)).await {
        Ok(b) => b,
        Err(FetchError::RangeNotSatisfiable) if have > 0 && job.r.size == Some(have) => return finish(job, have).await,
        Err(FetchError::RangeNotSatisfiable) => {
            let _ = std::fs::remove_file(&job.part);
            return Err(JobError::Failed("server rejected the resume; starting over".into()));
        }
        Err(FetchError::BadRange(e)) => {
            // The server answered the resume from somewhere else: the partial
            // file can't be trusted to line up. Start over.
            let _ = std::fs::remove_file(&job.part);
            return Err(JobError::Failed(e));
        }
        Err(e) => return Err(JobError::Failed(e.to_string())),
    };
    if !body.ranged {
        // The server ignored the range (or there was none): start over.
        have = 0;
    }
    let total = body.total;
    let mut file = std::fs::OpenOptions::new().create(true).write(true).append(have > 0).truncate(have == 0).open(&job.part)?;
    let mut last_note = Instant::now();
    inner.progress(&job.r.id, have, total, true);
    loop {
        if job.cancel.load(Ordering::Relaxed) {
            return Err(JobError::Cancelled);
        }
        let chunk = match body.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => {
                let _ = file.flush();
                return Err(JobError::Failed(e.to_string()));
            }
        };
        file.write_all(&chunk)?;
        have += chunk.len() as u64;
        let note = last_note.elapsed() >= PROGRESS_EVERY;
        if note {
            last_note = Instant::now();
        }
        inner.progress(&job.r.id, have, total, note);
    }
    blocking(move || file.sync_all()).await?;
    if let Some(t) = total
        && have != t
    {
        return Err(JobError::Failed(format!("incomplete: {have} of {t} bytes")));
    }
    finish(job, have).await
}

/// Checks the size and moves the finished `.part` into place.
async fn finish(job: &Job, have: u64) -> Result<u64, JobError> {
    if let Some(expected) = job.r.size
        && have != expected
    {
        let _ = std::fs::remove_file(&job.part);
        return Err(JobError::Failed(format!("size mismatch: got {have} bytes, server said {expected}")));
    }
    let (part, dest) = (job.part.clone(), job.dest.clone());
    blocking(move || std::fs::rename(part, dest)).await?;
    Ok(have)
}

fn backoff(attempts: u32) -> Duration {
    fetch::backoff(attempts, Duration::from_secs(4), Duration::from_secs(300))
}

/// The track's file name, the same as in the stream cache.
pub fn file_name(r: &TrackRef) -> String {
    streamcache::file_name(&r.id, r.suffix.as_deref())
}

/// The saver thread: writes the newest pins handed to it, skipping any that a
/// newer one superseded while it was busy, until the store goes away.
fn saver(dir: PathBuf, saves: Receiver<Save>, flush: Arc<Flush>) {
    while let Ok(first) = saves.recv() {
        let save = saves.try_iter().fold(first, |a, b| if b.revision > a.revision { b } else { a });
        let write = || -> io::Result<()> {
            let tmp = dir.join(format!("{STATE_FILE}.tmp"));
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&save.bytes)?;
            f.sync_all()?;
            std::fs::rename(&tmp, dir.join(STATE_FILE))
        };
        if let Err(e) = write() {
            tracing::warn!("saving offline state: {e}");
        }
        flush.mark(save.revision);
    }
}

/// The deleter thread: removes the files of tracks no pin needs any more, one
/// batch at a time, until the store goes away.
fn deleter(store: Weak<Inner>, dir: PathBuf, batches: Receiver<Vec<String>>) {
    while let Ok(mut ids) = batches.recv() {
        // Anything else that piled up goes in the same directory scan.
        ids.extend(batches.try_iter().flatten());
        let Some(inner) = store.upgrade() else { return };
        // Pinned again since (or downloading again): its files stay.
        ids.retain(|id| {
            let st = inner.lock();
            !st.pins.tracks.contains_key(id) && !st.active.contains_key(id)
        });
        drop(inner);
        remove_files(&dir, &ids);
    }
}

/// Deletes the finished and partial files of tracks `ids`, whatever their
/// suffix, in one pass over the directory.
fn remove_files(dir: &Path, ids: &[String]) {
    if ids.is_empty() {
        return;
    }
    let stems: HashSet<String> = ids.iter().map(|id| streamcache::escape_id(id)).collect();
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        // `<stem>`, `<stem>.<suffix>` or `<stem>.<suffix>.part`: escaped ids never contain '.'.
        let stem = name.split('.').next().unwrap_or(name);
        if stems.contains(stem) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Removes files nothing refers to (a crash between unpin and delete).
fn remove_strays(dir: &Path, pins: &Pins) {
    let known: std::collections::HashSet<String> = pins.tracks.values().flat_map(|s| {
        let n = file_name(&s.r);
        [format!("{n}.part"), n]
    }).collect();
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(STATE_FILE) || name.starts_with('.') || !e.path().is_file() {
            continue;
        }
        if !known.contains(name) {
            tracing::info!("offline: removing stray {name}");
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(unix)]
fn free_space(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is a valid C string and `s` a valid out pointer.
    if unsafe { libc::statvfs(path.as_ptr(), &mut s) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)]
    Some(s.f_bavail as u64 * s.f_frsize as u64)
}

#[cfg(windows)]
fn free_space(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let path: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
    let mut available = 0u64;
    // SAFETY: `path` is NUL-terminated and `available` a valid out pointer;
    // the totals we don't need may be null.
    let ok = unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut available, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(available)
}

/// Moves a store's files from `from` to `to` (run before opening it at `to`),
/// including the per-account stores in folders under it. Renames where it can,
/// copies across volumes. Files already at `to` win.
pub fn move_store(from: &Path, to: &Path) -> io::Result<()> {
    if from == to || !from.is_dir() {
        return Ok(());
    }
    move_files(from, to, true)
}

/// The folder for `account`'s downloads under the downloads folder `base`.
/// Each server account keeps its own pins and files, so signing in somewhere
/// else never fetches the old pins with the new server's URLs, and switching
/// back finds them again. A store kept directly in `base` by an older version
/// becomes this account's the first time.
pub fn account_store(base: &Path, account: u32) -> io::Result<PathBuf> {
    let dir = base.join(format!("{account:08x}"));
    if base.join(STATE_FILE).is_file() && !dir.join(STATE_FILE).exists() {
        move_files(base, &dir, false)?;
    }
    Ok(dir)
}

fn move_files(from: &Path, to: &Path, folders: bool) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let kind = e.file_type()?;
        if kind.is_dir() {
            if folders {
                move_files(&e.path(), &to.join(e.file_name()), true)?;
            }
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        let dest = to.join(e.file_name());
        if dest.exists() {
            continue;
        }
        if std::fs::rename(e.path(), &dest).is_err() {
            let tmp = to.join(format!(".{}.moving", e.file_name().to_string_lossy()));
            std::fs::copy(e.path(), &tmp)?;
            std::fs::rename(&tmp, &dest)?;
            std::fs::remove_file(e.path())?;
        }
    }
    // Leaves `from` behind only if something else is in it.
    let _ = std::fs::remove_dir(from);
    Ok(())
}

#[cfg(test)]
mod tests;
