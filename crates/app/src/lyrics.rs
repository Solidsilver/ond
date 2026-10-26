//! Song lyrics: picking the best version the server has, cleaning up what
//! lyric scrapers leave behind, laying the words out as stanzas with section
//! labels, and following synced lines along with playback.

/// Lyrics ready to show.
#[derive(Debug, Clone, PartialEq)]
pub enum Lyrics {
    /// Timed lines, sorted by start, offset already applied.
    Synced(Vec<Line>),
    /// Words without timing, one entry per line, already cleaned.
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
            // A positive offset shows the lines sooner (OpenSubsonic, as in LRC).
            let offset = v.offset.unwrap_or(0);
            let mut lines: Vec<Line> = v
                .line
                .into_iter()
                .filter_map(|l| Some(Line { start_ms: l.start?.saturating_sub(offset).max(0), text: squash(&l.value) }))
                .collect();
            lines.sort_by_key(|l| l.start_ms);
            Lyrics::Synced(lines)
        } else {
            Lyrics::Plain(clean(v.line.into_iter().map(|l| l.value)))
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

/// Plain text from `getLyrics`, cleaned and split into lines; blank results
/// (or nothing but debris) are none.
pub fn from_text(text: &str) -> Option<Lyrics> {
    let l = Lyrics::Plain(clean(text.replace("\r\n", "\n").lines().map(str::to_owned)));
    (word_count(&l) > 0).then_some(l)
}

fn word_count(l: &Lyrics) -> usize {
    let words = |t: &str| if section_label(t).is_some() { 0 } else { t.split_whitespace().count() };
    match l {
        Lyrics::Synced(lines) => lines.iter().map(|l| words(&l.text)).sum(),
        Lyrics::Plain(lines) => lines.iter().map(|l| words(l)).sum(),
    }
}

/// One line with its whitespace tidied: no-break and odd spaces become plain
/// ones, runs collapse, ends are trimmed.
fn squash(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Plain lyrics with scraper debris removed: the "12 Contributors…Lyrics"
/// header, "You might also like", "See … Live / Get tickets as low as $…",
/// a trailing "Embed", and section markers glued onto lyric lines split off.
/// Whitespace is tidied and blank runs collapse to one stanza break.
pub fn clean(lines: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in lines {
        let mut line = squash(&raw);
        // The scraped page's header sits on the first real line, often glued
        // to the first section marker: "4 ContributorsTitle Lyrics[Verse 1]".
        if !out.iter().any(|l| !l.is_empty()) {
            line = strip_header(&line);
        }
        if line.is_empty() {
            out.push(line);
            continue;
        }
        // "You might also like" is glued between lines; it splits them.
        // Debris leaves nothing behind, not even a blank line.
        for piece in line.split("You might also like") {
            let piece = strip_tickets(piece.trim());
            if !piece.is_empty() {
                out.extend(split_markers(&piece));
            }
        }
    }
    // A trailing "Embed" (often with the page's counter: "123Embed").
    while let Some(last) = out.iter().rposition(|l| !l.is_empty()) {
        let Some(rest) = out[last].strip_suffix("Embed") else { break };
        let rest = rest.trim_end_matches(|c: char| c.is_ascii_digit()).trim().to_owned();
        if rest.is_empty() {
            out.remove(last);
        } else {
            out[last] = rest;
            break;
        }
    }
    // Blank runs become one break; none at either end.
    let mut tidy: Vec<String> = Vec::with_capacity(out.len());
    for l in out {
        if l.is_empty() && tidy.last().is_none_or(String::is_empty) {
            continue;
        }
        tidy.push(l);
    }
    while tidy.last().is_some_and(String::is_empty) {
        tidy.pop();
    }
    tidy
}

/// "4 ContributorsJesus, Friend Of Sinners Lyrics[Verse 1]" → "[Verse 1]".
fn strip_header(line: &str) -> String {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    let rest = line[digits..].trim_start();
    if digits > 0 && rest.starts_with("Contributor") {
        return match rest.find("Lyrics") {
            Some(i) => rest[i + "Lyrics".len()..].trim().to_owned(),
            None => String::new(),
        };
    }
    // "Song Title Lyrics[Intro]": the title header without the counter.
    if let Some(i) = line.find("Lyrics[") {
        return line[i + "Lyrics".len()..].to_owned();
    }
    line.to_owned()
}

/// Drops a concert ad ("See Casting Crowns LiveGet tickets as low as $45"),
/// keeping any lyric it was glued to.
fn strip_tickets(line: &str) -> String {
    const AD: &str = "Get tickets as low as";
    let Some(at) = line.find(AD) else { return line.to_owned() };
    let start = line[..at].rfind("See ").unwrap_or(at);
    let mut kept = line[..start].trim().to_owned();
    // Anything after the price (rare) is lyric text again.
    let after = line[at + AD.len()..].trim_start().trim_start_matches('$').trim_start_matches(|c: char| c.is_ascii_digit() || c == '.').trim();
    if !after.is_empty() {
        if !kept.is_empty() {
            kept.push(' ');
        }
        kept.push_str(after);
    }
    kept
}

/// Splits bracketed section markers out of a line: "a heart divided[Chorus]"
/// → ["a heart divided", "[Chorus]"]. Other brackets ("[laughs]") stay put.
fn split_markers(line: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find(']').map(|c| open + c) else { break };
        let marker = &rest[open..=close];
        if !names_section(&marker[1..marker.len() - 1]) {
            break;
        }
        let before = rest[..open].trim();
        if !before.is_empty() {
            parts.push(before.to_owned());
        }
        parts.push(marker.to_owned());
        rest = &rest[close + 1..];
    }
    let rest = rest.trim();
    if !rest.is_empty() || parts.is_empty() {
        parts.push(rest.to_owned());
    }
    parts
}

/// Words that name a part of a song, for markers written without brackets
/// ("Chorus:", "(Instrumental)") or inside them ("[Verse 1]").
const SECTION_WORDS: &[&str] = &[
    "verse", "chorus", "pre-chorus", "prechorus", "post-chorus", "bridge", "intro", "outro", "hook", "refrain", "interlude",
    "instrumental", "solo", "break", "breakdown", "coda", "tag", "repeat", "spoken", "drop", "part", "skit",
];

/// Whether `inner` starts with a section's name ("Verse 2", "Chorus: X").
fn names_section(inner: &str) -> bool {
    let first = inner.trim().split([' ', ':']).next().unwrap_or("").to_lowercase();
    SECTION_WORDS.contains(&first.as_str())
}

/// When `line` marks a section ("[Verse 1]", "[Chorus: Mac Powell]",
/// "(Instrumental)", "Chorus:"), its label: "Verse 1", "Chorus · Mac Powell".
pub fn section_label(line: &str) -> Option<String> {
    let l = line.trim();
    let tidy = |inner: &str| {
        let parts: Vec<&str> = inner.split(':').map(str::trim).filter(|s| !s.is_empty()).collect();
        (!parts.is_empty()).then(|| parts.join(" · "))
    };
    let named = names_section;
    if let Some(inner) = l.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        // Square brackets on a line of their own are always a marker on
        // scraped pages; inside a line only known names count (see `split_markers`).
        return if inner.contains(['[', ']']) { None } else { tidy(inner) };
    }
    if let Some(inner) = l.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        return if named(inner) && inner.split_whitespace().count() <= 4 { tidy(inner) } else { None };
    }
    if let Some(inner) = l.strip_suffix(':') {
        return if named(inner) && inner.split_whitespace().count() <= 3 { tidy(inner) } else { None };
    }
    None
}

