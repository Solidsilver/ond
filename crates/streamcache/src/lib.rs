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
//! This is not the offline store: that keeps its own never-evicted directory,
//! and the app plays from it first (as a `file://` source). Both name files
//! with [`file_name`], so pinning a track that's complete here can hard-link
//! it instead of downloading it again.
//!
//! # Locking
//!
//! There are three kinds of lock. Take them in this order, never the reverse.
//!
//! 1. The prefetch slot. Nothing else is locked under it. Dropping a `Hold`
//!    takes the index lock, so a `Hold` taken out of the slot is dropped after
//!    the slot is unlocked.
//! 2. The cache index.
//! 3. One download's state.
//!
//! Code holding a download's state lock never takes the index lock. `finish`
//! unlocks the index before it marks the download done, and `release` takes
//! the state lock while it already holds the index lock. A download's holder
//! count changes only under the index lock. That keeps a download found in
//! the index alive while it's handed out. [`Abort`] runs under a download's
//! state lock, so it must not call back into the cache.

pub mod disk;
mod download;
pub mod fetch;
mod ranges;

pub use fetch::{CONNECT_TIMEOUT, READ_TIMEOUT, ReplyCheck};

use download::Download;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

pub const DEFAULT_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The first wait before a download retries, doubling after each failure.
pub const RETRY: Duration = Duration::from_millis(250);

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
    /// The first wait before retrying a failed or short response; it doubles
    /// on each further failure (250 ms by default).
    pub retry: Duration,
    /// Keeps the server's error replies out of the cache: a download whose
    /// body it flags fails instead of completing, and a small file it flags
    /// is deleted at startup. The default keeps every body.
    pub reply_check: ReplyCheck,
}

impl CacheConfig {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into(), max_bytes: DEFAULT_MAX_BYTES, read_timeout: READ_TIMEOUT, retry: RETRY, reply_check: ReplyCheck::default() }
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

    /// The original file of track `id`.
    pub fn original(id: impl Into<String>, suffix: Option<String>) -> Self {
        Self::new(id, suffix)
    }

    /// Track `id` converted by the server to `format` at `kbps`: cached apart
    /// from the original and from other bitrates.
    pub fn transcode(id: &str, format: &str, kbps: u32) -> Self {
        Self::new(format!("{id}~{format}{kbps}"), Some(format.to_owned()))
    }

    /// The file's name in the cache directory; see [`file_name`].
    pub fn file_name(&self) -> String {
        file_name(&self.id, self.suffix.as_deref())
    }
}

/// `<escaped id>.<suffix>`: how the stream cache and the offline store name a
/// track's file. Both use this, so a finished cache file can be hard-linked
/// into the offline store under the name it expects. `NameBuf` builds the
/// same name on the stack for lookups; a change here changes it too.
pub fn file_name(id: &str, suffix: Option<&str>) -> String {
    let mut name = escape_id(id);
    if let Some(s) = suffix {
        let s: String = s.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
        if !s.is_empty() {
            name.push('.');
            name.push_str(&s.to_ascii_lowercase());
        }
    }
    name
}

/// A [`CacheKey`] made of borrowed parts, for checking the cache without
/// allocating (see [`StreamCache::is_complete`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheKeyRef<'a> {
    /// Like [`CacheKey::original`].
    Original { id: &'a str, suffix: Option<&'a str> },
    /// Like [`CacheKey::transcode`].
    Transcode { id: &'a str, format: &'a str, kbps: u32 },
}

/// Longest file name the cache can hold. Every filesystem we target caps a
/// name at 255 bytes (NTFS at 255 UTF-16 units, the same for ASCII), and our
/// names are ASCII, so a longer name can't be a file in the cache.
const NAME_MAX: usize = 255;

/// A file name built on the stack, the way [`file_name`] builds one.
struct NameBuf {
    buf: [u8; NAME_MAX],
    len: usize,
    /// The name didn't fit, so no such file can exist.
    overflow: bool,
}

