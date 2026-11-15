//! A source that records optional calls and refuses unexpected library work,
//! so tests can prove that capabilities stop requests at the app boundary.

use super::*;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct Fake {
    pub account: accounts::AccountId,
    pub capabilities: Capabilities,
    pub scrobbles: AtomicUsize,
    pub lyric_calls: AtomicUsize,
    pub covers: Mutex<HashMap<String, Vec<u8>>>,
}

impl Fake {
    pub fn new(capabilities: Capabilities) -> Self {
        Self {
            account: crate::test_tracks::account(),
            capabilities,
            scrobbles: AtomicUsize::new(0),
            lyric_calls: AtomicUsize::new(0),
            covers: Mutex::default(),
        }
    }
}

#[async_trait]
impl MusicSource for Fake {
    fn account(&self) -> accounts::AccountId {
        self.account
    }
    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }
    fn paged(&self) -> Option<Arc<dyn Fetch>> {
        None
    }
    async fn ping(&self) -> Result<String, Failure> {
        panic!("unexpected ping")
    }
    async fn album(&self, _: &str) -> Result<(AlbumTracks, UserMeta), Failure> {
        panic!("unexpected album")
    }
    async fn newest(&self, _: u32) -> Result<Vec<Album>, Failure> {
        panic!("unexpected newest")
    }
    async fn playlists(&self) -> Result<Vec<Playlist>, Failure> {
        panic!("unexpected playlists")
    }
    async fn playlist(&self, _: &str) -> Result<(Vec<Track>, UserMeta), Failure> {
        panic!("unexpected playlist")
    }
    async fn starred(&self) -> Result<(Starred, Vec<(String, u8)>), Failure> {
        panic!("unexpected starred")
    }
    async fn lyrics(&self, _: &str, _: Option<&str>, _: &str) -> Result<Option<Lyrics>, Failure> {
        self.lyric_calls.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
    async fn song_stats(&self, _: &str) -> Result<SongStats, Failure> {
        panic!("unexpected song stats")
    }
    async fn set_starred(&self, _: StarKind, _: &str, _: bool) -> Result<(), Failure> {
        panic!("unexpected star")
    }
    async fn set_rating(&self, _: &str, _: u8) -> Result<(), Failure> {
        panic!("unexpected rating")
    }
    async fn create_playlist(&self, _: &str, _: &[String]) -> Result<(String, String), Failure> {
        panic!("unexpected create playlist")
    }
    async fn edit_playlist(&self, _: &str, _: &PlaylistEdit) -> Result<(), Failure> {
        panic!("unexpected edit playlist")
    }
    async fn scrobble(&self, _: &str, _: u64, _: bool) -> Result<(), Failure> {
        self.scrobbles.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn cover(&self, id: &str, _: u32) -> Result<Vec<u8>, Failure> {
        Ok(self.covers.lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned().unwrap_or_else(|| panic!("unexpected cover {id}")))
    }
    fn stream(&self, _: &Track, _: Option<&audio::Transcode>) -> (String, Option<String>) {
        panic!("unexpected stream")
    }
    fn download_url(&self, _: &str) -> Option<String> {
        panic!("unexpected download")
    }
}
