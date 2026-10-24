//! User preferences held in memory as the one source of truth, saved to the
//! settings file in the background. A change is visible at once; the file is
//! written shortly after the last change in a burst (a dragged slider is one
//! write, not one per frame), and on the way out.

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
    /// The file couldn't be parsed; it was moved to `aside` and defaults used.
    Unreadable { aside: PathBuf },
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
    /// Reads the settings file at `path`. An unreadable file is moved aside
    /// (never silently overwritten) and the defaults used instead.
    pub fn open(path: impl Into<PathBuf>) -> (Self, Loaded) {
        let path = path.into();
        let (settings, loaded) = match std::fs::read(&path) {
            Err(_) => (Settings::default(), Loaded::Missing),
            Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
                Ok(mut s) => {
                    s.migrate();
                    (s, Loaded::Read)
                }
                Err(e) => {
                    let aside = set_aside(&path);
                    tracing::warn!("settings file unreadable ({e}); moved to {} and starting from defaults", aside.display());
                    (Settings::default(), Loaded::Unreadable { aside })
                }
            },
        };
        (Self::with(settings, path), loaded)
    }

    fn with(settings: Settings, path: PathBuf) -> Self {
        let shared = Arc::new(Shared { state: Mutex::new(State { settings, revision: 0, written: 0, closing: false }), wake: Condvar::new(), path });
        let writer_shared = shared.clone();
        let writer = std::thread::Builder::new().name("settings-writer".into()).spawn(move || writer_loop(&writer_shared)).ok();
        Self { handle: Arc::new(Handle { shared, writer: Mutex::new(writer) }) }
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
    /// (asked for every frame, say) doesn't cause a save.
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

    /// Records a successful login, keeping every other preference as it is.
    pub fn set_login(&self, server: &str, username: &str) {
        self.edit(|s| {
            s.server = server.to_owned();
            s.username = username.to_owned();
            s.signed_out = false;
        });
    }

    /// Whether an account was ever saved (else the login form starts empty).
    pub fn has_login(&self) -> bool {
        self.get(|s| !s.server.is_empty())
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
        let (next, timeout) = shared.wake.wait_timeout(state, DEBOUNCE).unwrap_or_else(|e| e.into_inner());
        state = next;
        if state.closing || !timeout.timed_out() || state.revision != seen {
            continue;
        }
        drop(state);
        flush(shared);
        state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
    }
}

/// Writes the latest preferences if anything changed since the last write.
/// The snapshot is taken while holding the writer lock, so a later write never
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
    write(&shared.path, &snapshot);
}

