//! The stored rows, one struct per shape a build has written.
//!
//! Rows are postcard, which writes fields in order with no names or tags. The
//! bytes only mean something to the exact struct that wrote them, so the rules
//! Ond's `library` crate spells out hold here too:
//!
//! - Once a build has shipped a struct, it is frozen. No field is added,
//!   removed, reordered or retyped. postcard reads none of those as a
//!   compatible change: an added field fails to decode old rows, and a removed
//!   trailing one decodes them into the wrong fields.
//! - Freezing a struct freezes every type inside it. Nested types are
//!   versioned the same way (`SeriesV1`, `TrackFileV1`, ...), never shared with
//!   the app's models in [`sources`]: those are free to change, these aren't.
//! - postcard writes an enum variant as its index. While an enum is current a
//!   variant may be added at the end, since old rows never hold its index.
//!   Once frozen, not even that.
//!
//! To change a stored type, copy it to the next version, point the row at the
//! copy, give each struct that holds it a new version too, bump
//! [`SCHEMA_VERSION`](crate::SCHEMA_VERSION) and add a step to `migrate` that
//! rewrites the rows. The golden-bytes tests fail when the current types'
//! shape changes without that.
//!
//! Units are the ones on disk: seconds as `f64` for positions and durations,
//! milliseconds as `i64` since the Unix epoch for times, which is what the
//! sources report.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use sources::{AccountId, Chapter, Item, ItemKey, ItemSummary, Position, Progress, ProgressUpdate, Series, Stamp, TrackFile};

/// What the shelf stores for one item: the listing's view of it, and the rest
/// once it has been fetched.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct ItemV1 {
    pub summary: SummaryV1,
    /// `None` until the item is opened or downloaded: a listing has no tracks,
    /// chapters or description.
    pub details: Option<DetailsV1>,
}

/// The fields a listing gives, and the only ones a sync rewrites.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct SummaryV1 {
    pub id: String,
    pub library: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub authors: Vec<String>,
    pub narrators: Vec<String>,
    pub series: Vec<SeriesV1>,
    pub duration: Option<f64>,
    pub cover: Option<String>,
    pub added_ms: Option<i64>,
    pub updated_ms: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct SeriesV1 {
    pub name: String,
    pub sequence: Option<String>,
}

/// Everything an item has that the listing doesn't.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct DetailsV1 {
    pub description: Option<String>,
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub published_year: Option<String>,
    pub publisher: Option<String>,
    pub language: Option<String>,
    pub asin: Option<String>,
    pub isbn: Option<String>,
}

impl From<&Item> for DetailsV1 {
    fn from(item: &Item) -> Self {
        Self {
            description: item.description.clone(),
            genres: item.genres.clone(),
            tags: item.tags.clone(),
            published_year: item.published_year.clone(),
            publisher: item.publisher.clone(),
            language: item.language.clone(),
            asin: item.asin.clone(),
            isbn: item.isbn.clone(),
        }
    }
}

