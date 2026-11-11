//! Offline downloads: albums, playlists and tracks the user pinned, kept as
//! original files in a directory that's never evicted.
//!
//! [`OfflineStore`] keeps the pins (see [`pins`]) in `offline.json` next to the
//! files and runs a small download queue on the tokio runtime: two at a time,
//! resumed with `Range` after an interruption or a restart (when the server's
//! file is provably still the same, because the last 64 KB on disk match what
//! it sends again), retried with backoff, and checked against the size the
//! server reported. A track that's already complete in the stream cache is
//! hard-linked (or copied) instead of downloaded again.
//!
//! Files are named by [`streamcache::file_name`], exactly as in the stream
//! cache, with `.part` appended while downloading. The engine plays a finished one as a
//! `file://` source; see [`OfflineStore::local_path`].
//!
//! This file holds the store and its state machine: what's pinned, what's
//! downloading, and the snapshot the UI reads. The rest is split by what it
//! touches: `download` runs one track's download, `persist` is everything
//! else that writes to or deletes from the folder (the saver and deleter
//! threads, and reading the pins at open), and `moving` moves a store to
//! another folder or adopts an older one into an account's.
//!
//! Three things hold everywhere. The store never deletes a file it can't
//! prove is stray: names come from [`streamcache::escape_id`], which never
//! emits '.', so a file's stem is its track's identity and hidden files are
//! never a track's; and while a state file that failed to load is set aside
//! (see `persist::load_pins`), no file the pins don't name is removed. The
//! state file is written atomically (a temporary file, synced, then renamed).
//! And once the store is open, nothing it does touches the disk on the
//! caller's thread or under the state lock.

mod download;
mod moving;
mod persist;
pub mod pins;

pub use moving::{MoveOutcome, MoveProgress, account_store, move_store, move_store_with};
pub use pins::{Pin, PinRecord, Pins, TrackRef};

use download::{backoff, reconcile, worker};
use moving::remove_moving;
use persist::{Chore, Flush, Save, deleter, free_space, known_names, load_pins, remove_strays, saver, set_aside_files, strays};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use streamcache::ReplyCheck;
use streamcache::disk::InFlight;
use streamcache::fetch;
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
/// How long dropping the store waits for its saves and file writes to finish.
const SHUTDOWN_BUDGET: Duration = Duration::from_secs(5);
/// How long a free-space reading is shown before it's measured again.
const FREE_SPACE_EVERY: Duration = Duration::from_secs(5);

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
    /// Tells the server's error replies from tracks, so one is never kept
    /// as a download. The default keeps every body.
    pub reply_check: ReplyCheck,
}

/// One track's download, as the UI shows it.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackState {
    Queued,
    Downloading {
        done: u64,
        total: Option<u64>,
    },
    Done {
        size: u64,
    },
    /// Gave up after several attempts; retried on reconnect or by hand.
    Failed {
        error: String,
    },
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
        if self.bytes_total > 0 { (self.bytes_done as f64 / self.bytes_total as f64).clamp(0.0, 1.0) as f32 } else { self.done as f32 / self.total as f32 }
    }
}

/// A download attempt's failure, without its URL or credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadError {
    pub message: String,
    /// The HTTP rejection, when the server answered with an error status.
    pub http_status: Option<u16>,
}

impl From<String> for DownloadError {
    fn from(message: String) -> Self {
        Self { message, http_status: None }
    }
}

impl From<&str> for DownloadError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}

/// Everything the UI needs, built once per change.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub version: u64,
    pub tracks: HashMap<String, TrackState>,
    /// The most recent failed attempt, including tracks waiting to retry.
    /// Callers can repair credentials before the retry budget runs out.
    pub last_errors: HashMap<String, DownloadError>,
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
    /// don't name are kept (see [`SetAside`]); `None` normally.
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
}

/// A handle to the store; copies share it. The store shuts down when the last
/// copy is dropped.
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
    ///
    /// Downloads can't start new file calls from then on, and it waits for the
    /// calls the aborted downloads left running on the blocking pool. A write
    /// landing after a new store here read the `.part` length would corrupt
    /// that store's resume. Both waits share one budget.
    fn drop(&mut self) {
        let deadline = Instant::now() + SHUTDOWN_BUDGET;
        self.inner.io.close();
        self.inner.close();
        for w in &self.workers {
            w.abort();
        }
        self.inner.flush.wait_for(self.inner.flush.requested.load(Ordering::Acquire), SHUTDOWN_BUDGET);
        if !self.inner.io.wait_idle(deadline.saturating_duration_since(Instant::now())) {
            tracing::warn!("offline: closing {} with file writes still running", self.inner.dir.display());
        }
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
    reply_check: ReplyCheck,
    state: Mutex<State>,
    wake: Notify,
    on_change: OnChange,
    /// Track ids whose files should go, deleted on a thread of their own so an
    /// unpin never waits on the disk (or holds the state lock while it does).
    doomed: Mutex<Sender<Chore>>,
    /// Pins to write, saved on a thread of their own for the same reason.
    saves: Mutex<Sender<Save>>,
    flush: Arc<Flush>,
    /// Downloads' file calls on the blocking pool, which outlive an aborted
    /// worker. Dropping the store waits for them.
    io: Arc<InFlight>,
}

