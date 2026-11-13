//! Local music folders as a [`MusicSource`].
//!
//! Scans configured folders with [`scan`], writes the resolved artists, albums,
//! and tracks into the account's [`Library`], persists [`ScanIndex`] at
//! `Sources/<account>/scan.json` for incremental rescans, and serves `file://`
//! URIs, embedded/folder covers, and sidecar or embedded lyrics.

use super::{Capabilities, MusicSource, Playlist, StarKind};
use crate::bridge::PlaylistEdit;
use crate::failure::Failure;
use crate::favorites::{Starred, UserMeta};
use crate::lyrics::{LyricLine, Lyrics, SongStats, StructuredLyrics};
use crate::sync::Fetch;
use async_trait::async_trait;
use library::{Album, AlbumTracks, Artist, ArtistLink, Contributor, Library, Track};
use scan::{ScanIndex, ScanProgress, ScanRoot, ScannedAlbum, ScannedArtist, ScannedTrack, TrackLyrics, TrackMove};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

const SCAN_STAMP_KEY: &str = "scan_stamp";
const SAVED_NEWEST_KEY: &str = "newest";
const NEWEST_LIMIT: usize = 120;

/// Summary of one completed [`LocalSource::sync`] pass.
#[derive(Debug, Clone)]
pub struct LocalSync {
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    pub newest_ids: Vec<String>,
    pub album_count: usize,
    pub track_count: usize,
    pub reused: usize,
    pub rescanned: usize,
    pub skipped: usize,
    #[allow(dead_code)]
    pub skipped_paths: Vec<PathBuf>,
    pub missing_roots: Vec<PathBuf>,
    pub moves: Vec<TrackMove>,
    pub stamp: String,
}

/// Typed failure from [`LocalSource::sync`] so callers can tell missing folders
/// apart from user cancellation and database errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalSyncError {
    Cancelled,
    FolderMissing(Vec<PathBuf>),
    Storage(String),
}

impl std::fmt::Display for LocalSyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Scan stopped"),
            Self::FolderMissing(paths) => f.write_str(&format_missing_folders(paths)),
            Self::Storage(msg) => write!(f, "Couldn't save scanned library: {msg}"),
        }
    }
}

impl std::error::Error for LocalSyncError {}

impl From<LocalSyncError> for Failure {
    fn from(err: LocalSyncError) -> Self {
        Failure::Local(err.to_string())
    }
}

fn format_missing_folders(paths: &[PathBuf]) -> String {
    match paths {
        [] => "Music folder not found".into(),
        [only] => format!("Music folder not found: {}", only.display()),
        many => format!("{} music folders not found ({})", many.len(), many[0].display()),
    }
}

#[derive(Default)]
struct LocalState {
    index: ScanIndex,
    album_rows: HashMap<String, Album>,
    albums: HashMap<String, Vec<Track>>,
    last_moves: Vec<TrackMove>,
    last_skipped: Vec<PathBuf>,
}

/// Music source backed by one or more local folders on disk.
pub struct LocalSource {
    account: accounts::AccountId,
    folders: Vec<ScanRoot>,
    scan_index_path: Option<PathBuf>,
    state: Mutex<LocalState>,
}

impl LocalSource {
    /// Creates a local-folder source for `account`.
    ///
    /// When `data_dir` is supplied, loads any saved incremental scan index from
    /// `Sources/<account>/scan.json` so playback, covers, and lyrics work right
    /// away on restart before the next rescan finishes.
    pub fn new(account: accounts::AccountId, folders: Vec<ScanRoot>, data_dir: Option<&Path>) -> Self {
        let scan_index_path = data_dir.map(|d| crate::storage::account_dir(d, account).join("scan.json"));
        let index = match &scan_index_path {
            Some(path) => ScanIndex::load(path).unwrap_or_else(|e| {
                tracing::warn!("loading scan index {}: {e}", path.display());
                ScanIndex::default()
            }),
            None => ScanIndex::default(),
        };
        Self { account, folders, scan_index_path, state: Mutex::new(LocalState { index, ..LocalState::default() }) }
    }

    #[allow(dead_code)]
    pub fn folders(&self) -> Vec<PathBuf> {
        self.folders.iter().map(|r| r.path.clone()).collect()
    }

