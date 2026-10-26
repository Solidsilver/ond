//! Frame-time budget on a large library: 2,000 albums, 25,000 tracks, a
//! 500-item queue and a 20,000-song playlist. Headless (no GPU): it times egui's layout and
//! tessellation of the real views, which is the CPU cost per frame.
//!
//! `cargo test -p ond-player --release perf -- --ignored --nocapture`

use super::*;
use std::time::{Duration, Instant};

const ALBUMS_N: usize = 2_000;
const TRACKS_N: usize = 25_000;
const QUEUE_N: usize = 500;
const PINS_N: usize = 2_000;
const PLAYLIST_N: usize = 20_000;

fn big_library(app: &mut OndApp) {
    let albums: Vec<Album> = (0..ALBUMS_N)
        .map(|i| {
            let mut a = album(i % ALBUMS.len());
            a.id = format!("big-{i}");
            a.name = format!("{} {i}", a.name);
            a
        })
        .collect();
    let per = TRACKS_N / ALBUMS_N + 1;
    let mut tracks = Vec::with_capacity(TRACKS_N);
    'outer: for i in 0..ALBUMS_N {
        let (name, artist, ..) = ALBUMS[i % ALBUMS.len()];
        for n in 0..per {
            if tracks.len() == TRACKS_N {
                break 'outer;
            }
            let title = format!("{} {} part {}", name.split(' ').next().unwrap_or("Song"), i, n + 1);
            let mut t = track(format!("bt{i}-{n}"), i % ALBUMS.len(), &title, artist, n as u32 + 1, 1, 180 + (n as u32 * 13) % 240, None, 16, 44_100);
            t.album_id = format!("big-{i}");
            tracks.push(t);
        }
    }
    app.data.newest = albums.iter().take(500).map(|a| a.id.clone()).collect();
    app.data.set_albums(albums);
    app.data.set_all_tracks(tracks);
    app.rebuild_search();
}

fn big_queue(app: &mut OndApp) {
    let tracks: Vec<Track> = app.data.tracks.all.iter().take(QUEUE_N).cloned().collect();
    app.player.queue = crate::queue::Queue::default();
    app.player.queue.play_now(tracks, 0, Source::Album { id: "big-0".into(), name: "A long queue".into() });
}

struct Timing {
    mean: Duration,
    p95: Duration,
    max: Duration,
}

/// Times `n` frames after a short warm-up.
fn time_frames(h: &mut Harness<'_, Option<OndApp>>, n: usize) -> Timing {
    for _ in 0..5 {
        h.step();
    }
    let passes = h.ctx.cumulative_pass_nr();
    let mut runs: Vec<Duration> = (0..n)
        .map(|_| {
            let t = Instant::now();
            h.step();
            t.elapsed()
        })
        .collect();
    assert!(h.ctx.cumulative_pass_nr() - passes >= n as u64, "every timed step ran a full pass");
    runs.sort();
    let mean = runs.iter().sum::<Duration>() / n as u32;
    Timing { mean, p95: runs[n * 95 / 100], max: runs[n - 1] }
}

fn scene(h: &mut Harness<'_, Option<OndApp>>, route: Route) {
    reset(h, FULL, route);
    big_queue(app_mut(h));
}

