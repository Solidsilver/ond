//! Container probing and tag reading through symphonia.

use serde::{Deserialize, Serialize};
use std::path::Path;
use symphonia::core::codecs::audio::well_known::{
    CODEC_ID_FLAC, CODEC_ID_MP1, CODEC_ID_MP2, CODEC_ID_MP3, CODEC_ID_OPUS, CODEC_ID_PCM_ALAW, CODEC_ID_PCM_F32BE, CODEC_ID_PCM_F32LE, CODEC_ID_PCM_F64BE,
    CODEC_ID_PCM_F64LE, CODEC_ID_PCM_MULAW, CODEC_ID_PCM_S16BE, CODEC_ID_PCM_S16LE, CODEC_ID_PCM_S24BE, CODEC_ID_PCM_S24LE, CODEC_ID_PCM_S32BE,
    CODEC_ID_PCM_S32LE, CODEC_ID_PCM_U8, CODEC_ID_VORBIS,
};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, MetadataRevision, StandardTag, StandardVisualKey, Tag, well_known};
use symphonia::core::units::Timestamp;

/// A credit on a local file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeContributor {
    pub role: String,
    pub sub_role: Option<String>,
    pub name: String,
}

/// Raw audio properties and tags extracted from one audio file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbedFile {
    pub title: String,
    pub sort_title: Option<String>,
    pub artists: Vec<String>,
    pub sort_artist: Option<String>,
    pub album: Option<String>,
    pub sort_album: Option<String>,
    pub album_artists: Vec<String>,
    pub sort_album_artist: Option<String>,
    pub composer: Option<String>,
    pub track: Option<u32>,
    pub track_total: Option<u32>,
    pub disc: Option<u32>,
    pub disc_total: Option<u32>,
    pub compilation: bool,
    pub year: Option<i32>,
    pub genres: Vec<String>,
    pub labels: Vec<String>,
    pub contributors: Vec<ProbeContributor>,
    pub duration_secs: Option<u32>,
    pub duration_ms: Option<u64>,
    pub suffix: String,
    pub content_type: Option<String>,
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
    pub has_embedded_art: bool,
    pub has_embedded_lyrics: bool,
}

/// Probe `path`, verify it holds an audio track, and read its tags and properties.
pub fn probe_file(path: &Path, size: u64) -> Result<ProbedFile, String> {
    let mut reader = open_reader(path)?;
    let (audio_props, duration_ms) = extract_audio_props(reader.as_ref(), size)?;

    let mut out = ProbedFile {
        title: fallback_title(path),
        sort_title: None,
        artists: Vec::new(),
        sort_artist: None,
        album: None,
        sort_album: None,
        album_artists: Vec::new(),
        sort_album_artist: None,
        composer: None,
        track: None,
        track_total: None,
        disc: None,
        disc_total: None,
        compilation: false,
        year: None,
        genres: Vec::new(),
        labels: Vec::new(),
        contributors: Vec::new(),
        duration_secs: duration_ms.map(|ms| ((ms + 500) / 1000) as u32),
        duration_ms,
        suffix: extension_lower(path),
        content_type: content_type_for(path, &audio_props),
        bit_rate_kbps: audio_props.bit_rate_kbps,
        bit_depth: audio_props.bit_depth,
        sample_rate: audio_props.sample_rate,
        channels: audio_props.channels,
        track_gain: None,
        album_gain: None,
        track_peak: None,
        album_peak: None,
        mb_recording_id: None,
        mb_track_id: None,
        mb_release_track_id: None,
        mb_album_id: None,
        mb_release_group_id: None,
        mb_artist_ids: Vec::new(),
        mb_album_artist_ids: Vec::new(),
        has_embedded_art: false,
        has_embedded_lyrics: false,
    };

    let mut has_explicit_title = false;
    let revisions = collect_revisions(reader.as_mut());
    for rev in &revisions {
        if !rev.media.visuals.is_empty() || rev.per_track.iter().any(|t| !t.metadata.visuals.is_empty()) {
            out.has_embedded_art = true;
        }
        let tags = rev.media.tags.iter().chain(rev.per_track.iter().flat_map(|t| t.metadata.tags.iter()));
        for tag in tags {
            apply_tag(&mut out, tag, &mut has_explicit_title);
        }
    }

    Ok(out)
}