    /// Moves `(old track id -> new track id)` detected by the most recent completed scan.
    #[allow(dead_code)]
    pub fn last_moves(&self) -> Vec<TrackMove> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).last_moves.clone()
    }

    /// Files with audio extensions skipped during the most recent completed scan.
    #[allow(dead_code)]
    pub fn last_skipped(&self) -> Vec<PathBuf> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).last_skipped.clone()
    }

    /// Scan signature of the current index (`""` before any completed scan).
    #[allow(dead_code)]
    pub fn scan_stamp(&self) -> String {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).index.stamp.clone()
    }

    /// Incrementally scans `folders` and updates `library` when anything changed.
    ///
    /// When all configured folders are missing or unreadable, returns
    /// [`LocalSyncError::FolderMissing`] without touching `library` or the saved
    /// scan index. When only some folders are missing, cached albums from those
    /// folders are kept and [`LocalSync::missing_roots`] lists the missing paths.
    pub fn sync(&self, library: &Library, cancel: &AtomicBool, mut on_progress: impl FnMut(ScanProgress)) -> Result<LocalSync, LocalSyncError> {
        let prev = self.state.lock().unwrap_or_else(|e| e.into_inner()).index.clone();
        let result = scan::scan_folders(&self.folders, &prev, cancel, &mut on_progress);
        if result.cancelled {
            return Err(LocalSyncError::Cancelled);
        }
        if !self.folders.is_empty() && result.missing_roots.len() >= self.folders.len() {
            return Err(LocalSyncError::FolderMissing(result.missing_roots));
        }

        let artists: Vec<Artist> = result.artists.into_iter().map(convert_artist).collect();
        let albums: Vec<Album> = result.albums.into_iter().map(convert_album).collect();
        let tracks: Vec<Track> = result.tracks.into_iter().map(convert_track).collect();

        let mut album_rows: HashMap<String, Album> = HashMap::with_capacity(albums.len());
        let mut by_album: HashMap<String, Vec<Track>> = HashMap::with_capacity(albums.len());
        for album in &albums {
            album_rows.insert(album.id.clone(), album.clone());
            by_album.insert(album.id.clone(), Vec::new());
        }
        for track in &tracks {
            if let Some(list) = by_album.get_mut(&track.album_id) {
                list.push(track.clone());
            }
        }
        let newest_ids = newest_album_ids(&albums, NEWEST_LIMIT);

        let stamp = result.index.stamp.clone();
        let prev_stamp = library.saved::<String>(SCAN_STAMP_KEY).ok().flatten();
        let db_matches = prev_stamp.as_deref() == Some(stamp.as_str())
            && !stamp.is_empty()
            && library.album_stamps().map(|s| s.len() == albums.len()).unwrap_or(false)
            && library.track_count().map(|n| n == tracks.len() as u64).unwrap_or(false);

        if !db_matches {
            let keep: HashSet<String> = albums.iter().map(|a| a.id.clone()).collect();
            let batch: Vec<AlbumTracks> =
                albums.iter().map(|a| AlbumTracks { album: a.clone(), tracks: by_album.get(&a.id).cloned().unwrap_or_default() }).collect();
            let write = (|| -> library::Result<()> {
                library.retain_albums(&keep)?;
                library.upsert_albums_tracks(batch)?;
                library.replace_artists(artists.clone())?;
                library.save(SAVED_NEWEST_KEY, &newest_ids)?;
                library.save(SCAN_STAMP_KEY, &stamp)?;
                Ok(())
            })();
            if let Err(e) = write {
                return Err(LocalSyncError::Storage(e.to_string()));
            }
        }

        if let Some(path) = &self.scan_index_path
            && let Err(e) = result.index.save(path)
        {
            tracing::warn!("saving scan index {}: {e}", path.display());
        }

        let summary = LocalSync {
            album_count: albums.len(),
            track_count: tracks.len(),
            albums,
            artists,
            newest_ids,
            reused: result.reused,
            rescanned: result.rescanned,
            skipped: result.skipped,
            skipped_paths: result.skipped_paths.clone(),
            missing_roots: result.missing_roots,
            moves: result.moves.clone(),
            stamp,
        };

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.index = result.index;
        state.album_rows = album_rows;
        state.albums = by_album;
        state.last_moves = result.moves;
        state.last_skipped = result.skipped_paths;

        Ok(summary)
    }

    /// Path on disk for a scanned track id.
    pub fn track_path(&self, id: &str) -> Option<PathBuf> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).index.track_path(id).map(Path::to_path_buf)
    }

    fn cover_path(&self, cover_id: &str) -> Option<PathBuf> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).index.cover_path(cover_id).map(Path::to_path_buf)
    }
}

#[async_trait]
impl MusicSource for LocalSource {
    fn account(&self) -> accounts::AccountId {
        self.account
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::local()
    }

    fn paged(&self) -> Option<Arc<dyn Fetch>> {
        None
    }

