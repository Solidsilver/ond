use super::*;

use sources::{Chapter, Item, ItemSummary, Library, LibraryKind, Position, ProgressUpdate, Series, Stamp, TrackFile};
use std::time::Duration;

fn account() -> sources::AccountId {
    "2f1a5c3e-8a4b-4f6d-9e0a-1c2b3d4e5f60".parse().unwrap()
}

fn library() -> Library {
    Library { id: "lib-1".into(), name: "Books".into(), kind: LibraryKind::Books }
}

fn summary(id: &str, title: &str) -> ItemSummary {
    ItemSummary {
        id: id.into(),
        library: "lib-1".into(),
        title: title.into(),
        subtitle: Some("Book One".into()),
        authors: vec!["Frank Herbert".into()],
        narrators: vec!["Scott Brick".into(), "Simon Vance".into()],
        series: vec![Series { name: "Dune".into(), sequence: Some("1".into()) }],
        duration: Some(75600.5),
        cover: Some(id.into()),
        added: Some(Stamp::from_millis(1_765_000_000_000)),
        updated: Some(Stamp::from_millis(1_766_000_000_000)),
    }
}

fn item(id: &str) -> Item {
    Item {
        summary: summary(id, "Dune"),
        description: Some("A desert planet.".into()),
        genres: vec!["Science Fiction".into()],
        tags: vec!["favourite".into(), "re-read".into()],
        published_year: Some("1965".into()),
        publisher: Some("Chilton".into()),
        language: Some("English".into()),
        asin: Some("B0001".into()),
        isbn: Some("9780441013593".into()),
        tracks: vec![
            TrackFile {
                index: 1,
                start_offset: 0.0,
                duration: Some(3600.5),
                suffix: Some("m4b".into()),
                size: Some(500_000_000),
                mime: Some("audio/mp4".into()),
                content_id: "101".into(),
            },
            TrackFile {
                index: 2,
                start_offset: 3600.5,
                duration: Some(72000.0),
                suffix: Some("mp3".into()),
                size: Some(400_000_000),
                mime: Some("audio/mpeg".into()),
                content_id: "102".into(),
            },
        ],
        chapters: vec![Chapter { start: 0.0, end: 3600.0, title: "Chapter 1".into() }, Chapter { start: 3600.0, end: 7200.0, title: "Chapter 2".into() }],
    }
}

fn key(item: &str) -> ItemKey {
    ItemKey::new(account(), item)
}

fn changes(items: &[ItemSummary], removed: &[&str]) -> Changes<ItemSummary> {
    Changes {
        account: account(),
        items: items.to_vec(),
        removed: removed.iter().map(|id| (*id).to_owned()).collect(),
        stamp: Some(Stamp::from_millis(1_766_000_000_000)),
    }
}

fn update(position: f64, finished: bool, at_ms: i64, listened: u64) -> ProgressUpdate {
    ProgressUpdate {
        item: key("li-1"),
        position: Position::from_secs(position),
        finished,
        at: Stamp::from_millis(at_ms).system_time(),
        listened: Duration::from_secs(listened),
    }
}

fn shelf(dir: &tempfile::TempDir) -> Shelf {
    Shelf::open(dir.path().join("shelf.redb")).unwrap()
}