/// Extract embedded front cover (or first visual) bytes from `path`.
pub fn read_embedded_cover(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let mut reader = open_reader(path)?;
    let revisions = collect_revisions(reader.as_mut());
    let mut fallback: Option<Vec<u8>> = None;
    for rev in &revisions {
        let visuals = rev.media.visuals.iter().chain(rev.per_track.iter().flat_map(|t| t.metadata.visuals.iter()));
        for visual in visuals {
            if visual.data.is_empty() {
                continue;
            }
            if visual.usage == Some(StandardVisualKey::FrontCover) {
                return Ok(Some(visual.data.to_vec()));
            }
            if fallback.is_none() {
                fallback = Some(visual.data.to_vec());
            }
        }
    }
    Ok(fallback)
}

/// Extract embedded lyrics text from `path`.
pub fn read_embedded_lyrics(path: &Path) -> Result<Option<String>, String> {
    let mut reader = open_reader(path)?;
    let revisions = collect_revisions(reader.as_mut());
    for rev in &revisions {
        let tags = rev.media.tags.iter().chain(rev.per_track.iter().flat_map(|t| t.metadata.tags.iter()));
        for tag in tags {
            if let Some(text) = lyrics_from_tag(tag) {
                return Ok(Some(text));
            }
        }
    }
    Ok(None)
}

fn open_reader(path: &Path) -> Result<Box<dyn FormatReader>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    let ext = extension_lower(path);
    if !ext.is_empty() {
        hint.with_extension(&ext);
    }
    symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| format!("probe {}: {e}", path.display()))
}

/// Collect revisions ordered richest-first so ID3v2/Vorbis/FLAC tags win over truncated ID3v1 tags.
fn collect_revisions(reader: &mut dyn FormatReader) -> Vec<MetadataRevision> {
    let mut revs = Vec::new();
    let mut meta = reader.metadata();
    loop {
        if let Some(rev) = meta.current() {
            revs.push(rev.clone());
        }
        if meta.pop().is_none() {
            break;
        }
    }
    revs.sort_by_key(metadata_priority);
    revs
}

fn metadata_priority(rev: &MetadataRevision) -> u8 {
    if rev.info.metadata == well_known::METADATA_ID_ID3V1 { 2 } else { 0 }
}

struct AudioProps {
    sample_rate: Option<u32>,
    bit_depth: Option<u32>,
    channels: Option<u32>,
    bit_rate_kbps: Option<u32>,
    mime_hint: Option<&'static str>,
}

fn extract_audio_props(reader: &dyn FormatReader, file_size: u64) -> Result<(AudioProps, Option<u64>), String> {
    let track = reader.default_track(TrackType::Audio).ok_or_else(|| "no audio track".to_owned())?;
    let Some(params) = track.codec_params.as_ref().and_then(|p| p.audio()) else {
        return Err("track has no audio codec parameters".to_owned());
    };
    let sample_rate = params.sample_rate;
    let channels = params.channels.as_ref().map(|c| c.count() as u32);
    let bit_depth = params.bits_per_sample.or(params.bits_per_coded_sample);

    let duration_ms = match (track.num_frames, track.time_base, sample_rate) {
        (Some(frames), Some(tb), _) => Timestamp::try_from(frames).ok().and_then(|ts| tb.calc_time(ts)).map(|t| {
            let (secs, nanos) = t.parts();
            u64::try_from(secs).unwrap_or(0).saturating_mul(1000).saturating_add(u64::from(nanos / 1_000_000))
        }),
        (Some(frames), None, Some(sr)) if sr > 0 => Some(frames.saturating_mul(1000) / u64::from(sr)),
        _ => None,
    };

    let bit_rate_kbps = match duration_ms {
        Some(ms) if ms > 0 => Some(((file_size.saturating_mul(8)) / ms) as u32),
        _ => None,
    };

    let codec = params.codec;
    let mime_hint = if codec == CODEC_ID_FLAC {
        Some("audio/flac")
    } else if codec == CODEC_ID_MP1 || codec == CODEC_ID_MP2 || codec == CODEC_ID_MP3 {
        Some("audio/mpeg")
    } else if codec == CODEC_ID_VORBIS {
        Some("audio/ogg")
    } else if codec == CODEC_ID_OPUS {
        Some("audio/opus")
    } else if codec == CODEC_ID_PCM_S16LE
        || codec == CODEC_ID_PCM_S24LE
        || codec == CODEC_ID_PCM_S32LE
        || codec == CODEC_ID_PCM_S16BE
        || codec == CODEC_ID_PCM_S24BE
        || codec == CODEC_ID_PCM_S32BE
        || codec == CODEC_ID_PCM_U8
        || codec == CODEC_ID_PCM_F32LE
        || codec == CODEC_ID_PCM_F32BE
        || codec == CODEC_ID_PCM_F64LE
        || codec == CODEC_ID_PCM_F64BE
        || codec == CODEC_ID_PCM_ALAW
        || codec == CODEC_ID_PCM_MULAW
    {
        Some("audio/wav")
    } else {
        None
    };

    Ok((AudioProps { sample_rate, bit_depth, channels, bit_rate_kbps, mime_hint }, duration_ms))
}

