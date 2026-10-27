//! Offline downloads: albums, playlists and tracks the user pinned, kept as
//! original files in a directory that's never evicted.
//!
//! [`OfflineStore`] keeps the pins (see [`pins`]) in `offline.json` next to the
//! files and runs a small download queue on the tokio runtime: two at a time,
//! resumed with `Range` after an interruption or a restart (when the server's
//! file is provably still the same, see [`RESUME_OVERLAP`]), retried with
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
use streamcache::disk::{PartFile, blocking};
use streamcache::fetch::{self, FetchError};
use tokio::sync::Notify;

const STATE_FILE: &str = "offline.json";
/// A state file that failed to load, kept under this prefix; see [`load_pins`].
const SET_ASIDE: &str = "offline.json.bad-";
/// Downloads running at once.
const CONCURRENCY: usize = 2;
/// Failed attempts before a track waits for a manual retry or a reconnect.
const MAX_ATTEMPTS: u32 = 6;
/// Progress repaints at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(150);
/// A resume asks for this much of what the `.part` already holds again, and
/// appends only if those bytes match: proof that the server's file is still
/// the one the partial download came from.
const RESUME_OVERLAP: u64 = 64 * 1024;

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
    /// A state file that couldn't be read was set aside, so files the pins
    /// don't name are kept (see [`load_pins`]); `None` normally.
    pub set_aside: Option<SetAside>,
}

/// The state file couldn't be read and was set aside: the downloads it named
/// are no longer listed, and their files stay in the folder (no cleanup)
/// until [`OfflineStore::remove_set_aside`] or the user deletes the
/// `offline.json.bad-*` file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SetAside {
    /// The set-aside state files.
    pub files: Vec<PathBuf>,
    /// Files in the folder that the pins don't name.
    pub unknown: usize,
    /// Their bytes.
    pub unknown_bytes: u64,
    /// [`OfflineStore::remove_set_aside`] is under way.
    pub removing: bool,
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

/// A handle to the store; copies share it. The store shuts down when the last
/// copy is dropped (see [`Owner`]).
#[derive(Clone)]
pub struct OfflineStore {
    inner: Arc<Inner>,
    _owner: Arc<Owner>,
}

/// Held only by the [`OfflineStore`] handles, so its drop is the last handle's.
/// The workers hold [`Inner`] but not this.
struct Owner {
    inner: Arc<Inner>,
    workers: Vec<tokio::task::AbortHandle>,
}

