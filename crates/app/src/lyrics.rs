//! Song lyrics: picking the best version the server has, and following
//! synced lines along with playback.

/// Lyrics ready to show.
#[derive(Debug, Clone, PartialEq)]
pub enum Lyrics {
    /// Timed lines, sorted by start, offset already applied.
    Synced(Vec<Line>),
    /// Words without timing, one entry per line.
    Plain(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// Milliseconds into the track.
    pub start_ms: i64,
    pub text: String,
}

/// Where a track's lyrics stand.
#[derive(Debug, Clone, PartialEq)]
pub enum Fetch {
    Loading,
    /// `None`: the server has none.
    Done(Option<Lyrics>),
}

/// The best of the server's versions: synced over unsynced, then the one with
/// the most words (a sidecar `.lrc` usually beats a stub tag).
pub fn pick(versions: Vec<subsonic::StructuredLyrics>) -> Option<Lyrics> {
    let mut best: Option<(bool, usize, Lyrics)> = None;
    for v in versions {
        let synced = v.synced && v.line.iter().any(|l| l.start.is_some());
        let lyrics = if synced {
            let offset = v.offset.unwrap_or(0);
            let mut lines: Vec<Line> = v
                .line
                .into_iter()
                .filter_map(|l| Some(Line { start_ms: (l.start? + offset).max(0), text: l.value.trim_end().to_owned() }))
                .collect();
            lines.sort_by_key(|l| l.start_ms);
            Lyrics::Synced(lines)
        } else {
            Lyrics::Plain(v.line.into_iter().map(|l| l.value.trim_end().to_owned()).collect())
        };
        let words = word_count(&lyrics);
        if words == 0 {
            continue;
        }
        if best.as_ref().is_none_or(|(s, w, _)| (synced, words) > (*s, *w)) {
            best = Some((synced, words, lyrics));
        }
    }
    best.map(|(_, _, l)| l)
}

/// Plain text from `getLyrics`, split into lines; blank results are none.
pub fn from_text(text: &str) -> Option<Lyrics> {
    let lines: Vec<String> = text.replace("\r\n", "\n").lines().map(|l| l.trim_end().to_owned()).collect();
    let l = Lyrics::Plain(lines);
    (word_count(&l) > 0).then_some(l)
}

fn word_count(l: &Lyrics) -> usize {
    match l {
        Lyrics::Synced(lines) => lines.iter().map(|l| l.text.split_whitespace().count()).sum(),
        Lyrics::Plain(lines) => lines.iter().map(|l| l.split_whitespace().count()).sum(),
    }
}

/// The line being sung at `pos_ms`: the last one that has started. Before the
/// first line, none.
pub fn current(lines: &[Line], pos_ms: i64) -> Option<usize> {
    lines.partition_point(|l| l.start_ms <= pos_ms).checked_sub(1)
}

/// Milliseconds until the next line starts, for scheduling the next repaint.
pub fn until_next(lines: &[Line], pos_ms: i64) -> Option<i64> {
    let next = lines.partition_point(|l| l.start_ms <= pos_ms);
    lines.get(next).map(|l| l.start_ms - pos_ms)
}

/// Auto-follow: the view tracks the sung line unless the user scrolled away;
/// it picks back up after [`RESUME_SECS`] or when asked.
#[derive(Debug, Clone, Copy, Default)]
pub struct Follow {
    /// When the user last scrolled by hand (app clock, seconds).
    manual_at: Option<f64>,
}

pub const RESUME_SECS: f64 = 4.0;

impl Follow {
    pub fn scrolled_by_hand(&mut self, now: f64) {
        self.manual_at = Some(now);
    }

    pub fn resume(&mut self) {
        self.manual_at = None;
    }

    pub fn following(&self, now: f64) -> bool {
        self.manual_at.is_none_or(|t| now - t >= RESUME_SECS)
    }

    /// Seconds until following resumes on its own, while paused.
    pub fn resumes_in(&self, now: f64) -> Option<f64> {
        self.manual_at.map(|t| RESUME_SECS - (now - t)).filter(|s| *s > 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use subsonic::{LyricLine, StructuredLyrics};

    fn ln(start: Option<i64>, v: &str) -> LyricLine {
        LyricLine { start, value: v.into() }
    }

    fn lines(starts: &[i64]) -> Vec<Line> {
        starts.iter().map(|&s| Line { start_ms: s, text: format!("at {s}") }).collect()
    }

    #[test]
    fn picks_synced_over_plain_and_applies_offset() {
        let plain = StructuredLyrics { synced: false, line: vec![ln(None, "many many words here"), ln(None, "and more words")], ..Default::default() };
        let synced = StructuredLyrics { synced: true, offset: Some(-200), line: vec![ln(Some(1000), "one"), ln(Some(100), "zero")], ..Default::default() };
        let got = pick(vec![plain, synced]).unwrap();
        // Sorted, shifted, never negative.
        assert_eq!(got, Lyrics::Synced(vec![Line { start_ms: 0, text: "zero".into() }, Line { start_ms: 800, text: "one".into() }]));
    }

    #[test]
    fn empty_versions_are_none() {
        let blank = StructuredLyrics { synced: true, line: vec![ln(Some(0), "  "), ln(Some(10), "")], ..Default::default() };
        assert_eq!(pick(vec![blank]), None);
        assert_eq!(pick(Vec::new()), None);
        assert_eq!(from_text("\n  \n"), None);
        assert_eq!(from_text("a\r\nb"), Some(Lyrics::Plain(vec!["a".into(), "b".into()])));
    }

    #[test]
    fn synced_flag_without_times_counts_as_plain() {
        let odd = StructuredLyrics { synced: true, line: vec![ln(None, "no times at all")], ..Default::default() };
        assert!(matches!(pick(vec![odd]), Some(Lyrics::Plain(_))));
    }

    #[test]
    fn current_line_follows_position() {
        let l = lines(&[1000, 4000, 4000, 9000]);
        assert_eq!(current(&l, 0), None);
        assert_eq!(current(&l, 999), None);
        assert_eq!(current(&l, 1000), Some(0));
        assert_eq!(current(&l, 3999), Some(0));
        // Two lines at once: the later one wins.
        assert_eq!(current(&l, 4000), Some(2));
        assert_eq!(current(&l, 60_000), Some(3));
        assert_eq!(until_next(&l, 0), Some(1000));
        assert_eq!(until_next(&l, 4500), Some(4500));
        assert_eq!(until_next(&l, 9000), None);
    }

    #[test]
    fn follow_pauses_after_a_manual_scroll() {
        let mut f = Follow::default();
        assert!(f.following(10.0));
        f.scrolled_by_hand(10.0);
        assert!(!f.following(11.0));
        assert!(f.resumes_in(11.0).is_some_and(|s| (s - 3.0).abs() < 1e-9));
        assert!(f.following(10.0 + RESUME_SECS));
        f.scrolled_by_hand(20.0);
        f.resume();
        assert!(f.following(20.5));
    }
}
