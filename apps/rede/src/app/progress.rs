//! Where each book is, as the covers say it: [`Listened`] is one book's
//! state as the interface shows it, not started, so much left at the saved
//! speed, or finished.

use super::view_ctx::ViewCtx;
use sources::{ItemSummary, Progress};

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

impl Listened {
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