/// One row of laid-out lyrics.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    /// A section label ("Verse 1"), drawn small above its stanza.
    Section(String),
    /// Space between stanzas.
    Break,
    /// A line of words. `line` is its index in the synced lines (None for
    /// plain lyrics); a synced blank line is a pause, drawn while it's sung.
    Text { text: String, line: Option<usize> },
}

/// Plain lyrics as stanzas: section markers become labels, blank lines breaks.
pub fn rows_plain(lines: &[String]) -> Vec<Row> {
    tidy_rows(lines.iter().map(|l| match section_label(l) {
        Some(s) => Row::Section(s),
        None if l.trim().is_empty() => Row::Break,
        None => Row::Text { text: l.clone(), line: None },
    }))
}

/// Synced lines the same way; each text row remembers its line for timing
/// and seeking. A blank synced line is a break plus a (pause) row, so the
/// gap can show while it's "sung".
pub fn rows_synced(lines: &[Line]) -> Vec<Row> {
    let mut rows = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        match section_label(&l.text) {
            Some(s) => rows.push(Row::Section(s)),
            None if l.text.trim().is_empty() => {
                rows.push(Row::Break);
                rows.push(Row::Text { text: String::new(), line: Some(i) });
            }
            None => rows.push(Row::Text { text: l.text.clone(), line: Some(i) }),
        }
    }
    tidy_rows(rows)
}

