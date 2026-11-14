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

/// Whether Ond looks for a newer version of itself (copies from the GitHub
/// releases only). Room for installing them itself later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateCheck {
    Off,
    /// Says when one is out, with a link to it.
    #[default]
    Notify,
}

/// What playback does offline with a song that isn't downloaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Unplayable {
    /// Skips to the next song that's downloaded.
    #[default]
    Skip,
    /// Stops on it, with a notice.
    Pause,
}

impl Unplayable {
    pub const ALL: [Unplayable; 2] = [Unplayable::Skip, Unplayable::Pause];

    pub fn label(self) -> &'static str {
        match self {
            Unplayable::Skip => "Skip to a downloaded song",
            Unplayable::Pause => "Pause",
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

/// What kind of music source a [`SourceEntry`] describes.
///
/// Unknown kinds (from newer builds or hand edits) fall back to
/// [`SourceKind::Subsonic`] so the rest of the file stays readable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    #[default]
    Subsonic,
    Local,
}

impl<'de> Deserialize<'de> for SourceKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = serde_json::Value::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            Some(s) if s.eq_ignore_ascii_case("local") => SourceKind::Local,
            _ => SourceKind::Subsonic,
        })
    }
}

/// One local music folder configured on a source, paired with a stable key so
/// moving, removing, or reordering folders preserves their track and cover ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FolderEntry {
    pub key: String,
    pub path: PathBuf,
}

impl FolderEntry {
    /// Creates a folder entry with a fresh random key.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { key: accounts::AccountId::new().to_string(), path: path.into() }
    }

    /// Creates a folder entry with an explicit key.
    #[cfg(test)]
    pub fn with_key(key: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self { key: key.into(), path: path.into() }
    }
}

impl<'de> Deserialize<'de> for FolderEntry {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum RawFolder {
            Path(PathBuf),
            Entry {
                #[serde(default)]
                key: String,
                path: PathBuf,
            },
        }
        match RawFolder::deserialize(deserializer)? {
            RawFolder::Path(path) => Ok(Self::new(path)),
            RawFolder::Entry { key, path } => Ok(if key.is_empty() { Self::new(path) } else { Self { key, path } }),
        }
    }
}

/// One configured music source in `settings.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceEntry {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub signed_out: bool,
    #[serde(default = "accounts::AccountId::new")]
    pub id: accounts::AccountId,
    #[serde(default)]
    pub kind: SourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "is_first_color")]
    pub color: accounts::SourceColor,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub server: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<FolderEntry>,
}

impl SourceEntry {
    pub fn subsonic(id: accounts::AccountId, server: impl Into<String>, username: impl Into<String>) -> Self {
        Self {
            id,
            signed_out: false,
            kind: SourceKind::Subsonic,
            name: None,
            color: accounts::SourceColor::default(),
            server: server.into(),
            username: username.into(),
            folders: Vec::new(),
        }
    }

    pub fn local(id: accounts::AccountId, name: Option<String>, color: accounts::SourceColor, folders: Vec<FolderEntry>) -> Self {
        Self { id, signed_out: false, kind: SourceKind::Local, name, color, server: String::new(), username: String::new(), folders }
    }

    /// Configured folder paths without keys, for display and forms.
    pub fn folder_paths(&self) -> Vec<PathBuf> {
        self.folders.iter().map(|f| f.path.clone()).collect()
    }

    /// Configured folders paired with their stable keys for [`scan::scan_folders`].
    pub fn scan_roots(&self) -> Vec<scan::ScanRoot> {
        self.folders.iter().map(|f| scan::ScanRoot::new(f.key.clone(), f.path.clone())).collect()
    }

    fn matches_subsonic(&self, server: &str, username: &str) -> bool {
        self.kind == SourceKind::Subsonic
            && self.username == username
            && self.server.trim().trim_end_matches('/').eq_ignore_ascii_case(server.trim().trim_end_matches('/'))
    }

