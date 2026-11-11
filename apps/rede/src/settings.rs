//! Rede's settings: the accounts and the few preferences saved as JSON in the
//! platform's config folder, where the app's files live, and the tokens in the
//! system keyring.
//!
//! One account is shown in the UI, but the file holds a list: the ids, tracks
//! and downloads are all keyed by account already, so a second one is a row in
//! the list rather than a new shape.

use serde::{Deserialize, Serialize};
use sources::AccountId;
use std::path::PathBuf;

/// The keychain service, the same as the bundle id.
const KEYRING_SERVICE: &str = "dev.solidsilver.rede";

/// How the library is ordered. Saved, so the shelf opens as it was left. A
/// name this build doesn't know (a newer Rede's order) reads as the default
/// ([`Settings::from_json`] drops just that setting).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LibrarySort {
    #[default]
    Title,
    Author,
    Added,
    /// The book heard last first; never-heard books after, by title.
    Listened,
    /// The most complete first; unstarted books after, by title.
    Progress,
}

impl LibrarySort {
    pub const ALL: [LibrarySort; 5] = [LibrarySort::Title, LibrarySort::Author, LibrarySort::Added, LibrarySort::Listened, LibrarySort::Progress];

    pub fn label(self) -> &'static str {
        match self {
            LibrarySort::Title => "Title",
            LibrarySort::Author => "Author",
            LibrarySort::Added => "Recently added",
            LibrarySort::Listened => "Recently listened",
            LibrarySort::Progress => "Progress",
        }
    }

    /// Whether the order follows where the books are, as well as what they
    /// are: these are worked out again when a position changes.
    pub fn by_position(self) -> bool {
        matches!(self, LibrarySort::Listened | LibrarySort::Progress)
    }
}

/// Which books the library shows, by how far along they are. Saved, as the
/// order is; an unknown name reads as [`All`](LibraryFilter::All).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LibraryFilter {
    #[default]
    All,
    InProgress,
    NotStarted,
    Finished,
}

impl LibraryFilter {
    pub const ALL: [LibraryFilter; 4] = [LibraryFilter::All, LibraryFilter::InProgress, LibraryFilter::NotStarted, LibraryFilter::Finished];

    pub fn label(self) -> &'static str {
        match self {
            LibraryFilter::All => "All",
            LibraryFilter::InProgress => "In progress",
            LibraryFilter::NotStarted => "Not started",
            LibraryFilter::Finished => "Finished",
        }
    }
}

/// A library the account had at its last sync, remembered so the shelf can be
/// read at the next start before the server answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownLibrary {
    pub id: String,
    pub name: String,
}

/// One Audiobookshelf account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    /// Local and permanent: made when the account is added, and never derived
    /// from the address, which can change.
    pub id: AccountId,
    pub server: String,
    pub username: String,
    /// Stable for this installation, so the server can tell this device's play
    /// sessions apart from another Rede's.
    pub device_id: String,
    /// The book libraries seen at the last sync.
    #[serde(default)]
    pub libraries: Vec<KnownLibrary>,
    /// How the account last signed in, for Settings to say. Unknown for an
    /// account saved before Rede kept it, until it signs in again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_in: Option<SignInMethod>,
}

/// How an account proves who it is: a password typed into Rede, or the
/// server's single sign-on in the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SignInMethod {
    Password,
    OpenId,
}

impl SignInMethod {
    /// How Settings says it: "Signed in with …".
    pub fn phrase(self) -> &'static str {
        match self {
            SignInMethod::Password => "a password",
            SignInMethod::OpenId => "single sign-on (OpenID)",
        }
    }
}

impl Account {
    /// A new account for `server` and `username`, with the ids it keeps.
    pub fn new(server: impl Into<String>, username: impl Into<String>) -> Self {
        Self {
            id: AccountId::new(),
            server: server.into(),
            username: username.into(),
            device_id: uuid::Uuid::new_v4().to_string(),
            libraries: Vec::new(),
            sign_in: None,
        }
    }