    fn local(&self) -> Option<&LocalSource> {
        Some(self)
    }

    async fn ping(&self) -> Result<String, Failure> {
        if self.folders.iter().any(|r| r.path.exists()) { Ok("Local folders".into()) } else { Err(Failure::Local("Configured music folder not found".into())) }
    }

    async fn album(&self, id: &str) -> Result<(AlbumTracks, UserMeta), Failure> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let album = state.album_rows.get(id).cloned().ok_or_else(|| Failure::Local("Album not found in local scan".into()))?;
        let tracks = state.albums.get(id).cloned().unwrap_or_default();
        Ok((AlbumTracks { album, tracks }, empty_user_meta()))
    }

    async fn newest(&self, count: u32) -> Result<Vec<Album>, Failure> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut albums: Vec<Album> = state.album_rows.values().cloned().collect();
        sort_newest(&mut albums);
        albums.truncate(count as usize);
        Ok(albums)
    }

    async fn playlists(&self) -> Result<Vec<Playlist>, Failure> {
        Ok(Vec::new())
    }

    async fn playlist(&self, _id: &str) -> Result<(Vec<Track>, UserMeta), Failure> {
        Err(Failure::Local("Playlists are not supported by local folders".into()))
    }

    async fn starred(&self) -> Result<(Starred, Vec<(String, u8)>), Failure> {
        Ok((Starred { songs: Vec::new(), albums: Vec::new(), artists: Vec::new() }, Vec::new()))
    }

    async fn lyrics(&self, track_id: &str, _artist: Option<&str>, _title: &str) -> Result<Option<Lyrics>, Failure> {
        let Some(path) = self.track_path(track_id) else {
            return Ok(None);
        };
        let raw = tokio::task::spawn_blocking(move || scan::read_lyrics(&path))
            .await
            .map_err(|e| Failure::Local(format!("Couldn't read lyrics: {e}")))?
            .map_err(|e| Failure::Local(format!("Couldn't read lyrics: {e}")))?;
        Ok(raw.and_then(convert_lyrics))
    }

    async fn song_stats(&self, _id: &str) -> Result<SongStats, Failure> {
        Ok(SongStats::default())
    }

    async fn set_starred(&self, _kind: StarKind, _id: &str, _on: bool) -> Result<(), Failure> {
        Err(Failure::Local("Favorites are not supported by local folders".into()))
    }

    async fn set_rating(&self, _id: &str, _rating: u8) -> Result<(), Failure> {
        Err(Failure::Local("Ratings are not supported by local folders".into()))
    }

    async fn create_playlist(&self, _name: &str, _song_ids: &[String]) -> Result<(String, String), Failure> {
        Err(Failure::Local("Playlists are not supported by local folders".into()))
    }

    async fn edit_playlist(&self, _id: &str, _edit: &PlaylistEdit) -> Result<(), Failure> {
        Err(Failure::Local("Playlists are not supported by local folders".into()))
    }

    async fn scrobble(&self, _id: &str, _time_ms: u64, _submission: bool) -> Result<(), Failure> {
        Err(Failure::Local("Scrobbling is not supported by local folders".into()))
    }

    async fn cover(&self, cover_id: &str, _size: u32) -> Result<Vec<u8>, Failure> {
        let Some(path) = self.cover_path(cover_id) else {
            return Err(Failure::Local("Cover not found".into()));
        };
        let emb = cover_id.starts_with("emb-");
        tokio::task::spawn_blocking(move || {
            if emb {
                scan::read_embedded_cover(&path)
                    .map_err(|e| Failure::Local(format!("Couldn't read embedded cover {}: {e}", path.display())))?
                    .ok_or_else(|| Failure::Local("Embedded cover not found".into()))
            } else {
                std::fs::read(&path).map_err(|e| Failure::Local(format!("Couldn't read cover {}: {e}", path.display())))
            }
        })
        .await
        .map_err(|e| Failure::Local(format!("Couldn't read cover: {e}")))?
    }

    fn stream(&self, track: &Track, _transcode: Option<&audio::Transcode>) -> (String, Option<String>) {
        let url = self.track_path(&track.id).map(|p| format!("file://{}", p.to_string_lossy())).unwrap_or_default();
        (url, track.suffix.clone())
    }

    fn download_url(&self, _id: &str) -> Option<String> {
        None
    }
}

fn empty_user_meta() -> UserMeta {
    UserMeta { starred: Vec::new(), ratings: Vec::new() }
}

fn convert_artist(a: ScannedArtist) -> Artist {
    Artist { id: a.id, name: a.name, sort: a.sort, album_count: a.album_count, cover_art: a.cover_art }
}

