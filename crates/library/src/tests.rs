use super::*;

fn album(id: &str, name: &str, artist: &str, year: i32) -> subsonic::Album {
    subsonic::Album {
        id: id.into(),
        name: name.into(),
        artist: Some(artist.into()),
        artist_id: Some(format!("ar-{artist}")),
        cover_art: None,
        song_count: 0,
        duration: 0,
        year: Some(year),
        genre: None,
        created: None,
        starred: None,
        play_count: None,
        sort_name: None,
    }
}

fn song(id: &str, album_id: &str, disc: u32, track: u32) -> subsonic::Song {
    subsonic::Song {
        id: id.into(),
        title: format!("t{track}"),
        album: None,
        album_id: Some(album_id.into()),
        artist: None,
        artist_id: None,
        track: Some(track),
        disc_number: Some(disc),
        year: None,
        genre: None,
        cover_art: None,
        size: None,
        content_type: None,
        suffix: Some("flac".into()),
        duration: Some(100),
        bit_rate: None,
        bit_depth: Some(24),
        sampling_rate: Some(88_200),
        channel_count: Some(2),
        replay_gain: None,
    }
}

#[test]
fn albums_sorted_by_artist_then_year() {
    let lib = Library::in_memory().unwrap();
    lib.upsert_albums([
        album("3", "Discovery", "Daft Punk", 2001),
        album("1", "Kind of Blue", "Miles Davis", 1959),
        album("2", "Homework", "Daft Punk", 1997),
    ])
    .unwrap();
    let names: Vec<_> = lib.albums().unwrap().into_iter().map(|a| a.name).collect();
    assert_eq!(names, ["Homework", "Discovery", "Kind of Blue"]);
    assert_eq!(lib.album_count().unwrap(), 3);
}

#[test]
fn upsert_is_idempotent() {
    let lib = Library::in_memory().unwrap();
    lib.upsert_albums([album("1", "A", "X", 2000)]).unwrap();
    lib.upsert_albums([album("1", "A (Remaster)", "X", 2000)]).unwrap();
    let all = lib.albums().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].name, "A (Remaster)");
}

#[test]
fn album_tracks_in_disc_track_order_and_replaced_on_resync() {
    let lib = Library::in_memory().unwrap();
    let with_songs = |songs| subsonic::AlbumWithSongs { album: album("al", "A", "X", 2000), song: songs };
    lib.upsert_album_tracks(with_songs(vec![
        song("c", "al", 2, 1),
        song("b", "al", 1, 10),
        song("a", "al", 1, 2),
        song("gone", "al", 1, 3),
    ]))
    .unwrap();
    // An album id that is a prefix of another must not leak tracks across.
    lib.upsert_album_tracks(subsonic::AlbumWithSongs { album: album("al2", "B", "X", 2001), song: vec![song("z", "al2", 1, 1)] })
        .unwrap();

    let ids: Vec<_> = lib.album_tracks("al").unwrap().into_iter().map(|t| t.id).collect();
    assert_eq!(ids, ["a", "gone", "b", "c"]);

    lib.upsert_album_tracks(with_songs(vec![song("a", "al", 1, 2), song("b", "al", 1, 10)])).unwrap();
    let ids: Vec<_> = lib.album_tracks("al").unwrap().into_iter().map(|t| t.id).collect();
    assert_eq!(ids, ["a", "b"]);
    assert_eq!(lib.album_tracks("al2").unwrap().len(), 1);
}

#[test]
fn quality_label() {
    let t = Track::from_song(song("a", "al", 1, 1), "al");
    assert_eq!(t.quality_label(), "FLAC 24/88.2");
}

#[test]
fn persists_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lib.redb");
    Library::open(&path).unwrap().upsert_albums([album("1", "A", "X", 2000)]).unwrap();
    assert_eq!(Library::open(&path).unwrap().album_count().unwrap(), 1);
}

fn assert_consistent(lib: &Library) {
    assert_eq!(lib.check_indexes().unwrap(), Vec::<String>::new());
}

#[test]
fn sort_key_folds_case_and_accents() {
    let lib = Library::in_memory().unwrap();
    lib.upsert_albums([
        album("1", "Zebra", "bjork", 2001),
        album("2", "Vespertine", "Björk", 2001),
        album("3", "Alpha", "BJORK", 1993),
        album("4", "Anything", "Air", 1998),
    ])
    .unwrap();
    let names: Vec<_> = lib.albums().unwrap().into_iter().map(|a| a.name).collect();
    // "Air" < "bjork"; the three spellings of Björk group together, then year, then title.
    assert_eq!(names, ["Anything", "Alpha", "Vespertine", "Zebra"]);
}

#[test]
fn album_update_moves_index_entry() {
    let lib = Library::in_memory().unwrap();
    lib.upsert_albums([album("1", "A", "Zed", 2000), album("2", "B", "Mid", 2000)]).unwrap();
    lib.upsert_albums([album("1", "A", "Aaa", 2000)]).unwrap();
    let ids: Vec<_> = lib.albums().unwrap().into_iter().map(|a| a.id).collect();
    assert_eq!(ids, ["1", "2"]);
    assert_consistent(&lib);
}

