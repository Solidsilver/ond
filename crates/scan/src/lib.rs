//! Local music folder scanner.
//!
//! Walks one or more music folders, reads audio tags and technical properties
//! via symphonia, groups tracks into albums and artists (detecting folder
//! compilations while keeping disc numbers), discovers embedded and folder
//! covers, and reads `.lrc` sidecar or embedded lyrics.
//!
//! Track IDs are deterministic hashes of paths relative to their folder root,
//! so moving an entire library root keeps all IDs intact. Rescans skip files
//! whose `(size, mtime)` stamp matches [`ScanIndex`], and pair tracks that
//! vanish and appear in the same pass by MusicBrainz recording ID or matching
//! tags and length so callers can migrate user state across file moves.

mod group;
mod lyrics;
mod probe;

pub use lyrics::{TimedLine, TrackLyrics};

use group::{RawScannedFile, find_folder_cover};
use probe::ProbedFile;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

const INDEX_VERSION: u32 = 1;

/// Audio file extensions probed during a scan. Non-audio files (covers,
/// lyrics, OS metadata) are left untouched rather than counted as skipped.
const AUDIO_EXTS: &[&str] = &[
    "aac", "aif", "aifc", "aiff", "alac", "ape", "caf", "dff", "dsf", "flac", "m4a", "m4b", "mka", "mp1", "mp2", "mp3", "mp4", "mpc", "oga", "ogg", "opus",
    "spx", "tak", "tta", "wav", "wave", "webm", "wma", "wv",
];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("scan index format error: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("probe error: {0}")]
    Probe(String),
}

/// Progress reported during [`scan_folders`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanProgress {
    pub scanned: usize,
    pub total: usize,
    pub skipped: usize,
    pub current: Option<PathBuf>,
}

/// A file detected as moved within a single incremental rescan pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackMove {
    pub from: String,
    pub to: String,
}

/// An artist reference carried by a track or album.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistRef {
    pub id: Option<String>,
    pub name: String,
    pub sort: Option<String>,
}

/// A non-primary credit on a track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contributor {
    pub role: String,
    pub sub_role: Option<String>,
    pub name: String,
    pub artist_id: Option<String>,
}

/// One scanned audio file with resolved IDs and tags.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScannedTrack {
    pub id: String,
    pub path: PathBuf,
    pub rel_path: String,
    pub album_id: String,
    pub title: String,
    pub sort_title: Option<String>,
    pub artist: Option<String>,
    pub sort_artist: Option<String>,
    pub album_artist: Option<String>,
    pub sort_album_artist: Option<String>,
    pub album: String,
    pub sort_album: Option<String>,
    pub artists: Vec<ArtistRef>,
    pub album_artists: Vec<ArtistRef>,
    pub track: Option<u32>,
    pub track_total: Option<u32>,
    pub disc: Option<u32>,
    pub disc_total: Option<u32>,
    pub compilation: bool,
    pub year: Option<i32>,
    pub genres: Vec<String>,
    pub labels: Vec<String>,
    pub composer: Option<String>,
    pub contributors: Vec<Contributor>,
    pub duration_secs: Option<u32>,
    pub duration_ms: Option<u64>,
    pub suffix: String,
    pub content_type: Option<String>,
    pub size: u64,
    pub bit_rate_kbps: Option<u32>,
    pub bit_depth: Option<u32>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub track_gain: Option<f32>,
    pub album_gain: Option<f32>,
    pub track_peak: Option<f32>,
    pub album_peak: Option<f32>,
    pub mb_recording_id: Option<String>,
    pub mb_track_id: Option<String>,
    pub mb_release_track_id: Option<String>,
    pub mb_album_id: Option<String>,
    pub mb_release_group_id: Option<String>,
    pub mb_artist_ids: Vec<String>,
    pub mb_album_artist_ids: Vec<String>,
    pub cover_art: Option<String>,
    pub has_embedded_art: bool,
    pub has_embedded_lyrics: bool,
    pub modified_secs: Option<i64>,
}

