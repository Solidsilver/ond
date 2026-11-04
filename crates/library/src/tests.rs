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
        user_rating: None,
        sort_name: None,
        record_labels: Vec::new(),
        genres: Vec::new(),
        artists: Vec::new(),
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
        display_composer: None,
        contributors: Vec::new(),
        artists: Vec::new(),
        starred: None,
        user_rating: None,
        play_count: None,
        played: None,
    }
}

#[test]
fn albums_sorted_by_artist_then_year() {
    let lib = Library::in_memory().unwrap();
    lib.upsert_albums([album("3", "Discovery", "Daft Punk", 2001), album("1", "Kind of Blue", "Miles Davis", 1959), album("2", "Homework", "Daft Punk", 1997)])
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
    lib.upsert_album_tracks(with_songs(vec![song("c", "al", 2, 1), song("b", "al", 1, 10), song("a", "al", 1, 2), song("gone", "al", 1, 3)])).unwrap();
    // An album id that is a prefix of another must not leak tracks across.
    lib.upsert_album_tracks(subsonic::AlbumWithSongs { album: album("al2", "B", "X", 2001), song: vec![song("z", "al2", 1, 1)] }).unwrap();

    let ids: Vec<_> = lib.album_tracks("al").unwrap().into_iter().map(|t| t.id).collect();
    assert_eq!(ids, ["a", "gone", "b", "c"]);

    lib.upsert_album_tracks(with_songs(vec![song("a", "al", 1, 2), song("b", "al", 1, 10)])).unwrap();
    let ids: Vec<_> = lib.album_tracks("al").unwrap().into_iter().map(|t| t.id).collect();
    assert_eq!(ids, ["a", "b"]);
    assert_eq!(lib.album_tracks("al2").unwrap().len(), 1);
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
    lib.upsert_album_tracks(subsonic::AlbumWithSongs { album: album("al", "A", "X", 2000), song: vec![song("a", "al", 1, 1), song("b", "al", 1, 2)] }).unwrap();
    lib.upsert_album_tracks(subsonic::AlbumWithSongs { album: album("al2", "B", "X", 2001), song: vec![song("c", "al2", 1, 1)] }).unwrap();

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
    let artist = |id: &str, name: &str| subsonic::Artist { id: id.into(), name: name.into(), sort_name: None, album_count: 1, cover_art: None };
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
    let mut names: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
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
    assert!(err.to_string().contains("newer version of Ond"), "{err}");
    assert_eq!(files_in(dir.path()), ["library.redb"], "nothing moved or deleted");
}

#[test]
fn v1_rows_migrate_to_current() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lib.redb");
    // Hand-build a v1 database: v1 rows and index entries, schema 1.
    {
        let db = redb::Database::create(&path).unwrap();
        let wt = db.begin_write().unwrap();
        {
            wt.open_table(META).unwrap().insert(SCHEMA_KEY, 1).unwrap();
            let v1 = AlbumV1 {
                id: "al-1".into(),
                name: "Kind of Blue".into(),
                sort: None,
                artist: Some("Miles Davis".into()),
                artist_ref: None,
                year: Some(1959),
                genre: Some("Jazz".into()),
                song_count: 1,
                duration_secs: 562,
                cover_art: None,
                created: None,
            };
            wt.open_table(ALBUMS).unwrap().insert("al-1", encode(&v1).unwrap().as_slice()).unwrap();
            wt.open_table(ALBUMS_BY_SORT).unwrap().insert(("miles davis", 1959, "kind of blue", "al-1"), ()).unwrap();
            let t1 = TrackV1 {
                id: "s-1".into(),
                album_id: "al-1".into(),
                title: "So What".into(),
                artist: None,
                track: Some(1),
                disc: Some(1),
                duration_secs: Some(562),
                suffix: None,
                content_type: None,
                size: None,
                bit_rate_kbps: None,
                bit_depth: None,
                sample_rate: None,
                channels: None,
                track_gain: None,
                album_gain: None,
                track_peak: None,
                album_peak: None,
            };
            wt.open_table(TRACKS).unwrap().insert("s-1", encode(&t1).unwrap().as_slice()).unwrap();
            wt.open_table(TRACKS_BY_ALBUM).unwrap().insert(("al-1", 1, 1, "s-1"), ()).unwrap();
        }
        wt.commit().unwrap();
    }
    let lib = Library::open(&path).unwrap();
    assert_eq!(lib.schema_version().unwrap(), SCHEMA_VERSION);
    let albums = lib.albums().unwrap();
    assert_eq!(albums[0].name, "Kind of Blue");
    assert_eq!(albums[0].all_genres(), ["Jazz"]);
    assert!(albums[0].labels.is_empty());
    let tracks = lib.album_tracks("al-1").unwrap();
    assert_eq!(tracks[0].title, "So What");
    assert!(tracks[0].contributors.is_empty());
    assert!(lib.check_indexes().unwrap().is_empty());
}

#[test]
fn credits_are_stored_and_findable() {
    let lib = Library::in_memory().unwrap();
    let mut s = song("s-1", "al-1", 1, 1);
    s.display_composer = Some("Scott LaFaro".into());
    s.contributors = vec![subsonic::Contributor {
        role: "performer".into(),
        sub_role: Some("piano".into()),
        artist: subsonic::ArtistRef { id: Some("ar-1".into()), name: "Bill Evans".into() },
    }];
    let mut a = album("al-1", "Sunday at the Village Vanguard", "Bill Evans Trio", 1961);
    a.record_labels = vec![subsonic::Named { name: "Riverside".into() }];
    lib.upsert_album_tracks(subsonic::AlbumWithSongs { album: a, song: vec![s] }).unwrap();
    let t = &lib.album_tracks("al-1").unwrap()[0];
    assert_eq!(t.composer.as_deref(), Some("Scott LaFaro"));
    assert_eq!(t.contributors[0].sub_role.as_deref(), Some("piano"));
    assert_eq!(lib.album("al-1").unwrap().unwrap().labels, ["Riverside"]);
    assert_eq!(lib.albums_crediting("Bill Evans").unwrap(), ["al-1"]);
    assert_eq!(lib.albums_crediting("Scott LaFaro").unwrap(), ["al-1"]);
    // Folded like sort keys.
    assert_eq!(lib.albums_crediting("scott lafaro").unwrap(), ["al-1"]);
    assert!(lib.albums_crediting("Nobody").unwrap().is_empty());
    assert_consistent(&lib);
}

