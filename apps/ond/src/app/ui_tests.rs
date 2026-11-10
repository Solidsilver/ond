//! Behaviour tests on the real views, driven through egui_kittest: keys,
//! drags, clicks and what they change, on the fixture library. Unlike the
//! snapshots these run in CI.

use super::fixture::*;
use super::*;
use egui::{Vec2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use std::path::Path;

/// A song with long credits and a long ReplayGain line in the Info tab, then a
/// short one in the same app: at both column widths everything stays inside
/// the queue column (nothing widened it, nothing scrolled it sideways).
#[test]
fn info_stays_inside_its_column_after_a_long_song() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = full_window(dir.path());
    let check = |h: &mut Harness<'_, Option<OndApp>>, what: &str| {
        let right = app_mut(h).prefs_now().columns.right;
        let column = egui::Rect::from_x_y_ranges(FULL.x - right..=FULL.x, 0.0..=FULL.y);
        for label in ["Up next", "Info", "Elisha A. Hoffman", "Anthony Johnson Showalter"] {
            for n in h.query_all_by_label_contains(label) {
                let r = n.rect();
                if r.center().x < FULL.x - right - 40.0 {
                    continue; // the same word elsewhere in the window
                }
                assert!(column.contains(r.min) && r.max.x <= column.max.x + 0.5, "{what}: “{label}” at {r:?} outside the column {column:?}");
            }
        }
    };
    for right in [super::layout::RIGHT_DEFAULT, super::layout::RIGHT_MIN] {
        reset(&mut h, FULL, Route::Album(album_id(0)));
        {
            let mut c = app_mut(&mut h).prefs_now().columns;
            c.right = right;
            app_mut(&mut h).set_columns(c);
        }
        let saved = long_info(app_mut(&mut h));
        app_mut(&mut h).ui.queue_ui.tab = QueueTab::Info;
        h.run_steps(4);
        check(&mut h, "long song");
        app_mut(&mut h).player.queue.current = saved;
        h.run_steps(4);
        check(&mut h, "short song after the long one");
    }
    *h.state_mut() = None;
}

/// The column edges answer the pointer: a resize cursor over them, dragging
/// changes the width (within its range), and the widths are saved.
#[test]
fn column_edges_drag_and_persist() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = full_window(dir.path());
    reset(&mut h, FULL, Route::Recent);
    // (The harness's own panel has an 8 pt margin around the app.)
    let edge = pos2(8.0 + super::layout::LEFT_DEFAULT, 400.0);
    h.hover_at(edge);
    h.run_steps(2);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeHorizontal, "resize cursor over the sidebar's edge");
    h.drag_at(edge);
    h.run_steps(1);
    h.hover_at(edge + vec2(60.0, 0.0));
    h.run_steps(1);
    h.drop_at(edge + vec2(60.0, 0.0));
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).prefs_now().columns.left, super::layout::LEFT_DEFAULT + 60.0);
    // The queue's edge, dragged far out, stops at its limit.
    let q = pos2(FULL.x - 8.0 - super::layout::RIGHT_DEFAULT, 400.0);
    h.drag_at(q);
    h.run_steps(1);
    h.hover_at(q - vec2(600.0, 0.0));
    h.run_steps(1);
    h.drop_at(q - vec2(600.0, 0.0));
    h.run_steps(2);
    let c = app_mut(&mut h).prefs_now().columns;
    assert_eq!(c.right, c.right_limit(FULL.x));
    // Saved when let go, and restored from there.
    app_mut(&mut h).prefs.flush();
    let saved = Settings::read(&dir.path().join("settings.json")).expect("settings written");
    let restored = super::layout::Columns::restore(saved.sidebar_width, saved.queue_width);
    assert_eq!(restored, c);
    *h.state_mut() = None;
}

#[test]
fn the_now_playing_heart_toggles_both_ways() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = full_window(dir.path());
    reset(&mut h, FULL, Route::Recent);
    let id = app_mut(&mut h).player.queue.current.as_ref().unwrap().track.id.clone();
    let loved = |h: &mut Harness<'_, Option<OndApp>>| app_mut(h).favorites.is_on(StarKind::Song, &id);
    assert!(loved(&mut h), "the fixture's playing song starts loved");
    h.get_by_label(&crate::keys::shown("Unlove (⇧⌘L)")).click();
    h.run_steps(2);
    assert!(!loved(&mut h), "unloved from the now-playing display");
    // Clicking straight onto the heart again (the display under it reads as
    // not hovered while the pointer is on the heart) must love it again.
    h.get_by_label(&crate::keys::shown("Love (⇧⌘L)")).click();
    h.run_steps(2);
    assert!(loved(&mut h), "loved again from the now-playing display");
    *h.state_mut() = None;
}