    /// What the client tells the server about this device.
    pub fn device(&self) -> abs::DeviceInfo {
        abs::DeviceInfo::new(&self.device_id, env!("CARGO_PKG_VERSION"))
    }

    /// The keyring entry's name: the account's own id, which no other account
    /// shares.
    fn keyring_account(&self) -> String {
        self.id.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// The accounts, oldest first. The UI shows the first for now.
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub theme: shell::theme::ThemePref,
    /// How large the interface is drawn, in percent (one of
    /// [`shell::zoom::STEPS`]; any other number reads as the nearest).
    #[serde(default = "shell::zoom::default_percent", deserialize_with = "shell::zoom::lenient")]
    pub zoom: u16,
    #[serde(default)]
    pub sort: LibrarySort,
    #[serde(default)]
    pub filter: LibraryFilter,
    #[serde(default)]
    pub playback: PlaybackSettings,
    #[serde(default)]
    pub downloads: DownloadSettings,
    #[serde(default)]
    pub window: crate::window::Windows,
    /// The chapters column is shown beside the page. Off until it's asked
    /// for: a first start has nothing playing to put in it.
    #[serde(default)]
    pub show_panel: bool,
    /// The sidebar is folded to its rail of icons. Off, so a first start
    /// shows the places and the account by name.
    #[serde(default)]
    pub sidebar_collapsed: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            accounts: Vec::new(),
            theme: Default::default(),
            zoom: shell::zoom::DEFAULT,
            sort: Default::default(),
            filter: Default::default(),
            playback: Default::default(),
            downloads: Default::default(),
            window: Default::default(),
            show_panel: false,
            sidebar_collapsed: false,
        }
    }
}

/// Download policy; an absent limit keeps downloads until explicitly removed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadSettings {
    pub remove_when_finished: bool,
    pub limit: Option<u64>,
}

/// Listening preferences are separate from engine time: skip and rewind are
/// book seconds, while the speed changes how long those seconds take to hear.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackSettings {
    pub skip_back: u32,
    pub skip_forward: u32,
    pub speed: f32,
    #[serde(default)]
    pub voice_boost: bool,
    #[serde(default)]
    pub skip_silence: bool,
    /// The listening level, 0..=1. The sleep timer's fade scales from it, so
    /// the book comes back to this level rather than to full.
    pub volume: f32,
    /// Silent, keeping `volume` to go back to.
    pub muted: bool,
    pub rewind: u32,
    pub sleep_minutes: u32,
    pub sleep_rewind: u32,
}
impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
            skip_back: 15,
            skip_forward: 30,
            speed: 1.0,
            voice_boost: false,
            skip_silence: false,
            volume: 1.0,
            muted: false,
            rewind: 5,
            sleep_minutes: 30,
            sleep_rewind: 10,
        }
    }
}

impl PlaybackSettings {
    /// What the engine's volume is: the level, or silence while muted.
    pub fn level(self) -> f32 {
        if self.muted { 0.0 } else { self.volume.clamp(0.0, 1.0) }
    }
}

/// Every value in `obj` that isn't itself an object, with its path.
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

/// Puts `value` at the dotted-down `path` of `root`, creating objects on the
/// way.
fn set(root: &mut serde_json::Value, path: &[String], value: serde_json::Value) {
    let mut here = root;
    for (i, key) in path.iter().enumerate() {
        let object = match here {
            serde_json::Value::Object(o) => o,
            other => {
                *other = serde_json::Value::Object(serde_json::Map::new());
                other.as_object_mut().expect("just made an object")
            }
        };
        if i + 1 == path.len() {
            object.insert(key.clone(), value);
            return;
        }
        here = object.entry(key.clone()).or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    }
}

