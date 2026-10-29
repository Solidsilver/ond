//! Non-secret settings live in a JSON file; the password lives in the system
//! keychain. The app reads and changes them through [`crate::prefs::Prefs`].

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The Keychain service, the same as the bundle id.
const KEYRING_SERVICE: &str = "dev.solidsilver.ond";

/// How a page of albums or artists lays them out: tiles, or a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ViewMode {
    #[default]
    Grid,
    List,
}

/// When the volume slider shows the device's own level in dB (bit-perfect,
/// on a device with a volume control).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DbReadout {
    /// While the slider is hovered or dragged.
    #[default]
    Adjusting,
    Always,
    Never,
}

impl DbReadout {
    pub const ALL: [DbReadout; 3] = [DbReadout::Adjusting, DbReadout::Always, DbReadout::Never];

    pub fn label(self) -> &'static str {
        match self {
            DbReadout::Adjusting => "While adjusting",
            DbReadout::Always => "Always",
            DbReadout::Never => "Never",
        }
    }
}

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub server: String,
    pub username: String,
    /// Size cap for the streaming cache; `None` uses the default (2 GB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_cache_mb: Option<u64>,
    /// Output device UID; `None` follows the system default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_device: Option<String>,
    /// The device last chosen, kept while following the system: "Choose a
    /// device" goes back to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_device: Option<String>,
    /// Playback mode the user picked per device UID.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub device_modes: std::collections::HashMap<String, DeviceMode>,
    /// Normal mode switches the device to each song's sample rate. Off, the
    /// device stays as the system has it and Ond converts the rate.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub output_match_rate: bool,
    /// Bit-perfect: the volume slider's knob has a green dot while it sets the device's own volume.
    #[serde(default = "yes")]
    pub device_volume_green: bool,
    /// Bit-perfect: when the device's own level shows in dB beside the slider.
    #[serde(default)]
    pub device_volume_db: DbReadout,
    /// Devices left out of the output menu, by UID (Settings › Output lists them all).
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub device_hidden: std::collections::BTreeSet<String>,
    /// Groups the user put devices in, by UID, for the output menu. Devices
    /// not listed go in the default groups (hardware, then "Other").
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub device_groups: std::collections::HashMap<String, String>,
    /// A toast names the output device whenever playback moves to another.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub device_change_toast: bool,
    /// Light, dark, or follow the system.
    #[serde(default)]
    pub theme: crate::theme::ThemePref,
    /// The accent color, used whenever the album art doesn't give one.
    #[serde(default, deserialize_with = "crate::theme::lenient_accent")]
    pub accent: crate::theme::Accent,
    /// Colors adapt to the album art: the accent, the now-playing display and
    /// the Player take the playing cover's, and an album's page its own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub adapt_colors: bool,
    /// A blurred copy of the cover behind the Player and Mini.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub backdrop: bool,
    /// The track's waveform as the seek bar, and how its bars look.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub waveform: bool,
    #[serde(default)]
    pub waveform_bars: crate::visuals::waveform::Bars,
    /// Letters beside long lists sorted by name, to jump along them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub alpha_rail: bool,
    /// Artists' photos from the server in the Artists list.
    #[serde(default = "yes")]
    pub artist_photos: bool,
    /// Grid or list, for the pages of albums and for Artists.
    #[serde(default)]
    pub album_view: ViewMode,
    #[serde(default)]
    pub artist_view: ViewMode,
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
    /// Experimental visuals (Settings › Experimental), all off by default.
    #[serde(default, skip_serializing_if = "crate::visuals::Experimental::all_off")]
    pub experimental: crate::visuals::Experimental,
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
            output_match_rate: false,
            last_device: None,
            device_volume_green: true,
            device_volume_db: DbReadout::default(),
            device_change_toast: false,
            device_hidden: Default::default(),
            device_groups: Default::default(),
            theme: Default::default(),
            accent: Default::default(),
            adapt_colors: false,
            backdrop: false,
            waveform: false,
            waveform_bars: Default::default(),
            alpha_rail: false,
            artist_photos: true,
            album_view: ViewMode::Grid,
            artist_view: ViewMode::Grid,
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
            experimental: Default::default(),
        }
    }
}

/// Where downloads go for a folder the user picked: always a folder of
/// Ond's own inside it, since the store tidies away files it doesn't know,
/// and those must never be the user's.
pub fn offline_folder(chosen: PathBuf) -> PathBuf {
    if chosen.file_name().is_some_and(|n| n == "Ond Offline") { chosen } else { chosen.join("Ond Offline") }
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
        // On macOS the folders are named `dev.solidsilver.ond`, the bundle id.
        // The library and downloads go in the local (not roaming) folder on
        // Windows; elsewhere that's the same as the data folder.
        let dirs = directories::ProjectDirs::from("dev", "solidsilver", "ond").ok_or_else(|| anyhow::anyhow!("no home directory"))?;
        std::fs::create_dir_all(dirs.config_dir())?;
        std::fs::create_dir_all(dirs.data_local_dir())?;
        Ok(Self {
            config_file: dirs.config_dir().join("settings.json"),
            library_db: dirs.data_local_dir().join("library.redb"),
            stream_cache: dirs.cache_dir().join("streams"),
            art_cache: dirs.cache_dir().join("art"),
            offline: dirs.data_local_dir().join("Offline"),
        })
    }
}

