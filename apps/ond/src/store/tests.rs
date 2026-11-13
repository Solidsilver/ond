use super::*;

fn acc(last: u8) -> AccountId {
    let s = format!("00000000-0000-4000-8000-0000000000{last:02x}");
    s.parse().unwrap()
}

#[test]
fn stars_and_ratings_round_trip_and_persist_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("ond.redb");
    let a = acc(1);
    let b = acc(2);
    let t1 = ItemKey::new(a, "t-1");
    let t2 = ItemKey::new(a, "t-2");
    let al1 = ItemKey::new(a, "al-1");
    let ar1 = ItemKey::new(a, "ar-1");
    let other = ItemKey::new(b, "t-1");

    {
        let store = OndStore::open(&db_path).unwrap();
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
        store.set_starred(StarKind::Song, &t1, true, None).unwrap();
        store.set_starred(StarKind::Song, &t2, true, None).unwrap();
        store.set_starred(StarKind::Song, &t2, false, None).unwrap();
        store.set_starred(StarKind::Album, &al1, true, None).unwrap();
        store.set_starred(StarKind::Artist, &ar1, true, Some("Bill Evans")).unwrap();
        store.set_starred(StarKind::Song, &other, true, None).unwrap();

        store.set_rating(&t1, 5).unwrap();
        store.set_rating(&t2, 3).unwrap();
        store.set_rating(&t2, 0).unwrap();
        store.set_rating(&other, 2).unwrap();
    }

    let reopened = OndStore::open(&db_path).unwrap();
    assert!(reopened.is_starred(StarKind::Song, &t1).unwrap());
    assert!(!reopened.is_starred(StarKind::Song, &t2).unwrap());
    assert!(reopened.is_starred(StarKind::Album, &al1).unwrap());
    assert!(reopened.is_starred(StarKind::Artist, &ar1).unwrap());

    let starred = reopened.starred(a).unwrap();
    assert_eq!(starred.songs, ["t-1"]);
    assert_eq!(starred.albums, ["al-1"]);
    assert_eq!(starred.artists, vec![("ar-1".to_owned(), Some("Bill Evans".to_owned()))]);

    assert_eq!(reopened.rating(&t1).unwrap(), 5);
    assert_eq!(reopened.rating(&t2).unwrap(), 0);
    assert_eq!(reopened.ratings(a).unwrap(), vec![("t-1".to_owned(), 5)]);
    assert_eq!(reopened.ratings(b).unwrap(), vec![("t-1".to_owned(), 2)]);
}

#[test]
fn playlists_support_create_rename_edit_reorder_delete_and_mixed_sources() {
    let store = OndStore::in_memory().unwrap();
    let local = acc(1);
    let nav = acc(2);
    let k1 = ItemKey::new(local, "l-1");
    let k2 = ItemKey::new(nav, "n-2");
    let k3 = ItemKey::new(local, "l-3");

    let created = store.create_playlist("Late Night", &[k1.clone(), k2.clone()]).unwrap();
    assert_eq!(created.name, "Late Night");
    assert_eq!(created.entries, [k1.clone(), k2.clone()]);
    assert!(created.created_at <= created.changed_at);

    assert!(store.add_to_playlist(&created.id, std::slice::from_ref(&k3)).unwrap());
    assert_eq!(store.playlist(&created.id).unwrap().unwrap().entries, [k1.clone(), k2.clone(), k3.clone()]);

    assert!(store.rename_playlist(&created.id, "Quiet Jazz").unwrap());
    assert!(store.replace_playlist_entries(&created.id, vec![k3.clone(), k1.clone(), k2.clone()]).unwrap());
    assert!(store.remove_from_playlist(&created.id, &[1, 99]).unwrap());

    let fetched = store.playlist(&created.id).unwrap().unwrap();
    assert_eq!(fetched.name, "Quiet Jazz");
    assert_eq!(fetched.entries, [k3, k2]);

    let second = store.create_playlist("Ambient", &[]).unwrap();
    let listed: Vec<_> = store.playlists().unwrap().into_iter().map(|p| p.name).collect();
    assert_eq!(listed, ["Ambient", "Quiet Jazz"]);

    assert!(store.delete_playlist(&second.id).unwrap());
    assert!(!store.delete_playlist(&second.id).unwrap());
    assert_eq!(store.playlists().unwrap().len(), 1);
}