fn extension_lower(path: &Path) -> String {
    path.extension().and_then(|s| s.to_str()).map(str::to_ascii_lowercase).unwrap_or_default()
}

fn content_type_for(path: &Path, props: &AudioProps) -> Option<String> {
    let mime = match extension_lower(path).as_str() {
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "mp2" | "mp1" => "audio/mpeg",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "m4a" | "m4b" | "mp4" | "aac" | "alac" => "audio/mp4",
        "wav" | "wave" => "audio/wav",
        "aif" | "aiff" | "aifc" => "audio/aiff",
        "caf" => "audio/x-caf",
        "mka" | "webm" => "audio/webm",
        _ => return props.mime_hint.map(str::to_owned),
    };
    Some(mime.to_owned())
}

fn fallback_title(path: &Path) -> String {
    path.file_stem().and_then(|s| s.to_str()).filter(|s| !s.trim().is_empty()).unwrap_or("Unknown Track").to_owned()
}

fn clean_value(s: &str) -> Option<String> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn push_unique(list: &mut Vec<String>, value: &str) {
    if let Some(v) = clean_value(value)
        && !list.iter().any(|existing| existing == &v)
    {
        list.push(v);
    }
}

fn parse_year_str(s: &str) -> Option<i32> {
    let digits: String = s.trim().chars().take_while(char::is_ascii_digit).collect();
    if digits.len() >= 4 { digits[..4].parse::<i32>().ok().filter(|&y| y > 0) } else { None }
}

fn parse_pair_numbers(s: &str) -> (Option<u32>, Option<u32>) {
    let trimmed = s.trim();
    if let Some((a, b)) = trimmed.split_once('/') {
        (a.trim().parse::<u32>().ok().filter(|&n| n > 0), b.trim().parse::<u32>().ok().filter(|&n| n > 0))
    } else {
        (trimmed.parse::<u32>().ok().filter(|&n| n > 0), None)
    }
}

fn parse_gain_or_peak(raw: &str) -> Option<f32> {
    let token = raw.split_whitespace().next()?;
    token.parse::<f32>().ok().filter(|v| v.is_finite())
}

fn lyrics_from_tag(tag: &Tag) -> Option<String> {
    if let Some(StandardTag::Lyrics(text)) = &tag.std {
        let trimmed = text.trim();
        return (!trimmed.is_empty()).then(|| trimmed.to_owned());
    }
    let key = tag.raw.key.to_ascii_uppercase();
    if key == "LYRICS" || key == "UNSYNCEDLYRICS" || key == "USLT" || key.ends_with(":LYRICS") {
        let s = tag.raw.value.to_string();
        let trimmed = s.trim();
        return (!trimmed.is_empty()).then(|| trimmed.to_owned());
    }
    None
}