/// Leaving a maximized full window: the compositor answers an un-maximize
/// with the window's old size, so the new size must wait until it has let go,
/// or the old size lands after it and wins (GNOME and KDE both do this). On
/// macOS a zoomed window simply takes the new size.
#[test]
fn leaving_a_maximized_window_resizes_after_it_lets_go() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), FULL, |ui, app| {
        app.main_ui(ui);
        app.track_window(ui.ctx());
    });
    reset(&mut h, FULL, Route::Recent);
    let set_max = |h: &mut Harness<'_, Option<OndApp>>, on: bool| {
        h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().maximized = Some(on);
    };
    set_max(&mut h, true);
    h.run_steps(2);
    let commands = |h: &mut Harness<'_, Option<OndApp>>| {
        h.step();
        h.output().viewport_output.values().flat_map(|v| v.commands.clone()).collect::<Vec<_>>()
    };
    let resized = |c: &[egui::ViewportCommand]| c.iter().any(|c| matches!(c, egui::ViewportCommand::InnerSize(_)));

    app_mut(&mut h).set_mode(Some(WindowMode::Mini));
    let first = commands(&mut h);
    if cfg!(target_os = "macos") {
        // Zoom is just a size there: straight to the new one.
        assert!(resized(&first) && !first.contains(&egui::ViewportCommand::Maximized(false)), "{first:?}");
        *h.state_mut() = None;
        return;
    }
    assert!(first.contains(&egui::ViewportCommand::Maximized(false)), "un-maximizes: {first:?}");
    assert!(!resized(&first), "but doesn't resize yet: {first:?}");
    for _ in 0..3 {
        let c = commands(&mut h);
        assert!(!resized(&c), "still maximized, still waiting: {c:?}");
    }
    set_max(&mut h, false);
    let after = commands(&mut h);
    assert!(after.contains(&egui::ViewportCommand::InnerSize(vec2(280.0, 280.0))), "the mini size, once let go: {after:?}");
    *h.state_mut() = None;
}

/// A long album name stays on one line in the Player, cut short, so it
/// doesn't push the transport and the queue down.
#[test]
fn a_long_album_name_doesnt_push_the_player_down() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), vec2(380.0, 880.0), |ui, app| app.main_ui(ui));
    reset(&mut h, vec2(380.0, 880.0), Route::Recent);
    app_mut(&mut h).edit_windows(|w| w.mode = WindowMode::Player);
    h.run_steps(3);
    let before = h.get_by_label("Details").rect();
    {
        let app = app_mut(&mut h);
        let id = app.player.queue.current.as_ref().unwrap().track.album_id.clone();
        let i = app.data.by_id[&id];
        Arc::make_mut(&mut app.data.albums)[i].name = "The Complete Recordings at the Village Vanguard, Including Alternate Takes and Rehearsals".into();
    }
    h.run_steps(3);
    assert_eq!(h.get_by_label("Details").rect().min.y, before.min.y, "everything below the title stays put");
    *h.state_mut() = None;
}

/// The play button over a hovered album cover plays the album. It covers
/// part of the tile, so the pointer on it must not count as leaving the tile.
#[test]
fn the_play_button_on_an_album_cover_plays_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = full_window(dir.path());
    reset(&mut h, FULL, Route::Recent);
    h.run_steps(2);
    let target = album_id(1);
    let label = {
        let a = app_mut(&mut h).data.album(&target).unwrap();
        format!("{} by {}", a.name, a.artist.as_deref().unwrap_or("Unknown artist"))
    };
    let tile = h.get_by_label(&label).rect();
    // Onto the cover, then onto its play button (bottom left of the art).
    h.hover_at(tile.center());
    h.run_steps(2);
    h.get_by_label("Play album").click();
    h.run_steps(3);
    // The album's tracks are queued to play (the test's null output can't
    // actually play them, so look at the queue, not at what's current).
    let q = &app_mut(&mut h).player.queue;
    let queued = q.history.iter().chain(&q.current).chain(&q.upcoming).filter(|i| matches!(&i.source, Source::Album { id, .. } if *id == target)).count();
    assert!(queued > 0, "the album was queued to play");
    *h.state_mut() = None;
}