fn artist_ref(id: &str, name: &str) -> subsonic::ArtistRef {
    subsonic::ArtistRef { id: Some(id.into()), name: name.into() }
}

#[test]
fn split_artists_are_kept_as_entities_and_credited() {
    let lib = Library::in_memory().unwrap();
    let mut a = album("al-1", "Ondes", "Johannes Motschmann, David Panzl, Boris Bolles", 2019);
    a.artists = vec![artist_ref("ar-1", "Johannes Motschmann"), artist_ref("ar-2", "David Panzl"), artist_ref("ar-3", "Boris Bolles")];
    let mut s = song("s-1", "al-1", 1, 1);
    s.artist = a.artist.clone();
    s.artists = a.artists.clone();
    lib.upsert_album_tracks(subsonic::AlbumWithSongs { album: a, song: vec![s] }).unwrap();

    let row = lib.album("al-1").unwrap().unwrap();
    assert_eq!(row.artists.len(), 3);
    assert!(row.has_artist(Some("ar-2"), "David Panzl"));
    assert!(row.has_artist(None, "Boris Bolles"));
    assert!(!row.has_artist(Some("ar-9"), "Someone Else"));
    // Each split artist finds the album; the joined string is credited too.
    assert_eq!(lib.albums_crediting("David Panzl").unwrap(), ["al-1"]);
    assert_eq!(lib.albums_crediting("Johannes Motschmann, David Panzl, Boris Bolles").unwrap(), ["al-1"]);
    assert_consistent(&lib);
}

#[test]
fn credits_follow_track_updates_and_removal() {
    let lib = Library::in_memory().unwrap();
    let with = |id: &str, songs| subsonic::AlbumWithSongs { album: album(id, id, "X", 2000), song: songs };
    let mut a = song("a", "one", 1, 1);
    a.display_composer = Some("Cole Porter".into());
    let mut b = song("b", "one", 1, 2);
    b.display_composer = Some("Cole Porter".into());
    lib.upsert_album_tracks(with("one", vec![a.clone(), b])).unwrap();
    assert_eq!(lib.albums_crediting("Cole Porter").unwrap(), ["one"]);

    // Re-credited on resync: the old name goes, the album stays for the new one.
    a.display_composer = Some("Gershwin".into());
    lib.upsert_album_tracks(with("one", vec![a])).unwrap();
    assert!(lib.albums_crediting("Cole Porter").unwrap().is_empty());
    assert_eq!(lib.albums_crediting("Gershwin").unwrap(), ["one"]);
    assert_consistent(&lib);

    lib.remove_album("one").unwrap();
    assert!(lib.albums_crediting("Gershwin").unwrap().is_empty());
    assert_consistent(&lib);
}

#[test]
fn stamps_written_with_tracks_and_dropped_with_album() {
    let lib = Library::in_memory().unwrap();
    let mut a = album("al", "A", "X", 2000);
    a.song_count = 1;
    a.cover_art = Some("al-al_65a1b2c3".into());
    let expected = Album::from(a.clone()).sync_stamp();
    lib.upsert_album_tracks(subsonic::AlbumWithSongs { album: a.clone(), song: vec![song("s", "al", 1, 1)] }).unwrap();
    assert_eq!(lib.album_stamps().unwrap().get("al"), Some(&expected));

    // Any change the album list reports changes the stamp: Navidrome bumps the
    // cover art id when an album is updated.
    let mut edited = a.clone();
    edited.cover_art = Some("al-al_65a1b2ff".into());
    assert_ne!(Album::from(edited).sync_stamp(), expected);
    let mut more = a;
    more.song_count = 2;
    assert_ne!(Album::from(more).sync_stamp(), expected);

    lib.remove_album("al").unwrap();
    assert!(lib.album_stamps().unwrap().is_empty());
}

#[test]
fn bulk_songs_store_without_stamping_and_counts_add_up() {
    let lib = Library::in_memory().unwrap();
    lib.upsert_albums([album("one", "One", "X", 2000), album("two", "Two", "X", 2001)]).unwrap();
    // A page boundary splits album "two" across two pages.
    lib.store_songs(vec![song("a", "one", 1, 1), song("b", "one", 1, 2), song("c", "two", 1, 1)]).unwrap();
    lib.store_songs(vec![song("d", "two", 1, 2), {
        let mut s = song("x", "", 1, 1);
        s.album_id = None;
        s
    }])
    .unwrap();
    assert!(lib.album_stamps().unwrap().is_empty());
    let counts = lib.track_counts().unwrap();
    assert_eq!(counts.get("one"), Some(&2));
    assert_eq!(counts.get("two"), Some(&2));
    assert_eq!(lib.track_count().unwrap(), 4, "songs without an album are skipped");
    let all: Vec<_> = lib.all_tracks().unwrap().into_iter().map(|t| t.id).collect();
    assert_eq!(all, ["a", "b", "c", "d"]);
    // Re-storing the same page is harmless.
    lib.store_songs(vec![song("a", "one", 1, 1)]).unwrap();
    assert_eq!(lib.track_count().unwrap(), 4);
    assert_consistent(&lib);
}

#[test]
fn retain_albums_drops_what_the_server_no_longer_has() {
    let lib = Library::in_memory().unwrap();
    for id in ["a", "b", "c"] {
        lib.upsert_album_tracks(subsonic::AlbumWithSongs { album: album(id, id, "X", 2000), song: vec![song(&format!("{id}-1"), id, 1, 1)] }).unwrap();
    }
    let keep = ["a".to_owned(), "c".to_owned()].into_iter().collect();
    assert_eq!(lib.retain_albums(&keep).unwrap(), 1);
    assert_eq!(lib.albums().unwrap().into_iter().map(|a| a.id).collect::<Vec<_>>(), ["a", "c"]);
    assert_eq!(lib.track_count().unwrap(), 2);
    assert!(!lib.album_stamps().unwrap().contains_key("b"));
    assert_consistent(&lib);
}

#[test]
fn replace_artists_drops_stale_ones() {
    let lib = Library::in_memory().unwrap();
    let artist = |id: &str, name: &str| subsonic::Artist { id: id.into(), name: name.into(), sort_name: None, album_count: 1, cover_art: None };
    lib.replace_artists([artist("1", "Old Name"), artist("2", "Kept")]).unwrap();
    lib.replace_artists([artist("2", "Kept"), artist("3", "New")]).unwrap();
    let names: Vec<_> = lib.artists().unwrap().into_iter().map(|a| a.name).collect();
    assert_eq!(names, ["Kept", "New"]);
}