impl NameBuf {
    fn of(key: CacheKeyRef<'_>) -> Self {
        let mut name = Self { buf: [0; NAME_MAX], len: 0, overflow: false };
        match key {
            CacheKeyRef::Original { id, suffix } => {
                name.push_escaped(id.as_bytes());
                if let Some(s) = suffix {
                    name.push_suffix(s);
                }
            }
            CacheKeyRef::Transcode { id, format, kbps } => {
                // `{id}~{format}{kbps}`, as `CacheKey::transcode` spells it.
                name.push_escaped(id.as_bytes());
                name.push_escaped(b"~");
                name.push_escaped(format.as_bytes());
                let mut digits = [0u8; 10];
                let mut n = kbps;
                let mut i = digits.len();
                loop {
                    i -= 1;
                    digits[i] = b'0' + (n % 10) as u8;
                    n /= 10;
                    if n == 0 {
                        break;
                    }
                }
                name.push_escaped(&digits[i..]);
                name.push_suffix(format);
            }
        }
        name
    }

    fn push(&mut self, b: u8) {
        match self.buf.get_mut(self.len) {
            Some(slot) => {
                *slot = b;
                self.len += 1;
            }
            None => self.overflow = true,
        }
    }

    /// Appends `bytes` escaped as [`escape_id`] does.
    fn push_escaped(&mut self, bytes: &[u8]) {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        for &b in bytes {
            if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' {
                self.push(b);
            } else {
                self.push(b'%');
                self.push(HEX[usize::from(b >> 4)]);
                self.push(HEX[usize::from(b & 0xF)]);
            }
        }
    }

    /// Appends `.suffix` as [`file_name`] does.
    fn push_suffix(&mut self, suffix: &str) {
        let mut kept = suffix.bytes().filter(u8::is_ascii_alphanumeric).take(8).peekable();
        if kept.peek().is_some() {
            self.push(b'.');
            for b in kept {
                self.push(b.to_ascii_lowercase());
            }
        }
    }

    /// The name, unless it was too long to be a file.
    fn as_str(&self) -> Option<&str> {
        if self.overflow {
            return None;
        }
        std::str::from_utf8(&self.buf[..self.len]).ok()
    }
}

/// An id made filesystem-safe: ids are opaque server strings, so anything
/// outside `[A-Za-z0-9_-]` is percent-escaped. The result never contains `.`.
pub fn escape_id(id: &str) -> String {
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

/// How much of a file is on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub downloaded: u64,
    pub len: Option<u64>,
    pub complete: bool,
}

