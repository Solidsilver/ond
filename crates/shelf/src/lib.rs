//! Rede's shelf: one redb database per account.
//!
//! Like Ond's `library` mirror, the UI reads only from here, so browsing never
//! waits on the network; a sync writes what a source listed, and details
//! (tracks, chapters, description) are written when a book is opened or
//! downloaded. One file per account, so removing an account is deleting its
//! file along with its downloads, and an unreachable server doesn't touch
//! another account's shelf. The merged view across accounts is built in memory
//! by the app.
//!
//! Layout: one primary table per record, keyed by `(account, item)` for
//! everything that belongs to an item, plus hand-written index tables whose
//! keys are tuples and whose values are `()`. Every write touches a record and
//! its index entries in one transaction.
//!
//! - **Progress** is an append-only log keyed by a counter's id, with a second
//!   table holding the ids that haven't been sent yet. The log is the truth
//!   about how a book was listened to; the position per item is derived from
//!   it and stored apart, so reading a position is one lookup.
//! - **Bookmarks** are rows keyed by an internal id, found by the pair a source
//!   treats as their identity: the item and the exact book time (never a rounded
//!   one). Each local edit writes its journal entry in the same transaction, so
//!   it is visible at once, survives a restart, and is replayed to the source in
//!   the order it was made.
//! - **Deleting an item** (gone from the server) drops its row, its index
//!   entry, its tracks and its chapters. Its progress, bookmarks and position
//!   stay: a book that's missing from one listing may come back, and losing a
//!   position to a bad scan would be worse than keeping a few rows.
//! - **Details survive a summary update.** A listing carries less than a
//!   details fetch, and Audiobookshelf's `updatedAt` doesn't move for every
//!   change to a book's files, so there is nothing reliable to invalidate on.
//!   The app refetches details when it opens a book, which is the policy the
//!   design asks for.
//!
//! Schema versions: `SCHEMA_VERSION` stamps the file. A file from a newer
//! build is refused with [`Error::NewerSchema`] rather than read as whatever
//! its bytes happen to mean, and an upgrade with no step is
//! [`Error::NoMigration`]. The stored rows themselves are frozen (`model`).

mod bookmarks;
mod model;
pub use bookmarks::{BookmarkChange, BookmarkSnapshot, PendingBookmark};

use model::{BookmarkV1, ChaptersV1, DetailsV1, ItemV1, LibrarySyncV1, LocalListeningV1, PositionV1, ProgressEntryV1, ProgressEntryV2, SummaryV1, TracksV1};

use redb::{Database, ReadableDatabase, ReadableTable, Table, TableDefinition, WriteTransaction};
use serde::{Serialize, de::DeserializeOwned};
use sources::{Changes, Item, ItemKey, ItemSummary, Library, Position, Progress, ProgressUpdate, Stamp};
use std::path::Path;
use std::time::SystemTime;

/// Bump when a stored row or an index layout changes, and handle the step in
/// `migrate`.
pub(crate) const SCHEMA_VERSION: u32 = 3;

const META: TableDefinition<&str, u32> = TableDefinition::new("meta");
/// Ids that only go up: log entries and bookmarks are ordered by them, so the
/// log is append-only and an id is never handed out twice.
const COUNTERS: TableDefinition<&str, u64> = TableDefinition::new("counters");
/// (account, item) → [`ItemV1`].
const ITEMS: TableDefinition<(&str, &str), &[u8]> = TableDefinition::new("items");
/// (library, account, item) → (): a library's items, in key order.
const ITEMS_BY_LIBRARY: TableDefinition<(&str, &str, &str), ()> = TableDefinition::new("items_by_library");
/// (account, item) → [`TracksV1`].
const TRACKS: TableDefinition<(&str, &str), &[u8]> = TableDefinition::new("tracks");
/// (account, item) → [`ChaptersV1`].
const CHAPTERS: TableDefinition<(&str, &str), &[u8]> = TableDefinition::new("chapters");
/// Log id → [`ProgressEntryV2`], append-only.
const PROGRESS: TableDefinition<u64, &[u8]> = TableDefinition::new("progress");
/// Log id → (): the entries no source has been told about yet, in log order.
const UNSYNCED: TableDefinition<u64, ()> = TableDefinition::new("progress_unsynced");
/// (account, item) → [`PositionV1`]: the newest log entry, kept apart.
const POSITIONS: TableDefinition<(&str, &str), &[u8]> = TableDefinition::new("positions");
/// (account, item, bookmark id) → [`BookmarkV1`].
const BOOKMARKS: TableDefinition<(&str, &str, u64), &[u8]> = TableDefinition::new("bookmarks");
/// Edit id → [`BookmarkEditV1`](model::BookmarkEditV1): bookmark changes the
/// server hasn't taken yet, in the order they were made.
const BOOKMARK_EDITS: TableDefinition<u64, &[u8]> = TableDefinition::new("bookmark_edits");
/// Library id → [`LibrarySyncV1`].
const SYNC: TableDefinition<&str, &[u8]> = TableDefinition::new("sync");