#[test]
fn meta_values_round_trip() {
    let lib = Library::in_memory().unwrap();
    assert_eq!(lib.meta("bulk_offset").unwrap(), None);
    lib.set_meta("bulk_offset", Some(1500)).unwrap();
    assert_eq!(lib.meta("bulk_offset").unwrap(), Some(1500));
    lib.set_meta("bulk_offset", None).unwrap();
    assert_eq!(lib.meta("bulk_offset").unwrap(), None);
}

#[test]
fn v2_rows_migrate_to_v3_with_credits_index() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lib.redb");
    {
        let db = redb::Database::create(&path).unwrap();
        let wt = db.begin_write().unwrap();
        {
            wt.open_table(META).unwrap().insert(SCHEMA_KEY, 2).unwrap();
            let v2 = AlbumV2::from(AlbumV1 {
                id: "al-1".into(),
                name: "Sunday".into(),
                sort: None,
                artist: Some("Bill Evans Trio".into()),
                artist_ref: None,
                year: Some(1961),
                genre: None,
                song_count: 1,
                duration_secs: 370,
                cover_art: None,
                created: None,
            });
            wt.open_table(ALBUMS).unwrap().insert("al-1", encode(&v2).unwrap().as_slice()).unwrap();
            wt.open_table(ALBUMS_BY_SORT).unwrap().insert(("bill evans trio", 1961, "sunday", "al-1"), ()).unwrap();
            let mut t2 = TrackV2::from(TrackV1 {
                id: "s-1".into(),
                album_id: "al-1".into(),
                title: "Gloria's Step".into(),
                artist: Some("Bill Evans Trio".into()),
                track: Some(1),
                disc: Some(1),
                duration_secs: Some(370),
                suffix: None,
                content_type: None,
                size: None,
                bit_rate_kbps: None,
                bit_depth: None,
                sample_rate: None,
                channels: None,
                track_gain: None,
                album_gain: None,
                track_peak: None,
                album_peak: None,
            });
            t2.composer = Some("Scott LaFaro".into());
            wt.open_table(TRACKS).unwrap().insert("s-1", encode(&t2).unwrap().as_slice()).unwrap();
            wt.open_table(TRACKS_BY_ALBUM).unwrap().insert(("al-1", 1, 1, "s-1"), ()).unwrap();
            // No v2 database had stamps, but one here makes the assertion
            // below mean something: migrated rows lack fields only a refetch
            // fills, so their stamps must go.
            wt.open_table(ALBUM_STAMPS).unwrap().insert("al-1", 42).unwrap();
        }
        wt.commit().unwrap();
    }
    let lib = Library::open(&path).unwrap();
    assert_eq!(lib.schema_version().unwrap(), SCHEMA_VERSION);
    assert!(lib.albums().unwrap()[0].artists.is_empty());
    assert_eq!(lib.album_tracks("al-1").unwrap()[0].composer.as_deref(), Some("Scott LaFaro"));
    assert_eq!(lib.albums_crediting("Scott LaFaro").unwrap(), ["al-1"]);
    assert!(lib.album_stamps().unwrap().is_empty(), "migrated tracks get fetched again by the next sync");
    assert_consistent(&lib);
}

#[test]
fn analysis_records_persist_and_replace() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("analysis.redb");
    {
        let store = AnalysisStore::open(&path).unwrap();
        store.put("s-1", b"one").unwrap();
        store.put("s-2", b"two").unwrap();
        store.put("s-1", b"uno").unwrap();
    }
    let store = AnalysisStore::open(&path).unwrap();
    let mut rows = store.records().unwrap();
    rows.sort();
    assert_eq!(rows, [("s-1".to_owned(), b"uno".to_vec()), ("s-2".to_owned(), b"two".to_vec())]);
}

#[test]
fn analysis_survives_a_library_rebuild_and_old_records_move_over() {
    let dir = tempfile::tempdir().unwrap();
    let lib_path = dir.path().join("library.redb");
    let store = AnalysisStore::open(dir.path().join("analysis.redb")).unwrap();
    store.put("new", b"kept").unwrap();
    {
        // An older build kept analysis in the library database.
        let lib = Library::open(&lib_path).unwrap();
        let wt = lib.db.begin_write().unwrap();
        {
            let mut t = wt.open_table(ANALYSIS).unwrap();
            t.insert("old", b"moved".as_slice()).unwrap();
            t.insert("new", b"stale".as_slice()).unwrap();
        }
        wt.commit().unwrap();
        assert_eq!(store.adopt(&lib).unwrap(), 2);
        assert!(lib.legacy_analysis_records().unwrap().is_empty(), "taken over once");
        assert_eq!(store.adopt(&lib).unwrap(), 0);
    }
    // The library is thrown away and rebuilt (e.g. a newer-schema db deleted).
    std::fs::remove_file(&lib_path).unwrap();
    let _fresh = Library::open(&lib_path).unwrap();
    let mut rows = store.records().unwrap();
    rows.sort();
    assert_eq!(rows, [("new".to_owned(), b"kept".to_vec()), ("old".to_owned(), b"moved".to_vec())]);
}

#[test]
fn another_account_clears_the_mirror_but_the_same_one_keeps_it() {
    let lib = Library::in_memory().unwrap();
    // A database from before accounts were recorded belongs to whoever claims it first.
    lib.upsert_album_tracks(subsonic::AlbumWithSongs { album: album("al", "A", "X", 2000), song: vec![song("a", "al", 1, 1)] }).unwrap();
    lib.set_meta("bulk_done", Some(1)).unwrap();
    assert!(!lib.claim_account(7).unwrap());
    lib.save("newest", &vec!["al".to_owned()]).unwrap();
    assert_eq!(lib.album_count().unwrap(), 1);
    // Reconnecting as the same account changes nothing.
    assert!(!lib.claim_account(7).unwrap());
    assert_eq!(lib.track_count().unwrap(), 1);
    // Another server or user: everything from the old one goes, flags included.
    assert!(lib.claim_account(8).unwrap());
    assert_eq!(lib.album_count().unwrap(), 0);
    assert_eq!(lib.track_count().unwrap(), 0);
    assert!(lib.album_stamps().unwrap().is_empty());
    assert_eq!(lib.meta("bulk_done").unwrap(), None);
    assert_eq!(lib.saved::<Vec<String>>("newest").unwrap(), None);
    assert!(lib.check_indexes().unwrap().is_empty());
    assert_eq!(lib.schema_version().unwrap(), SCHEMA_VERSION);
    assert!(!lib.claim_account(8).unwrap());
}