#[test]
#[ignore = "frame-time budget on a large library; run in release with --ignored"]
fn perf() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::builder()
        .with_size(FULL)
        .with_pixels_per_point(2.0)
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .with_os(egui::os::OperatingSystem::Mac)
        .build_ui_state(
            |ui, app: &mut Option<OndApp>| {
                if let Some(app) = app {
                    app.main_ui(ui);
                }
            },
            None,
        );
    let ctx = h.ctx.clone();
    let mut app = populated(&ctx, dir.path());
    big_library(&mut app);
    *h.state_mut() = Some(app);

    let mut results: Vec<(String, Timing)> = Vec::new();

    scene(&mut h, Route::Albums);
    results.push(("albums grid".into(), time_frames(&mut h, 60)));

    scene(&mut h, Route::Songs);
    h.hover_at(pos2(700.0, 500.0));
    for _ in 0..40 {
        h.event(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, -20_000.0), modifiers: egui::Modifiers::NONE, phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel });
        h.step();
    }
    results.push(("songs, mid-scroll".into(), time_frames(&mut h, 60)));
    // Sanity: the frames really drew the list, scrolled away from the top.
    let rows: Vec<String> = h.query_all_by_label_contains(" part ").map(|n| format!("{n:?}").chars().take(80).collect()).take(2).collect();
    println!("  songs rows on screen, e.g. {rows:?}");
    assert!(!rows.is_empty(), "songs view drew no rows");

    scene(&mut h, Route::Album("big-7".into()));
    results.push(("album page".into(), time_frames(&mut h, 60)));

    scene(&mut h, Route::Recent);
    results.push(("recent + 500-item queue".into(), time_frames(&mut h, 60)));

    scene(&mut h, Route::Albums);
    app_mut(&mut h).actions.push(Action::OpenPalette);
    h.run_steps(2);
    {
        let app = app_mut(&mut h);
        if let (Some(p), Some(index)) = (app.ui.palette.as_mut(), app.search.index.as_mut()) {
            p.query = "blue part".into();
            p.refresh(index);
        }
    }
    results.push(("palette open".into(), time_frames(&mut h, 60)));

    // The Songs view sorted by each column: the first frame pays the sort,
    // the rest must reuse it.
    let mut first_sort: Vec<(String, Duration)> = Vec::new();
    for (col, name) in [
        (songs::Column::Title, "title"),
        (songs::Column::Artist, "artist"),
        (songs::Column::Album, "album"),
        (songs::Column::Time, "time"),
        (songs::Column::Format, "format"),
    ] {
        scene(&mut h, Route::Songs);
        app_mut(&mut h).ui.songs.sort = col;
        let t = Instant::now();
        h.step();
        first_sort.push((format!("songs by {name}, first frame"), t.elapsed()));
        results.push((format!("songs by {name}"), time_frames(&mut h, 30)));
    }

    scene(&mut h, Route::Albums);
    app_mut(&mut h).actions.push(Action::OpenSettings(settings_page::Section::Library));
    h.run_steps(2);
    results.push(("settings → library".into(), time_frames(&mut h, 60)));
    app_mut(&mut h).ui.overlay = None;

    // 2,000 single-song downloads on the Offline page.
    {
        let app = app_mut(&mut h);
        let pins = app.data.tracks.all.iter().take(PINS_N).map(|t| (Pin::Track(t.id.clone()), t.title.clone(), vec![track_ref(t)])).collect();
        app.downloads.store().pin_many(pins);
        app.downloads.store().set_paused(true);
        app.downloads.snap = app.downloads.store().snapshot();
    }
    scene(&mut h, Route::Offline);
    results.push((format!("offline, {PINS_N} song pins"), time_frames(&mut h, 60)));

    // 2,000 loved songs.
    {
        let app = app_mut(&mut h);
        let songs = app.data.tracks.all.iter().take(PINS_N).cloned().collect();
        app.favorites.load(crate::favorites::Starred { songs, albums: Vec::new(), artists: Vec::new() }, Vec::new());
    }
    scene(&mut h, Route::Favorites);
    results.push((format!("favorites, {PINS_N} songs"), time_frames(&mut h, 60)));

    // A 20,000-song playlist of the user's own, scrolled part-way down
    // with a row hovered (its heart and actions showing).
    {
        let app = app_mut(&mut h);
        let songs: Vec<Track> = app.data.tracks.all.iter().take(PLAYLIST_N).cloned().collect();
        app.data.playlists.push(subsonic::Playlist { id: "big-pl".into(), name: "Everything".into(), song_count: PLAYLIST_N as u32, duration: 0, cover_art: None, owner: None });
        app.data.playlist_tracks.insert("big-pl".into(), Arc::new(songs));
    }
    scene(&mut h, Route::Playlist { id: "big-pl".into(), name: "Everything".into() });
    h.hover_at(pos2(700.0, 500.0));
    for _ in 0..20 {
        h.event(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, -20_000.0), modifiers: egui::Modifiers::NONE, phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel });
        h.step();
    }
    results.push((format!("playlist, {PLAYLIST_N} songs"), time_frames(&mut h, 60)));
    // Sanity: the frames drew the list's rows (the queue's are "… by …"),
    // scrolled away from the top.
    let first = app_mut(&mut h).data.tracks.all[0].title.clone();
    let rows: Vec<String> = h.query_all_by_label_contains(" part ").map(|n| format!("{n:?}")).filter(|l| l.contains("ListBoxOption") && !l.contains(" by ")).map(|l| l.chars().take(80).collect()).take(2).collect();
    println!("  playlist rows on screen, e.g. {rows:?}");
    assert!(!rows.is_empty(), "playlist view drew no rows");
    assert!(h.query_by_label(&first).is_none(), "the playlist scrolled");

    // What a sync costs the UI thread: the messages carrying the library, then
    // the once-per-batch rebuild of derived data (the search index).
    let (albums, tracks) = {
        let app = app_mut(&mut h);
        (app.data.albums.to_vec(), app.data.tracks.all.to_vec())
    };
    let mut sync_ui = Vec::new();
    for _ in 0..5 {
        let app = app_mut(&mut h);
        // Grouping and indexing the tracks is the bridge's job (a worker thread).
        let set = Box::new(crate::tracks::TrackSet::new(tracks.clone()));
        let t = Instant::now();
        app.handle(Msg::Albums(albums.clone()));
        app.handle(Msg::AllTracks(set));
        app.after_messages();
        sync_ui.push(t.elapsed());
    }
    sync_ui.sort();

    // Settings → Library shows how much is left to analyse (only with a
    // server, so the frames above skip it): time the count itself.
    let unanalysed = {
        let app = app_mut(&mut h);
        let _ = app.unanalysed();
        let t = Instant::now();
        for _ in 0..10 {
            std::hint::black_box(app.unanalysed());
        }
        t.elapsed() / 10
    };

    println!("frame times ({ALBUMS_N} albums, {TRACKS_N} tracks, {QUEUE_N}-item queue):");
    for (name, t) in &results {
        println!("  {name:<26} mean {:>6.2} ms   p95 {:>6.2} ms   max {:>6.2} ms", ms(t.mean), ms(t.p95), ms(t.max));
    }

    for (name, d) in &first_sort {
        println!("  {name:<26} {:>6.2} ms", ms(*d));
    }
    println!("  unanalysed() per call: {:.2} ms", ms(unanalysed));
    println!("  sync messages + rebuild on the UI thread: median {:.2} ms", ms(sync_ui[2]));

    experimental_costs(&mut h);

    // Idle: nothing playing, nothing hovered, nothing animating. After the
    // frames settle, nothing should ask for another one.
    reset(&mut h, FULL, Route::Albums);
    {
        let app = app_mut(&mut h);
        app.engine.status.preview(PlayState::Stopped, 0.0, 0.0, 0.0);
    }
    h.remove_cursor();
    h.run_steps(40);
    let idle_repaint = h.ctx.has_requested_repaint();
    println!("  idle requests a repaint: {idle_repaint}");

    if !cfg!(debug_assertions) {
        for (name, t) in &results {
            assert!(t.mean < Duration::from_millis(4), "{name}: mean {:?}", t.mean);
        }
    }
    assert!(!idle_repaint, "idle frames keep asking for repaints");
}