#[test]
fn apply_moves_migrates_stars_ratings_and_playlist_entries_for_that_account_only() {
    let store = OndStore::in_memory().unwrap();
    let a = acc(1);
    let b = acc(2);

    // Include a swap/chain (1 -> 2, 2 -> 3) to verify order independence.
    let a1 = ItemKey::new(a, "old-1");
    let a2 = ItemKey::new(a, "old-2");
    let b1 = ItemKey::new(b, "old-1");

    store.set_starred(StarKind::Song, &a1, true, None).unwrap();
    store.set_starred(StarKind::Song, &b1, true, None).unwrap();
    store.set_rating(&a1, 4).unwrap();
    store.set_rating(&a2, 5).unwrap();
    store.set_rating(&b1, 2).unwrap();

    let pl = store.create_playlist("Walk", &[a1.clone(), b1.clone(), a2.clone()]).unwrap();

    let moved =
        store.apply_moves(a, &[TrackMove { from: "old-1".into(), to: "old-2".into() }, TrackMove { from: "old-2".into(), to: "new-3".into() }]).unwrap();
    assert_eq!(moved, 5);

    assert!(!store.is_starred(StarKind::Song, &a1).unwrap());
    assert!(store.is_starred(StarKind::Song, &ItemKey::new(a, "old-2")).unwrap());
    assert!(store.is_starred(StarKind::Song, &b1).unwrap(), "other account stays untouched");

    assert_eq!(store.rating(&a1).unwrap(), 0);
    assert_eq!(store.rating(&ItemKey::new(a, "old-2")).unwrap(), 4);
    assert_eq!(store.rating(&ItemKey::new(a, "new-3")).unwrap(), 5);
    assert_eq!(store.rating(&b1).unwrap(), 2);

    let updated = store.playlist(&pl.id).unwrap().unwrap();
    assert_eq!(updated.entries, [ItemKey::new(a, "old-2"), b1, ItemKey::new(a, "new-3")]);
}

#[test]
fn removing_an_account_drops_its_stars_ratings_and_playlist_entries() {
    let store = OndStore::in_memory().unwrap();
    let doomed = acc(1);
    let kept = acc(2);

    store.set_starred(StarKind::Song, &ItemKey::new(doomed, "s1"), true, None).unwrap();
    store.set_starred(StarKind::Album, &ItemKey::new(doomed, "al1"), true, None).unwrap();
    store.set_starred(StarKind::Artist, &ItemKey::new(doomed, "ar1"), true, Some("Doomed")).unwrap();
    store.set_rating(&ItemKey::new(doomed, "s1"), 5).unwrap();

    store.set_starred(StarKind::Song, &ItemKey::new(kept, "s1"), true, None).unwrap();
    store.set_rating(&ItemKey::new(kept, "s1"), 4).unwrap();

    let pl = store.create_playlist("Mixed", &[ItemKey::new(doomed, "s1"), ItemKey::new(kept, "s1")]).unwrap();

    store.remove_account(doomed).unwrap();

    let gone_stars = store.starred(doomed).unwrap();
    assert!(gone_stars.songs.is_empty() && gone_stars.albums.is_empty() && gone_stars.artists.is_empty());
    assert!(store.ratings(doomed).unwrap().is_empty());

    assert!(store.is_starred(StarKind::Song, &ItemKey::new(kept, "s1")).unwrap());
    assert_eq!(store.rating(&ItemKey::new(kept, "s1")).unwrap(), 4);

    let pl_after = store.playlist(&pl.id).unwrap().unwrap();
    assert_eq!(pl_after.entries, [ItemKey::new(kept, "s1")]);
}