/// Linux draws its own window buttons and resize edges over the content
/// (`csd`). They must take only their own spots: the Mini player still shows
/// its controls on hover, the toolbar's Mini button still works, and the
/// close button closes.
#[test]
fn our_window_frame_leaves_the_content_working() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = full_window(dir.path());
    let ctx = h.ctx.clone();
    crate::csd::preview(&ctx, true);
    crate::csd::set_window(&ctx, true, true);
    reset(&mut h, FULL, Route::Recent);
    h.run_steps(2);

    // The window's top edge, over the toolbar: dragging resizes, not moves.
    let sent = |h: &Harness<'_, Option<OndApp>>, cmd: &egui::ViewportCommand| h.output().viewport_output.values().any(|v| v.commands.contains(cmd));
    let top = pos2(FULL.x / 2.0, 1.0);
    let mut seen = Vec::new();
    for e in [
        egui::Event::PointerMoved(top),
        egui::Event::PointerButton { pos: top, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
        egui::Event::PointerMoved(top + vec2(0.0, 20.0)),
        egui::Event::PointerButton { pos: top + vec2(0.0, 20.0), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
    ] {
        h.event(e);
        h.step();
        seen.extend(h.output().viewport_output.values().flat_map(|v| v.commands.clone()));
    }
    assert!(seen.contains(&egui::ViewportCommand::BeginResize(egui::ResizeDirection::North)), "the top edge resizes: {seen:?}");
    assert!(!seen.contains(&egui::ViewportCommand::StartDrag), "and doesn't move the window");
    h.run_steps(2);

    h.get_by_label(&crate::keys::shown("Mini player (⇧⌘M)")).click();
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).prefs_now().mode, WindowMode::Mini, "the toolbar's Mini button switches");

    h.set_size(vec2(280.0, 280.0));
    h.hover_at(pos2(140.0, 140.0));
    h.run_steps(20);
    assert!(h.query_by_label("Next").is_some(), "hovering Mini shows its controls");

    // Back to the full window: the close button closes.
    app_mut(&mut h).edit_windows(|w| w.mode = WindowMode::Full);
    crate::csd::set_window(&ctx, true, true);
    h.set_size(FULL);
    h.run_steps(2);
    h.get_by_label("Close").click();
    h.run_steps(1);
    assert!(sent(&h, &egui::ViewportCommand::Close), "the close button asks the window to close");
    *h.state_mut() = None;
}

#[test]
fn list_keys_stay_with_an_open_sheet() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = full_window(dir.path());

    // Songs with a selection, Settings open on top.
    reset(&mut h, FULL, Route::Songs);
    let playing = |h: &mut Harness<'_, Option<OndApp>>| app_mut(h).player.queue.current.as_ref().map(|c| c.track.id.clone());
    let before = playing(&mut h);
    app_mut(&mut h).ui.songs.selected = Some(0);
    app_mut(&mut h).ui.overlay = Some(Overlay::Settings);
    h.run_steps(1);
    h.key_press(egui::Key::ArrowDown);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).ui.songs.selected, Some(0), "↓ moved the list under the sheet");
    assert_eq!(playing(&mut h), before, "⏎ played from the list under the sheet");

    // The queue with a selected item: ⌫ must not remove it.
    let app = app_mut(&mut h);
    let uid = app.player.queue.upcoming[0].uid;
    let len = app.player.queue.upcoming.len();
    app.ui.focus = Focus::Queue;
    app.ui.queue_ui.selected = Some(uid);
    h.run_steps(1);
    h.key_press(egui::Key::Backspace);
    h.key_press(egui::Key::Delete);
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).player.queue.upcoming.len(), len, "⌫ edited the queue under the sheet");

    // Closed again: the same keys reach the queue.
    app_mut(&mut h).ui.overlay = None;
    h.run_steps(1);
    h.key_press(egui::Key::Backspace);
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).player.queue.upcoming.len(), len - 1, "⌫ works once the sheet is closed");
    *h.state_mut() = None;
}

/// After the list of downloads couldn't be read, the Offline page says so;
/// "Remove them" asks first, then deletes the files the list doesn't name
/// and the unreadable list, and the notice goes.
#[test]
fn a_set_aside_list_of_downloads_is_shown_and_removed_after_asking() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = full_window(dir.path());
    let app = app_mut(&mut h);
    let store = app.downloads.store.take().expect("the store opens at start");
    let folder = store.dir().to_owned();
    drop(store);
    std::fs::write(folder.join("offline.json"), b"{\"pins\": [tor").unwrap();
    std::fs::write(folder.join("lost.flac"), vec![0u8; 3000]).unwrap();
    app.downloads.store = Some((app.downloads.open)(folder.clone(), None).unwrap());
    app.downloads.refresh();
    let bad = || std::fs::read_dir(&folder).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("offline.json.bad-")).count();
    assert_eq!(bad(), 1);

    reset(&mut h, FULL, Route::Offline);
    h.get_by_label("Ond couldn't read its list of downloads");
    h.get_by_label("1 file (3.0 KB) it no longer knows is kept in the downloads folder.");
    h.get_by_label("Remove them…").click();
    h.run_steps(2);
    assert!(folder.join("lost.flac").exists(), "nothing deleted before the answer");
    h.get_by_label("Delete").click();
    h.run_steps(2);
    let start = std::time::Instant::now();
    while app_mut(&mut h).downloads.snap.set_aside.is_some() {
        assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for the removal");
        std::thread::sleep(Duration::from_millis(10));
        app_mut(&mut h).downloads.refresh();
    }
    h.run_steps(2);
    assert!(!folder.join("lost.flac").exists());
    assert_eq!(bad(), 0);
    assert!(h.query_by_label("Ond couldn't read its list of downloads").is_none(), "the notice goes");
    *h.state_mut() = None;
}

