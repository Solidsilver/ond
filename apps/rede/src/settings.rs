//! Rede's settings: the accounts and the few preferences saved as JSON in the
//! platform's config folder, where the app's files live, and the tokens in the
//! system keyring.
//!
//! All accounts live in the same list, each keyed by its local [`AccountId`]
//! with its own shelf and downloads folder; `scope` remembers which one is
//! open.

pub use shell::updates::{UpdateChannel, UpdateCheck};

use accounts::{AccountId, Scope, SourceColor};
use serde::{Deserialize, Serialize};
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

/// Which books the library shows: by how far along they are, or those on
/// this computer. Saved, as the order is; an unknown name reads as
/// [`All`](LibraryFilter::All).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LibraryFilter {
    #[default]
    All,
    InProgress,
    NotStarted,
    Finished,
    /// Every file of the book is on this computer.
    Downloaded,
}

impl LibraryFilter {
    pub const ALL: [LibraryFilter; 5] =
        [LibraryFilter::All, LibraryFilter::InProgress, LibraryFilter::NotStarted, LibraryFilter::Finished, LibraryFilter::Downloaded];

    pub fn label(self) -> &'static str {
        match self {
            LibraryFilter::All => "All",
            LibraryFilter::InProgress => "In progress",
            LibraryFilter::NotStarted => "Not started",
            LibraryFilter::Finished => "Finished",
            LibraryFilter::Downloaded => "Downloaded",
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
    /// The dot colour shown when more than one account is configured.
    #[serde(default)]
    pub color: SourceColor,
    /// What the user renamed the account to in Settings' Sources tab;
    /// without one it goes by its username.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// True after signing out so the row stays in the switcher until the user
    /// signs back in or removes it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub signed_out: bool,
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
            color: SourceColor::default(),
            name: None,
            signed_out: false,
        }
    }

    /// What the account is called: the name it was given, else its
    /// username.
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.username)
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
    /// Look for newer Rede versions (GitHub distributions only).
    #[serde(default)]
    pub updates: UpdateCheck,
    /// Absent follows the installed build's release channel.
    #[serde(default, deserialize_with = "shell::updates::lenient_update_channel", skip_serializing_if = "Option::is_none")]
    pub update_channel: Option<UpdateChannel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped_update: Option<String>,
    /// The accounts, oldest first.
    #[serde(default)]
    pub accounts: Vec<Account>,
    /// Which account is open. Rede shows one source at a time, so `only` names
    /// the active account; `all` (from an older file) or a missing id falls
    /// back leniently to the first account.
    #[serde(default)]
    pub scope: Scope,
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
            updates: UpdateCheck::default(),
            update_channel: None,
            skipped_update: None,
            accounts: Vec::new(),
            scope: Scope::default(),
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

fn same_server(a: &str, b: &str) -> bool {
    a.trim().trim_end_matches('/').eq_ignore_ascii_case(b.trim().trim_end_matches('/'))
}

impl Settings {
    /// The active account named by `scope`, falling back to the first account
    /// when `scope` is still `all` (an older file) or names an account that was
    /// removed.
    pub fn account(&self) -> Option<&Account> {
        self.scope.single().and_then(|id| self.account_by_id(id)).or_else(|| self.accounts.first())
    }

