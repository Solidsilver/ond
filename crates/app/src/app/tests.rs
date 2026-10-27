//! Headless frames of the main window with a populated library, queue and
//! palette: catches layout panics and checks the views wire up to actions.
use super::*;
use crate::nav::FilterKind;

struct NullOutput;

impl audio::OutputBackend for NullOutput {
    fn name(&self) -> &str {
        "null"
    }
    fn device_caps(&mut self) -> Option<audio::DeviceCaps> {
        None
    }
    fn open(&mut self, _: audio::StreamFormat, _: audio::RingInput, _: Arc<audio::Status>) -> Result<(), audio::OutputError> {
        Ok(())
    }
    fn drained(&self) -> bool {
        true
    }
    fn close(&mut self) {}
}

fn track(id: &str, album: &str, n: u32) -> Track {
    Track {
        id: id.into(),
        album_id: album.into(),
        title: format!("Track {n} with a fairly long title that has to be cut"),
        artist: Some("Bill Evans Trio".into()),
        artist_ref: None,
        track: Some(n),
        disc: Some(1 + n / 8),
        duration_secs: Some(240 + n),
        suffix: Some("flac".into()),
        content_type: None,
        size: None,
        bit_rate_kbps: None,
        bit_depth: Some(16),
        sample_rate: Some(44_100),
        channels: Some(2),
        track_gain: None,
        album_gain: None,
        track_peak: None,
        album_peak: None,
        composer: Some("Scott LaFaro".into()),
        contributors: vec![
            library::Contributor { role: "performer".into(), sub_role: Some("piano".into()), name: "Bill Evans".into(), artist_id: None },
            library::Contributor { role: "composer".into(), sub_role: None, name: "Scott LaFaro".into(), artist_id: None },
        ],
        cover_art: None,
        artists: Vec::new(),
    }
}

fn app(ctx: &egui::Context, dir: &std::path::Path) -> OndApp {
    let paths = Paths {
        config_file: dir.join("settings.json"),
        library_db: dir.join("library.redb"),
        stream_cache: dir.join("streams"),
        art_cache: dir.join("art"),
        offline: dir.join("offline"),
    };
    let mut app = OndApp::build(ctx, paths, Box::new(NullOutput)).unwrap();
    app.login = None;
    let albums: Vec<Album> = (0..60)
        .map(|i| Album {
            id: format!("al-{i}"),
            name: format!("Album {i}"),
            sort: None,
            artist: Some(if i % 3 == 0 { "Bill Evans Trio".into() } else { format!("Artist {}", i % 7) }),
            artist_ref: None,
            year: Some(1955 + i),
            genre: Some("Jazz".into()),
            song_count: 20,
            duration_secs: 3000,
            cover_art: Some(format!("al-{i}")),
            created: Some(format!("2024-01-{:02}", 1 + i % 28)),
            labels: vec!["Riverside".into()],
            genres: vec!["Jazz".into(), "Cool Jazz".into()],
            artists: Vec::new(),
        })
        .collect();
    app.data.set_albums(albums);
    let tracks: Vec<Track> = (1..=20).map(|n| track(&format!("t-{n}"), "al-0", n)).collect();
    app.data.set_all_tracks(tracks.clone());
    app.rebuild_search();
    app.data.playlists = vec![subsonic::Playlist { id: "pl-1".into(), name: "Late night".into(), song_count: 3, duration: 900, cover_art: None, owner: None }];
    app.data.playlist_tracks.insert("pl-1".into(), Arc::new(tracks[..3].to_vec()));
    app.data.credited.insert("Scott LaFaro".into(), vec!["al-1".into(), "al-0".into()]);
    app.player.queue.play_now(tracks.clone(), 2, Source::Album { id: "al-0".into(), name: "Album 0".into() });
    app.player.queue.play_next(tracks[..2].to_vec());
    app.player.queue.add(tracks[5..15].to_vec());
    app
}

fn frame(ctx: &egui::Context, app: &mut OndApp, events: Vec<egui::Event>) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1360.0, 860.0))),
        events,
        ..Default::default()
    };
    let mut out = ctx.run_ui(input, |ui| app.main_ui(ui));
    // No renderer here to upload textures to.
    out.textures_delta.clear();
}

/// A frame at `time` seconds, so colors can be seen easing.
fn frame_at(ctx: &egui::Context, app: &mut OndApp, time: f64) {
    let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1360.0, 860.0))), time: Some(time), ..Default::default() };
    let mut out = ctx.run_ui(input, |ui| app.main_ui(ui));
    out.textures_delta.clear();
}

#[test]
fn quitting_waits_only_a_moment_for_blocking_work() {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).build().unwrap();
    let (started_tx, started) = channel();
    let (release, stuck) = channel::<()>();
    // A disk or keychain call that doesn't come back.
    rt.spawn_blocking(move || {
        let _ = started_tx.send(());
        let _ = stuck.recv_timeout(Duration::from_secs(60));
    });
    started.recv().unwrap();
    let t = std::time::Instant::now();
    drop(AppRuntime(Some(rt)));
    assert!(t.elapsed() < Duration::from_secs(30), "quitting waited {:?}", t.elapsed());
    drop(release);
}

