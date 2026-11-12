//! Quick search over the books in memory: no server call, and nothing stored.
//! The haystack is built once per library version (see [`Books::index`]);
//! each keystroke only scores it, with the ranking Ond's palette uses
//! (`shell::search`: literal first, fuzzy second).
//!
//! A query finds books, and the authors, narrators and series that match
//! as names of their own, each listed once however many books carry it. The
//! top hit is the best literal match of all of them. On equal footing a name
//! comes first ("le guin" is her, not one of her books), unless a book's
//! title is the query itself ("dune" is the book before the series).
//!
//! Weaker matches go when there are stronger ones anywhere: if something has
//! every word of the query, what has only some is left out ("le guin"
//! doesn't list Don Leslie for his "le"), and scattered letters count only
//! when nothing has any word as typed.
//!
//! An empty query lists the books in progress, the most recently listened
//! first, so ⌘K ⏎ is the way back to a book.
//!
//! [`Books::index`]: super::Books::index

use super::progress::Positions;
use shell::search::{Hay, Matcher, Query, Rank, Scored, plain, scored};
use sources::ItemSummary;
use std::collections::BTreeMap;

/// Results shown per group.
pub const MAX_BOOKS: usize = 6;
pub const MAX_NAMES: usize = 3;
/// Books in progress listed for an empty query.
pub const MAX_IN_PROGRESS: usize = 8;

/// What a name is in a book's listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Author,
    Narrator,
    Series,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Author, Kind::Narrator, Kind::Series];

    /// The group's title in the palette.
    pub fn group(self) -> &'static str {
        match self {
            Kind::Author => "Authors",
            Kind::Narrator => "Narrators",
            Kind::Series => "Series",
        }
    }

    /// What one of them is, as a row says it.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Author => "Author",
            Kind::Narrator => "Narrator",
            Kind::Series => "Series",
        }
    }
}

/// An author, a narrator or a series, and how many books carry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Name {
    pub name: String,
    pub books: usize,
}

/// The library as search sees it: a haystack per book and per name. Book
/// indices are the listing's (`Books::all`) for the version it was built
/// from.
pub struct BookIndex {
    book_hay: Vec<Hay>,
    /// Each title, folded as the query is, to tell an exact title.
    titles: Vec<String>,
    /// Authors, narrators and series, in [`Kind::ALL`]'s order, each sorted
    /// by name.
    names: [Vec<Name>; 3],
    name_hay: [Vec<Hay>; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Book(usize),
    Name(Kind, usize),
}

/// What a query found, as indices into the listing and the index's names.
/// `top` isn't repeated in its group. For an empty query only `in_progress`
/// has anything.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Results {
    pub in_progress: Vec<usize>,
    pub top: Option<Hit>,
    pub books: Vec<usize>,
    /// In [`Kind::ALL`]'s order.
    pub names: [Vec<usize>; 3],
}

impl Results {
    /// Each group's title and size, in the order drawn.
    pub fn groups(&self) -> [(&'static str, usize); 6] {
        [
            ("Continue listening", self.in_progress.len()),
            ("Top hit", usize::from(self.top.is_some())),
            ("Books", self.books.len()),
            (Kind::Author.group(), self.names[0].len()),
            (Kind::Narrator.group(), self.names[1].len()),
            (Kind::Series.group(), self.names[2].len()),
        ]
    }

    pub fn len(&self) -> usize {
        self.groups().iter().map(|(_, n)| n).sum()
    }

    /// The `n`th row in the order drawn.
    pub fn hit(&self, n: usize) -> Option<Hit> {
        let mut n = n;
        if let Some(&i) = self.in_progress.get(n) {
            return Some(Hit::Book(i));
        }
        n -= self.in_progress.len();
        if let Some(top) = self.top {
            if n == 0 {
                return Some(top);
            }
            n -= 1;
        }
        if let Some(&i) = self.books.get(n) {
            return Some(Hit::Book(i));
        }
        n -= self.books.len();
        for (kind, names) in Kind::ALL.into_iter().zip(&self.names) {
            if let Some(&i) = names.get(n) {
                return Some(Hit::Name(kind, i));
            }
            n -= names.len();
        }
        None
    }
}

impl BookIndex {
    pub fn new(books: &[ItemSummary]) -> Self {
        // Title first (books are mostly found by it), then who wrote and read
        // it and its series, so a name chosen from the palette finds its
        // books.
        let book_hay = books
            .iter()
            .map(|b| {
                let mut parts = vec![b.title.as_str()];
                parts.extend(b.subtitle.as_deref());
                parts.extend(b.authors.iter().map(String::as_str));
                parts.extend(b.narrators.iter().map(String::as_str));
                parts.extend(b.series.iter().map(|s| s.name.as_str()));
                Hay::new(&parts.join(" — "))
            })
            .collect();
        let titles = books.iter().map(|b| plain(&b.title)).collect();
        let names = Kind::ALL.map(|kind| {
            let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
            for b in books {
                let mut seen: Vec<&str> = match kind {
                    Kind::Author => b.authors.iter().map(String::as_str).collect(),
                    Kind::Narrator => b.narrators.iter().map(String::as_str).collect(),
                    Kind::Series => b.series.iter().map(|s| s.name.as_str()).collect(),
                };
                seen.retain(|n| !n.trim().is_empty());
                seen.sort_unstable();
                seen.dedup();
                for name in seen {
                    *counts.entry(name).or_default() += 1;
                }
            }
            counts.into_iter().map(|(name, books)| Name { name: name.to_owned(), books }).collect::<Vec<_>>()
        });
        let name_hay = std::array::from_fn(|k| names[k].iter().map(|n: &Name| Hay::new(&n.name)).collect());
        Self { book_hay, titles, names, name_hay }
    }

