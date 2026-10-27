//! Wire types. Field names follow the OpenSubsonic JSON spelling via `rename_all`.
//! Everything not guaranteed by the spec is optional so older servers still decode.

use serde::{Deserialize, Serialize};

use crate::lenient;

#[derive(Deserialize)]
pub(crate) struct Envelope {
    #[serde(rename = "subsonic-response")]
    pub response: Body,
}

#[derive(Deserialize)]
pub(crate) struct Body {
    pub status: String,
    pub error: Option<ApiError>,
    #[serde(flatten)]
    pub info: ServerInfo,
    /// Whatever payload key the method returned (`albumList2`, `album`, ...).
    #[serde(flatten)]
    pub rest: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ApiError {
    pub code: i32,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    #[serde(default)]
    pub version: String,
    #[serde(rename = "type")]
    pub server_type: Option<String>,
    pub server_version: Option<String>,
    #[serde(default)]
    pub open_subsonic: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlbumListType {
    AlphabeticalByName,
    AlphabeticalByArtist,
    Newest,
    Recent,
    Frequent,
    Random,
    Starred,
}

impl AlbumListType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AlphabeticalByName => "alphabeticalByName",
            Self::AlphabeticalByArtist => "alphabeticalByArtist",
            Self::Newest => "newest",
            Self::Recent => "recent",
            Self::Frequent => "frequent",
            Self::Random => "random",
            Self::Starred => "starred",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: String,
    pub name: String,
    pub artist: Option<String>,
    pub artist_id: Option<String>,
    pub cover_art: Option<String>,
    #[serde(default, deserialize_with = "lenient::int_or_zero")]
    pub song_count: u32,
    /// Seconds.
    #[serde(default, deserialize_with = "lenient::int_or_zero")]
    pub duration: u32,
    #[serde(default, deserialize_with = "lenient::opt_int")]
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub created: Option<String>,
    /// When the user starred it (absent when not starred).
    pub starred: Option<String>,
    #[serde(default, deserialize_with = "lenient::opt_int")]
    pub play_count: Option<u64>,
    /// The user's 1–5 rating; absent or 0 when unrated.
    #[serde(default, deserialize_with = "lenient::opt_int")]
    pub user_rating: Option<u8>,
    /// OpenSubsonic: sort-friendly name without leading articles.
    pub sort_name: Option<String>,
    /// OpenSubsonic.
    #[serde(default, deserialize_with = "lenient::list")]
    pub record_labels: Vec<Named>,
    /// OpenSubsonic: every genre, where `genre` is only the first.
    #[serde(default, deserialize_with = "lenient::list")]
    pub genres: Vec<Named>,
    /// OpenSubsonic: the album artists as separate entities (the `artist`
    /// string joins them for display).
    #[serde(default, deserialize_with = "lenient::list")]
    pub artists: Vec<ArtistRef>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AlbumWithSongs {
    #[serde(flatten)]
    pub album: Album,
    #[serde(default, deserialize_with = "lenient::list")]
    pub song: Vec<Song>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReplayGain {
    pub track_gain: Option<f32>,
    pub album_gain: Option<f32>,
    pub track_peak: Option<f32>,
    pub album_peak: Option<f32>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Song {
    pub id: String,
    pub title: String,
    pub album: Option<String>,
    pub album_id: Option<String>,
    pub artist: Option<String>,
    pub artist_id: Option<String>,
    #[serde(default, deserialize_with = "lenient::opt_int")]
    pub track: Option<u32>,
    #[serde(default, deserialize_with = "lenient::opt_int")]
    pub disc_number: Option<u32>,
    #[serde(default, deserialize_with = "lenient::opt_int")]
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub cover_art: Option<String>,
    #[serde(default, deserialize_with = "lenient::opt_int")]
    pub size: Option<u64>,
    pub content_type: Option<String>,
    pub suffix: Option<String>,
    /// Seconds.
    #[serde(default, deserialize_with = "lenient::opt_int")]
    pub duration: Option<u32>,
    /// kbps.
    #[serde(default, deserialize_with = "lenient::opt_int")]
    pub bit_rate: Option<u32>,
    /// OpenSubsonic.
    #[serde(default, deserialize_with = "lenient::opt_int")]
    pub bit_depth: Option<u32>,
    /// OpenSubsonic, Hz.
    #[serde(default, deserialize_with = "lenient::opt_int")]
    pub sampling_rate: Option<u32>,
    /// OpenSubsonic.
    #[serde(default, deserialize_with = "lenient::opt_int")]
    pub channel_count: Option<u32>,
    /// OpenSubsonic.
    #[serde(default, deserialize_with = "lenient::opt")]
    pub replay_gain: Option<ReplayGain>,
    /// OpenSubsonic: composers joined for display.
    pub display_composer: Option<String>,
    /// OpenSubsonic: everyone credited besides the main artists, by role.
    #[serde(default, deserialize_with = "lenient::list")]
    pub contributors: Vec<Contributor>,
    /// OpenSubsonic: the track artists as separate entities.
    #[serde(default, deserialize_with = "lenient::list")]
    pub artists: Vec<ArtistRef>,
    /// When the user starred it (absent when not starred).
    pub starred: Option<String>,
    /// The user's 1–5 rating; absent or 0 when unrated.
    #[serde(default, deserialize_with = "lenient::opt_int")]
    pub user_rating: Option<u8>,
    /// How often the user has played it (Navidrome and OpenSubsonic servers).
    #[serde(default, deserialize_with = "lenient::opt_int")]
    pub play_count: Option<u64>,
    /// When the user last played it (RFC 3339), where the server tracks it.
    #[serde(default)]
    pub played: Option<String>,
}

/// `{ "name": ... }` objects: record labels, genres.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct Named {
    pub name: String,
}

/// OpenSubsonic credit: `role` is e.g. "composer", "producer", "performer";
/// `subRole` narrows it, e.g. the instrument for a performer.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Contributor {
    pub role: String,
    pub sub_role: Option<String>,
    pub artist: ArtistRef,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ArtistRef {
    pub id: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: String,
    pub name: String,
    #[serde(default, deserialize_with = "lenient::int_or_zero")]
    pub song_count: u32,
    /// Seconds.
    #[serde(default, deserialize_with = "lenient::int_or_zero")]
    pub duration: u32,
    pub cover_art: Option<String>,
    pub owner: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PlaylistWithSongs {
    #[serde(flatten)]
    pub playlist: Playlist,
    #[serde(default, deserialize_with = "lenient::list")]
    pub entry: Vec<Song>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    pub id: String,
    pub name: String,
    #[serde(default, deserialize_with = "lenient::int_or_zero")]
    pub album_count: u32,
    pub cover_art: Option<String>,
    pub sort_name: Option<String>,
}

/// `getStarred2`: what the user starred, of each kind.
#[derive(Debug, Default, Deserialize)]
pub struct Starred2 {
    #[serde(default, deserialize_with = "lenient::list")]
    pub artist: Vec<Artist>,
    #[serde(default, deserialize_with = "lenient::list")]
    pub album: Vec<Album>,
    #[serde(default, deserialize_with = "lenient::list")]
    pub song: Vec<Song>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Starred2Payload {
    #[serde(default)]
    pub starred2: Starred2,
}

/// What a star applies to; the API takes a different parameter for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StarKind {
    Song,
    Album,
    Artist,
}

impl StarKind {
    pub(crate) fn param(self) -> &'static str {
        match self {
            StarKind::Song => "id",
            StarKind::Album => "albumId",
            StarKind::Artist => "artistId",
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AlbumList2Payload {
    pub album_list2: AlbumList,
}

#[derive(Deserialize)]
pub(crate) struct AlbumList {
    #[serde(default, deserialize_with = "lenient::list")]
    pub album: Vec<Album>,
}

#[derive(Deserialize)]
pub(crate) struct AlbumPayload {
    pub album: AlbumWithSongs,
}

#[derive(Deserialize)]
pub(crate) struct SongPayload {
    pub song: Song,
}

#[derive(Deserialize)]
pub(crate) struct ArtistsPayload {
    pub artists: ArtistIndexes,
}

#[derive(Deserialize)]
pub(crate) struct ArtistIndexes {
    #[serde(default, deserialize_with = "lenient::list")]
    pub index: Vec<ArtistIndex>,
}

#[derive(Deserialize)]
pub(crate) struct ArtistIndex {
    #[serde(default, deserialize_with = "lenient::list")]
    pub artist: Vec<Artist>,
}

/// `search3` results. Navidrome returns everything for an empty query, which
/// makes it a paged bulk export of the library.
#[derive(Debug, Default, Deserialize)]
pub struct SearchResult3 {
    #[serde(default, deserialize_with = "lenient::list")]
    pub artist: Vec<Artist>,
    #[serde(default, deserialize_with = "lenient::list")]
    pub album: Vec<Album>,
    #[serde(default, deserialize_with = "lenient::list")]
    pub song: Vec<Song>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchResult3Payload {
    #[serde(default)]
    pub search_result3: SearchResult3,
}

#[derive(Deserialize)]
pub(crate) struct PlaylistsPayload {
    pub playlists: PlaylistList,
}

#[derive(Deserialize)]
pub(crate) struct PlaylistList {
    #[serde(default, deserialize_with = "lenient::list")]
    pub playlist: Vec<Playlist>,
}

#[derive(Deserialize)]
pub(crate) struct PlaylistPayload {
    pub playlist: PlaylistWithSongs,
}

/// OpenSubsonic structured lyrics: one language / version of a song's words.
/// `start` and `offset` are milliseconds; unsynced lyrics have no `start`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StructuredLyrics {
    pub lang: Option<String>,
    #[serde(default)]
    pub synced: bool,
    /// Shift to apply to every line's start (positive: the words come later).
    #[serde(default, deserialize_with = "lenient::opt_int")]
    pub offset: Option<i64>,
    #[serde(default, deserialize_with = "lenient::list")]
    pub line: Vec<LyricLine>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LyricLine {
    #[serde(default, deserialize_with = "lenient::opt_int")]
    pub start: Option<i64>,
    #[serde(default)]
    pub value: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LyricsListPayload {
    #[serde(default)]
    pub lyrics_list: LyricsList,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LyricsList {
    #[serde(default, deserialize_with = "lenient::list")]
    pub structured_lyrics: Vec<StructuredLyrics>,
}

#[derive(Deserialize, Default)]
pub(crate) struct LyricsPayload {
    #[serde(default)]
    pub lyrics: Option<LyricsText>,
}

#[derive(Deserialize, Default)]
pub(crate) struct LyricsText {
    pub value: Option<String>,
}