#[test]
fn retaining_albums_also_drops_tracks_whose_album_is_not_stored() {
    let lib = Library::in_memory().unwrap();
    lib.upsert_album_tracks(subsonic::AlbumWithSongs { album: album("keep", "K", "X", 2000), song: vec![song("k1", "keep", 1, 1)] }).unwrap();
    lib.upsert_album_tracks(subsonic::AlbumWithSongs { album: album("gone", "G", "X", 2001), song: vec![song("g1", "gone", 1, 1)] }).unwrap();
    // A bulk sync stored songs of an album the album list never had.
    lib.store_songs(vec![song("o1", "orphan", 1, 1), song("o2", "orphan", 1, 2)]).unwrap();
    assert_eq!(lib.track_count().unwrap(), 4);

    let keep: HashSet<String> = ["keep".to_owned()].into();
    assert_eq!(lib.retain_albums(&keep).unwrap(), 1);
    let ids: Vec<_> = lib.album_tracks("keep").unwrap().into_iter().map(|t| t.id).collect();
    assert_eq!(ids, ["k1"]);
    assert_eq!(lib.track_count().unwrap(), 1, "gone's and the orphan's tracks are removed");
    assert!(lib.check_indexes().unwrap().is_empty());
}

#[test]
fn fold_spells_out_letters_unicode_does_not_decompose() {
    for (name, folded) in [
        ("Øystein Sevåg", "oystein sevag"),
        ("Łódź", "lodz"),
        ("Straße", "strasse"),
        ("STRAẞE", "strasse"),
        ("Ærøskøbing", "aeroskobing"),
        ("Đorđe Balašević", "dorde balasevic"),
        ("Œuvre", "oeuvre"),
        ("Þórir", "thorir"),
        ("Ðe", "de"),
        ("Ħal", "hal"),
        ("Ŧest", "test"),
        ("Iı", "ii"),
        ("  Björk ", "bjork"),
    ] {
        assert_eq!(fold(name), folded, "{name}");
    }
}

#[test]
fn letters_without_a_decomposition_sort_with_their_base_letter() {
    let lib = Library::in_memory().unwrap();
    lib.upsert_albums([album("1", "Z", "Zappa", 1970), album("2", "O", "Øystein Sevåg", 1989), album("3", "N", "Nina Simone", 1965)]).unwrap();
    let names: Vec<_> = lib.albums().unwrap().into_iter().map(|a| a.name).collect();
    assert_eq!(names, ["N", "O", "Z"]);
}

#[test]
fn v3_indexes_are_rebuilt_with_the_new_folding() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lib.redb");
    // A v3 database whose keys were made by the old `fold`, which left ø as is.
    {
        let lib = Library::open(&path).unwrap();
        let mut a = album("al-1", "Link", "Øystein Sevåg", 1989);
        a.song_count = 1;
        let mut s = song("s-1", "al-1", 1, 1);
        s.display_composer = Some("Øystein Sevåg".into());
        lib.upsert_album_tracks(subsonic::AlbumWithSongs { album: a.clone(), song: vec![s] }).unwrap();
        lib.upsert_artists([subsonic::Artist { id: "ar-1".into(), name: "Øystein Sevåg".into(), album_count: 1, cover_art: None, sort_name: None }]).unwrap();
        let wt = lib.db.begin_write().unwrap();
        {
            wt.open_table(META).unwrap().insert(SCHEMA_KEY, 3).unwrap();
            let mut albums = wt.open_table(ALBUMS_BY_SORT).unwrap();
            albums.retain(|_, _| false).unwrap();
            albums.insert(("øystein sevag", 1989, "link", "al-1"), ()).unwrap();
            let mut artists = wt.open_table(ARTISTS_BY_NAME).unwrap();
            artists.retain(|_, _| false).unwrap();
            artists.insert(("øystein sevag", "ar-1"), ()).unwrap();
            let mut credits = wt.open_table(CREDITS).unwrap();
            credits.retain(|_, _| false).unwrap();
            credits.insert(("øystein sevag", "al-1", "s-1"), ()).unwrap();
        }
        wt.commit().unwrap();
        assert!(!lib.check_indexes().unwrap().is_empty(), "the fixture has old keys");
    }
    let lib = Library::open(&path).unwrap();
    assert_eq!(lib.schema_version().unwrap(), SCHEMA_VERSION);
    assert_consistent(&lib);
    assert_eq!(lib.albums_crediting("Oystein Sevag").unwrap(), ["al-1"]);
    assert_eq!(lib.artists().unwrap().len(), 1);
    // Rows didn't change, so the tracks don't need fetching again.
    assert!(lib.album_stamps().unwrap().contains_key("al-1"));
    // A later sync of the same album replaces its entries instead of adding to them.
    lib.upsert_albums([album("al-1", "Link", "Øystein Sevåg", 1989)]).unwrap();
    assert_eq!(lib.albums().unwrap().len(), 1);
    assert_consistent(&lib);
}

#[test]
fn a_row_that_cannot_be_read_is_left_out_of_lists() {
    let lib = Library::in_memory().unwrap();
    lib.upsert_album_tracks(subsonic::AlbumWithSongs {
        album: album("al-1", "Good", "X", 2000),
        song: vec![song("s-1", "al-1", 1, 1), song("s-2", "al-1", 1, 2)],
    })
    .unwrap();
    lib.upsert_albums([album("al-2", "Bad", "Y", 2001)]).unwrap();
    lib.upsert_artists([subsonic::Artist { id: "ar-1".into(), name: "X".into(), album_count: 1, cover_art: None, sort_name: None }]).unwrap();
    let wt = lib.db.begin_write().unwrap();
    {
        wt.open_table(ALBUMS).unwrap().insert("al-2", [0xffu8; 3].as_slice()).unwrap();
        wt.open_table(TRACKS).unwrap().insert("s-2", [0xffu8; 3].as_slice()).unwrap();
        wt.open_table(ARTISTS).unwrap().insert("ar-1", [0xffu8; 3].as_slice()).unwrap();
    }
    wt.commit().unwrap();
    assert_eq!(lib.albums().unwrap().into_iter().map(|a| a.id).collect::<Vec<_>>(), ["al-1"]);
    assert_eq!(lib.album_tracks("al-1").unwrap().into_iter().map(|t| t.id).collect::<Vec<_>>(), ["s-1"]);
    assert_eq!(lib.all_tracks().unwrap().len(), 1);
    assert!(lib.artists().unwrap().is_empty());
}