#[test]
fn an_album_page_takes_its_own_colors_and_everything_else_the_playing_ones() {
    use crate::visuals::colors::Tints;
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    let solid = |c: egui::Color32| egui::ColorImage::new([32, 32], vec![c; 32 * 32]);
    app.art.preload("al-0", solid(egui::Color32::from_rgb(170, 30, 40)));
    app.art.preload("al-5", solid(egui::Color32::from_rgb(30, 60, 170)));
    let playing: Vec<Track> = (1..=3).map(|n| track(&format!("t-{n}"), "al-0", n)).collect();
    app.player.queue.play_now(playing, 0, Source::Added);
    app.ui.nav.go(Route::Album("al-5".into()));
    let time = std::cell::Cell::new(0.0);
    let settle = |app: &mut OndApp| {
        // The covers' looks are worked out in the background.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while ["al-0", "al-5"].iter().any(|c| app.art.look(c).is_none()) {
            assert!(std::time::Instant::now() < deadline, "the looks never arrived");
            std::thread::sleep(Duration::from_millis(5));
            app.art.poll();
        }
        for _ in 0..4 {
            time.set(time.get() + 0.5);
            frame_at(&ctx, app, time.get());
        }
    };
    let tints_of = |app: &mut OndApp, cover: &str| {
        let look = app.art.look(cover).unwrap();
        app.ui.tint_cache.cover(cover, look.palette.as_ref())
    };
    let shown = |ctx: &egui::Context| ctx.data(|d| d.get_temp::<Tints>(crate::visuals::colors::id())).unwrap();
    let dark = ctx.global_style().visuals.dark_mode;

    // Not adapting: the chosen accent everywhere, album page or not.
    app.prefs.edit(|s| s.accent = theme::Accent::Teal);
    settle(&mut app);
    let teal = Tints::base(theme::Accent::Teal);
    assert_eq!((shown(&ctx), app.ui.page_tints), (teal, Some(teal)));
    assert_eq!(Tokens::of_ctx(&ctx).accent, teal.of(dark).accent);

    // Adapting: the window in the playing album's colors, the page in its own.
    app.prefs.edit(|s| s.adapt_colors = true);
    settle(&mut app);
    let (red, blue) = (tints_of(&mut app, "al-0"), tints_of(&mut app, "al-5"));
    assert_ne!(red, blue);
    assert_ne!(red.of(dark).accent, teal.of(dark).accent);
    assert_eq!(shown(&ctx), red, "the now-playing display and the rest follow the playing track");
    assert_eq!(app.ui.page_tints, Some(blue), "the album page follows its album");
    // Drawing the page swaps its colors in, and puts the playing ones back after.
    let inside = theme::with_tints(&ctx, app.ui.page_tints, || Tokens::of_ctx(&ctx).accent);
    assert_eq!(inside, blue.of(dark).accent);
    assert_eq!(Tokens::of_ctx(&ctx).accent, red.of(dark).accent);

    // Leaving the page: its colors ease back to the playing album's.
    app.ui.nav.go(Route::Recent);
    for _ in 0..2 {
        time.set(time.get() + 0.3);
        frame_at(&ctx, &mut app, time.get());
    }
    let easing = app.ui.page_tints.unwrap();
    assert!(easing != blue && easing != red, "on the way back");
    settle(&mut app);
    assert_eq!(app.ui.page_tints, Some(red));

    // Back to the album: no wait, its colors are already known.
    app.ui.nav.go(Route::Album("al-5".into()));
    frame_at(&ctx, &mut app, time.get() + 0.01);
    assert_eq!(app.ui.page_tint.target(), Some(blue), "heading for the album's colors at once");
}

#[test]
fn repeat_queue_with_nothing_playable_stops() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    app.session.online = false;
    app.player.queue.end = crate::queue::EndAction::RepeatQueue;
    let pass = app.player.queue.pass_len();
    app.start_current();
    assert!(app.player.queue.history.len() <= 2 * pass, "history grew to {}", app.player.queue.history.len());
    assert_eq!(app.ui.toasts.texts().last().copied(), Some("Nothing in the queue is downloaded"));
}

#[test]
fn every_view_draws() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    let routes = [
        Route::Recent,
        Route::Albums,
        Route::Artists,
        Route::Artist { name: "Scott LaFaro".into(), id: None },
        Route::Artist { name: "Bill Evans Trio".into(), id: None },
        Route::Album("al-0".into()),
        Route::Album("al-missing".into()),
        Route::Playlist { id: "pl-1".into(), name: "Late night".into() },
        Route::Filter { kind: FilterKind::Genre, value: "Cool Jazz".into() },
        Route::Filter { kind: FilterKind::Label, value: "Nobody".into() },
        Route::Offline,
        Route::Songs,
    ];
    for (k, r) in routes.into_iter().enumerate() {
        app.ui.nav.go(r);
        // Hover somewhere different each time so hover-only details draw too.
        let at = egui::pos2(300.0 + 37.0 * k as f32, 180.0 + 41.0 * k as f32);
        for _ in 0..2 {
            frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        }
    }
    app.ui.queue_ui.tab = QueueTab::History;
    app.player.queue.advance();
    frame(&ctx, &mut app, Vec::new());
    app.prefs.edit(|s| s.show_queue = false);
    frame(&ctx, &mut app, Vec::new());
    app.ui.nav.go(Route::Album("al-0".into()));
    frame(&ctx, &mut app, Vec::new());
    assert_eq!(app.ui.nav.trail(), [&Route::Songs, &Route::Album("al-0".into())]);
}

#[test]
fn palette_opens_searches_and_queues() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    let cmd_k = egui::Event::Key { key: egui::Key::K, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND };
    frame(&ctx, &mut app, vec![cmd_k]);
    assert!(app.ui.palette.is_some());
    // The field takes focus on the palette's first frame; type after that.
    frame(&ctx, &mut app, Vec::new());
    frame(&ctx, &mut app, vec![egui::Event::Text("album 12".into())]);
    frame(&ctx, &mut app, Vec::new());
    assert_eq!(app.ui.palette.as_ref().map(|p| p.query.as_str()), Some("album 12"));
    let before = app.player.queue.upcoming.len();
    let alt_enter = egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::ALT };
    frame(&ctx, &mut app, vec![alt_enter]);
    assert!(app.ui.palette.is_none());
    // Album 12's tracks aren't loaded, so the action waits for them...
    assert_eq!(app.player.queue.upcoming.len(), before);
    assert_eq!(app.pending.len(), 1);
    // ...and runs when they arrive.
    let tracks: Vec<Track> = (1..=3).map(|n| track(&format!("x-{n}"), "al-12", n)).collect();
    app.handle(Msg::Tracks { album_id: "al-12".into(), tracks, fresh: true });
    assert_eq!(app.player.queue.upcoming.len(), before + 3);
    assert!(app.pending.is_empty());
    // Played next: right after what the user already chose to play next.
    assert_eq!(app.player.queue.upcoming[2].track.id, "x-1");
}