#[test]
fn remove_album_drops_tracks_and_index_entries() {
    let lib = Library::in_memory().unwrap();
    lib.upsert_album_tracks(subsonic::AlbumWithSongs {
        album: album("al", "A", "X", 2000),
        song: vec![song("a", "al", 1, 1), song("b", "al", 1, 2)],
    })
    .unwrap();
    lib.upsert_album_tracks(subsonic::AlbumWithSongs { album: album("al2", "B", "X", 2001), song: vec![song("c", "al2", 1, 1)] })
        .unwrap();

    assert!(lib.remove_album("al").unwrap());
    assert!(!lib.remove_album("al").unwrap());
    assert!(lib.album("al").unwrap().is_none());
    assert!(lib.album_tracks("al").unwrap().is_empty());
    assert_eq!(lib.album_tracks("al2").unwrap().len(), 1);
    assert_eq!(lib.album_count().unwrap(), 1);
    assert_consistent(&lib);
}

#[test]
fn track_moved_between_albums_leaves_no_stale_entry() {
    let lib = Library::in_memory().unwrap();
    let with = |id: &str, songs| subsonic::AlbumWithSongs { album: album(id, id, "X", 2000), song: songs };
    lib.upsert_album_tracks(with("one", vec![song("t", "one", 1, 1)])).unwrap();
    lib.upsert_album_tracks(with("two", vec![song("t", "two", 1, 5)])).unwrap();
    assert!(lib.album_tracks("one").unwrap().is_empty());
    assert_eq!(lib.album_tracks("two").unwrap()[0].track, Some(5));
    assert_consistent(&lib);
}

#[test]
fn resync_keeps_indexes_consistent() {
    let lib = Library::in_memory().unwrap();
    let with = |songs| subsonic::AlbumWithSongs { album: album("al", "A", "X", 2000), song: songs };
    lib.upsert_album_tracks(with(vec![song("a", "al", 1, 1), song("b", "al", 1, 2)])).unwrap();
    // Same ids, renumbered: old (disc, track) keys must be gone.
    lib.upsert_album_tracks(with(vec![song("a", "al", 1, 2), song("b", "al", 1, 1)])).unwrap();
    let ids: Vec<_> = lib.album_tracks("al").unwrap().into_iter().map(|t| t.id).collect();
    assert_eq!(ids, ["b", "a"]);
    assert_consistent(&lib);
}

#[test]
fn artists_sorted_by_folded_name() {
    let lib = Library::in_memory().unwrap();
    let artist = |id: &str, name: &str| subsonic::Artist {
        id: id.into(),
        name: name.into(),
        sort_name: None,
        album_count: 1,
        cover_art: None,
    };
    lib.upsert_artists([artist("1", "Émile"), artist("2", "eagles"), artist("3", "Zappa")]).unwrap();
    lib.upsert_artists([artist("2", "Eagles")]).unwrap();
    let names: Vec<_> = lib.artists().unwrap().into_iter().map(|a| a.name).collect();
    assert_eq!(names, ["Eagles", "Émile", "Zappa"]);
    assert_eq!(lib.artists().unwrap().len(), 3);
}

#[test]
fn schema_version_recorded_and_newer_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lib.redb");
    assert_eq!(Library::open(&path).unwrap().schema_version().unwrap(), SCHEMA_VERSION);

    let db = redb::Database::create(&path).unwrap();
    let wt = db.begin_write().unwrap();
    wt.open_table(META).unwrap().insert(SCHEMA_KEY, SCHEMA_VERSION + 1).unwrap();
    wt.commit().unwrap();
    drop(db);
    assert!(matches!(Library::open(&path), Err(Error::NewerSchema { .. })));
}

fn files_in(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> =
        std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

#[test]
fn old_redb_format_is_moved_aside_and_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.redb");
    {
        let lib = Library::open(&path).unwrap();
        lib.upsert_albums([album("a1", "Old", "X", 2000)]).unwrap();
    }
    // Make it look like a redb 2 file: both commit slots claim file format 2,
    // which is exactly what the pre-redb-4 build left on disk.
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[64] = 2;
    bytes[64 + 128] = 2;
    std::fs::write(&path, &bytes).unwrap();
    let err = Library::open(&path).err().expect("redb 4 refuses the old format");
    assert!(err.is_damage(), "{err}");

    let (lib, aside) = Library::open_or_recover(&path).unwrap();
    let aside = aside.expect("old file moved aside");
    assert!(aside.file_name().unwrap().to_string_lossy().starts_with("library.redb.bad-"));
    assert_eq!(std::fs::read(&aside).unwrap(), bytes, "kept byte for byte");
    assert_eq!(lib.album_count().unwrap(), 0, "fresh database, to be filled by a sync");
    assert_eq!(files_in(dir.path()).len(), 2);
}

#[test]
fn garbage_file_is_moved_aside_and_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.redb");
    std::fs::write(&path, vec![0x5a; 8192]).unwrap();
    let (lib, aside) = Library::open_or_recover(&path).unwrap();
    assert!(aside.is_some());
    assert_eq!(lib.schema_version().unwrap(), SCHEMA_VERSION);
}

#[test]
fn newer_schema_is_refused_not_destroyed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.redb");
    drop(Library::open(&path).unwrap());
    let db = redb::Database::create(&path).unwrap();
    let wt = db.begin_write().unwrap();
    wt.open_table(META).unwrap().insert(SCHEMA_KEY, SCHEMA_VERSION + 1).unwrap();
    wt.commit().unwrap();
    drop(db);

    let err = Library::open_or_recover(&path).err().expect("newer schema must not open");
    assert!(matches!(err, Error::NewerSchema { .. }));
    assert!(err.to_string().contains("newer version of Listener"), "{err}");
    assert_eq!(files_in(dir.path()), ["library.redb"], "nothing moved or deleted");
}