/// Sign out in the server popover asks first: Cancel keeps the session, and
/// only the dialog's own Sign out signs out.
#[test]
fn signing_out_from_the_server_popover_waits_for_the_answer() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = full_window(dir.path());
    reset(&mut h, FULL, Route::Recent);
    let ask = |h: &mut Harness<'static, Option<OndApp>>| {
        h.get_by_label("Connected to music.example.com. Click for details.").click();
        h.run_steps(2);
        // A frame with the pointer over it first: the popover settles where it
        // goes, and the click lands on the item and not where it was.
        h.get_by_label("Sign out…").hover();
        h.run_steps(1);
        h.get_by_label("Sign out…").click();
        h.run_steps(3);
    };
    ask(&mut h);
    let app = app_mut(&mut h);
    assert!(matches!(&app.ui.overlay, Some(dialogs::Overlay::SignOut { host, from_settings: false }) if host == "music.example.com"));
    assert!(app.login.is_none() && app.session.online, "not signed out yet");
    h.get_by_label("Sign out of music.example.com?");
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    let app = app_mut(&mut h);
    assert!(app.ui.overlay.is_none() && app.login.is_none() && app.session.online, "Cancel keeps the session");

    ask(&mut h);
    h.get_by_label("Sign out").click();
    h.run_steps(3);
    let app = app_mut(&mut h);
    assert!(app.login.is_some() && !app.session.online, "signed out once confirmed");
    *h.state_mut() = None;
}

#[test]
fn cancelling_a_sign_out_asked_from_settings_goes_back_to_settings() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = full_window(dir.path());
    reset(&mut h, FULL, Route::Recent);
    app_mut(&mut h).apply(Action::OpenSettings(settings_page::Section::Account));
    h.run_steps(2);
    h.get_by_label("Sign out…").click();
    h.run_steps(3);
    assert!(matches!(app_mut(&mut h).ui.overlay, Some(dialogs::Overlay::SignOut { from_settings: true, .. })));
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    let app = app_mut(&mut h);
    assert!(matches!(app.ui.overlay, Some(dialogs::Overlay::Settings)), "Settings is back");
    assert!(app.login.is_none() && app.session.online, "still signed in");
    *h.state_mut() = None;
}

/// ⌘+, ⌘− and ⌘0 step the zoom (egui's own keys don't step it again), one
/// toast says where it is, and Settings shows the same level and sets it.
#[test]
fn the_zoom_keys_step_the_zoom_and_settings_follows() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = full_window(dir.path());
    reset(&mut h, FULL, Route::Recent);
    let cmd = egui::Modifiers::COMMAND;
    h.key_press_modifiers(cmd | egui::Modifiers::SHIFT, egui::Key::Plus);
    h.run_steps(2);
    assert_eq!(h.ctx.zoom_factor(), 1.1);
    h.key_press_modifiers(cmd, egui::Key::Equals);
    h.run_steps(2);
    assert_eq!(h.ctx.zoom_factor(), 1.25, "one step a press");
    let app = app_mut(&mut h);
    assert_eq!(app.prefs_now().zoom, 125);
    let zoom_toasts: Vec<_> = app.ui.toasts.texts().into_iter().filter(|t| t.starts_with("Zoom")).map(str::to_owned).collect();
    assert_eq!(zoom_toasts, ["Zoom 125%"], "the last toast replaces the one before");
    h.key_press_modifiers(cmd, egui::Key::Minus);
    h.run_steps(2);
    assert_eq!(h.ctx.zoom_factor(), 1.1);
    app_mut(&mut h).apply(Action::OpenSettings(settings_page::Section::Appearance));
    h.run_steps(2);
    h.get_by_label("zoom, 110%");
    h.get_by_label("More zoom").click();
    h.run_steps(2);
    assert_eq!((app_mut(&mut h).prefs_now().zoom, h.ctx.zoom_factor()), (125, 1.25));
    h.get_by_label("zoom, 125%");
    // ⌘0 back to actual size, with Settings still open.
    h.key_press_modifiers(cmd, egui::Key::Num0);
    h.run_steps(2);
    assert_eq!((app_mut(&mut h).prefs_now().zoom, h.ctx.zoom_factor()), (100, 1.0));
    *h.state_mut() = None;
}