    fn keyring_account(&self) -> String {
        self.id.to_string()
    }

    fn legacy_keyring_account(&self) -> Option<String> {
        (!self.username.is_empty() && !self.server.is_empty()).then(|| format!("{}@{}", self.username, self.server))
    }

    /// Blocking: may show a Keychain prompt. Call off the UI thread.
    /// Reads the account-id entry first, falling back once to `<user>@<server>`
    /// and migrating it to `<account-id>`.
    pub fn load_password(&self) -> Option<String> {
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, &self.keyring_account())
            && let Ok(pw) = entry.get_password()
        {
            return Some(pw);
        }
        let legacy = self.legacy_keyring_account()?;
        let old = keyring::Entry::new(KEYRING_SERVICE, &legacy).ok()?;
        let pw = old.get_password().ok()?;
        if let Ok(new) = keyring::Entry::new(KEYRING_SERVICE, &self.keyring_account())
            && new.set_password(&pw).is_ok()
        {
            let _ = old.delete_credential();
        }
        Some(pw)
    }

    /// Blocking: may show a Keychain prompt. Call off the UI thread.
    pub fn forget_password(&self) {
        if let Ok(e) = keyring::Entry::new(KEYRING_SERVICE, &self.keyring_account()) {
            let _ = e.delete_credential();
        }
        if let Some(legacy) = self.legacy_keyring_account()
            && let Ok(e) = keyring::Entry::new(KEYRING_SERVICE, &legacy)
        {
            let _ = e.delete_credential();
        }
    }

    /// Blocking: may show a Keychain prompt. Call off the UI thread.
    pub fn store_password(&self, password: &str) -> anyhow::Result<()> {
        keyring::Entry::new(KEYRING_SERVICE, &self.keyring_account())?.set_password(password)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self")]
pub struct Settings {
    /// Configured sources (`subsonic` servers and `local` folders).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<SourceEntry>,
    /// Which sources are active in the UI (defaults to `all: true`).
    #[serde(default, skip_serializing_if = "is_default_scope")]
    pub scope: accounts::Scope,
    /// Combine matching albums and songs in scopes with several sources.
    #[serde(default)]
    pub combine_copies: bool,
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
    /// Offline, what playback does with a song that isn't downloaded.
    #[serde(default)]
    pub offline_unplayable: Unplayable,
    /// Offline by choice: Ond doesn't connect, and only what can play shows.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub work_offline: bool,
    /// Look for a newer Ond.
    #[serde(default)]
    pub updates: UpdateCheck,
    /// A newer version the user chose to skip: it isn't mentioned again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped_update: Option<String>,
    /// Light, dark, or follow the system.
    #[serde(default)]
    pub theme: crate::theme::ThemePref,
    /// How large the interface is drawn, in percent (one of
    /// [`shell::zoom::STEPS`]; any other number reads as the nearest).
    #[serde(default = "shell::zoom::default_percent", deserialize_with = "shell::zoom::lenient")]
    pub zoom: u16,
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
    /// Where "play next" and "add to queue" put tracks, and what playing
    /// something new does with them.
    #[serde(default)]
    pub queue: crate::queue::QueuePrefs,
    /// Experimental visuals (Settings › Experimental), all off by default.
    #[serde(default, skip_serializing_if = "crate::visuals::Experimental::all_off")]
    pub experimental: crate::visuals::Experimental,
}

impl Serialize for Settings {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Settings::serialize(self, serializer)
    }
}

impl<'de> Deserialize<'de> for Settings {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::IntoDeserializer;
        let mut value = serde_json::Value::deserialize(deserializer)?;
        migrate_legacy_value(&mut value);
        // Inner remote impl calls field deserializers directly without recursing.
        #[derive(Deserialize)]
        struct Helper(#[serde(with = "Settings")] Settings);
        Helper::deserialize(value.into_deserializer()).map(|h| h.0).map_err(serde::de::Error::custom)
    }
}