/// Handles what the bridge sends until `done` says so (or 3 s pass).
fn until(app: &mut OndApp, mut done: impl FnMut(&OndApp) -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !done(app) {
        assert!(std::time::Instant::now() < deadline, "timed out waiting on the bridge");
        match app.inbox.recv_timeout(Duration::from_millis(20)) {
            Ok(msg) => app.handle(msg),
            Err(_) => continue,
        }
    }
}

#[test]
fn the_search_index_is_built_off_the_ui_thread_and_stale_builds_are_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    let album = |id: &str, name: &str| Album {
        id: id.into(),
        name: name.into(),
        sort: None,
        artist: Some("Bill Evans".into()),
        artist_ref: None,
        year: None,
        genre: None,
        song_count: 0,
        duration_secs: 0,
        cover_art: None,
        created: None,
        labels: Vec::new(),
        genres: Vec::new(),
        artists: Vec::new(),
    };
    app.handle(Msg::Albums(vec![album("a1", "Portrait in Jazz")]));
    app.after_messages();
    assert!(!app.search.stale, "the rebuild was handed off");
    let first = app.search.generation;
    // Before the first build lands, the library changes again.
    app.handle(Msg::Albums(vec![album("a1", "Portrait in Jazz"), album("a2", "Explorations")]));
    app.after_messages();
    assert_eq!(app.search.generation, first + 1);
    until(&mut app, |a| a.search.index.as_ref().is_some_and(|s| s.albums.len() == 2));
    // The older build, if it lands now, doesn't replace the newer index.
    let stale = SearchIndex::new(&[album("x", "Old")], &[], &[]);
    app.handle(Msg::SearchIndex { generation: first, index: Box::new(stale) });
    assert_eq!(app.search.index.as_ref().map(|s| s.albums.len()), Some(2));
}

#[test]
fn a_load_that_fails_drops_what_waited_for_it() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    assert!(app.session.client.is_none(), "offline: nothing can be fetched");
    let before = app.player.queue.upcoming.len();
    // Album 12 isn't stored either: the play and the pin both wait, once each.
    app.apply(Action::Album("al-12".into(), QueueAction::Add));
    app.apply(Action::Album("al-12".into(), QueueAction::PlayNext));
    app.apply(Action::Offline(OfflineAction::Keep(Keep::Album("al-12".into()), true)));
    app.apply(Action::Offline(OfflineAction::Keep(Keep::Album("al-12".into()), true)));
    assert_eq!(app.pending.len(), 1, "the newer action replaced the older");
    assert_eq!(app.downloads.pending_pins.len(), 1);
    // The load fails: both give up.
    until(&mut app, |a| a.pending.is_empty() && a.downloads.pending_pins.is_empty());
    // The tracks turn up later anyway (browsing, a sync): nothing plays or pins.
    let tracks: Vec<Track> = (1..=3).map(|n| track(&format!("x-{n}"), "al-12", n)).collect();
    app.handle(Msg::Tracks { album_id: "al-12".into(), tracks, fresh: true });
    assert_eq!(app.player.queue.upcoming.len(), before, "a stale action played");
    assert!(!app.downloads.store().is_pinned(&Pin::Album("al-12".into())), "a stale pin was made");
}

#[test]
fn a_failed_album_sync_stops_saying_syncing() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    app.session.online = true;
    app.sync.albums = true;
    app.handle(Msg::AlbumSyncDone(Err("sync: timed out".into())));
    assert!(!app.sync.albums && !app.sync.tracks);
    assert!(app.ui.toasts.texts().contains(&"sync: timed out"));
    // While an album sync runs, Sync now doesn't start a second one.
    app.sync.albums = true;
    app.session.client = Some(subsonic::Client::new("http://127.0.0.1:9", subsonic::Credentials::new("u", "p")).unwrap());
    app.sync.label = "syncing… 500".into();
    app.apply(Action::SyncNow);
    assert_eq!(app.sync.label, "syncing… 500", "a second album sync started");
}

#[test]
fn window_modes_draw_and_switch() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    let key = |key, modifiers| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
    frame(&ctx, &mut app, vec![key(egui::Key::M, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT)]);
    assert_eq!(app.prefs_now().mode, WindowMode::Mini);
    // Hovered (overlay drawn) and not.
    for at in [egui::pos2(100.0, 100.0), egui::pos2(-10.0, -10.0)] {
        for _ in 0..3 {
            frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        }
    }
    frame(&ctx, &mut app, vec![key(egui::Key::Num2, egui::Modifiers::COMMAND | egui::Modifiers::ALT)]);
    assert_eq!(app.prefs_now().mode, WindowMode::Player);
    for _ in 0..2 {
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(egui::pos2(120.0, 60.0))]);
    }
    // Browsing goes back to the full window.
    app.actions.push(Action::Go(Route::Album("al-0".into())));
    frame(&ctx, &mut app, Vec::new());
    assert_eq!(app.prefs_now().mode, WindowMode::Full);
    // ⌘K from a small mode does too.
    app.edit_windows(|w| w.mode = WindowMode::Player);
    frame(&ctx, &mut app, vec![key(egui::Key::K, egui::Modifiers::COMMAND)]);
    assert_eq!(app.prefs_now().mode, WindowMode::Full);
    assert!(app.ui.palette.is_some());
}

