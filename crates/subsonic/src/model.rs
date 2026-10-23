//! Wire types. Field names follow the OpenSubsonic JSON spelling via `rename_all`.
//! Everything not guaranteed by the spec is optional so older servers still decode.

use serde::{Deserialize, Serialize};

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
    #[serde(default)]
    pub song_count: u32,
    /// Seconds.
    #[serde(default)]
    pub duration: u32,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub created: Option<String>,
    pub starred: Option<String>,
    pub play_count: Option<u64>,
    /// OpenSubsonic: sort-friendly name without leading articles.
    pub sort_name: Option<String>,
    /// OpenSubsonic.
    #[serde(default)]
    pub record_labels: Vec<Named>,
    /// OpenSubsonic: every genre, where `genre` is only the first.
    #[serde(default)]
    pub genres: Vec<Named>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AlbumWithSongs {
    #[serde(flatten)]
    pub album: Album,
    #[serde(default)]
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
    pub track: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub cover_art: Option<String>,
    pub size: Option<u64>,
    pub content_type: Option<String>,
    pub suffix: Option<String>,
    /// Seconds.
    pub duration: Option<u32>,
    /// kbps.
    pub bit_rate: Option<u32>,
    /// OpenSubsonic.
    pub bit_depth: Option<u32>,
    /// OpenSubsonic, Hz.
    pub sampling_rate: Option<u32>,
    /// OpenSubsonic.
    pub channel_count: Option<u32>,
    /// OpenSubsonic.
    pub replay_gain: Option<ReplayGain>,
    /// OpenSubsonic: composers joined for display.
    pub display_composer: Option<String>,
    /// OpenSubsonic: everyone credited besides the main artists, by role.
    #[serde(default)]
    pub contributors: Vec<Contributor>,
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
    #[serde(default)]
    pub song_count: u32,
    /// Seconds.
    #[serde(default)]
    pub duration: u32,
    pub cover_art: Option<String>,
    pub owner: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PlaylistWithSongs {
    #[serde(flatten)]
    pub playlist: Playlist,
    #[serde(default)]
    pub entry: Vec<Song>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub album_count: u32,
    pub cover_art: Option<String>,
    pub sort_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AlbumList2Payload {
    pub album_list2: AlbumList,
}

#[derive(Deserialize)]
pub(crate) struct AlbumList {
    #[serde(default)]
    pub album: Vec<Album>,
}

#[derive(Deserialize)]
pub(crate) struct AlbumPayload {
    pub album: AlbumWithSongs,
}

#[derive(Deserialize)]
pub(crate) struct ArtistsPayload {
    pub artists: ArtistIndexes,
}

#[derive(Deserialize)]
pub(crate) struct ArtistIndexes {
    #[serde(default)]
    pub index: Vec<ArtistIndex>,
}

#[derive(Deserialize)]
pub(crate) struct ArtistIndex {
    #[serde(default)]
    pub artist: Vec<Artist>,
}

#[derive(Deserialize)]
pub(crate) struct PlaylistsPayload {
    pub playlists: PlaylistList,
}

#[derive(Deserialize)]
pub(crate) struct PlaylistList {
    #[serde(default)]
    pub playlist: Vec<Playlist>,
}

#[derive(Deserialize)]
pub(crate) struct PlaylistPayload {
    pub playlist: PlaylistWithSongs,
}
