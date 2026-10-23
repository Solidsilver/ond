//! Progressive, seekable on-disk cache for streamed tracks.
//!
//! [`StreamCache::open`] returns a blocking `Read + Seek` source over a file that
//! a tokio task is still downloading. Reads block until their bytes arrive; a
//! read far from the download cursor makes the task switch to a `Range` request
//! at that offset and fill in the skipped part afterwards.
//!
//! Layout: `<dir>/<id>.<suffix>` for complete files, `<id>.<suffix>.<n>.part` while
//! downloading. Completed files are kept for replay and evicted least recently
//! used past `max_bytes`. Partial files are discarded: when the last reader (or
//! the prefetch slot) lets go of an unfinished download it is cancelled and its
//! `.part` deleted, and `.part` files left over from a previous run are deleted
//! on startup. Resuming would mean persisting the range map and trusting the
//! server file hasn't changed underneath, which isn't worth it for a cache.
//!
//! This is not the offline store. Offline downloads will live in their own
//! directory that is never evicted, and the engine will check it first (as a
//! `file://` source) before falling back to this cache. Pinning a track that's
//! already complete here can be a rename or hard link into that directory.

mod download;
mod ranges;

use download::Download;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

pub const DEFAULT_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Give up connecting after this long.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A response that sends nothing for this long counts as broken: the download
/// retries and, where the server supports Range, resumes where it stopped.
pub const READ_TIMEOUT: Duration = Duration::from_secs(20);

/// An HTTP client for downloads, with a connect timeout and a read timeout
/// (time allowed between received bytes, not for the whole body).
pub fn http_client(read_timeout: Duration) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder().connect_timeout(CONNECT_TIMEOUT).read_timeout(read_timeout).build()
}

/// How often a blocked reader re-checks its abort flag.
const POLL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone)]
pub struct CacheConfig {
    pub dir: PathBuf,
    /// Size cap for completed files. Downloads in progress aren't counted.
    pub max_bytes: u64,
    /// How long a download may go without receiving a byte before it counts
    /// as stalled and retries ([`READ_TIMEOUT`] by default).
    pub read_timeout: Duration,
}

impl CacheConfig {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into(), max_bytes: DEFAULT_MAX_BYTES, read_timeout: READ_TIMEOUT }
    }
}

/// Identifies one cached file: the server's track id plus its file suffix.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub id: String,
    pub suffix: Option<String>,
}

impl CacheKey {
    pub fn new(id: impl Into<String>, suffix: Option<String>) -> Self {
        Self { id: id.into(), suffix }
    }

    /// A filesystem-safe name. Ids are opaque server strings, so anything outside
    /// `[A-Za-z0-9_-]` is percent-escaped.
    fn file_name(&self) -> String {
        let mut name = String::with_capacity(self.id.len() + 6);
        for b in self.id.bytes() {
            if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' {
                name.push(b as char);
            } else {
                name.push_str(&format!("%{b:02X}"));
            }
        }
        if let Some(s) = &self.suffix {
            let s: String = s.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
            if !s.is_empty() {
                name.push('.');
                name.push_str(&s.to_ascii_lowercase());
            }
        }
        name
    }
}

/// How much of a file is on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub downloaded: u64,
    pub len: Option<u64>,
    pub complete: bool,
}

/// Lets a blocked read give up early (track change, seek, shutdown).
pub type Abort = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Clone)]
pub struct StreamCache {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for StreamCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamCache").field("dir", &self.inner.cfg.dir).finish_non_exhaustive()
    }
}

struct Inner {
    cfg: CacheConfig,
    rt: tokio::runtime::Handle,
    client: reqwest::Client,
    index: Mutex<Index>,
    /// At most one prefetch holds a download open at a time.
    prefetch: Mutex<Option<Hold>>,
    /// Makes each download's `.part` name unique, so a cancelled download that's
    /// still winding down never touches its replacement's file.
    seq: AtomicU64,
}

