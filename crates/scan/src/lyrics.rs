//! Lyric discovery and parsing: sidecar `.lrc` beside the track, then embedded tags.

use crate::probe;
use std::path::{Path, PathBuf};

/// One timed line parsed from LRC text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimedLine {
    pub start_ms: i64,
    pub text: String,
}

/// Lyrics extracted for a local track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackLyrics {
    /// Timestamped LRC lines sorted by start time, with optional global `[offset:...]` in ms.
    Synced { offset_ms: Option<i64>, lines: Vec<TimedLine> },
    /// Plain unsynced lyric text.
    Plain(String),
}

/// Read lyrics for `track_path`: `.lrc` beside the file first, then embedded tags.
pub fn read_lyrics(track_path: &Path) -> Result<Option<TrackLyrics>, String> {
    if let Some(lrc_path) = find_sidecar_lrc(track_path)
        && let Ok(bytes) = std::fs::read(&lrc_path)
    {
        let raw = String::from_utf8_lossy(&bytes);
        if let Some(parsed) = parse_lyrics_text(&raw) {
            return Ok(Some(parsed));
        }
    }

    if let Some(embedded) = probe::read_embedded_lyrics(track_path)?
        && let Some(parsed) = parse_lyrics_text(&embedded)
    {
        return Ok(Some(parsed));
    }

    Ok(None)
}

fn find_sidecar_lrc(track_path: &Path) -> Option<PathBuf> {
    let direct = track_path.with_extension("lrc");
    if direct.is_file() {
        return Some(direct);
    }
    let stem = track_path.file_stem()?.to_str()?;
    let parent = track_path.parent()?;
    let entries = std::fs::read_dir(parent).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && let (Some(s), Some(ext)) = (path.file_stem().and_then(|v| v.to_str()), path.extension().and_then(|v| v.to_str()))
            && ext.eq_ignore_ascii_case("lrc")
            && s == stem
        {
            return Some(path);
        }
    }
    None
}

/// Parse text as timed LRC if it contains `[mm:ss...]` timestamps, else plain text.
pub fn parse_lyrics_text(raw: &str) -> Option<TrackLyrics> {
    let cleaned = raw.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    if cleaned.trim().is_empty() {
        return None;
    }

    let mut offset_ms: Option<i64> = None;
    let mut timed: Vec<TimedLine> = Vec::new();
    let mut saw_timestamp = false;

    for line in cleaned.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(offset) = parse_offset_tag(trimmed) {
            offset_ms = Some(offset);
            continue;
        }
        let (stamps, text) = parse_line_timestamps(trimmed);
        if !stamps.is_empty() {
            saw_timestamp = true;
            for start_ms in stamps {
                timed.push(TimedLine { start_ms, text: text.to_owned() });
            }
        }
    }

    if saw_timestamp {
        timed.sort_by_key(|l| l.start_ms);
        return Some(TrackLyrics::Synced { offset_ms, lines: timed });
    }

    Some(TrackLyrics::Plain(cleaned))
}

fn parse_offset_tag(line: &str) -> Option<i64> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    let val = inner.strip_prefix("offset:").or_else(|| inner.strip_prefix("OFFSET:"))?;
    val.trim().parse::<i64>().ok()
}

fn parse_line_timestamps(mut line: &str) -> (Vec<i64>, &str) {
    let mut stamps = Vec::new();
    while let Some(rest) = line.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            break;
        };
        let tag = &rest[..end];
        if let Some(ms) = parse_timestamp(tag) {
            stamps.push(ms);
            line = rest[end + 1..].trim_start();
        } else {
            break;
        }
    }
    (stamps, line.trim())
}

fn parse_timestamp(tag: &str) -> Option<i64> {
    let (mins_s, sec_part) = tag.split_once(':')?;
    let mins: i64 = mins_s.trim().parse().ok()?;
    if mins < 0 {
        return None;
    }
    let (secs, millis) = if let Some((s, frac)) = sec_part.split_once('.') {
        let secs: i64 = s.parse().ok()?;
        let millis = parse_fraction_ms(frac)?;
        (secs, millis)
    } else if let Some((s, frac)) = sec_part.split_once(':') {
        let secs: i64 = s.parse().ok()?;
        let millis = parse_fraction_ms(frac)?;
        (secs, millis)
    } else {
        let secs: i64 = sec_part.parse().ok()?;
        (secs, 0)
    };
    if !(0..60).contains(&secs) {
        return None;
    }
    Some(mins * 60_000 + secs * 1_000 + millis)
}

fn parse_fraction_ms(frac: &str) -> Option<i64> {
    if frac.is_empty() || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let padded = match frac.len() {
        1 => format!("{frac}00"),
        2 => format!("{frac}0"),
        _ => frac[..3].to_owned(),
    };
    padded.parse::<i64>().ok()
}
