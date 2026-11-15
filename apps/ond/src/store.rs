//! Ond's own persistent store (`<data>/ond.redb`), next to `Sources/`.
//!
//! Holds user state that does not belong solely to one remote server: stars
//! and ratings for sources whose own capabilities lack them, and Ond-managed
//! playlists (which may later span sources). Records use `redb` with frozen
//! versioned postcard structs, an explicit `schema_version` in `META`, and
//! move-aside recovery on file corruption.

mod model;
#[cfg(test)]
mod tests;

use crate::source::StarKind;
use accounts::{AccountId, ItemKey};
use model::{ApartV1, PlaylistEntryV1, PlaylistV1, RatingV1, StarredV1};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition, WriteTransaction};
use scan::TrackMove;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Bump when a row layout or table changes, and handle the step in `migrate`.
pub(crate) const SCHEMA_VERSION: u32 = 2;

const META: TableDefinition<&str, u32> = TableDefinition::new("meta");
/// `(account_uuid, kind_tag, item_id)` -> `StarredV1` postcard bytes.
const STARS: TableDefinition<(&str, u8, &str), &[u8]> = TableDefinition::new("stars");
/// `(account_uuid, track_id)` -> `RatingV1` postcard bytes.
const RATINGS: TableDefinition<(&str, &str), &[u8]> = TableDefinition::new("ratings");
/// `playlist_id` -> `PlaylistV1` postcard bytes.
const PLAYLISTS: TableDefinition<&str, &[u8]> = TableDefinition::new("playlists");
/// Ordered canonical album ItemKeys -> `ApartV1` postcard bytes.
const APART: TableDefinition<(&str, &str), &[u8]> = TableDefinition::new("album_apart_v1");

pub type ApartPairs = std::collections::BTreeSet<(ItemKey, ItemKey)>;

pub fn album_pair(a: ItemKey, b: ItemKey) -> (ItemKey, ItemKey) {
    if a < b { (a, b) } else { (b, a) }
}

/// A change to which album copies are kept apart.
#[derive(Debug, Clone)]
pub enum Separation {
    /// Keep each pair apart.
    Keep(ApartPairs),
    /// Forget exactly these pairs: the undo of a [`Keep`](Self::Keep),
    /// which leaves the album's other choices standing.
    Forget(ApartPairs),
    /// Forget every pair involving this album.
    CombineAgain(ItemKey),
}

const SCHEMA_KEY: &str = "schema_version";

const KIND_SONG: u8 = 1;
const KIND_ALBUM: u8 = 2;
const KIND_ARTIST: u8 = 3;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Db(Box<redb::Error>),
    #[error("corrupt store row: {0}")]
    Codec(#[from] postcard::Error),
    #[error(
        "The Ond database comes from a newer version of Ond (schema v{found}; this build reads v{SCHEMA_VERSION}). Update Ond, or delete ond.redb to start fresh"
    )]
    NewerSchema { found: u32 },
    #[error("This build of Ond can't upgrade ond.redb from schema v{from}. Update Ond, or delete ond.redb to start fresh")]
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

impl Error {
    pub fn is_damage(&self) -> bool {
        match self {
            Error::Db(e) => match e.as_ref() {
                redb::Error::UpgradeRequired(_) | redb::Error::Corrupted(_) => true,
                redb::Error::Io(io) => matches!(io.kind(), std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof),
                _ => false,
            },
            Error::Codec(_) => true,
            Error::NewerSchema { .. } | Error::NoMigration { .. } => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

fn encode<T: Serialize>(v: &T) -> Result<Vec<u8>> {
    Ok(postcard::to_stdvec(v)?)
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    Ok(postcard::from_bytes(bytes)?)
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64
}

fn kind_tag(kind: StarKind) -> u8 {
    match kind {
        StarKind::Song => KIND_SONG,
        StarKind::Album => KIND_ALBUM,
        StarKind::Artist => KIND_ARTIST,
    }
}

/// Favourites stored in [`OndStore`] for one account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountStarred {
    pub songs: Vec<String>,
    pub albums: Vec<String>,
    pub artists: Vec<(String, Option<String>)>,
}

/// An Ond-managed playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OndPlaylist {
    pub id: String,
    pub name: String,
    pub entries: Vec<ItemKey>,
    pub created_at: i64,
    pub changed_at: i64,
}

/// One resolved track line used when exporting a playlist to M3U.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct M3uTrack {
    pub location: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub duration_secs: Option<u32>,
}