impl Settings {
    /// The account the UI shows, if one was ever signed in.
    pub fn account(&self) -> Option<&Account> {
        self.accounts.first()
    }

    /// Remembers `account` as the one in use: replacing the first account's
    /// details, or adding the first one.
    pub fn put_account(&mut self, account: Account) {
        match self.accounts.first_mut() {
            Some(existing) => *existing = account,
            None => self.accounts.push(account),
        }
    }

    /// The account with `id`, whichever position it holds.
    pub fn account_by_id_mut(&mut self, id: AccountId) -> Option<&mut Account> {
        self.accounts.iter_mut().find(|a| a.id == id)
    }

    /// Reads a settings file's contents. A value this build doesn't know (a
    /// format a newer Rede added, a typo) costs only that setting: it's left at
    /// its default and the rest still counts, so going back a version doesn't
    /// sign the user out. `Err` only when the bytes aren't JSON at all.
    pub fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let value: serde_json::Value = serde_json::from_slice(bytes)?;
        if let Ok(s) = serde_json::from_value(value.clone()) {
            return Ok(s);
        }
        let serde_json::Value::Object(top) = value else {
            return serde_json::from_value(value);
        };
        let mut leaves = Vec::new();
        leaves_of(&top, &mut Vec::new(), &mut leaves);
        let mut kept = serde_json::to_value(Settings::default())?;
        for (path, leaf) in leaves {
            let mut tried = kept.clone();
            set(&mut tried, &path, leaf);
            match serde_json::from_value::<Settings>(tried.clone()) {
                Ok(_) => kept = tried,
                Err(e) => tracing::warn!("setting {}: {e}; using the default", path.join(".")),
            }
        }
        serde_json::from_value(kept)
    }

    /// Blocking: may show a keychain prompt. Call off the UI thread.
    pub fn load_tokens(&self, account: &Account) -> Option<abs::Tokens> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, &account.keyring_account()).ok()?;
        let stored: StoredTokens = serde_json::from_str(&entry.get_password().ok()?).ok()?;
        Some(abs::Tokens::new(stored.access, stored.refresh))
    }

    /// Blocking: may show a keychain prompt. Call off the UI thread.
    pub fn store_tokens(&self, account: &Account, tokens: &abs::Tokens) -> anyhow::Result<()> {
        let stored = StoredTokens { access: tokens.access().to_owned(), refresh: tokens.refresh().to_owned() };
        keyring::Entry::new(KEYRING_SERVICE, &account.keyring_account())?.set_password(&serde_json::to_string(&stored)?)?;
        Ok(())
    }

    /// Blocking: may show a keychain prompt. Call off the UI thread. A missing
    /// entry isn't an error: signing out twice is fine.
    pub fn forget_tokens(&self, account: &Account) {
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, &account.keyring_account()) {
            let _ = entry.delete_credential();
        }
    }
}

/// The keyring's one string for an account: the pair, as JSON. `Debug` isn't
/// derived, so a token can't reach a log by accident.
#[derive(Serialize, Deserialize)]
struct StoredTokens {
    access: String,
    refresh: String,
}

pub struct Paths {
    pub config_file: PathBuf,
    /// One database per account, named after its id.
    pub shelves: PathBuf,
    /// Downloaded cover art, evictable (`~/Library/Caches/...` on macOS).
    pub art_cache: PathBuf,
}

impl Paths {
    /// The platform's folders, made if they aren't there: on macOS
    /// `dev.solidsilver.rede`, the bundle id.
    pub fn resolve() -> anyhow::Result<Self> {
        let dirs = directories::ProjectDirs::from("dev", "solidsilver", "rede").ok_or_else(|| anyhow::anyhow!("no home directory"))?;
        std::fs::create_dir_all(dirs.config_dir())?;
        std::fs::create_dir_all(dirs.data_local_dir())?;
        Ok(Self {
            config_file: dirs.config_dir().join("settings.json"),
            shelves: dirs.data_local_dir().join("accounts"),
            art_cache: dirs.cache_dir().join("art"),
        })
    }