/// A zoom set in managed.json is the one drawn, and neither the keys nor
/// Settings change it.
#[test]
fn a_managed_zoom_stays_as_managed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("managed.json"), r#"{ "zoom": 125 }"#).unwrap();
    let mut h = full_window(dir.path());
    reset(&mut h, FULL, Route::Recent);
    assert_eq!(app_mut(&mut h).prefs_now().zoom, 125);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Num0);
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).prefs_now().zoom, 125, "locked");
    app_mut(&mut h).apply(Action::OpenSettings(settings_page::Section::Appearance));
    h.run_steps(2);
    assert!(egui_kittest::kittest::NodeT::accesskit_node(&h.get_by_label("More zoom")).is_disabled(), "the stepper is locked");
    assert_eq!(h.ctx.zoom_factor(), 1.25, "and drawn at it from the start");
    *h.state_mut() = None;
}

/// The full window with the fixture library, for the behaviour tests below.
fn full_window(dir: &Path) -> Harness<'static, Option<OndApp>> {
    window(dir, FULL, |ui, app| app.main_ui(ui))
}

/// A window `size` big with the fixture library, drawn by `draw`.
fn window(dir: &Path, size: Vec2, draw: fn(&mut egui::Ui, &mut OndApp)) -> Harness<'static, Option<OndApp>> {
    let mut h = Harness::builder().with_size(size).allow_missing_glyphs().build_ui_state(
        move |ui, app: &mut Option<OndApp>| {
            if let Some(app) = app {
                draw(ui, app);
            }
        },
        None,
    );
    let ctx = h.ctx.clone();
    *h.state_mut() = Some(populated(&ctx, dir));
    h
}

/// Holding the alphabet rail's last letter takes the album grid to the end,
/// and its first back to the start.
#[test]
fn the_alphabet_rail_jumps_the_grid() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = full_window(dir.path());
    reset(&mut h, FULL, Route::Albums);
    app_mut(&mut h).ui.sort = AlbumSort::Title;
    looks(&mut h, |s| s.alpha_rail = true);
    h.run_steps(2);
    let label = |h: &mut Harness<'_, Option<OndApp>>, last: bool| {
        let albums = &app_mut(h).data.albums;
        let pick = |a: &&library::Album| a.name.to_lowercase();
        let a = if last { albums.iter().max_by_key(pick) } else { albums.iter().min_by_key(pick) }.unwrap();
        format!("{} by {}", a.name, a.artist.as_deref().unwrap_or("Unknown artist"))
    };
    let (first, last) = (label(&mut h, false), label(&mut h, true));
    assert!(h.query_by_label(&first).is_some() && h.query_by_label(&last).is_none(), "the grid starts at the top");
    let hold = |h: &mut Harness<'_, Option<OndApp>>, frac: f32| {
        let rail = h.get_by_label("Jump to letter").rect();
        let at = pos2(rail.center().x, rail.top() + rail.height() * frac);
        h.event(egui::Event::PointerMoved(at));
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
        h.run_steps(4);
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
        h.run_steps(2);
    };
    hold(&mut h, 0.98);
    assert!(h.query_by_label(&last).is_some() && h.query_by_label(&first).is_none(), "the last letter shows the end");
    hold(&mut h, 0.02);
    assert!(h.query_by_label(&first).is_some(), "the first letter goes back to the start");
    *h.state_mut() = None;
}

/// The Songs view keeps the keyboard selection in view when the keys move
/// it, but the wheel can still scroll it away.
#[test]
fn songs_selection_scrolls_away_with_the_wheel() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = full_window(dir.path());
    reset(&mut h, FULL, Route::Songs);
    app_mut(&mut h).ui.focus = Focus::Main;
    let row_label = |h: &mut Harness<'_, Option<OndApp>>, row: usize| {
        let app = app_mut(h);
        let order = app.view().songs_order();
        let t = &app.data.tracks.all[order[row]];
        let playing = app.player.queue.current.as_ref().is_some_and(|c| c.track.id == t.id);
        format!("{}, {}{}", t.title, t.artist.as_deref().unwrap_or(""), if playing { ", playing" } else { "" })
    };
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).ui.songs.selected, Some(0));
    let first = row_label(&mut h, 0);
    assert!(h.query_by_label(&first).is_some(), "the selected row shows");

    // Small wheel steps: each leaves the row part-cut for a frame or two.
    wheel(&mut h, pos2(FULL.x * 0.4, FULL.y * 0.6), -400.0, 50);
    assert_eq!(app_mut(&mut h).ui.songs.selected, Some(0), "the selection stays");
    assert!(h.query_by_label(&first).is_none(), "the wheel scrolled the selected row away");

    // ↓ brings the new selection back into view.
    let second = row_label(&mut h, 1);
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(40);
    assert_eq!(app_mut(&mut h).ui.songs.selected, Some(1));
    assert!(h.query_by_label(&second).is_some(), "↓ scrolled to the selection");
    *h.state_mut() = None;
}

