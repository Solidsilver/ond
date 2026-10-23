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
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Bump when a model or index layout changes, and handle the step in `migrate`.
pub const SCHEMA_VERSION: u32 = 1;

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

const SCHEMA_KEY: &str = "schema_version";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Boxed: redb's error is large and `Result` is returned everywhere.
    #[error(transparent)]
    Db(Box<redb::Error>),
    #[error("corrupt row: {0}")]
    Codec(#[from] postcard::Error),
    #[error("library database was written by a newer version of Listener (schema v{found}, this build reads v{SCHEMA_VERSION}); update Listener, or delete the file to resync")]
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
                let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
                let mut aside = path.as_os_str().to_owned();
                aside.push(format!(".bad-{stamp}"));
                let aside = PathBuf::from(aside);
                std::fs::rename(path, &aside).map_err(|io| Error::Db(Box::new(redb::StorageError::Io(io).into())))?;
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
        }
        wt.commit()?;
        Ok(Self { db })
    }

    pub fn schema_version(&self) -> Result<u32> {
        let rt = self.db.begin_read()?;
        Ok(rt.open_table(META)?.get(SCHEMA_KEY)?.map_or(0, |v| v.value()))
    }

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

    /// Replaces an album's track list with what the server returned.
    pub fn upsert_album_tracks(&self, album: subsonic::AlbumWithSongs) -> Result<()> {
        let album_id = album.album.id.clone();
        let wt = self.db.begin_write()?;
        {
            let mut tracks = wt.open_table(TRACKS)?;
            let mut by_album = wt.open_table(TRACKS_BY_ALBUM)?;
            remove_album_tracks(&mut tracks, &mut by_album, &album_id)?;
            for s in album.song {
                put_track(&mut tracks, &mut by_album, &Track::from_song(s, &album_id))?;
            }
            let mut rows = wt.open_table(ALBUMS)?;
            let mut idx = wt.open_table(ALBUMS_BY_SORT)?;
            put_album(&mut rows, &mut idx, &Album::from(album.album))?;
        }
        Ok(wt.commit()?)
    }

    /// Deletes an album, its tracks and all their index entries. Returns whether
    /// the album existed.
    pub fn remove_album(&self, id: &str) -> Result<bool> {
        let wt = self.db.begin_write()?;
        let existed = {
            let mut tracks = wt.open_table(TRACKS)?;
            let mut by_album = wt.open_table(TRACKS_BY_ALBUM)?;
            remove_album_tracks(&mut tracks, &mut by_album, id)?;
            let mut rows = wt.open_table(ALBUMS)?;
            let mut idx = wt.open_table(ALBUMS_BY_SORT)?;
            let old = rows.remove(id)?.map(|g| decode::<Album>(g.value())).transpose()?;
            if let Some(old) = &old {
                let (artist, year, title) = old.sort_parts();
                idx.remove((artist.as_str(), year, title.as_str(), old.id.as_str()))?;
            }
            old.is_some()
        };
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
            out.extend(get_row(&rows, key.value().3)?);
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
        track_ids(&rt, album_id)?.iter().filter_map(|id| get_row(&rows, id).transpose()).collect()
    }

    pub fn artists(&self) -> Result<Vec<Artist>> {
        let rt = self.db.begin_read()?;
        let rows = rt.open_table(ARTISTS)?;
        let idx = rt.open_table(ARTISTS_BY_NAME)?;
        let mut out = Vec::with_capacity(idx.len()? as usize);
        for entry in idx.iter()? {
            let (key, _) = entry?;
            out.extend(get_row(&rows, key.value().1)?);
        }
        Ok(out)
    }

    pub fn album_count(&self) -> Result<u64> {
        Ok(self.db.begin_read()?.open_table(ALBUMS)?.len()?)
    }

    /// Checks that every index entry points at a row and every row has exactly
    /// its expected index entry. For tests and a future "verify library" command.
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
        Ok(problems)
    }
}

/// Schema upgrades, run inside the opening write transaction. Each step takes
/// the db from `from` to `from + 1`.
fn migrate(_wt: &WriteTransaction, from: u32) -> Result<()> {
    #[allow(clippy::match_single_binding)] // grows a branch per schema version
    for v in from..SCHEMA_VERSION {
        match v {
            // 0 => only possible for a db created before the meta table existed; nothing to do.
            _ => {}
        }
    }
    Ok(())
}

fn get_row<T: DeserializeOwned>(table: &impl ReadableTable<&'static str, &'static [u8]>, id: &str) -> Result<Option<T>> {
    table.get(id)?.map(|g| decode(g.value())).transpose()
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

fn put_track(rows: &mut Table<&'static str, &'static [u8]>, idx: &mut TrackIndex, t: &Track) -> Result<()> {
    // A track the server moved to another album leaves its old index entry behind
    // unless we drop it here.
    if let Some(old) = rows.insert(t.id.as_str(), encode(t)?.as_slice())? {
        let old: Track = decode(old.value())?;
        let (disc, no) = old.order_parts();
        idx.remove((old.album_id.as_str(), disc, no, old.id.as_str()))?;
    }
    let (disc, no) = t.order_parts();
    idx.insert((t.album_id.as_str(), disc, no, t.id.as_str()), ())?;
    Ok(())
}

fn remove_album_tracks(rows: &mut Table<&'static str, &'static [u8]>, idx: &mut TrackIndex, album_id: &str) -> Result<()> {
    for id in collect_track_ids(idx, album_id)? {
        if let Some(old) = rows.remove(id.as_str())? {
            let old: Track = decode(old.value())?;
            let (disc, no) = old.order_parts();
            idx.remove((old.album_id.as_str(), disc, no, old.id.as_str()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
