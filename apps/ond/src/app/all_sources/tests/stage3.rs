//! Stage 3 exercises the app actions, scoped bridges and real store handles.

use super::*;
use crate::app::{Keep, OfflineAction, PrefsAction};
use crate::favorites::{Starred, UserMeta};
use crate::settings::PreferCopy;
use offline::Pin;

fn heart(app: &mut OndApp, kind: StarKind, source: AccountId) {
    let id = key(source, "1");
    let track = (kind == StarKind::Song).then(|| app.data.track_copy(&id).cloned()).flatten().map(Box::new);
    app.apply(Action::ToggleStar { kind, id, track, name: None });
}

fn meta(app: &mut OndApp, source: AccountId, starred: bool, rating: u8) {
    let mut track = app.data.track_copy(&key(source, "1")).unwrap().clone();
    track.id = "1".into();
    track.album_id = "1".into();
    inject(
        app,
        Msg::Source {
            account: source,
            session: 0,
            message: Box::new(Msg::UserMeta {
                meta: UserMeta {
                    starred: vec![(StarKind::Song, "1".into(), starred), (StarKind::Album, "1".into(), starred)],
                    ratings: vec![("1".into(), rating)],
                },
                tracks: vec![track],
            }),
        },
    );
}

fn has_query(fake: &JsonFake, path: &str, query: &str) -> bool {
    fake.seen().iter().filter(|r| r.path() == path).any(|r| r.target.split('?').nth(1).unwrap().split('&').any(|q| q == query))
}

#[test]
fn a_star_on_either_copy_lights_both_and_unstarring_clears_both_servers() {
    let dir = tempfile::tempdir().unwrap();
    let one = server();
    let two = server();
    let mut app = copy_playback_rig(dir.path(), &one, &two);
    meta(&mut app, account(2), true, 0);
    for kind in [StarKind::Song, StarKind::Album] {
        assert!(app.favorites.is_on(kind, &key(account(1), "1")));
        heart(&mut app, kind, account(1));
        assert!(!app.favorites.is_on(kind, &key(account(2), "1")));
    }
    pump(&mut app, |_| one.count("/rest/unstar") == 2 && two.count("/rest/unstar") == 2);
    for fake in [&one, &two] {
        assert!(has_query(fake, "/rest/unstar", "id=1"));
        assert!(has_query(fake, "/rest/unstar", "albumId=1"));
    }
    for kind in [StarKind::Song, StarKind::Album] {
        heart(&mut app, kind, account(2));
    }
    pump(&mut app, |_| one.count("/rest/star") == 2 && two.count("/rest/star") == 2);
    assert_eq!(app.favorites.view.songs.len(), 1);
    assert_eq!(app.favorites.view.albums, [key(account(1), "1")]);
    combine(&mut app, false);
    assert_eq!(app.favorites.view.songs.len(), 2);
    assert_eq!(app.favorites.view.albums.len(), 2);
}

#[test]
fn a_source_without_stars_or_ratings_writes_onds_store_for_its_copy() {
    let dir = tempfile::tempdir().unwrap();
    let one = server();
    let two = server();
    let mut app = copy_playback_rig(dir.path(), &one, &two);
    let mut source = crate::source::fake::Fake::new(crate::source::Capabilities::local());
    source.account = account(2);
    app.session.accounts.get_mut(&account(2)).unwrap().source = Some(Arc::new(source));
    heart(&mut app, StarKind::Song, account(1));
    heart(&mut app, StarKind::Album, account(1));
    app.apply(Action::Rate(key(account(1), "1"), 4));
    let song = TrackKey::new(account(2), "1");
    pump(&mut app, |app| {
        app.bridge.ond_store().is_starred(StarKind::Song, &song).unwrap()
            && app.bridge.ond_store().is_starred(StarKind::Album, &song).unwrap()
            && app.bridge.ond_store().rating(&song).unwrap() == 4
            && one.count("/rest/setRating") == 1
    });
    assert_eq!(one.count("/rest/star"), 2);
    assert_eq!(two.count("/rest/star") + two.count("/rest/setRating"), 0);
    heart(&mut app, StarKind::Song, account(2));
    heart(&mut app, StarKind::Album, account(2));
    pump(&mut app, |app| {
        !app.bridge.ond_store().is_starred(StarKind::Song, &song).unwrap()
            && !app.bridge.ond_store().is_starred(StarKind::Album, &song).unwrap()
            && one.count("/rest/unstar") == 2
    });
}

