//! Bookmark edits and their optimistic rows commit together. A server snapshot
//! is installed under the same write lock, then outstanding edits are overlaid;
//! a pull that was in flight cannot erase a newly queued offline edit.

use super::*;
use crate::model::BookmarkEditV1;
use sources::AccountId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookmarkChange {
    Create,
    Rename,
    Delete,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingBookmark {
    pub id: u64,
    pub change: BookmarkChange,
    pub bookmark: sources::Bookmark,
    /// The row seen when the user edited it, for server-wins comparison.
    pub base: Option<sources::Bookmark>,
}

/// The revision and rows come from one read transaction, so a delayed pull
/// cannot replace a newer optimistic snapshot in the UI.
#[derive(Debug, Clone, PartialEq)]
pub struct BookmarkSnapshot {
    pub revision: u64,
    pub bookmarks: Vec<sources::Bookmark>,
}

fn row(bookmark: &sources::Bookmark) -> BookmarkV1 {
    BookmarkV1 { position: bookmark.position.secs(), note: Some(bookmark.title.clone()), created_ms: Stamp::from(bookmark.created).millis() }
}
fn value(key: &ItemKey, row: BookmarkV1) -> sources::Bookmark {
    sources::Bookmark {
        item: key.clone(),
        position: Position::from_secs(row.position),
        title: row.note.unwrap_or_default(),
        created: Stamp::from_millis(row.created_ms).system_time(),
    }
}
fn kind(change: BookmarkChange) -> u8 {
    match change {
        BookmarkChange::Create => 0,
        BookmarkChange::Rename => 1,
        BookmarkChange::Delete => 2,
    }
}
fn change(kind: u8) -> Result<BookmarkChange> {
    match kind {
        0 => Ok(BookmarkChange::Create),
        1 => Ok(BookmarkChange::Rename),
        2 => Ok(BookmarkChange::Delete),
        _ => Err(Error::Codec(postcard::Error::DeserializeBadEnum)),
    }
}

impl Shelf {
    /// Saves an edit and its visible effect atomically, before attempting any
    /// network call. Rename/delete use the exact existing position.
    pub fn queue_bookmark(&self, bookmark: &sources::Bookmark, change: BookmarkChange) -> Result<u64> {
        let wt = self.db.begin_write()?;
        let mut table = wt.open_table(BOOKMARKS)?;
        let base = find(&table, &bookmark.item, bookmark.position)?.map(|(_, row)| row);
        let mut desired = row(bookmark);
        if let Some(base) = &base {
            desired.created_ms = base.created_ms;
        }
        let edit = BookmarkEditV1 { account: bookmark.item.account.to_string(), item: bookmark.item.item.clone(), kind: kind(change), value: desired, base };
        apply(&wt, &mut table, &edit)?;
        let id = next_id(&mut wt.open_table(COUNTERS)?, "next_bookmark_edit")?;
        wt.open_table(BOOKMARK_EDITS)?.insert(id, encode(&edit)?.as_slice())?;
        next_id(&mut wt.open_table(COUNTERS)?, "bookmark_revision")?;
        drop(table);
        wt.commit()?;
        Ok(id)
    }

    pub fn pending_bookmarks(&self) -> Result<Vec<PendingBookmark>> {
        let rt = self.db.begin_read()?;
        let mut out = Vec::new();
        for entry in rt.open_table(BOOKMARK_EDITS)?.iter()? {
            let (id, bytes) = entry?;
            let edit: BookmarkEditV1 = decode(bytes.value())?;
            let account = edit.account.parse::<AccountId>().map_err(|_| Error::Codec(postcard::Error::DeserializeBadEncoding))?;
            let key = ItemKey::new(account, edit.item);
            out.push(PendingBookmark {
                id: id.value(),
                change: change(edit.kind)?,
                bookmark: value(&key, edit.value),
                base: edit.base.map(|b| value(&key, b)),
            });
        }
        Ok(out)
    }

    /// Acknowledges only edits actually completed; later edits remain durable.
    pub fn acknowledge_bookmark(&self, id: u64) -> Result<()> {
        let wt = self.db.begin_write()?;
        wt.open_table(BOOKMARK_EDITS)?.remove(id)?;
        wt.commit()?;
        Ok(())
    }

    pub fn all_bookmarks(&self) -> Result<Vec<sources::Bookmark>> {
        Ok(self.bookmark_snapshot()?.bookmarks)
    }

    pub fn bookmark_snapshot(&self) -> Result<BookmarkSnapshot> {
        let rt = self.db.begin_read()?;
        let revision = rt.open_table(COUNTERS)?.get("bookmark_revision")?.map_or(0, |v| v.value());
        let mut out = Vec::new();
        for entry in rt.open_table(BOOKMARKS)?.iter()? {
            let (key, bytes) = entry?;
            let (account, item, _) = key.value();
            let account = account.parse::<AccountId>().map_err(|_| Error::Codec(postcard::Error::DeserializeBadEncoding))?;
            out.push(value(&ItemKey::new(account, item), decode(bytes.value())?));
        }
        out.sort_by(|a, b| a.position.secs().total_cmp(&b.position.secs()));
        Ok(BookmarkSnapshot { revision, bookmarks: out })
    }

    /// The server is authoritative after replay. Outstanding concurrent edits
    /// are retained over the snapshot until their own comparison and replay.
    pub fn replace_bookmarks(&self, remote: &[sources::Bookmark]) -> Result<()> {
        let wt = self.db.begin_write()?;
        let mut table = wt.open_table(BOOKMARKS)?;
        table.retain(|_, _| false)?;
        for bookmark in remote {
            let edit =
                BookmarkEditV1 { account: bookmark.item.account.to_string(), item: bookmark.item.item.clone(), kind: 0, value: row(bookmark), base: None };
            apply(&wt, &mut table, &edit)?;
        }
        for entry in wt.open_table(BOOKMARK_EDITS)?.iter()? {
            let (_, bytes) = entry?;
            apply(&wt, &mut table, &decode(bytes.value())?)?;
        }
        next_id(&mut wt.open_table(COUNTERS)?, "bookmark_revision")?;
        drop(table);
        wt.commit()?;
        Ok(())
    }

    /// A remote position is a snapshot, not local listening or an unsent log
    /// entry. Keep it only when no newer local write has raced the pull.
    pub fn accept_position(&self, progress: &Progress) -> Result<bool> {
        let account = progress.item.account.to_string();
        let wt = self.db.begin_write()?;
        let mut table = wt.open_table(POSITIONS)?;
        let key = (account.as_str(), progress.item.item.as_str());
        if let Some(old) = table.get(key)? {
            let old: PositionV1 = decode(old.value())?;
            if Some(Stamp::from_millis(old.at_ms).system_time()) > progress.at {
                return Ok(false);
            }
        }
        let row = PositionV1 { position: progress.position.secs(), finished: progress.finished, at_ms: progress.at.map_or(0, |at| Stamp::from(at).millis()) };
        table.insert(key, encode(&row)?.as_slice())?;
        drop(table);
        wt.commit()?;
        Ok(true)
    }
}

fn find(table: &Table<'_, (&str, &str, u64), &[u8]>, key: &ItemKey, position: Position) -> Result<Option<(u64, BookmarkV1)>> {
    let account = key.account.to_string();
    for entry in table.range((account.as_str(), key.item.as_str(), 0)..)? {
        let (found, bytes) = entry?;
        let (a, i, id) = found.value();
        if a != account || i != key.item {
            break;
        }
        let row: BookmarkV1 = decode(bytes.value())?;
        if row.position == position.secs() {
            return Ok(Some((id, row)));
        }
    }
    Ok(None)
}
fn apply(wt: &WriteTransaction, table: &mut Table<'_, (&str, &str, u64), &[u8]>, edit: &BookmarkEditV1) -> Result<()> {
    let account = edit.account.parse::<AccountId>().map_err(|_| Error::Codec(postcard::Error::DeserializeBadEncoding))?;
    let key = ItemKey::new(account, &edit.item);
    let existing = find(table, &key, Position::from_secs(edit.value.position))?;
    let id = match existing {
        Some((id, _)) => id,
        None if edit.kind == 1 || edit.kind == 2 => return Ok(()),
        None => next_id(&mut wt.open_table(COUNTERS)?, NEXT_BOOKMARK)?,
    };
    let key = (edit.account.as_str(), edit.item.as_str(), id);
    if edit.kind == 2 {
        table.remove(key)?;
    } else {
        table.insert(key, encode(&edit.value)?.as_slice())?;
    }
    Ok(())
}

/// Preserve bookmarks saved before sync existed by turning them into creates.
/// Their frozen bytes and ids remain untouched until the first server pull.
pub(super) fn migrate_v1(wt: &WriteTransaction) -> Result<()> {
    let mut edits = wt.open_table(BOOKMARK_EDITS)?;
    let mut counters = wt.open_table(COUNTERS)?;
    for entry in wt.open_table(BOOKMARKS)?.iter()? {
        let (key, bytes) = entry?;
        let (account, item, _) = key.value();
        let mut value: BookmarkV1 = decode(bytes.value())?;
        if value.note.as_ref().is_none_or(|title| title.trim().is_empty()) {
            value.note = Some(format!("Bookmark at {} s", value.position));
        }
        let edit = BookmarkEditV1 { account: account.to_owned(), item: item.to_owned(), kind: 0, value, base: None };
        let id = next_id(&mut counters, "next_bookmark_edit")?;
        edits.insert(id, encode(&edit)?.as_slice())?;
    }
    Ok(())
}