/// No break at the start, after another break, or right before a section
/// (a label brings its own space); none at the end.
fn tidy_rows(rows: impl IntoIterator<Item = Row>) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::new();
    for r in rows {
        match r {
            Row::Break if out.is_empty() || matches!(out.last(), Some(Row::Break | Row::Section(_))) => {}
            Row::Section(_) if matches!(out.last(), Some(Row::Break)) => {
                out.pop();
                out.push(r);
            }
            _ => out.push(r),
        }
    }
    while matches!(out.last(), Some(Row::Break | Row::Section(_))) {
        out.pop();
    }
    out
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
        let synced = StructuredLyrics { synced: true, offset: Some(200), line: vec![ln(Some(1000), "one"), ln(Some(100), "zero")], ..Default::default() };
        let got = pick(vec![plain, synced]).unwrap();
        // Sorted, shifted, never negative.
        assert_eq!(got, Lyrics::Synced(vec![Line { start_ms: 0, text: "zero".into() }, Line { start_ms: 800, text: "one".into() }]));
    }

    /// OpenSubsonic, `structuredLyrics.offset`: "The offset to apply to all
    /// lyrics, in milliseconds. Positive means lyrics appear sooner, negative
    /// means later." (opensubsonic.netlify.app/docs/responses/structuredlyrics).
    /// Navidrome passes an `.lrc` file's `[offset:]` through unchanged, and in
    /// LRC too a positive offset shows the words earlier.
    #[test]
    fn a_positive_offset_shows_lines_sooner_and_a_negative_one_later() {
        let at = |offset| {
            let v = StructuredLyrics { synced: true, offset: Some(offset), line: vec![ln(Some(5000), "line")], ..Default::default() };
            match pick(vec![v]) {
                Some(Lyrics::Synced(l)) => l[0].start_ms,
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(at(300), 4700);
        assert_eq!(at(-300), 5300);
        assert_eq!(at(0), 5000);
    }

    #[test]
    fn empty_versions_are_none() {
        let blank = StructuredLyrics { synced: true, line: vec![ln(Some(0), "  "), ln(Some(10), "")], ..Default::default() };
        assert_eq!(pick(vec![blank]), None);
        assert_eq!(pick(Vec::new()), None);
        assert_eq!(from_text("\n  \n"), None);
        assert_eq!(from_text("a\r\nb"), Some(Lyrics::Plain(vec!["a".into(), "b".into()])));
        // Only markers and scraper debris: nothing to show.
        assert_eq!(from_text("4 ContributorsSong Lyrics[Intro]\n[Chorus]\n12Embed"), None);
        // Markers don't count as words in synced lyrics either.
        let only = StructuredLyrics { synced: true, line: vec![ln(Some(0), "[Instrumental]")], ..Default::default() };
        assert_eq!(pick(vec![only]), None);
    }

    #[test]
    fn synced_flag_without_times_counts_as_plain() {
        let odd = StructuredLyrics { synced: true, line: vec![ln(None, "no times at all")], ..Default::default() };
        assert!(matches!(pick(vec![odd]), Some(Lyrics::Plain(_))));
    }

    /// Text as one well-known scraper hands it to Navidrome (from a user's
    /// screen): a counter-and-title header glued to the first marker, markers
    /// glued to lines, "You might also like", a concert ad, "123Embed".
    const SCRAPED: &str = "4 ContributorsJesus, Friend Of Sinners Lyrics[Verse 1]\n\
Jesus friend of sinners\n\
We have strayed  so far away\n\
And a heart divided[Chorus]\n\
Oh, Jesus friend of sinners\n\
Open our eyes to the world\n\
Yeah, yeah, yeah\n\
\n\
\n\
[Verse 2]\n\
Jesus friend of sinners  \n\
The one who's writing in the sandYou might also like[Bridge]\n\
See Casting Crowns LiveGet tickets as low as $45\n\
When we judge the wounded\n\
Only what we're against123Embed";

    #[test]
    fn scraper_debris_is_removed() {
        let Some(Lyrics::Plain(lines)) = from_text(SCRAPED) else { panic!("plain lyrics expected") };
        assert_eq!(
            lines,
            [
                "[Verse 1]",
                "Jesus friend of sinners",
                "We have strayed so far away",
                "And a heart divided",
                "[Chorus]",
                "Oh, Jesus friend of sinners",
                "Open our eyes to the world",
                "Yeah, yeah, yeah",
                "",
                "[Verse 2]",
                "Jesus friend of sinners",
                "The one who's writing in the sand",
                "[Bridge]",
                "When we judge the wounded",
                "Only what we're against",
            ]
        );
        // An ad glued to a lyric keeps the lyric.
        assert_eq!(clean(["Hold on See X LiveGet tickets as low as $30".to_owned()]), ["Hold on"]);
        // "Embed" alone on the last line goes; a word ending in it would not
        // be on the last line of a scraped page, but a lyric "Embed" stays
        // when it isn't last.
        assert_eq!(clean(["a".to_owned(), "Embed".to_owned(), "b".to_owned()]), ["a", "Embed", "b"]);
    }

    #[test]
    fn plain_lyrics_become_labelled_stanzas() {
        let Some(Lyrics::Plain(lines)) = from_text(SCRAPED) else { panic!() };
        let rows = rows_plain(&lines);
        let text = |t: &str| Row::Text { text: t.into(), line: None };
        assert_eq!(rows[0], Row::Section("Verse 1".into()));
        assert_eq!(rows[1], text("Jesus friend of sinners"));
        assert_eq!(rows[4], Row::Section("Chorus".into()));
        // The blank line before [Verse 2] folds into the label's own space.
        assert_eq!(rows[8], Row::Section("Verse 2".into()));
        assert!(!rows.contains(&Row::Break), "{rows:?}");
        assert!(matches!(rows.last(), Some(Row::Text { .. })));
        // Without markers, blank lines are breaks.
        let rows = rows_plain(&["a".into(), "".into(), "".into(), "b".into(), "".into()]);
        assert_eq!(rows, [text("a"), Row::Break, text("b")]);
    }

    #[test]
    fn section_markers_are_recognised_and_ordinary_lines_are_not() {
        assert_eq!(section_label("[Verse 1]").as_deref(), Some("Verse 1"));
        assert_eq!(section_label("[Chorus: Mac Powell]").as_deref(), Some("Chorus · Mac Powell"));
        assert_eq!(section_label("(Instrumental)").as_deref(), Some("Instrumental"));
        assert_eq!(section_label("Chorus:").as_deref(), Some("Chorus"));
        assert_eq!(section_label("Pre-Chorus 2:").as_deref(), Some("Pre-Chorus 2"));
        assert_eq!(section_label("(Oh, oh, oh)"), None);
        assert_eq!(section_label("Verse after verse I sang it:"), None);
        assert_eq!(section_label("[]"), None);
        // A bracketed aside inside a lyric line stays put.
        assert_eq!(split_markers("I said [laughs] no"), ["I said [laughs] no"]);
        assert_eq!(split_markers("[Intro][Verse 1]"), ["[Intro]", "[Verse 1]"]);
    }

    #[test]
    fn synced_rows_keep_their_lines() {
        let lines = vec![
            Line { start_ms: 0, text: "[Verse 1]".into() },
            Line { start_ms: 1000, text: "one".into() },
            Line { start_ms: 2000, text: "".into() },
            Line { start_ms: 3000, text: "two".into() },
        ];
        assert_eq!(
            rows_synced(&lines),
            [
                Row::Section("Verse 1".into()),
                Row::Text { text: "one".into(), line: Some(1) },
                Row::Break,
                Row::Text { text: String::new(), line: Some(2) },
                Row::Text { text: "two".into(), line: Some(3) },
            ]
        );
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