#[derive(Default)]
struct Index {
    complete: HashMap<String, Entry>,
    total: u64,
    active: HashMap<String, Arc<Download>>,
}

struct Entry {
    size: u64,
    last_used: SystemTime,
}

impl StreamCache {
    /// Scans `cfg.dir`: completed files are indexed (last use = mtime), stale `.part` files deleted.
    pub fn new(cfg: CacheConfig, rt: tokio::runtime::Handle) -> io::Result<Self> {
        std::fs::create_dir_all(&cfg.dir)?;
        let mut index = Index::default();
        for entry in std::fs::read_dir(&cfg.dir)? {
            let entry = entry?;
            let path = entry.path();
            let meta = entry.metadata()?;
            if !meta.is_file() {
                continue;
            }
            if path.extension().is_some_and(|e| e == "part") {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            // An error reply cached as audio by an earlier version: it would fail every play.
            if meta.len() < 64 * 1024 && is_error_reply(&path) {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            index.total += meta.len();
            index.complete.insert(
                name.to_owned(),
                Entry { size: meta.len(), last_used: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH) },
            );
        }
        let inner = Inner {
            client: http_client(cfg.read_timeout).map_err(io::Error::other)?,
            cfg,
            rt,
            index: Mutex::new(index),
            prefetch: Mutex::new(None),
            seq: AtomicU64::new(0),
        };
        let mut idx = inner.lock_index();
        inner.evict(&mut idx, None);
        drop(idx);
        Ok(Self { inner: Arc::new(inner) })
    }

    /// Opens `key` for reading, downloading it from `url` if it isn't cached.
    ///
    /// Blocks until the server has answered (so the length is known), the
    /// download fails, or `abort` returns true.
    pub fn open(&self, key: &CacheKey, url: &str, abort: Abort) -> io::Result<CachedSource> {
        let name = key.file_name();
        if let Some(file) = self.inner.open_complete(&name) {
            let len = file.metadata()?.len();
            return Ok(CachedSource::File { file, len });
        }
        let dl = self.inner.acquire(&name, url);
        let hold = Hold { dl: dl.clone(), inner: self.inner.clone() };

        let mut st = dl.lock();
        while !st.headers && !st.complete {
            if let Some(e) = &st.error {
                return Err(io::Error::other(e.clone()));
            }
            if abort() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "aborted"));
            }
            st = dl.cond.wait_timeout(st, POLL).unwrap_or_else(|e| e.into_inner()).0;
        }
        let len = st.len;
        drop(st);
        let file = download::open_read(&dl)?;
        Ok(CachedSource::Stream(StreamReader { hold, file, pos: 0, len, abort }))
    }

    /// Starts downloading `key` in the background without a reader, replacing
    /// (and, if nothing else holds it, cancelling) the previous prefetch.
    pub fn prefetch(&self, key: &CacheKey, url: &str) {
        let name = key.file_name();
        let hold = if self.inner.lock_index().complete.contains_key(&name) {
            None
        } else {
            Some(Hold { dl: self.inner.acquire(&name, url), inner: self.inner.clone() })
        };
        let old = std::mem::replace(&mut *self.inner.lock_prefetch(), hold);
        drop(old);
    }

    pub fn clear_prefetch(&self) {
        let old = self.inner.lock_prefetch().take();
        drop(old);
    }

    pub fn progress(&self, key: &CacheKey) -> Option<Progress> {
        let name = key.file_name();
        let idx = self.inner.lock_index();
        if let Some(e) = idx.complete.get(&name) {
            return Some(Progress { downloaded: e.size, len: Some(e.size), complete: true });
        }
        let dl = idx.active.get(&name)?.clone();
        drop(idx);
        let st = dl.lock();
        Some(Progress { downloaded: st.ranges.total(), len: st.len, complete: st.complete })
    }

    /// Cancels every download and the prefetch. Completed files stay.
    pub fn cancel_all(&self) {
        self.clear_prefetch();
        for dl in self.inner.lock_index().active.values() {
            dl.cancel();
        }
    }

    /// The finished file for `key`, if it's cached (for the offline store to
    /// hard-link instead of downloading again).
    pub fn complete_path(&self, key: &CacheKey) -> Option<PathBuf> {
        let name = key.file_name();
        self.inner.lock_index().complete.contains_key(&name).then(|| self.inner.cfg.dir.join(name))
    }

    /// Bytes held by completed files.
    pub fn size(&self) -> u64 {
        self.inner.lock_index().total
    }

    pub fn dir(&self) -> &Path {
        &self.inner.cfg.dir
    }
}

