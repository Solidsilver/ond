//! Quick search over the local library mirror. The haystack is built when the
//! library changes (load, sync); each keystroke only scores it.
//!
//! Ranking is literal-first, fuzzy-second: every query word that starts a word
//! in the candidate counts most, then words found anywhere, and only then the
//! fuzzy score. So "bob" ranks Bob Dylan and B.o.B well above Boris Bolles or
//! Naughty Boy, which only match as scattered letters and are dropped. Pure
//! fuzzy matches are kept only when nothing in the group matches literally, so
//! typos ("knd blue") still find something.

use library::{Album, Artist, Track, fold};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32String};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Results shown per group.
pub const MAX_ARTISTS: usize = 4;
pub const MAX_ALBUMS: usize = 5;
pub const MAX_TRACKS: usize = 5;

pub struct ArtistEntry {
    pub id: Option<String>,
    pub name: String,
    /// What the list is sorted by: the server's sort name, else the name.
    pub sort: String,
    pub albums: usize,
}

pub struct AlbumEntry {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub year: Option<i32>,
}

pub struct TrackEntry {
    pub id: String,
    pub album_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
}

/// One searchable string: the fuzzy matcher's copy, and a folded,
/// punctuation-free copy for the literal checks.
struct Hay {
    fuzzy: Utf32String,
    plain: String,
}

impl Hay {
    fn new(s: &str) -> Self {
        Self { fuzzy: Utf32String::from(s), plain: plain(s) }
    }
}

/// Case-, accent- and punctuation-insensitive: "B.o.B" → "bob",
/// "Björk — Vespertine" → "bjork vespertine".
fn plain(s: &str) -> String {
    let folded = fold(s);
    let mut out = String::with_capacity(folded.len());
    for c in folded.chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if c.is_whitespace() && !out.ends_with(' ') && !out.is_empty() {
            out.push(' ');
        }
    }
    out.truncate(out.trim_end().len());
    out
}

pub struct SearchIndex {
    /// Shared, so the Artists view lists them without a copy per frame.
    pub artists: Arc<Vec<ArtistEntry>>,
    pub albums: Vec<AlbumEntry>,
    pub tracks: Vec<TrackEntry>,
    artist_hay: Vec<Hay>,
    album_hay: Vec<Hay>,
    track_hay: Vec<Hay>,
    matcher: Matcher,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Artist(usize),
    Album(usize),
    Track(usize),
}

/// Indices into the index's lists, best first. `top` is the best hit overall
/// and isn't repeated in its group.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Results {
    pub top: Option<Hit>,
    pub artists: Vec<usize>,
    pub albums: Vec<usize>,
    pub tracks: Vec<usize>,
}

impl Results {
    pub fn len(&self) -> usize {
        usize::from(self.top.is_some()) + self.artists.len() + self.albums.len() + self.tracks.len()
    }

    /// The `n`th row in display order: top hit, artists, albums, tracks.
    pub fn hit(&self, n: usize) -> Option<Hit> {
        let mut n = n;
        if let Some(top) = self.top {
            if n == 0 {
                return Some(top);
            }
            n -= 1;
        }
        if let Some(&i) = self.artists.get(n) {
            return Some(Hit::Artist(i));
        }
        n -= self.artists.len();
        if let Some(&i) = self.albums.get(n) {
            return Some(Hit::Album(i));
        }
        n -= self.albums.len();
        self.tracks.get(n).map(|&i| Hit::Track(i))
    }
}

/// How literally a candidate matches; compared before the fuzzy score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Rank {
    /// 3: every query word starts a word; 2: every query word appears;
    /// 1: some do; 0: only fuzzy.
    tier: u8,
    /// The whole candidate is the query.
    exact: bool,
    /// The candidate starts with the query.
    leading: bool,
}

fn rank(query: &str, words: &[&str], hay: &str) -> Rank {
    let prefixes = words.iter().filter(|w| hay.split(' ').any(|h| h.starts_with(**w))).count();
    let found = words.iter().filter(|w| hay.contains(**w)).count();
    let tier = if prefixes == words.len() {
        3
    } else if found == words.len() {
        2
    } else if found > 0 {
        1
    } else {
        0
    };
    Rank { tier, exact: hay == query, leading: hay.starts_with(query) }
}

/// Scored candidate: rank, fuzzy score, then a group-specific tiebreak
/// (higher is better), then library order.
type Scored = (Rank, u32, i64, usize);

