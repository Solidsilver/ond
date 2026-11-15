//! Album grouping, compilation detection, artist extraction, and folder cover lookup.

use crate::probe::{ProbeContributor, ProbedFile};
use crate::{ArtistRef, Contributor, ScannedAlbum, ScannedArtist, ScannedTrack, hash_id};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// Tracks together with the folder-level cover path discovered in each parent directory.
pub struct RawScannedFile {
    pub id: String,
    pub root_key: String,
    pub path: PathBuf,
    pub rel_path: String,
    pub rel_folder: String,
    pub folder_cover_rel: Option<String>,
    pub folder_cover_path: Option<PathBuf>,
    pub size: u64,
    pub modified_secs: Option<i64>,
    pub probed: ProbedFile,
}

/// Output of grouping scanned files into tracks, albums, artists, and cover paths.
pub struct GroupedLibrary {
    pub tracks: Vec<ScannedTrack>,
    pub albums: Vec<ScannedAlbum>,
    pub artists: Vec<ScannedArtist>,
    pub covers: BTreeMap<String, PathBuf>,
    pub track_paths: BTreeMap<String, PathBuf>,
}

/// Preferred folder artwork names, in priority order.
const FOLDER_ART_STEMS: &[&str] = &["cover", "folder", "front"];
const FOLDER_ART_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp"];

/// Find the highest-priority cover image inside `dir` (`cover`, `folder`, `front`).
pub fn find_folder_cover(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut candidates: Vec<(usize, usize, String, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let (Some(stem), Some(ext)) = (path.file_stem().and_then(|s| s.to_str()), path.extension().and_then(|s| s.to_str())) else {
            continue;
        };
        let stem_low = stem.to_ascii_lowercase();
        let ext_low = ext.to_ascii_lowercase();
        if let (Some(stem_rank), Some(ext_rank)) = (FOLDER_ART_STEMS.iter().position(|&s| s == stem_low), FOLDER_ART_EXTS.iter().position(|&e| e == ext_low)) {
            candidates.push((stem_rank, ext_rank, path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned(), path));
        }
    }
    candidates.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
    candidates.into_iter().next().map(|(_, _, _, p)| p)
}