impl Drop for Owner {
    /// Stops the store: running downloads stop (their partial files stay for
    /// next time), the workers end, and nothing it does afterwards reaches the
    /// disk, so a store opened on the same folder (switching back to this
    /// account) is the only one writing there. Then waits for saves already
    /// queued to reach the disk, so reopening (or moving) the store right after
    /// sees them.
    fn drop(&mut self) {
        self.inner.close();
        for w in &self.workers {
            w.abort();
        }
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
    doomed: Mutex<Sender<Chore>>,
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
    /// The last handle is gone: nothing more is claimed, saved or deleted.
    closed: bool,
    /// Tracks whose files the deleter is removing right now. One pinned again
    /// meanwhile isn't downloaded until it's done, so the delete can't unlink
    /// the new partial file.
    deleting: HashSet<String>,
    /// A state file was set aside (see [`load_pins`]); `None` normally.
    set_aside: Option<Aside>,
    /// File names [`Inner::sweep`] is deleting right now: a track pinned
    /// again meanwhile whose file has one of them waits until it's done.
    sweeping: HashSet<String>,
}

/// What [`State::set_aside`] keeps.
struct Aside {
    /// The `offline.json.bad-*` files.
    files: Vec<PathBuf>,
    /// The files in the folder the pins didn't name at open, with their
    /// sizes. Those pinned since are left out when counting.
    strays: Vec<(String, u64)>,
    removing: bool,
}

/// Work for the deleter thread.
enum Chore {
    /// Delete these tracks' files.
    Delete(Vec<String>),
    /// [`Inner::sweep`].
    Sweep,
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
        remove_moving(&cfg.dir);
        let (mut pins, trusted) = load_pins(&cfg.dir);
        // A finished file that's gone (deleted by hand) downloads again.
        for s in pins.tracks.values_mut() {
            if s.done.is_some() && !cfg.dir.join(file_name(&s.r)).is_file() {
                s.done = None;
            }
        }
        let set_aside = if trusted {
            remove_strays(&cfg.dir, &pins);
            None
        } else {
            tracing::warn!("offline: keeping files the pins don't know in {}, while a {SET_ASIDE}* file is there", cfg.dir.display());
            Some(Aside { files: set_aside_files(&cfg.dir), strays: strays(&cfg.dir, &pins), removing: false })
        };
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
                closed: false,
                deleting: HashSet::new(),
                set_aside,
                sweeping: HashSet::new(),
            }),
        });
        {
            let mut st = inner.lock();
            let snap = inner.build_snapshot(&mut st);
            st.snapshot = Arc::new(snap);
        }
        let workers = (0..CONCURRENCY)
            .map(|_| {
                let inner = inner.clone();
                rt.spawn(async move { worker(inner).await }).abort_handle()
            })
            .collect();
        // From here on, an early return drops this and stops the workers.
        let owner = Arc::new(Owner { inner: inner.clone(), workers });
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
        Ok(Self { inner, _owner: owner })
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
    ///
    /// A track not downloaded yet also takes the new suffix and size (the
    /// file was replaced on the server), so it isn't checked against a stale
    /// size, and one that failed is tried again now. A download already
    /// running carries on as it was: it records what it got when it ends.
    /// A finished file keeps its name and size.
    pub fn update(&self, pin: &Pin, label: impl Into<String>, tracks: Vec<TrackRef>) {
        let mut st = self.inner.lock();
        let Some(rec) = st.pins.record(pin) else { return };
        let same = rec.tracks.len() == tracks.len() && rec.tracks.iter().zip(&tracks).all(|(a, b)| a == &b.id);
        let label = label.into();
        let refreshed: Vec<String> = tracks
            .iter()
            .filter(|t| st.pins.tracks.get(&t.id).is_some_and(|s| s.done.is_none() && s.r != **t))
            .map(|t| t.id.clone())
            .collect();
        if same && rec.label == label && refreshed.is_empty() {
            return;
        }
        for id in &refreshed {
            if !st.active.contains_key(id) {
                st.failures.remove(id);
            }
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

    /// After a state file was set aside (see [`Snapshot::set_aside`]): deletes
    /// the files in the folder the pins don't name, then the set-aside state
    /// files, which turns cleanup back on. Runs in the background; the
    /// snapshot says when it's done. Does nothing if nothing was set aside.
    pub fn remove_set_aside(&self) {
        let mut st = self.inner.lock();
        let Some(aside) = st.set_aside.as_mut() else { return };
        if aside.removing {
            return;
        }
        aside.removing = true;
        self.inner.changed(st);
        let _ = self.inner.doomed.lock().unwrap_or_else(|e| e.into_inner()).send(Chore::Sweep);
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

    /// Shuts the store down (the last handle went): running downloads are
    /// cancelled and idle workers woken to end. Taken under the state lock, so
    /// a commit is either before it (and its save already counted in
    /// `flush.requested`) or doesn't happen.
    fn close(&self) {
        let mut st = self.lock();
        st.closed = true;
        for a in st.active.values() {
            a.cancel.store(true, Ordering::Relaxed);
        }
        drop(st);
        self.wake.notify_waiters();
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
            let _ = self.doomed.lock().unwrap_or_else(|e| e.into_inner()).send(Chore::Delete(ids));
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

    /// Picks the next track to fetch and marks it active.
    fn claim(&self) -> Next {
        let mut st = self.lock();
        if st.closed {
            return Next::Stop;
        }
        if st.pins.paused {
            return Next::Wait(None);
        }
        let now = Instant::now();
        let mut due: Option<Instant> = None;
        let mut blocked_by_limit = false;
        let used = st.pins.used() + st.active.values().map(|a| a.total.unwrap_or(0)).sum::<u64>();
        let mut pick = None;
        for id in st.pins.pending() {
            if st.active.contains_key(id) || st.deleting.contains(id) {
                continue;
            }
            if !st.sweeping.is_empty() {
                let name = file_name(&st.pins.tracks[id].r);
                if st.sweeping.contains(&name) || st.sweeping.contains(&format!("{name}.part")) {
                    continue;
                }
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
            // Not checked on disk here, under the lock: a cached copy that's
            // gone fails to promote and the job downloads instead (or, with
            // no server, fails and backs off like any download).
            let from_cache = st.cache.as_ref().and_then(|c| c(&r));
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
        let Some((r, url, from_cache)) = pick else { return Next::Wait(due) };
        let cancel = Arc::new(AtomicBool::new(false));
        st.active.insert(r.id.clone(), Active { cancel: cancel.clone(), done: 0, total: r.size });
        st.version += 1;
        let name = file_name(&r);
        let job = Job { part: self.dir.join(format!("{name}.part")), dest: self.dir.join(name), r, url, from_cache, cancel };
        drop(st);
        (self.on_change)();
        Next::Job(job)
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
        if st.closed {
            // Whatever it got to stays on disk for the next store here to
            // find; this one records nothing more.
            return;
        }
        if !st.pins.tracks.contains_key(&job.r.id) {
            // Unpinned while downloading.
            self.changed(st);
            return self.delete(vec![job.r.id]);
        }
        match result {
            Ok(size) => {
                st.failures.remove(&job.r.id);
                if let Some(s) = st.pins.tracks.get_mut(&job.r.id) {
                    // What was downloaded, under the name it got: a re-pin since
                    // may have brought another suffix or size.
                    s.r = job.r.clone();
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
        let set_aside = st.set_aside.as_ref().map(|a| {
            // Rare and short-lived, so the names are worked out each time.
            let known = known_names(&st.pins);
            let unknown: Vec<u64> = a.strays.iter().filter(|(n, _)| !known.contains(n)).map(|&(_, len)| len).collect();
            SetAside { files: a.files.clone(), unknown: unknown.len(), unknown_bytes: unknown.iter().sum(), removing: a.removing }
        });
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
            set_aside,
        }
    }
}

/// What a worker does next.
enum Next {
    Job(Job),
    /// Nothing to do until woken, or until a backoff ends (when there's one).
    Wait(Option<Instant>),
    /// The store shut down.
    Stop,
}

/// Downloads one track after another until the store shuts down (and is also
/// aborted then, so a download in flight stops at once).
async fn worker(inner: Arc<Inner>) {
    loop {
        // Registered before looking, so a change in between still wakes us.
        let notified = inner.wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let due = match inner.claim() {
            Next::Job(job) => {
                let result = run(&inner, &job).await;
                inner.complete(job, result);
                continue;
            }
            Next::Wait(due) => due,
            Next::Stop => return,
        };
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

/// Deletes `path` (if it's there) on the blocking pool.
async fn remove_file(path: &Path) {
    let path = path.to_owned();
    let _ = blocking(move || std::fs::remove_file(path)).await;
}

/// Takes a complete stream-cache file: a hard link where possible (same
/// volume; eviction there only unlinks its own name), else a copy.
fn promote(src: &Path, size: Option<u64>, part: &Path, dest: &Path) -> io::Result<u64> {
    let len = std::fs::metadata(src)?.len();
    if size.is_some_and(|s| s != len) {
        return Err(io::Error::other("cached file has the wrong size"));
    }
    let _ = std::fs::remove_file(dest);
    if std::fs::hard_link(src, dest).is_ok() {
        // A download started earlier isn't needed now.
        let _ = std::fs::remove_file(part);
    } else {
        std::fs::copy(src, part)?;
        std::fs::rename(part, dest)?;
    }
    Ok(len)
}

/// Every touch of the disk here runs on the blocking pool (see [`blocking`]).
async fn download(inner: &Inner, job: &Job, url: &str) -> Result<u64, JobError> {
    let part = job.part.clone();
    let mut have = blocking(move || std::fs::metadata(part)).await.map_or(0, |m| m.len());
    if job.r.size.is_some_and(|s| have > s) {
        have = 0;
    }
    loop {
        match fetch_into_part(inner, job, url, have).await? {
            Some(have) => return finish(job, have).await,
            // Never twice: from byte 0 there's nothing to match.
            None => {
                tracing::info!("offline: {} changed on the server since its partial download; starting over", job.r.id);
                have = 0;
            }
        }
    }
}

/// Downloads into the `.part`, resuming after its first `have` bytes when the
/// server proves it's the same file: its size matches the pin's, and the last
/// [`RESUME_OVERLAP`] bytes of the `.part`, asked for again, match. Returns
/// the bytes now in the `.part`, or `None` when the resume didn't line up
/// (start over from 0).
async fn fetch_into_part(inner: &Inner, job: &Job, url: &str, mut have: u64) -> Result<Option<u64>, JobError> {
    let overlap = have.min(RESUME_OVERLAP);
    let mut body = match fetch::get_from(&inner.http, url, (have > 0).then(|| have - overlap)).await {
        Ok(b) => b,
        Err(FetchError::RangeNotSatisfiable) if have > 0 => {
            // Even the overlap lies past the end: the file shrank.
            return Ok(None);
        }
        Err(FetchError::RangeNotSatisfiable) => {
            remove_file(&job.part).await;
            return Err(JobError::Failed("server rejected the resume; starting over".into()));
        }
        Err(FetchError::BadRange(e)) => {
            // The server answered the resume from somewhere else: the partial
            // file can't be trusted to line up. Start over.
            remove_file(&job.part).await;
            return Err(JobError::Failed(e));
        }
        Err(e) => return Err(JobError::Failed(e.to_string())),
    };
    // What the resumed body must start with.
    let mut expect: &[u8] = &[];
    let tail;
    if !body.ranged {
        // The server ignored the range (or there was none): start over.
        have = 0;
    } else if have > 0 {
        if job.r.size.is_some_and(|s| body.total != Some(s)) {
            return Ok(None);
        }
        let part = job.part.clone();
        tail = streamcache::disk::blocking(move || read_tail(&part, overlap)).await?;
        expect = &tail;
    }
    let total = body.total;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true).append(have > 0).truncate(have == 0);
    let file = PartFile::open(options, job.part.clone()).await?;
    let mut last_note = Instant::now();
    inner.progress(&job.r.id, have, total, true);
    loop {
        if job.cancel.load(Ordering::Relaxed) {
            return Err(JobError::Cancelled);
        }
        let chunk = match body.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => return Err(JobError::Failed(e.to_string())),
        };
        let mut chunk = chunk;
        if !expect.is_empty() {
            let n = expect.len().min(chunk.len());
            if chunk[..n] != expect[..n] {
                return Ok(None);
            }
            expect = &expect[n..];
            chunk = chunk.slice(n..);
        }
        let n = chunk.len() as u64;
        if n > 0 {
            file.write(chunk).await?;
        }
        have += n;
        let note = last_note.elapsed() >= PROGRESS_EVERY;
        if note {
            last_note = Instant::now();
        }
        inner.progress(&job.r.id, have, total, note);
    }
    file.sync_all().await?;
    if let Some(t) = total
        && have != t
    {
        return Err(JobError::Failed(format!("incomplete: {have} of {t} bytes")));
    }
    if !expect.is_empty() {
        return Err(JobError::Failed("the resume ended before it could be checked".into()));
    }
    Ok(Some(have))
}

/// The last `len` bytes of `path`.
fn read_tail(path: &Path, len: u64) -> io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::End(-(len as i64)))?;
    let mut buf = Vec::with_capacity(len as usize);
    f.take(len).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Checks the size and moves the finished `.part` into place.
async fn finish(job: &Job, have: u64) -> Result<u64, JobError> {
    if let Some(expected) = job.r.size
        && have != expected
    {
        remove_file(&job.part).await;
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
///
/// Saves can arrive out of order (two commits race to hand theirs over once
/// the state lock is released), so one older than a save already seen is
/// dropped rather than written over it.
fn saver(dir: PathBuf, saves: Receiver<Save>, flush: Arc<Flush>) {
    let mut newest = 0;
    while let Ok(first) = saves.recv() {
        let save = saves.try_iter().fold(first, |a, b| if b.revision > a.revision { b } else { a });
        if save.revision < newest {
            continue;
        }
        newest = save.revision;
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
fn deleter(store: Weak<Inner>, dir: PathBuf, chores: Receiver<Chore>) {
    while let Ok(first) = chores.recv() {
        // Anything else that piled up goes in the same directory scan.
        let (mut ids, mut sweep) = (Vec::new(), false);
        for chore in std::iter::once(first).chain(chores.try_iter()) {
            match chore {
                Chore::Delete(batch) => ids.extend(batch),
                Chore::Sweep => sweep = true,
            }
        }
        if !ids.is_empty() {
            let Some(inner) = store.upgrade() else { return };
            if !inner.deleting(&mut ids) {
                return;
            }
            drop(inner);
            remove_files(&dir, &ids);
            let Some(inner) = store.upgrade() else { return };
            inner.deleted(&ids);
        }
        if sweep {
            let Some(inner) = store.upgrade() else { return };
            if !inner.sweep() {
                return;
            }
        }
    }
}

impl Inner {
    /// [`OfflineStore::remove_set_aside`], on the deleter thread: deletes the
    /// files the pins don't name, then the set-aside state files. A track
    /// pinned meanwhile whose file is among them waits until they're gone
    /// (see [`State::sweeping`]). False once the store has shut down.
    fn sweep(&self) -> bool {
        let found = listing(&self.dir);
        let doomed: Vec<String> = {
            let mut st = self.lock();
            if st.closed {
                return false;
            }
            let known = known_names(&st.pins);
            let doomed: Vec<String> = found.into_iter().map(|(name, _)| name).filter(|n| !known.contains(n)).collect();
            st.sweeping = doomed.iter().cloned().collect();
            doomed
        };
        for name in &doomed {
            tracing::info!("offline: removing {name}, which the pins don't name");
            if let Err(e) = std::fs::remove_file(self.dir.join(name))
                && e.kind() != io::ErrorKind::NotFound
            {
                tracing::warn!("offline: removing {name}: {e}");
            }
        }
        // Last, so cleanup only comes back on once the files are gone.
        for f in set_aside_files(&self.dir) {
            if let Err(e) = std::fs::remove_file(&f) {
                tracing::warn!("offline: removing {}: {e}", f.display());
            }
        }
        let left = set_aside_files(&self.dir);
        let strays = if left.is_empty() { Vec::new() } else { listing(&self.dir) };
        let mut st = self.lock();
        st.sweeping.clear();
        st.set_aside = if left.is_empty() {
            None
        } else {
            let known = known_names(&st.pins);
            Some(Aside { files: left, strays: strays.into_iter().filter(|(n, _)| !known.contains(n)).collect(), removing: false })
        };
        let closed = st.closed;
        self.changed(st);
        !closed
    }

    /// Before the deleter removes the files of `ids`: drops those pinned again
    /// since (or downloading again), whose files stay, and holds back new
    /// downloads of the rest until [`deleted`](Self::deleted). False once the
    /// store has shut down: a store opened here since owns the folder, and
    /// what's left over is a stray to it.
    fn deleting(&self, ids: &mut Vec<String>) -> bool {
        let mut st = self.lock();
        if st.closed {
            return false;
        }
        ids.retain(|id| !st.pins.tracks.contains_key(id) && !st.active.contains_key(id));
        st.deleting.extend(ids.iter().cloned());
        true
    }

    /// The files of `ids` are gone: any pinned again meanwhile can download.
    fn deleted(&self, ids: &[String]) {
        let mut st = self.lock();
        for id in ids {
            st.deleting.remove(id);
        }
        if let Some(aside) = st.set_aside.as_mut() {
            // A file that was unknown at open, pinned and then unpinned, is gone now.
            let stems: HashSet<String> = ids.iter().map(|id| streamcache::escape_id(id)).collect();
            aside.strays.retain(|(name, _)| !stems.contains(stem(name)));
        }
        if ids.iter().any(|id| st.pins.tracks.contains_key(id)) {
            self.changed(st);
        }
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
        if stems.contains(stem(name)) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Reads the pins. Also says whether they can be trusted to name every
/// download, so files they don't know may be removed as strays.
///
/// A state file that can't be read or parsed (a torn write, a permissions
/// problem, a format change), or one missing beside downloads, may have named
/// files that are still here. It's moved aside as `offline.json.bad-<ms>` (or a
/// note is left under that name when there's nothing to move), and as long as
/// such a file is in the folder no stray is removed, in this session or any
/// later one: deleting gigabytes of downloads is never the way to recover.
fn load_pins(dir: &Path) -> (Pins, bool) {
    let path = dir.join(STATE_FILE);
    let problem = match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(pins) => return (pins, !has_set_aside(dir)),
            Err(e) => format!("can't be parsed: {e}"),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if !has_downloads(dir) {
                return (Pins::default(), !has_set_aside(dir));
            }
            "is missing, but downloads are here".to_owned()
        }
        Err(e) => format!("can't be read: {e}"),
    };
    let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let aside = dir.join(format!("{SET_ASIDE}{millis}"));
    let moved = path.symlink_metadata().is_ok() && std::fs::rename(&path, &aside).is_ok();
    if !moved {
        let note = format!("{STATE_FILE} {problem}. Files here that the pins don't name are kept until this file is deleted.\n");
        if let Err(e) = std::fs::write(&aside, note) {
            tracing::warn!("offline: noting the lost state at {}: {e}", aside.display());
        }
    }
    tracing::warn!("offline: {} {problem}; starting with no pins and keeping every file (see {})", path.display(), aside.display());
    (Pins::default(), false)
}

/// A state file that failed to load was set aside in `dir`.
fn has_set_aside(dir: &Path) -> bool {
    std::fs::read_dir(dir).map_or(true, |entries| entries.flatten().any(|e| e.file_name().to_string_lossy().starts_with(SET_ASIDE)))
}

/// `dir` holds files other than the store's own bookkeeping.
fn has_downloads(dir: &Path) -> bool {
    std::fs::read_dir(dir).map_or(true, |entries| {
        entries.flatten().any(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            !name.starts_with(STATE_FILE) && !name.starts_with('.') && e.file_type().is_ok_and(|t| t.is_file())
        })
    })
}

/// The set-aside state files in `dir` (see [`load_pins`]), oldest first.
fn set_aside_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(SET_ASIDE)).map(|e| e.path()).collect();
    files.sort();
    files
}

/// The names of every file the pins account for, finished or partial.
fn known_names(pins: &Pins) -> HashSet<String> {
    pins.tracks
        .values()
        .flat_map(|s| {
            let n = file_name(&s.r);
            [format!("{n}.part"), n]
        })
        .collect()
}

/// A download's file name up to the first '.': `<stem>`, `<stem>.<suffix>`
/// or `<stem>.<suffix>.part`, as escaped ids never contain '.'.
fn stem(name: &str) -> &str {
    name.split('.').next().unwrap_or(name)
}

/// The files in `dir` that could be downloads (not the store's own
/// bookkeeping or hidden files), with their sizes.
fn listing(dir: &Path) -> Vec<(String, u64)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            if name.starts_with(STATE_FILE) || name.starts_with('.') {
                return None;
            }
            let meta = std::fs::metadata(e.path()).ok().filter(std::fs::Metadata::is_file)?;
            Some((name, meta.len()))
        })
        .collect()
}

/// The files in `dir` the pins don't name, with their sizes.
fn strays(dir: &Path, pins: &Pins) -> Vec<(String, u64)> {
    let known = known_names(pins);
    listing(dir).into_iter().filter(|(name, _)| !known.contains(name)).collect()
}

/// Removes files nothing refers to (a crash between unpin and delete). Only
/// with pins that [`load_pins`] trusts.
fn remove_strays(dir: &Path, pins: &Pins) {
    for (name, _) in strays(dir, pins) {
        tracing::info!("offline: removing stray {name}");
        let _ = std::fs::remove_file(dir.join(name));
    }
}

/// Ends the temporary name a file is copied to by [`move_files`] (see [`moving_name`]).
const MOVING: &str = ".moving";

/// Where [`move_files`] copies `name` across volumes before renaming it into
/// place, so a copy cut short never passes for the file.
fn moving_name(name: &str) -> String {
    format!(".{name}{MOVING}")
}

fn is_moving(name: &str) -> bool {
    name.len() > 1 + MOVING.len() && name.starts_with('.') && name.ends_with(MOVING)
}

/// Deletes copies a move left unfinished (it quit or crashed mid-copy). The
/// file they were copies of is still whole where it came from, so these are
/// never needed, whether or not the pins are trusted.
fn remove_moving(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        if is_moving(&e.file_name().to_string_lossy()) && e.file_type().is_ok_and(|t| t.is_file()) {
            tracing::info!("offline: removing {}, left by a move cut short", e.path().display());
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

/// How far [`move_store_with`] has got, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MoveProgress {
    pub done: u64,
    pub total: u64,
}

/// Moves a store's files from `from` to `to` (run before opening it at `to`),
/// including the per-account stores in folders under it. Renames where it can,
/// copies across volumes. Files already at `to` win.
///
/// Stopping it anywhere (quitting, a crash) loses nothing, and running it again
/// finishes the job: a copy is made under a temporary name (see
/// [`moving_name`]) and renamed into place before the original goes, and a file
/// found at both ends with the same size is one whose copy was done.
pub fn move_store(from: &Path, to: &Path) -> io::Result<()> {
    move_store_with(from, to, &mut |_| {})
}

/// [`move_store`], telling `progress` after each file. It can take a while
/// across volumes: run it off the UI thread.
pub fn move_store_with(from: &Path, to: &Path, progress: &mut dyn FnMut(MoveProgress)) -> io::Result<()> {
    if from == to || !from.is_dir() {
        return Ok(());
    }
    let mut at = MoveProgress { done: 0, total: tree_size(from) };
    progress(at);
    move_files(from, to, true, &mut |bytes| {
        at.done += bytes;
        progress(at);
    })
}

/// Bytes in the files under `dir`.
fn tree_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => tree_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

/// The folder for `account`'s downloads under the downloads folder `base`.
/// Each server account keeps its own pins and files, so signing in somewhere
/// else never fetches the old pins with the new server's URLs, and switching
/// back finds them again. A store kept directly in `base` by an older version
/// becomes this account's the first time.
pub fn account_store(base: &Path, account: u32) -> io::Result<PathBuf> {
    let dir = base.join(format!("{account:08x}"));
    if base.join(STATE_FILE).is_file() && !dir.join(STATE_FILE).exists() {
        move_files(base, &dir, false, &mut |_| {})?;
    }
    Ok(dir)
}

/// Moves the files in `from` (with `folders`, the folders in it too) to `to`,
/// calling `moved` with each file's size once it's dealt with.
fn move_files(from: &Path, to: &Path, folders: bool, moved: &mut dyn FnMut(u64)) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let kind = e.file_type()?;
        if kind.is_dir() {
            if folders {
                move_files(&e.path(), &to.join(e.file_name()), true, moved)?;
            }
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        let len = e.metadata()?.len();
        if is_moving(&e.file_name().to_string_lossy()) {
            // A copy cut short by an earlier move into `from`.
            let _ = std::fs::remove_file(e.path());
            moved(len);
            continue;
        }
        let dest = to.join(e.file_name());
        if let Ok(there) = dest.symlink_metadata() {
            if there.is_file() && there.len() == len {
                // Copied, but the move stopped before the original went.
                std::fs::remove_file(e.path())?;
            }
            moved(len);
            continue;
        }
        if std::fs::rename(e.path(), &dest).is_err() {
            let tmp = to.join(moving_name(&e.file_name().to_string_lossy()));
            std::fs::copy(e.path(), &tmp)?;
            std::fs::rename(&tmp, &dest)?;
            std::fs::remove_file(e.path())?;
        }
        moved(len);
    }
    // Leaves `from` behind only if something else is in it.
    let _ = std::fs::remove_dir(from);
    Ok(())
}

#[cfg(test)]
mod tests;