fn migrate_legacy_value(value: &mut serde_json::Value) {
    let Some(obj) = value.as_object_mut() else { return };
    let legacy_account = obj.remove("account");
    let legacy_server = obj.remove("server");
    let legacy_username = obj.remove("username");
    let legacy_name = obj.remove("source_name");
    let legacy_color = obj.remove("source_color");

    let account = legacy_account.and_then(|v| serde_json::from_value::<accounts::AccountId>(v).ok());
    let server = legacy_server.and_then(|v| v.as_str().map(str::to_owned));
    let username = legacy_username.and_then(|v| v.as_str().map(str::to_owned));
    let name = legacy_name.and_then(|v| v.as_str().map(str::to_owned));
    let color = legacy_color.and_then(|v| serde_json::from_value::<accounts::SourceColor>(v).ok());

    let has_server = server.as_deref().is_some_and(|s| !s.is_empty());
    let has_user = username.as_deref().is_some_and(|s| !s.is_empty());
    if account.is_none() && !has_server && !has_user && name.is_none() && color.is_none() {
        return;
    }

    let sources = match obj.get_mut("sources") {
        Some(serde_json::Value::Array(arr)) => arr,
        _ => {
            obj.insert("sources".into(), serde_json::Value::Array(Vec::new()));
            match obj.get_mut("sources") {
                Some(serde_json::Value::Array(arr)) => arr,
                _ => unreachable!(),
            }
        }
    };

    let idx = sources.iter().position(|e| e.get("kind").and_then(|k| k.as_str()).is_none_or(|k| !k.eq_ignore_ascii_case("local")));
    let target = match idx {
        Some(i) => &mut sources[i],
        None => {
            let entry = serde_json::json!({
                "id": account.unwrap_or_else(accounts::AccountId::new),
                "kind": "subsonic",
            });
            sources.push(entry);
            sources.last_mut().expect("pushed entry")
        }
    };

    if let Some(map) = target.as_object_mut() {
        if let Some(id) = account
            && idx.is_some()
        {
            map.insert("id".into(), serde_json::to_value(id).unwrap_or(serde_json::Value::Null));
        }
        if let Some(s) = server {
            map.insert("server".into(), serde_json::Value::String(s));
        }
        if let Some(u) = username {
            map.insert("username".into(), serde_json::Value::String(u));
        }
        if let Some(n) = name {
            map.insert("name".into(), serde_json::Value::String(n));
        }
        if let Some(c) = color {
            map.insert("color".into(), serde_json::to_value(c).unwrap_or(serde_json::Value::Null));
        }
    }
}

fn is_default_scope(scope: &accounts::Scope) -> bool {
    *scope == accounts::Scope::default()
}

/// Each value in `obj` that isn't itself an object, with its path.
fn leaves_of(obj: &serde_json::Map<String, serde_json::Value>, at: &mut Vec<String>, out: &mut Vec<(Vec<String>, serde_json::Value)>) {
    for (key, value) in obj {
        at.push(key.clone());
        match value {
            serde_json::Value::Object(inner) if !inner.is_empty() => leaves_of(inner, at, out),
            _ => out.push((at.clone(), value.clone())),
        }
        at.pop();
    }
}

fn yes() -> bool {
    true
}

fn is_first_color(color: &accounts::SourceColor) -> bool {
    *color == accounts::SourceColor::default()
}