/// An album grouped from scanned tracks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScannedAlbum {
    pub id: String,
    pub name: String,
    pub sort: Option<String>,
    pub artist: Option<String>,
    pub artist_id: Option<String>,
    pub artists: Vec<ArtistRef>,
    pub year: Option<i32>,
    pub genres: Vec<String>,
    pub labels: Vec<String>,
    pub song_count: u32,
    pub duration_secs: u32,
    pub cover_art: Option<String>,
    pub compilation: bool,
    pub mb_album_id: Option<String>,
    pub mb_release_group_id: Option<String>,
    pub created_secs: Option<i64>,
}

/// An artist aggregated across scanned albums and tracks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScannedArtist {
    pub id: String,
    pub name: String,
    pub sort: Option<String>,
    pub album_count: u32,
    pub cover_art: Option<String>,
}

/// Modification stamp used for incremental rescans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct FileStamp {
    size: u64,
    mtime_secs: i64,
    mtime_nanos: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct IndexedFile {
    id: String,
    rel_path: String,
    stamp: Option<FileStamp>,
    probed: ProbedFile,
    mb_recording_key: Option<String>,
}

/// Persisted state from a previous scan for incremental rescans, cover
/// lookup, and path resolution.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScanIndex {
    version: u32,
    files: BTreeMap<String, IndexedFile>,
    covers: BTreeMap<String, PathBuf>,
    track_paths: BTreeMap<String, PathBuf>,
    /// Signature of last completed scan (`tracks:u64_hash`), for quick checks.
    pub stamp: String,
}

impl ScanIndex {
    /// Load a previously saved index, returning a fresh empty index when missing
    /// or from an incompatible version.
    pub fn load(path: &Path) -> Result<Self, Error> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(Error::Io(e)),
        };
        let Ok(index) = serde_json::from_slice::<Self>(&bytes) else {
            tracing::warn!("ignoring unreadable scan index at {}", path.display());
            return Ok(Self::default());
        };
        if index.version != INDEX_VERSION {
            return Ok(Self::default());
        }
        Ok(index)
    }

    /// Persist this index atomically via a temporary sibling file.
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec(self)?;
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Look up the current on-disk path of a track by id.
    pub fn track_path(&self, track_id: &str) -> Option<&Path> {
        self.track_paths.get(track_id).map(PathBuf::as_path)
    }

    /// Look up the file path backed by a `cover_art` id.
    pub fn cover_path(&self, cover_id: &str) -> Option<&Path> {
        self.covers.get(cover_id).map(PathBuf::as_path)
    }
}

/// Result of scanning one or more local folders.
#[derive(Debug, Clone)]
pub struct ScanResult {
    pub tracks: Vec<ScannedTrack>,
    pub albums: Vec<ScannedAlbum>,
    pub artists: Vec<ScannedArtist>,
    /// Files with audio extensions that could not be probed or had no audio track.
    pub skipped: usize,
    pub skipped_paths: Vec<PathBuf>,
    /// Files detected as moved from a previous track id to a new track id in this pass.
    pub moves: Vec<TrackMove>,
    /// Files reused from [`ScanIndex`] without re-probing.
    pub reused: usize,
    /// Files opened and probed in this scan.
    pub rescanned: usize,
    /// Whether scanning stopped early via the cancel flag.
    pub cancelled: bool,
    pub index: ScanIndex,
}

/// Deterministic track id from a path relative to its scanned root.
pub fn track_id_for_rel(rel_path: &str) -> String {
    hash_id(&format!("ond-track-v1\0{rel_path}"))
}

/// Scan `roots` once without incremental state.
pub fn scan(roots: &[PathBuf]) -> ScanResult {
    let cancel = AtomicBool::new(false);
    scan_folders(roots, &ScanIndex::default(), &cancel, |_| {})
}

