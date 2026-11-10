//! Where each book is, and how the covers and cards say it.
//!
//! [`Positions`] holds the latest position of every book the shelf or the
//! server knows, with a version bumped on every change, so what's derived
//! from it (the books in progress, most recently listened first) is worked
//! out once per change rather than per frame. [`Listened`] is one book's
//! state as the interface shows it: not started, so much left at the saved
//! speed, or finished.

use super::view_ctx::ViewCtx;
use sources::{ItemSummary, Progress};
use std::collections::HashMap;

/// Every book's latest position, by the book's own id.
#[derive(Default)]
pub struct Positions {
    by_book: HashMap<String, Progress>,
    /// Bumped by every change, so derived lists can be memoised on it.
    version: u64,
}

impl Positions {
    pub fn get(&self, id: &str) -> Option<&Progress> {
        self.by_book.get(id)
    }

    pub fn insert(&mut self, id: String, progress: Progress) {
        self.version += 1;
        self.by_book.insert(id, progress);
    }

    pub fn remove(&mut self, id: &str) -> Option<Progress> {
        self.version += 1;
        self.by_book.remove(id)
    }

    pub fn clear(&mut self) {
        self.version += 1;
        self.by_book.clear();
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.by_book.is_empty()
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// Whether any book is started and not finished: what decides whether the
    /// app opens on Home.
    pub fn any_in_progress(&self) -> bool {
        self.by_book.values().any(started)
    }
}

/// Begun, and not finished.
fn started(p: &Progress) -> bool {
    !p.finished && p.position.secs() > 0.0
}

/// Indices into `books` of those in progress, the one listened to most
/// recently first. A position the source didn't date goes after the dated
/// ones; ties fall back to the title and then the id, so the order is stable.
pub fn in_progress(books: &[ItemSummary], positions: &Positions) -> Vec<usize> {
    let mut ids: Vec<usize> = (0..books.len()).filter(|&i| positions.get(&books[i].id).is_some_and(started)).collect();
    ids.sort_by_cached_key(|&i| {
        let at = positions.get(&books[i].id).and_then(|p| p.at);
        (std::cmp::Reverse(at), books[i].title.to_lowercase(), books[i].id.clone())
    });
    ids
}

/// One book's listening, as a cover or a card shows it.
#[derive(Debug, Clone, PartialEq)]
pub enum Listened {
    NotStarted,
    InProgress {
        /// How much is behind, 0 to 1; `None` when the length isn't known.
        fraction: Option<f32>,
        /// Seconds left at the speed, when the length is known.
        left: Option<f64>,
    },
    Finished,
}

/// How far along a book is, without the figures: what the library's filter
/// sorts books by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    NotStarted,
    InProgress,
    Finished,
}

impl Listened {
    pub fn stage(&self) -> Stage {
        match self {
            Self::NotStarted => Stage::NotStarted,
            Self::InProgress { .. } => Stage::InProgress,
            Self::Finished => Stage::Finished,
        }
    }

    /// `progress` as saved, or `playing_at` when the book is the one loaded
    /// (the player is ahead of the last report). Time left is at `speed`.
    pub fn of(book: &ItemSummary, progress: Option<&Progress>, playing_at: Option<f64>, speed: f32) -> Self {
        if playing_at.is_none() && progress.is_some_and(|p| p.finished) {
            return Self::Finished;
        }
        let at = playing_at.or_else(|| progress.filter(|p| !p.finished).map(|p| p.position.secs())).unwrap_or(0.0);
        if at <= 0.0 {
            return Self::NotStarted;
        }
        let duration = book.duration.filter(|d| *d > 0.0);
        Self::InProgress {
            fraction: duration.map(|d| (at / d).clamp(0.0, 1.0) as f32),
            left: duration.map(|d| (d - at).max(0.0) / f64::from(crate::timeline::normalize_speed(speed))),
        }
    }

    /// What a tile's quiet line says: "4h 57m left", "Finished", or nothing
    /// for a book not started.
    pub fn line(&self) -> Option<String> {
        match self {
            Self::NotStarted => None,
            Self::InProgress { left: Some(left), .. } => Some(crate::timeline::span_left(*left)),
            Self::InProgress { left: None, .. } => Some("In progress".into()),
            Self::Finished => Some("Finished".into()),
        }
    }
}