#[test]
fn every_record_round_trips_through_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shelf.redb");
    let (dune, children) = (key("li-1"), key("li-2"));
    let (progressed, bookmark) = {
        let shelf = Shelf::open(&path).unwrap();
        shelf.apply_changes(&library(), &changes(&[summary("li-1", "Dune"), summary("li-2", "Children of Dune")], &[])).unwrap();
        shelf.set_details(&dune, &item("li-1")).unwrap();
        let progress = shelf.append_progress(&update(3725.5, false, 1_767_000_000_000, 300)).unwrap();
        let bookmark = shelf
            .add_bookmark(&dune, Position::from_secs(3600.5), Some("The sandworm bit".into()), Stamp::from_millis(1_767_000_000_000).system_time())
            .unwrap();
        let state = SyncState { cursor: Some("page=3".into()), newest: Some(Stamp::from_millis(1_766_000_000_000)) };
        shelf.set_sync_state("lib-1", &state).unwrap();
        (progress, bookmark)
    };

    let shelf = Shelf::open(&path).unwrap();
    assert_eq!(shelf.item(&dune).unwrap(), Some(item("li-1")));
    // An item the details were never fetched for has the listing's view only.
    assert!(shelf.item(&children).unwrap().is_none());
    assert_eq!(shelf.items("lib-1").unwrap().len(), 2);
    assert!(shelf.items("lib-2").unwrap().is_empty());

    let pending = shelf.pending_progress().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, progressed);
    assert_eq!(pending[0].update, update(3725.5, false, 1_767_000_000_000, 300));

    let position = shelf.position(&dune).unwrap().expect("logged");
    assert_eq!(position.item, dune);
    assert_eq!(position.position, Position::from_secs(3725.5));
    assert!(!position.finished);
    assert_eq!(position.at, Some(Stamp::from_millis(1_767_000_000_000).system_time()));

    let saved = shelf.bookmarks(&dune).unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].id, bookmark);
    assert_eq!(saved[0].position, Position::from_secs(3600.5));
    assert_eq!(saved[0].note.as_deref(), Some("The sandworm bit"));
    assert_eq!(shelf.bookmarks(&children).unwrap(), []);

    assert_eq!(shelf.sync_state("lib-1").unwrap(), Some(SyncState { cursor: Some("page=3".into()), newest: Some(Stamp::from_millis(1_766_000_000_000)) }));
    assert_eq!(shelf.sync_state("lib-2").unwrap(), None);
}

#[test]
fn the_progress_log_keeps_its_order_and_pending_skips_synced_entries() {
    let dir = tempfile::tempdir().unwrap();
    let shelf = shelf(&dir);
    let first = shelf.append_progress(&update(10.0, false, 1_767_000_000_000, 10)).unwrap();
    let second = shelf.append_progress(&update(20.0, false, 1_767_000_100_000, 20)).unwrap();
    // Appended later with an earlier time: the log keeps append order, which
    // is the order the source is told about them.
    let backdated = shelf.append_progress(&update(5.0, false, 1_766_999_000_000, 0)).unwrap();
    let last = shelf.append_progress(&update(30.0, true, 1_767_000_200_000, 30)).unwrap();
    assert_eq!([first, second, backdated, last], [1, 2, 3, 4]);

    assert_eq!(shelf.pending_progress().unwrap().iter().map(|p| p.id).collect::<Vec<_>>(), [1, 2, 3, 4]);
    shelf.mark_synced(&[first, backdated]).unwrap();
    let pending = shelf.pending_progress().unwrap();
    assert_eq!(pending.iter().map(|p| p.id).collect::<Vec<_>>(), [second, last]);
    assert!(pending[1].update.finished);
    assert_eq!(shelf.pending_progress().unwrap()[0].update.position, Position::from_secs(20.0));

    shelf.mark_synced(&[second, last]).unwrap();
    assert!(shelf.pending_progress().unwrap().is_empty());
    // The log itself keeps every entry, synced or not.
    assert_eq!(shelf.progress_rows().unwrap(), 4);
    // The position follows the newest entry, not the newest time.
    assert_eq!(shelf.position(&key("li-1")).unwrap().unwrap().position, Position::from_secs(30.0));
}

#[test]
fn apply_changes_removes_deleted_items_with_their_tracks_and_chapters() {
    let dir = tempfile::tempdir().unwrap();
    let shelf = shelf(&dir);
    let (dune, children) = (key("li-1"), key("li-2"));
    shelf.apply_changes(&library(), &changes(&[summary("li-1", "Dune"), summary("li-2", "Children of Dune")], &[])).unwrap();
    shelf.set_details(&dune, &item("li-1")).unwrap();
    shelf.set_details(&children, &item("li-2")).unwrap();
    assert_eq!(shelf.row_counts().unwrap(), (2, 2, 2));
    shelf.append_progress(&update(3725.5, false, 1_767_000_000_000, 300)).unwrap();
    shelf.add_bookmark(&dune, Position::from_secs(3600.5), None, Stamp::from_millis(1_767_000_000_000).system_time()).unwrap();

    shelf.apply_changes(&library(), &changes(&[], &["li-1"])).unwrap();
    assert_eq!(shelf.row_counts().unwrap(), (1, 1, 1));
    assert!(shelf.item(&dune).unwrap().is_none());
    assert_eq!(shelf.items("lib-1").unwrap().len(), 1);
    assert_eq!(shelf.items("lib-1").unwrap()[0].id, "li-2");
    // What was listened to and saved stays: a book missing from one listing
    // may come back, and a position is worth more than a stray row.
    assert_eq!(shelf.position(&dune).unwrap().unwrap().position, Position::from_secs(3725.5));
    assert_eq!(shelf.bookmarks(&dune).unwrap().len(), 1);
    assert_eq!(shelf.pending_progress().unwrap().len(), 1);

    // Removing an item that isn't there is a no-op, not an error.
    shelf.apply_changes(&library(), &changes(&[], &["li-9"])).unwrap();
    assert_eq!(shelf.row_counts().unwrap(), (1, 1, 1));
}

