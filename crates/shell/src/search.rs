//! The ranking behind both apps' quick search: what makes one candidate a
//! better answer to a query than another, over strings the app chose. The
//! app builds a haystack ([`Hay`]) when its library changes, and each
//! keystroke only scores it ([`scored`]); which groups there are, how many
//! of each are shown and which is the top hit stay with the app.
//!
//! Ranking is literal-first, fuzzy-second: every query word that starts a
//! word in the candidate counts most, then words found anywhere, and only
//! then the fuzzy score. So "bob" ranks Bob Dylan and B.o.B well above Boris
//! Bolles or Naughty Boy, which only match as scattered letters and are
//! dropped. Pure fuzzy matches are kept only when nothing in the haystack
//! matches literally, so typos ("knd blue") still find something.
//!
//! Folding is this module's own: Ond's library folds the same way for its
//! sort keys, but those are stored and frozen with its schema, and nothing
//! here may depend on an app's data.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Utf32String};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

/// Case- and accent-insensitive, with the letters Unicode doesn't decompose
/// (ø, ł, ß, æ, ...) spelled out: "Björk" is "bjork", "Øystein" "oystein".
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.trim().nfkd().filter(|c| !is_combining_mark(*c)).flat_map(char::to_lowercase) {
        match c {
            'ø' => out.push('o'),
            'ł' => out.push('l'),
            'ß' => out.push_str("ss"),
            'æ' => out.push_str("ae"),
            'œ' => out.push_str("oe"),
            'đ' | 'ð' => out.push('d'),
            'þ' => out.push_str("th"),
            'ħ' => out.push('h'),
            'ŧ' => out.push('t'),
            'ı' => out.push('i'),
            c => out.push(c),
        }
    }
    out
}

/// Case-, accent- and punctuation-insensitive: "B.o.B" → "bob",
/// "Björk — Vespertine" → "bjork vespertine".
pub fn plain(s: &str) -> String {
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

/// One searchable string: the fuzzy matcher's copy, and a folded,
/// punctuation-free copy for the literal checks.
pub struct Hay {
    fuzzy: Utf32String,
    plain: String,
}

impl Hay {
    pub fn new(s: &str) -> Self {
        Self { fuzzy: Utf32String::from(s), plain: plain(s) }
    }
}

/// The fuzzy matcher's scratch space: one per search, reused by every
/// keystroke.
pub struct Matcher(nucleo_matcher::Matcher);

impl Default for Matcher {
    fn default() -> Self {
        Self(nucleo_matcher::Matcher::new(Config::DEFAULT))
    }
}

/// What was typed, read once for every haystack it's scored against.
pub struct Query {
    plain: String,
    pattern: Pattern,
}

impl Query {
    /// `None` when there's nothing to search for (blank, or only
    /// punctuation).
    pub fn new(typed: &str) -> Option<Self> {
        let plain = plain(typed);
        (!plain.is_empty()).then(|| Self { plain, pattern: Pattern::parse(typed.trim(), CaseMatching::Ignore, Normalization::Smart) })
    }
}

/// How literally a candidate matches; compared before the fuzzy score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Rank {
    /// 3: every query word starts a word; 2: every query word appears;
    /// 1: some do; 0: only fuzzy.
    pub tier: u8,
    /// The whole candidate is the query.
    pub exact: bool,
    /// The candidate starts with the query.
    pub leading: bool,
}

impl Rank {
    /// Whether any of the query was found as typed, not only as scattered
    /// letters: what a top hit has to be.
    pub fn literal(self) -> bool {
        self.tier > 0
    }
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

/// A candidate that matched: how literally, its fuzzy score, the app's
/// tiebreak (higher is better, shorter candidates already preferred), and
/// where it is in the haystack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scored {
    pub rank: Rank,
    pub fuzzy: u32,
    pub tiebreak: i64,
    pub index: usize,
}

/// Every candidate in `hay` that matches `query`, best first: by rank, then
/// fuzzy score, then `tiebreak` (higher wins), then haystack order.
pub fn scored(query: &Query, hay: &[Hay], matcher: &mut Matcher, tiebreak: impl Fn(usize) -> i64) -> Vec<Scored> {
    let words: Vec<&str> = query.plain.split(' ').collect();
    // Cheap literal pass first; the fuzzy matcher only sees what survives.
    let literal: Vec<(Rank, usize)> = hay.iter().enumerate().map(|(i, h)| (rank(&query.plain, &words, &h.plain), i)).filter(|(r, _)| r.literal()).collect();
    let pool: Vec<(Rank, usize)> =
        if literal.is_empty() { (0..hay.len()).map(|i| (Rank { tier: 0, exact: false, leading: false }, i)).collect() } else { literal };
    let mut out: Vec<Scored> = pool
        .into_iter()
        .filter_map(|(rank, i)| {
            let score = query.pattern.score(hay[i].fuzzy.slice(..), &mut matcher.0);
            // Literal matches keep a place even if the fuzzy matcher disagrees.
            match (rank.tier, score) {
                (0, None) => None,
                (_, s) => Some(Scored { rank, fuzzy: s.unwrap_or(0), tiebreak: tiebreak(i) * 1000 - hay[i].plain.len() as i64, index: i }),
            }
        })
        .collect();
    out.sort_by(|a, b| b.rank.cmp(&a.rank).then(b.fuzzy.cmp(&a.fuzzy)).then(b.tiebreak.cmp(&a.tiebreak)).then(a.index.cmp(&b.index)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn best(query: &str, names: &[&str]) -> Vec<String> {
        let hay: Vec<Hay> = names.iter().map(|n| Hay::new(n)).collect();
        let q = Query::new(query).unwrap();
        scored(&q, &hay, &mut Matcher::default(), |_| 0).into_iter().map(|s| names[s.index].to_owned()).collect()
    }

    #[test]
    fn plain_strips_case_accents_and_punctuation() {
        assert_eq!(plain("B.o.B"), "bob");
        assert_eq!(plain("  Björk — Vespertine "), "bjork vespertine");
        assert_eq!(plain("AC/DC"), "acdc");
        assert_eq!(plain("Øystein Sevåg"), "oystein sevag");
    }

    #[test]
    fn a_blank_or_punctuation_only_query_is_no_query() {
        assert!(Query::new("  ").is_none());
        assert!(Query::new("...").is_none());
        assert!(Query::new("b").is_some());
    }

    #[test]
    fn word_starts_beat_letters_found_inside_words() {
        assert_eq!(best("dun", &["Redundant", "Dune"]), ["Dune", "Redundant"]);
    }

    #[test]
    fn scattered_letters_count_only_when_nothing_matches_as_typed() {
        assert_eq!(best("bob", &["Boris Bolles", "Bob Dylan"]), ["Bob Dylan"]);
        assert_eq!(best("fngrlngs", &["Fingerlings", "Noble Beast"]), ["Fingerlings"]);
    }

    #[test]
    fn the_tiebreak_settles_equal_matches() {
        let names = ["Le Guin", "Le Guin"];
        let hay: Vec<Hay> = names.iter().map(|n| Hay::new(n)).collect();
        let got = scored(&Query::new("guin").unwrap(), &hay, &mut Matcher::default(), |i| i as i64);
        assert_eq!(got.iter().map(|s| s.index).collect::<Vec<_>>(), [1, 0]);
    }
}
