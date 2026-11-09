//! Wire types, in the server's camelCase spelling via `rename_all`.
//!
//! Every field but an id is optional: the official API docs are no longer
//! maintained, servers differ by version, and a missing field is better shown
//! as a gap than as a failed page. Unknown fields are ignored. Lists decode
//! item by item ([`lenient::list`]), so one malformed book doesn't cost the
//! rest of a page.
//!
//! Units are the server's: timestamps are integer milliseconds since the
//! epoch, durations and positions are float seconds.

use serde::{Deserialize, Serialize};

use crate::lenient;

/// `GET /status`, which needs no sign-in.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    /// `"audiobookshelf"`.
    pub app: Option<String>,
    pub server_version: Option<String>,
    /// Whether the server has a root user yet.
    pub is_init: Option<bool>,
    pub language: Option<String>,
    /// `"local"`, `"openid"`.
    #[serde(default, deserialize_with = "lenient::list")]
    pub auth_methods: Vec<String>,
    #[serde(default)]
    pub auth_form_data: AuthFormData,
}

/// The public sign-in settings, with no provider credentials.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuthFormData {
    #[serde(rename = "authOpenIDButtonText")]
    pub auth_open_id_button_text: Option<String>,
    #[serde(default)]
    #[serde(rename = "authOpenIDAutoLaunch")]
    pub auth_open_id_auto_launch: bool,
    pub auth_login_custom_message: Option<String>,
}

impl ServerStatus {
    pub fn version(&self) -> Option<Version> {
        self.server_version.as_deref().and_then(Version::parse)
    }
}

/// A server version, `major.minor.patch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// The first version with access and refresh tokens. Older servers only
    /// hand out a token that never expires, which this crate doesn't use.
    pub const TOKENS: Version = Version(2, 26, 0);
    /// The first version that rejects a refresh token presented as an access
    /// token (GHSA-798w-wv6c-2c3h), and the first with `/api/me/progress` and
    /// `/api/me/bookmarks`.
    pub const REFRESH_FIX: Version = Version(2, 36, 0);

    /// `"2.37.2"`, `"v2.37.2"` or `"2.37"`; anything after the numbers (a
    /// `-beta` suffix) is ignored.
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim().trim_start_matches('v');
        let mut parts = s.split('.').map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>());
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        Some(Version(major, minor, patch))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// What a library holds; decides which side of the app shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MediaType {
    Book,
    Podcast,
    /// A kind a newer server added.
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    pub id: String,
    pub name: Option<String>,
    pub media_type: Option<MediaType>,
    pub display_order: Option<i64>,
    pub icon: Option<String>,
    pub provider: Option<String>,
    #[serde(default, deserialize_with = "lenient::list")]
    pub folders: Vec<Folder>,
    pub last_scan: Option<i64>,
    pub created_at: Option<i64>,
    pub last_update: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub id: String,
    pub full_path: Option<String>,
    pub added_at: Option<i64>,
}

#[derive(Deserialize)]
pub(crate) struct LibrariesReply {
    #[serde(default, deserialize_with = "lenient::list")]
    pub libraries: Vec<Library>,
}

/// One page of `GET /api/libraries/:id/items`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ItemsPage {
    #[serde(default, deserialize_with = "lenient::list")]
    pub results: Vec<LibraryItem>,
    /// Items in the whole library (with the filter applied).
    #[serde(default, deserialize_with = "lenient::opt_int")]
    pub total: Option<u64>,
    /// Echoed from the query string, so the server sends it as a string.
    #[serde(default, deserialize_with = "lenient::opt_int")]
    pub limit: Option<u32>,
    /// 0-based. Echoed from the query string like `limit`.
    #[serde(default, deserialize_with = "lenient::opt_int")]
    pub page: Option<u32>,
    #[serde(default, deserialize_with = "lenient::opt_int")]
    pub offset: Option<u64>,
    pub sort_by: Option<String>,
    pub sort_desc: Option<bool>,
}