#[test]
fn a_sync_keeps_the_details_it_already_has() {
    let dir = tempfile::tempdir().unwrap();
    let shelf = shelf(&dir);
    shelf.apply_changes(&library(), &changes(&[summary("li-1", "Dune")], &[])).unwrap();
    shelf.set_details(&key("li-1"), &item("li-1")).unwrap();
    let mut renamed = summary("li-1", "Dune (Deluxe)");
    renamed.updated = Some(Stamp::from_millis(1_766_100_000_000));
    shelf.apply_changes(&library(), &changes(&[renamed], &[])).unwrap();
    let stored = shelf.item(&key("li-1")).unwrap().expect("details survive a listing");
    assert_eq!(stored.summary.title, "Dune (Deluxe)");
    assert_eq!(stored.tracks.len(), 2);
    assert_eq!(stored.chapters.len(), 2);
    assert_eq!(shelf.row_counts().unwrap(), (1, 1, 1));
}

#[test]
fn an_item_moving_to_another_library_leaves_the_old_listing() {
    let dir = tempfile::tempdir().unwrap();
    let shelf = shelf(&dir);
    let other = Library { id: "lib-2".into(), name: "Other".into(), kind: LibraryKind::Books };
    shelf.apply_changes(&library(), &changes(&[summary("li-1", "Dune")], &[])).unwrap();
    shelf.apply_changes(&other, &changes(&[summary("li-1", "Dune")], &[])).unwrap();
    assert!(shelf.items("lib-1").unwrap().is_empty());
    assert_eq!(shelf.items("lib-2").unwrap().len(), 1);
    assert_eq!(shelf.row_counts().unwrap().0, 1);
}

#[test]
fn bookmarks_come_back_in_book_order_and_can_be_removed() {
    let dir = tempfile::tempdir().unwrap();
    let shelf = shelf(&dir);
    let at = |secs: f64| Position::from_secs(secs);
    let made = Stamp::from_millis(1_767_000_000_000).system_time();
    let late = shelf.add_bookmark(&key("li-1"), at(7200.0), None, made).unwrap();
    let early = shelf.add_bookmark(&key("li-1"), at(60.0), Some("Start".into()), made).unwrap();
    let ids: Vec<_> = shelf.bookmarks(&key("li-1")).unwrap().iter().map(|b| b.id).collect();
    assert_eq!(ids, [early, late]);

    assert!(shelf.remove_bookmark(&key("li-1"), late).unwrap());
    assert!(!shelf.remove_bookmark(&key("li-1"), late).unwrap());
    assert_eq!(shelf.bookmarks(&key("li-1")).unwrap().len(), 1);
    // Another item's bookmarks are untouched, and an id is never reused.
    assert!(shelf.bookmarks(&key("li-2")).unwrap().is_empty());
    assert_ne!(shelf.add_bookmark(&key("li-1"), at(10.0), None, made).unwrap(), late);
}

#[test]
fn a_file_from_a_newer_schema_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shelf.redb");
    drop(Shelf::open(&path).unwrap());
    {
        let db = Database::create(&path).unwrap();
        let wt = db.begin_write().unwrap();
        wt.open_table(META).unwrap().insert(SCHEMA_KEY, SCHEMA_VERSION + 1).unwrap();
        wt.commit().unwrap();
    }
    match Shelf::open(&path) {
        Err(Error::NewerSchema { found }) => assert_eq!(found, SCHEMA_VERSION + 1),
        Err(e) => panic!("expected a newer-schema refusal, got {e}"),
        Ok(_) => panic!("a file from a newer build opened, and its rows would be read as this build's"),
    }
}

