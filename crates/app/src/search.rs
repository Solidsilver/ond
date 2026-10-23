//! Quick search over the local library mirror. The haystack is built once per
//! album list (after load and after each sync); each keystroke only scores it.

use library::Album;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32String};
use std::collections::BTreeMap;

/// Results shown per group.
pub const PER_GROUP: usize = 8;

pub struct ArtistEntry {
    pub name: String,
    pub albums: usize,
}

pub struct AlbumEntry {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub year: Option<i32>,
}

/// Searchable copy of the library. Artists come from album artists, since only
/// albums are synced up front.
pub struct SearchIndex {
    pub artists: Vec<ArtistEntry>,
    pub albums: Vec<AlbumEntry>,
    artist_hay: Vec<Utf32String>,
    album_hay: Vec<Utf32String>,
    matcher: Matcher,
}

/// Indices into [`SearchIndex::artists`] / [`SearchIndex::albums`], best first.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Results {
    pub artists: Vec<usize>,
    pub albums: Vec<usize>,
}

impl Results {
    pub fn len(&self) -> usize {
        self.artists.len() + self.albums.len()
    }

    /// The `n`th row in display order (artists, then albums).
    pub fn hit(&self, n: usize) -> Option<Hit> {
        match self.artists.get(n) {
            Some(&i) => Some(Hit::Artist(i)),
            None => self.albums.get(n - self.artists.len()).map(|&i| Hit::Album(i)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    Artist(usize),
    Album(usize),
}

impl SearchIndex {
    pub fn new(albums: &[Album]) -> Self {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for a in albums {
            if let Some(artist) = a.artist.as_deref().filter(|s| !s.is_empty()) {
                *counts.entry(artist).or_default() += 1;
            }
        }
        let artists: Vec<_> = counts.into_iter().map(|(name, albums)| ArtistEntry { name: name.to_owned(), albums }).collect();
        let albums: Vec<_> = albums
            .iter()
            .map(|a| AlbumEntry {
                id: a.id.clone(),
                title: a.name.clone(),
                artist: a.artist.clone().unwrap_or_default(),
                year: a.year,
            })
            .collect();
        let artist_hay = artists.iter().map(|a| Utf32String::from(a.name.as_str())).collect();
        // Artist first so "evans sunday" and "sunday evans" both land.
        let album_hay = albums.iter().map(|a| Utf32String::from(format!("{} — {}", a.artist, a.title))).collect();
        Self { artists, albums, artist_hay, album_hay, matcher: Matcher::new(Config::DEFAULT) }
    }

    pub fn search(&mut self, query: &str) -> Results {
        let query = query.trim();
        if query.is_empty() {
            return Results::default();
        }
        let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
        Results {
            artists: top(&pattern, &self.artist_hay, &mut self.matcher),
            albums: top(&pattern, &self.album_hay, &mut self.matcher),
        }
    }
}

fn top(pattern: &Pattern, hay: &[Utf32String], matcher: &mut Matcher) -> Vec<usize> {
    let mut scored: Vec<(u32, usize)> =
        hay.iter().enumerate().filter_map(|(i, h)| pattern.score(h.slice(..), matcher).map(|s| (s, i))).collect();
    // Best score first; ties keep library (sorted) order.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(PER_GROUP).map(|(_, i)| i).collect()
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
        }
    }

    fn index() -> SearchIndex {
        SearchIndex::new(&[
            album("1", "Andrew Bird", "Fingerlings"),
            album("2", "Andrew Bird", "Noble Beast"),
            album("3", "Bill Evans Trio", "Sunday at the Village Vanguard"),
            album("4", "Miles Davis", "Kind of Blue"),
            album("5", "Björk", "Vespertine"),
        ])
    }

    #[test]
    fn artists_are_deduplicated_with_counts() {
        let idx = index();
        let bird = idx.artists.iter().find(|a| a.name == "Andrew Bird").unwrap();
        assert_eq!(bird.albums, 2);
        assert_eq!(idx.artists.len(), 4);
    }

    #[test]
    fn groups_artists_and_albums() {
        let mut idx = index();
        let r = idx.search("bird");
        assert_eq!(r.artists.iter().map(|&i| idx.artists[i].name.as_str()).collect::<Vec<_>>(), ["Andrew Bird"]);
        // Fuzzy matching also finds weaker hits (b-i-r-d is scattered through
        // "Bill Evans Trio — Sunday at…"); the real ones rank first.
        assert!(r.albums[..2].iter().all(|&i| idx.albums[i].artist == "Andrew Bird"));
        assert_eq!(r.hit(0), Some(Hit::Artist(r.artists[0])));
        assert_eq!(r.hit(1), Some(Hit::Album(r.albums[0])));
        assert_eq!(r.hit(r.len()), None);
    }

    #[test]
    fn matches_across_artist_and_title() {
        let mut idx = index();
        let r = idx.search("evans vanguard");
        assert!(r.artists.is_empty());
        assert_eq!(idx.albums[r.albums[0]].id, "3");
    }

    #[test]
    fn fuzzy_and_accent_insensitive() {
        let mut idx = index();
        let r = idx.search("knd blue");
        assert_eq!(idx.albums[r.albums[0]].id, "4");
        let r = idx.search("bjork");
        assert_eq!(idx.albums[r.albums[0]].id, "5");
    }

    #[test]
    fn empty_query_and_cap() {
        let mut idx = index();
        assert_eq!(idx.search("  "), Results::default());
        let many: Vec<_> = (0..30).map(|i| album(&i.to_string(), "Same Artist", &format!("Record {i}"))).collect();
        let mut big = SearchIndex::new(&many);
        assert_eq!(big.search("record").albums.len(), PER_GROUP);
    }
}