    /// An account's shelf: `accounts/<id>.redb`.
    pub fn shelf(&self, account: &Account) -> PathBuf {
        self.shelves.join(format!("{}.redb", account.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login() -> Settings {
        Settings { accounts: vec![Account::new("https://books.example.com", "sam")], ..Default::default() }
    }

    #[test]
    fn the_volume_is_full_until_it_is_moved_and_survives_a_save() {
        assert_eq!(Settings::default().playback.volume, 1.0, "100%");
        assert!(!Settings::default().playback.muted);
        let old = Settings::from_json(br#"{ "playback": { "speed": 1.5 } }"#).unwrap();
        assert_eq!((old.playback.volume, old.playback.muted), (1.0, false), "an older file has none");
        let mut settings = old;
        settings.playback.volume = 0.45;
        settings.playback.muted = true;
        let read = Settings::from_json(&serde_json::to_vec(&settings).unwrap()).unwrap();
        assert_eq!(read, settings);
        assert_eq!(read.playback.level(), 0.0, "muted is silence, with the level kept");
        assert_eq!(PlaybackSettings { muted: false, ..read.playback }.level(), 0.45);
    }

    #[test]
    fn speech_preferences_default_off_and_survive_a_save() {
        let old = Settings::from_json(br#"{ "playback": { "speed": 1.5 } }"#).unwrap();
        assert!(!old.playback.voice_boost && !old.playback.skip_silence);
        let mut settings = old;
        settings.playback.voice_boost = true;
        settings.playback.skip_silence = true;
        let read = Settings::from_json(&serde_json::to_vec(&settings).unwrap()).unwrap();
        assert_eq!(read, settings);
    }

    #[test]
    fn an_account_keeps_its_ids_across_a_save_and_a_read() {
        let s = login();
        let account = s.account().unwrap().clone();
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        let read = back.account().unwrap();
        assert_eq!(read.id, account.id, "the id names the shelf on disk");
        assert_eq!(read.device_id, account.device_id, "and tells the server who's playing");
        assert!(!read.device_id.is_empty());
        assert_eq!(read.device().client_name, "Rede");
        assert_eq!(read.device().device_id, read.device_id);
    }

    #[test]
    fn an_account_remembers_how_it_signed_in_and_an_older_file_reads_as_unknown() {
        let mut s = login();
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("sign_in"), "an unknown method isn't written");
        assert_eq!(Settings::from_json(json.as_bytes()).unwrap().account().unwrap().sign_in, None);
        s.accounts[0].sign_in = Some(SignInMethod::OpenId);
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains(r#""sign_in":"open-id""#));
        assert_eq!(Settings::from_json(json.as_bytes()).unwrap(), s);
    }

    #[test]
    fn a_new_account_gets_an_id_of_its_own() {
        let a = Account::new("https://one.example.com", "sam");
        let b = Account::new("https://one.example.com", "sam");
        assert_ne!(a.id, b.id, "the same server and user signed in twice are two accounts");
        assert_ne!(a.device_id, b.device_id);
    }

    #[test]
    fn a_value_this_build_does_not_know_costs_only_that_setting() {
        let s = Settings::from_json(br#"{ "accounts": [], "theme": "sepia", "sort": "by-length", "window": 7 }"#).unwrap();
        assert_eq!(s.theme, shell::theme::ThemePref::default());
        assert_eq!(s.sort, LibrarySort::default());
        assert!(s.accounts.is_empty());
    }

    #[test]
    fn the_librarys_order_and_filter_survive_a_save_and_an_unknown_one_reads_as_the_default() {
        for sort in LibrarySort::ALL {
            for filter in LibraryFilter::ALL {
                let s = Settings { sort, filter, ..Default::default() };
                assert_eq!(Settings::from_json(&serde_json::to_vec(&s).unwrap()).unwrap(), s);
            }
        }
        let s = Settings::from_json(br#"{ "sort": "listened", "filter": "in-progress" }"#).unwrap();
        assert_eq!((s.sort, s.filter), (LibrarySort::Listened, LibraryFilter::InProgress));
        let s = Settings::from_json(br#"{ "sort": "by-narrator", "filter": "abandoned", "theme": "dark" }"#).unwrap();
        assert_eq!((s.sort, s.filter, s.theme), (LibrarySort::Title, LibraryFilter::All, shell::theme::ThemePref::Dark));
        assert_eq!(Settings::from_json(br#"{ "sort": "author" }"#).unwrap().filter, LibraryFilter::All, "an older file has no filter");
    }

    #[test]
    fn a_zoom_level_off_the_steps_reads_as_the_nearest_and_costs_nothing_else() {
        assert_eq!(Settings::default().zoom, 100);
        assert_eq!(Settings::from_json(b"{}").unwrap().zoom, 100, "older files have none");
        assert_eq!(Settings::from_json(br#"{ "zoom": 175 }"#).unwrap().zoom, 175);
        assert_eq!(Settings::from_json(br#"{ "zoom": 60, "sort": "author" }"#).unwrap().zoom, 80);
        let odd = Settings::from_json(br#"{ "zoom": [1], "sort": "author" }"#).unwrap();
        assert_eq!((odd.zoom, odd.sort), (100, LibrarySort::Author));
    }

    #[test]
    fn an_older_file_with_only_a_login_reads_as_a_fresh_one() {
        let s = Settings::from_json(br#"{ "accounts": [] }"#).unwrap();
        assert_eq!(s, Settings::default());
        let dark = Settings::from_json(br#"{ "theme": "dark", "sort": "author" }"#).unwrap();
        assert_eq!((dark.theme, dark.sort), (shell::theme::ThemePref::Dark, LibrarySort::Author));
    }

    #[test]
    fn the_account_in_use_is_the_first_and_is_replaced_in_place() {
        let mut s = login();
        let mut again = s.account().unwrap().clone();
        again.username = "alex".into();
        s.put_account(again.clone());
        assert_eq!(s.accounts.len(), 1);
        assert_eq!(s.account().unwrap().username, "alex");
        assert_eq!(s.account_by_id_mut(again.id).map(|a| a.username.as_str()), Some("alex"));
        assert!(s.account_by_id_mut(AccountId::new()).is_none());
    }

    #[test]
    fn an_accounts_shelf_is_its_own_file() {
        let dirs = Paths { config_file: PathBuf::from("/c/settings.json"), shelves: PathBuf::from("/data/accounts"), art_cache: PathBuf::from("/c/art") };
        let a = Account::new("https://one.example.com", "sam");
        let b = Account::new("https://two.example.com", "sam");
        assert_eq!(dirs.shelf(&a), PathBuf::from(format!("/data/accounts/{}.redb", a.id)));
        assert_ne!(dirs.shelf(&a), dirs.shelf(&b));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn folders_are_named_after_the_bundle_id() {
        let dirs = directories::ProjectDirs::from("dev", "solidsilver", "rede").unwrap();
        assert!(dirs.config_dir().ends_with("dev.solidsilver.rede"), "{}", dirs.config_dir().display());
    }
}

#[cfg(test)]
mod download_tests {
    use super::*;

    #[test]
    fn an_older_settings_file_keeps_downloads_and_has_no_storage_limit() {
        let settings = Settings::from_json(b"{}").unwrap();
        assert_eq!(settings.downloads, DownloadSettings { remove_when_finished: false, limit: None });
        let mut settings = settings;
        settings.downloads = DownloadSettings { remove_when_finished: true, limit: Some(1024) };
        assert_eq!(Settings::from_json(&serde_json::to_vec(&settings).unwrap()).unwrap(), settings);
    }
}
