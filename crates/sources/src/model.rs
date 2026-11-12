//! What a book, a library or a position is, in Rede's terms.
//!
//! These are the models the app draws and the shelf stores; none of them
//! names a server, a JSON field or a wire format. A source maps its own reply
//! into them ([`AbsSource`](crate::AbsSource) does), so adding Jellyfin later
//! is a new mapping rather than a change to the app.
//!
//! Units are the ones the design uses: durations and positions are float
//! seconds, [`Stamp`]s are integer milliseconds since the epoch (what
//! Audiobookshelf's `addedAt` and `updatedAt` are), and listening time is a
//! [`Duration`]. Keeping the last two apart is why [`Position`] exists: a
//! position in a book and a moment in time look alike otherwise.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::id::{AccountId, ItemKey};

/// A library's id, as its source gave it. Only meaningful with the account it
/// came from.
pub type LibraryId = String;

/// Which side of the app a library's items belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LibraryKind {
    Books,
    Podcasts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    pub id: LibraryId,
    pub name: String,
    pub kind: LibraryKind,
}

/// A moment a source stamps an item with: milliseconds since the Unix epoch.
///
/// It's a newtype so it can't be mistaken for a [`Position`], a duration, or
/// the milliseconds since an arbitrary start that some servers report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Stamp(i64);

impl Stamp {
    pub const EPOCH: Stamp = Stamp(0);

    pub fn from_millis(ms: i64) -> Self {
        Self(ms)
    }

    pub fn millis(self) -> i64 {
        self.0
    }

    /// The moment as a [`SystemTime`]. A stamp before 1970 reads as the epoch:
    /// nothing Rede stores is earlier.
    pub fn system_time(self) -> SystemTime {
        UNIX_EPOCH.checked_add(Duration::from_millis(self.0.max(0).unsigned_abs())).unwrap_or(UNIX_EPOCH)
    }
}

impl From<SystemTime> for Stamp {
    fn from(time: SystemTime) -> Self {
        Self(time.duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)))
    }
}

impl From<Stamp> for SystemTime {
    fn from(stamp: Stamp) -> Self {
        stamp.system_time()
    }
}

/// A point in a book: seconds from its start.
///
/// It's the book's own timeline, which is what the progress log, the chapters
/// and the scrubber speak; it isn't the engine's output position, which moves
/// at a different rate once speed and silence removal are in play.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Position(f64);

impl Position {
    pub fn from_secs(secs: f64) -> Self {
        Self(secs)
    }

    pub fn secs(self) -> f64 {
        self.0
    }
}

/// One series a book is in, and where in it. A book can be in several.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Series {
    pub name: String,
    /// "1", "2.5", whatever the metadata said. A string because sources keep
    /// what was typed, including "Book 3".
    pub sequence: Option<String>,
}

/// What a listing says about an item: everything the library grid needs and
/// nothing that takes another request.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ItemSummary {
    /// The source's own id. Unique only within the account.
    pub id: String,
    pub library: LibraryId,
    pub title: String,
    pub subtitle: Option<String>,
    /// Names as the source spells them, in its order.
    pub authors: Vec<String>,
    pub narrators: Vec<String>,
    pub series: Vec<Series>,
    /// Seconds.
    pub duration: Option<f64>,
    /// What [`Source::cover`](crate::Source::cover) takes for this item.
    pub cover: Option<String>,
    pub added: Option<Stamp>,
    /// When the source last changed it, for deciding what to refetch.
    pub updated: Option<Stamp>,
}

/// One file of a book, as the book's timeline sees it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TrackFile {
    /// The source's number for the file, kept for display and for matching a
    /// downloaded file back to its track.
    pub index: u32,
    /// Where the file starts in the book, in seconds. The timeline is the
    /// tracks' start offsets laid end to end.
    pub start_offset: f64,
    pub duration: Option<f64>,
    /// The file's extension without the dot, for the stream cache and the
    /// offline store.
    pub suffix: Option<String>,
    pub size: Option<u64>,
    pub mime: Option<String>,
    /// The source's id for the file, which names it on disk. With the account
    /// it makes the [`ItemKey`] the cache uses.
    pub content_id: String,
}

/// A range on the book's timeline.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Chapter {
    /// Seconds from the start of the book.
    pub start: f64,
    pub end: f64,
    pub title: String,
}

/// A book with everything known about it. The summary is the listing's view;
/// the rest needs one more request, which is why the shelf keeps the two
/// apart.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Item {
    pub summary: ItemSummary,
    pub description: Option<String>,
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    /// As the source spells it ("2001", sometimes "Spring 2001").
    pub published_year: Option<String>,
    pub publisher: Option<String>,
    pub language: Option<String>,
    pub asin: Option<String>,
    pub isbn: Option<String>,
    pub tracks: Vec<TrackFile>,
    pub chapters: Vec<Chapter>,
}

/// A book's position on its source, as the source remembers it. Used to
/// reconcile with the local progress log when Rede starts or gains focus.
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub item: ItemKey,
    pub position: Position,
    pub finished: bool,
    /// When the source last saw it change; `None` if it doesn't say.
    pub at: Option<SystemTime>,
}