const SCHEMA_KEY: &str = "schema_version";
const NEXT_PROGRESS: &str = "next_progress";
const NEXT_BOOKMARK: &str = "next_bookmark";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Boxed: redb's error is large and `Result` is returned everywhere.
    #[error(transparent)]
    Db(Box<redb::Error>),
    #[error("corrupt row: {0}")]
    Codec(#[from] postcard::Error),
    #[error(
        "The shelf database comes from a newer version of Rede (schema v{found}; this build reads v{SCHEMA_VERSION}). Update Rede, or delete the file to sync again"
    )]
    NewerSchema { found: u32 },
    /// A build bug: `SCHEMA_VERSION` was bumped without a step in `migrate`.
    /// Not damage, so the file is left alone for a build that can upgrade it.
    #[error("This build of Rede can't upgrade the shelf database from schema v{from}. Update Rede, or delete the file to sync again")]
    NoMigration { from: u32 },
}

macro_rules! from_redb {
    ($($t:ty),*) => {$(
        impl From<$t> for Error {
            fn from(e: $t) -> Self {
                Self::Db(Box::new(e.into()))
            }
        }
    )*};
}
from_redb!(redb::Error, redb::DatabaseError, redb::TransactionError, redb::TableError, redb::StorageError, redb::CommitError);

pub type Result<T> = std::result::Result<T, Error>;

/// A log entry's id, which is also what [`Shelf::mark_synced`] takes.
pub type ProgressId = u64;
/// A bookmark's id, unique within its item.
pub type BookmarkId = u64;

/// One log entry that hasn't reached the source yet.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingProgress {
    pub id: ProgressId,
    pub update: ProgressUpdate,
    pub local: Option<LocalListening>,
}

/// The durable identity and starting place of listening without a server.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalListening {
    pub id: String,
    pub start: Position,
    pub started_at: SystemTime,
}

/// A saved place in a book.
#[derive(Debug, Clone, PartialEq)]
pub struct Bookmark {
    pub id: BookmarkId,
    pub position: Position,
    pub note: Option<String>,
    pub created: SystemTime,
}

/// How far a library's listing got, and how new what it saw was. The cursor is
/// opaque to the shelf: it's whatever the source needs to carry on (a page, a
/// token), and the newest stamp is the `since` of the next sync.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SyncState {
    pub cursor: Option<String>,
    pub newest: Option<Stamp>,
}

/// One account's database.
pub struct Shelf {
    db: Database,
}