impl ViewCtx<'_> {
    /// `book`'s listening: from the player when it's the book loaded, else as
    /// last saved, with the time left at the saved speed (as the book page
    /// says it).
    pub(super) fn listened(&self, book: &ItemSummary) -> Listened {
        let playing_at = self.player.book.as_ref().filter(|b| b.key.item == book.id).map(|_| self.player.position);
        Listened::of(book, self.positions.get(&book.id), playing_at, self.playback.speed)
    }

    /// The loaded book's stage, when the player is ahead of what was saved
    /// for it (started, and not reported yet): the library's filter goes by
    /// what its tile shows.
    pub(super) fn loaded_stage(&self) -> Option<(String, Stage)> {
        let loaded = self.player.book.as_ref()?;
        let id = &loaded.key.item;
        let saved = Listened::of(&loaded.item.summary, self.positions.get(id), None, 1.0).stage();
        let shown = Listened::of(&loaded.item.summary, self.positions.get(id), Some(self.player.position), 1.0).stage();
        (shown != saved).then(|| (id.clone(), shown))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sources::{ItemKey, Position};
    use std::time::{Duration, SystemTime};

    fn book(id: &str, title: &str) -> ItemSummary {
        ItemSummary { id: id.into(), title: title.into(), duration: Some(3_600.0), ..ItemSummary::default() }
    }

    fn at(id: &str, secs: f64, finished: bool, ago: Option<u64>) -> Progress {
        Progress {
            item: ItemKey::new(sources::AccountId::from_uuid(uuid::Uuid::nil()), id),
            position: Position::from_secs(secs),
            finished,
            at: ago.map(|s| SystemTime::now() - Duration::from_secs(s)),
        }
    }

    #[test]
    fn books_in_progress_are_the_last_listened_first_and_the_undated_last() {
        let books = vec![book("a", "Alpha"), book("b", "Beta"), book("c", "Gamma"), book("d", "Delta"), book("e", "Epsilon"), book("f", "Zeta")];
        let mut positions = Positions::default();
        positions.insert("a".into(), at("a", 100.0, false, Some(3_600)));
        positions.insert("b".into(), at("b", 100.0, false, Some(60)));
        positions.insert("c".into(), at("c", 100.0, true, Some(10)));
        positions.insert("d".into(), at("d", 0.0, false, Some(5)));
        positions.insert("e".into(), at("e", 50.0, false, None));
        positions.insert("f".into(), at("f", 50.0, false, None));
        let titles: Vec<&str> = in_progress(&books, &positions).into_iter().map(|i| books[i].title.as_str()).collect();
        // Finished and not begun are left out; undated ones go last, by title.
        assert_eq!(titles, ["Beta", "Alpha", "Epsilon", "Zeta"]);
        assert!(positions.any_in_progress());
    }

    #[test]
    fn every_change_moves_the_version_on() {
        let mut positions = Positions::default();
        let v = positions.version();
        positions.insert("a".into(), at("a", 1.0, false, None));
        assert!(positions.version() > v);
        let v = positions.version();
        positions.clear();
        assert!(positions.version() > v);
        assert!(positions.is_empty());
        assert!(!positions.any_in_progress());
    }

    #[test]
    fn a_book_reads_as_not_started_partway_or_finished() {
        let b = book("a", "Alpha");
        assert_eq!(Listened::of(&b, None, None, 1.0), Listened::NotStarted);
        assert_eq!(Listened::of(&b, None, None, 1.0).line(), None);
        let partway = Listened::of(&b, Some(&at("a", 900.0, false, None)), None, 1.5);
        assert_eq!(partway, Listened::InProgress { fraction: Some(0.25), left: Some(1_800.0) }, "time left is at the speed");
        assert_eq!(partway.line().as_deref(), Some("30m left"));
        let done = Listened::of(&b, Some(&at("a", 3_600.0, true, None)), None, 1.0);
        assert_eq!(done.line().as_deref(), Some("Finished"));
        // The book loaded in the player is where the player is.
        assert_eq!(Listened::of(&b, Some(&at("a", 900.0, false, None)), Some(1_800.0), 1.0), Listened::InProgress { fraction: Some(0.5), left: Some(1_800.0) });
        // Without a length, it's in progress but can't say how far.
        let unknown = ItemSummary { duration: None, ..b };
        assert_eq!(Listened::of(&unknown, Some(&at("a", 10.0, false, None)), None, 1.0).line().as_deref(), Some("In progress"));
    }
}