#[test]
fn m3u_export_formats_resolved_tracks_and_sanitizes_newlines() {
    let store = OndStore::in_memory().unwrap();
    let a = acc(1);
    let k1 = ItemKey::new(a, "t1");
    let k2 = ItemKey::new(a, "t2");
    let k_missing = ItemKey::new(a, "gone");
    let pl = store.create_playlist("Road", &[k1.clone(), k_missing, k2.clone()]).unwrap();

    let m3u = store
        .export_m3u(&pl.id, |key| match key.item.as_str() {
            "t1" => Some(M3uTrack {
                location: "/Music/01 Waltz\n.flac".into(),
                title: Some("Waltz for Debby".into()),
                artist: Some("Bill Evans\r\nTrio".into()),
                duration_secs: Some(272),
            }),
            "t2" => Some(M3uTrack { location: "/Music/02.flac".into(), title: Some("Blue in Green".into()), artist: None, duration_secs: None }),
            _ => None,
        })
        .unwrap()
        .unwrap();

    assert_eq!(m3u, "#EXTM3U\n#EXTINF:272,Bill EvansTrio - Waltz for Debby\n/Music/01 Waltz.flac\n#EXTINF:-1,Blue in Green\n/Music/02.flac\n");
    assert!(store.export_m3u("missing", |_| None).unwrap().is_none());
}

#[test]
fn golden_postcard_bytes_stay_readable_across_builds() {
    let star_bytes = [0x01, 0x04, b'B', b'i', b'l', b'l', 0x54];
    let rating_bytes = [0x05, 0x14];
    let pl_bytes = [2, b'p', b'1', 3, b'M', b'i', b'x', 1, 1, b'a', 2, b't', b'1', 2, 4];

    assert_eq!(encode(&StarredV1 { name: Some("Bill".into()), starred_at: 42 }).unwrap(), star_bytes);
    assert_eq!(encode(&RatingV1 { rating: 5, rated_at: 10 }).unwrap(), rating_bytes);
    assert_eq!(
        encode(&PlaylistV1 {
            id: "p1".into(),
            name: "Mix".into(),
            entries: vec![PlaylistEntryV1 { account: "a".into(), item: "t1".into() }],
            created_at: 1,
            changed_at: 2,
        })
        .unwrap(),
        pl_bytes
    );

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ond.redb");
    let a = acc(7);
    let a_str = a.to_string();
    let pl_real = encode(&PlaylistV1 {
        id: "pl-1".into(),
        name: "Saved".into(),
        entries: vec![PlaylistEntryV1 { account: a_str.clone(), item: "s-1".into() }],
        created_at: 100,
        changed_at: 200,
    })
    .unwrap();
    {
        let db = Database::create(&path).unwrap();
        let wt = db.begin_write().unwrap();
        {
            wt.open_table(META).unwrap().insert(SCHEMA_KEY, 1).unwrap();
            wt.open_table(STARS).unwrap().insert((a_str.as_str(), KIND_ARTIST, "ar-1"), star_bytes.as_slice()).unwrap();
            wt.open_table(RATINGS).unwrap().insert((a_str.as_str(), "s-1"), rating_bytes.as_slice()).unwrap();
            wt.open_table(PLAYLISTS).unwrap().insert("pl-1", pl_real.as_slice()).unwrap();
        }
        wt.commit().unwrap();
    }

    let store = OndStore::open(&path).unwrap();
    assert_eq!(store.starred(a).unwrap().artists, vec![("ar-1".to_owned(), Some("Bill".to_owned()))]);
    assert_eq!(store.rating(&ItemKey::new(a, "s-1")).unwrap(), 5);
    let pl = store.playlist("pl-1").unwrap().unwrap();
    assert_eq!(pl.name, "Saved");
    assert_eq!(pl.entries, [ItemKey::new(a, "s-1")]);
}

#[test]
fn corrupt_store_is_moved_aside_and_newer_schema_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ond.redb");
    std::fs::write(&path, vec![0x5a; 4096]).unwrap();
    let (store, aside) = OndStore::open_or_recover(&path).unwrap();
    assert!(aside.is_some());
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    drop(store);

    let db = Database::create(&path).unwrap();
    let wt = db.begin_write().unwrap();
    wt.open_table(META).unwrap().insert(SCHEMA_KEY, SCHEMA_VERSION + 1).unwrap();
    wt.commit().unwrap();
    drop(db);

    assert!(matches!(OndStore::open_or_recover(&path), Err(Error::NewerSchema { .. })));
}