#[test]
fn the_visualizer_opens_only_when_its_experiment_is_on_and_esc_leaves() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    let key = |key, modifiers| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
    frame(&ctx, &mut app, vec![key(egui::Key::T, egui::Modifiers::COMMAND)]);
    assert!(app.ui.visualizer.is_none(), "off: ⌘T does nothing");
    app.prefs.edit(|s| s.experimental.visualizer = true);
    frame(&ctx, &mut app, vec![key(egui::Key::T, egui::Modifiers::COMMAND)]);
    assert!(app.ui.visualizer.is_some());
    for _ in 0..3 {
        frame(&ctx, &mut app, Vec::new());
    }
    frame(&ctx, &mut app, vec![key(egui::Key::Escape, egui::Modifiers::NONE)]);
    assert!(app.ui.visualizer.is_none(), "Esc leaves");
    // Going somewhere else leaves it too, and so does turning it off.
    app.toggle_visualizer();
    app.apply(Action::Go(Route::Albums));
    assert!(app.ui.visualizer.is_none());
    app.toggle_visualizer();
    app.prefs.edit(|s| s.experimental.visualizer = false);
    frame(&ctx, &mut app, Vec::new());
    assert!(app.ui.visualizer.is_none());
}

#[test]
fn arrows_switch_the_visualizer_and_the_choice_is_kept() {
    use crate::visuals::visualizer::{Kind, Pick};
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    let key = |key, modifiers| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
    let showing = |app: &OndApp| app.ui.visualizer.as_ref().map(|v| v.kind());
    app.prefs.edit(|s| {
        s.experimental.visualizer = true;
        s.experimental.visualizer_pick = Pick::Particles;
    });
    app.toggle_visualizer();
    frame(&ctx, &mut app, Vec::new());
    assert_eq!(showing(&app), Some(Kind::Particles), "it opens on the one chosen");
    frame(&ctx, &mut app, vec![key(egui::Key::ArrowRight, egui::Modifiers::NONE)]);
    assert_eq!(showing(&app), Some(Kind::Ribbons));
    assert_eq!(app.prefs_now().experimental.visualizer_pick, Pick::Ribbons, "and that becomes the choice");
    frame(&ctx, &mut app, vec![key(egui::Key::ArrowRight, egui::Modifiers::NONE)]);
    assert_eq!(showing(&app), Some(Kind::Flow), "round from the last to the first");
    frame(&ctx, &mut app, vec![key(egui::Key::ArrowLeft, egui::Modifiers::NONE)]);
    assert_eq!(showing(&app), Some(Kind::Ribbons));
    // ⌘→ is still the next track, not a switch.
    frame(&ctx, &mut app, vec![key(egui::Key::ArrowRight, egui::Modifiers::COMMAND)]);
    assert_eq!(showing(&app), Some(Kind::Ribbons));
    // Kept for next time.
    app.prefs.flush();
    let saved = crate::settings::Settings::read(&dir.path().join("settings.json")).unwrap();
    assert_eq!(saved.experimental.visualizer_pick, Pick::Ribbons);
    app.close_visualizer();
    app.toggle_visualizer();
    assert_eq!(showing(&app), Some(Kind::Ribbons));
    // Shuffled, the arrows still switch, and it stays shuffled.
    app.close_visualizer();
    app.prefs.edit(|s| s.experimental.visualizer_pick = Pick::Shuffle);
    app.toggle_visualizer();
    frame(&ctx, &mut app, Vec::new());
    let first = showing(&app);
    frame(&ctx, &mut app, vec![key(egui::Key::ArrowRight, egui::Modifiers::NONE)]);
    assert_ne!(showing(&app), first);
    assert_eq!(app.prefs_now().experimental.visualizer_pick, Pick::Shuffle);
    // And each new track after the first brings another.
    app.player.queue.play_now(vec![track("t-first", "al-0", 1)], 0, Source::Added);
    frame(&ctx, &mut app, Vec::new());
    for n in 0..4 {
        let before = showing(&app);
        app.player.queue.play_now(vec![track(&format!("t-{n}"), "al-0", 1)], 0, Source::Added);
        frame(&ctx, &mut app, Vec::new());
        assert_ne!(showing(&app), before, "track {n}");
    }
}

#[test]
fn lyrics_go_to_the_sidebar_or_the_players_drawer() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    frame(&ctx, &mut app, Vec::new());
    // Full window: the sidebar switches to its Lyrics tab, and back.
    app.apply(Action::Lyrics);
    assert_eq!(app.ui.queue_ui.tab, QueueTab::Lyrics);
    app.apply(Action::Lyrics);
    assert_eq!(app.ui.queue_ui.tab, QueueTab::UpNext);
    // The artwork toggles to the Player (nothing used yet), where ⌥⌘L opens the drawer.
    app.apply(Action::Window(WindowAction::TogglePlayer));
    assert_eq!(app.prefs_now().mode, WindowMode::Player);
    app.apply(Action::Lyrics);
    frame(&ctx, &mut app, Vec::new());
    assert!(app.prefs_now().lyrics_open);
    // Mini, then back to full: the artwork now toggles to Mini.
    app.apply(Action::Window(WindowAction::ToggleMini));
    app.apply(Action::Window(WindowAction::SetMode(WindowMode::Full)));
    app.apply(Action::Window(WindowAction::TogglePlayer));
    assert_eq!(app.prefs_now().mode, WindowMode::Mini);
    // Lyrics from Mini open the Player with its drawer.
    app.edit_windows(|w| w.lyrics_open = false);
    app.apply(Action::Lyrics);
    assert_eq!(app.prefs_now().mode, WindowMode::Player);
    assert!(app.prefs_now().lyrics_open);
    for _ in 0..2 {
        frame(&ctx, &mut app, Vec::new());
    }
}