/// The playlist name field takes the keyboard each time the dialog opens,
/// however the last one was closed.
#[test]
fn name_dialog_focuses_its_field_after_esc() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = full_window(dir.path());
    reset(&mut h, FULL, Route::Recent);
    let focused = |h: &Harness<'_, Option<OndApp>>| h.ctx.memory(|m| m.focused().is_some());
    for round in 0..3 {
        app_mut(&mut h).ui.overlay = Some(Overlay::new_playlist(Vec::new()));
        h.run_steps(3);
        assert!(focused(&h), "round {round}: the name field has the keyboard");
        h.key_press(egui::Key::Escape);
        h.run_steps(3);
        assert!(app_mut(&mut h).ui.overlay.is_none(), "round {round}: Esc closed the dialog");
    }
    *h.state_mut() = None;
}

/// A click on the seek line seeks to where it was clicked; a preview left
/// by a drag that ended elsewhere doesn't stick.
#[test]
fn seek_line_click_seeks_there() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = full_window(dir.path());
    reset(&mut h, FULL, Route::Recent);
    h.get_by_label("Position").click();
    h.run_steps(2);
    let to = app_mut(&mut h).player.pending_seek.expect("the click seeked");
    assert!((to - 383.0 / 2.0).abs() < 2.0, "seeked to the middle of 383 s, got {to}");

    // A screen reader hears the position and the length.
    {
        use egui_kittest::kittest::NodeT;
        let node = h.get_by_label("Position").accesskit_node();
        assert_eq!(node.min_numeric_value(), Some(0.0));
        assert_eq!(node.max_numeric_value(), Some(383.0));
        assert!(node.numeric_value().is_some_and(|v| (0.0..=383.0).contains(&v)), "a position: {:?}", node.numeric_value());
        assert!(node.value().is_some_and(|v| v.ends_with(" of 6:23")), "spoken as a time: {:?}", node.value());
    }

    // A drag in another window mode that never ended here.
    app_mut(&mut h).ui.seek_preview = Some(10.0);
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).ui.seek_preview, None, "the stale preview went");
    *h.state_mut() = None;
}

/// A playlist of `n` songs, "Long song 0" onwards, owned by the user.
fn long_playlist(app: &mut OndApp, id: &str, n: usize) {
    let tracks: Vec<Track> = (0..n)
        .map(|k| {
            let mut t = filler(20, 1).remove(0);
            t.id = format!("long-{k}");
            t.title = format!("Long song {k}");
            t
        })
        .collect();
    app.data.playlists.push(subsonic::Playlist { id: id.into(), name: "Long".into(), song_count: n as u32, duration: 0, cover_art: None, owner: None });
    app.data.playlist_tracks.insert(id.into(), Arc::new(tracks));
}

/// A long playlist lays out only the rows on screen, and still reorders by
/// dragging, brings a song far down it into view when asked (as "Go to
/// album" does), and keeps the keyboard selection in view.
#[test]
fn a_long_playlist_reorders_reveals_and_follows_the_keys() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = full_window(dir.path());
    let id = "pl-long";
    long_playlist(app_mut(&mut h), id, 3000);
    reset(&mut h, FULL, Route::Playlist { id: id.into(), name: "Long".into() });
    let label = |k: usize| format!("Long song {k}");
    assert!(h.query_by_label(&label(0)).is_some(), "the first rows show");
    assert!(h.query_by_label(&label(2000)).is_none(), "rows off screen aren't laid out");

    // Drag the first song onto the lower half of the third: it lands after it.
    let (from, onto) = (h.get_by_label(&label(0)).rect(), h.get_by_label(&label(2)).rect());
    let (a, b) = (from.center(), pos2(onto.center().x, onto.center().y + onto.height() / 4.0));
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=6 {
        h.hover_at(a + (b - a) * (k as f32 / 6.0));
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(2);
    let order: Vec<String> = app_mut(&mut h).data.playlist_tracks[id].iter().take(4).map(|t| t.id.clone()).collect();
    assert_eq!(order, ["long-1", "long-2", "long-0", "long-3"], "the dragged song moved to just below the third");

    // Revealed: selected and scrolled to, though it was never laid out.
    h.remove_cursor();
    app_mut(&mut h).ui.reveal = Some("long-2000".into());
    h.run_steps(10);
    assert_eq!(app_mut(&mut h).ui.track_sel, Some(2000));
    assert!(app_mut(&mut h).ui.reveal.is_none(), "the reveal was used up");
    assert!(h.query_by_label(&label(2000)).is_some(), "scrolled to the revealed song");
    assert!(h.query_by_label(&label(0)).is_none());

    // The keys move the selection and bring it along.
    for _ in 0..30 {
        h.key_press(egui::Key::ArrowDown);
        h.run_steps(1);
    }
    h.run_steps(10);
    assert_eq!(app_mut(&mut h).ui.track_sel, Some(2030));
    assert!(h.query_by_label(&label(2030)).is_some(), "↓ kept the selection in view");
    *h.state_mut() = None;
}