/// Scan `roots` incrementally using `prev`, stopping early if `cancel` becomes
/// true and invoking `on_progress` after each candidate audio file.
pub fn scan_folders(roots: &[PathBuf], prev: &ScanIndex, cancel: &AtomicBool, mut on_progress: impl FnMut(ScanProgress)) -> ScanResult {
    struct Candidate {
        path: PathBuf,
        rel_path: String,
        rel_folder: String,
        folder_key: PathBuf,
        size: u64,
        stamp: Option<FileStamp>,
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    let mut seen_rel: HashSet<String> = HashSet::new();
    let mut folder_covers: HashMap<PathBuf, Option<(String, PathBuf)>> = HashMap::new();

    for root in roots {
        if !root.exists() {
            continue;
        }
        let mut walker = WalkDir::new(root).follow_links(true).sort_by_file_name().into_iter();
        while let Some(entry) = walker.next() {
            let Ok(entry) = entry else {
                continue;
            };
            if entry.file_type().is_dir() {
                // Skip hidden directories (like .Spotlight-V100, .Trash, .git).
                if entry.depth() > 0 && is_hidden(entry.path()) {
                    walker.skip_current_dir();
                }
                continue;
            }
            if !entry.file_type().is_file() || is_hidden(entry.path()) || !is_audio_file(entry.path()) {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(root) else {
                continue;
            };
            let rel_path = normalize_rel(rel);
            if !seen_rel.insert(rel_path.clone()) {
                continue;
            }
            let rel_folder = rel.parent().map(normalize_rel).unwrap_or_default();
            let parent_dir = entry.path().parent().unwrap_or(root).to_path_buf();
            folder_covers.entry(parent_dir.clone()).or_insert_with(|| {
                find_folder_cover(&parent_dir).and_then(|cover_path| {
                    let cover_rel = cover_path.strip_prefix(root).ok().map(normalize_rel)?;
                    Some((cover_rel, cover_path))
                })
            });

            let meta = entry.metadata().ok();
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let stamp = meta.as_ref().and_then(file_stamp);
            candidates.push(Candidate { path: entry.path().to_path_buf(), rel_path, rel_folder, folder_key: parent_dir, size, stamp });
        }
    }

    let total = candidates.len();
    let prev_valid = prev.version == INDEX_VERSION;
    let mut raw_files: Vec<RawScannedFile> = Vec::with_capacity(total);
    let mut new_entries: BTreeMap<String, IndexedFile> = BTreeMap::new();
    let mut skipped_paths: Vec<PathBuf> = Vec::new();
    let mut reused = 0usize;
    let mut rescanned = 0usize;
    let mut cancelled = false;

    for (idx, c) in candidates.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        on_progress(ScanProgress { scanned: idx, total, skipped: skipped_paths.len(), current: Some(c.path.clone()) });

        let id = track_id_for_rel(&c.rel_path);
        let reused_entry =
            if prev_valid { prev.files.get(&c.rel_path).filter(|old| old.stamp.is_some() && old.stamp == c.stamp).map(|old| old.probed.clone()) } else { None };

        let probed = match reused_entry {
            Some(p) => {
                reused += 1;
                p
            }
            None => match probe::probe_file(&c.path, c.size) {
                Ok(p) => {
                    rescanned += 1;
                    p
                }
                Err(err) => {
                    tracing::debug!("skipping undecodable audio file {}: {err}", c.path.display());
                    skipped_paths.push(c.path);
                    continue;
                }
            },
        };

        let mb_rec = probed.mb_recording_id.clone().or_else(|| probed.mb_track_id.clone());
        new_entries.insert(
            c.rel_path.clone(),
            IndexedFile { id: id.clone(), rel_path: c.rel_path.clone(), stamp: c.stamp, probed: probed.clone(), mb_recording_key: mb_rec },
        );

        let (folder_cover_rel, folder_cover_path) = match folder_covers.get(&c.folder_key).and_then(|v| v.as_ref()) {
            Some((r, p)) => (Some(r.clone()), Some(p.clone())),
            None => (None, None),
        };
        raw_files.push(RawScannedFile {
            id,
            path: c.path,
            rel_path: c.rel_path,
            rel_folder: c.rel_folder,
            folder_cover_rel,
            folder_cover_path,
            size: c.size,
            modified_secs: c.stamp.map(|s| s.mtime_secs),
            probed,
        });
    }

    on_progress(ScanProgress { scanned: total, total, skipped: skipped_paths.len(), current: None });

    let moves = if prev_valid && !cancelled { detect_moves(&prev.files, &new_entries) } else { Vec::new() };
    let grouped = group::group(raw_files);

    let stamp = compute_scan_stamp(&new_entries);
    let index = ScanIndex { version: INDEX_VERSION, files: new_entries, covers: grouped.covers, track_paths: grouped.track_paths, stamp };

    ScanResult {
        tracks: grouped.tracks,
        albums: grouped.albums,
        artists: grouped.artists,
        skipped: skipped_paths.len(),
        skipped_paths,
        moves,
        reused,
        rescanned,
        cancelled,
        index,
    }
}

/// Read cover image bytes for a `cover_art` id produced by this crate.
pub fn read_cover(index: &ScanIndex, cover_id: &str) -> Result<Option<Vec<u8>>, Error> {
    let Some(path) = index.cover_path(cover_id) else {
        return Ok(None);
    };
    if cover_id.starts_with("emb-") {
        return probe::read_embedded_cover(path).map_err(Error::Probe);
    }
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::Io(e)),
    }
}