/// Makes track `id` downloaded, the way a finished stream would: via a file
/// the stream cache "has", which the store links in.
fn download(app: &mut OndApp, dir: &std::path::Path, id: &str) {
    let file = dir.join(format!("cached-{id}.flac"));
    std::fs::write(&file, b"fLaC").unwrap();
    let want = id.to_owned();
    app.downloads.store().set_cache_lookup(Arc::new(move |r: &TrackRef| (r.id == want).then(|| file.clone())));
    let t = app.data.tracks.all.iter().find(|t| t.id == id).unwrap().clone();
    app.apply(Action::Offline(OfflineAction::Keep(Keep::Tracks(vec![t]), true)));
    let start = std::time::Instant::now();
    while !app.downloads.store().snapshot().is_done(id) {
        assert!(start.elapsed() < Duration::from_secs(5), "download of {id}");
        std::thread::sleep(Duration::from_millis(5));
    }
    app.downloads.snap = app.downloads.store().snapshot();
}

#[test]
fn every_startup_notice_is_shown() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("library.redb"), b"not a database").unwrap();
    std::fs::write(dir.path().join("settings.json"), b"{ not json").unwrap();
    let ctx = egui::Context::default();
    let app = app(&ctx, dir.path());
    let shown = app.ui.toasts.texts();
    assert!(shown.iter().any(|t| t.starts_with("Your library database couldn't be read")), "{shown:?}");
    assert!(shown.iter().any(|t| t.starts_with("Your settings couldn't be read")), "{shown:?}");
}

#[test]
fn another_account_gets_its_own_library_and_downloads() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    let client = || subsonic::Client::new("http://127.0.0.1:9", subsonic::Credentials::new("u", "p")).unwrap();
    let login = |user: &str| Box::new(Settings { server: "http://music.local".into(), username: user.into(), ..Default::default() });
    let connect = |app: &mut OndApp, user: &str, new_account: bool| {
        let session = app.bridge.session();
        app.handle(Msg::Connected { session, client: client(), settings: login(user), server: "test".into(), new_account });
    };
    connect(&mut app, "luke", false);
    store_opened(&mut app);
    download(&mut app, dir.path(), "t-2");
    assert!(app.downloads.snap.is_done("t-2"));

    // Someone else signs in: the shown library goes, and so do the queue
    // and the old account's downloads (not deleted, just not theirs).
    app.player.queue = crate::queue::Queue::default();
    let tracks: Vec<Track> = app.data.tracks.by_album["al-0"][..2].to_vec();
    app.queue_tracks(tracks, Source::Album { id: "al-0".into(), name: "Album 0".into() }, QueueAction::Play { start: 0 });
    connect(&mut app, "sam", true);
    assert!(app.data.albums.is_empty() && app.data.tracks.all.is_empty());
    assert!(app.player.queue.current.is_none());
    assert!(app.downloads.snap.pins.is_empty(), "nothing shows as downloaded while sam's store opens");
    store_opened(&mut app);
    assert!(app.downloads.snap.pins.is_empty(), "sam's store starts empty");
    assert!(!app.downloads.store().snapshot().is_done("t-2"));

    // Back to the first account: its downloads are where they were.
    connect(&mut app, "luke", true);
    store_opened(&mut app);
    assert!(app.downloads.store().snapshot().is_done("t-2"));
}

/// Waits for the offline store opening in the background, handling only
/// its messages: the rest go back in the inbox, in order, behind them.
fn store_opened(app: &mut OndApp) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut others = Vec::new();
    while app.downloads.store.is_none() || app.downloads.opening {
        assert!(std::time::Instant::now() < deadline, "the offline store never opened");
        match app.inbox.recv_timeout(Duration::from_millis(20)) {
            Ok(m @ (Msg::OfflineStore { .. } | Msg::OfflineMoving(_) | Msg::OfflineMoved(_))) => app.handle(m),
            Ok(m) => others.push(m),
            Err(_) => {}
        }
    }
    for m in others {
        app.bridge.post(m);
    }
}

#[test]
fn another_accounts_downloads_open_in_the_background_and_wait_for_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    let client = || subsonic::Client::new("http://127.0.0.1:9", subsonic::Credentials::new("u", "p")).unwrap();
    let login = |user: &str| Box::new(Settings { server: "http://music.local".into(), username: user.into(), ..Default::default() });
    let connect = |app: &mut OndApp, user: &str| {
        let session = app.bridge.session();
        app.handle(Msg::Connected { session, client: client(), settings: login(user), server: "test".into(), new_account: false });
    };
    connect(&mut app, "luke");
    // Signed in as someone else before luke's store came: it's dropped unused.
    connect(&mut app, "sam");
    assert!(app.downloads.store.is_none(), "opened on the UI thread");
    // Asked for while it opens: done once it's open.
    let t = app.data.tracks.all.iter().find(|t| t.id == "t-3").unwrap().clone();
    app.apply(Action::Offline(OfflineAction::Keep(Keep::Tracks(vec![t]), true)));
    app.apply(Action::Offline(OfflineAction::Pause(true)));
    store_opened(&mut app);
    let sam = login("sam").account_id().unwrap();
    assert!(app.downloads.store().dir().ends_with(format!("{sam:08x}")), "{}", app.downloads.store().dir().display());
    for a in std::mem::take(&mut app.actions) {
        app.apply(a);
    }
    assert!(app.downloads.store().is_pinned(&Pin::Track("t-3".into())));
    assert!(app.downloads.store().snapshot().paused);
}

#[test]
fn a_downloads_folder_set_in_managed_json_moves_the_downloads_there() {
    let dir = tempfile::tempdir().unwrap();
    let (old, chosen) = (dir.path().join("old"), dir.path().join("music"));
    let account = Settings { server: "http://music.local".into(), username: "luke".into(), signed_out: true, ..Default::default() };
    let store = old.join(format!("{:08x}", account.account_id().unwrap()));
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(store.join("t-2.flac"), b"fLaC").unwrap();
    let saved = Settings { offline_dir: Some(old.clone()), ..account };
    std::fs::write(dir.path().join("settings.json"), serde_json::to_vec(&saved).unwrap()).unwrap();
    std::fs::write(dir.path().join("managed.json"), serde_json::json!({ "offline_dir": chosen }).to_string()).unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    store_opened(&mut app);
    // A folder of Ond's own inside the one set, as when picked in Settings.
    let new = chosen.join("Ond Offline").join(store.file_name().unwrap());
    assert_eq!(app.downloads.store().dir(), new);
    assert!(new.join("t-2.flac").exists());
    assert!(!old.exists());
}