/// Dragging the toolbar's volume slider sets the volume and leaves the
/// window where it is.
#[test]
fn dragging_the_volume_slider_doesnt_move_the_window() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = full_window(dir.path());
    reset(&mut h, FULL, Route::Recent);
    h.run_steps(2);

    let rect = h.get_by_label("Volume").rect();
    app_mut(&mut h).prefs.edit(|s| s.volume = 0.0);
    h.run_steps(2);
    let (from, to) = (rect.left_center() + vec2(6.0, 0.0), rect.right_center() - vec2(6.0, 0.0));
    let mut seen = Vec::new();
    for e in [
        egui::Event::PointerMoved(from),
        egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
        egui::Event::PointerMoved(from + vec2(10.0, 0.0)),
        egui::Event::PointerMoved(to),
        egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
    ] {
        h.event(e);
        h.step();
        seen.extend(h.output().viewport_output.values().flat_map(|v| v.commands.clone()));
    }
    assert!(!seen.contains(&egui::ViewportCommand::StartDrag), "the window stays put: {seen:?}");
    assert!(app_mut(&mut h).prefs_now().volume > 0.9, "the volume follows the drag");
}

/// Settings set in managed.json stay as set: the actions that would change
/// them do nothing, and the settings file keeps the user's own values.
#[test]
fn managed_settings_cant_be_changed_in_the_app() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("settings.json"),
        r#"{ "server": "https://music.example.com", "username": "demo", "scrobble": true, "accent": "rose", "alpha_rail": false }"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("managed.json"), MANAGED).unwrap();
    let mut h = full_window(dir.path());
    reset(&mut h, FULL, Route::Recent);
    let app = app_mut(&mut h);
    let s = app.prefs_now();
    assert!(!s.scrobble && s.alpha_rail && s.accent == theme::Accent::Teal, "managed values apply");
    assert_eq!(s.volume, 1.0, "the volume isn't a managed setting");
    app.actions.push(Action::Prefs(PrefsAction::Scrobble(true)));
    app.actions.push(Action::Prefs(PrefsAction::AlphaRail(false)));
    app.actions.push(Action::Prefs(PrefsAction::Theme(ThemePref::Dark)));
    app.actions.push(Action::Output(OutputAction::ToggleBitPerfect));
    app.actions.push(Action::Output(OutputAction::SelectDevice("spk".into())));
    h.run_steps(2);
    let app = app_mut(&mut h);
    let s = app.prefs_now();
    assert!(!s.scrobble && s.alpha_rail, "locked: unchanged");
    assert_eq!(s.theme, ThemePref::Dark, "what isn't managed still changes");
    let saved = app.prefs.snapshot();
    assert_eq!(saved.device_modes.get("umc"), Some(&crate::settings::DeviceMode::BitPerfect));
    assert_eq!(saved.output_device.as_deref(), Some("umc"));
    app.prefs.flush();
    let file = std::fs::read_to_string(dir.path().join("settings.json")).unwrap();
    let file: serde_json::Value = serde_json::from_str(&file).unwrap();
    assert_eq!((&file["scrobble"], &file["accent"]), (&serde_json::json!(true), &serde_json::json!("rose")), "the user's own values are kept: {file}");
    assert!(file.get("alpha_rail").is_none(), "and off, as theirs was (not written when off): {file}");
    assert_eq!(file["theme"], "dark");
}

