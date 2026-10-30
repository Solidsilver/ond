//! Local mirror of the Navidrome library, stored in redb.
//!
//! The UI reads only from here, so browsing never waits on the network. Sync
//! writes whole pages from the server; artwork lives on disk next to the db,
//! never inside it.
//!
//! Layout: one primary table per model (id → postcard bytes) plus hand-written
//! index tables whose keys are tuples and whose values are `()`. Every write
//! updates the primary row and its index entries in the same transaction.

mod model;

pub use model::*;

use redb::{
    Database, ReadTransaction, ReadableDatabase, ReadableTable, ReadableTableMetadata, Table, TableDefinition,
    WriteTransaction,
};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Bump when a model or index layout changes, and handle the step in `migrate`.
pub const SCHEMA_VERSION: u32 = 4;

const META: TableDefinition<&str, u32> = TableDefinition::new("meta");
const ARTISTS: TableDefinition<&str, &[u8]> = TableDefinition::new("artists");
const ALBUMS: TableDefinition<&str, &[u8]> = TableDefinition::new("albums");
const TRACKS: TableDefinition<&str, &[u8]> = TableDefinition::new("tracks");
/// (sort name, artist id)
const ARTISTS_BY_NAME: TableDefinition<(&str, &str), ()> = TableDefinition::new("artists_by_name");
/// (artist, year, title, album id); strings are `fold`ed.
const ALBUMS_BY_SORT: TableDefinition<(&str, i32, &str, &str), ()> = TableDefinition::new("albums_by_sort");
/// (album id, disc, track, track id). Tuple keys compare field by field, so an
/// album id that is a prefix of another can't interleave with it.
const TRACKS_BY_ALBUM: TableDefinition<(&str, u32, u32, &str), ()> = TableDefinition::new("tracks_by_album");
/// (folded person name, album id, track id): who a track credits, in any role.
/// Keyed down to the track so removing one track leaves the album's other
/// credits in place.
const CREDITS: TableDefinition<(&str, &str, &str), ()> = TableDefinition::new("credits");
/// album id → [`Album::sync_stamp`] of the album when its tracks were last stored.
const ALBUM_STAMPS: TableDefinition<&str, u64> = TableDefinition::new("album_stamps");
/// Track id → an analysis record, opaque here (the `analysis` crate owns the format).
/// A new table rather than a schema bump: older builds simply ignore it.
const ANALYSIS: TableDefinition<&str, &[u8]> = TableDefinition::new("analysis");
/// Name → a copy of something the server lists outside the library (the
/// playlists, favorites, Recently added), kept for starting offline. Opaque
/// here; a new table rather than a schema bump, like `ANALYSIS`.
const SAVED: TableDefinition<&str, &[u8]> = TableDefinition::new("saved");

