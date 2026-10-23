//! Non-secret settings live in a JSON file; the password lives in the system
//! keychain. The app reads and changes them through [`crate::prefs::Prefs`].

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const KEYRING_SERVICE: &str = "dev.listener.navidrome";

/// How a device plays, as remembered per device. Saved as "normal" /
/// "bit-perfect", as earlier builds wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceMode {
    Normal,
    BitPerfect,
}

impl From<audio::PlaybackMode> for DeviceMode {
    fn from(m: audio::PlaybackMode) -> Self {
        match m {
            audio::PlaybackMode::Normal => DeviceMode::Normal,
            audio::PlaybackMode::BitPerfect => DeviceMode::BitPerfect,
        }
    }
}

impl From<DeviceMode> for audio::PlaybackMode {
    fn from(m: DeviceMode) -> Self {
        match m {
            DeviceMode::Normal => audio::PlaybackMode::Normal,
            DeviceMode::BitPerfect => audio::PlaybackMode::BitPerfect,
        }
    }
}

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
    /// Playback mode the user picked per device UID.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub device_modes: std::collections::HashMap<String, DeviceMode>,
    /// Light, dark, or follow the system.
    #[serde(default)]
    pub theme: crate::theme::ThemePref,
    /// Whether the queue column is open.
    #[serde(default = "yes")]
    pub show_queue: bool,
    /// Full / Player / Mini, and where each was.
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
    /// The diagnostics bar along the bottom (for the technically curious).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub diagnostics_bar: bool,
    /// The sidebar's width as last dragged; `None` is the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_width: Option<f32>,
    /// The queue column's width as last dragged; `None` is the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_width: Option<f32>,
    /// Software volume, 0..=1 (unused in bit-perfect mode).
    #[serde(default = "full")]
    pub volume: f32,
}

fn yes() -> bool {
    true
}

fn full() -> f32 {
    1.0
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
            diagnostics_bar: false,
            sidebar_width: None,
            queue_width: None,
            volume: 1.0,
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
    /// Reads the settings file without changing anything (the window's first
    /// size, before the app starts). `None` when missing or unreadable.
    pub fn read(path: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
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