impl Shelf {
    /// Opens (or creates) the database file at `path`.
    ///
    /// A file from a newer build is refused ([`Error::NewerSchema`]), never
    /// opened: its rows mean something this build can't read.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::init(Database::create(path)?)
    }

    /// Creates every table (so read transactions never see a missing one) and
    /// brings the schema up to [`SCHEMA_VERSION`].
    fn init(db: Database) -> Result<Self> {
        let wt = db.begin_write()?;
        {
            let mut meta = wt.open_table(META)?;
            match meta.get(SCHEMA_KEY)?.map(|v| v.value()) {
                Some(v) if v > SCHEMA_VERSION => return Err(Error::NewerSchema { found: v }),
                Some(v) if v < SCHEMA_VERSION => migrate(&wt, v, SCHEMA_VERSION)?,
                // No stamp means the file was just created; v1 is the first
                // schema, so no older file lacks one.
                _ => {}
            }
            meta.insert(SCHEMA_KEY, SCHEMA_VERSION)?;
            wt.open_table(COUNTERS)?;
            wt.open_table(ITEMS)?;
            wt.open_table(ITEMS_BY_LIBRARY)?;
            wt.open_table(TRACKS)?;
            wt.open_table(CHAPTERS)?;
            wt.open_table(PROGRESS)?;
            wt.open_table(UNSYNCED)?;
            wt.open_table(POSITIONS)?;
            wt.open_table(BOOKMARKS)?;
            wt.open_table(BOOKMARK_EDITS)?;
            wt.open_table(SYNC)?;
        }
        wt.commit()?;
        Ok(Self { db })
    }

    #[cfg(test)]
    pub fn schema_version(&self) -> Result<u32> {
        let rt = self.db.begin_read()?;
        Ok(rt.open_table(META)?.get(SCHEMA_KEY)?.map_or(0, |v| v.value()))
    }

    /// Writes what a sync saw: the items that changed, and the ones the source
    /// no longer has, which lose their row, tracks and chapters.
    ///
    /// `library` is authoritative: a change set belongs to one library, so its
    /// id is what the row and the listing index get.
    pub fn apply_changes(&self, library: &Library, changes: &Changes<ItemSummary>) -> Result<()> {
        let account = changes.account.to_string();
        let wt = self.db.begin_write()?;
        {
            let mut items = wt.open_table(ITEMS)?;
            let mut index = wt.open_table(ITEMS_BY_LIBRARY)?;
            let mut tracks = wt.open_table(TRACKS)?;
            let mut chapters = wt.open_table(CHAPTERS)?;
            for summary in &changes.items {
                let item = summary.id.as_str();
                let key = (account.as_str(), item);
                let old = items.get(key)?.map(|held| old_item(item, held.value()));
                let details = match &old {
                    Some(Some(row)) => {
                        // An item moved between libraries keeps its old listing
                        // entry unless it's removed here.
                        if row.summary.library != library.id {
                            index.remove((row.summary.library.as_str(), account.as_str(), item))?;
                        }
                        row.details.clone()
                    }
                    // A row that can't be read leaves its library unknown, so
                    // its entries are found by id instead.
                    Some(None) => {
                        index.retain(|(_, a, i), ()| a != account.as_str() || i != item)?;
                        None
                    }
                    None => None,
                };
                let mut row = SummaryV1::from(summary);
                row.library = library.id.clone();
                items.insert(key, encode(&ItemV1 { summary: row, details })?.as_slice())?;
                index.insert((library.id.as_str(), account.as_str(), item), ())?;
            }
            for item in &changes.removed {
                let item = item.as_str();
                let key = (account.as_str(), item);
                let old = items.get(key)?.map(|held| old_item(item, held.value()));
                items.remove(key)?;
                match &old {
                    Some(Some(row)) => {
                        index.remove((row.summary.library.as_str(), account.as_str(), item))?;
                    }
                    // Unreadable, so its library is unknown: scan off its
                    // entries. A row that isn't there has none.
                    Some(None) => {
                        index.retain(|(_, a, i), ()| a != account.as_str() || i != item)?;
                    }
                    None => {}
                }
                tracks.remove(key)?;
                chapters.remove(key)?;
            }
        }
        wt.commit()?;
        Ok(())
    }

    /// An item with its tracks, chapters and description, or `None` when there
    /// is no row or its details were never fetched (a listing alone can't make
    /// one). [`items`](Self::items) has the listing's view either way.
    pub fn item(&self, key: &ItemKey) -> Result<Option<Item>> {
        let account = key.account.to_string();
        let rt = self.db.begin_read()?;
        let Some(row) = get_row::<ItemV1>(&rt.open_table(ITEMS)?, &account, &key.item)? else { return Ok(None) };
        let tracks = get_row::<TracksV1>(&rt.open_table(TRACKS)?, &account, &key.item)?.unwrap_or_default();
        let chapters = get_row::<ChaptersV1>(&rt.open_table(CHAPTERS)?, &account, &key.item)?.unwrap_or_default();
        Ok(row.into_item(tracks, chapters))
    }

    /// A library's stored summaries, in key order (the app sorts what it
    /// draws).
    pub fn items(&self, library: &str) -> Result<Vec<ItemSummary>> {
        let rt = self.db.begin_read()?;
        let rows = rt.open_table(ITEMS)?;
        let mut out = Vec::new();
        for entry in rt.open_table(ITEMS_BY_LIBRARY)?.range((library, "", "")..)? {
            let (row_key, _) = entry?;
            let (found, account, item) = row_key.value();
            // Library ids that are prefixes of each other don't interleave;
            // the first entry of another library is the end of this one.
            if found != library {
                break;
            }
            if let Some(row) = get_row_or_skip::<ItemV1>(&rows, account, item)? {
                out.push(row.summary.into());
            }
        }
        Ok(out)
    }

    /// Stores everything fetched for `item`: its summary, its tracks and its
    /// chapters, in one transaction.
    ///
    /// The library recorded is the listed one's: a details fetch never moves
    /// an item, so the summary's own library is only used when the listing
    /// hasn't stored the item at all.
    pub fn set_details(&self, key: &ItemKey, item: &Item) -> Result<()> {
        let account = key.account.to_string();
        let row_key = (account.as_str(), key.item.as_str());
        let wt = self.db.begin_write()?;
        {
            let mut items = wt.open_table(ITEMS)?;
            let mut index = wt.open_table(ITEMS_BY_LIBRARY)?;
            let mut tracks = wt.open_table(TRACKS)?;
            let mut chapters = wt.open_table(CHAPTERS)?;
            let existing = items.get(row_key)?.and_then(|held| old_item(&key.item, held.value()));
            let mut summary = SummaryV1::from(&item.summary);
            // The row is where the key is; an item whose summary named another
            // id would be stored where nothing looks for it.
            summary.id.clone_from(&key.item);
            if let Some(existing) = &existing {
                summary.library = existing.summary.library.clone();
            }
            let row = ItemV1 { summary, details: Some(DetailsV1::from(item)) };
            items.insert(row_key, encode(&row)?.as_slice())?;
            if existing.is_none() {
                index.insert((row.summary.library.as_str(), account.as_str(), key.item.as_str()), ())?;
            }
            tracks.insert(row_key, encode(&TracksV1::from(item.tracks.as_slice()))?.as_slice())?;
            chapters.insert(row_key, encode(&ChaptersV1::from(item.chapters.as_slice()))?.as_slice())?;
        }
        wt.commit()?;
        Ok(())
    }

    /// Appends a position to the log and makes it the item's current one.
    ///
    /// The entry waits in [`pending_progress`](Self::pending_progress) until it
    /// is sent. For a source that has no share in progress (local folders,
    /// feeds) the log is the progress; nothing sends it.
    pub fn append_progress(&self, update: &ProgressUpdate) -> Result<ProgressId> {
        self.append_listening(update, None)
    }

    /// Appends a report and its local-session identity in one transaction.
    pub fn append_listening(&self, update: &ProgressUpdate, local: Option<&LocalListening>) -> Result<ProgressId> {
        let entry = ProgressEntryV1::new(&update.item, update);
        let position = PositionV1 { position: update.position.secs(), finished: update.finished, at_ms: entry.at_ms };
        let account = update.item.account.to_string();
        let wt = self.db.begin_write()?;
        let id = {
            let mut counters = wt.open_table(COUNTERS)?;
            let id = next_id(&mut counters, NEXT_PROGRESS)?;
            wt.open_table(PROGRESS)?.insert(
                id,
                encode(&ProgressEntryV2 {
                    entry,
                    local: local.map(|l| LocalListeningV1 { id: l.id.clone(), start: l.start.secs(), started_at_ms: Stamp::from(l.started_at).millis() }),
                })?
                .as_slice(),
            )?;
            wt.open_table(UNSYNCED)?.insert(id, ())?;
            wt.open_table(POSITIONS)?.insert((account.as_str(), update.item.item.as_str()), encode(&position)?.as_slice())?;
            id
        };
        wt.commit()?;
        Ok(id)
    }

    /// The log entries no source has been told about, oldest first.
    pub fn pending_progress(&self) -> Result<Vec<PendingProgress>> {
        let rt = self.db.begin_read()?;
        let log = rt.open_table(PROGRESS)?;
        let mut out = Vec::new();
        for entry in rt.open_table(UNSYNCED)?.iter()? {
            let (id, _) = entry?;
            let id = id.value();
            let Some(held) = log.get(id)? else {
                tracing::warn!("shelf: progress {id} is marked unsent but has no entry");
                continue;
            };
            match decode::<ProgressEntryV2>(held.value()) {
                Ok(row) => match row.entry.update() {
                    Some(update) => out.push(PendingProgress {
                        id,
                        update,
                        local: row.local.map(|l| LocalListening {
                            id: l.id,
                            start: Position::from_secs(l.start),
                            started_at: Stamp::from_millis(l.started_at_ms).system_time(),
                        }),
                    }),
                    None => tracing::warn!("shelf: progress {id} names an account that isn't one; not sending it"),
                },
                Err(e) => tracing::warn!("shelf: progress {id} can't be read: {e}"),
            }
        }
        Ok(out)
    }

    /// All listening for one local session through a durable log id, including
    /// already acknowledged reports. ABS upserts totals, so a later batch must
    /// carry the cumulative time rather than just its new reports.
    pub fn local_session_progress(&self, session: &str, through: ProgressId) -> Result<Vec<ProgressUpdate>> {
        let rt = self.db.begin_read()?;
        let mut updates = Vec::new();
        for row in rt.open_table(PROGRESS)?.range(..=through)? {
            let (_, bytes) = row?;
            let row: ProgressEntryV2 = decode(bytes.value())?;
            if row.local.as_ref().is_some_and(|l| l.id == session)
                && let Some(update) = row.entry.update()
            {
                updates.push(update);
            }
        }
        Ok(updates)
    }

    /// Marks entries as sent. The log keeps them; only the flag and the
    /// unsent index change.
    pub fn mark_synced(&self, ids: &[ProgressId]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let wt = self.db.begin_write()?;
        {
            let mut log = wt.open_table(PROGRESS)?;
            let mut unsynced = wt.open_table(UNSYNCED)?;
            for &id in ids {
                let row = match log.get(id)? {
                    Some(held) => match decode::<ProgressEntryV2>(held.value()) {
                        Ok(row) => Some(row),
                        Err(e) => {
                            tracing::warn!("shelf: progress {id} can't be read: {e}");
                            None
                        }
                    },
                    None => None,
                };
                if let Some(row) = row
                    && !row.entry.synced
                {
                    log.insert(id, encode(&ProgressEntryV2 { entry: ProgressEntryV1 { synced: true, ..row.entry }, ..row })?.as_slice())?;
                }
                unsynced.remove(id)?;
            }
        }
        wt.commit()?;
        Ok(())
    }

    /// Where an item was left: the newest logged position, or `None` if it was
    /// never played here.
    pub fn position(&self, key: &ItemKey) -> Result<Option<Progress>> {
        let account = key.account.to_string();
        let rt = self.db.begin_read()?;
        Ok(get_row::<PositionV1>(&rt.open_table(POSITIONS)?, &account, &key.item)?.map(|row| row.progress(key)))
    }

    /// An item's bookmarks, in the order they fall in the book.
    pub fn bookmarks(&self, key: &ItemKey) -> Result<Vec<Bookmark>> {
        let account = key.account.to_string();
        let rt = self.db.begin_read()?;
        let mut out = Vec::new();
        for entry in rt.open_table(BOOKMARKS)?.range((account.as_str(), key.item.as_str(), 0)..)? {
            let (key_of_row, bytes) = entry?;
            let (found_account, found_item, id) = key_of_row.value();
            if found_account != account || found_item != key.item.as_str() {
                break;
            }
            match decode::<BookmarkV1>(bytes.value()) {
                Ok(row) => out.push(Bookmark {
                    id,
                    position: Position::from_secs(row.position),
                    note: row.note,
                    created: Stamp::from_millis(row.created_ms).system_time(),
                }),
                Err(e) => tracing::warn!("shelf: bookmark {id} on {key} can't be read: {e}"),
            }
        }
        out.sort_by(|a, b| a.position.secs().total_cmp(&b.position.secs()).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }

    /// Removes a bookmark. Returns whether it was there.
    pub fn remove_bookmark(&self, key: &ItemKey, id: BookmarkId) -> Result<bool> {
        let account = key.account.to_string();
        let wt = self.db.begin_write()?;
        let removed = wt.open_table(BOOKMARKS)?.remove((account.as_str(), key.item.as_str(), id))?.is_some();
        wt.commit()?;
        Ok(removed)
    }

    pub fn sync_state(&self, library: &str) -> Result<Option<SyncState>> {
        let rt = self.db.begin_read()?;
        let Some(held) = rt.open_table(SYNC)?.get(library)? else { return Ok(None) };
        let row: LibrarySyncV1 = decode(held.value())?;
        Ok(Some(SyncState { cursor: row.cursor, newest: row.newest_ms.map(Stamp::from_millis) }))
    }

    pub fn set_sync_state(&self, library: &str, state: &SyncState) -> Result<()> {
        let row = LibrarySyncV1 { cursor: state.cursor.clone(), newest_ms: state.newest.map(Stamp::millis) };
        let wt = self.db.begin_write()?;
        wt.open_table(SYNC)?.insert(library, encode(&row)?.as_slice())?;
        wt.commit()?;
        Ok(())
    }

    /// (items, tracks, chapters): what a sync and a removal leave behind.
    #[cfg(test)]
    fn row_counts(&self) -> Result<(u64, u64, u64)> {
        use redb::ReadableTableMetadata;
        let rt = self.db.begin_read()?;
        Ok((rt.open_table(ITEMS)?.len()?, rt.open_table(TRACKS)?.len()?, rt.open_table(CHAPTERS)?.len()?))
    }

    /// Every entry in the progress log, sent or not.
    #[cfg(test)]
    fn progress_rows(&self) -> Result<u64> {
        use redb::ReadableTableMetadata;
        Ok(self.db.begin_read()?.open_table(PROGRESS)?.len()?)
    }
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    Ok(postcard::to_stdvec(value)?)
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    Ok(postcard::from_bytes(bytes)?)
}