impl SearchIndex {
    /// `artists` are the server's artist entities; when there are none yet
    /// (older server, not synced), artists are derived from album artists.
    pub fn new(albums: &[Album], artists: &[Artist], tracks: &[Track]) -> Self {
        let artists: Vec<ArtistEntry> = if artists.is_empty() {
            let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
            for a in albums {
                if let Some(artist) = a.artist.as_deref().filter(|s| !s.is_empty()) {
                    *counts.entry(artist).or_default() += 1;
                }
            }
            counts.into_iter().map(|(name, albums)| ArtistEntry { id: None, name: name.to_owned(), sort: name.to_owned(), albums }).collect()
        } else {
            artists.iter().map(|a| ArtistEntry { id: Some(a.id.clone()), name: a.name.clone(), sort: a.sort.clone().unwrap_or_else(|| a.name.clone()), albums: a.album_count as usize }).collect()
        };
        let album_names: std::collections::HashMap<&str, &str> = albums.iter().map(|a| (a.id.as_str(), a.name.as_str())).collect();
        let albums: Vec<AlbumEntry> = albums
            .iter()
            .map(|a| AlbumEntry { id: a.id.clone(), title: a.name.clone(), artist: a.artist.clone().unwrap_or_default(), year: a.year })
            .collect();
        let tracks: Vec<TrackEntry> = tracks
            .iter()
            .map(|t| TrackEntry {
                id: t.id.clone(),
                album_id: t.album_id.clone(),
                title: t.title.clone(),
                artist: t.artist.clone().unwrap_or_default(),
                album: album_names.get(t.album_id.as_str()).map(|s| (*s).to_owned()).unwrap_or_default(),
            })
            .collect();
        let artist_hay = artists.iter().map(|a| Hay::new(&a.name)).collect();
        // Artist first so "evans sunday" and "sunday evans" both land.
        let album_hay = albums.iter().map(|a| Hay::new(&format!("{} — {}", a.artist, a.title))).collect();
        // Title first: track searches are mostly by title.
        let track_hay = tracks.iter().map(|t| Hay::new(&format!("{} — {} — {}", t.title, t.artist, t.album))).collect();
        Self { artists: Arc::new(artists), albums, tracks, artist_hay, album_hay, track_hay, matcher: Matcher::new(Config::DEFAULT) }
    }

    pub fn search(&mut self, query: &str) -> Results {
        let q = plain(query);
        if q.is_empty() {
            return Results::default();
        }
        let words: Vec<&str> = q.split(' ').collect();
        let pattern = Pattern::parse(query.trim(), CaseMatching::Ignore, Normalization::Smart);
        // Artists with more albums, then shorter (more specific) names win ties.
        let artists = scored(&q, &words, &pattern, &self.artist_hay, &mut self.matcher, |i| self.artists[i].albums as i64);
        let albums = scored(&q, &words, &pattern, &self.album_hay, &mut self.matcher, |_| 0);
        let tracks = scored(&q, &words, &pattern, &self.track_hay, &mut self.matcher, |_| 0);

        // The top hit is the best literal match; on equal rank artists come
        // before albums before tracks. Pure fuzzy matches never make it.
        let best = |v: &[Scored]| v.first().map(|s| s.0).filter(|r| r.tier > 0);
        let candidates = [(best(&artists), 0), (best(&albums), 1), (best(&tracks), 2)];
        let top_group = candidates.iter().filter_map(|(r, g)| r.map(|r| (r, *g))).max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1))).map(|(_, g)| g);
        let take = |v: Vec<Scored>, skip_first: bool, n: usize| v.into_iter().skip(usize::from(skip_first)).take(n).map(|s| s.3).collect::<Vec<_>>();
        let top = match top_group {
            Some(0) => Some(Hit::Artist(artists[0].3)),
            Some(1) => Some(Hit::Album(albums[0].3)),
            Some(2) => Some(Hit::Track(tracks[0].3)),
            _ => None,
        };
        Results {
            top,
            artists: take(artists, top_group == Some(0), MAX_ARTISTS),
            albums: take(albums, top_group == Some(1), MAX_ALBUMS),
            tracks: take(tracks, top_group == Some(2), MAX_TRACKS),
        }
    }
}