/// A library item: a book, or a podcast with its episodes. The listing sends
/// it minified; [`Client::item`](crate::Client::item) sends it expanded, with
/// tracks, chapters and audio files in `media` and the user's progress.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryItem {
    pub id: String,
    #[serde(default, deserialize_with = "lenient::opt_text")]
    pub ino: Option<String>,
    pub library_id: Option<String>,
    pub folder_id: Option<String>,
    pub path: Option<String>,
    pub rel_path: Option<String>,
    pub is_file: Option<bool>,
    pub media_type: Option<MediaType>,
    pub mtime_ms: Option<i64>,
    pub birthtime_ms: Option<i64>,
    pub added_at: Option<i64>,
    /// Not bumped by every media change; see the API notes before diffing on
    /// it alone.
    pub updated_at: Option<i64>,
    pub is_missing: Option<bool>,
    pub is_invalid: Option<bool>,
    pub num_files: Option<u64>,
    pub size: Option<u64>,
    #[serde(default, deserialize_with = "lenient::opt")]
    pub media: Option<BookMedia>,
    /// With `include=progress` on an expanded item.
    #[serde(default, deserialize_with = "lenient::opt")]
    pub user_media_progress: Option<MediaProgress>,
    /// From `items-in-progress`: when the user last moved in it.
    pub progress_last_update: Option<i64>,
}