#[test]
fn saved_copies_read_back_and_unreadable_ones_read_as_missing() {
    let lib = Library::in_memory().unwrap();
    assert_eq!(lib.saved::<Vec<String>>("newest").unwrap(), None);
    lib.save("newest", &vec!["b".to_owned(), "a".to_owned()]).unwrap();
    assert_eq!(lib.saved::<Vec<String>>("newest").unwrap(), Some(vec!["b".to_owned(), "a".to_owned()]));
    // Saved by a build with another type: missing, not an error.
    lib.save("odd", &7u8).unwrap();
    assert_eq!(lib.saved::<Vec<(String, String)>>("odd").unwrap(), None);
}

/// Writes a database as schema `version` left it: rows in that version's shape
/// and index entries made the way it made them.
fn database_at_version(path: &std::path::Path, version: u32) {
    let db = redb::Database::create(path).unwrap();
    let wt = db.begin_write().unwrap();
    {
        wt.open_table(META).unwrap().insert(SCHEMA_KEY, version).unwrap();
        let v1 = AlbumV1 {
            id: "al-1".into(),
            name: "Link".into(),
            sort: None,
            artist: Some("Øystein Sevåg".into()),
            artist_ref: None,
            year: Some(1989),
            genre: Some("New Age".into()),
            song_count: 1,
            duration_secs: 300,
            cover_art: None,
            created: None,
        };
        let t1 = TrackV1 {
            id: "s-1".into(),
            album_id: "al-1".into(),
            title: "Link".into(),
            artist: Some("Øystein Sevåg".into()),
            track: Some(1),
            disc: Some(1),
            duration_secs: Some(300),
            suffix: None,
            content_type: None,
            size: None,
            bit_rate_kbps: None,
            bit_depth: None,
            sample_rate: None,
            channels: None,
            track_gain: None,
            album_gain: None,
            track_peak: None,
            album_peak: None,
        };
        let (album, track) = match version {
            1 => (encode(&v1).unwrap(), encode(&t1).unwrap()),
            2 => (encode(&AlbumV2::from(v1)).unwrap(), encode(&TrackV2::from(t1)).unwrap()),
            3 => (encode(&AlbumV3::from(AlbumV2::from(v1))).unwrap(), encode(&TrackV3::from(TrackV2::from(t1))).unwrap()),
            v => panic!("no fixture for schema v{v}: add one here, and a step for it in `migrate`"),
        };
        wt.open_table(ALBUMS).unwrap().insert("al-1", album.as_slice()).unwrap();
        wt.open_table(TRACKS).unwrap().insert("s-1", track.as_slice()).unwrap();
        // Keys from the `fold` of v1 to v3, which left ø as it was.
        wt.open_table(ALBUMS_BY_SORT).unwrap().insert(("øystein sevag", 1989, "link", "al-1"), ()).unwrap();
        wt.open_table(TRACKS_BY_ALBUM).unwrap().insert(("al-1", 1, 1, "s-1"), ()).unwrap();
        if version >= 3 {
            wt.open_table(CREDITS).unwrap().insert(("øystein sevag", "al-1", "s-1"), ()).unwrap();
        }
    }
    wt.commit().unwrap();
}

#[test]
fn every_old_schema_version_has_a_migration() {
    for version in 1..SCHEMA_VERSION {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lib.redb");
        database_at_version(&path, version);
        let lib = Library::open(&path).unwrap_or_else(|e| panic!("schema v{version} doesn't open: {e}"));
        assert_eq!(lib.schema_version().unwrap(), SCHEMA_VERSION);
        assert_consistent(&lib);
        assert_eq!(lib.albums().unwrap()[0].name, "Link", "v{version}");
        assert_eq!(lib.album_tracks("al-1").unwrap()[0].title, "Link", "v{version}");
        assert_eq!(lib.albums_crediting("Oystein Sevag").unwrap(), ["al-1"], "v{version}");
    }
}

#[test]
fn a_schema_step_without_a_migration_is_an_error() {
    let lib = Library::in_memory().unwrap();
    let wt = lib.db.begin_write().unwrap();
    let err = migrate(&wt, SCHEMA_VERSION, SCHEMA_VERSION + 1).unwrap_err();
    assert!(matches!(err, Error::NoMigration { from } if from == SCHEMA_VERSION), "{err}");
    assert!(!err.is_damage(), "a missing step is a build bug, not a reason to move the file aside");
    assert!(matches!(migrate(&wt, 0, 1), Err(Error::NoMigration { from: 0 })));
}

fn full_artist() -> Artist {
    Artist { id: "ar-1".into(), name: "Bill Evans".into(), sort: Some("Evans, Bill".into()), album_count: 3, cover_art: Some("ar-1_0".into()) }
}

fn full_album() -> Album {
    Album {
        id: "al-1".into(),
        name: "Sunday".into(),
        sort: Some("Sunday at".into()),
        artist: Some("Bill Evans Trio".into()),
        artist_ref: Some("ar-2".into()),
        year: Some(1961),
        genre: Some("Jazz".into()),
        song_count: 5,
        duration_secs: 2400,
        cover_art: Some("al-1_0".into()),
        created: Some("2024-01-02".into()),
        labels: vec!["Riverside".into()],
        genres: vec!["Jazz".into()],
        artists: vec![ArtistLink { id: Some("ar-2".into()), name: "Bill Evans Trio".into() }],
    }
}

fn full_track() -> Track {
    Track {
        id: "s-1".into(),
        album_id: "al-1".into(),
        title: "Gloria's Step".into(),
        artist: Some("Bill Evans Trio".into()),
        artist_ref: Some("ar-2".into()),
        track: Some(1),
        disc: Some(2),
        duration_secs: Some(370),
        suffix: Some("flac".into()),
        content_type: Some("audio/flac".into()),
        size: Some(300_000),
        bit_rate_kbps: Some(900),
        bit_depth: Some(24),
        sample_rate: Some(96_000),
        channels: Some(2),
        track_gain: Some(-1.5),
        album_gain: Some(-2.0),
        track_peak: Some(0.5),
        album_peak: Some(1.0),
        composer: Some("Scott LaFaro".into()),
        contributors: vec![Contributor {
            role: "performer".into(),
            sub_role: Some("bass".into()),
            name: "Scott LaFaro".into(),
            artist_id: Some("ar-3".into()),
        }],
        cover_art: Some("s-1_0".into()),
        artists: vec![ArtistLink { id: None, name: "Bill Evans".into() }],
    }
}

