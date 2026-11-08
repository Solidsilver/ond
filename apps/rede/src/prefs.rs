//! Rede's preferences, held in memory as the one source of truth and saved to
//! the settings file in the background. A change is visible at once; the file
//! is written shortly after the last change in a burst (a button pressed twice
//! is one write), and on the way out.

use crate::settings::Settings;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// How long the file waits after a change for more changes to arrive.
const DEBOUNCE: Duration = Duration::from_millis(400);

/// What reading the settings file found at start-up.
#[derive(Debug, Clone, PartialEq)]
pub enum Loaded {
    /// No file yet (first launch).
    Missing,
    Read,
    /// The file wasn't JSON; it was moved to `aside` and defaults used.
    Unreadable {
        aside: PathBuf,
    },
}

/// A cheap handle; clones share the same preferences.
#[derive(Clone)]
pub struct Prefs {
    handle: Arc<Handle>,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    path: PathBuf,
    debounce: Duration,
}

struct State {
    settings: Settings,
    /// Bumped by every change.
    revision: u64,
    /// The revision last written to the file.
    written: u64,
    /// The last handle went away: write what's left and stop.
    closing: bool,
}

/// Owns the writer thread; dropping the last [`Prefs`] flushes and joins it.
struct Handle {
    shared: Arc<Shared>,
    writer: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Prefs {
    /// Reads the settings file at `path`. A value this build doesn't know costs
    /// only that setting (see [`Settings::from_json`]); a file that isn't JSON
    /// is moved aside, never silently overwritten, and the defaults used.
    pub fn open(path: impl Into<PathBuf>) -> (Self, Loaded) {
        Self::open_debounced(path, DEBOUNCE)
    }

    /// [`open`](Self::open) with the wait for a burst of changes to end set by
    /// the caller, so a test needn't race the real one against the clock.
    fn open_debounced(path: impl Into<PathBuf>, debounce: Duration) -> (Self, Loaded) {
        let path = path.into();
        let (settings, loaded) = match std::fs::read(&path) {
            Err(_) => (Settings::default(), Loaded::Missing),
            Ok(bytes) => match Settings::from_json(&bytes) {
                Ok(s) => (s, Loaded::Read),
                Err(e) => {
                    let aside = set_aside(&path);
                    tracing::warn!("settings file unreadable ({e}); moved to {} and starting from defaults", aside.display());
                    (Settings::default(), Loaded::Unreadable { aside })
                }
            },
        };
        let shared = Arc::new(Shared { state: Mutex::new(State { settings, revision: 0, written: 0, closing: false }), wake: Condvar::new(), path, debounce });
        let writer_shared = shared.clone();
        let writer = std::thread::Builder::new().name("settings-writer".into()).spawn(move || writer_loop(&writer_shared)).ok();
        (Self { handle: Arc::new(Handle { shared, writer: Mutex::new(writer) }) }, loaded)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.handle.shared.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Reads one or more preferences.
    pub fn get<R>(&self, read: impl FnOnce(&Settings) -> R) -> R {
        read(&self.lock().settings)
    }

    /// A copy of everything.
    pub fn snapshot(&self) -> Settings {
        self.lock().settings.clone()
    }

    /// Changes preferences now and saves them shortly. Changes from any thread
    /// apply one at a time, so none is lost. An edit that changes nothing
    /// doesn't cause a save.
    pub fn edit(&self, change: impl FnOnce(&mut Settings)) {
        let mut state = self.lock();
        let before = state.settings.clone();
        change(&mut state.settings);
        if state.settings == before {
            return;
        }
        state.revision += 1;
        drop(state);
        self.handle.shared.wake.notify_all();
    }

    /// Writes now if anything changed (quitting, tests).
    pub fn flush(&self) {
        flush(&self.handle.shared);
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        {
            let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            state.closing = true;
        }
        self.shared.wake.notify_all();
        if let Some(writer) = self.writer.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = writer.join();
        }
        // Whatever the writer hadn't got to yet (or if it never started).
        flush(&self.shared);
    }
}

/// Waits for a change, then for the burst to end, then writes the latest.
fn writer_loop(shared: &Shared) {
    let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
    while !state.closing {
        if state.revision == state.written {
            state = shared.wake.wait(state).unwrap_or_else(|e| e.into_inner());
            continue;
        }
        // Let a burst of changes settle: each new change restarts the wait.
        let seen = state.revision;
        let (next, timeout) = shared.wake.wait_timeout(state, shared.debounce).unwrap_or_else(|e| e.into_inner());
        state = next;
        if state.closing || !timeout.timed_out() || state.revision != seen {
            continue;
        }
        drop(state);
        flush(shared);
        state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
    }
}

/// Writes the latest preferences if anything changed since the last write. The
/// snapshot is taken while holding the writer lock, so a later write never
/// carries older preferences than an earlier one.
fn flush(shared: &Shared) {
    static WRITER: Mutex<()> = Mutex::new(());
    let _one = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    let snapshot = {
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.revision == state.written {
            return;
        }
        state.written = state.revision;
        state.settings.clone()
    };
    if let Err(e) = write(&shared.path, &snapshot) {
        tracing::warn!("saving settings: {e}");
    }
}

fn write(path: &Path, settings: &Settings) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    replace_file(path, &path.with_extension("json.tmp"), &serde_json::to_vec_pretty(settings)?)?;
    Ok(())
}

