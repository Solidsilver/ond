//! What plays without the server, and working offline: then the library
//! loads through [`Playable`] (off the UI thread), so only what can play
//! shows.

use crate::bridge::SavedStarred;
use library::{Album, Artist, Track};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use streamcache::{CacheKey, StreamCache};

/// `track`'s file in the stream cache, if it was streamed whole: the
/// original, else as `transcode` converts it.
pub fn cached(cache: &StreamCache, transcode: Option<&audio::Transcode>, track: &Track) -> Option<PathBuf> {
    cache
        .complete_path(&CacheKey::original(track.id.clone(), track.suffix.clone()))
        .or_else(|| transcode.and_then(|t| cache.complete_path(&CacheKey::transcode(&track.id, &t.format, t.kbps))))
}

/// What can play without the server: downloaded, or streamed whole and
/// still cached.
#[derive(Clone)]
pub struct Playable {
    pub downloads: Arc<offline::Snapshot>,
    pub cache: StreamCache,
    pub transcode: Option<audio::Transcode>,
}

impl Playable {
    pub fn track(&self, t: &Track) -> bool {
        self.downloads.is_done(&t.id) || cached(&self.cache, self.transcode.as_ref(), t).is_some()
    }
}

/// The library cut down to what can play.
pub struct Kept {
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    pub tracks: Vec<Track>,
    album_ids: HashSet<String>,
    artist_ids: HashSet<String>,
}

impl Kept {
    /// The tracks that can play, the albums with any of them, and those
    /// albums' and tracks' artists.
    pub fn new(keep: &Playable, mut albums: Vec<Album>, mut artists: Vec<Artist>, mut tracks: Vec<Track>) -> Self {
        tracks.retain(|t| keep.track(t));
        let album_ids: HashSet<String> = tracks.iter().map(|t| t.album_id.clone()).collect();
        albums.retain(|a| album_ids.contains(&a.id));
        let mut ids: HashSet<&str> = HashSet::new();
        let mut names: HashSet<&str> = HashSet::new();
        for a in &albums {
            ids.extend(a.artist_ref.as_deref());
            names.extend(a.artist.as_deref());
            for l in &a.artists {
                ids.extend(l.id.as_deref());
                names.insert(&l.name);
            }
        }
        for t in &tracks {
            ids.extend(t.artist_ref.as_deref());
            for l in &t.artists {
                ids.extend(l.id.as_deref());
                names.insert(&l.name);
            }
        }
        artists.retain(|a| ids.contains(a.id.as_str()) || names.contains(a.name.as_str()));
        let artist_ids = artists.iter().map(|a| a.id.clone()).collect();
        Self { albums, artists, tracks, album_ids, artist_ids }
    }

    pub fn newest(&self, ids: &mut Vec<String>) {
        ids.retain(|id| self.album_ids.contains(id));
    }

    pub fn starred(&self, keep: &Playable, s: &mut SavedStarred) {
        s.starred.songs.retain(|t| keep.track(t));
        s.starred.albums.retain(|id| self.album_ids.contains(id));
        s.starred.artists.retain(|(id, _)| self.artist_ids.contains(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str, album: &str, artist: &str) -> Track {
        Track {
            id: id.into(),
            album_id: album.into(),
            title: id.into(),
            artist: None,
            artist_ref: Some(artist.into()),
            track: None,
            disc: None,
            duration_secs: None,
            suffix: Some("flac".into()),
            content_type: None,
            size: None,
            bit_rate_kbps: None,
            bit_depth: None,
            sample_rate: None,
            channels: None,
            track_gain: None,
            album_gain: None,
            track_peak: None,
            album_peak: None,
            composer: None,
            contributors: Vec::new(),
            cover_art: None,
            artists: Vec::new(),
        }
    }

    fn album(id: &str, artist: &str) -> Album {
        Album {
            id: id.into(),
            name: id.into(),
            sort: None,
            artist: None,
            artist_ref: Some(artist.into()),
            year: None,
            genre: None,
            song_count: 0,
            duration_secs: 0,
            cover_art: None,
            created: None,
            labels: Vec::new(),
            genres: Vec::new(),
            artists: Vec::new(),
        }
    }

    fn artist(id: &str) -> Artist {
        Artist { id: id.into(), name: id.to_uppercase(), sort: None, album_count: 1, cover_art: None }
    }

    #[test]
    fn keeps_what_can_play_and_whatever_it_belongs_to() {
        let dir = tempfile::tempdir().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let cache = StreamCache::new(streamcache::CacheConfig::new(dir.path()), rt.handle().clone()).unwrap();
        let downloads = offline::Snapshot { tracks: [("a1".to_owned(), offline::TrackState::Done { size: 1 })].into(), ..Default::default() };
        let keep = Playable { downloads: Arc::new(downloads), cache, transcode: None };
        let kept = Kept::new(
            &keep,
            vec![album("A", "x"), album("B", "y")],
            vec![artist("x"), artist("y"), artist("z")],
            vec![track("a1", "A", "z"), track("a2", "A", "x"), track("b1", "B", "y")],
        );
        assert_eq!(kept.tracks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["a1"]);
        assert_eq!(kept.albums.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["A"]);
        // The album's artist and the track's.
        assert_eq!(kept.artists.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["x", "z"]);
        let mut newest = vec!["B".to_owned(), "A".to_owned()];
        kept.newest(&mut newest);
        assert_eq!(newest, ["A"]);
    }
}
