//! Frame-time budget on a large library: 2,000 albums, 25,000 tracks and a
//! 500-item queue. Headless (no GPU): it times egui's layout and
//! tessellation of the real views, which is the CPU cost per frame.
//!
//! `cargo test -p listener --release perf -- --ignored --nocapture`

use super::*;
use std::time::{Duration, Instant};

const ALBUMS_N: usize = 2_000;
const TRACKS_N: usize = 25_000;
const QUEUE_N: usize = 500;

fn big_library(app: &mut ListenerApp) {
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

fn big_queue(app: &mut ListenerApp) {
    let tracks: Vec<Track> = app.data.all_tracks.iter().take(QUEUE_N).cloned().collect();
    app.queue = Queue::default();
    app.queue.play_now(tracks, 0, Source::Album { id: "big-0".into(), name: "A long queue".into() });
}

struct Timing {
    mean: Duration,
    p95: Duration,
    max: Duration,
}

/// Times `n` frames after a short warm-up.
fn time_frames(h: &mut Harness<'_, Option<ListenerApp>>, n: usize) -> Timing {
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

fn scene(h: &mut Harness<'_, Option<ListenerApp>>, route: Route) {
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
            |ui, app: &mut Option<ListenerApp>| {
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

    let mut results: Vec<(&str, Timing)> = Vec::new();

    scene(&mut h, Route::Albums);
    results.push(("albums grid", time_frames(&mut h, 60)));

    scene(&mut h, Route::Songs);
    h.hover_at(pos2(700.0, 500.0));
    for _ in 0..40 {
        h.event(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, -20_000.0), modifiers: egui::Modifiers::NONE, phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel });
        h.step();
    }
    results.push(("songs, mid-scroll", time_frames(&mut h, 60)));
    // Sanity: the frames really drew the list, scrolled away from the top.
    let rows: Vec<String> = h.query_all_by_label_contains(" part ").map(|n| format!("{n:?}").chars().take(80).collect()).take(2).collect();
    println!("  songs rows on screen, e.g. {rows:?}");
    assert!(!rows.is_empty(), "songs view drew no rows");

    scene(&mut h, Route::Album("big-7".into()));
    results.push(("album page", time_frames(&mut h, 60)));

    scene(&mut h, Route::Recent);
    results.push(("recent + 500-item queue", time_frames(&mut h, 60)));

    scene(&mut h, Route::Albums);
    app_mut(&mut h).actions.push(Action::OpenPalette);
    h.run_steps(2);
    {
        let app = app_mut(&mut h);
        if let (Some(p), Some(index)) = (app.palette.as_mut(), app.search.as_mut()) {
            p.query = "blue part".into();
            p.refresh(index);
        }
    }
    results.push(("palette open", time_frames(&mut h, 60)));

    println!("frame times ({ALBUMS_N} albums, {TRACKS_N} tracks, {QUEUE_N}-item queue):");
    for (name, t) in &results {
        println!("  {name:<26} mean {:>6.2} ms   p95 {:>6.2} ms   max {:>6.2} ms", ms(t.mean), ms(t.p95), ms(t.max));
    }

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

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
