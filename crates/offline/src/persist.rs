//! What reaches the disk besides downloads: the saver thread writing the pins
//! atomically, the deleter thread removing files no pin needs (and measuring
//! free space), and reading the pins at open with recovery from a state file
//! that can't be trusted.
//!
//! Nothing is deleted that the store can't prove is stray: a file counts as
//! a track's only by its stem (escaped ids never contain '.', so hidden files
//! are never a track's), and while a state file that failed to load is set
//! aside, nothing the pins don't name is removed at all.

use crate::{Aside, Inner, Pins, SET_ASIDE, STATE_FILE, file_name};
use std::collections::HashSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

/// A copy of the pins to write, taken under the state lock and written after it.
pub(crate) struct Save {
    pub(crate) revision: u64,
    pub(crate) bytes: Vec<u8>,
}

/// How far the saver thread got, for waiting on it.
#[derive(Default)]
pub(crate) struct Flush {
    /// The newest revision handed to the saver.
    pub(crate) requested: AtomicU64,
    /// The newest revision written (or given up on).
    done: Mutex<u64>,
    cond: Condvar,
}

impl Flush {
    pub(crate) fn wait_for(&self, revision: u64, timeout: Duration) {
        let done = self.done.lock().unwrap_or_else(|e| e.into_inner());
        let _ = self.cond.wait_timeout_while(done, timeout, |d| *d < revision);
    }

    pub(crate) fn mark(&self, revision: u64) {
        let mut done = self.done.lock().unwrap_or_else(|e| e.into_inner());
        *done = (*done).max(revision);
        self.cond.notify_all();
    }
}

/// Work for the deleter thread.
pub(crate) enum Chore {
    /// Delete these tracks' files.
    Delete(Vec<String>),
    /// [`Inner::sweep`].
    Sweep,
    /// Measure the free space for the snapshot.
    FreeSpace,
}

/// The saver thread: writes the newest pins handed to it, skipping any that a
/// newer one superseded while it was busy, until the store goes away.
///
/// Saves can arrive out of order (two commits race to hand theirs over once
/// the state lock is released), so one older than a save already seen is
/// dropped rather than written over it.
pub(crate) fn saver(dir: PathBuf, saves: Receiver<Save>, flush: Arc<Flush>) {
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
pub(crate) fn deleter(store: Weak<Inner>, dir: PathBuf, chores: Receiver<Chore>) {
    while let Ok(first) = chores.recv() {
        // Anything else that piled up goes in the same directory scan.
        let (mut ids, mut sweep, mut measure) = (Vec::new(), false, false);
        for chore in std::iter::once(first).chain(chores.try_iter()) {
            match chore {
                Chore::Delete(batch) => ids.extend(batch),
                Chore::Sweep => sweep = true,
                Chore::FreeSpace => measure = true,
            }
        }
        if measure {
            // Not under the state lock: this can take seconds.
            let free = free_space(&dir);
            let Some(inner) = store.upgrade() else { return };
            let mut st = inner.lock();
            st.measuring_free = false;
            let old = st.free.replace((Instant::now(), free));
            if old.is_none_or(|(_, was)| was != free) {
                inner.changed(st);
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
    /// [`OfflineStore::remove_set_aside`](crate::OfflineStore::remove_set_aside), on the deleter thread: deletes the
    /// files the pins don't name, then the set-aside state files. A track
    /// pinned meanwhile whose file is among them waits until they're gone
    /// (see [`State::sweeping`](crate::State::sweeping)). False once the store has shut down.
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
    pub(crate) fn deleting(&self, ids: &mut Vec<String>) -> bool {
        let mut st = self.lock();
        if st.closed {
            return false;
        }
        ids.retain(|id| !st.pins.tracks.contains_key(id) && !st.active.contains_key(id));
        st.deleting.extend(ids.iter().cloned());
        true
    }

    /// The files of `ids` are gone: any pinned again meanwhile can download.
    pub(crate) fn deleted(&self, ids: &[String]) {
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
///
/// Hidden files are never a track's: an empty id escapes to the empty stem,
/// which is every hidden file's (`.adopting`, `.DS_Store`, a `.moving` copy).
fn remove_files(dir: &Path, ids: &[String]) {
    if ids.is_empty() {
        return;
    }
    let stems: HashSet<String> = ids.iter().map(|id| streamcache::escape_id(id)).collect();
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with('.') && stems.contains(stem(name)) {
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
pub(crate) fn load_pins(dir: &Path) -> (Pins, bool) {
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
pub(crate) fn set_aside_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(SET_ASIDE)).map(|e| e.path()).collect();
    files.sort();
    files
}

/// The names of every file the pins account for, finished or partial.
pub(crate) fn known_names(pins: &Pins) -> HashSet<String> {
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
pub(crate) fn strays(dir: &Path, pins: &Pins) -> Vec<(String, u64)> {
    let known = known_names(pins);
    listing(dir).into_iter().filter(|(name, _)| !known.contains(name)).collect()
}

/// Removes files nothing refers to (a crash between unpin and delete). Only
/// with pins that [`load_pins`] trusts.
pub(crate) fn remove_strays(dir: &Path, pins: &Pins) {
    for (name, _) in strays(dir, pins) {
        tracing::info!("offline: removing stray {name}");
        let _ = std::fs::remove_file(dir.join(name));
    }
}

#[cfg(unix)]
pub(crate) fn free_space(dir: &Path) -> Option<u64> {
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
pub(crate) fn free_space(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let path: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
    let mut available = 0u64;
    // SAFETY: `path` is NUL-terminated and `available` a valid out pointer;
    // the totals we don't need may be null.
    let ok = unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut available, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(available)
}