fn convert_artist_link(a: scan::ArtistRef) -> ArtistLink {
    ArtistLink { id: a.id, name: a.name }
}

fn convert_album(a: ScannedAlbum) -> Album {
    Album {
        id: a.id,
        name: a.name,
        sort: a.sort,
        artist: a.artist,
        artist_ref: a.artist_id,
        year: a.year,
        genre: a.genres.first().cloned(),
        song_count: a.song_count,
        duration_secs: a.duration_secs,
        cover_art: a.cover_art,
        created: a.created_secs.map(format_unix_iso),
        labels: a.labels,
        genres: a.genres,
        artists: a.artists.into_iter().map(convert_artist_link).collect(),
    }
}

fn convert_track(t: ScannedTrack) -> Track {
    let artist_ref = t.artists.first().and_then(|a| a.id.clone());
    Track {
        id: t.id,
        album_id: t.album_id,
        title: t.title,
        artist: t.artist,
        artist_ref,
        track: t.track,
        disc: t.disc,
        duration_secs: t.duration_secs,
        suffix: (!t.suffix.is_empty()).then_some(t.suffix),
        content_type: t.content_type,
        size: Some(t.size),
        bit_rate_kbps: t.bit_rate_kbps,
        bit_depth: t.bit_depth,
        sample_rate: t.sample_rate,
        channels: t.channels,
        track_gain: t.track_gain,
        album_gain: t.album_gain,
        track_peak: t.track_peak,
        album_peak: t.album_peak,
        composer: t.composer,
        contributors: t.contributors.into_iter().map(|c| Contributor { role: c.role, sub_role: c.sub_role, name: c.name, artist_id: c.artist_id }).collect(),
        cover_art: t.cover_art,
        artists: t.artists.into_iter().map(convert_artist_link).collect(),
    }
}

fn convert_lyrics(raw: TrackLyrics) -> Option<Lyrics> {
    match raw {
        TrackLyrics::Synced { offset_ms, lines } => {
            let structured = StructuredLyrics {
                synced: true,
                offset: offset_ms,
                line: lines.into_iter().map(|l| LyricLine { start: Some(l.start_ms), value: l.text }).collect(),
            };
            crate::lyrics::pick(vec![structured])
        }
        TrackLyrics::Plain(text) => crate::lyrics::from_text(&text),
    }
}

fn sort_newest(albums: &mut [Album]) {
    albums.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| a.name.cmp(&b.name)));
}

fn newest_album_ids(albums: &[Album], limit: usize) -> Vec<String> {
    let mut copy = albums.to_vec();
    sort_newest(&mut copy);
    copy.truncate(limit);
    copy.into_iter().map(|a| a.id).collect()
}