/// Read cover image bytes embedded in `track_path`.
pub fn read_embedded_cover(track_path: &Path) -> Result<Option<Vec<u8>>, Error> {
    probe::read_embedded_cover(track_path).map_err(Error::Probe)
}

/// Read lyrics for `track_path` (sidecar `.lrc` first, then embedded tag).
pub fn read_lyrics(track_path: &Path) -> Result<Option<TrackLyrics>, Error> {
    lyrics::read_lyrics(track_path).map_err(Error::Probe)
}

/// Parse raw text as timed LRC if timestamps are present, else plain lyrics.
pub fn parse_lyrics_text(raw: &str) -> Option<TrackLyrics> {
    lyrics::parse_lyrics_text(raw)
}

fn detect_moves(prev: &BTreeMap<String, IndexedFile>, curr: &BTreeMap<String, IndexedFile>) -> Vec<TrackMove> {
    let prev_ids: HashMap<&str, &IndexedFile> = prev.values().map(|f| (f.id.as_str(), f)).collect();
    let curr_ids: HashMap<&str, &IndexedFile> = curr.values().map(|f| (f.id.as_str(), f)).collect();

    let vanished: Vec<&IndexedFile> = prev.values().filter(|f| !curr_ids.contains_key(f.id.as_str())).collect();
    let appeared: Vec<&IndexedFile> = curr.values().filter(|f| !prev_ids.contains_key(f.id.as_str())).collect();
    if vanished.is_empty() || appeared.is_empty() {
        return Vec::new();
    }

    let mut used_appeared: HashSet<usize> = HashSet::new();
    let mut used_vanished: HashSet<usize> = HashSet::new();
    let mut moves = Vec::new();

    // First pass: pair unique MusicBrainz recording ids.
    let mut appeared_by_mbid: HashMap<&str, Vec<usize>> = HashMap::new();
    for (idx, a) in appeared.iter().enumerate() {
        if let Some(mbid) = a.mb_recording_key.as_deref() {
            appeared_by_mbid.entry(mbid).or_default().push(idx);
        }
    }
    let mut vanished_mbid_counts: HashMap<&str, usize> = HashMap::new();
    for v in &vanished {
        if let Some(mbid) = v.mb_recording_key.as_deref() {
            *vanished_mbid_counts.entry(mbid).or_default() += 1;
        }
    }
    for (vi, v) in vanished.iter().enumerate() {
        let Some(mbid) = v.mb_recording_key.as_deref() else {
            continue;
        };
        if vanished_mbid_counts.get(mbid).copied().unwrap_or(0) != 1 {
            continue;
        }
        if let Some(candidates) = appeared_by_mbid.get(mbid)
            && candidates.len() == 1
        {
            let ai = candidates[0];
            if used_appeared.insert(ai) {
                used_vanished.insert(vi);
                moves.push(TrackMove { from: v.id.clone(), to: appeared[ai].id.clone() });
            }
        }
    }

    // Second pass: pair unique tag + length signatures for remaining tracks.
    #[derive(Hash, PartialEq, Eq)]
    struct MoveSig<'a> {
        title: &'a str,
        artists: &'a [String],
        album: Option<&'a str>,
        album_artists: &'a [String],
        track: Option<u32>,
        disc: Option<u32>,
        duration_ms: Option<u64>,
    }

    fn sig(f: &IndexedFile) -> MoveSig<'_> {
        MoveSig {
            title: &f.probed.title,
            artists: &f.probed.artists,
            album: f.probed.album.as_deref(),
            album_artists: &f.probed.album_artists,
            track: f.probed.track,
            disc: f.probed.disc,
            duration_ms: f.probed.duration_ms,
        }
    }

    let mut appeared_by_sig: HashMap<MoveSig<'_>, Vec<usize>> = HashMap::new();
    for (idx, a) in appeared.iter().enumerate() {
        if !used_appeared.contains(&idx) {
            appeared_by_sig.entry(sig(a)).or_default().push(idx);
        }
    }
    let mut vanished_sig_counts: HashMap<MoveSig<'_>, usize> = HashMap::new();
    for (vi, v) in vanished.iter().enumerate() {
        if !used_vanished.contains(&vi) {
            *vanished_sig_counts.entry(sig(v)).or_default() += 1;
        }
    }

    for (vi, v) in vanished.iter().enumerate() {
        if used_vanished.contains(&vi) {
            continue;
        }
        let s = sig(v);
        if vanished_sig_counts.get(&s).copied().unwrap_or(0) != 1 {
            continue;
        }
        if let Some(candidates) = appeared_by_sig.get(&s)
            && candidates.len() == 1
        {
            let ai = candidates[0];
            if used_appeared.insert(ai) {
                used_vanished.insert(vi);
                moves.push(TrackMove { from: v.id.clone(), to: appeared[ai].id.clone() });
            }
        }
    }

    moves
}