/// One entry of the local progress log: a position, and what it took to get
/// there.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgressUpdate {
    pub item: ItemKey,
    pub position: Position,
    pub finished: bool,
    /// When it happened on this device.
    pub at: SystemTime,
    /// Listening time since the previous entry, for the source's statistics.
    pub listened: Duration,
}

/// A track of an open play session, ready to fetch.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SessionTrack {
    pub index: u32,
    pub start_offset: f64,
    pub duration: Option<f64>,
    pub size: Option<u64>,
    pub mime: Option<String>,
    /// Absolute, and for Audiobookshelf carrying the access token in its
    /// query: a secret. Don't log it, and rebuild it after a refresh.
    pub url: String,
}

/// A session on the source that this device is playing in.
///
/// Some sources remember where the listening is happening (Audiobookshelf uses
/// it for statistics and for closing the session when playback stops), so the
/// session is what [`report`](crate::Source::report) and
/// [`close`](crate::Source::close) take.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaySession {
    /// The source's id for the session.
    pub id: String,
    /// The item being played, as the id [`open`](crate::Source::open) was
    /// given.
    pub item: String,
    /// Where playback should start, from the source's own progress.
    pub start: Position,
    pub duration: Option<f64>,
    pub tracks: Vec<SessionTrack>,
}

/// What a source can do. The app checks these and hides what's missing, so it
/// never has to know which kind of source it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities {
    /// Search on the server rather than in the local mirror.
    pub server_search: bool,
    /// Bookmarks live on the server ([`Source::bookmarks`](crate::Source::bookmarks)).
    pub bookmarks: bool,
    /// Pushes changes as they happen (Audiobookshelf's socket.io).
    pub live_updates: bool,
    /// Podcast feeds and podcast libraries.
    pub subscriptions: bool,
    pub transcoding: bool,
    /// Listening statistics recorded without a server session.
    pub local_sessions: bool,
}

/// The device a play session comes from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceInfo {
    /// Stable for an installation: a source that ties sessions to a device
    /// closes the old one when the same id opens a new session.
    pub device_id: String,
    pub client_name: String,
    pub client_version: String,
    pub manufacturer: Option<String>,
    /// Shown in a server's session list ("macOS").
    pub model: Option<String>,
}

/// What a listing returned: the items that changed, the ones that are gone,
/// and how far the listing reaches.
///
/// A change set belongs to the account it came from, because the summaries in
/// it don't carry one; the shelf keys rows by [`ItemKey`], which needs both.
#[derive(Debug, Clone, PartialEq)]
pub struct Changes<T> {
    pub account: AccountId,
    pub items: Vec<T>,
    /// Ids the source no longer has. Empty when it can't tell, which is the
    /// case for a listing that only reports what's there.
    pub removed: Vec<String>,
    /// The newest [`ItemSummary::updated`] the source listed, to pass back as
    /// `since` next time. It covers everything that was listed, not only what
    /// is in `items`.
    pub stamp: Option<Stamp>,
}

impl<T> Changes<T> {
    pub fn new(account: AccountId) -> Self {
        Self { account, items: Vec::new(), removed: Vec::new(), stamp: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stamp_and_a_system_time_are_the_same_moment() {
        let now = UNIX_EPOCH + Duration::from_millis(1_766_000_000_123);
        let stamp = Stamp::from(now);
        assert_eq!(stamp.millis(), 1_766_000_000_123);
        assert_eq!(stamp.system_time(), now);
        assert_eq!(Stamp::from(now).system_time(), now);
        // A clock before 1970 is the epoch, not a panic.
        assert_eq!(Stamp::from(UNIX_EPOCH - Duration::from_secs(5)).millis(), 0);
        assert_eq!(Stamp::from_millis(-5).system_time(), UNIX_EPOCH);
    }

    #[test]
    fn positions_compare_and_default_to_the_start() {
        assert_eq!(Position::default(), Position::from_secs(0.0));
        assert!(Position::from_secs(1.5) > Position::from_secs(1.0));
        assert_eq!(Position::from_secs(90.5).secs(), 90.5);
    }
}

/// A saved place. Position is the source's exact book time: rounding it would
/// change the identity of an Audiobookshelf bookmark.
#[derive(Debug, Clone, PartialEq)]
pub struct Bookmark {
    pub item: ItemKey,
    pub position: Position,
    pub title: String,
    pub created: SystemTime,
}

/// Listening recorded on this device without an open server session.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalSession {
    pub id: String,
    pub item: ItemKey,
    pub library: String,
    pub duration: Option<f64>,
    pub start: Position,
    pub current: Position,
    pub listened: Duration,
    pub started_at: SystemTime,
    pub updated_at: SystemTime,
}

/// A stretch of listening the source remembers, on any device: what listening
/// stats are made of.
#[derive(Debug, Clone, PartialEq)]
pub struct Listen {
    /// The item's own id.
    pub item: String,
    /// What the source called the book then, for an item no longer listed.
    pub title: Option<String>,
    pub author: Option<String>,
    pub started: Stamp,
    /// Time spent listening, at whatever speed: not how much of the book went
    /// by.
    pub listened: Duration,
}
