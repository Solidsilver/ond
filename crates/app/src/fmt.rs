//! Text for numbers the UI shows: times, lengths, sizes, gains, counts. One
//! place, so the same quantity reads the same everywhere.

use library::Track;

/// A playback position or track length: "3:07", "1:02:45".
pub fn clock(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// A day from an ISO date ("2024-01-02", or with a time after it) as the
/// lists show it: "Jan 2, 2024". Anything else as it is.
pub fn date(iso: &str) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mut parts = iso.get(..10).unwrap_or(iso).splitn(3, '-').map(str::parse::<u32>);
    match (parts.next(), parts.next(), parts.next()) {
        (Some(Ok(y)), Some(Ok(m @ 1..=12)), Some(Ok(d))) => format!("{} {d}, {y}", MONTHS[m as usize - 1]),
        _ => iso.to_owned(),
    }
}

/// How long a list plays: "41 min", "1 hr 12 min" (minutes rounded up).
pub fn span(secs: u32) -> String {
    let mins = secs.div_ceil(60);
    if mins >= 60 {
        let m = mins % 60;
        if m == 0 { format!("{} hr", mins / 60) } else { format!("{} hr {m} min", mins / 60) }
    } else {
        format!("{mins} min")
    }
}

/// A rough estimate: "under a minute", "about 25 min", "about 3 hr".
pub fn about(secs: f64) -> String {
    let mins = (secs / 60.0).round();
    if mins < 1.0 {
        "under a minute".into()
    } else if mins < 90.0 {
        format!("about {mins} min")
    } else {
        format!("about {} hr", (mins / 60.0).round())
    }
}

/// "1.2 GB", "340 MB", "512 bytes".
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    match u {
        0 => format!("{n} bytes"),
        _ if v < 10.0 => format!("{v:.1} {}", UNITS[u]),
        _ => format!("{v:.0} {}", UNITS[u]),
    }
}

/// Swaps ASCII hyphens for the typographic minus sign (−).
pub fn minus(s: String) -> String {
    if s.contains('-') { s.replace('-', "−") } else { s }
}

/// A gain or level with one decimal and its unit, with a true minus sign:
/// "+1.5 dB", "−6.0 dB", "−16.9 LUFS".
pub fn level(v: f64, unit: &str) -> String {
    minus(format!("{v:.1} {unit}"))
}

/// A signed gain: "+1.5 dB", "−6.0 dB", and "0 dB" for anything that rounds to it.
pub fn gain(db: f32) -> String {
    if db.abs() < 0.05 { "0 dB".into() } else { minus(format!("{db:+.1} dB")) }
}

/// "1 song", "3 songs" (for words that pluralise with an "s").
pub fn plural(n: usize, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") }
}

/// 1866 → "1,866".
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// A 1–5 rating in words: "3 stars". The UI draws stars as icons, never text.
pub fn stars(rating: u8) -> String {
    plural(usize::from(rating.min(5)), "star")
}

/// 44100 → "44.1", 48000 → "48" (the engine's rule, so the status line agrees).
pub use audio::khz;

/// The file's format: "FLAC 24/88.2", or just the codec when the server
/// omits depth and rate.
pub fn quality(t: &Track) -> String {
    let codec = t.suffix.as_deref().unwrap_or("?").to_uppercase();
    match (t.bit_depth, t.sample_rate) {
        (Some(d), Some(r)) if d > 0 => format!("{codec} {d}/{}", khz(r)),
        (_, Some(r)) => format!("{codec} {}", khz(r)),
        _ => codec,
    }
}

/// The analysis as label/value rows: "−11.2 LUFS", "−0.4 dBTP", "DR9".
pub fn measurements(a: &analysis::TrackAnalysis) -> Vec<(&'static str, String)> {
    let mut rows = Vec::new();
    if let Some(l) = a.integrated_lufs {
        rows.push(("Loudness", level(l, "LUFS")));
    }
    if let Some(r) = a.loudness_range_lu {
        rows.push(("Range", format!("{r:.1} LU")));
    }
    if let Some(p) = a.true_peak_dbtp {
        rows.push(("True peak", level(p, "dBTP")));
    }
    if let Some(dr) = a.dr_rounded() {
        rows.push(("Dynamic range", format!("DR{dr}")));
    }
    if let Some(b) = a.effective_bits {
        rows.push(("Bits used", format!("{b}")));
    }
    rows
}

