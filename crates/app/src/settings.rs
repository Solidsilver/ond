//! Non-secret settings live in a JSON file; the password lives in the system keychain.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const KEYRING_SERVICE: &str = "dev.listener.navidrome";

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
    /// Where offline downloads live; `None` uses the default (next to the library).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline_dir: Option<PathBuf>,
    /// Cap on offline downloads, in GB; `None` is unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline_limit_gb: Option<u64>,
    /// Downloads to move into `offline_dir` at the next launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline_move_from: Option<PathBuf>,
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
            offline_dir: None,
            offline_limit_gb: None,
            offline_move_from: None,
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

    pub fn save(&self, paths: &Paths) -> anyhow::Result<()> {
        std::fs::write(&paths.config_file, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    /// Loads, changes and saves the settings file (UI preferences the user just
    /// changed). Does nothing before the first login has written the file.
    pub fn update(paths: &Paths, change: impl FnOnce(&mut Settings)) {
        let Some(mut s) = Self::load(paths) else { return };
        change(&mut s);
        if let Err(e) = s.save(paths) {
            tracing::warn!("saving settings: {e}");
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
    pub fn store_password(&self, password: &str) -> anyhow::Result<()> {
        keyring::Entry::new(KEYRING_SERVICE, &self.keyring_account())?.set_password(password)?;
        Ok(())
    }
}