/// What to do when a golden-bytes test fails.
const SHAPE_CHANGED: &str = "the stored shape of a shelf row changed, so rows already on disk no longer decode as this type. \
    Don't edit the bytes to match: freeze the old struct (and every type inside it) as the previous version, \
    add the next version, bump SCHEMA_VERSION and add a step to `migrate` (see the rules in model.rs). \
    Then add the new bytes as the new version's test";

fn full_item_v1() -> ItemV1 {
    ItemV1 { summary: SummaryV1::from(&summary("li-1", "Dune")), details: Some(DetailsV1::from(&item("li-1"))) }
}

fn full_tracks_v1() -> TracksV1 {
    TracksV1::from(item("li-1").tracks.as_slice())
}

fn full_chapters_v1() -> ChaptersV1 {
    ChaptersV1::from(item("li-1").chapters.as_slice())
}

fn full_progress_v1() -> ProgressEntryV1 {
    ProgressEntryV1 { synced: true, ..ProgressEntryV1::new(&key("li-1"), &update(3725.5, false, 1_767_000_000_000, 300)) }
}

fn full_position_v1() -> PositionV1 {
    PositionV1 { position: 3725.5, finished: false, at_ms: 1_767_000_000_000 }
}

fn full_bookmark_v1() -> BookmarkV1 {
    BookmarkV1 { position: 3600.5, note: Some("The sandworm bit".into()), created_ms: 1_767_000_000_000 }
}

fn full_sync_v1() -> LibrarySyncV1 {
    LibrarySyncV1 { cursor: Some("page=3".into()), newest_ms: Some(1_766_000_000_000) }
}

#[test]
fn item_v1_keeps_its_wire_format() {
    const BYTES: &[u8] = &[
        4, 108, 105, 45, 49, 5, 108, 105, 98, 45, 49, 4, 68, 117, 110, 101, 1, 8, 66, 111, 111, 107, 32, 79, 110, 101, 1, 13, 70, 114, 97, 110, 107, 32, 72,
        101, 114, 98, 101, 114, 116, 2, 11, 83, 99, 111, 116, 116, 32, 66, 114, 105, 99, 107, 11, 83, 105, 109, 111, 110, 32, 86, 97, 110, 99, 101, 1, 4, 68,
        117, 110, 101, 1, 1, 49, 1, 0, 0, 0, 0, 8, 117, 242, 64, 1, 4, 108, 105, 45, 49, 1, 128, 200, 149, 163, 222, 102, 1, 128, 240, 235, 220, 229, 102, 1,
        1, 16, 65, 32, 100, 101, 115, 101, 114, 116, 32, 112, 108, 97, 110, 101, 116, 46, 1, 15, 83, 99, 105, 101, 110, 99, 101, 32, 70, 105, 99, 116, 105,
        111, 110, 2, 9, 102, 97, 118, 111, 117, 114, 105, 116, 101, 7, 114, 101, 45, 114, 101, 97, 100, 1, 4, 49, 57, 54, 53, 1, 7, 67, 104, 105, 108, 116,
        111, 110, 1, 7, 69, 110, 103, 108, 105, 115, 104, 1, 5, 66, 48, 48, 48, 49, 1, 13, 57, 55, 56, 48, 52, 52, 49, 48, 49, 51, 53, 57, 51,
    ];
    assert_eq!(encode(&full_item_v1()).unwrap(), BYTES, "{SHAPE_CHANGED}");
    assert_eq!(decode::<ItemV1>(BYTES).unwrap(), full_item_v1());
}

#[test]
fn tracks_v1_keeps_its_wire_format() {
    const BYTES: &[u8] = &[
        2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 33, 172, 64, 1, 3, 109, 52, 98, 1, 128, 202, 181, 238, 1, 1, 9, 97, 117, 100, 105, 111, 47, 109, 112,
        52, 3, 49, 48, 49, 2, 0, 0, 0, 0, 0, 33, 172, 64, 1, 0, 0, 0, 0, 0, 148, 241, 64, 1, 3, 109, 112, 51, 1, 128, 136, 222, 190, 1, 1, 10, 97, 117, 100,
        105, 111, 47, 109, 112, 101, 103, 3, 49, 48, 50,
    ];
    assert_eq!(encode(&full_tracks_v1()).unwrap(), BYTES, "{SHAPE_CHANGED}");
    assert_eq!(decode::<TracksV1>(BYTES).unwrap(), full_tracks_v1());
}