/// What to do when a golden-bytes test fails.
const SHAPE_CHANGED: &str = "the stored shape of a library row changed, so rows already on disk no longer decode as this type. \
    Don't edit the bytes to match: freeze the old struct (and every type inside it) as the previous version, \
    add the next version, bump SCHEMA_VERSION and add a step to `migrate` (see the rules in model.rs). \
    Then add the new bytes as the new version's test";

#[test]
fn artist_v1_keeps_its_wire_format() {
    const BYTES: &[u8] = &[
        4, 97, 114, 45, 49, 10, 66, 105, 108, 108, 32, 69, 118, 97, 110, 115, 1, 11, 69, 118, 97, 110, 115, 44, 32, 66, 105, 108, 108, 3, 1, 6, 97, 114, 45,
        49, 95, 48,
    ];
    assert_eq!(encode(&full_artist()).unwrap(), BYTES, "{SHAPE_CHANGED}");
    assert_eq!(decode::<Artist>(BYTES).unwrap(), full_artist());
}

#[test]
fn album_v3_keeps_its_wire_format() {
    const BYTES: &[u8] = &[
        4, 97, 108, 45, 49, 6, 83, 117, 110, 100, 97, 121, 1, 9, 83, 117, 110, 100, 97, 121, 32, 97, 116, 1, 15, 66, 105, 108, 108, 32, 69, 118, 97, 110, 115,
        32, 84, 114, 105, 111, 1, 4, 97, 114, 45, 50, 1, 210, 30, 1, 4, 74, 97, 122, 122, 5, 224, 18, 1, 6, 97, 108, 45, 49, 95, 48, 1, 10, 50, 48, 50, 52, 45,
        48, 49, 45, 48, 50, 1, 9, 82, 105, 118, 101, 114, 115, 105, 100, 101, 1, 4, 74, 97, 122, 122, 1, 1, 4, 97, 114, 45, 50, 15, 66, 105, 108, 108, 32, 69,
        118, 97, 110, 115, 32, 84, 114, 105, 111,
    ];
    assert_eq!(encode(&full_album()).unwrap(), BYTES, "{SHAPE_CHANGED}");
    assert_eq!(decode::<Album>(BYTES).unwrap(), full_album());
}

#[test]
fn track_v3_keeps_its_wire_format() {
    const BYTES: &[u8] = &[
        3, 115, 45, 49, 4, 97, 108, 45, 49, 13, 71, 108, 111, 114, 105, 97, 39, 115, 32, 83, 116, 101, 112, 1, 15, 66, 105, 108, 108, 32, 69, 118, 97, 110,
        115, 32, 84, 114, 105, 111, 1, 4, 97, 114, 45, 50, 1, 1, 1, 2, 1, 242, 2, 1, 4, 102, 108, 97, 99, 1, 10, 97, 117, 100, 105, 111, 47, 102, 108, 97, 99,
        1, 224, 167, 18, 1, 132, 7, 1, 24, 1, 128, 238, 5, 1, 2, 1, 0, 0, 192, 191, 1, 0, 0, 0, 192, 1, 0, 0, 0, 63, 1, 0, 0, 128, 63, 1, 12, 83, 99, 111, 116,
        116, 32, 76, 97, 70, 97, 114, 111, 1, 9, 112, 101, 114, 102, 111, 114, 109, 101, 114, 1, 4, 98, 97, 115, 115, 12, 83, 99, 111, 116, 116, 32, 76, 97,
        70, 97, 114, 111, 1, 4, 97, 114, 45, 51, 1, 5, 115, 45, 49, 95, 48, 1, 0, 10, 66, 105, 108, 108, 32, 69, 118, 97, 110, 115,
    ];
    assert_eq!(encode(&full_track()).unwrap(), BYTES, "{SHAPE_CHANGED}");
    assert_eq!(decode::<Track>(BYTES).unwrap(), full_track());
}

/// What to do when a frozen version's golden-bytes test fails.
const FROZEN_CHANGED: &str = "a frozen row version changed shape. Rows written by older builds are decoded as this type by \
    `migrate`, so it must stay exactly as it was when that schema was current; make the change in the current version instead";

fn full_album_v1() -> AlbumV1 {
    AlbumV1 {
        id: "al-1".into(),
        name: "Sunday".into(),
        sort: Some("Sunday at".into()),
        artist: Some("Bill Evans Trio".into()),
        artist_ref: Some("ar-2".into()),
        year: Some(1961),
        genre: Some("Jazz".into()),
        song_count: 5,
        duration_secs: 2400,
        cover_art: Some("al-1_0".into()),
        created: Some("2024-01-02".into()),
    }
}

fn full_album_v2() -> AlbumV2 {
    AlbumV2 { labels: vec!["Riverside".into()], genres: vec!["Jazz".into()], ..AlbumV2::from(full_album_v1()) }
}

fn full_track_v1() -> TrackV1 {
    TrackV1 {
        id: "s-1".into(),
        album_id: "al-1".into(),
        title: "Gloria's Step".into(),
        artist: Some("Bill Evans Trio".into()),
        track: Some(1),
        disc: Some(2),
        duration_secs: Some(370),
        suffix: Some("flac".into()),
        content_type: Some("audio/flac".into()),
        size: Some(300_000),
        bit_rate_kbps: Some(900),
        bit_depth: Some(24),
        sample_rate: Some(96_000),
        channels: Some(2),
        track_gain: Some(-1.5),
        album_gain: Some(-2.0),
        track_peak: Some(0.5),
        album_peak: Some(1.0),
    }
}

fn full_track_v2() -> TrackV2 {
    TrackV2 {
        artist_ref: Some("ar-2".into()),
        composer: Some("Scott LaFaro".into()),
        contributors: vec![Contributor {
            role: "performer".into(),
            sub_role: Some("bass".into()),
            name: "Scott LaFaro".into(),
            artist_id: Some("ar-3".into()),
        }],
        cover_art: Some("s-1_0".into()),
        ..TrackV2::from(full_track_v1())
    }
}