/// Ond's cross-source persistent database (`<data>/ond.redb`).
pub struct OndStore {
    db: Database,
}

impl OndStore {
    /// Opens or creates the store at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::init(Database::create(path)?)
    }

    /// Opens or creates the store at `path`; moves unreadable/corrupt files
    /// aside as `<name>.bad-<unix secs>` and starts fresh, returning the moved path.
    pub fn open_or_recover(path: impl AsRef<Path>) -> Result<(Self, Option<PathBuf>)> {
        let path = path.as_ref();
        match Self::open(path) {
            Ok(store) => Ok((store, None)),
            Err(e) if e.is_damage() => {
                let aside = set_aside(path)?;
                tracing::warn!("Ond store was unreadable; moved to {} and starting fresh", aside.display());
                Ok((Self::open(path)?, Some(aside)))
            }
            Err(e) => Err(e),
        }
    }

    /// In-memory store for tests or fallback startup modes.
    pub fn in_memory() -> Result<Self> {
        Self::init(Database::builder().create_with_backend(redb::backends::InMemoryBackend::new())?)
    }

    fn init(db: Database) -> Result<Self> {
        let wt = db.begin_write()?;
        {
            let mut meta = wt.open_table(META)?;
            let found = meta.get(SCHEMA_KEY)?.map(|v| v.value());
            match found {
                Some(v) if v > SCHEMA_VERSION => return Err(Error::NewerSchema { found: v }),
                Some(v) if v < SCHEMA_VERSION => migrate(&wt, v, SCHEMA_VERSION)?,
                Some(_) | None => {}
            }
            meta.insert(SCHEMA_KEY, SCHEMA_VERSION)?;
            wt.open_table(STARS)?;
            wt.open_table(RATINGS)?;
            wt.open_table(PLAYLISTS)?;
            wt.open_table(APART)?;
        }
        wt.commit()?;
        Ok(Self { db })
    }

    #[cfg(test)]
    pub fn schema_version(&self) -> Result<u32> {
        let rt = self.db.begin_read()?;
        Ok(rt.open_table(META)?.get(SCHEMA_KEY)?.map_or(0, |v| v.value()))
    }

    pub fn apart_pairs(&self) -> Result<ApartPairs> {
        let rt = self.db.begin_read()?;
        let mut pairs = ApartPairs::new();
        for row in rt.open_table(APART)?.iter()? {
            let (keys, bytes) = row?;
            let _: ApartV1 = decode(bytes.value())?;
            let (a, b) = keys.value();
            if let (Ok(a), Ok(b)) = (ItemKey::parse(a), ItemKey::parse(b)) {
                pairs.insert(album_pair(a, b));
            }
        }
        Ok(pairs)
    }

    /// Keeps only explicitly requested pairs apart, in one transaction.
    pub fn keep_apart(&self, pairs: &ApartPairs) -> Result<()> {
        let wt = self.db.begin_write()?;
        {
            let mut table = wt.open_table(APART)?;
            let bytes = encode(&ApartV1 { kept_at: now_secs() })?;
            for (a, b) in pairs {
                if a.account != b.account {
                    let (a, b) = album_pair(a.clone(), b.clone());
                    table.insert((a.to_string().as_str(), b.to_string().as_str()), bytes.as_slice())?;
                }
            }
        }
        Ok(wt.commit()?)
    }

    /// Forgets exactly `pairs`, whichever way round each was given.
    pub fn forget_apart(&self, pairs: &ApartPairs) -> Result<()> {
        let wt = self.db.begin_write()?;
        {
            let mut table = wt.open_table(APART)?;
            for (a, b) in pairs {
                let (a, b) = album_pair(a.clone(), b.clone());
                table.remove((a.to_string().as_str(), b.to_string().as_str()))?;
            }
        }
        Ok(wt.commit()?)
    }

    /// Either album can undo its exclusions, including absent copies.
    pub fn combine_again(&self, album: &ItemKey) -> Result<()> {
        let wt = self.db.begin_write()?;
        {
            let spelling = album.to_string();
            wt.open_table(APART)?.retain(|(a, b), _| a != spelling && b != spelling)?;
        }
        Ok(wt.commit()?)
    }

    /// Loves or unloves `key` as `kind` (`name` is kept for artist stars).
    pub fn set_starred(&self, kind: StarKind, key: &ItemKey, on: bool, name: Option<&str>) -> Result<()> {
        let acc = key.account.to_string();
        let tag = kind_tag(kind);
        let wt = self.db.begin_write()?;
        {
            let mut stars = wt.open_table(STARS)?;
            if on {
                let row = StarredV1 { name: name.map(str::to_owned), starred_at: now_secs() };
                stars.insert((acc.as_str(), tag, key.item.as_str()), encode(&row)?.as_slice())?;
            } else {
                stars.remove((acc.as_str(), tag, key.item.as_str()))?;
            }
        }
        Ok(wt.commit()?)
    }

    /// Whether `key` is starred as `kind`.
    #[cfg(test)]
    pub fn is_starred(&self, kind: StarKind, key: &ItemKey) -> Result<bool> {
        let acc = key.account.to_string();
        let tag = kind_tag(kind);
        let rt = self.db.begin_read()?;
        Ok(rt.open_table(STARS)?.get((acc.as_str(), tag, key.item.as_str()))?.is_some())
    }

    /// All favourites stored for `account`.
    pub fn starred(&self, account: AccountId) -> Result<AccountStarred> {
        let acc = account.to_string();
        let rt = self.db.begin_read()?;
        let table = rt.open_table(STARS)?;
        let mut out = AccountStarred::default();
        for entry in table.iter()? {
            let (k, v) = entry?;
            let (row_acc, kind, item) = k.value();
            if row_acc != acc {
                continue;
            }
            match kind {
                KIND_SONG => out.songs.push(item.to_owned()),
                KIND_ALBUM => out.albums.push(item.to_owned()),
                KIND_ARTIST => {
                    let row = decode::<StarredV1>(v.value()).ok();
                    out.artists.push((item.to_owned(), row.and_then(|r| r.name)));
                }
                _ => {}
            }
        }
        Ok(out)
    }

    /// Sets or clears (`rating == 0`) a track rating (`1..=5`).
    pub fn set_rating(&self, key: &ItemKey, rating: u8) -> Result<()> {
        let acc = key.account.to_string();
        let wt = self.db.begin_write()?;
        {
            let mut ratings = wt.open_table(RATINGS)?;
            if rating == 0 {
                ratings.remove((acc.as_str(), key.item.as_str()))?;
            } else {
                let row = RatingV1 { rating: rating.min(5), rated_at: now_secs() };
                ratings.insert((acc.as_str(), key.item.as_str()), encode(&row)?.as_slice())?;
            }
        }
        Ok(wt.commit()?)
    }

    /// Stored rating (`0` when unrated) for `key`.
    #[cfg(test)]
    pub fn rating(&self, key: &ItemKey) -> Result<u8> {
        let acc = key.account.to_string();
        let rt = self.db.begin_read()?;
        let Some(bytes) = rt.open_table(RATINGS)?.get((acc.as_str(), key.item.as_str()))? else {
            return Ok(0);
        };
        Ok(decode::<RatingV1>(bytes.value())?.rating)
    }

    /// All non-zero ratings stored for `account`.
    pub fn ratings(&self, account: AccountId) -> Result<Vec<(String, u8)>> {
        let acc = account.to_string();
        let rt = self.db.begin_read()?;
        let table = rt.open_table(RATINGS)?;
        let mut out = Vec::new();
        for entry in table.iter()? {
            let (k, v) = entry?;
            let (row_acc, item) = k.value();
            if row_acc != acc {
                continue;
            }
            if let Ok(row) = decode::<RatingV1>(v.value())
                && row.rating > 0
            {
                out.push((item.to_owned(), row.rating));
            }
        }
        Ok(out)
    }

    /// All Ond playlists, ordered alphabetically by name (case-insensitive) then id.
    pub fn playlists(&self) -> Result<Vec<OndPlaylist>> {
        let rt = self.db.begin_read()?;
        let table = rt.open_table(PLAYLISTS)?;
        let mut out = Vec::new();
        for entry in table.iter()? {
            let (_, v) = entry?;
            match decode::<PlaylistV1>(v.value()) {
                Ok(raw) => out.push(playlist_from_v1(raw)),
                Err(e) => tracing::warn!("skipping unreadable playlist row in ond.redb: {e}"),
            }
        }
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }

    /// Fetches one Ond playlist by id.
    pub fn playlist(&self, id: &str) -> Result<Option<OndPlaylist>> {
        let rt = self.db.begin_read()?;
        let Some(bytes) = rt.open_table(PLAYLISTS)?.get(id)? else {
            return Ok(None);
        };
        Ok(Some(playlist_from_v1(decode::<PlaylistV1>(bytes.value())?)))
    }

    /// Creates a new Ond playlist and returns it.
    pub fn create_playlist(&self, name: &str, entries: &[ItemKey]) -> Result<OndPlaylist> {
        let id = accounts::AccountId::new().to_string();
        let now = now_secs();
        let raw = PlaylistV1 { id: id.clone(), name: name.to_owned(), entries: entries.iter().map(entry_to_v1).collect(), created_at: now, changed_at: now };
        let wt = self.db.begin_write()?;
        wt.open_table(PLAYLISTS)?.insert(id.as_str(), encode(&raw)?.as_slice())?;
        wt.commit()?;
        Ok(playlist_from_v1(raw))
    }

    /// Renames an Ond playlist. Returns `false` if no playlist with `id` exists.
    pub fn rename_playlist(&self, id: &str, name: &str) -> Result<bool> {
        self.update_playlist(id, |pl| {
            pl.name = name.to_owned();
            true
        })
    }

    /// Deletes an Ond playlist. Returns `false` if no playlist with `id` existed.
    pub fn delete_playlist(&self, id: &str) -> Result<bool> {
        let wt = self.db.begin_write()?;
        let existed = wt.open_table(PLAYLISTS)?.remove(id)?.is_some();
        wt.commit()?;
        Ok(existed)
    }

    /// Appends `entries` to an Ond playlist.
    pub fn add_to_playlist(&self, id: &str, entries: &[ItemKey]) -> Result<bool> {
        self.update_playlist(id, |pl| {
            pl.entries.extend(entries.iter().map(entry_to_v1));
            true
        })
    }

    /// Applies a view's edit only if its raw entry snapshot is still current.
    /// Missing mirror rows must never turn a stale edit into a destructive replace.
    pub fn replace_playlist_entries_if_unchanged(&self, id: &str, expected: &[ItemKey], entries: Vec<ItemKey>) -> Result<bool> {
        let mut applied = false;
        self.update_playlist(id, |pl| {
            if pl.entries.len() != expected.len()
                || pl.entries.iter().zip(expected).any(|(entry, key)| entry.account != key.account.to_string() || entry.item != key.item)
            {
                return false;
            }
            pl.entries = entries.iter().map(entry_to_v1).collect();
            applied = true;
            true
        })?;
        Ok(applied)
    }

    fn update_playlist(&self, id: &str, edit: impl FnOnce(&mut PlaylistV1) -> bool) -> Result<bool> {
        let wt = self.db.begin_write()?;
        let updated = {
            let mut table = wt.open_table(PLAYLISTS)?;
            let Some(bytes) = table.get(id)?.map(|g| g.value().to_vec()) else {
                return Ok(false);
            };
            let mut pl: PlaylistV1 = decode(&bytes)?;
            if edit(&mut pl) {
                pl.changed_at = now_secs();
                table.insert(id, encode(&pl)?.as_slice())?;
            }
            true
        };
        wt.commit()?;
        Ok(updated)
    }

    /// Migrates track stars, ratings, and playlist entries for `account` across
    /// detected file moves `(old track id -> new track id)`. Returns how many
    /// items were rewritten.
    pub fn apply_moves(&self, account: AccountId, moves: &[TrackMove]) -> Result<usize> {
        if moves.is_empty() {
            return Ok(0);
        }
        let acc = account.to_string();
        let rename: HashMap<&str, &str> = moves.iter().filter(|m| m.from != m.to).map(|m| (m.from.as_str(), m.to.as_str())).collect();
        if rename.is_empty() {
            return Ok(0);
        }

        let wt = self.db.begin_write()?;
        let mut count = 0usize;
        {
            // Snapshot and rewrite stars atomically so chains/swaps don't collide.
            let mut stars = wt.open_table(STARS)?;
            let mut moved_stars = Vec::new();
            for (&from, &to) in &rename {
                if let Some(bytes) = stars.get((acc.as_str(), KIND_SONG, from))?.map(|g| g.value().to_vec()) {
                    moved_stars.push((from, to, bytes));
                }
            }
            for (from, _, _) in &moved_stars {
                stars.remove((acc.as_str(), KIND_SONG, *from))?;
            }
            for (_, to, bytes) in moved_stars {
                stars.insert((acc.as_str(), KIND_SONG, to), bytes.as_slice())?;
                count += 1;
            }

            let mut ratings = wt.open_table(RATINGS)?;
            let mut moved_ratings = Vec::new();
            for (&from, &to) in &rename {
                if let Some(bytes) = ratings.get((acc.as_str(), from))?.map(|g| g.value().to_vec()) {
                    moved_ratings.push((from, to, bytes));
                }
            }
            for (from, _, _) in &moved_ratings {
                ratings.remove((acc.as_str(), *from))?;
            }
            for (_, to, bytes) in moved_ratings {
                ratings.insert((acc.as_str(), to), bytes.as_slice())?;
                count += 1;
            }

            let mut playlists = wt.open_table(PLAYLISTS)?;
            let mut changed_playlists = Vec::new();
            let now = now_secs();
            for entry in playlists.iter()? {
                let (id, bytes) = entry?;
                let mut pl: PlaylistV1 = decode(bytes.value())?;
                let mut touched = false;
                for item in &mut pl.entries {
                    if item.account == acc
                        && let Some(&to) = rename.get(item.item.as_str())
                    {
                        item.item = to.to_owned();
                        touched = true;
                        count += 1;
                    }
                }
                if touched {
                    pl.changed_at = now;
                    changed_playlists.push((id.value().to_owned(), pl));
                }
            }
            for (id, pl) in changed_playlists {
                playlists.insert(id.as_str(), encode(&pl)?.as_slice())?;
            }
        }
        wt.commit()?;
        Ok(count)
    }

    /// Drops every star and rating for `account` and strips its tracks from Ond playlists.
    pub fn remove_account(&self, account: AccountId) -> Result<()> {
        let acc = account.to_string();
        let wt = self.db.begin_write()?;
        {
            wt.open_table(STARS)?.retain(|(a, _, _), _| a != acc)?;
            wt.open_table(RATINGS)?.retain(|(a, _), _| a != acc)?;

            let mut playlists = wt.open_table(PLAYLISTS)?;
            let mut changed = Vec::new();
            let now = now_secs();
            for entry in playlists.iter()? {
                let (id, bytes) = entry?;
                let mut pl: PlaylistV1 = decode(bytes.value())?;
                let before = pl.entries.len();
                pl.entries.retain(|e| e.account != acc);
                if pl.entries.len() != before {
                    pl.changed_at = now;
                    changed.push((id.value().to_owned(), pl));
                }
            }
            for (id, pl) in changed {
                playlists.insert(id.as_str(), encode(&pl)?.as_slice())?;
            }
        }
        wt.commit()?;
        Ok(())
    }

    /// Formats playlist `id` as Extended M3U (`#EXTM3U`) using `resolve` to map
    /// each `ItemKey` to a file path / location and metadata. Returns `None` if
    /// no playlist with `id` exists.
    pub fn export_m3u(&self, id: &str, mut resolve: impl FnMut(&ItemKey) -> Option<M3uTrack>) -> Result<Option<String>> {
        let Some(pl) = self.playlist(id)? else {
            return Ok(None);
        };
        let resolved: Vec<M3uTrack> = pl.entries.iter().filter_map(&mut resolve).collect();
        Ok(Some(format_m3u(&resolved)))
    }
}