#[test]
fn downloads_moved_to_another_folder_move_in_the_background() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = (dir.path().join("old"), dir.path().join("new"));
    let account = Settings { server: "http://music.local".into(), username: "luke".into(), ..Default::default() };
    let store = old.join(format!("{:08x}", account.account_id().unwrap()));
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(store.join("t-2.flac"), b"fLaC").unwrap();
    let mut saved = Settings { offline_dir: Some(new.clone()), offline_move_from: Some(old.clone()), ..account };
    saved.signed_out = true;
    std::fs::write(dir.path().join("settings.json"), serde_json::to_vec(&saved).unwrap()).unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    assert!(app.downloads.store.is_none(), "the move doesn't hold up the first frame");
    assert!(app.ui.toasts.texts().iter().any(|t| t.starts_with("Moving downloads to")), "{:?}", app.ui.toasts.texts());
    store_opened(&mut app);
    assert_eq!(app.downloads.store().dir(), new.join(store.file_name().unwrap()));
    assert!(new.join(store.file_name().unwrap()).join("t-2.flac").exists());
    assert!(!old.exists());
    assert_eq!(app.prefs.snapshot().offline_move_from, None, "done, so not moved again");
    let shown = app.ui.toasts.texts();
    assert!(shown.iter().any(|t| t.starts_with("Downloads moved to")), "{shown:?}");
    assert!(!shown.iter().any(|t| t.starts_with("Moving downloads")), "the progress notice gives way: {shown:?}");
}

#[test]
fn offline_the_queue_plays_only_what_is_downloaded() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    download(&mut app, dir.path(), "t-2");
    assert!(!app.session.online);
    let tracks: Vec<Track> = app.data.tracks.by_album["al-0"][..4].to_vec();
    assert!(app.source(0, None, &tracks[0], None).is_none(), "offline and not downloaded: nothing to play");
    let local = app.source(0, None, &tracks[1], None).unwrap();
    assert!(local.url.starts_with("file://") && local.url.ends_with("t-2.flac"), "{}", local.url);

    // Playing from the top skips to the downloaded song and says so.
    app.player.queue = crate::queue::Queue::default();
    app.queue_tracks(tracks.clone(), Source::Album { id: "al-0".into(), name: "Album 0".into() }, QueueAction::Play { start: 0 });
    assert_eq!(app.player.queue.current.as_ref().unwrap().track.id, "t-2");
    assert_eq!(app.ui.toasts.texts().last().copied(), Some("Skipped 1 song that isn't downloaded"));
    // Nothing after it can play, so the engine isn't told about a next track.
    assert_eq!(app.player.sent_next(), None);
    // When it ends, the rest is skipped and playback stops cleanly.
    app.handle_engine(audio::Event::Ended { track_id: "t-2".into(), token: 0 });
    assert!(app.player.queue.current.is_none());

    // Back online: everything plays again, downloaded songs still from disk.
    let client = subsonic::Client::new("http://127.0.0.1:9", subsonic::Credentials::new("u", "p")).unwrap();
    app.handle(Msg::Connected { session: app.bridge.session(), client, settings: Box::default(), server: "test".into(), new_account: false });
    assert!(app.session.online);
    assert!(app.source(0, None, &tracks[0], None).unwrap().url.contains("/rest/stream"));
    assert!(app.source(0, None, &tracks[1], None).unwrap().url.starts_with("file://"));
}

#[test]
fn errors_in_one_batch_are_all_shown() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    app.session.online = true;
    // Two failures handled before the next frame: both get a toast.
    app.handle(Msg::Error("playlists: timed out".into()));
    app.handle(Msg::Error("favorites: timed out".into()));
    let shown = app.ui.toasts.texts();
    assert!(shown.contains(&"playlists: timed out") && shown.contains(&"favorites: timed out"), "{shown:?}");
}

#[test]
fn a_library_analysis_resumes_once_the_server_answers() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    let client = || subsonic::Client::new("http://127.0.0.1:9", subsonic::Credentials::new("u", "p")).unwrap();
    // Held, so what's queued stays queued.
    app.analysis.runner.set_paused(true);
    app.prefs.edit(|s| s.analysis_resume = true);
    // Startup: the library and the stored results load well before the
    // server answers (or offline, never). Nothing has run, so nothing finished.
    app.handle(Msg::AnalysesLoaded(Vec::new()));
    app.handle(Msg::AllTracks(Box::new(TrackSet::new(app.data.tracks.all.to_vec()))));
    for _ in 0..3 {
        app.check_library_analysis();
    }
    assert!(app.prefs_now().analysis_resume, "the run is still due");
    assert!(!app.analysis.runner.progress().active());

    app.handle(Msg::Connected { session: app.bridge.session(), client: client(), settings: Box::default(), server: "test".into(), new_account: false });
    assert_eq!(app.analysis.runner.progress().total, 20, "the whole library is queued");
    app.check_library_analysis();
    assert!(app.prefs_now().analysis_resume);

    // Signing out drops the old account's queue, but the run is still due for the next.
    app.sign_out();
    assert!(!app.analysis.runner.progress().active());
    app.check_library_analysis();
    assert!(app.prefs_now().analysis_resume);
    app.handle(Msg::Connected { session: app.bridge.session(), client: client(), settings: Box::default(), server: "test".into(), new_account: false });
    assert_eq!(app.analysis.runner.progress().total, 20);

    // The queue runs dry: done, and not resumed at the next launch.
    app.analysis.runner.clear();
    app.check_library_analysis();
    assert!(!app.prefs_now().analysis_resume);
}