/// A book's media. The `num_*` counts come with the minified item; the lists
/// only with the expanded one.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BookMedia {
    pub id: Option<String>,
    #[serde(default, deserialize_with = "lenient::opt")]
    pub metadata: Option<BookMetadata>,
    pub cover_path: Option<String>,
    #[serde(default, deserialize_with = "lenient::list")]
    pub tags: Vec<String>,
    pub num_tracks: Option<u32>,
    pub num_audio_files: Option<u32>,
    pub num_chapters: Option<u32>,
    #[serde(default, deserialize_with = "lenient::opt_float")]
    pub duration: Option<f64>,
    pub size: Option<u64>,
    /// Every audio file, excluded ones included.
    #[serde(default, deserialize_with = "lenient::list")]
    pub audio_files: Vec<AudioFile>,
    #[serde(default, deserialize_with = "lenient::list")]
    pub chapters: Vec<Chapter>,
    /// The files that play, in order, with where each starts in the book.
    #[serde(default, deserialize_with = "lenient::list")]
    pub tracks: Vec<AudioTrack>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BookMetadata {
    pub title: Option<String>,
    pub subtitle: Option<String>,
    /// Comma-joined names, as the listing sends them.
    pub author_name: Option<String>,
    /// "Last, First" for sorting.
    #[serde(rename = "authorNameLF")]
    pub author_name_lf: Option<String>,
    pub narrator_name: Option<String>,
    /// "Series #1, Other", as the listing sends it.
    pub series_name: Option<String>,
    /// The expanded item's authors.
    #[serde(default, deserialize_with = "lenient::list")]
    pub authors: Vec<Person>,
    #[serde(default, deserialize_with = "lenient::list")]
    pub narrators: Vec<String>,
    /// The expanded item's series, or the one series of a series filter.
    #[serde(default, deserialize_with = "lenient::list")]
    pub series: Vec<SeriesRef>,
    #[serde(default, deserialize_with = "lenient::list")]
    pub genres: Vec<String>,
    #[serde(default, deserialize_with = "lenient::opt_text")]
    pub published_year: Option<String>,
    pub published_date: Option<String>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub description_plain: Option<String>,
    pub isbn: Option<String>,
    pub asin: Option<String>,
    pub language: Option<String>,
    pub explicit: Option<bool>,
    pub abridged: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Person {
    pub id: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct SeriesRef {
    pub id: String,
    pub name: Option<String>,
    /// "1", "2.5", "Book 3": whatever the metadata said.
    #[serde(default, deserialize_with = "lenient::opt_text")]
    pub sequence: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Chapter {
    #[serde(default, deserialize_with = "lenient::opt_int", skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    #[serde(default, deserialize_with = "lenient::opt_float", skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    #[serde(default, deserialize_with = "lenient::opt_float", skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileMetadata {
    pub filename: Option<String>,
    pub ext: Option<String>,
    /// The path on the server's disk.
    pub path: Option<String>,
    pub rel_path: Option<String>,
    pub size: Option<u64>,
    pub mtime_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioFile {
    pub index: Option<u32>,
    #[serde(default, deserialize_with = "lenient::opt_text")]
    pub ino: Option<String>,
    #[serde(default, deserialize_with = "lenient::opt")]
    pub metadata: Option<FileMetadata>,
    pub added_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub track_num_from_meta: Option<i64>,
    pub disc_num_from_meta: Option<i64>,
    pub exclude: Option<bool>,
    pub error: Option<String>,
    pub format: Option<String>,
    #[serde(default, deserialize_with = "lenient::opt_float")]
    pub duration: Option<f64>,
    pub bit_rate: Option<u64>,
    pub codec: Option<String>,
    pub channels: Option<u32>,
    pub channel_layout: Option<String>,
    pub language: Option<String>,
    #[serde(default, deserialize_with = "lenient::list")]
    pub chapters: Vec<Chapter>,
    pub mime_type: Option<String>,
}

/// One file of a book as it plays: an [`AudioFile`] plus where it starts in
/// the book and the relative URL it's fetched from.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrack {
    /// 1-based for books.
    pub index: Option<u32>,
    #[serde(default, deserialize_with = "lenient::opt_text")]
    pub ino: Option<String>,
    pub title: Option<String>,
    /// Where this file starts in the book, in seconds.
    #[serde(default, deserialize_with = "lenient::opt_float")]
    pub start_offset: Option<f64>,
    #[serde(default, deserialize_with = "lenient::opt_float")]
    pub duration: Option<f64>,
    /// `/api/items/:id/file/:ino`, relative to the server and without a
    /// token; [`Client::file_url`](crate::Client::file_url) makes it fetchable.
    pub content_url: Option<String>,
    pub mime_type: Option<String>,
    pub codec: Option<String>,
    pub bit_rate: Option<u64>,
    #[serde(default, deserialize_with = "lenient::opt")]
    pub metadata: Option<FileMetadata>,
}

impl AudioTrack {
    /// The file's size in bytes, when the server said.
    pub fn size(&self) -> Option<u64> {
        self.metadata.as_ref().and_then(|m| m.size)
    }
}

/// The device a play session or local session comes from. Starting a session
/// closes any other open session of the same user on the same `device_id`,
/// so it must stay the same for an installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub device_id: String,
    pub client_name: String,
    pub client_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    /// Shown in the server's session list ("macOS").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl DeviceInfo {
    pub const CLIENT_NAME: &str = "Rede";

    pub fn new(device_id: impl Into<String>, client_version: impl Into<String>) -> Self {
        Self { device_id: device_id.into(), client_name: Self::CLIENT_NAME.to_owned(), client_version: client_version.into(), manufacturer: None, model: None }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }
}

/// How the server serves a session's audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayMethod {
    DirectPlay,
    DirectStream,
    Transcode,
    Local,
}

impl PlayMethod {
    fn from_code(n: u8) -> Option<Self> {
        Some(match n {
            0 => PlayMethod::DirectPlay,
            1 => PlayMethod::DirectStream,
            2 => PlayMethod::Transcode,
            3 => PlayMethod::Local,
            _ => return None,
        })
    }
}

/// An open play session, from `POST /api/items/:id/play`. Sessions live in
/// the server's memory: after a restart, syncing one answers 404.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackSession {
    pub id: String,
    pub user_id: Option<String>,
    pub library_id: Option<String>,
    pub library_item_id: Option<String>,
    pub episode_id: Option<String>,
    pub media_type: Option<MediaType>,
    pub display_title: Option<String>,
    pub display_author: Option<String>,
    pub cover_path: Option<String>,
    #[serde(default, deserialize_with = "lenient::opt_float")]
    pub duration: Option<f64>,
    #[serde(default, deserialize_with = "lenient::opt_int")]
    play_method: Option<u8>,
    pub media_player: Option<String>,
    #[serde(default, deserialize_with = "lenient::list")]
    pub chapters: Vec<Chapter>,
    #[serde(default, deserialize_with = "lenient::opt")]
    pub media_metadata: Option<BookMetadata>,
    /// Where playback should resume, in seconds.
    #[serde(default, deserialize_with = "lenient::opt_float")]
    pub current_time: Option<f64>,
    #[serde(default, deserialize_with = "lenient::opt_float")]
    pub start_time: Option<f64>,
    #[serde(default, deserialize_with = "lenient::opt_float")]
    pub time_listening: Option<f64>,
    pub started_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub server_version: Option<String>,
    #[serde(default, deserialize_with = "lenient::list")]
    pub audio_tracks: Vec<AudioTrack>,
    #[serde(default, deserialize_with = "lenient::opt")]
    pub library_item: Option<LibraryItem>,
}

impl PlaybackSession {
    pub fn play_method(&self) -> Option<PlayMethod> {
        self.play_method.and_then(PlayMethod::from_code)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlayRequest<'a> {
    pub device_info: &'a DeviceInfo,
    pub supported_mime_types: &'a [String],
    pub force_direct_play: bool,
    pub force_transcode: bool,
    pub media_player: &'a str,
}

/// A report on an open play session.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSync {
    /// The position now, in seconds.
    pub current_time: f64,
    /// Seconds listened since the last sync; added to the listening stats.
    pub time_listened: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
}