    pub fn name(&self, kind: Kind, i: usize) -> Option<&Name> {
        self.names[kind as usize].get(i)
    }

    /// What `query` finds. An empty one lists the books in progress from
    /// `positions` (keyed by book id), the most recently listened first.
    pub fn search(&self, query: &str, matcher: &mut Matcher, books: &[ItemSummary], positions: &Positions) -> Results {
        let Some(q) = Query::new(query) else { return Results { in_progress: in_progress(books, positions), ..Results::default() } };
        let mut found = scored(&q, &self.book_hay, matcher, |_| 0);
        // People and series with more books win ties.
        let mut names: [Vec<Scored>; 3] = std::array::from_fn(|k| scored(&q, &self.name_hay[k], matcher, |i| self.names[k][i].books as i64));
        let strongest = std::iter::once(&found).chain(&names).filter_map(|v| v.first()).map(|s| s.rank.tier).max().unwrap_or(0);
        let floor = strongest.min(2);
        found.retain(|s| s.rank.tier >= floor);
        for v in &mut names {
            v.retain(|s| s.rank.tier >= floor);
        }
        // A book's haystack is more than its title, so it's never the query
        // exactly; its title can be.
        let typed = plain(query);
        let title_rank = |s: &Scored| {
            let title = &self.titles[s.index];
            Rank { exact: s.rank.exact || *title == typed, leading: s.rank.leading || title.starts_with(&typed), ..s.rank }
        };
        // The best literal match. On equal rank a book titled as typed, then
        // authors, narrators and series, then other books. Pure fuzzy
        // matches never make it.
        let best = |v: &[Scored]| v.first().map(|s| s.rank).filter(|r| r.literal());
        let book = found.first().map(|s| (title_rank(s), self.titles[s.index] == typed));
        let candidates = [
            book.filter(|(r, _)| r.literal()).map(|(r, titled)| (r, if titled { 4 } else { 0 })),
            best(&names[0]).map(|r| (r, 3)),
            best(&names[1]).map(|r| (r, 2)),
            best(&names[2]).map(|r| (r, 1)),
        ];
        let top_group = candidates.iter().enumerate().filter_map(|(g, c)| c.map(|c| (c, g))).max_by_key(|(c, _)| *c).map(|(_, g)| g);
        let take = |v: &[Scored], skip_first: bool, n: usize| v.iter().skip(usize::from(skip_first)).take(n).map(|s| s.index).collect::<Vec<_>>();
        let top = match top_group {
            Some(0) => Some(Hit::Book(found[0].index)),
            Some(g) => Some(Hit::Name(Kind::ALL[g - 1], names[g - 1][0].index)),
            None => None,
        };
        Results {
            in_progress: Vec::new(),
            top,
            books: take(&found, top_group == Some(0), MAX_BOOKS),
            names: std::array::from_fn(|k| take(&names[k], top_group == Some(k + 1), MAX_NAMES)),
        }
    }
}

/// The books in progress as an empty query lists them: Home's order, cut to
/// what the palette shows.
fn in_progress(books: &[ItemSummary], positions: &Positions) -> Vec<usize> {
    let mut ids = super::progress::in_progress(books, positions);
    ids.truncate(MAX_IN_PROGRESS);
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use sources::{ItemKey, Position, Progress, Series};
    use std::time::{Duration, SystemTime};

    fn book(id: &str, title: &str, author: &str, narrator: &str, series: Option<&str>) -> ItemSummary {
        ItemSummary {
            id: id.into(),
            title: title.into(),
            authors: vec![author.into()],
            narrators: vec![narrator.into()],
            series: series.map(|s| vec![Series { name: s.into(), sequence: None }]).unwrap_or_default(),
            ..ItemSummary::default()
        }
    }

    fn shelf() -> Vec<ItemSummary> {
        vec![
            book("lh", "The Left Hand of Darkness", "Ursula K. Le Guin", "George Guidall", Some("Hainish")),
            book("du", "Dune", "Frank Herbert", "Scott Brick", Some("Dune")),
            book("di", "The Dispossessed", "Ursula K. Le Guin", "Don Leslie", Some("Hainish")),
            book("we", "A Wizard of Earthsea", "Ursula K. Le Guin", "Rob Inglis", Some("Earthsea")),
            book("ki", "Kindred", "Octavia E. Butler", "Kim Staunton", None),
            book("pi", "Piranesi", "Susanna Clarke", "Chiwetel Ejiofor", None),
        ]
    }

    fn search(query: &str, positions: &Positions) -> (Results, Vec<String>) {
        let books = shelf();
        let index = BookIndex::new(&books);
        let r = index.search(query, &mut Matcher::default(), &books, positions);
        let rows = (0..r.len())
            .map(|n| match r.hit(n).unwrap() {
                Hit::Book(i) => books[i].title.clone(),
                Hit::Name(kind, i) => format!("{}: {}", kind.label(), index.name(kind, i).unwrap().name),
            })
            .collect();
        (r, rows)
    }

    fn progress(id: &str, secs: f64, finished: bool, ago: u64) -> (String, Progress) {
        let at = Some(SystemTime::now() - Duration::from_secs(ago));
        (id.into(), Progress { item: ItemKey::new(sources::AccountId::from_uuid(uuid::Uuid::nil()), id), position: Position::from_secs(secs), finished, at })
    }

    #[test]
    fn a_title_finds_its_book_as_the_top_hit() {
        let (r, rows) = search("piranesi", &Positions::default());
        assert_eq!(r.top, Some(Hit::Book(5)));
        assert_eq!(rows, ["Piranesi"]);
    }

    #[test]
    fn an_author_is_listed_once_with_every_book_of_theirs() {
        let (r, rows) = search("le guin", &Positions::default());
        assert_eq!(rows[0], "Author: Ursula K. Le Guin", "she's the top hit, not one of her books");
        assert!(matches!(r.top, Some(Hit::Name(Kind::Author, _))));
        assert_eq!(r.books.len(), 3, "all three of her books: {rows:?}");
        assert_eq!(rows.len(), 4, "no one else for a stray \"le\": {rows:?}");
    }

    #[test]
    fn narrators_and_series_are_names_of_their_own() {
        let (_, rows) = search("guidall", &Positions::default());
        assert!(rows.contains(&"Narrator: George Guidall".to_owned()), "{rows:?}");
        assert!(rows.contains(&"The Left Hand of Darkness".to_owned()), "the books they read are found too: {rows:?}");
        let (_, rows) = search("hainish", &Positions::default());
        assert!(rows.contains(&"Series: Hainish".to_owned()), "{rows:?}");
        assert!(rows.contains(&"The Dispossessed".to_owned()));
    }

    #[test]
    fn on_equal_footing_the_book_is_the_top_hit_and_its_namesake_series_follows() {
        let (r, rows) = search("dune", &Positions::default());
        assert_eq!(r.top, Some(Hit::Book(1)));
        assert_eq!(rows, ["Dune", "Series: Dune"]);
    }

    #[test]
    fn typos_still_find_a_book_but_never_as_the_top_hit() {
        let (r, rows) = search("kndred", &Positions::default());
        assert_eq!(r.top, None);
        assert_eq!(rows.first().map(String::as_str), Some("Kindred"));
    }

    #[test]
    fn an_empty_query_lists_the_books_in_progress_most_recent_first() {
        let mut positions = Positions::default();
        for (id, p) in [progress("du", 600.0, false, 3_600), progress("pi", 60.0, false, 60), progress("ki", 9_000.0, true, 10), progress("we", 0.0, false, 5)]
        {
            positions.insert(id, p);
        }
        let (r, rows) = search("  ", &positions);
        assert_eq!(rows, ["Piranesi", "Dune"], "finished and unstarted books aren't in progress");
        assert_eq!(r.top, None);
        let (_, rows) = search("", &Positions::default());
        assert!(rows.is_empty());
    }

    #[test]
    fn nothing_found_is_no_rows() {
        let (r, _) = search("zzqx", &Positions::default());
        assert_eq!(r.len(), 0);
        assert_eq!(r.hit(0), None);
    }

    #[test]
    fn groups_are_capped() {
        let books: Vec<ItemSummary> = (0..20).map(|i| book(&i.to_string(), &format!("Record {i}"), &format!("Record Author {i}"), "Reader", None)).collect();
        let index = BookIndex::new(&books);
        let r = index.search("record", &mut Matcher::default(), &books, &Positions::default());
        assert!(r.books.len() <= MAX_BOOKS && r.names.iter().all(|n| n.len() <= MAX_NAMES));
        assert_eq!(r.hit(0), r.top);
        assert_eq!(r.hit(r.len()), None);
    }
}