/// Each experimental visual's frame cost (and the album art's colors'): the
/// same scenes with it off, then on.
fn experimental_costs(h: &mut Harness<'_, Option<OndApp>>) {
    type Switch = fn(&mut crate::settings::Settings);
    let features: [(&str, Switch); 4] = [
        ("colors from the art", |s| s.adapt_colors = true),
        ("blurred artwork", |s| s.experimental.backdrop = true),
        ("waveform seek bar", |s| s.experimental.waveform = true),
        ("live spectrum", |s| s.experimental.spectrum = true),
    ];
    println!("experimental visuals, mean frame time off → on:");
    for (name, on) in features {
        for (scene, player) in [("full window", false), ("Player", true)] {
            let setup = |h: &mut Harness<'_, Option<OndApp>>| {
                reset(h, if player { vec2(380.0, 880.0) } else { FULL }, Route::Recent);
                if player {
                    app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
                }
            };
            setup(h);
            let off = time_with_audio(h, 60);
            setup(h);
            app_mut(h).prefs.edit(on);
            fixture_waveform(app_mut(h));
            settle_look(h);
            let with = time_with_audio(h, 60);
            println!("  {name:<22} {scene:<12} {:>6.3} ms → {:>6.3} ms", ms(off), ms(with));
        }
    }
    // The spectrogram, with the Info tab showing.
    for (scene, player) in [("full window", false), ("Player", true)] {
        let setup = |h: &mut Harness<'_, Option<OndApp>>| {
            reset(h, if player { vec2(380.0, 880.0) } else { FULL }, Route::Recent);
            if player {
                app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
            }
            app_mut(h).ui.queue_ui.tab = QueueTab::Info;
        };
        setup(h);
        let off = time_with_audio(h, 60);
        setup(h);
        experiment(h, |x| x.spectrogram = true);
        let with = time_with_audio(h, 60);
        println!("  {:<22} {scene:<12} {:>6.3} ms → {:>6.3} ms", "spectrogram in Info", ms(off), ms(with));
    }
    // The visualizer, full window: each one's CPU side (the scene and the
    // paint callback; the GPU work isn't timed here) and the painter version.
    use crate::visuals::visualizer::{Kind, Pick};
    let target = Some(crate::visuals::gpu::Target::plain(eframe::egui_wgpu::wgpu::TextureFormat::Bgra8Unorm));
    for (scene, kind, gpu) in Kind::ALL.map(|k| (format!("{}, GPU (CPU side)", k.label()), k, target)).into_iter().chain(Kind::ALL.map(|k| (format!("{}, painter", k.label()), k, None))) {
        reset(h, FULL, Route::Recent);
        let off = time_with_audio(h, 60);
        reset(h, FULL, Route::Recent);
        experiment(h, |x| {
            x.visualizer = true;
            x.visualizer_pick = Pick::of(kind);
        });
        let app = app_mut(h);
        app.gpu = gpu;
        app.toggle_visualizer();
        let with = time_with_audio(h, 60);
        app_mut(h).close_visualizer();
        app_mut(h).gpu = None;
        println!("  {scene:<35} {:>6.3} ms → {:>6.3} ms", ms(off), ms(with));
    }
    reset(h, FULL, Route::Recent);
}

/// Mean time of `n` frames, each with fresh audio in the tap as if playing
/// (the copy itself is the output callback's cost, not the frame's).
fn time_with_audio(h: &mut Harness<'_, Option<OndApp>>, n: usize) -> Duration {
    steps_with_audio(h, 5);
    let mut total = Duration::ZERO;
    for _ in 0..n {
        feed_tap(app_mut(h), 1024);
        let t = Instant::now();
        h.step();
        total += t.elapsed();
    }
    total / n as u32
}

fn ms(d: Duration) -> f64 {

    d.as_secs_f64() * 1000.0
}