/// Lets a blocked read give up early (track change, seek, shutdown).
///
/// Called while the download's state lock is held, every 50 ms while an open
/// or a read waits. Keep it to loading a flag: it must not block or call
/// into the cache (see the crate's locking order).
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
            if meta.len() < 64 * 1024 && is_error_reply(&cfg.reply_check, &path) {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            index.total += meta.len();
            index.complete.insert(name.to_owned(), Entry { size: meta.len(), last_used: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH) });
        }
        let inner = Inner {
            client: fetch::client(cfg.read_timeout).map_err(io::Error::other)?,
            cfg,
            rt,
            index: Mutex::new(index),
            prefetch: Mutex::new(None),
            seq: AtomicU64::new(0),
        };
        let victims = inner.evict(&mut inner.lock_index(), None);
        inner.remove(victims);
        Ok(Self { inner: Arc::new(inner) })
    }

    /// Opens `key` for reading, downloading it from `url` if it isn't cached.
    ///
    /// Blocks until the server has answered (so the length is known), the
    /// download fails, or `abort` returns true.
    pub fn open(&self, key: &CacheKey, url: &str, abort: Abort) -> io::Result<CachedSource> {
        let name = key.file_name();
        let dl = loop {
            if let Some(file) = self.inner.open_complete(&name) {
                let len = file.metadata()?.len();
                return Ok(CachedSource::File { file, len });
            }
            // `None` means a download finished the file since we looked, so open that.
            if let Some(dl) = self.inner.acquire(&name, url) {
                break dl;
            }
        };
        // Declared before `st` on purpose: the early returns below drop `st`
        // first, so `Hold::drop` takes the index lock without the download's.
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
        let hold = self.inner.acquire(&name, url).map(|dl| Hold { dl, inner: self.inner.clone() });
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
    ///
    /// The cancelled downloads leave the index at once, so an `open` right
    /// after starts afresh instead of attaching to one that's winding down.
    /// Readers still holding them fail their next blocked read.
    pub fn cancel_all(&self) {
        self.clear_prefetch();
        for (_, dl) in self.inner.lock_index().active.drain() {
            dl.cancel();
        }
    }

    /// The finished file for `key`, if it's cached (for the offline store to
    /// hard-link instead of downloading again).
    pub fn complete_path(&self, key: &CacheKey) -> Option<PathBuf> {
        let name = key.file_name();
        self.inner.lock_index().complete.contains_key(&name).then(|| self.inner.cfg.dir.join(name))
    }

    /// Whether `key` is cached in full. It doesn't allocate, so the app can
    /// ask every frame. It agrees with [`complete_path`](Self::complete_path).
    pub fn is_complete(&self, key: CacheKeyRef<'_>) -> bool {
        let name = NameBuf::of(key);
        name.as_str().is_some_and(|name| self.inner.lock_index().complete.contains_key(name))
    }

    /// Forgets every completed file and deletes it (in the background), for
    /// when the files came from another server. Downloads in progress finish
    /// on their own.
    pub fn clear(&self) {
        self.clear_prefetch();
        let victims: Vec<String> = {
            let mut idx = self.inner.lock_index();
            idx.total = 0;
            idx.complete.drain().map(|(name, _)| name).collect()
        };
        let inner = self.inner.clone();
        self.inner.rt.spawn_blocking(move || inner.remove(victims));
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
        let path = self.cfg.dir.join(name);
        let now = SystemTime::now();
        // The read-open stays under the index lock so "indexed but gone" is
        // settled against the same view `is_complete` reads.
        let file = {
            let mut idx = self.lock_index();
            let entry = idx.complete.get_mut(name)?;
            match File::open(&path) {
                Ok(f) => {
                    entry.last_used = now;
                    f
                }
                Err(_) => {
                    // Deleted behind our back: forget it and download again.
                    let size = idx.complete.remove(name).map_or(0, |e| e.size);
                    idx.total -= size;
                    return None;
                }
            }
        };
        // Persist recency for the next startup scan; best effort, and off the
        // lock that every `is_complete` call takes.
        if let Ok(w) = std::fs::OpenOptions::new().write(true).open(&path) {
            let _ = w.set_modified(now);
        }
        Some(file)
    }

    /// Returns the active download for `name`, starting one if needed, with
    /// one more holder. Returns `None` if `name` is complete.
    ///
    /// Cancelling a download takes it out of `active` under the same lock, so
    /// one found here is live; a cancelled one is replaced all the same.
    /// Completeness is checked under the lock too, so a download that finished
    /// after the caller last looked isn't started again.
    fn acquire(self: &Arc<Self>, name: &str, url: &str) -> Option<Arc<Download>> {
        let mut idx = self.lock_index();
        if idx.complete.contains_key(name) {
            return None;
        }
        if let Some(dl) = idx.active.get(name).filter(|dl| !dl.is_cancelled()) {
            // Whoever started it holds it until it leaves `active`.
            let before = dl.holders.fetch_add(1, Ordering::Relaxed);
            debug_assert!(before > 0, "{name}: an active download without holders");
            return Some(dl.clone());
        }
        let dl = Arc::new(Download::new(
            name.to_owned(),
            self.cfg.dir.join(format!("{name}.{}.part", self.seq.fetch_add(1, Ordering::Relaxed))),
            self.cfg.dir.join(name),
            self.cfg.retry,
        ));
        dl.holders.store(1, Ordering::Relaxed);
        idx.active.insert(name.to_owned(), dl.clone());
        drop(idx);

        let (inner, task_dl, url) = (self.clone(), dl.clone(), url.to_owned());
        self.rt.spawn(async move {
            let result = download::run(&task_dl, &inner.client, &inner.cfg.reply_check, &url).await;
            // Renaming and evicting touch the disk: keep them off the async workers.
            let _ = tokio::task::spawn_blocking(move || inner.finish(&task_dl, result)).await;
        });
        Some(dl)
    }

    fn release(&self, dl: &Arc<Download>) {
        let mut idx = self.lock_index();
        let before = dl.holders.fetch_sub(1, Ordering::Relaxed);
        debug_assert!(before > 0, "{}: released more holds than were taken", dl.name);
        if before == 1 && !dl.lock().complete {
            // Nobody wants the rest of this file; stop fetching it. The task deletes the `.part`.
            dl.cancel();
            if idx.active.get(&dl.name).is_some_and(|a| Arc::ptr_eq(a, dl)) {
                idx.active.remove(&dl.name);
            }
        }
    }

    fn finish(&self, dl: &Arc<Download>, result: Result<u64, String>) {
        let mut idx = self.lock_index();
        if idx.active.get(&dl.name).is_some_and(|a| Arc::ptr_eq(a, dl)) {
            idx.active.remove(&dl.name);
        }
        // Renamed under the index lock, like `remove` moves files away, so a
        // late delete of an evicted copy can't land on this one. `run` already
        // synced the data, so only the rename waits on the disk here.
        let result = result.and_then(|len| {
            std::fs::rename(&dl.part_path, &dl.final_path).map_err(|e| format!("cache file: {e}"))?;
            Ok(len)
        });
        match result {
            Ok(len) => {
                let old = idx.complete.insert(dl.name.clone(), Entry { size: len, last_used: SystemTime::now() });
                idx.total = idx.total - old.map_or(0, |e| e.size) + len;
                let victims = self.evict(&mut idx, Some(&dl.name));
                drop(idx);
                dl.finish(Ok(()));
                self.remove(victims);
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

    /// Drops least recently used files from the index until under the cap,
    /// never `keep`, and returns them for [`remove`](Self::remove), which
    /// deletes them once the index lock is released.
    fn evict(&self, idx: &mut Index, keep: Option<&str>) -> Vec<String> {
        let mut victims = Vec::new();
        while idx.total > self.cfg.max_bytes {
            let Some(victim) = idx.complete.iter().filter(|(n, _)| Some(n.as_str()) != keep).min_by_key(|(_, e)| e.last_used).map(|(n, _)| n.clone()) else {
                break;
            };
            let size = idx.complete.remove(&victim).map_or(0, |e| e.size);
            idx.total -= size;
            victims.push(victim);
        }
        victims
    }

    /// Deletes evicted files. Open handles keep working: unlinking an open file
    /// is fine on every platform we target (std opens with FILE_SHARE_DELETE on
    /// Windows).
    ///
    /// This runs after the index let the victims go, so a victim may have been
    /// downloaded again since: if it's back in the index, the file is the new
    /// copy and stays. Otherwise it's moved to a `.part` name (swept at startup
    /// if the delete doesn't happen) under the index lock, the same lock
    /// `finish` renames under, and deleted after.
    fn remove(&self, victims: Vec<String>) {
        for victim in victims {
            let doomed = {
                let idx = self.lock_index();
                if idx.complete.contains_key(&victim) {
                    continue;
                }
                let doomed = self.cfg.dir.join(format!("{victim}.{}.part", self.seq.fetch_add(1, Ordering::Relaxed)));
                match std::fs::rename(self.cfg.dir.join(&victim), &doomed) {
                    Ok(()) => doomed,
                    Err(e) => {
                        if e.kind() != io::ErrorKind::NotFound {
                            tracing::warn!("evict {victim}: {e}");
                        }
                        continue;
                    }
                }
            };
            if let Err(e) = std::fs::remove_file(&doomed) {
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
            // The length can arrive late (a stream that didn't declare one) or
            // shrink (one that declared more than it has).
            self.len = st.len.or(self.len);
            if let Some(len) = self.len
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
        }
    }
}

impl Seek for StreamReader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
            SeekFrom::End(d) => match self.hold.dl.lock().len.or(self.len) {
                Some(len) => len.checked_add_signed(d),
                None => return Err(io::Error::new(io::ErrorKind::Unsupported, "length unknown")),
            },
        };
        self.pos = target.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before start"))?;
        Ok(self.pos)
    }
}

/// A file on disk that `check` calls an error reply rather than audio.
fn is_error_reply(check: &ReplyCheck, path: &Path) -> bool {
    std::fs::read(path).is_ok_and(|bytes| check.is_error(None, &bytes))
}

#[cfg(test)]
mod tests;