/// Replaces `path` with `bytes` through `tmp`, renamed over it, so a reader
/// never sees half a file. The bytes reach the disk before the rename: without
/// that a power cut can leave an empty file behind the new name (and empty
/// settings would sign the user out). It waits for the disk, so call it off the
/// UI thread.
pub fn replace_file(path: &Path, tmp: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::File::create(tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(tmp, path)?;
    // The rename itself lasts once the directory is on disk too. Best effort:
    // the file is whole either way.
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
        let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
    }
    Ok(())
}

/// `settings.json` → `settings.json.bad-<unix seconds>`.
fn set_aside(path: &Path) -> PathBuf {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(format!(".bad-{secs}"));
    let aside = path.with_file_name(name);
    if let Err(e) = std::fs::rename(path, &aside) {
        tracing::warn!("setting aside {}: {e}", path.display());
    }
    aside
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Account;

    fn read(path: &Path) -> Settings {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    fn account() -> Account {
        Account::new("https://books.example.com", "sam")
    }

    #[test]
    fn a_change_reaches_the_file_once_the_wait_is_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let (prefs, loaded) = Prefs::open_debounced(&path, Duration::from_millis(10));
        assert_eq!(loaded, Loaded::Missing);
        prefs.edit(|s| s.sort = crate::settings::LibrarySort::Author);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !path.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(read(&path).sort, crate::settings::LibrarySort::Author, "the writer thread saved it without a flush");
    }

    #[test]
    fn an_edit_that_changes_nothing_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let (prefs, _) = Prefs::open(&path);
        let sort = prefs.get(|s| s.sort);
        prefs.edit(|s| s.sort = sort);
        std::thread::sleep(DEBOUNCE * 3);
        assert!(!path.exists(), "no change, no write");
    }

    #[test]
    fn a_login_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        {
            let (prefs, _) = Prefs::open(&path);
            let a = account();
            prefs.edit(|s| {
                s.put_account(a.clone());
                s.theme = shell::theme::ThemePref::Dark;
            });
            // Dropped right away: the change is still written.
        }
        let (prefs, loaded) = Prefs::open(&path);
        assert_eq!(loaded, Loaded::Read);
        assert_eq!(prefs.get(|s| s.account().map(|a| a.username.clone())), Some("sam".into()));
        assert_eq!(prefs.get(|s| s.theme), shell::theme::ThemePref::Dark);
    }

    #[test]
    fn a_file_that_isnt_json_is_set_aside_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let (prefs, loaded) = Prefs::open(&path);
        let Loaded::Unreadable { aside } = loaded else { panic!("expected unreadable, got {loaded:?}") };
        assert_eq!(std::fs::read(&aside).unwrap(), b"{ not json", "the old file is kept as it was");
        assert!(prefs.get(|s| s.account().is_none()));
        prefs.edit(|s| s.theme = shell::theme::ThemePref::Light);
        prefs.flush();
        assert_eq!(read(&path).theme, shell::theme::ThemePref::Light, "a fresh file replaces it");
    }

    #[test]
    fn a_burst_of_changes_is_one_write() {
        // An hour's wait can't end mid-burst however the scheduler stalls; the
        // last value reaches the file only when the handle goes away.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let (prefs, _) = Prefs::open_debounced(&path, Duration::from_secs(3600));
        for _ in 0..20 {
            prefs.edit(|s| {
                s.sort = if s.sort == crate::settings::LibrarySort::Title { crate::settings::LibrarySort::Author } else { crate::settings::LibrarySort::Title }
            });
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!path.exists(), "nothing written while changes keep coming");
        drop(prefs);
        assert_eq!(read(&path).sort, crate::settings::LibrarySort::Title, "the last value, once the burst is over");
    }

    #[test]
    fn replacing_a_file_leaves_the_new_contents_and_no_temporary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let tmp = dir.path().join("settings.json.tmp");
        replace_file(&path, &tmp, b"first").unwrap();
        replace_file(&path, &tmp, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert!(!tmp.exists());
        // A directory that isn't there is an error, not a panic.
        let gone = dir.path().join("missing").join("settings.json");
        assert!(replace_file(&gone, &gone.with_extension("json.tmp"), b"x").is_err());
    }
}