/// The analysis in one line: "DR9 · −11.2 LUFS · peak −0.4 dBTP".
pub fn loudness_line(a: &analysis::TrackAnalysis) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(dr) = a.dr_rounded() {
        parts.push(format!("DR{dr}"));
    }
    if let Some(l) = a.integrated_lufs {
        parts.push(level(l, "LUFS"));
    }
    if let Some(p) = a.true_peak_dbtp {
        parts.push(format!("peak {}", level(p, "dBTP")));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// What the file is and how it plays, as label/value rows: shared by the Info
/// panel and the Player's details so both read the same.
pub fn track_facts(t: &Track, playback: Option<String>) -> Vec<(&'static str, String)> {
    let mut rows = Vec::new();
    if t.suffix.is_some() || t.sample_rate.is_some() {
        rows.push(("Format", quality(t)));
    }
    if let Some(k) = t.bit_rate_kbps.filter(|k| *k > 0) {
        rows.push(("Bitrate", format!("{k} kbps")));
    }
    if let Some(c) = t.channels {
        rows.push((
            "Channels",
            match c {
                1 => "Mono".into(),
                2 => "Stereo".into(),
                n => format!("{n} channels"),
            },
        ));
    }
    if let Some(d) = t.duration_secs {
        rows.push(("Length", clock(f64::from(d))));
    }
    if let Some(b) = t.size {
        rows.push(("Size", bytes(b)));
    }
    if let Some(rg) = replay_gain(t) {
        rows.push(("ReplayGain", rg));
    }
    if let Some(p) = playback {
        rows.push(("Playback", p));
    }
    rows
}

/// "track −6.0 dB · album −7.0 dB · peak 0.99", or `None` without tags.
pub fn replay_gain(t: &Track) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(g) = t.track_gain {
        parts.push(format!("track {}", minus(format!("{g:+.1} dB"))));
    }
    if let Some(g) = t.album_gain {
        parts.push(format!("album {}", minus(format!("{g:+.1} dB"))));
    }
    if let Some(p) = t.track_peak {
        parts.push(format!("peak {p:.2}"));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_and_lengths() {
        assert_eq!(clock(187.4), "3:07");
        assert_eq!(clock(3765.0), "1:02:45");
        assert_eq!(span(41 * 60), "41 min");
        assert_eq!(span(41 * 60 + 1), "42 min");
        assert_eq!(span(72 * 60), "1 hr 12 min");
        assert_eq!(span(120 * 60), "2 hr");
        assert_eq!(about(20.0), "under a minute");
        assert_eq!(about(25.0 * 60.0), "about 25 min");
        assert_eq!(about(3.0 * 3600.0), "about 3 hr");
    }

    #[test]
    fn dates() {
        assert_eq!(date("2024-01-02T10:00:00Z"), "Jan 2, 2024");
        assert_eq!(date("1999-12-31"), "Dec 31, 1999");
        assert_eq!(date("sometime"), "sometime");
        assert_eq!(date("2024-13-01"), "2024-13-01");
    }

    #[test]
    fn sizes_counts_and_signs() {
        assert_eq!(bytes(512), "512 bytes");
        assert_eq!(bytes(1_234_000), "1.2 MB");
        assert_eq!(bytes(42_000_000_000), "42 GB");
        assert_eq!(gain(-6.0), "−6.0 dB");
        assert_eq!(gain(0.02), "0 dB");
        assert_eq!(gain(1.5), "+1.5 dB");
        assert_eq!(level(-16.94, "LUFS"), "−16.9 LUFS");
        assert_eq!(plural(1, "song"), "1 song");
        assert_eq!(plural(3, "song"), "3 songs");
        assert_eq!(thousands(1866), "1,866");
        assert_eq!(thousands(25_000), "25,000");
        assert_eq!(stars(3), "3 stars");
        assert_eq!(stars(1), "1 star");
        assert_eq!(capitalize("double bass"), "Double bass");
    }

    #[test]
    fn quality_reads_like_the_status_line() {
        let mut t = Track {
            id: "t".into(),
            album_id: "a".into(),
            title: "x".into(),
            artist: None,
            artist_ref: None,
            track: None,
            disc: None,
            duration_secs: None,
            suffix: None,
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
        };
        assert_eq!(quality(&t), "?");
        t.suffix = Some("flac".into());
        t.bit_depth = Some(24);
        t.sample_rate = Some(88_200);
        assert_eq!(quality(&t), "FLAC 24/88.2");
        t.bit_depth = None;
        t.sample_rate = Some(48_000);
        assert_eq!(quality(&t), "FLAC 48");
    }
}