#[test]
fn frozen_album_v1_keeps_its_wire_format() {
    const BYTES: &[u8] = &[
        4, 97, 108, 45, 49, 6, 83, 117, 110, 100, 97, 121, 1, 9, 83, 117, 110, 100, 97, 121, 32, 97, 116, 1, 15, 66, 105, 108, 108, 32, 69, 118, 97, 110, 115,
        32, 84, 114, 105, 111, 1, 4, 97, 114, 45, 50, 1, 210, 30, 1, 4, 74, 97, 122, 122, 5, 224, 18, 1, 6, 97, 108, 45, 49, 95, 48, 1, 10, 50, 48, 50, 52, 45,
        48, 49, 45, 48, 50,
    ];
    assert_eq!(encode(&full_album_v1()).unwrap(), BYTES, "{FROZEN_CHANGED}");
    assert_eq!(decode::<AlbumV1>(BYTES).unwrap(), full_album_v1());
}

#[test]
fn frozen_album_v2_keeps_its_wire_format() {
    const BYTES: &[u8] = &[
        4, 97, 108, 45, 49, 6, 83, 117, 110, 100, 97, 121, 1, 9, 83, 117, 110, 100, 97, 121, 32, 97, 116, 1, 15, 66, 105, 108, 108, 32, 69, 118, 97, 110, 115,
        32, 84, 114, 105, 111, 1, 4, 97, 114, 45, 50, 1, 210, 30, 1, 4, 74, 97, 122, 122, 5, 224, 18, 1, 6, 97, 108, 45, 49, 95, 48, 1, 10, 50, 48, 50, 52, 45,
        48, 49, 45, 48, 50, 1, 9, 82, 105, 118, 101, 114, 115, 105, 100, 101, 1, 4, 74, 97, 122, 122,
    ];
    assert_eq!(encode(&full_album_v2()).unwrap(), BYTES, "{FROZEN_CHANGED}");
    assert_eq!(decode::<AlbumV2>(BYTES).unwrap(), full_album_v2());
}

#[test]
fn frozen_track_v1_keeps_its_wire_format() {
    const BYTES: &[u8] = &[
        3, 115, 45, 49, 4, 97, 108, 45, 49, 13, 71, 108, 111, 114, 105, 97, 39, 115, 32, 83, 116, 101, 112, 1, 15, 66, 105, 108, 108, 32, 69, 118, 97, 110,
        115, 32, 84, 114, 105, 111, 1, 1, 1, 2, 1, 242, 2, 1, 4, 102, 108, 97, 99, 1, 10, 97, 117, 100, 105, 111, 47, 102, 108, 97, 99, 1, 224, 167, 18, 1,
        132, 7, 1, 24, 1, 128, 238, 5, 1, 2, 1, 0, 0, 192, 191, 1, 0, 0, 0, 192, 1, 0, 0, 0, 63, 1, 0, 0, 128, 63,
    ];
    assert_eq!(encode(&full_track_v1()).unwrap(), BYTES, "{FROZEN_CHANGED}");
    assert_eq!(decode::<TrackV1>(BYTES).unwrap(), full_track_v1());
}

#[test]
fn frozen_track_v2_keeps_its_wire_format() {
    const BYTES: &[u8] = &[
        3, 115, 45, 49, 4, 97, 108, 45, 49, 13, 71, 108, 111, 114, 105, 97, 39, 115, 32, 83, 116, 101, 112, 1, 15, 66, 105, 108, 108, 32, 69, 118, 97, 110,
        115, 32, 84, 114, 105, 111, 1, 4, 97, 114, 45, 50, 1, 1, 1, 2, 1, 242, 2, 1, 4, 102, 108, 97, 99, 1, 10, 97, 117, 100, 105, 111, 47, 102, 108, 97, 99,
        1, 224, 167, 18, 1, 132, 7, 1, 24, 1, 128, 238, 5, 1, 2, 1, 0, 0, 192, 191, 1, 0, 0, 0, 192, 1, 0, 0, 0, 63, 1, 0, 0, 128, 63, 1, 12, 83, 99, 111, 116,
        116, 32, 76, 97, 70, 97, 114, 111, 1, 9, 112, 101, 114, 102, 111, 114, 109, 101, 114, 1, 4, 98, 97, 115, 115, 12, 83, 99, 111, 116, 116, 32, 76, 97,
        70, 97, 114, 111, 1, 4, 97, 114, 45, 51, 1, 5, 115, 45, 49, 95, 48,
    ];
    assert_eq!(encode(&full_track_v2()).unwrap(), BYTES, "{FROZEN_CHANGED}");
    assert_eq!(decode::<TrackV2>(BYTES).unwrap(), full_track_v2());
}

#[test]
fn saved_copies_from_another_schema_or_shape_read_as_missing() {
    let lib = Library::in_memory().unwrap();
    let put_raw = |name: &str, bytes: &[u8]| {
        let wt = lib.db.begin_write().unwrap();
        wt.open_table(SAVED).unwrap().insert(name, bytes).unwrap();
        wt.commit().unwrap();
    };
    let value = vec!["al-1".to_owned()];
    // Untagged, as builds before the tag wrote them.
    put_raw("untagged", &encode(&value).unwrap());
    assert_eq!(lib.saved::<Vec<String>>("untagged").unwrap(), None);
    // Tagged by another schema, whose Track may have had another shape.
    let mut other = saved_tag();
    other[2] = other[2].wrapping_add(1);
    put_raw("other", &[other.as_slice(), &encode(&value).unwrap()].concat());
    assert_eq!(lib.saved::<Vec<String>>("other").unwrap(), None);
    // A type that lost a trailing field decodes, with bytes to spare.
    lib.save("wider", &(value.clone(), 7u32)).unwrap();
    assert_eq!(lib.saved::<Vec<String>>("wider").unwrap(), None);
    lib.save("same", &value).unwrap();
    assert_eq!(lib.saved::<Vec<String>>("same").unwrap(), Some(value));
}

#[test]
fn saved_values_keep_their_tag() {
    let lib = Library::in_memory().unwrap();
    lib.save("n", &1u8).unwrap();
    let rt = lib.db.begin_read().unwrap();
    let row = rt.open_table(SAVED).unwrap().get("n").unwrap().unwrap();
    // Marker, layout 1, SCHEMA_VERSION as little-endian u32, then the value.
    let [a, b, c, d] = SCHEMA_VERSION.to_le_bytes();
    assert_eq!(row.value(), [0xa5, 1, a, b, c, d, 1], "the saved row layout changed: older copies would read as missing or wrong");
}

