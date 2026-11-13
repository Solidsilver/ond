//! The music library as the app asks for it. Callers read capabilities and
//! ask the source, never which kind it is: a Subsonic server (paged servers
//! share sync.rs) or local folders (docs/sources.md). The one place that
//! maps a configured kind to anything is [`Kind`], which says how each is
//! named and what it can do before its source is connected.

pub mod artists;
pub mod identity;
pub mod local;
pub mod subsonic;

use crate::bridge::PlaylistEdit;
use crate::failure::Failure;
use crate::favorites::{Starred, UserMeta};
use crate::lyrics::{Lyrics, SongStats};
use crate::sync::Fetch;
use async_trait::async_trait;
use library::{Album, AlbumTracks, Track};
use std::sync::Arc;

pub type TrackKey = accounts::ItemKey;

pub fn matches_track(key: &TrackKey, account: Option<accounts::AccountId>, id: &str) -> bool {
    Some(key.account) == crate::source::identity::account(id).or(account) && key.item == crate::source::identity::bare(id)
}

/// The qualified key keeps files apart across sources.
pub fn stream_name(key: &TrackKey) -> std::borrow::Cow<'_, str> {
    std::borrow::Cow::Owned(key.to_string())
}

pub type Src = Arc<dyn MusicSource>;

#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    pub stars: bool,
    pub ratings: bool,
    pub playlists: bool,
    pub lyrics: bool,
    pub scrobbling: bool,
    pub transcoding: bool,
    pub downloads: bool,
    pub network: bool,
}

impl Capabilities {
    /// All features supported by a standard OpenSubsonic server.
    pub const fn subsonic() -> Self {
        Self { stars: true, ratings: true, playlists: true, lyrics: true, scrobbling: true, transcoding: true, downloads: true, network: true }
    }

    /// Capabilities of a local-folder source.
    pub const fn local() -> Self {
        Self { stars: false, ratings: false, playlists: false, lyrics: true, scrobbling: false, transcoding: false, downloads: false, network: false }
    }

    /// Effective capabilities when Ond's own store (`ond.redb`) backs
    /// hearts, ratings and playlists for any source that lacks them.
    pub const fn with_ond_store(self) -> Self {
        Self { stars: true, ratings: true, playlists: true, ..self }
    }

    /// Empty capabilities for minimal test fakes.
    #[cfg(test)]
    pub const fn none() -> Self {
        Self { stars: false, ratings: false, playlists: false, lyrics: false, scrobbling: false, transcoding: false, downloads: false, network: false }
    }
}

/// How a configured kind of source is named and drawn, and what it can do
/// before its source has connected (the source itself says so after).
#[derive(Debug, Clone, Copy)]
pub struct Kind {
    /// "Navidrome", "Music folder": a source's kind in Settings.
    pub name: &'static str,
    pub icon: crate::icons::Icon,
    /// The word for fetching what changed: "Sync now", "Rescan".
    pub sync: &'static str,
    /// A sync reads files on this computer: it's called a scan.
    pub scans: bool,
    pub capabilities: Capabilities,
}

impl Kind {
    pub const fn of(kind: crate::settings::SourceKind) -> Self {
        use crate::settings::SourceKind;
        match kind {
            SourceKind::Subsonic => {
                Self { name: "Navidrome", icon: crate::icons::Icon::Disc, sync: "Sync now", scans: false, capabilities: Capabilities::subsonic() }
            }
            SourceKind::Local => {
                Self { name: "Music folder", icon: crate::icons::Icon::Folder, sync: "Rescan", scans: true, capabilities: Capabilities::local() }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StarKind {
    Song,
    Album,
    Artist,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub song_count: u32,
    pub duration: u32,
    pub cover_art: Option<String>,
    pub owner: Option<String>,
}

#[async_trait]
pub trait MusicSource: Send + Sync {
    fn account(&self) -> accounts::AccountId;
    fn capabilities(&self) -> Capabilities;
    /// A folder won't page through a server's library.
    fn paged(&self) -> Option<Arc<dyn Fetch>>;
    /// Local source handle when this source scans local folders.
    fn local(&self) -> Option<&local::LocalSource> {
        None
    }
    /// The same friendly server description and failures used at sign-in.
    async fn ping(&self) -> Result<String, Failure>;
    async fn album(&self, id: &str) -> Result<(AlbumTracks, UserMeta), Failure>;
    async fn newest(&self, count: u32) -> Result<Vec<Album>, Failure>;
    async fn playlists(&self) -> Result<Vec<Playlist>, Failure>;
    async fn playlist(&self, id: &str) -> Result<(Vec<Track>, UserMeta), Failure>;
    async fn starred(&self) -> Result<(Starred, Vec<(String, u8)>), Failure>;
    async fn lyrics(&self, track_id: &str, artist: Option<&str>, title: &str) -> Result<Option<Lyrics>, Failure>;
    async fn song_stats(&self, id: &str) -> Result<SongStats, Failure>;
    async fn set_starred(&self, kind: StarKind, id: &str, on: bool) -> Result<(), Failure>;
    async fn set_rating(&self, id: &str, rating: u8) -> Result<(), Failure>;
    async fn create_playlist(&self, name: &str, song_ids: &[String]) -> Result<(String, String), Failure>;
    async fn edit_playlist(&self, id: &str, edit: &PlaylistEdit) -> Result<(), Failure>;
    async fn scrobble(&self, id: &str, time_ms: u64, submission: bool) -> Result<(), Failure>;
    async fn cover(&self, cover_id: &str, size: u32) -> Result<Vec<u8>, Failure>;
    fn stream(&self, track: &Track, transcode: Option<&audio::Transcode>) -> (String, Option<String>);
    /// Sources advertising downloads supply a URL for every track.
    fn download_url(&self, id: &str) -> Option<String>;
}

#[cfg(test)]
pub(crate) mod fake;