impl Inner {
    fn lock_index(&self) -> MutexGuard<'_, Index> {
        self.index.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lock_prefetch(&self) -> MutexGuard<'_, Option<Hold>> {
        self.prefetch.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn open_complete(&self, name: &str) -> Option<File> {
        let mut idx = self.lock_index();
        let entry = idx.complete.get_mut(name)?;
        let path = self.cfg.dir.join(name);
        match File::open(&path) {
            Ok(f) => {
                let now = SystemTime::now();
                entry.last_used = now;
                // Persist recency for the next startup scan; best effort.
                if let Ok(w) = std::fs::OpenOptions::new().write(true).open(&path) {
                    let _ = w.set_modified(now);
                }
                Some(f)
            }
            Err(_) => {
                // Deleted behind our back: forget it and download again.
                let size = idx.complete.remove(name).map_or(0, |e| e.size);
                idx.total -= size;
                None
            }
        }
    }

    /// Returns the active download for `name`, starting one if needed, with one more holder.
    fn acquire(self: &Arc<Self>, name: &str, url: &str) -> Arc<Download> {
        let mut idx = self.lock_index();
        if let Some(dl) = idx.active.get(name) {
            dl.holders.fetch_add(1, Ordering::Relaxed);
            return dl.clone();
        }
        let dl = Arc::new(Download::new(
            name.to_owned(),
            self.cfg.dir.join(format!("{name}.{}.part", self.seq.fetch_add(1, Ordering::Relaxed))),
            self.cfg.dir.join(name),
        ));
        dl.holders.store(1, Ordering::Relaxed);
        idx.active.insert(name.to_owned(), dl.clone());
        drop(idx);

        let (inner, task_dl, url) = (self.clone(), dl.clone(), url.to_owned());
        self.rt.spawn(async move {
            let result = download::run(&task_dl, &inner.client, &url).await;
            inner.finish(&task_dl, result);
        });
        dl
    }

    fn release(&self, dl: &Arc<Download>) {
        let mut idx = self.lock_index();
        if dl.holders.fetch_sub(1, Ordering::Relaxed) == 1 && !dl.lock().complete {
            // Nobody wants the rest of this file; stop fetching it. The task deletes the `.part`.
            dl.cancel();
            if idx.active.get(&dl.name).is_some_and(|a| Arc::ptr_eq(a, dl)) {
                idx.active.remove(&dl.name);
            }
        }
    }

    fn finish(&self, dl: &Arc<Download>, result: Result<u64, String>) {
        let result = result.and_then(|len| {
            std::fs::rename(&dl.part_path, &dl.final_path).map_err(|e| format!("cache file: {e}"))?;
            Ok(len)
        });
        let mut idx = self.lock_index();
        if idx.active.get(&dl.name).is_some_and(|a| Arc::ptr_eq(a, dl)) {
            idx.active.remove(&dl.name);
        }
        match result {
            Ok(len) => {
                let old = idx.complete.insert(dl.name.clone(), Entry { size: len, last_used: SystemTime::now() });
                idx.total = idx.total - old.map_or(0, |e| e.size) + len;
                self.evict(&mut idx, Some(&dl.name));
                drop(idx);
                dl.finish(Ok(()));
            }
            Err(e) => {
                drop(idx);
                let _ = std::fs::remove_file(&dl.part_path);
                if !dl.is_cancelled() {
                    tracing::warn!("download {}: {e}", dl.name);
                }
                dl.finish(Err(e));
            }
        }
    }