#[test]
fn ratings_follow_the_preferred_copy_and_a_write_updates_every_copy() {
    let dir = tempfile::tempdir().unwrap();
    let one = server();
    let two = server();
    let mut app = copy_playback_rig(dir.path(), &one, &two);
    meta(&mut app, account(1), false, 2);
    meta(&mut app, account(2), false, 5);
    assert_eq!(app.favorites.rating(&key(account(2), "1")), 2);
    app.prefs.edit(|s| s.sources.reverse());
    app.project_sources();
    assert_eq!(app.favorites.rating(&key(account(1), "1")), 5);
    app.apply(Action::Prefs(PrefsAction::PreferCopy(PreferCopy::BestQuality)));
    assert_eq!(app.favorites.rating(&key(account(2), "1")), 2);
    app.apply(Action::Rate(key(account(2), "1"), 3));
    pump(&mut app, |_| one.count("/rest/setRating") == 1 && two.count("/rest/setRating") == 1);
    for fake in [&one, &two] {
        assert!(has_query(fake, "/rest/setRating", "id=1"));
        assert!(has_query(fake, "/rest/setRating", "rating=3"));
    }
    assert_eq!(app.favorites.rating(&key(account(1), "1")), 3);
    combine(&mut app, false);
    assert_eq!(app.favorites.rating(&key(account(2), "1")), 3);
}

#[test]
fn stars_ratings_and_counts_keep_their_source_behavior_with_combining_off() {
    let dir = tempfile::tempdir().unwrap();
    let one = server();
    let two = server();
    let mut app = copy_playback_rig(dir.path(), &one, &two);
    combine(&mut app, false);
    heart(&mut app, StarKind::Song, account(2));
    app.apply(Action::Rate(key(account(2), "1"), 4));
    pump(&mut app, |_| two.count("/rest/star") == 1 && two.count("/rest/setRating") == 1);
    assert_eq!(one.count("/rest/star") + one.count("/rest/setRating"), 0);
    assert!(!app.favorites.is_on(StarKind::Song, &key(account(1), "1")));
    assert_eq!(app.favorites.rating(&key(account(1), "1")), 0);
    for (a, n) in [(account(1), 3), (account(2), 8)] {
        inject(
            &mut app,
            Msg::Source {
                account: a,
                session: 0,
                message: Box::new(Msg::SongStats { track_id: "1".into(), stats: Some(super::super::super::SongStats { plays: Some(n), last_played: None }) }),
            },
        );
    }
    assert_eq!(app.song_stats[&key(account(1), "1")].as_ref().unwrap().plays, Some(3));
    combine(&mut app, true);
    assert_eq!(app.song_stats[&key(account(1), "1")].as_ref().unwrap().plays, Some(11));
    combine(&mut app, false);
    assert_eq!(app.song_stats[&key(account(2), "1")].as_ref().unwrap().plays, Some(8));
}

fn count_server(plays: u64) -> JsonFake {
    JsonFake::start(move |_| {
        Answer::ok(json!({"subsonic-response":{"status":"ok","version":"1.16.1","song":{"id":"1","title":"Song","playCount":plays}}}).to_string())
    })
}

#[test]
fn requesting_stats_fetches_each_copy_once_and_shows_the_sum_for_either_key() {
    let dir = tempfile::tempdir().unwrap();
    let one = count_server(7);
    let two = count_server(11);
    let mut app = copy_playback_rig(dir.path(), &one, &two);
    app.apply(Action::NeedSongStats(key(account(2), "1")));
    pump(&mut app, |a| a.song_stats.get(&key(account(1), "1")).and_then(Option::as_ref).is_some_and(|s| s.plays == Some(18)));
    app.apply(Action::NeedSongStats(key(account(1), "1")));
    assert_eq!(app.song_stats[&key(account(2), "1")].as_ref().unwrap().plays, Some(18));
    for fake in [&one, &two] {
        assert_eq!(fake.count("/rest/getSong"), 1);
        assert!(has_query(fake, "/rest/getSong", "id=1"));
        assert_eq!(fake.count("/rest/scrobble"), 0);
    }
}