const SCHEMA_KEY: &str = "schema_version";
/// Which server account the mirror belongs to (a hash of server and user).
const ACCOUNT_KEY: &str = "account";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Boxed: redb's error is large and `Result` is returned everywhere.
    #[error(transparent)]
    Db(Box<redb::Error>),
    #[error("corrupt row: {0}")]
    Codec(#[from] postcard::Error),
    #[error("The library database comes from a newer version of Ond (schema v{found}; this build reads v{SCHEMA_VERSION}). Update Ond, or delete the file to sync again")]
    NewerSchema { found: u32 },
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
from_redb!(
    redb::Error,
    redb::DatabaseError,
    redb::TransactionError,
    redb::TableError,
    redb::StorageError,
    redb::CommitError
);

impl Error {
    /// The file is unreadable as a database of ours (old redb format, corrupt
    /// pages, garbage), as opposed to locked, newer, or an I/O failure.
    pub fn is_damage(&self) -> bool {
        match self {
            Error::Db(e) => match e.as_ref() {
                redb::Error::UpgradeRequired(_) | redb::Error::Corrupted(_) => true,
                redb::Error::Io(io) => matches!(io.kind(), std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof),
                _ => false,
            },
            Error::Codec(_) => true,
            Error::NewerSchema { .. } => false,
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

pub struct Library {
    db: Database,
}

/// Analysis results (track id → an opaque record), in a database of their
/// own: the library database mirrors the server and may be thrown away and
/// rebuilt at any time, while these took hours of decoding to compute.
pub struct AnalysisStore {
    db: Database,
}

impl AnalysisStore {
    /// Opens (or creates) the store at `path`. An unreadable file is moved
    /// aside (`<name>.bad-<unix secs>`) and a fresh store started.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let open = |p: &Path| Self::init(Database::create(p)?);
        match open(path) {
            Err(e) if e.is_damage() => {
                set_aside(path)?;
                open(path)
            }
            other => other,
        }
    }

    pub fn in_memory() -> Result<Self> {
        Self::init(Database::builder().create_with_backend(redb::backends::InMemoryBackend::new())?)
    }

    fn init(db: Database) -> Result<Self> {
        let wt = db.begin_write()?;
        wt.open_table(ANALYSIS)?;
        wt.commit()?;
        Ok(Self { db })
    }

    /// Takes over records an older build kept in the library database (once:
    /// they're removed there after they're safely here). Returns how many.
    pub fn adopt(&self, library: &Library) -> Result<usize> {
        let rows = library.legacy_analysis_records()?;
        if rows.is_empty() {
            return Ok(0);
        }
        let wt = self.db.begin_write()?;
        {
            let mut t = wt.open_table(ANALYSIS)?;
            for (id, bytes) in &rows {
                // A newer result already here wins.
                if t.get(id.as_str())?.is_none() {
                    t.insert(id.as_str(), bytes.as_slice())?;
                }
            }
        }
        wt.commit()?;
        library.clear_legacy_analysis()?;
        Ok(rows.len())
    }

    /// Every stored record, by track id.
    pub fn records(&self) -> Result<Vec<(String, Vec<u8>)>> {
        analysis_rows(&self.db)
    }

    pub fn put(&self, track_id: &str, record: &[u8]) -> Result<()> {
        let wt = self.db.begin_write()?;
        wt.open_table(ANALYSIS)?.insert(track_id, record)?;
        Ok(wt.commit()?)
    }
}

impl Library {
    /// Opens (or creates) the database file at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::init(Database::create(path)?)
    }

    /// Opens the database at `path`; if it can't be read (old file format,
    /// corruption) moves it aside as `<name>.bad-<unix secs>` and starts fresh.
    /// Returns where the old file went, so the caller can log it and resync.
    ///
    /// A database from a newer build is never moved aside: that's a version
    /// mismatch, not damage, and the newer build may still want it.
    pub fn open_or_recover(path: impl AsRef<Path>) -> Result<(Self, Option<PathBuf>)> {
        let path = path.as_ref();
        match Self::open(path) {
            Ok(lib) => Ok((lib, None)),
            Err(e) if e.is_damage() => {
                let aside = set_aside(path)?;
                Ok((Self::open(path)?, Some(aside)))
            }
            Err(e) => Err(e),
        }
    }

    pub fn in_memory() -> Result<Self> {
        Self::init(Database::builder().create_with_backend(redb::backends::InMemoryBackend::new())?)
    }

    /// Creates every table (so read transactions never see a missing one) and
    /// brings the schema up to `SCHEMA_VERSION`.
    fn init(db: Database) -> Result<Self> {
        let wt = db.begin_write()?;
        {
            let mut meta = wt.open_table(META)?;
            let found = meta.get(SCHEMA_KEY)?.map(|v| v.value());
            match found {
                Some(v) if v > SCHEMA_VERSION => return Err(Error::NewerSchema { found: v }),
                Some(v) if v < SCHEMA_VERSION => migrate(&wt, v)?,
                _ => {}
            }
            meta.insert(SCHEMA_KEY, SCHEMA_VERSION)?;
            wt.open_table(ARTISTS)?;
            wt.open_table(ALBUMS)?;
            wt.open_table(TRACKS)?;
            wt.open_table(ARTISTS_BY_NAME)?;
            wt.open_table(ALBUMS_BY_SORT)?;
            wt.open_table(TRACKS_BY_ALBUM)?;
            wt.open_table(CREDITS)?;
            wt.open_table(ALBUM_STAMPS)?;
            wt.open_table(ANALYSIS)?;
            wt.open_table(SAVED)?;
        }
        wt.commit()?;
        Ok(Self { db })
    }

    #[cfg(test)]
    pub fn schema_version(&self) -> Result<u32> {
        let rt = self.db.begin_read()?;
        Ok(rt.open_table(META)?.get(SCHEMA_KEY)?.map_or(0, |v| v.value()))
    }

    /// A small counter or flag kept alongside the data (e.g. sync progress).
    pub fn meta(&self, key: &str) -> Result<Option<u32>> {
        let rt = self.db.begin_read()?;
        Ok(rt.open_table(META)?.get(key)?.map(|v| v.value()))
    }

    pub fn set_meta(&self, key: &str, value: Option<u32>) -> Result<()> {
        assert_ne!(key, SCHEMA_KEY, "the schema version is managed by `init`");
        let wt = self.db.begin_write()?;
        {
            let mut meta = wt.open_table(META)?;
            match value {
                Some(v) => meta.insert(key, v)?,
                None => meta.remove(key)?,
            };
        }
        Ok(wt.commit()?)
    }

    /// Records that the mirror belongs to `account` (a stable hash of server
    /// and user). When it held another account's library, everything that came
    /// from that server is cleared first and `true` is returned. A database
    /// from before accounts were recorded is taken to be this account's.
    pub fn claim_account(&self, account: u32) -> Result<bool> {
        let wt = self.db.begin_write()?;
        let cleared = {
            let mut meta = wt.open_table(META)?;
            let previous = meta.get(ACCOUNT_KEY)?.map(|v| v.value());
            let other = previous.is_some_and(|p| p != account);
            if other {
                wt.open_table(ARTISTS)?.retain(|_, _| false)?;
                wt.open_table(ALBUMS)?.retain(|_, _| false)?;
                wt.open_table(TRACKS)?.retain(|_, _| false)?;
                wt.open_table(ARTISTS_BY_NAME)?.retain(|_, _| false)?;
                wt.open_table(ALBUMS_BY_SORT)?.retain(|_, _| false)?;
                wt.open_table(TRACKS_BY_ALBUM)?.retain(|_, _| false)?;
                wt.open_table(CREDITS)?.retain(|_, _| false)?;
                wt.open_table(ALBUM_STAMPS)?.retain(|_, _| false)?;
                wt.open_table(ANALYSIS)?.retain(|_, _| false)?;
                wt.open_table(SAVED)?.retain(|_, _| false)?;
                // Sync progress and similar flags were about the old library.
                meta.retain(|k, _| k == SCHEMA_KEY)?;
            }
            if previous != Some(account) {
                meta.insert(ACCOUNT_KEY, account)?;
            }
            other
        };
        wt.commit()?;
        Ok(cleared)
    }

    /// Replaces the whole artist list with what the server returned.
    pub fn replace_artists(&self, artists: impl IntoIterator<Item = subsonic::Artist>) -> Result<()> {
        let wt = self.db.begin_write()?;
        {
            let mut rows = wt.open_table(ARTISTS)?;
            let mut idx = wt.open_table(ARTISTS_BY_NAME)?;
            rows.retain(|_, _| false)?;
            idx.retain(|_, _| false)?;
            for a in artists {
                let a = Artist::from(a);
                rows.insert(a.id.as_str(), encode(&a)?.as_slice())?;
                idx.insert((a.sort_name().as_str(), a.id.as_str()), ())?;
            }
        }
        Ok(wt.commit()?)
    }

    #[cfg(test)]
    pub fn upsert_artists(&self, artists: impl IntoIterator<Item = subsonic::Artist>) -> Result<()> {
        let wt = self.db.begin_write()?;
        {
            let mut rows = wt.open_table(ARTISTS)?;
            let mut idx = wt.open_table(ARTISTS_BY_NAME)?;
            for a in artists {
                let a = Artist::from(a);
                if let Some(old) = rows.insert(a.id.as_str(), encode(&a)?.as_slice())? {
                    let old: Artist = decode(old.value())?;
                    idx.remove((old.sort_name().as_str(), old.id.as_str()))?;
                }
                idx.insert((a.sort_name().as_str(), a.id.as_str()), ())?;
            }
        }
        Ok(wt.commit()?)
    }

    pub fn upsert_albums(&self, albums: impl IntoIterator<Item = subsonic::Album>) -> Result<()> {
        let wt = self.db.begin_write()?;
        {
            let mut rows = wt.open_table(ALBUMS)?;
            let mut idx = wt.open_table(ALBUMS_BY_SORT)?;
            for a in albums {
                put_album(&mut rows, &mut idx, &Album::from(a))?;
            }
        }
        Ok(wt.commit()?)
    }

    /// Replaces an album's track list with what the server returned, and
    /// stamps it as current.
    pub fn upsert_album_tracks(&self, album: subsonic::AlbumWithSongs) -> Result<()> {
        self.upsert_albums_tracks(vec![album])
    }

    /// [`upsert_album_tracks`](Self::upsert_album_tracks) for several albums in
    /// one transaction.
    pub fn upsert_albums_tracks(&self, albums: Vec<subsonic::AlbumWithSongs>) -> Result<()> {
        let wt = self.db.begin_write()?;
        {
            let mut tt = TrackTables::open(&wt)?;
            let mut rows = wt.open_table(ALBUMS)?;
            let mut idx = wt.open_table(ALBUMS_BY_SORT)?;
            let mut stamps = wt.open_table(ALBUM_STAMPS)?;
            for album in albums {
                let album_id = album.album.id.clone();
                tt.remove_album(&album_id)?;
                for s in album.song {
                    tt.put(&Track::from_song(s, &album_id))?;
                }
                let row = Album::from(album.album);
                stamps.insert(album_id.as_str(), row.sync_stamp())?;
                put_album(&mut rows, &mut idx, &row)?;
            }
        }
        Ok(wt.commit()?)
    }

    /// Stores songs as they come, e.g. pages of a bulk `search3` export.
    /// Unlike [`upsert_album_tracks`](Self::upsert_album_tracks) this never
    /// removes anything, and stamps nothing: a page holds parts of albums.
    pub fn store_songs(&self, songs: Vec<subsonic::Song>) -> Result<()> {
        let wt = self.db.begin_write()?;
        {
            let mut tt = TrackTables::open(&wt)?;
            for s in songs {
                let Some(album_id) = s.album_id.clone() else { continue };
                tt.put(&Track::from_song(s, &album_id))?;
            }
        }
        Ok(wt.commit()?)
    }

    /// Analysis records stored here by older builds; they now live in an
    /// [`AnalysisStore`], which takes them over with [`AnalysisStore::adopt`].
    fn legacy_analysis_records(&self) -> Result<Vec<(String, Vec<u8>)>> {
        analysis_rows(&self.db)
    }

    fn clear_legacy_analysis(&self) -> Result<()> {
        let wt = self.db.begin_write()?;
        wt.open_table(ANALYSIS)?.retain(|_, _| false)?;
        Ok(wt.commit()?)
    }

    /// Keeps a copy of `value` under `name` (see [`saved`](Self::saved)).
    pub fn save<T: Serialize>(&self, name: &str, value: &T) -> Result<()> {
        let bytes = encode(value)?;
        let wt = self.db.begin_write()?;
        wt.open_table(SAVED)?.insert(name, bytes.as_slice())?;
        Ok(wt.commit()?)
    }

    /// The copy saved under `name`. One written by a build whose types
    /// differed reads as missing: it's a copy, and the server has the original.
    pub fn saved<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>> {
        let rt = self.db.begin_read()?;
        let Some(row) = rt.open_table(SAVED)?.get(name)? else { return Ok(None) };
        match decode(row.value()) {
            Ok(v) => Ok(Some(v)),
            Err(e) => {
                tracing::debug!("saved {name}: {e}");
                Ok(None)
            }
        }
    }

    /// Stamps of albums whose tracks are stored and current as of that stamp.
    pub fn album_stamps(&self) -> Result<HashMap<String, u64>> {
        let rt = self.db.begin_read()?;
        let mut out = HashMap::new();
        for entry in rt.open_table(ALBUM_STAMPS)?.iter()? {
            let (id, stamp) = entry?;
            out.insert(id.value().to_owned(), stamp.value());
        }
        Ok(out)
    }

    pub fn set_album_stamps(&self, stamps: impl IntoIterator<Item = (String, u64)>) -> Result<()> {
        let wt = self.db.begin_write()?;
        {
            let mut table = wt.open_table(ALBUM_STAMPS)?;
            for (id, stamp) in stamps {
                table.insert(id.as_str(), stamp)?;
            }
        }
        Ok(wt.commit()?)
    }

    /// Stored tracks per album id.
    pub fn track_counts(&self) -> Result<HashMap<String, u32>> {
        let rt = self.db.begin_read()?;
        let mut out = HashMap::new();
        for entry in rt.open_table(TRACKS_BY_ALBUM)?.iter()? {
            let (key, _) = entry?;
            *out.entry(key.value().0.to_owned()).or_default() += 1;
        }
        Ok(out)
    }

    /// Number of stored tracks. Public for the app's sync tests.
    pub fn track_count(&self) -> Result<u64> {
        Ok(self.db.begin_read()?.open_table(TRACKS)?.len()?)
    }

    /// Every stored track, grouped by album in disc/track order.
    pub fn all_tracks(&self) -> Result<Vec<Track>> {
        let rt = self.db.begin_read()?;
        let rows = rt.open_table(TRACKS)?;
        let idx = rt.open_table(TRACKS_BY_ALBUM)?;
        let mut out = Vec::with_capacity(idx.len()? as usize);
        for entry in idx.iter()? {
            let (key, _) = entry?;
            out.extend(get_row_or_skip(&rows, key.value().3)?);
        }
        Ok(out)
    }

    /// Removes every album not in `keep` (gone from the server), with its
    /// tracks. Returns how many were removed.
    pub fn retain_albums(&self, keep: &HashSet<String>) -> Result<usize> {
        // One write transaction from scan to delete, so an album stored in
        // between can't be judged by a stale list.
        let wt = self.db.begin_write()?;
        let removed = {
            let mut t = AlbumTables::open(&wt)?;
            let mut gone = Vec::new();
            for entry in t.rows.iter()? {
                let (id, _) = entry?;
                if !keep.contains(id.value()) {
                    gone.push(id.value().to_owned());
                }
            }
            for id in &gone {
                t.remove(id)?;
            }
            // Tracks stored by a bulk sync for an album the library never had.
            let mut orphans: Vec<String> = Vec::new();
            for entry in t.tracks.by_album.iter()? {
                let (key, _) = entry?;
                let album = key.value().0;
                if orphans.last().map(String::as_str) != Some(album) && t.rows.get(album)?.is_none() {
                    orphans.push(album.to_owned());
                }
            }
            for id in &orphans {
                t.remove(id)?;
            }
            gone.len()
        };
        wt.commit()?;
        Ok(removed)
    }

    /// Deletes an album, its tracks and all their index entries. Returns whether
    /// the album existed.
    pub fn remove_album(&self, id: &str) -> Result<bool> {
        let wt = self.db.begin_write()?;
        let existed = AlbumTables::open(&wt)?.remove(id)?;
        wt.commit()?;
        Ok(existed)
    }

    /// All albums ordered by artist, then year, then title.
    pub fn albums(&self) -> Result<Vec<Album>> {
        let rt = self.db.begin_read()?;
        let rows = rt.open_table(ALBUMS)?;
        let idx = rt.open_table(ALBUMS_BY_SORT)?;
        let mut out = Vec::with_capacity(idx.len()? as usize);
        for entry in idx.iter()? {
            let (key, _) = entry?;
            out.extend(get_row_or_skip(&rows, key.value().3)?);
        }
        Ok(out)
    }

    pub fn album(&self, id: &str) -> Result<Option<Album>> {
        let rt = self.db.begin_read()?;
        get_row(&rt.open_table(ALBUMS)?, id)
    }

    /// An album's tracks in disc/track order. Empty until the album is synced.
    pub fn album_tracks(&self, album_id: &str) -> Result<Vec<Track>> {
        let rt = self.db.begin_read()?;
        let rows = rt.open_table(TRACKS)?;
        track_ids(&rt, album_id)?.iter().filter_map(|id| get_row_or_skip(&rows, id).transpose()).collect()
    }

    pub fn artists(&self) -> Result<Vec<Artist>> {
        let rt = self.db.begin_read()?;
        let rows = rt.open_table(ARTISTS)?;
        let idx = rt.open_table(ARTISTS_BY_NAME)?;
        let mut out = Vec::with_capacity(idx.len()? as usize);
        for entry in idx.iter()? {
            let (key, _) = entry?;
            out.extend(get_row_or_skip(&rows, key.value().1)?);
        }
        Ok(out)
    }

    /// Albums with a stored track crediting `name` in any role (artist,
    /// composer, performer, ...), case- and accent-insensitively.
    pub fn albums_crediting(&self, name: &str) -> Result<Vec<String>> {
        let rt = self.db.begin_read()?;
        let credits = rt.open_table(CREDITS)?;
        let who = fold(name);
        let mut ids = std::collections::BTreeSet::new();
        for entry in credits.range((who.as_str(), "", "")..)? {
            let (key, _) = entry?;
            let (person, album, _) = key.value();
            if person != who {
                break;
            }
            ids.insert(album.to_owned());
        }
        Ok(ids.into_iter().collect())
    }

    #[cfg(test)]
    pub fn album_count(&self) -> Result<u64> {
        Ok(self.db.begin_read()?.open_table(ALBUMS)?.len()?)
    }

    /// Checks that every index entry points at a row and every row has exactly
    /// its expected index entry. Public for the app's sync tests.
    pub fn check_indexes(&self) -> Result<Vec<String>> {
        let rt = self.db.begin_read()?;
        let mut problems = Vec::new();

        let albums = rt.open_table(ALBUMS)?;
        let album_idx = rt.open_table(ALBUMS_BY_SORT)?;
        for entry in albums.iter()? {
            let (id, bytes) = entry?;
            let a: Album = decode(bytes.value())?;
            let (artist, year, title) = a.sort_parts();
            if album_idx.get((artist.as_str(), year, title.as_str(), id.value()))?.is_none() {
                problems.push(format!("album {} missing from index", id.value()));
            }
        }
        if album_idx.len()? != albums.len()? {
            problems.push(format!("album index has {} entries for {} rows", album_idx.len()?, albums.len()?));
        }

        let tracks = rt.open_table(TRACKS)?;
        let track_idx = rt.open_table(TRACKS_BY_ALBUM)?;
        for entry in tracks.iter()? {
            let (id, bytes) = entry?;
            let t: Track = decode(bytes.value())?;
            let (disc, no) = t.order_parts();
            if track_idx.get((t.album_id.as_str(), disc, no, id.value()))?.is_none() {
                problems.push(format!("track {} missing from index", id.value()));
            }
        }
        if track_idx.len()? != tracks.len()? {
            problems.push(format!("track index has {} entries for {} rows", track_idx.len()?, tracks.len()?));
        }

        let credits = rt.open_table(CREDITS)?;
        let mut expected = 0u64;
        for entry in tracks.iter()? {
            let (id, bytes) = entry?;
            let t: Track = decode(bytes.value())?;
            for name in t.credited_names() {
                expected += 1;
                if credits.get((name.as_str(), t.album_id.as_str(), id.value()))?.is_none() {
                    problems.push(format!("credit {name} on track {} missing", id.value()));
                }
            }
        }
        if credits.len()? != expected {
            problems.push(format!("credit index has {} entries, expected {expected}", credits.len()?));
        }
        Ok(problems)
    }
}

/// Schema upgrades, run inside the opening write transaction. Each step takes
/// the db from `from` to `from + 1`.
/// Moves an unreadable database file out of the way as `<name>.bad-<unix secs>`.
fn set_aside(path: &Path) -> Result<PathBuf> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut aside = path.as_os_str().to_owned();
    aside.push(format!(".bad-{stamp}"));
    let aside = PathBuf::from(aside);
    std::fs::rename(path, &aside).map_err(|io| Error::Db(Box::new(redb::StorageError::Io(io).into())))?;
    Ok(aside)
}

/// Every row of the analysis table in `db` (either database keeps one).
fn analysis_rows(db: &Database) -> Result<Vec<(String, Vec<u8>)>> {
    let rt = db.begin_read()?;
    let mut out = Vec::new();
    for entry in rt.open_table(ANALYSIS)?.iter()? {
        let (id, bytes) = entry?;
        out.push((id.value().to_owned(), bytes.value().to_vec()));
    }
    Ok(out)
}

fn migrate(wt: &WriteTransaction, from: u32) -> Result<()> {
    for v in from..SCHEMA_VERSION {
        match v {
            // 0 => only possible for a db created before the meta table existed; nothing to do.
            0 => {}
            // v2 adds labels/genres to albums and credits/composer/cover to tracks.
            // Index keys don't change, so rows are rewritten in place; the new
            // fields fill in the next time an album is synced.
            1 => {
                rewrite_rows::<AlbumV1, AlbumV2>(&mut wt.open_table(ALBUMS)?)?;
                rewrite_rows::<TrackV1, TrackV2>(&mut wt.open_table(TRACKS)?)?;
            }
            // v3 adds artist entities to albums and tracks, filled in by the
            // next sync (which refetches an album's tracks only when its stamp
            // is missing, so stamps go), and the credits index (built in step 3).
            2 => {
                rewrite_rows::<AlbumV2, AlbumV3>(&mut wt.open_table(ALBUMS)?)?;
                rewrite_rows::<TrackV2, TrackV3>(&mut wt.open_table(TRACKS)?)?;
                wt.open_table(ALBUM_STAMPS)?.retain(|_, _| false)?;
            }
            // v4 folds more letters (ø, ł, ß, æ, ...) in sort and credit keys.
            // Rows don't change, so stamps stay; every key made by `fold` is
            // rebuilt from them.
            3 => rebuild_folded_indexes(wt)?,
            _ => {}
        }
    }
    Ok(())
}

/// Rebuilds every index whose keys `fold` makes (artists by name, albums by
/// sort, credits) from the rows.
fn rebuild_folded_indexes(wt: &WriteTransaction) -> Result<()> {
    let mut artist_idx = wt.open_table(ARTISTS_BY_NAME)?;
    artist_idx.retain(|_, _| false)?;
    for entry in wt.open_table(ARTISTS)?.iter()? {
        let (id, bytes) = entry?;
        let a: Artist = decode(bytes.value())?;
        artist_idx.insert((a.sort_name().as_str(), id.value()), ())?;
    }
    let mut album_idx = wt.open_table(ALBUMS_BY_SORT)?;
    album_idx.retain(|_, _| false)?;
    for entry in wt.open_table(ALBUMS)?.iter()? {
        let (id, bytes) = entry?;
        let a: Album = decode(bytes.value())?;
        let (artist, year, title) = a.sort_parts();
        album_idx.insert((artist.as_str(), year, title.as_str(), id.value()), ())?;
    }
    let mut credits = wt.open_table(CREDITS)?;
    credits.retain(|_, _| false)?;
    for entry in wt.open_table(TRACKS)?.iter()? {
        let (id, bytes) = entry?;
        let t: Track = decode(bytes.value())?;
        for name in t.credited_names() {
            credits.insert((name.as_str(), t.album_id.as_str(), id.value()), ())?;
        }
    }
    Ok(())
}

fn rewrite_rows<Old: DeserializeOwned, New: Serialize + From<Old>>(table: &mut Table<&'static str, &'static [u8]>) -> Result<()> {
    let mut rows = Vec::new();
    for entry in table.iter()? {
        let (id, bytes) = entry?;
        rows.push((id.value().to_owned(), New::from(decode::<Old>(bytes.value())?)));
    }
    for (id, row) in rows {
        table.insert(id.as_str(), encode(&row)?.as_slice())?;
    }
    Ok(())
}

fn get_row<T: DeserializeOwned>(table: &impl ReadableTable<&'static str, &'static [u8]>, id: &str) -> Result<Option<T>> {
    table.get(id)?.map(|g| decode(g.value())).transpose()
}

/// [`get_row`] for bulk reads: a row that doesn't decode is logged and left
/// out, so one bad row doesn't empty a whole list. Storage errors still fail.
fn get_row_or_skip<T: DeserializeOwned>(table: &impl ReadableTable<&'static str, &'static [u8]>, id: &str) -> Result<Option<T>> {
    let Some(bytes) = table.get(id)? else { return Ok(None) };
    match decode(bytes.value()) {
        Ok(row) => Ok(Some(row)),
        Err(e) => {
            tracing::warn!("library: skipping row {id} that can't be read: {e}");
            Ok(None)
        }
    }
}

fn track_ids(rt: &ReadTransaction, album_id: &str) -> Result<Vec<String>> {
    collect_track_ids(&rt.open_table(TRACKS_BY_ALBUM)?, album_id)
}

type TrackIndex<'t> = Table<'t, (&'static str, u32, u32, &'static str), ()>;

fn collect_track_ids(
    idx: &impl ReadableTable<(&'static str, u32, u32, &'static str), ()>,
    album_id: &str,
) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    for entry in idx.range((album_id, 0, 0, "")..)? {
        let (key, _) = entry?;
        let (album, _, _, id) = key.value();
        if album != album_id {
            break;
        }
        ids.push(id.to_owned());
    }
    Ok(ids)
}