fn full() -> f32 {
    1.0
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            scope: accounts::Scope::default(),
            combine_copies: false,
            stream_cache_mb: None,
            output_device: None,
            device_modes: Default::default(),
            output_match_rate: false,
            last_device: None,
            device_volume_green: true,
            device_volume_db: DbReadout::default(),
            device_change_toast: false,
            offline_unplayable: Unplayable::default(),
            work_offline: false,
            updates: UpdateCheck::default(),
            skipped_update: None,
            device_hidden: Default::default(),
            device_groups: Default::default(),
            theme: Default::default(),
            zoom: shell::zoom::DEFAULT,
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
            queue: Default::default(),
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
        if self.signed_out
            && let Some(entry) = self.primary_mut()
        {
            entry.signed_out = true;
        }
        // "Colors from the album art" grew out of Experimental into a setting.
        if std::mem::take(&mut self.experimental.art_colors) {
            self.adapt_colors = true;
        }
    }

    /// Reads the settings file without changing anything (the window's first
    /// size, before the app starts). `None` when missing or unreadable.
    pub fn read(path: &Path) -> Option<Self> {
        Self::from_json(&std::fs::read(path).ok()?).ok()
    }

    /// Reads a settings file's contents. A value this build doesn't know (a
    /// format a newer Ond added, a typo) costs only that setting. It's logged
    /// and left at its default, and the rest of the file still counts, so
    /// going back a version doesn't reset everything or sign the user out.
    /// `Err` only when the bytes aren't JSON.
    pub fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let value: serde_json::Value = serde_json::from_slice(bytes)?;
        if let Ok(s) = serde_json::from_value(value.clone()) {
            return Ok(s);
        }
        let serde_json::Value::Object(top) = value else {
            return serde_json::from_value(value);
        };
        // Each leaf goes over the defaults in turn and stays only if the
        // whole still reads.
        let mut leaves = Vec::new();
        leaves_of(&top, &mut Vec::new(), &mut leaves);
        let mut kept = serde_json::to_value(Settings::default())?;
        for (path, leaf) in leaves {
            let mut tried = kept.clone();
            crate::managed::set(&mut tried, &path, leaf);
            match serde_json::from_value::<Settings>(tried.clone()) {
                Ok(_) => kept = tried,
                Err(e) => tracing::warn!("setting {}: {e}; using the default", path.join(".")),
            }
        }
        serde_json::from_value(kept)
    }

    pub fn primary(&self) -> Option<&SourceEntry> {
        self.sources.first()
    }

    pub fn primary_mut(&mut self) -> Option<&mut SourceEntry> {
        self.sources.first_mut()
    }

    pub fn subsonic(&self) -> Option<&SourceEntry> {
        self.sources.iter().find(|s| s.kind == SourceKind::Subsonic)
    }

    pub fn subsonic_mut(&mut self) -> Option<&mut SourceEntry> {
        self.sources.iter_mut().find(|s| s.kind == SourceKind::Subsonic)
    }

    pub fn source(&self, id: accounts::AccountId) -> Option<&SourceEntry> {
        self.sources.iter().find(|s| s.id == id)
    }

    pub fn account(&self) -> Option<accounts::AccountId> {
        self.primary().map(|s| s.id)
    }

    pub fn server(&self) -> &str {
        self.subsonic().map_or("", |s| s.server.as_str())
    }

    pub fn username(&self) -> &str {
        self.subsonic().map_or("", |s| s.username.as_str())
    }

    pub fn source_name(&self) -> Option<&str> {
        self.primary().and_then(|s| s.name.as_deref())
    }

    pub fn source_color(&self) -> accounts::SourceColor {
        self.primary().map_or_else(accounts::SourceColor::default, |s| s.color)
    }

    /// Names the source `id`; `None` goes back to its default name.
    pub fn set_source_name(&mut self, id: accounts::AccountId, name: Option<String>) {
        if let Some(entry) = self.sources.iter_mut().find(|s| s.id == id) {
            entry.name = name;
        }
    }

    pub fn set_source_color(&mut self, id: accounts::AccountId, color: accounts::SourceColor) {
        if let Some(entry) = self.sources.iter_mut().find(|s| s.id == id) {
            entry.color = color;
        }
    }

    /// Appends a local-folder source entry with a fresh [`accounts::AccountId`] and next
    /// unused colour, returning the new account id.
    pub fn add_local(&mut self, name: Option<String>, folders: Vec<FolderEntry>) -> accounts::AccountId {
        let id = accounts::AccountId::new();
        let color = accounts::next_color(self.sources.iter().map(|s| s.color));
        self.sources.push(SourceEntry::local(id, name, color, folders));
        id
    }

    /// Puts `id` first in a request's settings snapshot, so the existing
    /// credential and storage helpers address it. Browsing uses `scope`;
    /// changing this order does not stop any other configured source.
    pub fn make_primary(&mut self, id: accounts::AccountId) {
        if let Some(at) = self.sources.iter().position(|s| s.id == id) {
            let entry = self.sources.remove(at);
            self.sources.insert(0, entry);
        }
    }

    /// Replaces a local source's folders.
    pub fn set_folders(&mut self, id: accounts::AccountId, folders: Vec<FolderEntry>) {
        if let Some(entry) = self.sources.iter_mut().find(|s| s.id == id && s.kind == SourceKind::Local) {
            entry.folders = folders;
        }
    }

    /// Takes the source `id` off the list, returning it.
    pub fn remove_source(&mut self, id: accounts::AccountId) -> Option<SourceEntry> {
        let at = self.sources.iter().position(|s| s.id == id)?;
        Some(self.sources.remove(at))
    }

    /// Sets or updates a Subsonic account in this sign-in request, keeping
    /// every other configured source. The requested account goes first.
    ///
    /// The same server and user keep their mirror and downloads, including
    /// after signing out. Compare addresses as the existing mirror does.
    pub fn sign_in(&self, server: &str, username: &str) -> Self {
        let mut next = self.clone();
        let existing = next.sources.iter().find(|s| s.matches_subsonic(server, username)).cloned();
        let entry = match existing {
            Some(mut s) => {
                s.server = server.to_owned();
                s.username = username.to_owned();
                s.signed_out = false;
                s
            }
            None => SourceEntry::subsonic(accounts::AccountId::new(), server, username),
        };
        next.sources.retain(|s| s.id != entry.id);
        next.sources.insert(0, entry);
        next.signed_out = false;
        next
    }

    /// A stable id for the server account (server address and user), used to
    /// keep each account's library mirror and downloads apart. `None` before a
    /// login, and while a source that isn't a server is the one in use. The address is compared without case or a trailing slash.
    pub fn account_id(&self) -> Option<u32> {
        let s = self.primary().filter(|s| s.kind == SourceKind::Subsonic)?;
        let server = s.server.trim().trim_end_matches('/').to_lowercase();
        if server.is_empty() || s.username.is_empty() {
            return None;
        }
        // FNV-1a: stable across builds and Rust versions, unlike `DefaultHasher`.
        let key = format!("{server}\n{}", s.username);
        Some(key.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193)))
    }

    /// Blocking: may show a Keychain prompt. Call off the UI thread.
    pub fn load_password(&self) -> Option<String> {
        self.primary().filter(|s| s.kind == SourceKind::Subsonic)?.load_password()
    }

    /// Blocking: may show a Keychain prompt. Call off the UI thread.
    pub fn forget_password(&self) {
        if let Some(s) = self.subsonic() {
            s.forget_password();
        }
    }

    /// Blocking: may show a Keychain prompt. Call off the UI thread.
    pub fn store_password(&self, password: &str) -> anyhow::Result<()> {
        let Some(s) = self.subsonic() else {
            anyhow::bail!("no Subsonic source configured");
        };
        s.store_password(password)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login(server: &str, user: &str) -> Settings {
        Settings { sources: vec![SourceEntry::subsonic(accounts::AccountId::new(), server, user)], ..Default::default() }
    }

    #[test]
    fn a_zoom_level_off_the_steps_reads_as_the_nearest_and_costs_nothing_else() {
        let read = |json: &str| serde_json::from_str::<Settings>(json).unwrap();
        assert_eq!(read("{}").zoom, 100, "older files have none");
        assert_eq!(read(r#"{ "zoom": 150 }"#).zoom, 150);
        assert_eq!(read(r#"{ "zoom": 133, "theme": "dark" }"#).zoom, 125);
        assert_eq!(read(r#"{ "zoom": 400 }"#).zoom, 200);
        let odd = read(r#"{ "zoom": "huge", "theme": "dark" }"#);
        assert_eq!((odd.zoom, odd.theme), (100, crate::theme::ThemePref::Dark));
    }

    #[test]
    fn the_same_server_and_user_reuse_the_account_even_after_signing_out() {
        let old = login("https://music.example.com", "me").sign_in("https://music.example.com", "me");
        let account = old.account().unwrap();
        assert_eq!(old.sign_in("HTTPS://Music.Example.com/", "me").account(), Some(account));
        assert_ne!(old.sign_in("https://new-address.example.com", "me").account(), Some(account));
        assert_ne!(old.sign_in("https://music.example.com", "someone-else").account(), Some(account));
        let signed_out = Settings { signed_out: true, ..old };
        assert_eq!(signed_out.sign_in("https://music.example.com", "me").account(), Some(account));
        let empty = serde_json::to_value(Settings::default()).unwrap();
        assert!(empty.get("account").is_none() && empty.get("sources").is_none());
    }

    #[test]
    fn the_servers_name_and_colour_stay_with_its_account_and_another_starts_fresh() {
        let mut named = login("https://music.example.com", "me").sign_in("https://music.example.com", "me");
        let id = named.account().unwrap();
        named.set_source_name(id, Some("Home".into()));
        named.set_source_color(id, accounts::SourceColor::new(3));
        let json = serde_json::to_value(&named).unwrap();
        assert_eq!((&json["sources"][0]["name"], &json["sources"][0]["color"]), (&serde_json::json!("Home"), &serde_json::json!(3)));
        assert!(json.get("source_name").is_none() && json.get("source_color").is_none());
        let again = named.sign_in("https://music.example.com/", "me");
        assert_eq!((again.source_name(), again.source_color()), (Some("Home"), accounts::SourceColor::new(3)));
        let other = named.sign_in("https://other.example.com", "me");
        assert_eq!((other.source_name(), other.source_color()), (None, accounts::SourceColor::default()));
        let fresh = serde_json::to_value(Settings::default()).unwrap();
        assert!(fresh.get("sources").is_none(), "nothing written until a source is configured");
    }

    #[test]
    fn old_single_account_settings_read_as_one_subsonic_source_and_save_in_new_shape() {
        let id = accounts::AccountId::new();
        let legacy = serde_json::json!({
            "account": id,
            "server": "https://music.example.com",
            "username": "me",
            "source_name": "Home server",
            "source_color": 2,
        });
        let settings = Settings::from_json(&serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert_eq!(settings.sources.len(), 1);
        let entry = &settings.sources[0];
        assert_eq!(entry.id, id);
        assert_eq!(entry.kind, SourceKind::Subsonic);
        assert_eq!(entry.name.as_deref(), Some("Home server"));
        assert_eq!(entry.color, accounts::SourceColor::new(2));
        assert_eq!((entry.server.as_str(), entry.username.as_str()), ("https://music.example.com", "me"));

        let saved = serde_json::to_value(&settings).unwrap();
        assert!(saved.get("account").is_none());
        assert!(saved.get("server").is_none());
        assert!(saved.get("username").is_none());
        assert!(saved.get("source_name").is_none());
        assert!(saved.get("source_color").is_none());
        assert_eq!(saved["sources"][0]["id"], serde_json::json!(id));
        assert_eq!(saved["sources"][0]["kind"], "subsonic");
    }

    #[test]
    fn sources_list_holds_local_and_subsonic_entries_and_falls_back_on_unknown_kind() {
        let mut s = Settings::default();
        let sub_id = accounts::AccountId::new();
        s.sources.push(SourceEntry::subsonic(sub_id, "https://music.example.com", "me"));
        let local_id = s.add_local(Some("Music folder".into()), vec![FolderEntry::new("/Users/me/Music")]);
        s.scope = accounts::Scope::all();
        s.scope.toggle_chip(sub_id);
        s.scope.toggle_chip(local_id);

        let json = serde_json::to_vec(&s).unwrap();
        let back = Settings::from_json(&json).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.sources[1].kind, SourceKind::Local);
        assert_eq!(back.sources[1].folder_paths(), vec![PathBuf::from("/Users/me/Music")]);

        // An unrecognized kind falls back leniently without dropping the entry,
        // and plain folder path strings get fresh keys.
        let future = serde_json::json!({
            "sources": [
                { "id": local_id, "kind": "plex", "name": "Future", "color": 1, "folders": ["/tmp/music"] }
            ]
        });
        let parsed = Settings::from_json(&serde_json::to_vec(&future).unwrap()).unwrap();
        assert_eq!(parsed.sources.len(), 1);
        assert_eq!(parsed.sources[0].kind, SourceKind::Subsonic);
        assert_eq!(parsed.sources[0].name.as_deref(), Some("Future"));
        assert_eq!(parsed.sources[0].folder_paths(), vec![PathBuf::from("/tmp/music")]);
        assert!(!parsed.sources[0].folders[0].key.is_empty());
    }

    #[test]
    fn the_source_in_use_is_the_first_and_the_others_stay_configured() {
        let server = Settings::default().sign_in("https://music.example.com", "me");
        let server_id = server.account().unwrap();
        assert!(server.account_id().is_some());
        let mut both = server.clone();
        let folder = both.add_local(None, vec![FolderEntry::new("/Users/me/Music")]);
        assert_eq!(both.account(), Some(server_id), "adding a folder doesn't make it the one in use by itself");
        both.make_primary(folder);
        assert_eq!(both.account(), Some(folder));
        assert_eq!(both.sources.len(), 2, "the server stays configured");
        assert_eq!(both.account_id(), None, "no server's downloads while a folder is in use");
        assert_eq!(both.server(), "https://music.example.com", "its address is still there to sign in with");
        both.set_folders(folder, vec![FolderEntry::new("/Volumes/Archive")]);
        assert_eq!(both.source(folder).unwrap().folder_paths(), [PathBuf::from("/Volumes/Archive")]);
        both.set_folders(server_id, vec![FolderEntry::new("/nowhere")]);
        assert!(both.source(server_id).unwrap().folders.is_empty(), "a server has no folders");

        let back = both.sign_in("https://music.example.com/", "me");
        assert_eq!(back.account(), Some(server_id), "signing in to the server makes it the one in use again");
        assert_eq!(back.sources.len(), 2);
        let mut removed = back.clone();
        assert_eq!(removed.remove_source(folder).map(|e| e.kind), Some(SourceKind::Local));
        assert_eq!(removed.sources.len(), 1);
        assert!(removed.remove_source(folder).is_none());
    }

    #[test]
    fn source_helper_exposes_primary_entry_and_keychain_key_uses_account_id() {
        let id = accounts::AccountId::new();
        let entry = SourceEntry::subsonic(id, "https://music.example.com", "me");
        assert_eq!(entry.keyring_account(), id.to_string());
        assert_eq!(entry.legacy_keyring_account().as_deref(), Some("me@https://music.example.com"));
        let s = Settings { sources: vec![entry], ..Default::default() };
        assert_eq!(s.source(id).unwrap().username, "me");
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
        let mut s: Settings =
            serde_json::from_str(r#"{"server": "", "username": "", "experimental": {"backdrop": true, "waveform": true, "spectrum": true}}"#).unwrap();
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