/// Group raw scanned tracks into albums and artists, resolving compilations and covers.
pub fn group(files: Vec<RawScannedFile>) -> GroupedLibrary {
    let mut covers: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut track_paths: BTreeMap<String, PathBuf> = BTreeMap::new();

    // Detect folder+album combinations with mixed artists and no explicit album artist.
    let mut folder_album_artists: HashMap<(String, String, String), BTreeSet<String>> = HashMap::new();
    let mut folder_album_has_explicit_aa: HashMap<(String, String, String), bool> = HashMap::new();
    let mut folder_album_comp_flag: HashMap<(String, String, String), bool> = HashMap::new();

    for f in &files {
        let album_title = album_title_for(f);
        let key = (f.root_key.clone(), f.rel_folder.clone(), album_title.to_lowercase());
        if !f.probed.album_artists.is_empty() {
            folder_album_has_explicit_aa.insert(key.clone(), true);
        }
        if f.probed.compilation {
            folder_album_comp_flag.insert(key.clone(), true);
        }
        let artist_key = f.probed.artists.first().map(|a| a.to_lowercase()).unwrap_or_default();
        if !artist_key.is_empty() {
            folder_album_artists.entry(key).or_default().insert(artist_key);
        }
    }

    let compilation_folders: BTreeSet<(String, String, String)> = folder_album_artists
        .iter()
        .filter_map(|(k, artists)| {
            let explicit_aa = folder_album_has_explicit_aa.get(k).copied().unwrap_or(false);
            let comp_flag = folder_album_comp_flag.get(k).copied().unwrap_or(false);
            if !explicit_aa && (artists.len() > 1 || comp_flag) { Some(k.clone()) } else { None }
        })
        .collect();

    struct PendingAlbum {
        id: String,
        title: String,
        sort: Option<String>,
        album_artist: Option<String>,
        album_artist_sort: Option<String>,
        album_artist_mbid: Option<String>,
        artists: Vec<ArtistRef>,
        tracks: Vec<ScannedTrack>,
        folder_cover: Option<String>,
        embedded_cover: Option<String>,
        compilation: bool,
        mb_album_id: Option<String>,
        mb_release_group_id: Option<String>,
        created_secs: Option<i64>,
    }

    let mut albums_map: BTreeMap<String, PendingAlbum> = BTreeMap::new();

    for f in files {
        let album_title = album_title_for(&f);
        let folder_album_key = (f.root_key.clone(), f.rel_folder.clone(), album_title.to_lowercase());
        let is_folder_comp = compilation_folders.contains(&folder_album_key);

        let effective_album_artist = if !f.probed.album_artists.is_empty() {
            Some(f.probed.album_artists.join(", "))
        } else if is_folder_comp {
            Some("Various Artists".to_owned())
        } else if !f.probed.artists.is_empty() {
            Some(f.probed.artists.join(", "))
        } else {
            None
        };

        let is_compilation =
            f.probed.compilation || is_folder_comp || effective_album_artist.as_deref().is_some_and(|a| a.eq_ignore_ascii_case("Various Artists"));

        let aa_group_key = effective_album_artist.as_deref().unwrap_or("").to_lowercase();
        let album_id = hash_id(&format!("ond-album-v1\0{aa_group_key}\0{}", album_title.to_lowercase()));

        let emb_cover_id = f.probed.has_embedded_art.then(|| format!("emb-{}", f.id));
        if let Some(ref cid) = emb_cover_id {
            covers.insert(cid.clone(), f.path.clone());
        }
        let dir_cover_id = match (&f.folder_cover_rel, &f.folder_cover_path) {
            (Some(rel), Some(path)) => {
                let cid = format!("dir-{}", hash_id(&format!("ond-cover-v2\0{}\0{rel}", f.root_key)));
                covers.insert(cid.clone(), path.clone());
                Some(cid)
            }
            _ => None,
        };
        // Embedded art takes priority over folder images.
        let track_cover = emb_cover_id.clone().or_else(|| dir_cover_id.clone());

        track_paths.insert(f.id.clone(), f.path.clone());

        let artist_display = (!f.probed.artists.is_empty()).then(|| f.probed.artists.join(", "));
        let track_artist_refs = build_artist_refs(&f.probed.artists, f.probed.sort_artist.as_deref(), &f.probed.mb_artist_ids);
        let album_artist_refs = if !f.probed.album_artists.is_empty() {
            build_artist_refs(&f.probed.album_artists, f.probed.sort_album_artist.as_deref(), &f.probed.mb_album_artist_ids)
        } else if is_folder_comp {
            vec![ArtistRef { id: Some(artist_id("Various Artists")), name: "Various Artists".to_owned(), sort: None }]
        } else {
            track_artist_refs.clone()
        };

        let contributors = f
            .probed
            .contributors
            .iter()
            .map(|c: &ProbeContributor| Contributor {
                role: c.role.clone(),
                sub_role: c.sub_role.clone(),
                name: c.name.clone(),
                artist_id: Some(artist_id(&c.name)),
            })
            .collect();

        let track = ScannedTrack {
            id: f.id,
            root_key: f.root_key,
            path: f.path,
            rel_path: f.rel_path,
            album_id: album_id.clone(),
            title: f.probed.title,
            sort_title: f.probed.sort_title,
            artist: artist_display,
            sort_artist: f.probed.sort_artist,
            album_artist: effective_album_artist.clone(),
            sort_album_artist: f.probed.sort_album_artist.clone(),
            album: album_title.clone(),
            sort_album: f.probed.sort_album.clone(),
            artists: track_artist_refs,
            album_artists: album_artist_refs.clone(),
            track: f.probed.track,
            track_total: f.probed.track_total,
            disc: f.probed.disc,
            disc_total: f.probed.disc_total,
            compilation: is_compilation,
            year: f.probed.year,
            genres: f.probed.genres,
            labels: f.probed.labels,
            composer: f.probed.composer,
            contributors,
            duration_secs: f.probed.duration_secs,
            duration_ms: f.probed.duration_ms,
            suffix: f.probed.suffix,
            content_type: f.probed.content_type,
            size: f.size,
            bit_rate_kbps: f.probed.bit_rate_kbps,
            bit_depth: f.probed.bit_depth,
            sample_rate: f.probed.sample_rate,
            channels: f.probed.channels,
            track_gain: f.probed.track_gain,
            album_gain: f.probed.album_gain,
            track_peak: f.probed.track_peak,
            album_peak: f.probed.album_peak,
            mb_recording_id: f.probed.mb_recording_id,
            mb_track_id: f.probed.mb_track_id,
            mb_release_track_id: f.probed.mb_release_track_id,
            mb_album_id: f.probed.mb_album_id.clone(),
            mb_release_group_id: f.probed.mb_release_group_id.clone(),
            mb_artist_ids: f.probed.mb_artist_ids,
            mb_album_artist_ids: f.probed.mb_album_artist_ids.clone(),
            cover_art: track_cover,
            has_embedded_art: f.probed.has_embedded_art,
            has_embedded_lyrics: f.probed.has_embedded_lyrics,
            modified_secs: f.modified_secs,
        };

        let entry = albums_map.entry(album_id.clone()).or_insert_with(|| PendingAlbum {
            id: album_id,
            title: album_title,
            sort: track.sort_album.clone(),
            album_artist: effective_album_artist,
            album_artist_sort: track.sort_album_artist.clone(),
            album_artist_mbid: track.mb_album_artist_ids.first().cloned(),
            artists: album_artist_refs,
            tracks: Vec::new(),
            folder_cover: None,
            embedded_cover: None,
            compilation: is_compilation,
            mb_album_id: track.mb_album_id.clone(),
            mb_release_group_id: track.mb_release_group_id.clone(),
            created_secs: track.modified_secs,
        });

        if entry.sort.is_none() {
            entry.sort = track.sort_album.clone();
        }
        if entry.album_artist_sort.is_none() {
            entry.album_artist_sort = track.sort_album_artist.clone();
        }
        if entry.mb_album_id.is_none() {
            entry.mb_album_id = track.mb_album_id.clone();
        }
        if entry.mb_release_group_id.is_none() {
            entry.mb_release_group_id = track.mb_release_group_id.clone();
        }
        if entry.embedded_cover.is_none() {
            entry.embedded_cover = emb_cover_id;
        }
        if entry.folder_cover.is_none() {
            entry.folder_cover = dir_cover_id;
        }
        if is_compilation {
            entry.compilation = true;
        }
        if let Some(m) = track.modified_secs {
            entry.created_secs = Some(entry.created_secs.map_or(m, |cur| cur.max(m)));
        }
        entry.tracks.push(track);
    }

    let mut all_tracks = Vec::new();
    let mut albums = Vec::with_capacity(albums_map.len());
    let mut artist_counts: BTreeMap<String, ArtistAccum> = BTreeMap::new();

    for mut pending in albums_map.into_values() {
        pending.tracks.sort_by(|a, b| {
            (a.disc.unwrap_or(1), a.track.unwrap_or(u32::MAX), &a.rel_path, &a.id).cmp(&(b.disc.unwrap_or(1), b.track.unwrap_or(u32::MAX), &b.rel_path, &b.id))
        });
        let song_count = pending.tracks.len() as u32;
        let duration_secs: u32 = pending.tracks.iter().filter_map(|t| t.duration_secs).sum();
        let year = pending.tracks.iter().find_map(|t| t.year);
        let mut genres = Vec::new();
        let mut labels = Vec::new();
        for t in &pending.tracks {
            for g in &t.genres {
                if !genres.contains(g) {
                    genres.push(g.clone());
                }
            }
            for l in &t.labels {
                if !labels.contains(l) {
                    labels.push(l.clone());
                }
            }
        }
        let cover_art = pending.embedded_cover.or(pending.folder_cover);
        let primary_artist_id = pending.album_artist.as_deref().map(artist_id);

        for a in &pending.artists {
            let aid = a.id.clone().unwrap_or_else(|| artist_id(&a.name));
            let acc = artist_counts.entry(aid.clone()).or_insert_with(|| ArtistAccum {
                id: aid,
                name: a.name.clone(),
                sort: a.sort.clone(),
                album_ids: BTreeSet::new(),
                cover_art: None,
            });
            if acc.sort.is_none() {
                acc.sort = a.sort.clone();
            }
            if acc.cover_art.is_none() {
                acc.cover_art = cover_art.clone();
            }
            acc.album_ids.insert(pending.id.clone());
        }
        // Also record individual track artists if distinct from album artist so artist browse/links resolve.
        for t in &pending.tracks {
            for a in &t.artists {
                let aid = a.id.clone().unwrap_or_else(|| artist_id(&a.name));
                let acc = artist_counts.entry(aid.clone()).or_insert_with(|| ArtistAccum {
                    id: aid,
                    name: a.name.clone(),
                    sort: a.sort.clone(),
                    album_ids: BTreeSet::new(),
                    cover_art: None,
                });
                if acc.sort.is_none() {
                    acc.sort = a.sort.clone();
                }
                if acc.cover_art.is_none() {
                    acc.cover_art = t.cover_art.clone().or_else(|| cover_art.clone());
                }
                acc.album_ids.insert(pending.id.clone());
            }
        }

        let _ = pending.album_artist_sort;
        let _ = pending.album_artist_mbid;

        albums.push(ScannedAlbum {
            id: pending.id,
            name: pending.title,
            sort: pending.sort,
            artist: pending.album_artist,
            artist_id: primary_artist_id,
            artists: pending.artists,
            year,
            genres,
            labels,
            song_count,
            duration_secs,
            cover_art,
            compilation: pending.compilation,
            mb_album_id: pending.mb_album_id,
            mb_release_group_id: pending.mb_release_group_id,
            created_secs: pending.created_secs,
        });
        all_tracks.extend(pending.tracks);
    }

    let artists = artist_counts
        .into_values()
        .map(|a| ScannedArtist { id: a.id, name: a.name, sort: a.sort, album_count: a.album_ids.len() as u32, cover_art: a.cover_art })
        .collect();

    GroupedLibrary { tracks: all_tracks, albums, artists, covers, track_paths }
}

struct ArtistAccum {
    id: String,
    name: String,
    sort: Option<String>,
    album_ids: BTreeSet<String>,
    cover_art: Option<String>,
}

fn album_title_for(f: &RawScannedFile) -> String {
    if let Some(ref album) = f.probed.album {
        return album.clone();
    }
    f.path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()).filter(|s| !s.trim().is_empty()).unwrap_or("Unknown Album").to_owned()
}

pub fn artist_id(name: &str) -> String {
    hash_id(&format!("ond-artist-v1\0{}", name.to_lowercase()))
}

fn build_artist_refs(names: &[String], single_sort: Option<&str>, _mbids: &[String]) -> Vec<ArtistRef> {
    names
        .iter()
        .enumerate()
        .map(|(i, name)| ArtistRef { id: Some(artist_id(name)), name: name.clone(), sort: if i == 0 { single_sort.map(str::to_owned) } else { None } })
        .collect()
}