/// A temporary file renamed over the old one, so a reader never sees half of it.
fn write(path: &Path, settings: &Settings) {
    let result = (|| -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(settings)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if let Err(e) = result {
        tracing::warn!("saving settings: {e}");
    }
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
    use crate::settings::DeviceMode;
    use crate::theme::ThemePref;

    fn read(path: &Path) -> Settings {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn an_existing_settings_file_reads_every_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        // Every field the app has ever written, as an older build wrote them.
        std::fs::write(
            &path,
            r#"{
  "server": "https://music.example.com",
  "username": "me",
  "stream_cache_mb": 4096,
  "output_device": "uid-umc22",
  "device_modes": { "uid-umc22": "bit-perfect", "uid-speakers": "normal" },
  "theme": "dark",
  "show_queue": false,
  "window": { "mode": "mini" },
  "scrobble": false,
  "signed_out": true,
  "playback": { "replay_gain": "album", "preamp_db": 2.0, "untagged_db": -6.0, "quality": "auto", "format": "mp3", "kbps": 192 },
  "offline_dir": "/Volumes/Music/Ond Offline",
  "offline_limit_gb": 50,
  "offline_move_from": "/old/place",
  "analyze_playing": true,
  "analysis_resume": true,
  "diagnostics_bar": true,
  "sidebar_width": 240.0,
  "queue_width": 360.0
}"#,
        )
        .unwrap();
        let (prefs, loaded) = Prefs::open(&path);
        assert_eq!(loaded, Loaded::Read);
        let s = prefs.snapshot();
        assert_eq!(s.server, "https://music.example.com");
        assert_eq!(s.stream_cache_mb, Some(4096));
        assert_eq!(s.output_device.as_deref(), Some("uid-umc22"));
        assert_eq!(s.device_modes.get("uid-umc22"), Some(&DeviceMode::BitPerfect));
        assert_eq!(s.device_modes.get("uid-speakers"), Some(&DeviceMode::Normal));
        assert_eq!(s.theme, ThemePref::Dark);
        assert!(!s.show_queue && !s.scrobble && s.signed_out);
        assert_eq!(s.window.mode, crate::window::WindowMode::Mini);
        assert_eq!(s.playback.replay_gain, crate::playback::RgMode::Album);
        assert_eq!(s.playback.kbps, 192);
        assert_eq!(s.offline_limit_gb, Some(50));
        assert!(s.analyze_playing && s.analysis_resume && s.diagnostics_bar);
        assert_eq!((s.sidebar_width, s.queue_width), (Some(240.0), Some(360.0)));
        assert_eq!(s.volume, 1.0, "older files have no volume: full");
        // Written back, the modes keep their old spelling.
        prefs.edit(|_| {});
        prefs.flush();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(r#""uid-umc22": "bit-perfect""#), "{text}");
    }

    #[test]
    fn an_unreadable_file_is_set_aside_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let (prefs, loaded) = Prefs::open(&path);
        let Loaded::Unreadable { aside } = loaded else { panic!("expected unreadable, got {loaded:?}") };
        assert_eq!(std::fs::read(&aside).unwrap(), b"{ not json", "the old file is kept as it was");
        assert!(!prefs.has_login());
        prefs.edit(|s| s.theme = ThemePref::Light);
        prefs.flush();
        assert_eq!(read(&path).theme, ThemePref::Light, "a fresh file replaces it");
    }

    #[test]
    fn a_burst_of_changes_is_one_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let (prefs, _) = Prefs::open(&path);
        for i in 0..50 {
            prefs.edit(|s| s.volume = i as f32 / 50.0);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!path.exists(), "nothing written while changes keep coming");
        std::thread::sleep(DEBOUNCE * 3);
        assert_eq!(read(&path).volume, 49.0 / 50.0, "the last value, once things settle");
    }

    #[test]
    fn an_edit_that_changes_nothing_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let (prefs, _) = Prefs::open(&path);
        let volume = prefs.get(|s| s.volume);
        prefs.edit(|s| s.volume = volume);
        std::thread::sleep(DEBOUNCE * 3);
        assert!(!path.exists(), "no change, no write");
    }

    #[test]
    fn volume_and_preferences_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        {
            let (prefs, _) = Prefs::open(&path);
            prefs.edit(|s| {
                s.volume = 0.42;
                s.device_modes.insert("uid".into(), DeviceMode::BitPerfect);
            });
            // Dropped right away: the change is still written.
        }
        let (prefs, loaded) = Prefs::open(&path);
        assert_eq!(loaded, Loaded::Read);
        assert_eq!(prefs.get(|s| s.volume), 0.42);
        assert_eq!(prefs.get(|s| s.device_modes.get("uid").copied()), Some(DeviceMode::BitPerfect));
    }

    #[test]
    fn a_login_keeps_preferences_changed_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let (prefs, _) = Prefs::open(&path);
        prefs.edit(|s| s.theme = ThemePref::Dark);
        prefs.set_login("https://new", "me");
        prefs.flush();
        let now = read(&path);
        assert_eq!(now.server, "https://new");
        assert_eq!(now.theme, ThemePref::Dark);
        assert!(!now.signed_out);
    }

    #[test]
    fn concurrent_edits_are_all_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let (prefs, _) = Prefs::open(&path);
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let prefs = prefs.clone();
                std::thread::spawn(move || {
                    for k in 0..10 {
                        prefs.edit(|s| {
                            s.device_modes.insert(format!("dev-{i}-{k}"), DeviceMode::Normal);
                        });
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        prefs.flush();
        assert_eq!(read(&path).device_modes.len(), 80);
    }
}