/// Every control in every view has a name a screen reader can read out (and
/// a test can find it by). The harness checks this only after `run`, which
/// most tests here don't call, so this walks the views and asks each time.
#[test]
fn every_view_names_its_controls() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = full_window(dir.path());
    let routes = [
        Route::Recent,
        Route::Albums,
        Route::Artists,
        Route::Songs,
        Route::Favorites,
        Route::Offline,
        Route::Album(album_id(0)),
        Route::Album(album_id(1)),
        Route::Playlist { id: "pl-0".into(), name: "Late night, low volume".into() },
        Route::Artist { name: "Bill Evans".into(), id: None },
    ];
    for route in routes {
        reset(&mut h, FULL, route);
        h.check_accessibility();
    }
    // The Offline page needs pins for `pin_row` to draw: one finished, one
    // on its way, one of each kind of thing that can be kept.
    {
        let pin = |pin: offline::Pin, label: &str, done: usize| offline::PinSummary {
            pin,
            label: label.into(),
            total: 12,
            done,
            failed: 0,
            bytes_done: 0,
            bytes_total: 0,
            downloading: done < 12,
        };
        let app = app_mut(&mut h);
        let pins = vec![
            pin(offline::Pin::Album(album_id(0)), "Album pin", 12),
            pin(offline::Pin::Playlist("pl-0".into()), "Playlist pin", 5),
            pin(offline::Pin::Track("t-3".into()), "Song pin", 12),
        ];
        app.downloads.snap = std::sync::Arc::new(offline::Snapshot { pins, ..(*app.downloads.snap).clone() });
    }
    reset(&mut h, FULL, Route::Offline);
    assert!(h.query_all_by_label_contains("Playlist pin").next().is_some(), "the pinned rows drew");
    h.check_accessibility();
    for tab in [QueueTab::UpNext, QueueTab::History, QueueTab::Lyrics, QueueTab::Info] {
        reset(&mut h, FULL, Route::Recent);
        app_mut(&mut h).ui.queue_ui.tab = tab;
        h.run_steps(3);
        h.check_accessibility();
        // And the empty state each of the queue's tabs shows with nothing in it.
        let q = &mut app_mut(&mut h).player.queue;
        q.upcoming.clear();
        q.history.clear();
        q.current = None;
        h.run_steps(3);
        h.check_accessibility();
    }
    reset(&mut h, FULL, Route::Album(album_id(0)));
    app_mut(&mut h).actions.push(Action::OpenPalette);
    h.run_steps(2);
    {
        let app = app_mut(&mut h);
        if let (Some(p), Some(index)) = (app.ui.palette.as_mut(), app.search.index.as_mut()) {
            p.query = "bob".into();
            p.refresh(index);
        }
    }
    h.run_steps(2);
    assert!(app_mut(&mut h).ui.palette.is_some(), "the palette is open");
    h.check_accessibility();
    for form in [
        session::LoginForm { server: "http://localhost:4533".into(), ..Default::default() },
        session::LoginForm { server: "https://music.example.com".into(), username: "demo".into(), busy: true, ..Default::default() },
    ] {
        reset(&mut h, FULL, Route::Recent);
        app_mut(&mut h).login = Some(form);
        h.run_steps(2);
        h.check_accessibility();
    }
    for section in [
        settings_page::Section::General,
        settings_page::Section::Appearance,
        settings_page::Section::Playback,
        settings_page::Section::Queue,
        settings_page::Section::Output,
        settings_page::Section::Offline,
        settings_page::Section::Library,
        settings_page::Section::Account,
        settings_page::Section::About,
    ] {
        reset(&mut h, FULL, Route::Recent);
        app_mut(&mut h).actions.push(Action::OpenSettings(section));
        h.run_steps(2);
        h.check_accessibility();
    }
    for (mode, size) in [(WindowMode::Player, vec2(380.0, 880.0)), (WindowMode::Mini, vec2(280.0, 280.0))] {
        reset(&mut h, size, Route::Recent);
        app_mut(&mut h).edit_windows(|w| w.mode = mode);
        h.hover_at(pos2(size.x / 2.0, size.y / 2.0));
        h.run_steps(3);
        h.check_accessibility();
    }
    *h.state_mut() = None;
}

/// With nothing queued and nothing playing, the album grid still stops at the
/// queue column: the column's empty state doesn't hand its room to the page.
#[test]
fn an_empty_queue_keeps_its_column() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = full_window(dir.path());
    reset(&mut h, FULL, Route::Recent);
    {
        let app = app_mut(&mut h);
        app.player.queue = crate::queue::Queue::default();
        app.engine.status.preview(PlayState::Stopped, 0.0, 0.0, 0.0);
    }
    h.run_steps(4);
    let right = app_mut(&mut h).prefs_now().columns.right;
    let edge = FULL.x - right;
    assert!(h.query_by_label("Up next").is_some_and(|n| n.rect().left() >= edge), "the queue's tabs are in its column");
    let mut tiles = 0;
    for (title, ..) in ALBUMS {
        for n in h.query_all_by_label_contains(title).filter(|n| n.rect().left() < edge) {
            tiles += 1;
            assert!(n.rect().right() <= edge, "“{title}” at {:?} reaches into the queue column (from x {edge})", n.rect());
        }
    }
    assert!(tiles > 0, "the grid was drawn");
    *h.state_mut() = None;
}