fn put_album(
    rows: &mut Table<&'static str, &'static [u8]>,
    idx: &mut Table<(&'static str, i32, &'static str, &'static str), ()>,
    a: &Album,
) -> Result<()> {
    if let Some(old) = rows.insert(a.id.as_str(), encode(a)?.as_slice())? {
        let old: Album = decode(old.value())?;
        let (artist, year, title) = old.sort_parts();
        idx.remove((artist.as_str(), year, title.as_str(), old.id.as_str()))?;
    }
    let (artist, year, title) = a.sort_parts();
    idx.insert((artist.as_str(), year, title.as_str(), a.id.as_str()), ())?;
    Ok(())
}

/// The tables removing an album touches, opened once per transaction.
struct AlbumTables<'t> {
    rows: Table<'t, &'static str, &'static [u8]>,
    idx: Table<'t, (&'static str, i32, &'static str, &'static str), ()>,
    stamps: Table<'t, &'static str, u64>,
    tracks: TrackTables<'t>,
}

impl<'t> AlbumTables<'t> {
    fn open(wt: &'t WriteTransaction) -> Result<Self> {
        Ok(Self {
            rows: wt.open_table(ALBUMS)?,
            idx: wt.open_table(ALBUMS_BY_SORT)?,
            stamps: wt.open_table(ALBUM_STAMPS)?,
            tracks: TrackTables::open(wt)?,
        })
    }

