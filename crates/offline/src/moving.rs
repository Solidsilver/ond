//! Moving a store to another folder, and adopting a store an older version
//! kept straight in the downloads folder into an account's folder. Plain
//! filesystem work, run before a store is opened at the destination.
//!
//! A move never loses a file: a copy is made under a temporary hidden name and
//! renamed into place before the original goes, an original is deleted only
//! when the destination holds a file of the same size by that name, and one
//! that loses to a different file stays where it was.

use crate::STATE_FILE;
use std::io;
use std::path::{Path, PathBuf};

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
pub(crate) fn remove_moving(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        if is_moving(&e.file_name().to_string_lossy()) && e.file_type().is_ok_and(|t| t.is_file()) {
            tracing::info!("offline: removing {}, left by a move cut short", e.path().display());
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// How far [`move_store_with`] has got, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MoveProgress {
    pub done: u64,
    pub total: u64,
}

/// What a move left behind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MoveOutcome {
    /// Files still where they came from, because a file of another size had
    /// the same name at the destination. The destination's file wins, and the
    /// original stays, since nothing proves it's a stray. Their folder stays
    /// too. Empty when everything moved.
    pub kept: Vec<PathBuf>,
}

/// Moves a store's files from `from` to `to` (run before opening it at `to`),
/// including the per-account stores in folders under it. Renames where it can,
/// copies across volumes. Files already at `to` win, and the originals that
/// lost are listed in the outcome and logged.
///
/// Stopping it anywhere (quitting, a crash) loses nothing, and running it again
/// finishes the job. A copy is made under a temporary name and renamed into
/// place before the original goes, and a file found at both ends with the same
/// size is one whose copy was done.
pub fn move_store(from: &Path, to: &Path) -> io::Result<MoveOutcome> {
    move_store_with(from, to, &mut |_| {})
}

/// [`move_store`], telling `progress` after each file. It can take a while
/// across volumes, so run it off the UI thread.
///
/// Refuses a `to` inside `from`: the walk would find the new folder among the
/// files to move and carry everything one level deeper into itself. The paths
/// are compared resolved, so a symlink or a trailing slash can't hide that
/// they're the same folder; moving a folder onto itself would see every file
/// "already at the destination" and delete it. A `from` inside `to` is fine:
/// only `from` is walked, and `to`'s own files are never touched.
pub fn move_store_with(from: &Path, to: &Path, progress: &mut dyn FnMut(MoveProgress)) -> io::Result<MoveOutcome> {
    let mut outcome = MoveOutcome::default();
    if !from.is_dir() {
        return Ok(outcome);
    }
    let (real_from, real_to) = (std::fs::canonicalize(from)?, resolve(to)?);
    if real_from == real_to {
        return Ok(outcome);
    }
    if real_to.starts_with(&real_from) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("can't move downloads into {}, which is inside {}", to.display(), from.display())));
    }
    let mut at = MoveProgress { done: 0, total: tree_size(from) };
    progress(at);
    move_files(
        from,
        to,
        true,
        &mut |bytes| {
            at.done += bytes;
            progress(at);
        },
        &mut outcome,
    )?;
    for kept in &outcome.kept {
        tracing::info!("offline: kept {}, as {} already has a different file by that name", kept.display(), to.display());
    }
    Ok(outcome)
}

/// `path` with symlinks and `..` resolved by the filesystem as far as it
/// exists, and the rest (which has no symlinks to follow) lexically.
fn resolve(path: &Path) -> io::Result<PathBuf> {
    use std::path::Component;
    let mut real = PathBuf::new();
    let mut exists = true;
    for c in path.components() {
        match c {
            Component::CurDir => continue,
            Component::ParentDir if !exists => {
                real.pop();
                continue;
            }
            Component::Prefix(_) => {
                // A drive alone would resolve to that drive's current folder.
                real.push(c);
                continue;
            }
            _ => real.push(c),
        }
        if exists {
            match std::fs::canonicalize(&real) {
                Ok(r) => real = r,
                Err(e) if e.kind() == io::ErrorKind::NotFound => exists = false,
                Err(e) => return Err(e),
            }
        }
    }
    Ok(real)
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
///
/// That adoption moves files one by one, in no set order, so a crash can stop
/// it after `offline.json` moved and before the downloads did. A marker
/// written in the account's folder before the first file moves says its
/// adoption is unfinished, and comes off once every file is there; that
/// account carries the move on at its next open, and no other account
/// touches what's left in `base` (a second account signing in between would
/// otherwise take them). A second `offline.json` in `base` while this account
/// has its own is another store, and it stays put rather than mixing two sets
/// of pins.
pub fn account_store(base: &Path, account: u32) -> io::Result<PathBuf> {
    let dir = base.join(format!("{account:08x}"));
    let marker = dir.join(ADOPTING_FILE);
    let (theirs, mine) = (base.join(STATE_FILE).is_file(), dir.join(STATE_FILE).exists());
    let start = theirs && !mine;
    let resume = !theirs && marker.is_file() && has_leftovers(base);
    if start || resume {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&marker, b"")?;
        let mut outcome = MoveOutcome::default();
        move_files(base, &dir, false, &mut |_| {}, &mut outcome)?;
        for kept in &outcome.kept {
            tracing::warn!("offline: kept {}, as {} already has a different file by that name", kept.display(), dir.display());
        }
        std::fs::remove_file(&marker)?;
    }
    Ok(dir)
}

/// In an account's folder while an older store is being adopted into it.
/// Hidden, so the store's sweep leaves it alone.
pub(crate) const ADOPTING_FILE: &str = ".adopting";

/// `dir` holds files an adoption into an account folder still has to move: any
/// but hidden ones (a `.DS_Store`), apart from copies a move left unfinished.
fn has_leftovers(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            e.file_type().is_ok_and(|t| t.is_file()) && (!name.starts_with('.') || is_moving(&name))
        })
    })
}

/// Moves the files in `from` (with `folders`, the folders in it too) to `to`,
/// calling `moved` with each file's size once it's dealt with. Originals kept
/// because `to` has a different file by the same name go in `outcome`.
fn move_files(from: &Path, to: &Path, folders: bool, moved: &mut dyn FnMut(u64), outcome: &mut MoveOutcome) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let kind = e.file_type()?;
        if kind.is_dir() {
            if folders {
                move_files(&e.path(), &to.join(e.file_name()), true, moved, outcome)?;
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
            } else {
                outcome.kept.push(e.path());
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