    /// Deletes least recently used files until under the cap, never `keep`.
    /// Open handles keep working: unlinking an open file is fine on every platform
    /// we target (std opens with FILE_SHARE_DELETE on Windows).
    fn evict(&self, idx: &mut Index, keep: Option<&str>) {
        while idx.total > self.cfg.max_bytes {
            let Some(victim) = idx
                .complete
                .iter()
                .filter(|(n, _)| Some(n.as_str()) != keep)
                .min_by_key(|(_, e)| e.last_used)
                .map(|(n, _)| n.clone())
            else {
                break;
            };
            let size = idx.complete.remove(&victim).map_or(0, |e| e.size);
            idx.total -= size;
            if let Err(e) = std::fs::remove_file(self.cfg.dir.join(&victim))
                && e.kind() != io::ErrorKind::NotFound
            {
                tracing::warn!("evict {victim}: {e}");
            }
        }
    }
}

/// Keeps a download alive; dropping the last one cancels it if unfinished.
struct Hold {
    dl: Arc<Download>,
    inner: Arc<Inner>,
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.inner.release(&self.dl);
    }
}

/// A readable, seekable track: either a finished file or one still downloading.
pub enum CachedSource {
    File { file: File, len: u64 },
    Stream(StreamReader),
}

impl CachedSource {
    pub fn byte_len(&self) -> Option<u64> {
        match self {
            Self::File { len, .. } => Some(*len),
            Self::Stream(s) => s.len,
        }
    }
}

impl Read for CachedSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::File { file, .. } => file.read(buf),
            Self::Stream(s) => s.read(buf),
        }
    }
}

impl Seek for CachedSource {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        match self {
            Self::File { file, .. } => file.seek(to),
            Self::Stream(s) => s.seek(to),
        }
    }
}

pub struct StreamReader {
    hold: Hold,
    file: File,
    pos: u64,
    len: Option<u64>,
    abort: Abort,
}

impl Read for StreamReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let dl = &self.hold.dl;
        let mut st = dl.lock();
        loop {
            if let Some(len) = st.len
                && self.pos >= len
            {
                return Ok(0);
            }
            let avail = st.ranges.available_from(self.pos);
            if avail > 0 {
                drop(st);
                let want = avail.min(buf.len() as u64) as usize;
                self.file.seek(SeekFrom::Start(self.pos))?;
                let n = self.file.read(&mut buf[..want])?;
                self.pos += n as u64;
                return Ok(n);
            }
            if st.complete {
                return Ok(0);
            }
            if let Some(e) = &st.error {
                return Err(io::Error::other(e.clone()));
            }
            if (self.abort)() || dl.is_cancelled() {
                return Err(io::Error::other("read aborted"));
            }
            if st.wanted != Some(self.pos) {
                st.wanted = Some(self.pos);
                dl.wake.notify_one();
            }
            st = dl.cond.wait_timeout(st, POLL).unwrap_or_else(|e| e.into_inner()).0;
            // A length may have arrived with the first bytes of an unknown-length stream.
            self.len = self.len.or(st.len);
        }
    }
}

impl Seek for StreamReader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
            SeekFrom::End(d) => match self.len.or(self.hold.dl.lock().len) {
                Some(len) => len.checked_add_signed(d),
                None => return Err(io::Error::new(io::ErrorKind::Unsupported, "length unknown")),
            },
        };
        self.pos = target.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before start"))?;
        Ok(self.pos)
    }
}

/// A Subsonic error reply (JSON or XML) rather than audio.
fn is_error_reply(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else { return false };
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
    let text = text.trim_start();
    (text.starts_with('{') && text.contains("subsonic-response")) || text.starts_with("<?xml") || text.starts_with("<subsonic-response")
}

#[cfg(test)]
mod tests;