#[test]
fn signing_out_forgets_lyrics_and_song_stats() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    let client = || subsonic::Client::new("http://127.0.0.1:9", subsonic::Credentials::new("u", "p")).unwrap();
    app.handle(Msg::Connected { session: app.bridge.session(), client: client(), settings: Box::default(), server: "test".into(), new_account: false });
    let t = app.data.tracks.all[0].clone();
    // Asked for, still on their way when the user signs out.
    app.apply(Action::NeedLyrics(Box::new(t.clone())));
    app.apply(Action::NeedSongStats(t.id.clone()));
    assert!(matches!(app.lyrics.get(&t.id), Some(crate::lyrics::Fetch::Loading)));
    assert!(app.song_stats.contains_key(&t.id));
    app.sign_out();
    assert!(app.lyrics.is_empty() && app.song_stats.is_empty(), "nothing left loading for good, nor for the next account");
    // Signed in again: asked for afresh.
    app.handle(Msg::Connected { session: app.bridge.session(), client: client(), settings: Box::default(), server: "test".into(), new_account: false });
    app.apply(Action::NeedLyrics(Box::new(t.clone())));
    assert!(matches!(app.lyrics.get(&t.id), Some(crate::lyrics::Fetch::Loading)));
    // Another account: what was fetched for the last one goes too.
    app.handle(Msg::Lyrics { track_id: t.id.clone(), lyrics: None });
    app.handle(Msg::SongStats { track_id: t.id.clone(), stats: None });
    app.handle(Msg::Connected { session: app.bridge.session(), client: client(), settings: Box::default(), server: "test".into(), new_account: true });
    assert!(app.lyrics.is_empty() && app.song_stats.is_empty());
}

#[test]
fn playing_an_album_stored_only_in_part_waits_for_the_server() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    let all: Vec<Track> = (1..=20).map(|n| track(&format!("p-{n}"), "al-3", n)).collect();
    // A sync cut short stored 5 of its 20 songs.
    app.data.tracks.set_album("al-3", all[..5].to_vec());
    app.player.queue = crate::queue::Queue::default();
    // Queued (offline nothing here is downloaded, so it's all skipped into history).
    let queued = |app: &OndApp| app.player.queue.history.len() + app.player.queue.pass_len();

    // Offline: what's stored is all there is.
    app.apply(Action::Album("al-3".into(), QueueAction::Play { start: 0 }));
    assert_eq!(queued(&app), 5);

    // Online: the server's listing, not the stored part.
    app.player.queue = crate::queue::Queue::default();
    app.session.client = Some(subsonic::Client::new("http://127.0.0.1:9", subsonic::Credentials::new("u", "p")).unwrap());
    app.apply(Action::Album("al-3".into(), QueueAction::Play { start: 0 }));
    assert_eq!(queued(&app), 0, "waits");
    // The stored copy arrives first: still waiting.
    app.handle(Msg::Tracks { album_id: "al-3".into(), tracks: all[..5].to_vec(), fresh: false });
    assert_eq!(queued(&app), 0);
    app.handle(Msg::Tracks { album_id: "al-3".into(), tracks: all.clone(), fresh: true });
    assert_eq!(queued(&app), 20);
    assert!(app.pending.is_empty());

    // The server can't be reached: play what's stored rather than nothing.
    app.player.queue = crate::queue::Queue::default();
    app.data.tracks.set_album("al-3", all[..5].to_vec());
    app.apply(Action::Album("al-3".into(), QueueAction::Play { start: 0 }));
    app.handle(Msg::LoadFailed { key: ListKey::Album("al-3".into()), error: Some("timed out".into()) });
    assert_eq!(queued(&app), 5);
    assert!(app.pending.is_empty());

    // A complete album plays at once.
    app.player.queue = crate::queue::Queue::default();
    app.apply(Action::Album("al-0".into(), QueueAction::Play { start: 0 }));
    assert_eq!(queued(&app), 20);
}

#[test]
fn signing_out_ignores_a_reconnect_that_lands_afterwards() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    let client = || subsonic::Client::new("http://127.0.0.1:9", subsonic::Credentials::new("u", "p")).unwrap();
    // Signed in, then the server went away: a reconnect loop is running.
    let before = app.bridge.session();
    app.handle(Msg::Connected { session: before, client: client(), settings: Box::default(), server: "test".into(), new_account: false });
    assert!(app.session.online && app.session.client.is_some());
    app.handle(Msg::Unreachable { session: before, username: "u".into(), error: "gone".into() });
    assert!(app.session.unreachable);

    app.sign_out();
    assert!(app.login.is_some());
    assert!(app.session.client.is_none() && !app.session.online && !app.session.unreachable);
    assert_ne!(app.bridge.session(), before, "sign-out ends the session");

    // The old loop reaches the server after all: stale, ignored.
    app.handle(Msg::Connected { session: before, client: client(), settings: Box::default(), server: "test".into(), new_account: false });
    app.handle(Msg::Unreachable { session: before, username: "u".into(), error: "gone".into() });
    assert!(app.login.is_some(), "still on the login screen");
    assert!(app.session.client.is_none() && !app.session.online && !app.session.unreachable);

    // A fresh sign-in in the new session works.
    app.handle(Msg::Connected { session: app.bridge.session(), client: client(), settings: Box::default(), server: "test".into(), new_account: false });
    assert!(app.login.is_none() && app.session.online);
}