    /// The account with `id`, whichever position it holds.
    pub fn account_by_id(&self, id: AccountId) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    /// The existing account for `(server, username)`, ignoring trailing slashes
    /// and case differences in the server URL.
    pub fn find_account(&self, server: &str, username: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.username == username && same_server(&a.server, server))
    }

    /// Reuses an existing account for `(server, username)` (keeping its id,
    /// device id, shelf, downloads and colour) or builds a new one with the
    /// next colour over existing accounts.
    pub fn account_for_sign_in(&self, server: impl Into<String>, username: impl Into<String>, sign_in: Option<SignInMethod>) -> Account {
        let server = server.into();
        let username = username.into();
        if let Some(existing) = self.find_account(&server, &username) {
            Account { server, username, sign_in, signed_out: false, ..existing.clone() }
        } else {
            let color = accounts::next_color(self.accounts.iter().map(|a| a.color));
            Account { color, sign_in, ..Account::new(server, username) }
        }
    }

    /// Stores `account` (updating the row with the same id in place, or adding
    /// a new row) and makes it the active account.
    pub fn put_account(&mut self, account: Account) {
        let id = account.id;
        match self.account_by_id_mut(id) {
            Some(existing) => *existing = account,
            None => self.accounts.push(account),
        }
        self.scope.choose_one(id);
    }

    /// Records whether `id` is signed out while keeping its row and local data.
    pub fn set_signed_out(&mut self, id: AccountId, signed_out: bool) {
        if let Some(existing) = self.account_by_id_mut(id) {
            existing.signed_out = signed_out;
        }
    }

    /// Gives `id` the name `name`; its username, or nothing, takes the given
    /// name away.
    pub fn rename_account(&mut self, id: AccountId, name: &str) {
        if let Some(existing) = self.account_by_id_mut(id) {
            let name = name.trim();
            existing.name = (!name.is_empty() && name != existing.username).then(|| name.to_owned());
        }
    }

    /// Gives `id` the dot colour `color`.
    pub fn set_color(&mut self, id: AccountId, color: SourceColor) {
        if let Some(existing) = self.account_by_id_mut(id) {
            existing.color = color;
        }
    }

    /// Removes `id` from `accounts` and `scope`, picking the first remaining
    /// account when the removed account was active.
    pub fn remove_account(&mut self, id: AccountId) -> Option<Account> {
        let index = self.accounts.iter().position(|a| a.id == id)?;
        let removed = self.accounts.remove(index);
        self.scope.drop_account(id);
        if self.scope.single().and_then(|active| self.account_by_id(active)).is_none()
            && let Some(first) = self.accounts.first()
        {
            self.scope.choose_one(first.id);
        }
        Some(removed)
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

    /// An account's downloads folder: `downloads/<account_hex>`.
    pub fn downloads(&self, account: &Account) -> PathBuf {
        self.shelves.with_file_name("downloads").join(format!("{:08x}", account.id.as_uuid().as_u128()))
    }

    /// Deletes an inactive account's shelf database and downloaded files if
    /// present; missing files are ignored.
    pub fn purge_account(&self, account: &Account) -> std::io::Result<()> {
        match std::fs::remove_file(self.shelf(account)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        match std::fs::remove_dir_all(self.downloads(account)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_preferences_round_trip_betas_and_unknown_channels_follow_the_build() {
        use shell::updates::{UpdateChannel, UpdateCheck, Version};
        let s = super::Settings::from_json(br#"{"updates":"off","update_channel":"betas","skipped_update":"0.1.0-beta.2"}"#).unwrap();
        assert_eq!(s.updates, UpdateCheck::Off);
        assert_eq!(s.update_channel, Some(UpdateChannel::Betas));
        let back = super::Settings::from_json(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(back.update_channel, s.update_channel);
        assert_eq!(back.skipped_update, s.skipped_update);
        assert_eq!(back.skipped_update.as_deref().and_then(Version::parse).unwrap().to_string(), "0.1.0-beta.2");
        for channel in [r#""nightly""#, "42", "{}", "null"] {
            let json = format!(r#"{{"update_channel":{channel},"skipped_update":"0.1.0-beta.2"}}"#);
            let read: super::Settings = serde_json::from_str(&json).unwrap();
            assert_eq!(read.update_channel, None);
            assert_eq!(read.skipped_update.as_deref(), Some("0.1.0-beta.2"));
        }
        let future = super::Settings::from_json(br#"{"updates":"install","update_channel":"betas"}"#).unwrap();
        assert_eq!(future.updates, UpdateCheck::Notify);
        assert_eq!(future.update_channel, Some(UpdateChannel::Betas));
        assert_eq!(super::Settings::from_json(b"{}").unwrap().update_channel, None);
    }

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
    fn signing_in_again_reuses_the_matching_account_and_a_second_adds_one() {
        let mut s = login();
        let first_id = s.account().unwrap().id;
        let first_device = s.account().unwrap().device_id.clone();

        let reused = s.account_for_sign_in("HTTPS://BOOKS.EXAMPLE.COM/", "sam", Some(SignInMethod::Password));
        assert_eq!(reused.id, first_id);
        assert_eq!(reused.device_id, first_device);
        assert_eq!(reused.color, SourceColor::new(0));
        assert!(!reused.signed_out);
        s.put_account(reused);
        assert_eq!(s.accounts.len(), 1);
        assert_eq!(s.scope, Scope::only(first_id));

        let other = s.account_for_sign_in("https://other.example.com", "sam", Some(SignInMethod::OpenId));
        assert_ne!(other.id, first_id);
        assert_eq!(other.color, SourceColor::new(1));
        s.put_account(other.clone());
        assert_eq!(s.accounts.len(), 2);
        assert_eq!(s.scope, Scope::only(other.id));
        assert_eq!(s.account().map(|a| a.id), Some(other.id));

        assert_eq!(s.account_by_id_mut(other.id).map(|a| a.username.as_str()), Some("sam"));
        assert!(s.account_by_id_mut(AccountId::new()).is_none());
    }

    #[test]
    fn an_old_settings_file_without_scope_opens_on_its_first_account_and_active_scope_survives_a_save() {
        let legacy_first = Account::new("https://one.example.com", "sam");
        let legacy_json = serde_json::json!({
            "accounts": [{
                "id": legacy_first.id,
                "server": legacy_first.server,
                "username": legacy_first.username,
                "device_id": legacy_first.device_id,
            }]
        });
        let loaded = Settings::from_json(&serde_json::to_vec(&legacy_json).unwrap()).unwrap();
        assert_eq!(loaded.account().map(|a| a.id), Some(legacy_first.id));
        assert_eq!(loaded.accounts[0].color, SourceColor::default());
        assert!(!loaded.accounts[0].signed_out);

        let mut s = loaded;
        let second = s.account_for_sign_in("https://two.example.com", "alex", Some(SignInMethod::Password));
        s.put_account(second.clone());
        let saved = serde_json::to_vec(&s).unwrap();
        let reopened = Settings::from_json(&saved).unwrap();
        assert_eq!(reopened.scope, Scope::only(second.id));
        assert_eq!(reopened.account().map(|a| a.id), Some(second.id));

        let stale = Settings { scope: Scope::only(AccountId::new()), ..reopened };
        assert_eq!(stale.account().map(|a| a.id), Some(legacy_first.id), "an unknown active id falls back to the first account");
    }

    #[test]
    fn removing_an_account_drops_it_from_settings_and_deletes_its_local_files() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths { config_file: dir.path().join("settings.json"), shelves: dir.path().join("accounts"), art_cache: dir.path().join("art") };
        std::fs::create_dir_all(&paths.shelves).unwrap();

        let mut s = login();
        let first = s.account().unwrap().clone();
        let second = s.account_for_sign_in("https://two.example.com", "sam", None);
        s.put_account(first.clone());
        s.put_account(second.clone());

        std::fs::write(paths.shelf(&second), b"shelf").unwrap();
        std::fs::create_dir_all(paths.downloads(&second)).unwrap();
        std::fs::write(paths.downloads(&second).join("book.mp3"), b"audio").unwrap();

        let removed = s.remove_account(second.id).unwrap();
        assert_eq!(removed.id, second.id);
        assert_eq!(s.accounts.len(), 1);
        assert_eq!(s.account().map(|a| a.id), Some(first.id));

        paths.purge_account(&removed).unwrap();
        assert!(!paths.shelf(&removed).exists());
        assert!(!paths.downloads(&removed).exists());
        paths.purge_account(&removed).unwrap();
    }

    #[test]
    fn a_renamed_account_keeps_its_name_across_a_save_and_its_username_takes_it_back() {
        let mut s = login();
        let id = s.accounts[0].id;
        assert_eq!(s.accounts[0].display_name(), "sam");
        assert!(!serde_json::to_string(&s).unwrap().contains("\"name\""), "no name is written until one is given");
        s.rename_account(id, "  Home  ");
        s.set_color(id, SourceColor::new(4));
        let read = Settings::from_json(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!((read.accounts[0].display_name(), read.accounts[0].color), ("Home", SourceColor::new(4)));
        let again = read.account_for_sign_in("https://books.example.com", "sam", None);
        assert_eq!(again.display_name(), "Home", "signing in again keeps the name");
        s.rename_account(id, "sam");
        assert_eq!(s.accounts[0].name, None);
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