#[test]
fn chapters_v1_keeps_its_wire_format() {
    const BYTES: &[u8] = &[
        2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 32, 172, 64, 9, 67, 104, 97, 112, 116, 101, 114, 32, 49, 0, 0, 0, 0, 0, 32, 172, 64, 0, 0, 0, 0, 0, 32, 188,
        64, 9, 67, 104, 97, 112, 116, 101, 114, 32, 50,
    ];
    assert_eq!(encode(&full_chapters_v1()).unwrap(), BYTES, "{SHAPE_CHANGED}");
    assert_eq!(decode::<ChaptersV1>(BYTES).unwrap(), full_chapters_v1());
}

#[test]
fn progress_entry_v1_keeps_its_wire_format() {
    const BYTES: &[u8] = &[
        36, 50, 102, 49, 97, 53, 99, 51, 101, 45, 56, 97, 52, 98, 45, 52, 102, 54, 100, 45, 57, 101, 48, 97, 45, 49, 99, 50, 98, 51, 100, 52, 101, 53, 102, 54,
        48, 4, 108, 105, 45, 49, 0, 0, 0, 0, 0, 27, 173, 64, 0, 128, 152, 194, 150, 237, 102, 224, 167, 18, 1,
    ];
    assert_eq!(encode(&full_progress_v1()).unwrap(), BYTES, "{SHAPE_CHANGED}");
    assert_eq!(decode::<ProgressEntryV1>(BYTES).unwrap(), full_progress_v1());
}

#[test]
fn position_v1_keeps_its_wire_format() {
    const BYTES: &[u8] = &[0, 0, 0, 0, 0, 27, 173, 64, 0, 128, 152, 194, 150, 237, 102];
    assert_eq!(encode(&full_position_v1()).unwrap(), BYTES, "{SHAPE_CHANGED}");
    assert_eq!(decode::<PositionV1>(BYTES).unwrap(), full_position_v1());
}

#[test]
fn bookmark_v1_keeps_its_wire_format() {
    const BYTES: &[u8] =
        &[0, 0, 0, 0, 0, 33, 172, 64, 1, 16, 84, 104, 101, 32, 115, 97, 110, 100, 119, 111, 114, 109, 32, 98, 105, 116, 128, 152, 194, 150, 237, 102];
    assert_eq!(encode(&full_bookmark_v1()).unwrap(), BYTES, "{SHAPE_CHANGED}");
    assert_eq!(decode::<BookmarkV1>(BYTES).unwrap(), full_bookmark_v1());
}

#[test]
fn library_sync_v1_keeps_its_wire_format() {
    const BYTES: &[u8] = &[1, 6, 112, 97, 103, 101, 61, 51, 1, 128, 240, 235, 220, 229, 102];
    assert_eq!(encode(&full_sync_v1()).unwrap(), BYTES, "{SHAPE_CHANGED}");
    assert_eq!(decode::<LibrarySyncV1>(BYTES).unwrap(), full_sync_v1());
}

fn source_bookmark(id: &str, position: f64, title: &str) -> sources::Bookmark {
    sources::Bookmark { item: key(id), position: Position::from_secs(position), title: title.into(), created: Stamp::from_millis(1).system_time() }
}

#[test]
fn bookmark_edits_and_their_exact_positions_survive_a_restart_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shelf.redb");
    let original = source_bookmark("book", 1_234.000_000_000_000_2, "Here");
    let renamed = sources::Bookmark { title: "There".into(), ..original.clone() };
    let ids = {
        let shelf = Shelf::open(&path).unwrap();
        let create = shelf.queue_bookmark(&original, BookmarkChange::Create).unwrap();
        let rename = shelf.queue_bookmark(&renamed, BookmarkChange::Rename).unwrap();
        assert_eq!(shelf.all_bookmarks().unwrap().as_slice(), std::slice::from_ref(&renamed));
        let delete = shelf.queue_bookmark(&renamed, BookmarkChange::Delete).unwrap();
        assert!(shelf.bookmarks(&original.item).unwrap().is_empty());
        [create, rename, delete]
    };
    let shelf = Shelf::open(&path).unwrap();
    let pending = shelf.pending_bookmarks().unwrap();
    assert_eq!(pending.iter().map(|p| p.id).collect::<Vec<_>>(), ids);
    assert_eq!(pending[1].base.as_ref(), Some(&original));
    assert_eq!(pending[2].base.as_ref(), Some(&renamed));
    assert_eq!(pending[2].bookmark.position.secs().to_bits(), original.position.secs().to_bits());
    shelf.acknowledge_bookmark(ids[0]).unwrap();
    assert_eq!(shelf.pending_bookmarks().unwrap().len(), 2);
}