fn scored(query: &str, words: &[&str], pattern: &Pattern, hay: &[Hay], matcher: &mut Matcher, tiebreak: impl Fn(usize) -> i64) -> Vec<Scored> {
    // Cheap literal pass first; the fuzzy matcher only sees what survives.
    let literal: Vec<(Rank, usize)> = hay.iter().enumerate().map(|(i, h)| (rank(query, words, &h.plain), i)).filter(|(r, _)| r.tier > 0).collect();
    let pool: Vec<(Rank, usize)> = if literal.is_empty() {
        (0..hay.len()).map(|i| (Rank { tier: 0, exact: false, leading: false }, i)).collect()
    } else {
        literal
    };
    let mut out: Vec<Scored> = pool
        .into_iter()
        .filter_map(|(r, i)| {
            let score = pattern.score(hay[i].fuzzy.slice(..), matcher);
            // Literal matches keep a place even if the fuzzy matcher disagrees.
            match (r.tier, score) {
                (0, None) => None,
                (_, s) => Some((r, s.unwrap_or(0), tiebreak(i) * 1000 - hay[i].plain.len() as i64, i)),
            }
        })
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(b.2.cmp(&a.2)).then(a.3.cmp(&b.3)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(id: &str, artist: &str, name: &str) -> Album {
        Album {
            id: id.into(),
            name: name.into(),
            sort: None,
            artist: Some(artist.into()),
            artist_ref: None,
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

    fn artist(id: &str, name: &str, albums: u32) -> Artist {
        Artist { id: id.into(), name: name.into(), sort: None, album_count: albums, cover_art: None }
    }

    fn track(id: &str, album_id: &str, title: &str, artist: &str) -> Track {
        Track {
            id: id.into(),
            album_id: album_id.into(),
            title: title.into(),
            artist: Some(artist.into()),
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
        }
    }

    fn index() -> SearchIndex {
        SearchIndex::new(
            &[
                album("1", "Andrew Bird", "Fingerlings"),
                album("2", "Andrew Bird", "Noble Beast"),
                album("3", "Bill Evans Trio", "Sunday at the Village Vanguard"),
                album("4", "Miles Davis", "Kind of Blue"),
                album("5", "Björk", "Vespertine"),
            ],
            &[],
            &[track("t1", "4", "So What", "Miles Davis"), track("t2", "3", "Gloria's Step", "Bill Evans Trio")],
        )
    }

    fn names(idx: &SearchIndex, r: &Results) -> Vec<String> {
        (0..r.len())
            .map(|n| match r.hit(n).unwrap() {
                Hit::Artist(i) => idx.artists[i].name.clone(),
                Hit::Album(i) => idx.albums[i].title.clone(),
                Hit::Track(i) => idx.tracks[i].title.clone(),
            })
            .collect()
    }

    #[test]
    fn artists_derived_from_albums_until_real_ones_arrive() {
        let idx = index();
        let bird = idx.artists.iter().find(|a| a.name == "Andrew Bird").unwrap();
        assert_eq!(bird.albums, 2);
        assert_eq!(idx.artists.len(), 4);
        let real = SearchIndex::new(&[], &[artist("ar-1", "Johannes Motschmann", 3), artist("ar-2", "David Panzl", 1)], &[]);
        assert_eq!(real.artists.len(), 2);
        assert_eq!(real.artists[0].id.as_deref(), Some("ar-1"));
    }

    #[test]
    fn bob_prefers_word_starts_and_drops_scattered_letters() {
        let artists = [
            artist("1", "Johannes Motschmann, David Panzl, Boris Bolles", 1),
            artist("2", "Naughty Boy feat. Beyoncé & Arrow Benjamin", 1),
            artist("3", "Boney M. by Doug Laurent", 1),
            artist("4", "Bob Seger & the Silver Bullet Band", 1),
            artist("5", "Bob Marley & The Wailers", 1),
            artist("6", "Bob Dylan", 2),
            artist("7", "B.o.B", 2),
        ];
        let albums = [album("a1", "B.o.B", "B.o.B Presents: The Adventures of Bobby Ray"), album("a2", "Bob Dylan", "Blonde on Blonde")];
        let mut idx = SearchIndex::new(&albums, &artists, &[]);
        let r = idx.search("bob");
        let got = names(&idx, &r);
        // B.o.B is the query exactly once punctuation goes; then the Bobs, most albums first.
        assert_eq!(got[..4], ["B.o.B", "Bob Dylan", "Bob Marley & The Wailers", "Bob Seger & the Silver Bullet Band"]);
        for weak in ["Boris Bolles", "Naughty Boy", "Boney M."] {
            assert!(!got.iter().any(|g| g.contains(weak)), "{weak} should not match \"bob\": {got:?}");
        }
        // Albums still get their group even with plenty of artist hits.
        assert_eq!(r.albums.len(), 2);
    }

    #[test]
    fn groups_are_capped_and_balanced() {
        let artists: Vec<Artist> = (0..20).map(|i| artist(&i.to_string(), &format!("Record Artist {i}"), 1)).collect();
        let albums: Vec<Album> = (0..20).map(|i| album(&i.to_string(), "Someone", &format!("Record {i}"))).collect();
        let tracks: Vec<Track> = (0..20).map(|i| track(&i.to_string(), "0", &format!("Record Track {i}"), "Someone")).collect();
        let mut idx = SearchIndex::new(&albums, &artists, &tracks);
        let r = idx.search("record");
        assert!(r.top.is_some());
        assert!(r.artists.len() <= MAX_ARTISTS && r.albums.len() <= MAX_ALBUMS && r.tracks.len() <= MAX_TRACKS);
        // The top hit is the best artist; four more artists follow it.
        assert_eq!(r.len(), 1 + 4 + 5 + 5);
        // Display order: top hit, then each group.
        assert_eq!(r.hit(0), r.top);
        assert!(matches!(r.hit(1), Some(Hit::Artist(_))));
        assert!(matches!(r.hit(r.len() - 1), Some(Hit::Track(_))));
        assert_eq!(r.hit(r.len()), None);
    }

    #[test]
    fn tracks_are_searchable_and_can_be_the_top_hit() {
        let mut idx = index();
        let r = idx.search("so what");
        assert_eq!(r.top, Some(Hit::Track(0)));
        let r = idx.search("gloria");
        assert_eq!(names(&idx, &r)[0], "Gloria's Step");
    }

    #[test]
    fn matches_across_artist_and_title() {
        let mut idx = index();
        let r = idx.search("evans vanguard");
        assert_eq!(r.top, Some(Hit::Album(2)));
        // The artist matches one of the words: shown, but below the album.
        assert_eq!(r.artists.iter().map(|&i| idx.artists[i].name.as_str()).collect::<Vec<_>>(), ["Bill Evans Trio"]);
    }

    #[test]
    fn fuzzy_typos_and_accents_still_match() {
        let mut idx = index();
        let r = idx.search("knd blue");
        assert_eq!(names(&idx, &r)[0], "Kind of Blue");
        let r = idx.search("bjork");
        assert!(names(&idx, &r).contains(&"Vespertine".to_owned()));
        // Nothing literal at all: fall back to fuzzy, but no top hit.
        let r = idx.search("fngrlngs");
        assert_eq!(r.top, None);
        assert_eq!(idx.albums[r.albums[0]].title, "Fingerlings");
    }

    #[test]
    fn empty_query() {
        let mut idx = index();
        assert_eq!(idx.search("  "), Results::default());
        assert_eq!(idx.search("..."), Results::default());
    }

    #[test]
    fn plain_strips_case_accents_and_punctuation() {
        assert_eq!(plain("B.o.B"), "bob");
        assert_eq!(plain("  Björk — Vespertine "), "bjork vespertine");
        assert_eq!(plain("AC/DC"), "acdc");
    }

    /// Keystroke latency on a library the size of the user's (1866 albums, ~20k
    /// tracks). Generous bound for debug builds; `cargo test --release -- --nocapture`
    /// prints the real numbers.
    #[test]
    fn keystrokes_stay_fast_on_a_full_library() {
        let words = ["blue", "night", "river", "song", "love", "light", "dance", "home", "road", "summer", "rain", "fire"];
        let albums: Vec<Album> =
            (0..1866).map(|i| album(&format!("al{i}"), &format!("Artist {} {}", words[i % 12], i % 700), &format!("{} {} {i}", words[(i * 7) % 12], words[(i * 5) % 12]))).collect();
        let artists: Vec<Artist> = (0..700).map(|i| artist(&format!("ar{i}"), &format!("Artist {} {i}", words[i % 12]), 3)).collect();
        let tracks: Vec<Track> = (0..20_000)
            .map(|i| track(&format!("t{i}"), &format!("al{}", i % 1866), &format!("{} of the {} {i}", words[(i * 3) % 12], words[(i * 11) % 12]), "Someone"))
            .collect();
        let built = std::time::Instant::now();
        let mut idx = SearchIndex::new(&albums, &artists, &tracks);
        let build = built.elapsed();
        // Each keystroke's median of 5 runs, so one scheduler hiccup (other
        // tests run in parallel) can't fail it; the worst median must stay
        // well inside a frame budget in release, generous in debug.
        let mut worst = std::time::Duration::ZERO;
        for q in ["b", "bl", "blu", "blue", "blue n", "blue ni", "blue night", "zzzz", "artist 4", "summer rain 12"] {
            let mut runs: Vec<std::time::Duration> = (0..5)
                .map(|_| {
                    let t = std::time::Instant::now();
                    let _ = idx.search(q);
                    t.elapsed()
                })
                .collect();
            runs.sort();
            worst = worst.max(runs[2]);
        }
        println!("search index: build {build:?}, worst keystroke (median of 5) {worst:?}");
        let budget = std::time::Duration::from_millis(if cfg!(debug_assertions) { 1000 } else { 40 });
        assert!(worst < budget, "worst keystroke {worst:?} (budget {budget:?})");
    }
}