fn download_stores(app: &mut OndApp, dir: &std::path::Path) {
    let cache = dir.join("download.flac");
    std::fs::write(&cache, b"fLaC").unwrap();
    for a in [account(1), account(2)] {
        let mut settings = app.prefs.snapshot();
        settings.make_primary(a);
        let open = app.downloads.open.clone();
        let mut downloads = super::super::super::Downloads::start(dir.join("pins"), &settings, open, &app.multi.bridges[&a], &mut Vec::new()).unwrap();
        let file = cache.clone();
        downloads.store.as_ref().unwrap().set_cache_lookup(Arc::new(move |_| Some(file.clone())));
        downloads.refresh();
        if app.downloads.owner == Some(a) {
            app.downloads = downloads;
        } else {
            app.multi.downloads.insert(a, downloads);
        }
    }
}

fn keep_song(app: &mut OndApp, a: AccountId, n: &str, on: bool) {
    let track = app.data.track_copy(&key(a, n)).unwrap().clone();
    app.apply(Action::Offline(OfflineAction::Keep(Keep::Tracks(vec![track]), on)));
}

fn pin_source_song(app: &OndApp, a: AccountId, n: &str) {
    let track = app.data.track_copy(&key(a, n)).unwrap();
    let id = track.id.clone();
    app.downloads_for(a).unwrap().store.as_ref().unwrap().pin(
        Pin::Track(id.clone()),
        &track.title,
        vec![offline::TrackRef::new(id, track.suffix.clone(), None)],
    );
}

#[test]
fn pinning_chooses_only_the_preferred_copy_and_removing_drops_all_pinned_copies() {
    let dir = tempfile::tempdir().unwrap();
    let one = server();
    let two = server();
    let mut app = copy_playback_rig(dir.path(), &one, &two);
    download_stores(&mut app, dir.path());
    app.prefs.edit(|s| s.sources.reverse());
    app.project_sources();
    keep_song(&mut app, account(1), "1", true);
    let first = app.downloads_for(account(1)).unwrap().store.as_ref().unwrap();
    let second = app.downloads_for(account(2)).unwrap().store.as_ref().unwrap();
    assert!(!first.is_pinned(&Pin::Track(key(account(1), "1"))));
    assert!(second.is_pinned(&Pin::Track(key(account(2), "1"))));
    app.apply(Action::Prefs(PrefsAction::PreferCopy(PreferCopy::BestQuality)));
    keep_song(&mut app, account(2), "1", true);
    assert!(app.downloads_for(account(1)).unwrap().store.as_ref().unwrap().is_pinned(&Pin::Track(key(account(1), "1"))));
    keep_song(&mut app, account(1), "1", false);
    for a in [account(1), account(2)] {
        assert!(!app.downloads_for(a).unwrap().store.as_ref().unwrap().is_pinned(&Pin::Track(key(a, "1"))));
    }
    combine(&mut app, false);
    keep_song(&mut app, account(2), "1", true);
    assert!(!app.downloads_for(account(1)).unwrap().store.as_ref().unwrap().is_pinned(&Pin::Track(key(account(1), "1"))));
    assert!(app.downloads_for(account(2)).unwrap().store.as_ref().unwrap().is_pinned(&Pin::Track(key(account(2), "1"))));
}

#[test]
fn an_album_download_is_complete_with_mixed_copies_and_appears_once_offline() {
    let dir = tempfile::tempdir().unwrap();
    let one = server();
    let two = server();
    let mut app = copy_playback_rig(dir.path(), &one, &two);
    inject_copy(&mut app, account(1), "Record", Some("release"), &[1, 2]);
    inject_copy(&mut app, account(2), "Record", Some("release"), &[1, 2]);
    download_stores(&mut app, dir.path());
    pin_source_song(&app, account(1), "1");
    pin_source_song(&app, account(2), "2");
    testkit::wait_until("mixed pinned copies", || {
        [(account(1), "1"), (account(2), "2")].iter().all(|(a, n)| app.downloads_for(*a).unwrap().store.as_ref().unwrap().snapshot().is_done(&key(*a, n)))
    });
    app.poll_downloads();
    let album = key(account(1), "1");
    let mut view = app.view();
    assert!(view.album_pin(&album).unwrap().complete());
    let other = key(account(2), "1");
    assert!(view.album_pin(&other).unwrap().complete());
    assert_eq!(view.downloads.snap.pins.iter().filter(|p| matches!(&p.pin, Pin::Album(_))).count(), 1);
    assert!(matches!(view.download_state(view.data.track(&key(account(1), "2")).unwrap()), Some(offline::TrackState::Done { .. })));
    assert!(view.track_pin(view.data.track(&key(account(1), "2")).unwrap()).is_some());
    let ctx = egui::Context::default();
    let mut output = ctx.run_ui(Default::default(), |ui| view.offline_page(ui));
    output.textures_delta.clear();
    app.apply(Action::Offline(OfflineAction::Pause(true)));
    app.poll_downloads();
    assert!(app.view().downloads.snap.paused);
    for a in [account(1), account(2)] {
        assert!(app.downloads_for(a).unwrap().snap.paused);
    }
    app.apply(Action::Offline(OfflineAction::Pause(false)));
    app.poll_downloads();
    assert!(!app.view().downloads.snap.paused);
    app.apply(Action::Offline(OfflineAction::Unpin(Pin::Album(album))));
    app.poll_downloads();
    assert!(app.view().downloads.snap.pins.is_empty());
}