fn apply_tag(out: &mut ProbedFile, tag: &Tag, has_explicit_title: &mut bool) {
    match &tag.std {
        Some(StandardTag::TrackTitle(v)) => {
            if !*has_explicit_title && let Some(clean) = clean_value(v) {
                out.title = clean;
                *has_explicit_title = true;
            }
        }
        Some(StandardTag::SortTrackTitle(v)) => {
            if out.sort_title.is_none() {
                out.sort_title = clean_value(v);
            }
        }
        Some(StandardTag::Artist(v)) => push_unique(&mut out.artists, v),
        Some(StandardTag::SortArtist(v)) => {
            if out.sort_artist.is_none() {
                out.sort_artist = clean_value(v);
            }
        }
        Some(StandardTag::Album(v)) => {
            if out.album.is_none() {
                out.album = clean_value(v);
            }
        }
        Some(StandardTag::SortAlbum(v)) => {
            if out.sort_album.is_none() {
                out.sort_album = clean_value(v);
            }
        }
        Some(StandardTag::AlbumArtist(v)) => push_unique(&mut out.album_artists, v),
        Some(StandardTag::SortAlbumArtist(v)) => {
            if out.sort_album_artist.is_none() {
                out.sort_album_artist = clean_value(v);
            }
        }
        Some(StandardTag::Composer(v)) => {
            if let Some(name) = clean_value(v) {
                if out.composer.is_none() {
                    out.composer = Some(name.clone());
                }
                push_contributor(&mut out.contributors, "composer", None, &name);
            }
        }
        Some(StandardTag::Lyricist(v)) => {
            if let Some(name) = clean_value(v) {
                push_contributor(&mut out.contributors, "lyricist", None, &name);
            }
        }
        Some(StandardTag::Conductor(v)) => {
            if let Some(name) = clean_value(v) {
                push_contributor(&mut out.contributors, "conductor", None, &name);
            }
        }
        Some(StandardTag::Producer(v)) => {
            if let Some(name) = clean_value(v) {
                push_contributor(&mut out.contributors, "producer", None, &name);
            }
        }
        Some(StandardTag::Arranger(v)) => {
            if let Some(name) = clean_value(v) {
                push_contributor(&mut out.contributors, "arranger", None, &name);
            }
        }
        Some(StandardTag::Engineer(v)) => {
            if let Some(name) = clean_value(v) {
                push_contributor(&mut out.contributors, "engineer", None, &name);
            }
        }
        Some(StandardTag::MixEngineer(v)) => {
            if let Some(name) = clean_value(v) {
                push_contributor(&mut out.contributors, "mixer", None, &name);
            }
        }
        Some(StandardTag::Remixer(v)) => {
            if let Some(name) = clean_value(v) {
                push_contributor(&mut out.contributors, "remixer", None, &name);
            }
        }
        Some(StandardTag::Performer(v)) => {
            if let Some(name) = clean_value(v) {
                push_contributor(&mut out.contributors, "performer", None, &name);
            }
        }
        Some(StandardTag::TrackNumber(n)) => {
            if out.track.is_none() && *n > 0 {
                out.track = u32::try_from(*n).ok();
            }
        }
        Some(StandardTag::TrackTotal(n)) => {
            if out.track_total.is_none() && *n > 0 {
                out.track_total = u32::try_from(*n).ok();
            }
        }
        Some(StandardTag::DiscNumber(n)) => {
            if out.disc.is_none() && *n > 0 {
                out.disc = u32::try_from(*n).ok();
            }
        }
        Some(StandardTag::DiscTotal(n)) => {
            if out.disc_total.is_none() && *n > 0 {
                out.disc_total = u32::try_from(*n).ok();
            }
        }
        Some(StandardTag::CompilationFlag(flag)) => {
            if *flag {
                out.compilation = true;
            }
        }
        Some(StandardTag::ReleaseYear(y)) | Some(StandardTag::RecordingYear(y)) | Some(StandardTag::OriginalReleaseYear(y)) => {
            if out.year.is_none() && *y > 0 {
                out.year = Some(i32::from(*y));
            }
        }
        Some(StandardTag::ReleaseDate(d)) | Some(StandardTag::RecordingDate(d)) | Some(StandardTag::OriginalReleaseDate(d)) => {
            if out.year.is_none() {
                out.year = parse_year_str(d);
            }
        }
        Some(StandardTag::Genre(v)) => push_unique(&mut out.genres, v),
        Some(StandardTag::Label(v)) => push_unique(&mut out.labels, v),
        Some(StandardTag::ReplayGainTrackGain(v)) => {
            if out.track_gain.is_none() {
                out.track_gain = parse_gain_or_peak(v);
            }
        }
        Some(StandardTag::ReplayGainAlbumGain(v)) => {
            if out.album_gain.is_none() {
                out.album_gain = parse_gain_or_peak(v);
            }
        }
        Some(StandardTag::ReplayGainTrackPeak(v)) => {
            if out.track_peak.is_none() {
                out.track_peak = parse_gain_or_peak(v);
            }
        }
        Some(StandardTag::ReplayGainAlbumPeak(v)) => {
            if out.album_peak.is_none() {
                out.album_peak = parse_gain_or_peak(v);
            }
        }
        Some(StandardTag::MusicBrainzRecordingId(v)) => {
            if out.mb_recording_id.is_none() {
                out.mb_recording_id = clean_value(v);
            }
        }
        Some(StandardTag::MusicBrainzTrackId(v)) => {
            if out.mb_track_id.is_none() {
                out.mb_track_id = clean_value(v);
            }
            // Vorbis comments commonly store the recording MBID under MUSICBRAINZ_TRACKID.
            if out.mb_recording_id.is_none() {
                out.mb_recording_id = clean_value(v);
            }
        }
        Some(StandardTag::MusicBrainzReleaseTrackId(v)) => {
            if out.mb_release_track_id.is_none() {
                out.mb_release_track_id = clean_value(v);
            }
        }
        Some(StandardTag::MusicBrainzAlbumId(v)) => {
            if out.mb_album_id.is_none() {
                out.mb_album_id = clean_value(v);
            }
        }
        Some(StandardTag::MusicBrainzReleaseGroupId(v)) => {
            if out.mb_release_group_id.is_none() {
                out.mb_release_group_id = clean_value(v);
            }
        }
        Some(StandardTag::MusicBrainzArtistId(v)) => push_unique(&mut out.mb_artist_ids, v),
        Some(StandardTag::MusicBrainzAlbumArtistId(v)) => push_unique(&mut out.mb_album_artist_ids, v),
        Some(StandardTag::Lyrics(v)) => {
            if !v.trim().is_empty() {
                out.has_embedded_lyrics = true;
            }
        }
        _ => apply_raw_tag(out, tag),
    }
}