/// The next id from a counter. Ids only go up, so an id is never handed out
/// twice and a log's order is its ids' order.
fn next_id(counters: &mut Table<&str, u64>, name: &str) -> Result<u64> {
    let id = counters.get(name)?.map_or(1, |held| held.value().saturating_add(1));
    counters.insert(name, id)?;
    Ok(id)
}

/// The item row a write is about to replace or remove. One that can't be read
/// is logged and reads as missing: the write replaces it anyway, and failing
/// would leave it there to fail every later sync too.
fn old_item(id: &str, bytes: &[u8]) -> Option<ItemV1> {
    match decode(bytes) {
        Ok(row) => Some(row),
        Err(e) => {
            tracing::warn!("shelf: dropping item {id} that can't be read: {e}");
            None
        }
    }
}

fn get_row<T: DeserializeOwned>(table: &impl ReadableTable<(&'static str, &'static str), &'static [u8]>, account: &str, item: &str) -> Result<Option<T>> {
    table.get((account, item))?.map(|held| decode(held.value())).transpose()
}

/// [`get_row`] for bulk reads: a row that doesn't decode is logged and left
/// out, so one bad row doesn't empty a whole list. Storage errors still fail.
fn get_row_or_skip<T: DeserializeOwned>(
    table: &impl ReadableTable<(&'static str, &'static str), &'static [u8]>,
    account: &str,
    item: &str,
) -> Result<Option<T>> {
    let Some(held) = table.get((account, item))? else { return Ok(None) };
    match decode(held.value()) {
        Ok(row) => Ok(Some(row)),
        Err(e) => {
            tracing::warn!("shelf: skipping item {account}/{item} that can't be read: {e}");
            Ok(None)
        }
    }
}

/// Schema upgrades from `from` to `to`, run inside the opening write
/// transaction. Each step takes the database from `v` to `v + 1`; there is one
/// step per version bump. A version with no step is an error rather than a
/// no-op, because stamping old rows with the new version would leave them
/// unreadable or wrong for good.
fn migrate(wt: &WriteTransaction, from: u32, to: u32) -> Result<()> {
    for version in from..to {
        match version {
            // Existing bookmark bytes stay frozen. Only the journal is new.
            1 => {
                bookmarks::migrate_v1(wt)?;
            }
            2 => {
                let mut log = wt.open_table(PROGRESS)?;
                let entries: Vec<_> = log
                    .iter()?
                    .map(|row| {
                        let (id, bytes) = row?;
                        let entry = decode::<ProgressEntryV1>(bytes.value())?;
                        Ok((id.value(), encode(&ProgressEntryV2 { entry, local: None })?))
                    })
                    .collect::<Result<_>>()?;
                for (id, bytes) in entries {
                    log.insert(id, bytes.as_slice())?;
                }
            }
            _ => return Err(Error::NoMigration { from: version }),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