#[test]
fn downloading_a_combined_album_pins_preferred_songs_including_its_partial_union() {
    let dir = tempfile::tempdir().unwrap();
    let one = server();
    let two = server();
    let mut app = copy_playback_rig(dir.path(), &one, &two);
    inject_copy(&mut app, account(1), "Record", Some("release"), &[1]);
    inject_copy(&mut app, account(2), "Record", Some("release"), &[1, 2]);
    download_stores(&mut app, dir.path());
    app.apply(Action::Offline(OfflineAction::Keep(Keep::Album(key(account(1), "1")), true)));
    for (a, n, pinned) in [(account(1), "1", true), (account(2), "1", false), (account(2), "2", true)] {
        assert_eq!(app.downloads_for(a).unwrap().store.as_ref().unwrap().is_pinned(&Pin::Track(key(a, n))), pinned);
    }
}

#[test]
fn favorite_rows_use_the_union_once_even_when_only_the_non_lead_was_starred() {
    let dir = tempfile::tempdir().unwrap();
    let one = server();
    let two = server();
    let mut app = copy_playback_rig(dir.path(), &one, &two);
    let mut track = app.data.track_copy(&key(account(2), "1")).unwrap().clone();
    track.id = "1".into();
    track.album_id = "1".into();
    inject(
        &mut app,
        Msg::Source {
            account: account(2),
            session: 0,
            message: Box::new(Msg::Starred { starred: Starred { songs: vec![track], albums: vec!["1".into()], artists: Vec::new() }, ratings: Vec::new() }),
        },
    );
    assert_eq!(app.favorites.view.songs[0].id, key(account(1), "1"));
    assert_eq!(app.favorites.view.albums, [key(account(1), "1")]);
    app.apply_scope(ScopeAction::One(account(1)));
    assert!(!app.favorites.is_on(StarKind::Song, &key(account(1), "1")));
    assert!(app.favorites.view.songs.is_empty());
    app.apply_scope(ScopeAction::All);
    assert_eq!(app.favorites.view.songs.len(), 1);
}

#[test]
fn a_rejected_copy_restores_its_own_prior_star_and_rating() {
    let dir = tempfile::tempdir().unwrap();
    let one = server();
    let two = JsonFake::start(|_| {
        Answer::ok(json!({"subsonic-response":{"status":"failed","version":"1.16.1","error":{"code":50,"message":"Denied by copy"}}}).to_string())
    });
    let mut app = copy_playback_rig(dir.path(), &one, &two);
    meta(&mut app, account(1), true, 2);
    meta(&mut app, account(2), false, 5);
    heart(&mut app, StarKind::Song, account(1));
    app.apply(Action::Rate(key(account(1), "1"), 3));
    pump(&mut app, |a| {
        a.ui.toasts.texts().iter().any(|s| s.contains("Couldn't save the heart")) && a.ui.toasts.texts().iter().any(|s| s.contains("Couldn't save the rating"))
    });
    assert!(!app.favorites.is_on(StarKind::Song, &key(account(1), "1")));
    assert_eq!(app.favorites.rating(&key(account(1), "1")), 3);
    app.apply_scope(ScopeAction::One(account(2)));
    assert!(!app.favorites.is_on(StarKind::Song, &key(account(2), "1")));
    assert_eq!(app.favorites.rating(&key(account(2), "1")), 5);
}
