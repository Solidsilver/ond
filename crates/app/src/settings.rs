//! Non-secret settings live in a JSON file; the password lives in the system keychain.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const KEYRING_SERVICE: &str = "dev.listener.navidrome";

/// One settings writer at a time (the UI thread and background tasks both save).
static WRITER: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub server: String,
    pub username: String,
    /// Size cap for the streaming cache; `None` uses the default (2 GB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_cache_mb: Option<u64>,
    /// Output device UID; `None` follows the system default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_device: Option<String>,
    /// Playback mode the user picked per device UID: "normal" or "bit-perfect".
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub device_modes: std::collections::HashMap<String, String>,
    /// Light, dark, or follow the system.
    #[serde(default)]
    pub theme: crate::theme::ThemePref,
    /// Whether the queue column is open.
    #[serde(default = "yes")]
    pub show_queue: bool,
    /// Full / Compact / Mini, and where each was.
    #[serde(default)]
    pub window: crate::window::Windows,
    /// Report plays to the server (which passes them on to Last.fm / ListenBrainz).
    #[serde(default = "yes")]
    pub scrobble: bool,
    /// The user signed out: show the login screen instead of reconnecting.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub signed_out: bool,
    /// ReplayGain and streaming quality.
    #[serde(default)]
    pub playback: crate::playback::PlaybackPrefs,
    /// Where offline downloads live; `None` uses the default (next to the library).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline_dir: Option<PathBuf>,
    /// Cap on offline downloads, in GB; `None` is unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline_limit_gb: Option<u64>,
    /// Downloads to move into `offline_dir` at the next launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline_move_from: Option<PathBuf>,
    /// Analyse each track as it plays.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub analyze_playing: bool,
    /// A library-wide analysis was started and hasn't finished: carry on next launch.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub analysis_resume: bool,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server: String::new(),
            username: String::new(),
            stream_cache_mb: None,
            output_device: None,
            device_modes: Default::default(),
            theme: Default::default(),
            show_queue: true,
            window: Default::default(),
            scrobble: true,
            signed_out: false,
            playback: Default::default(),
            offline_dir: None,
            offline_limit_gb: None,
            offline_move_from: None,
            analyze_playing: false,
            analysis_resume: false,
        }
    }
}

pub struct Paths {
    pub config_file: PathBuf,
    pub library_db: PathBuf,
    /// Streamed audio, evictable (`~/Library/Caches/...` on macOS).
    pub stream_cache: PathBuf,
    /// Downloaded cover art, evictable.
    pub art_cache: PathBuf,
    /// Offline downloads, never evicted (default location).
    pub offline: PathBuf,
}

impl Paths {
    pub fn resolve() -> anyhow::Result<Self> {
        let dirs = directories::ProjectDirs::from("dev", "listener", "Listener")
            .ok_or_else(|| anyhow::anyhow!("no home directory"))?;
        std::fs::create_dir_all(dirs.config_dir())?;
        std::fs::create_dir_all(dirs.data_dir())?;
        Ok(Self {
            config_file: dirs.config_dir().join("settings.json"),
            library_db: dirs.data_dir().join("library.redb"),
            stream_cache: dirs.cache_dir().join("streams"),
            art_cache: dirs.cache_dir().join("art"),
            offline: dirs.data_dir().join("Offline"),
        })
    }
}

impl Settings {
    pub fn load(paths: &Paths) -> Option<Self> {
        let bytes = std::fs::read(&paths.config_file).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Writes the whole file: a temporary file renamed over the old one, so a
    /// reader never sees half of it.
    pub fn save(&self, paths: &Paths) -> anyhow::Result<()> {
        let _one_writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
        self.write(paths)
    }

    fn write(&self, paths: &Paths) -> anyhow::Result<()> {
        let tmp = paths.config_file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, &paths.config_file)?;
        Ok(())
    }

    /// Loads, changes and saves the settings file (UI preferences the user just
    /// changed). Does nothing before the first login has written the file.
    /// Changes from any thread are applied one at a time, so none is lost.
    pub fn update(paths: &Paths, change: impl FnOnce(&mut Settings)) {
        let _one_writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
        let Some(mut s) = Self::load(paths) else { return };
        change(&mut s);
        if let Err(e) = s.write(paths) {
            tracing::warn!("saving settings: {e}");
        }
    }

    /// After a successful login: records the account in the settings file,
    /// keeping every other preference as it is on disk now (they may have
    /// changed while the login was in flight).
    pub fn save_login(&self, paths: &Paths) -> anyhow::Result<()> {
        let _one_writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
        match Self::load(paths) {
            Some(mut s) => {
                s.server = self.server.clone();
                s.username = self.username.clone();
                s.signed_out = false;
                s.write(paths)
            }
            None => self.write(paths),
        }
    }

    fn keyring_account(&self) -> String {
        format!("{}@{}", self.username, self.server)
    }

    /// Blocking: may show a Keychain prompt. Call off the UI thread.
    pub fn load_password(&self) -> Option<String> {
        keyring::Entry::new(KEYRING_SERVICE, &self.keyring_account()).ok()?.get_password().ok()
    }

    /// Blocking: may show a Keychain prompt. Call off the UI thread.
    pub fn forget_password(&self) {
        if let Ok(e) = keyring::Entry::new(KEYRING_SERVICE, &self.keyring_account()) {
            let _ = e.delete_credential();
        }
    }

    /// Blocking: may show a Keychain prompt. Call off the UI thread.
    pub fn store_password(&self, password: &str) -> anyhow::Result<()> {
        keyring::Entry::new(KEYRING_SERVICE, &self.keyring_account())?.set_password(password)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(dir: &std::path::Path) -> Paths {
        Paths {
            config_file: dir.join("settings.json"),
            library_db: dir.join("library.redb"),
            stream_cache: dir.join("streams"),
            art_cache: dir.join("art"),
            offline: dir.join("offline"),
        }
    }

    #[test]
    fn a_login_keeps_preferences_changed_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(dir.path());
        let loaded = Settings { server: "https://old".into(), username: "me".into(), ..Default::default() };
        loaded.save(&paths).unwrap();
        // While the login is in flight the user picks a theme and the app clears a move.
        Settings::update(&paths, |s| {
            s.theme = crate::theme::ThemePref::Dark;
            s.offline_move_from = None;
        });
        let login = Settings { server: "https://new".into(), ..loaded };
        login.save_login(&paths).unwrap();
        let now = Settings::load(&paths).unwrap();
        assert_eq!(now.server, "https://new");
        assert_eq!(now.theme, crate::theme::ThemePref::Dark);
    }

    #[test]
    fn concurrent_updates_are_all_kept() {
        let dir = tempfile::tempdir().unwrap();
        let paths = std::sync::Arc::new(paths(dir.path()));
        Settings::default().save(&paths).unwrap();
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let paths = paths.clone();
                std::thread::spawn(move || {
                    for k in 0..10 {
                        Settings::update(&paths, |s| {
                            s.device_modes.insert(format!("dev-{i}-{k}"), "normal".into());
                        });
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let s = Settings::load(&paths).expect("the file is always whole");
        assert_eq!(s.device_modes.len(), 80);
    }
}