/// Listening done without an open play session (offline), reported through
/// `POST /api/session/local`. The server keeps its own progress when that is
/// newer than `updated_at`, which is why `updated_at` isn't optional: a
/// session without it always overwrites.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSession {
    /// A UUID the client makes up; sending the same id again updates the
    /// session.
    pub id: String,
    pub library_item_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    pub start_time: f64,
    pub current_time: f64,
    /// Seconds listened in this session.
    pub time_listening: f64,
    /// Milliseconds since the epoch.
    pub started_at: i64,
    /// When the position last really changed, in milliseconds since the
    /// epoch. This decides whether the server's progress or this session
    /// wins.
    pub updated_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalSessionBody<'a> {
    #[serde(flatten)]
    pub session: &'a LocalSession,
    pub device_info: &'a DeviceInfo,
    pub media_type: MediaType,
    pub play_method: u8,
    pub media_player: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalAllBody<'a> {
    pub device_info: &'a DeviceInfo,
    pub sessions: Vec<LocalSessionBody<'a>>,
}

#[derive(Deserialize)]
pub(crate) struct LocalAllReply {
    #[serde(default, deserialize_with = "lenient::list")]
    pub results: Vec<LocalSyncResult>,
}

/// What became of one session sent to `POST /api/session/local-all`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocalSyncResult {
    pub id: String,
    #[serde(default)]
    pub success: bool,
    /// Whether the session moved the user's progress; `false` when the
    /// server's progress was newer.
    pub progress_synced: Option<bool>,
    pub error: Option<String>,
}

/// The user's position in one book or episode.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaProgress {
    /// The progress row's own id, which is what deleting it takes.
    pub id: String,
    pub user_id: Option<String>,
    pub library_item_id: Option<String>,
    pub episode_id: Option<String>,
    pub media_item_id: Option<String>,
    pub media_item_type: Option<String>,
    #[serde(default, deserialize_with = "lenient::opt_float")]
    pub duration: Option<f64>,
    /// 0 to 1.
    #[serde(default, deserialize_with = "lenient::opt_float")]
    pub progress: Option<f64>,
    #[serde(default, deserialize_with = "lenient::opt_float")]
    pub current_time: Option<f64>,
    pub is_finished: Option<bool>,
    pub hide_from_continue_listening: Option<bool>,
    /// When it last changed, in milliseconds since the epoch.
    pub last_update: Option<i64>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

/// A change to the user's progress in one item. Only the fields that are set
/// are sent. The server overwrites without comparing times, so deciding which
/// write wins is the caller's job, and two of its rules bite: `is_finished:
/// false` on a finished item resets the position to 0 and ignores a
/// `current_time` sent with it, and a position within 10 seconds of the end
/// (with `duration` set) marks the item finished.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_finished: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hide_from_continue_listening: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    /// Stored as the progress's `last_update`, even if older than what's
    /// there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_update: Option<i64>,
}

/// One entry of a batch progress update: which item, and the change.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemProgressUpdate {
    pub library_item_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_id: Option<String>,
    #[serde(flatten)]
    pub update: ProgressUpdate,
}

/// A bookmark, keyed by its item and its time.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Bookmark {
    pub library_item_id: String,
    /// Seconds into the book. Renaming or deleting matches this exactly.
    pub time: f64,
    pub title: Option<String>,
    pub created_at: Option<i64>,
}

#[derive(Serialize)]
pub(crate) struct BookmarkBody<'a> {
    pub time: f64,
    pub title: &'a str,
}

/// The signed-in user as `GET /api/me` sends it. The legacy `token` the
/// server still includes is deliberately not modelled: it never expires, and
/// this crate neither uses nor keeps it.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub username: Option<String>,
    /// `root`, `admin`, `user` or `guest`.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    #[serde(default, deserialize_with = "lenient::list")]
    pub media_progress: Vec<MediaProgress>,
    #[serde(default, deserialize_with = "lenient::list")]
    pub bookmarks: Vec<Bookmark>,
    #[serde(default, deserialize_with = "lenient::list")]
    pub libraries_accessible: Vec<String>,
    #[serde(default, deserialize_with = "lenient::opt")]
    pub permissions: Option<Permissions>,
    pub is_active: Option<bool>,
    pub is_locked: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Permissions {
    pub download: Option<bool>,
    pub update: Option<bool>,
    pub delete: Option<bool>,
    pub upload: Option<bool>,
    pub access_all_libraries: Option<bool>,
    pub access_explicit_content: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ItemsInProgressReply {
    #[serde(default, deserialize_with = "lenient::list")]
    pub library_items: Vec<LibraryItem>,
}