fn compute_scan_stamp(files: &BTreeMap<String, IndexedFile>) -> String {
    let mut hasher = Sha256::new();
    for (rel, f) in files {
        hasher.update(rel.as_bytes());
        hasher.update(b"\0");
        if let Some(s) = f.stamp {
            hasher.update(s.size.to_le_bytes());
            hasher.update(s.mtime_secs.to_le_bytes());
            hasher.update(s.mtime_nanos.to_le_bytes());
        }
        hasher.update(b"\0");
    }
    hex_prefix(&hasher.finalize())
}

pub(crate) fn hash_id(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    hex_prefix(&digest)
}

fn hex_prefix(digest: &[u8]) -> String {
    let mut out = String::with_capacity(32);
    for &b in &digest[..16] {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn is_hidden(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'))
}

fn is_audio_file(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()).is_some_and(|ext| {
        let lower = ext.to_ascii_lowercase();
        AUDIO_EXTS.contains(&lower.as_str())
    })
}

fn normalize_rel(rel: &Path) -> String {
    let parts: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    parts.join("/")
}

fn file_stamp(meta: &std::fs::Metadata) -> Option<FileStamp> {
    let modified = meta.modified().ok()?;
    let (mtime_secs, mtime_nanos) = match modified.duration_since(UNIX_EPOCH) {
        Ok(d) => (i64::try_from(d.as_secs()).ok()?, d.subsec_nanos()),
        Err(e) => {
            let d = e.duration();
            let secs = i64::try_from(d.as_secs()).ok()?;
            (-secs, d.subsec_nanos())
        }
    };
    Some(FileStamp { size: meta.len(), mtime_secs, mtime_nanos })
}

#[cfg(test)]
mod tests;