#[test]
fn an_inflight_snapshot_cannot_erase_a_newer_local_bookmark_edit() {
    let dir = tempfile::tempdir().unwrap();
    let shelf = Shelf::open(dir.path().join("shelf.redb")).unwrap();
    let phone = source_bookmark("book", 200.5, "Phone");
    shelf.replace_bookmarks(std::slice::from_ref(&phone)).unwrap();
    let before = shelf.bookmark_snapshot().unwrap();
    let local = source_bookmark("book", 100.25, "Local");
    shelf.queue_bookmark(&local, BookmarkChange::Create).unwrap();
    shelf.queue_bookmark(&phone, BookmarkChange::Delete).unwrap();
    shelf.replace_bookmarks(&[phone]).unwrap();
    let after = shelf.bookmark_snapshot().unwrap();
    assert!(after.revision > before.revision);
    assert_eq!(after.bookmarks, [local]);
}

#[test]
fn upgrading_v1_preserves_frozen_bookmarks_and_queues_them_for_sync() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shelf.redb");
    let original = full_bookmark_v1();
    {
        let db = Database::create(&path).unwrap();
        let wt = db.begin_write().unwrap();
        wt.open_table(META).unwrap().insert(SCHEMA_KEY, 1).unwrap();
        wt.open_table(COUNTERS).unwrap();
        wt.open_table(BOOKMARKS).unwrap().insert((account().to_string().as_str(), "book", 1), encode(&original).unwrap().as_slice()).unwrap();
        wt.commit().unwrap();
    }
    let shelf = Shelf::open(&path).unwrap();
    assert_eq!(shelf.schema_version().unwrap(), 2);
    let saved = shelf.bookmarks(&key("book")).unwrap();
    assert_eq!(saved[0].note, original.note);
    assert_eq!(shelf.pending_bookmarks().unwrap()[0].change, BookmarkChange::Create);
}

#[test]
fn bookmark_edit_v1_keeps_its_wire_format() {
    use crate::model::BookmarkEditV1;
    let value = BookmarkV1 { position: 1.5, note: Some("n".into()), created_ms: 1 };
    let edit = BookmarkEditV1 { account: "a".into(), item: "b".into(), kind: 1, value: value.clone(), base: Some(value) };
    const BYTES: &[u8] = &[1, 97, 1, 98, 1, 0, 0, 0, 0, 0, 0, 248, 63, 1, 1, 110, 2, 1, 0, 0, 0, 0, 0, 0, 248, 63, 1, 1, 110, 2];
    assert_eq!(encode(&edit).unwrap(), BYTES, "{SHAPE_CHANGED}");
    assert_eq!(decode::<BookmarkEditV1>(BYTES).unwrap(), edit);
}

#[test]
fn accepting_a_phone_position_does_not_create_listening_or_overwrite_a_newer_local_write() {
    let dir = tempfile::tempdir().unwrap();
    let shelf = Shelf::open(dir.path().join("shelf.redb")).unwrap();
    let mut remote = Progress { item: key("book"), position: Position::from_secs(500.0), finished: false, at: Some(Stamp::from_millis(10).system_time()) };
    assert!(shelf.accept_position(&remote).unwrap());
    assert!(shelf.pending_progress().unwrap().is_empty());
    let local = ProgressUpdate {
        item: remote.item.clone(),
        position: Position::from_secs(600.0),
        finished: false,
        at: Stamp::from_millis(20).system_time(),
        listened: Duration::ZERO,
    };
    shelf.append_progress(&local).unwrap();
    remote.position = Position::from_secs(700.0);
    assert!(!shelf.accept_position(&remote).unwrap());
    assert_eq!(shelf.position(&remote.item).unwrap().unwrap().position.secs(), 600.0);
}