/// Formats a Unix timestamp in seconds as an ISO-8601 UTC date string so
/// `crate::fmt::date` and chronological `Album::created` sorts agree.
fn format_unix_iso(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;
    // Howard Hinnant's civil-from-days algorithm anchored at 1970-01-01.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = y + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;
    use accounts::AccountId;
    use std::fs;
    use tempfile::TempDir;

    use crate::test_tracks::flac as flac_with_tags;

    #[tokio::test]
    async fn local_source_syncs_library_and_serves_streams_covers_and_lyrics() {
        let dir = TempDir::new().unwrap();
        let data = dir.path().join("data");
        let music = dir.path().join("music");
        let album_dir = music.join("Artist").join("Album");
        fs::create_dir_all(&album_dir).unwrap();

        let track1_path = album_dir.join("01 One.flac");
        let track2_path = album_dir.join("02 Two.flac");
        fs::write(
            &track1_path,
            flac_with_tags(
                &[
                    ("TITLE", "Track One"),
                    ("ARTIST", "Artist A"),
                    ("ALBUM", "Record"),
                    ("ALBUMARTIST", "Artist A"),
                    ("TRACKNUMBER", "1"),
                    ("MUSICBRAINZ_TRACKID", "mb-rec-1"),
                ],
                Some(b"embedded-png"),
            ),
        )
        .unwrap();
        fs::write(album_dir.join("01 One.lrc"), "[00:01.00]Hello sidecar\n").unwrap();
        fs::write(
            &track2_path,
            flac_with_tags(
                &[
                    ("TITLE", "Track Two"),
                    ("ARTIST", "Artist A"),
                    ("ALBUM", "Record"),
                    ("ALBUMARTIST", "Artist A"),
                    ("TRACKNUMBER", "2"),
                    ("LYRICS", "Plain embedded line"),
                ],
                None,
            ),
        )
        .unwrap();
        fs::write(album_dir.join("cover.jpg"), b"folder-jpeg").unwrap();

        let account = AccountId::new();
        let roots = vec![ScanRoot::new("music", music.clone())];
        let source = LocalSource::new(account, roots.clone(), Some(&data));
        assert!(!source.capabilities().stars);
        assert!(!source.capabilities().ratings);
        assert!(!source.capabilities().playlists);
        assert!(!source.capabilities().scrobbling);
        assert!(!source.capabilities().transcoding);
        assert!(!source.capabilities().downloads);
        assert!(!source.capabilities().network);
        assert!(source.capabilities().lyrics);
        assert!(source.paged().is_none());

        fs::create_dir_all(&data).unwrap();
        let lib = Library::open(data.join("lib.redb")).unwrap();
        let cancel = AtomicBool::new(false);
        let mut last_progress = None;
        let synced = source.sync(&lib, &cancel, |p| last_progress = Some(p)).unwrap();
        let last_progress = last_progress.unwrap();
        assert_eq!((last_progress.scanned, last_progress.total), (2, 2));
        assert_eq!(synced.album_count, 1);
        assert_eq!(synced.track_count, 2);
        assert_eq!(lib.track_count().unwrap(), 2);

        let tracks = lib.all_tracks().unwrap();
        let t1 = tracks.iter().find(|t| t.title == "Track One").unwrap();
        let t2 = tracks.iter().find(|t| t.title == "Track Two").unwrap();

        let (url, suffix) = source.stream(t1, None);
        assert_eq!(url, format!("file://{}", track1_path.display()));
        assert_eq!(suffix.as_deref(), Some("flac"));
        assert!(source.download_url(&t1.id).is_none());

        let emb = source.cover(t1.cover_art.as_deref().unwrap(), 300).await.unwrap();
        assert_eq!(emb, b"embedded-png");
        let dir_cov = source.cover(t2.cover_art.as_deref().unwrap(), 300).await.unwrap();
        assert_eq!(dir_cov, b"folder-jpeg");

        let l1 = source.lyrics(&t1.id, None, &t1.title).await.unwrap().unwrap();
        assert_eq!(l1, Lyrics::Synced(vec![crate::lyrics::Line { start_ms: 1000, text: "Hello sidecar".into() }]));
        let l2 = source.lyrics(&t2.id, None, &t2.title).await.unwrap().unwrap();
        assert_eq!(l2, Lyrics::Plain(vec!["Plain embedded line".into()]));

        assert!(matches!(source.set_starred(StarKind::Song, &t1.id, true).await, Err(Failure::Local(_))));
        assert!(matches!(source.set_rating(&t1.id, 5).await, Err(Failure::Local(_))));
        assert!(matches!(source.create_playlist("List", std::slice::from_ref(&t1.id)).await, Err(Failure::Local(_))));

        // Re-open with saved scan.json: playback, covers, and lyrics work before a rescan.
        let reopened = LocalSource::new(account, roots, Some(&data));
        let (re_url, _) = reopened.stream(t1, None);
        assert_eq!(re_url, url);
        assert_eq!(reopened.cover(t1.cover_art.as_deref().unwrap(), 300).await.unwrap(), b"embedded-png");

        // Moving track 1 inside the watched tree updates its id and reports a move.
        let moved_path = album_dir.join("Disc1-01 One.flac");
        fs::rename(&track1_path, &moved_path).unwrap();
        let old_id = t1.id.clone();
        let rescan = reopened.sync(&lib, &cancel, |_| {}).unwrap();
        assert_eq!(rescan.reused, 1);
        assert_eq!(rescan.rescanned, 1);
        assert_eq!(rescan.moves.len(), 1);
        assert_eq!(rescan.moves[0].from, old_id);
        assert_ne!(rescan.moves[0].to, old_id);

        // Renaming the music folder away must preserve the library and scan index
        // while reporting the missing root.
        let away = dir.path().join("music-away");
        let index_file = crate::storage::account_dir(&data, account).join("scan.json");
        let index_before = fs::read(&index_file).unwrap();
        fs::rename(&music, &away).unwrap();
        let err = reopened.sync(&lib, &cancel, |_| {}).unwrap_err();
        assert_eq!(err, LocalSyncError::FolderMissing(vec![music.clone()]));
        assert_eq!(lib.albums().unwrap().len(), 1, "existing albums stay when folder is missing");
        assert_eq!(lib.track_count().unwrap(), 2);
        assert_eq!(fs::read(&index_file).unwrap(), index_before, "scan index stays untouched");
    }
}
