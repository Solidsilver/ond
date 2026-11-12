//! The music library as the app asks for it. Callers read capabilities and
//! ask the source, never which kind it is. Subsonic is the only kind until
//! local folders arrive (docs/sources.md); paged servers share sync.rs.

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
    Some(key.account) == account && key.item == id
}

/// The qualified key keeps files apart across sources.
pub fn stream_name(key: &TrackKey) -> std::borrow::Cow<'_, str> {
    std::borrow::Cow::Owned(key.to_string())
}

pub type Src = Arc<dyn MusicSource>;

#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    pub scrobbling: bool,
    pub downloads: bool,
    pub lyrics: bool,
    pub playlists: bool,
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