#[test]
fn an_unchanged_artist_list_writes_nothing() {
    let lib = Library::in_memory().unwrap();
    let artist = |id: &str, name: &str| subsonic::Artist { id: id.into(), name: name.into(), sort_name: None, album_count: 1, cover_art: None };
    assert!(lib.replace_artists([artist("1", "Bill Evans"), artist("2", "Øystein Sevåg")]).unwrap());
    assert!(!lib.replace_artists([artist("2", "Øystein Sevåg"), artist("1", "Bill Evans")]).unwrap(), "same artists, another order");
    assert_eq!(lib.artists().unwrap().len(), 2);
    // Any difference writes the whole list: a field, a missing artist, a new one.
    let mut more = artist("1", "Bill Evans");
    more.album_count = 2;
    assert!(lib.replace_artists([more.clone(), artist("2", "Øystein Sevåg")]).unwrap());
    assert_eq!(lib.artists().unwrap()[0].album_count, 2);
    assert!(lib.replace_artists([more.clone()]).unwrap());
    assert!(lib.replace_artists([more, artist("3", "Nina Simone")]).unwrap());
    let names: Vec<_> = lib.artists().unwrap().into_iter().map(|a| a.name).collect();
    assert_eq!(names, ["Bill Evans", "Nina Simone"]);
}

#[test]
fn lists_come_out_in_index_order() {
    let lib = Library::in_memory().unwrap();
    // Folding, equal sort keys told apart by id, years before titles, and
    // album ids that are prefixes of each other.
    lib.upsert_albums([
        album("b", "Same", "Björk", 2001),
        album("a", "Same", "bjork", 2001),
        album("c", "Alpha", "BJORK", 2004),
        album("d", "Zed", "Øystein Sevåg", 1989),
        album("d2", "Anything", "Air", 1998),
    ])
    .unwrap();
    lib.store_songs(vec![song("t3", "d", 2, 1), song("t1", "d", 1, 9), song("t2", "d2", 1, 1), song("t0", "d", 1, 9), song("t4", "a", 1, 1)]).unwrap();
    let artist = |id: &str, name: &str| subsonic::Artist { id: id.into(), name: name.into(), sort_name: None, album_count: 1, cover_art: None };
    lib.replace_artists([artist("2", "eagles"), artist("1", "Eagles"), artist("3", "Émile"), artist("4", "Øystein")]).unwrap();

    let rt = lib.db.begin_read().unwrap();
    let album_idx: Vec<String> = rt.open_table(ALBUMS_BY_SORT).unwrap().iter().unwrap().map(|e| e.unwrap().0.value().3.to_owned()).collect();
    let track_idx: Vec<String> = rt.open_table(TRACKS_BY_ALBUM).unwrap().iter().unwrap().map(|e| e.unwrap().0.value().3.to_owned()).collect();
    let artist_idx: Vec<String> = rt.open_table(ARTISTS_BY_NAME).unwrap().iter().unwrap().map(|e| e.unwrap().0.value().1.to_owned()).collect();
    assert_eq!(lib.albums().unwrap().into_iter().map(|a| a.id).collect::<Vec<_>>(), album_idx);
    assert_eq!(lib.all_tracks().unwrap().into_iter().map(|t| t.id).collect::<Vec<_>>(), track_idx);
    assert_eq!(lib.artists().unwrap().into_iter().map(|a| a.id).collect::<Vec<_>>(), artist_idx);
    assert_eq!(album_idx, ["d2", "a", "b", "c", "d"]);
    assert_eq!(track_idx, ["t4", "t0", "t1", "t3", "t2"]);
}

/// An album with two credited tracks, then its album row and its first
/// track's row overwritten with bytes that don't decode.
fn library_with_unreadable_rows() -> Library {
    let lib = Library::in_memory().unwrap();
    let mut a = song("s-1", "al-1", 1, 1);
    a.display_composer = Some("Cole Porter".into());
    let mut b = song("s-2", "al-1", 1, 2);
    b.display_composer = Some("Gershwin".into());
    lib.upsert_album_tracks(subsonic::AlbumWithSongs { album: album("al-1", "A", "X", 2000), song: vec![a, b] }).unwrap();
    let wt = lib.db.begin_write().unwrap();
    {
        wt.open_table(ALBUMS).unwrap().insert("al-1", [0xffu8; 3].as_slice()).unwrap();
        wt.open_table(TRACKS).unwrap().insert("s-1", [0xffu8; 3].as_slice()).unwrap();
    }
    wt.commit().unwrap();
    lib
}

#[test]
fn a_write_over_an_undecodable_row_succeeds_and_leaves_indexes_consistent() {
    let lib = library_with_unreadable_rows();
    // A new year moves the album's sort key, and a new track number the
    // track's: the old entries can only be found by id.
    lib.upsert_albums([album("al-1", "A", "X", 2001)]).unwrap();
    let mut moved = song("s-1", "al-1", 2, 7);
    moved.display_composer = Some("Ellington".into());
    lib.store_songs(vec![moved]).unwrap();
    assert_consistent(&lib);
    assert_eq!(lib.albums().unwrap()[0].year, Some(2001));
    assert_eq!(lib.album_tracks("al-1").unwrap().into_iter().map(|t| t.id).collect::<Vec<_>>(), ["s-2", "s-1"]);
    assert!(lib.albums_crediting("Cole Porter").unwrap().is_empty());
    assert_eq!(lib.albums_crediting("Ellington").unwrap(), ["al-1"]);

    // A whole-album resync goes through the same rows.
    let lib = library_with_unreadable_rows();
    lib.upsert_album_tracks(subsonic::AlbumWithSongs { album: album("al-1", "A", "X", 1999), song: vec![song("s-1", "al-1", 1, 1)] }).unwrap();
    assert_consistent(&lib);
    assert_eq!(lib.album_tracks("al-1").unwrap().len(), 1);
}

#[test]
fn removing_an_undecodable_row_succeeds_and_leaves_indexes_consistent() {
    let lib = library_with_unreadable_rows();
    assert!(lib.remove_album("al-1").unwrap());
    assert_consistent(&lib);
    assert_eq!(lib.album_count().unwrap(), 0);
    assert_eq!(lib.track_count().unwrap(), 0);
    assert!(lib.albums_crediting("Cole Porter").unwrap().is_empty());

    let lib = library_with_unreadable_rows();
    assert_eq!(lib.retain_albums(&HashSet::new()).unwrap(), 1);
    assert_consistent(&lib);
    assert_eq!(lib.track_count().unwrap(), 0);
}