/// An item's files, in book order: the timeline is their start offsets laid
/// end to end.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub(crate) struct TracksV1 {
    pub files: Vec<TrackFileV1>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct TrackFileV1 {
    pub index: u32,
    pub start_offset: f64,
    pub duration: Option<f64>,
    pub suffix: Option<String>,
    pub size: Option<u64>,
    pub mime: Option<String>,
    pub content_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub(crate) struct ChaptersV1 {
    pub chapters: Vec<ChapterV1>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct ChapterV1 {
    pub start: f64,
    pub end: f64,
    pub title: String,
}

impl From<&ItemSummary> for SummaryV1 {
    fn from(summary: &ItemSummary) -> Self {
        Self {
            id: summary.id.clone(),
            library: summary.library.clone(),
            title: summary.title.clone(),
            subtitle: summary.subtitle.clone(),
            authors: summary.authors.clone(),
            narrators: summary.narrators.clone(),
            series: summary.series.iter().map(SeriesV1::from).collect(),
            duration: summary.duration,
            cover: summary.cover.clone(),
            added_ms: summary.added.map(Stamp::millis),
            updated_ms: summary.updated.map(Stamp::millis),
        }
    }
}

impl From<SummaryV1> for ItemSummary {
    fn from(row: SummaryV1) -> Self {
        Self {
            id: row.id,
            library: row.library,
            title: row.title,
            subtitle: row.subtitle,
            authors: row.authors,
            narrators: row.narrators,
            series: row.series.into_iter().map(Series::from).collect(),
            duration: row.duration,
            cover: row.cover,
            added: row.added_ms.map(Stamp::from_millis),
            updated: row.updated_ms.map(Stamp::from_millis),
        }
    }
}

impl From<&Series> for SeriesV1 {
    fn from(series: &Series) -> Self {
        Self { name: series.name.clone(), sequence: series.sequence.clone() }
    }
}

impl From<SeriesV1> for Series {
    fn from(row: SeriesV1) -> Self {
        Self { name: row.name, sequence: row.sequence }
    }
}

impl From<&[TrackFile]> for TracksV1 {
    fn from(files: &[TrackFile]) -> Self {
        Self {
            files: files
                .iter()
                .map(|file| TrackFileV1 {
                    index: file.index,
                    start_offset: file.start_offset,
                    duration: file.duration,
                    suffix: file.suffix.clone(),
                    size: file.size,
                    mime: file.mime.clone(),
                    content_id: file.content_id.clone(),
                })
                .collect(),
        }
    }
}

impl From<TracksV1> for Vec<TrackFile> {
    fn from(row: TracksV1) -> Self {
        row.files
            .into_iter()
            .map(|file| TrackFile {
                index: file.index,
                start_offset: file.start_offset,
                duration: file.duration,
                suffix: file.suffix,
                size: file.size,
                mime: file.mime,
                content_id: file.content_id,
            })
            .collect()
    }
}

impl From<&[Chapter]> for ChaptersV1 {
    fn from(chapters: &[Chapter]) -> Self {
        Self { chapters: chapters.iter().map(|c| ChapterV1 { start: c.start, end: c.end, title: c.title.clone() }).collect() }
    }
}

impl From<ChaptersV1> for Vec<Chapter> {
    fn from(row: ChaptersV1) -> Self {
        row.chapters.into_iter().map(|c| Chapter { start: c.start, end: c.end, title: c.title }).collect()
    }
}

impl ItemV1 {
    /// The app's item, or `None` when its details were never fetched: a
    /// summary alone can't make the tracks, chapters and description an `Item`
    /// promises.
    pub(crate) fn into_item(self, tracks: TracksV1, chapters: ChaptersV1) -> Option<Item> {
        let details = self.details?;
        Some(Item {
            summary: self.summary.into(),
            description: details.description,
            genres: details.genres,
            tags: details.tags,
            published_year: details.published_year,
            publisher: details.publisher,
            language: details.language,
            asin: details.asin,
            isbn: details.isbn,
            tracks: tracks.into(),
            chapters: chapters.into(),
        })
    }
}

/// One entry of the append-only progress log.
///
/// `synced` is the one field a write changes after the entry is appended:
/// [`Shelf::mark_synced`](crate::Shelf::mark_synced) sets it, and the log
/// itself keeps the entry either way, so the order and the times a device
/// listened survive a sync.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct ProgressEntryV1 {
    pub account: String,
    pub item: String,
    pub position: f64,
    pub finished: bool,
    pub at_ms: i64,
    pub listened_ms: u64,
    pub synced: bool,
}

impl ProgressEntryV1 {
    pub(crate) fn new(key: &ItemKey, update: &ProgressUpdate) -> Self {
        Self {
            account: key.account.to_string(),
            item: key.item.clone(),
            position: update.position.secs(),
            finished: update.finished,
            at_ms: Stamp::from(update.at).millis(),
            listened_ms: u64::try_from(update.listened.as_millis()).unwrap_or(u64::MAX),
            synced: false,
        }
    }

    /// The entry as the app and the sources see it. An account string that
    /// doesn't parse is damage, and reads as an unreadable row.
    pub(crate) fn update(&self) -> Option<ProgressUpdate> {
        Some(ProgressUpdate {
            item: ItemKey::new(self.account.parse::<AccountId>().ok()?, self.item.clone()),
            position: Position::from_secs(self.position),
            finished: self.finished,
            at: Stamp::from_millis(self.at_ms).system_time(),
            listened: Duration::from_millis(self.listened_ms),
        })
    }
}

/// Where an item is now: the newest log entry for it, kept alongside the log
/// so reading a position doesn't mean replaying it.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub(crate) struct PositionV1 {
    pub position: f64,
    pub finished: bool,
    pub at_ms: i64,
}

impl PositionV1 {
    pub(crate) fn progress(self, key: &ItemKey) -> Progress {
        Progress {
            item: key.clone(),
            position: Position::from_secs(self.position),
            finished: self.finished,
            at: Some(Stamp::from_millis(self.at_ms).system_time()),
        }
    }
}

/// A bookmark: a place in a book with a note, and when it was made. Its id is
/// the shelf's, so two bookmarks at the same second stay apart and a note
/// keeps its identity when the position is the same.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct BookmarkV1 {
    pub position: f64,
    pub note: Option<String>,
    pub created_ms: i64,
}

/// How far a library's listing got, and how new what it saw was.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub(crate) struct LibrarySyncV1 {
    pub cursor: Option<String>,
    pub newest_ms: Option<i64>,
}

/// Ordered bookmark intent. All nested rows are frozen independently of the
/// live source model; kind 0=create, 1=rename, 2=delete is part of this format.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct BookmarkEditV1 {
    pub account: String,
    pub item: String,
    pub kind: u8,
    pub value: BookmarkV1,
    pub base: Option<BookmarkV1>,
}

/// The local session identity is written with each report so a restart or a
/// lost HTTP response can replay the same session without counting it twice.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct LocalListeningV1 {
    pub id: String,
    pub start: f64,
    pub started_at_ms: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct ProgressEntryV2 {
    pub entry: ProgressEntryV1,
    pub local: Option<LocalListeningV1>,
}