impl Settings {
    /// Carries settings from older builds over to where they live now.
    pub fn migrate(&mut self) {
        // "Colors from the album art" grew out of Experimental into a setting.
        if std::mem::take(&mut self.experimental.art_colors) {
            self.adapt_colors = true;
        }
    }

    /// Reads the settings file without changing anything (the window's first
    /// size, before the app starts). `None` when missing or unreadable.
    pub fn read(path: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }

    /// A stable id for the server account (server address and user), used to
    /// keep each account's library mirror and downloads apart. `None` before a
    /// login. The address is compared without case or a trailing slash.
    pub fn account_id(&self) -> Option<u32> {
        let server = self.server.trim().trim_end_matches('/').to_lowercase();
        if server.is_empty() || self.username.is_empty() {
            return None;
        }
        // FNV-1a: stable across builds and Rust versions, unlike `DefaultHasher`.
        let key = format!("{server}\n{}", self.username);
        Some(key.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193)))
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

    fn login(server: &str, user: &str) -> Settings {
        Settings { server: server.into(), username: user.into(), ..Default::default() }
    }

    #[test]
    fn the_account_id_is_stable_and_tells_accounts_apart() {
        let a = login("https://music.example.com/", "luke").account_id();
        assert!(a.is_some());
        assert_eq!(a, login("HTTPS://Music.Example.com", "luke").account_id());
        assert_ne!(a, login("https://music.example.com", "sam").account_id());
        assert_ne!(a, login("https://other.example.com", "luke").account_id());
        assert_eq!(login("", "luke").account_id(), None);
        // Fixed across builds: downloads live in a folder named after it.
        assert_eq!(login("http://localhost:4533", "admin").account_id(), Some(0x5cd1_2c42));
    }

    #[test]
    fn colors_are_saved_and_read_back() {
        use crate::theme::Accent;
        let s = Settings::default();
        assert_eq!((s.accent, s.adapt_colors), (Accent::Blue, false), "the design's own blue, not adapting");
        for accent in [Accent::Teal, Accent::Custom([12, 200, 99])] {
            let s = Settings { accent, adapt_colors: true, ..Default::default() };
            let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
            assert_eq!((back.accent, back.adapt_colors), (accent, true));
        }
        // An older file has neither; a newer one may name a preset this build lacks.
        let old: Settings = serde_json::from_str(r#"{"server": "", "username": ""}"#).unwrap();
        assert_eq!((old.accent, old.adapt_colors), (Accent::Blue, false));
        let newer: Settings = serde_json::from_str(r#"{"server": "", "username": "", "accent": "chartreuse", "theme": "dark"}"#).unwrap();
        assert_eq!(newer.accent, Accent::Blue);
        assert_eq!(newer.theme, crate::theme::ThemePref::Dark, "the rest still reads");
    }

    #[test]
    fn the_experimental_art_colors_switch_becomes_the_setting() {
        let mut s: Settings = serde_json::from_str(r#"{"server": "", "username": "", "experimental": {"art_colors": true, "backdrop": true}}"#).unwrap();
        s.migrate();
        assert!(s.adapt_colors);
        assert!(!s.experimental.art_colors);
        let saved = serde_json::to_string(&s).unwrap();
        assert!(!saved.contains("art_colors"), "{saved}");
    }

    #[test]
    fn appearance_switches_start_off_and_experiments_that_moved_start_over() {
        let fresh = Settings::default();
        assert!(!fresh.backdrop && !fresh.waveform && !fresh.adapt_colors && !fresh.alpha_rail && fresh.experimental.all_off());
        // An experiment's switch isn't carried over when the feature moves
        // out of Experimental: the old key is dropped, the rest still reads.
        let mut s: Settings = serde_json::from_str(r#"{"server": "", "username": "", "experimental": {"backdrop": true, "waveform": true, "spectrum": true}}"#).unwrap();
        s.migrate();
        assert!(!s.backdrop && !s.waveform && s.experimental.spectrum);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn folders_are_named_after_the_bundle_id() {
        let dirs = directories::ProjectDirs::from("dev", "solidsilver", "ond").unwrap();
        assert!(dirs.data_dir().ends_with("dev.solidsilver.ond"), "{}", dirs.data_dir().display());
        assert!(dirs.cache_dir().ends_with("dev.solidsilver.ond"), "{}", dirs.cache_dir().display());
    }
}