#[test]
fn keeping_an_album_waits_for_its_tracks_and_the_offline_place_draws() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    // Album 5's tracks aren't loaded: the pin waits for them.
    app.apply(Action::Offline(OfflineAction::Keep(Keep::Album("al-5".into()), true)));
    assert!(!app.downloads.store().is_pinned(&Pin::Album("al-5".into())));
    let tracks: Vec<Track> = (1..=3).map(|n| track(&format!("y-{n}"), "al-5", n)).collect();
    app.handle(Msg::Tracks { album_id: "al-5".into(), tracks: tracks.clone(), fresh: false });
    assert!(app.downloads.store().is_pinned(&Pin::Album("al-5".into())));
    assert_eq!(app.downloads.store().pin_tracks(&Pin::Album("al-5".into())).unwrap().1, ["y-1", "y-2", "y-3"]);
    // A partial list from the local library (mid-sync, rebuilt) never shrinks the pin.
    app.downloads.snap = app.downloads.store().snapshot();
    app.handle(Msg::Tracks { album_id: "al-5".into(), tracks: tracks[..1].to_vec(), fresh: false });
    // (The stored album now lists only y-1; a library-wide refresh sees that.)
    let all = app.data.tracks.all.to_vec();
    app.handle(Msg::AllTracks(Box::new(TrackSet::new(all))));
    assert_eq!(app.downloads.store().pin_tracks(&Pin::Album("al-5".into())).unwrap().1, ["y-1", "y-2", "y-3"]);
    // The album changed on the server: the pin follows.
    app.handle(Msg::Tracks { album_id: "al-5".into(), tracks: tracks[..2].to_vec(), fresh: true });
    assert_eq!(app.downloads.store().pin_tracks(&Pin::Album("al-5".into())).unwrap().1, ["y-1", "y-2"]);
    download(&mut app, dir.path(), "t-3");

    for route in [Route::Offline, Route::Album("al-5".into()), Route::Album("al-0".into()), Route::Recent, Route::Songs] {
        app.ui.nav.go(route);
        for _ in 0..2 {
            frame(&ctx, &mut app, vec![egui::Event::PointerMoved(egui::pos2(500.0, 300.0))]);
        }
    }
    // ⇧⌘D on the album page toggles it.
    app.ui.nav.go(Route::Album("al-5".into()));
    let key = egui::Event::Key { key: egui::Key::D, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND | egui::Modifiers::SHIFT };
    frame(&ctx, &mut app, vec![key]);
    assert!(!app.downloads.store().is_pinned(&Pin::Album("al-5".into())));
    assert!(app.downloads.store().is_pinned(&Pin::Track("t-3".into())));
}

#[test]
fn gapless_start_advances_the_queue() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    let next = app.player.queue.peek_next().unwrap().clone();
    let format = audio::StreamFormat { sample_rate: 44_100, channels: 2, bits_per_sample: Some(16) };
    let started = |i: &crate::queue::Item| audio::Event::TrackStarted {
        track_id: i.track.id.clone(),
        token: i.uid,
        format,
        gapless: true,
        restart: false,
    };
    app.handle_engine(started(&next));
    assert_eq!(app.player.queue.current.as_ref().unwrap().uid, next.uid);
    // A repeat of the same event (or the current track) changes nothing.
    let upcoming = app.player.queue.upcoming.len();
    app.handle_engine(started(&next));
    assert_eq!(app.player.queue.upcoming.len(), upcoming);
}

#[test]
fn the_playing_song_queued_next_still_advances_gaplessly() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    let format = audio::StreamFormat { sample_rate: 44_100, channels: 2, bits_per_sample: Some(16) };
    let started = |i: &crate::queue::Item| audio::Event::TrackStarted {
        track_id: i.track.id.clone(),
        token: i.uid,
        format,
        gapless: true,
        restart: false,
    };
    // "Play next" on the song that's playing: the same track, a new queue item.
    let playing = app.player.queue.current.clone().unwrap();
    app.player.queue.clear();
    app.player.queue.play_next(vec![playing.track.clone()]);
    let copy = app.player.queue.peek_next().unwrap().clone();
    assert_eq!(copy.track.id, playing.track.id);
    assert_ne!(copy.uid, playing.uid);
    app.handle_engine(started(&copy));
    assert_eq!(app.player.queue.current.as_ref().unwrap().uid, copy.uid, "the queue moved to the copy");

    // Repeat queue over a single item: each gapless start is the next pass.
    app.player.queue = crate::queue::Queue::default();
    app.player.queue.end = crate::queue::EndAction::RepeatQueue;
    app.player.queue.play_now(vec![playing.track.clone()], 0, Source::Added);
    let first = app.player.queue.current.clone().unwrap();
    app.handle_engine(started(&first));
    let second = app.player.queue.current.clone().unwrap();
    assert_ne!(second.uid, first.uid, "a new pass began");
    assert_eq!(app.player.queue.history.len(), 1);
}

#[test]
fn a_lost_device_is_a_notice_and_its_return_an_offer_until_playing_there() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path());
    let dev = |uid: &str, name: &str| audio::DeviceInfo { uid: uid.into(), name: name.into(), kind: audio::DeviceKind::Usb, caps: Default::default(), is_default: false };
    let list = || vec![dev("spk", "Speakers"), dev("dac", "DAC")];
    app.handle_engine(audio::Event::DeviceLost { name: "DAC".into() });
    assert!(app.ui.toasts.texts().iter().any(|t| t.starts_with("DAC disconnected")), "{:?}", app.ui.toasts.texts());
    app.handle_engine(audio::Event::DeviceReturned { uid: "dac".into(), name: "DAC".into() });
    assert!(app.ui.toasts.texts().contains(&"DAC is back"));
    // Something else playing changes nothing; playing through the DAC again ends the offer.
    app.handle_engine(audio::Event::Devices { devices: list(), current: Some("spk".into()) });
    assert!(app.ui.toasts.texts().contains(&"DAC is back"));
    app.handle_engine(audio::Event::Devices { devices: list(), current: Some("dac".into()) });
    assert!(!app.ui.toasts.texts().contains(&"DAC is back"));
}