    /// Removes an album, its tracks, its stamp and their index entries.
    /// Returns whether the album row existed.
    fn remove(&mut self, id: &str) -> Result<bool> {
        self.tracks.remove_album(id)?;
        self.stamps.remove(id)?;
        let old = self.rows.remove(id)?.map(|g| decode::<Album>(g.value())).transpose()?;
        if let Some(old) = &old {
            let (artist, year, title) = old.sort_parts();
            self.idx.remove((artist.as_str(), year, title.as_str(), old.id.as_str()))?;
        }
        Ok(old.is_some())
    }
}

/// The tables a track write touches, opened once per transaction.
struct TrackTables<'t> {
    rows: Table<'t, &'static str, &'static [u8]>,
    by_album: TrackIndex<'t>,
    credits: Table<'t, (&'static str, &'static str, &'static str), ()>,
}

impl<'t> TrackTables<'t> {
    fn open(wt: &'t WriteTransaction) -> Result<Self> {
        Ok(Self { rows: wt.open_table(TRACKS)?, by_album: wt.open_table(TRACKS_BY_ALBUM)?, credits: wt.open_table(CREDITS)? })
    }

    fn put(&mut self, t: &Track) -> Result<()> {
        // A track the server moved to another album leaves its old index entries
        // behind unless we drop them here.
        let old = self.rows.insert(t.id.as_str(), encode(t)?.as_slice())?.map(|g| decode::<Track>(g.value())).transpose()?;
        if let Some(old) = old {
            self.unindex(&old)?;
        }
        let (disc, no) = t.order_parts();
        self.by_album.insert((t.album_id.as_str(), disc, no, t.id.as_str()), ())?;
        for name in t.credited_names() {
            self.credits.insert((name.as_str(), t.album_id.as_str(), t.id.as_str()), ())?;
        }
        Ok(())
    }

    fn unindex(&mut self, old: &Track) -> Result<()> {
        let (disc, no) = old.order_parts();
        self.by_album.remove((old.album_id.as_str(), disc, no, old.id.as_str()))?;
        for name in old.credited_names() {
            self.credits.remove((name.as_str(), old.album_id.as_str(), old.id.as_str()))?;
        }
        Ok(())
    }

    fn remove_album(&mut self, album_id: &str) -> Result<()> {
        for id in collect_track_ids(&self.by_album, album_id)? {
            let old = self.rows.remove(id.as_str())?.map(|g| decode::<Track>(g.value())).transpose()?;
            if let Some(old) = old {
                self.unindex(&old)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