struct State {
    pins: Pins,
    url: Option<UrlFn>,
    /// An error from credentials replaced while a request was in flight must
    /// not reject the new source or spend its retry budget.
    source_version: u64,
    cache: Option<CacheLookup>,
    limit: Option<u64>,
    active: HashMap<String, Active>,
    failures: HashMap<String, Failure>,
    limit_reached: bool,
    version: u64,
    snapshot: Arc<Snapshot>,
    /// Free space on the volume and when it was measured. The deleter thread
    /// measures it, because statting a sleeping network volume can take
    /// seconds and the snapshot is built under this lock on the UI thread.
    free: Option<(Instant, Option<u64>)>,
    /// The deleter has been asked to measure the free space again.
    measuring_free: bool,
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

struct Active {
    cancel: Arc<AtomicBool>,
    done: u64,
    total: Option<u64>,
}

struct Failure {
    attempts: u32,
    retry_at: Instant,
    error: DownloadError,
}

struct Job {
    r: TrackRef,
    url: Option<String>,
    source_version: u64,
    from_cache: Option<PathBuf>,
    part: PathBuf,
    dest: PathBuf,
    cancel: Arc<AtomicBool>,
}

#[derive(Debug)]
enum JobError {
    Cancelled,
    /// Worth trying again after a backoff.
    Failed(DownloadError),
    /// Would fail the same way every time: no retries until something changes
    /// (a refresh of the pin, a reconnect, a manual retry).
    Stale(String),
}

impl From<io::Error> for JobError {
    fn from(e: io::Error) -> Self {
        Self::Failed(e.to_string().into())
    }
}

impl OfflineStore {
    /// Loads the pins, checks the files are still there, removes strays and
    /// starts the download workers on `rt`.
    pub fn open(cfg: OfflineConfig, rt: tokio::runtime::Handle, on_change: OnChange) -> io::Result<Self> {
        std::fs::create_dir_all(&cfg.dir)?;
        remove_moving(&cfg.dir);
        let (mut pins, trusted) = load_pins(&cfg.dir);
        for s in pins.tracks.values_mut() {
            let path = cfg.dir.join(file_name(&s.r));
            if s.done.is_some() {
                // A finished file that's gone (deleted by hand) downloads again.
                if !path.is_file() {
                    s.done = None;
                }
            } else {
                s.done = reconcile(&path, s.r.size);
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
        // Measured here, off the UI thread, so the first snapshot has it.
        let free = Some((Instant::now(), free_space(&cfg.dir)));
        let inner = Arc::new(Inner {
            doomed: Mutex::new(doomed),
            saves: Mutex::new(saves),
            flush: flush.clone(),
            io: Arc::default(),
            // Offline downloads aren't waited on by playback: a slow server gets longer.
            http: fetch::client(Duration::from_secs(60)).map_err(io::Error::other)?,
            dir: cfg.dir,
            reply_check: cfg.reply_check,
            wake: Notify::new(),
            on_change,
            state: Mutex::new(State {
                pins,
                url: None,
                source_version: 0,
                cache: None,
                limit: cfg.limit,
                active: HashMap::new(),
                failures: HashMap::new(),
                limit_reached: false,
                version: 1,
                snapshot: Arc::new(Snapshot::default()),
                free,
                measuring_free: false,
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
        std::thread::Builder::new().name("offline-delete".into()).spawn(move || deleter(weak, dir, doomed_rx)).map_err(io::Error::other)?;
        let dir = inner.dir.clone();
        std::thread::Builder::new().name("offline-save".into()).spawn(move || saver(dir, saves_rx, flush)).map_err(io::Error::other)?;
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
        st.source_version += 1;
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
        let refreshed: Vec<String> =
            tracks.iter().filter(|t| st.pins.tracks.get(&t.id).is_some_and(|s| s.done.is_none() && s.r != **t)).map(|t| t.id.clone()).collect();
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

    /// The pins as they are now, for the [`saver`] thread to write once the lock is released.
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
        let job = Job { part: self.dir.join(format!("{name}.part")), dest: self.dir.join(name), r, url, source_version: st.source_version, from_cache, cancel };
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
            Err(_) if job.source_version != st.source_version => {}
            Err(JobError::Failed(error)) => {
                tracing::warn!("offline download {}: {error}", job.r.id);
                let attempts = st.failures.get(&job.r.id).map_or(0, |f| f.attempts) + 1;
                let retry_at = Instant::now() + backoff(attempts);
                st.failures.insert(job.r.id.clone(), Failure { attempts, retry_at, error });
            }
            Err(JobError::Stale(error)) => {
                tracing::warn!("offline download {}: {error}", job.r.id);
                st.failures.insert(job.r.id.clone(), Failure { attempts: MAX_ATTEMPTS, retry_at: Instant::now(), error: error.into() });
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
                TrackState::Failed { error: f.error.to_string() }
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
        // A stale reading is shown until the deleter thread brings a new one.
        let free = st.free.and_then(|(_, free)| free);
        if st.free.is_none_or(|(at, _)| at.elapsed() >= FREE_SPACE_EVERY) && !st.measuring_free {
            st.measuring_free = self.doomed.lock().unwrap_or_else(|e| e.into_inner()).send(Chore::FreeSpace).is_ok();
        }
        let set_aside = st.set_aside.as_ref().map(|a| {
            // Rare and short-lived, so the names are worked out each time.
            let known = known_names(&st.pins);
            let unknown: Vec<u64> = a.strays.iter().filter(|(n, _)| !known.contains(n)).map(|&(_, len)| len).collect();
            SetAside { files: a.files.clone(), unknown: unknown.len(), unknown_bytes: unknown.iter().sum(), removing: a.removing }
        });
        Snapshot {
            version: st.version,
            tracks,
            last_errors: st.failures.iter().map(|(id, failure)| (id.clone(), failure.error.clone())).collect(),
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

/// The track's file name, the same as in the stream cache.
pub fn file_name(r: &TrackRef) -> String {
    streamcache::file_name(&r.id, r.suffix.as_deref())
}

#[cfg(test)]
mod tests;