fn push_contributor(list: &mut Vec<ProbeContributor>, role: &str, sub_role: Option<String>, name: &str) {
    if !list.iter().any(|c| c.role == role && c.sub_role == sub_role && c.name == name) {
        list.push(ProbeContributor { role: role.to_owned(), sub_role, name: name.to_owned() });
    }
}

fn apply_raw_tag(out: &mut ProbedFile, tag: &Tag) {
    let key = tag.raw.key.to_ascii_uppercase();
    let val = tag.raw.value.to_string();
    if let Some(clean) = clean_value(&val) {
        if key == "LYRICS" || key == "UNSYNCEDLYRICS" || key == "USLT" || key.ends_with(":LYRICS") {
            out.has_embedded_lyrics = true;
            return;
        }
        if key == "REPLAYGAIN_TRACK_GAIN" || key.ends_with(":REPLAYGAIN_TRACK_GAIN") {
            out.track_gain = out.track_gain.or_else(|| parse_gain_or_peak(&clean));
        } else if key == "REPLAYGAIN_ALBUM_GAIN" || key.ends_with(":REPLAYGAIN_ALBUM_GAIN") {
            out.album_gain = out.album_gain.or_else(|| parse_gain_or_peak(&clean));
        } else if key == "REPLAYGAIN_TRACK_PEAK" || key.ends_with(":REPLAYGAIN_TRACK_PEAK") {
            out.track_peak = out.track_peak.or_else(|| parse_gain_or_peak(&clean));
        } else if key == "REPLAYGAIN_ALBUM_PEAK" || key.ends_with(":REPLAYGAIN_ALBUM_PEAK") {
            out.album_peak = out.album_peak.or_else(|| parse_gain_or_peak(&clean));
        } else if key == "COMPILATION" || key == "TCMP" || key == "ITUNESCOMPILATION" {
            if matches!(clean.to_ascii_lowercase().as_str(), "1" | "true" | "yes") {
                out.compilation = true;
            }
        } else if key == "ALBUMSORT" || key == "ALBUMSORTORDER" || key == "TSOA" {
            if out.sort_album.is_none() {
                out.sort_album = Some(clean);
            }
        } else if key == "ALBUMARTISTSORT" || key == "ALBUMARTISTSORTORDER" || key == "TSO2" {
            if out.sort_album_artist.is_none() {
                out.sort_album_artist = Some(clean);
            }
        } else if key == "ARTISTSORT" || key == "ARTISTSORTORDER" || key == "TSOP" {
            if out.sort_artist.is_none() {
                out.sort_artist = Some(clean);
            }
        } else if key == "TITLESORT" || key == "TITLESORTORDER" || key == "TSOT" {
            if out.sort_title.is_none() {
                out.sort_title = Some(clean);
            }
        } else if key == "ALBUMARTIST" || key == "ALBUM ARTIST" || key == "TPE2" {
            push_unique(&mut out.album_artists, &clean);
        } else if key == "MUSICBRAINZ RECORDING ID" || key == "MUSICBRAINZ_RECORDINGID" || key.ends_with(":MUSICBRAINZ RECORDING ID") {
            if out.mb_recording_id.is_none() {
                out.mb_recording_id = Some(clean);
            }
        } else if key == "MUSICBRAINZ TRACK ID" || key == "MUSICBRAINZ_TRACKID" || key.ends_with(":MUSICBRAINZ TRACK ID") {
            if out.mb_track_id.is_none() {
                out.mb_track_id = Some(clean.clone());
            }
            if out.mb_recording_id.is_none() {
                out.mb_recording_id = Some(clean);
            }
        } else if key == "MUSICBRAINZ ALBUM ID" || key == "MUSICBRAINZ_ALBUMID" || key.ends_with(":MUSICBRAINZ ALBUM ID") {
            if out.mb_album_id.is_none() {
                out.mb_album_id = Some(clean);
            }
        } else if key == "MUSICBRAINZ RELEASE GROUP ID" || key == "MUSICBRAINZ_RELEASEGROUPID" || key.ends_with(":MUSICBRAINZ RELEASE GROUP ID") {
            if out.mb_release_group_id.is_none() {
                out.mb_release_group_id = Some(clean);
            }
        } else if key == "MUSICBRAINZ ARTIST ID" || key == "MUSICBRAINZ_ARTISTID" || key.ends_with(":MUSICBRAINZ ARTIST ID") {
            push_unique(&mut out.mb_artist_ids, &clean);
        } else if key == "MUSICBRAINZ ALBUM ARTIST ID" || key == "MUSICBRAINZ_ALBUMARTISTID" || key.ends_with(":MUSICBRAINZ ALBUM ARTIST ID") {
            push_unique(&mut out.mb_album_artist_ids, &clean);
        } else if key == "TRACKNUMBER" || key == "TRCK" {
            let (n, total) = parse_pair_numbers(&clean);
            if out.track.is_none() {
                out.track = n;
            }
            if out.track_total.is_none() {
                out.track_total = total;
            }
        } else if key == "DISCNUMBER" || key == "TPOS" {
            let (n, total) = parse_pair_numbers(&clean);
            if out.disc.is_none() {
                out.disc = n;
            }
            if out.disc_total.is_none() {
                out.disc_total = total;
            }
        } else if key == "DATE" || key == "YEAR" || key == "ORIGINALDATE" || key == "TYER" || key == "TDRC" {
            if out.year.is_none() {
                out.year = parse_year_str(&clean);
            }
        } else if key == "LABEL" || key == "ORGANIZATION" || key == "PUBLISHER" || key == "TPUB" {
            push_unique(&mut out.labels, &clean);
        }
    }
}