/// Formats resolved tracks as Extended M3U (`#EXTM3U`).
pub fn format_m3u<'a>(tracks: impl IntoIterator<Item = &'a M3uTrack>) -> String {
    let mut out = String::from("#EXTM3U\n");
    for tr in tracks {
        let loc = sanitize_line(&tr.location);
        if loc.is_empty() {
            continue;
        }
        let dur = tr.duration_secs.map(|s| s as i64).unwrap_or(-1);
        let artist = tr.artist.as_deref().map(sanitize_line).filter(|s| !s.is_empty());
        let title = tr.title.as_deref().map(sanitize_line).filter(|s| !s.is_empty());
        let label = match (artist, title) {
            (Some(a), Some(t)) => Some(format!("{a} - {t}")),
            (None, Some(t)) => Some(t),
            (Some(a), None) => Some(a),
            (None, None) => None,
        };
        if let Some(label) = label {
            out.push_str(&format!("#EXTINF:{dur},{label}\n"));
        }
        out.push_str(&loc);
        out.push('\n');
    }
    out
}

fn sanitize_line(s: &str) -> String {
    s.chars().filter(|&c| c != '\r' && c != '\n').collect()
}

fn entry_to_v1(key: &ItemKey) -> PlaylistEntryV1 {
    PlaylistEntryV1 { account: key.account.to_string(), item: key.item.clone() }
}

fn playlist_from_v1(raw: PlaylistV1) -> OndPlaylist {
    let entries = raw.entries.into_iter().filter_map(|e| e.account.parse::<AccountId>().ok().map(|acc| ItemKey::new(acc, e.item))).collect();
    OndPlaylist { id: raw.id, name: raw.name, entries, created_at: raw.created_at, changed_at: raw.changed_at }
}

fn set_aside(path: &Path) -> Result<PathBuf> {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let dest = path.with_file_name(format!("{name}.bad-{secs}"));
    if let Err(e) = std::fs::rename(path, &dest) {
        return Err(Error::Db(Box::new(e.into())));
    }
    Ok(dest)
}

fn migrate(wt: &WriteTransaction, mut from: u32, to: u32) -> Result<()> {
    while from < to {
        match from {
            1 => {
                wt.open_table(APART)?;
            }
            _ => return Err(Error::NoMigration { from }),
        }
        from += 1;
    }
    Ok(())
}
