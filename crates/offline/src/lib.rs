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
//! Files are `<id>.<suffix>` (the id percent-escaped, as in the stream cache),
//! with `.part` appended while downloading. The engine plays a finished one as a
//! `file://` source; see [`OfflineStore::local_path`].

pub mod pins;

pub use pins::{Pin, PinRecord, Pins, TrackRef};

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

const STATE_FILE: &str = "offline.json";
/// Downloads running at once.
const CONCURRENCY: usize = 2;
/// Failed attempts before a track waits for a manual retry or a reconnect.
const MAX_ATTEMPTS: u32 = 6;
/// Progress repaints at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(150);

/// Makes the stream URL for a track id; `None` while the server is unreachable.
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
        let inner = Arc::new(Inner {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .read_timeout(Duration::from_secs(60))
                .build()
                .map_err(io::Error::other)?,
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
        let mut st = self.inner.lock();
        let gone = st.pins.set(pin, label.into(), tracks);
        self.inner.forget(&mut st, gone);
        self.inner.persist(&st);
        self.inner.changed(st);
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
        self.inner.forget(&mut st, gone);
        self.inner.persist(&st);
        self.inner.changed(st);
    }

    /// Unpins; files no other pin needs are deleted.
    pub fn unpin(&self, pin: &Pin) {
        let mut st = self.inner.lock();
        let gone = st.pins.remove(pin);
        self.inner.forget(&mut st, gone);
        self.inner.persist(&st);
        self.inner.changed(st);
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
        self.inner.persist(&st);
        self.inner.changed(st);
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

    /// Deletes the files of tracks no pin needs; a running download of one is
    /// cancelled and cleans up after itself.
    fn forget(&self, st: &mut State, gone: Vec<String>) {
        for id in gone {
            st.failures.remove(&id);
            if let Some(a) = st.active.get(&id) {
                a.cancel.store(true, Ordering::Relaxed);
                continue;
            }
            remove_files(&self.dir, &id);
        }
    }

    fn persist(&self, st: &State) {
        let write = || -> io::Result<()> {
            let tmp = self.dir.join(format!("{STATE_FILE}.tmp"));
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&serde_json::to_vec(&st.pins).map_err(io::Error::other)?)?;
            f.sync_all()?;
            std::fs::rename(&tmp, self.dir.join(STATE_FILE))
        };
        if let Err(e) = write() {
            tracing::warn!("saving offline state: {e}");
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
            drop(st);
            remove_files(&self.dir, &job.r.id);
            let st = self.lock();
            return self.changed(st);
        }
        match result {
            Ok(size) => {
                st.failures.remove(&job.r.id);
                if let Some(s) = st.pins.tracks.get_mut(&job.r.id) {
                    s.done = Some(size);
                }
                self.persist(&st);
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
        match promote(src, job) {
            Ok(size) => return Ok(size),
            Err(e) => tracing::debug!("promote {} from cache: {e}", job.r.id),
        }
    }
    let Some(url) = &job.url else { return Err(JobError::Failed("server unreachable".into())) };
    download(inner, job, url).await
}

/// Takes a complete stream-cache file: a hard link where possible (same
/// volume; eviction there only unlinks its own name), else a copy.
fn promote(src: &Path, job: &Job) -> io::Result<u64> {
    let len = std::fs::metadata(src)?.len();
    if job.r.size.is_some_and(|s| s != len) {
        return Err(io::Error::other("cached file has the wrong size"));
    }
    let _ = std::fs::remove_file(&job.dest);
    if std::fs::hard_link(src, &job.dest).is_err() {
        std::fs::copy(src, &job.part)?;
        std::fs::rename(&job.part, &job.dest)?;
    }
    Ok(len)
}

async fn download(inner: &Inner, job: &Job, url: &str) -> Result<u64, JobError> {
    use reqwest::StatusCode;
    use reqwest::header::{CONTENT_RANGE, CONTENT_TYPE, RANGE};

    let mut have = std::fs::metadata(&job.part).map_or(0, |m| m.len());
    if job.r.size.is_some_and(|s| have > s) {
        have = 0;
    }
    let mut req = inner.http.get(url);
    if have > 0 {
        req = req.header(RANGE, format!("bytes={have}-"));
    }
    let mut resp = req.send().await.map_err(|e| JobError::Failed(e.to_string()))?;
    let status = resp.status();
    let total = match status {
        StatusCode::PARTIAL_CONTENT => resp
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit('/').next())
            .and_then(|t| t.parse::<u64>().ok()),
        StatusCode::OK => {
            // The server ignored the range (or there was none): start over.
            have = 0;
            resp.content_length()
        }
        StatusCode::RANGE_NOT_SATISFIABLE if have > 0 && job.r.size == Some(have) => {
            return finish(job, have);
        }
        StatusCode::RANGE_NOT_SATISFIABLE => {
            let _ = std::fs::remove_file(&job.part);
            return Err(JobError::Failed("server rejected the resume; starting over".into()));
        }
        s => return Err(JobError::Failed(format!("http {s}"))),
    };
    // Subsonic reports errors as a JSON body with 200.
    if resp.headers().get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|ct| ct.contains("json")) {
        let body = resp.text().await.unwrap_or_default();
        return Err(JobError::Failed(format!("server error: {}", body.chars().take(200).collect::<String>())));
    }
    let mut file = std::fs::OpenOptions::new().create(true).write(true).append(have > 0).truncate(have == 0).open(&job.part)?;
    let mut last_note = Instant::now();
    inner.progress(&job.r.id, have, total, true);
    loop {
        if job.cancel.load(Ordering::Relaxed) {
            return Err(JobError::Cancelled);
        }
        let chunk = match resp.chunk().await {
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
    file.sync_all()?;
    drop(file);
    if let Some(t) = total
        && have != t
    {
        return Err(JobError::Failed(format!("incomplete: {have} of {t} bytes")));
    }
    finish(job, have)
}

/// Checks the size and moves the finished `.part` into place.
fn finish(job: &Job, have: u64) -> Result<u64, JobError> {
    if let Some(expected) = job.r.size
        && have != expected
    {
        let _ = std::fs::remove_file(&job.part);
        return Err(JobError::Failed(format!("size mismatch: got {have} bytes, server said {expected}")));
    }
    std::fs::rename(&job.part, &job.dest)?;
    Ok(have)
}

fn backoff(attempts: u32) -> Duration {
    Duration::from_secs((2u64 << attempts.min(8)).min(300))
}

/// Filesystem-safe `<id>.<suffix>`, the same scheme as the stream cache.
pub fn file_name(r: &TrackRef) -> String {
    let mut name = escape(&r.id);
    if let Some(s) = &r.suffix {
        let s: String = s.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
        if !s.is_empty() {
            name.push('.');
            name.push_str(&s.to_ascii_lowercase());
        }
    }
    name
}

fn escape(id: &str) -> String {
    let mut name = String::with_capacity(id.len() + 6);
    for b in id.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' {
            name.push(b as char);
        } else {
            name.push_str(&format!("%{b:02X}"));
        }
    }
    name
}

/// Deletes the finished and partial files of track `id`, whatever their suffix.
fn remove_files(dir: &Path, id: &str) {
    let stem = escape(id);
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if name == stem || name.strip_prefix(&stem).is_some_and(|rest| rest.starts_with('.')) {
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

#[cfg(not(unix))]
fn free_space(_: &Path) -> Option<u64> {
    None
}

/// Moves a store's files from `from` to `to` (run before opening it at `to`).
/// Renames where it can, copies across volumes. Files already at `to` win.
pub fn move_store(from: &Path, to: &Path) -> io::Result<()> {
    if from == to || !from.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        if !e.file_type()?.is_file() {
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

/// "1.2 GB", "340 MB".
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    match u {
        0 => format!("{n} bytes"),
        _ if v < 10.0 => format!("{v:.1} {}", UNITS[u]),
        _ => format!("{v:.0} {}", UNITS[u]),
    }
}

#[cfg(test)]
mod tests;
