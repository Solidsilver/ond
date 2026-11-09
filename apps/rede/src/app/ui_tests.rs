//! Behaviour tests on the real views, driven through egui_kittest: what a
//! click does, what a control is called, and where the text lands. These run in
//! CI; the PNGs in `snapshots.rs` are for looking at.

use super::fixture::*;
use super::*;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use std::path::Path;

/// A window with the app in it, its state left for the test to look at.
fn window(dir: &Path, books: usize) -> Harness<'static, Option<RedeApp>> {
    let mut h = Harness::builder().with_size(egui::vec2(FULL[0], FULL[1])).with_pixels_per_point(2.0).with_os(egui::os::OperatingSystem::Mac).build_ui_state(
        |ui, app: &mut Option<RedeApp>| {
            if let Some(app) = app {
                if app.login.is_some() {
                    app.login_ui(ui);
                } else {
                    app.main_ui(ui);
                }
            }
        },
        None,
    );
    let ctx = h.ctx.clone();
    *h.state_mut() = Some(app(&ctx, dir, books));
    h
}

fn app_mut<'a>(h: &'a mut Harness<'static, Option<RedeApp>>) -> &'a mut RedeApp {
    h.state_mut().as_mut().expect("the fixture app")
}

/// Every control a route paints is named for the accessibility tree, which is
/// also how a test (or a screen reader) finds it.
#[test]
fn every_view_names_its_controls() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    for route in [Route::Library, book_route(0), Route::Book("book-gone".into())] {
        app_mut(&mut h).go(route);
        h.run_steps(3);
        h.check_accessibility();
    }
    for paused in [false, true] {
        playing(app_mut(&mut h), paused);
        app_mut(&mut h).go(book_route(0));
        h.run_steps(3);
        h.check_accessibility();
        for label in ["Previous chapter (⇧←)", "Back 15 seconds", "Forward 30 seconds", "Next chapter (⇧→)", "Chapter position", "Playback speed, 1.5×"]
        {
            h.get_by_label(label);
        }
        h.get_by_label(if paused { "Play" } else { "Pause" });
    }
    for mode in [crate::window::Mode::Player, crate::window::Mode::Mini] {
        app_mut(&mut h).mode = mode;
        let size = mode.size();
        h.set_size(egui::vec2(size[0], size[1]));
        h.run_steps(3);
        h.check_accessibility();
        for label in ["Back 15 seconds", "Forward 30 seconds", "Play"] {
            h.get_by_label(label);
        }
        if mode == crate::window::Mode::Player {
            h.get_by_label("Chapter position");
            h.get_by_label("Sleep timer");
        }
    }
    app_mut(&mut h).mode = crate::window::Mode::Full;
    h.set_size(egui::vec2(FULL[0], FULL[1]));
    // The sign-in screen, and the same with an error under the button.
    *h.state_mut() = Some(signed_out(&h.ctx.clone(), dir.path()));
    h.run_steps(3);
    h.check_accessibility();
}

#[test]
fn a_cover_opens_its_book_and_back_returns() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    h.run_steps(2);
    h.get_by_label_contains("Dune by Frank Herbert").click();
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).ui.nav.current(), &Route::Book("book-1".into()));
    h.get_by_label("← Back").click();
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).ui.nav.current(), &Route::Library);
}

#[test]
fn the_sort_pill_reorders_the_shelf_and_is_saved() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    h.run_steps(2);
    h.get_by_label("Author").click();
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).prefs.get(|s| s.sort), crate::settings::LibrarySort::Author);
    // The first cover on screen is the first book by author: Brontë's.
    let first = h.query_all_by_label_contains(" by ").next().map(|n| format!("{n:?}")).unwrap_or_default();
    assert!(first.contains("Jane Eyre"), "the shelf is in author order: {first}");
}

#[test]
fn the_book_page_offers_play_and_lists_its_chapters() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    app_mut(&mut h).go(book_route(0));
    h.run_steps(3);
    // The page has a usable control before any device has been opened.
    let play = h.get_by_label("Play").rect();
    assert!(play.width() > 0.0, "the Play button is drawn on the page");
    assert_eq!(h.query_all_by_label_contains("A Wizard").count(), 0, "the library is not on screen");
    assert!(h.query_all_by_label_contains("Chapter 1").count() >= 1, "and the chapters are listed");
}

#[test]
fn a_book_page_lists_its_chapters_with_their_lengths() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    app_mut(&mut h).go(book_route(0));
    h.run_steps(3);
    // Twelve chapters of 56:40 each, and the page says so once per row.
    assert_eq!(h.query_all_by_label_contains("56:40").count(), 12, "each chapter shows its length");
    assert_eq!(h.query_all_by_label_contains("Read by George Guidall").count(), 1, "the narrator is on the page");
    assert_eq!(h.query_all_by_label_contains("Hainish #1").count(), 1, "and the series and number");
}

#[test]
fn an_empty_library_says_so_instead_of_showing_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), 0);
    h.run_steps(2);
    h.get_by_label("No books yet");
}

#[test]
fn signing_out_is_in_the_account_menu() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    h.run_steps(2);
    h.get_by_label("sam").click();
    h.run_steps(2);
    h.get_by_label("Sign out").click();
    h.run_steps(2);
    assert!(app_mut(&mut h).login.is_some(), "the sign-in screen comes back");
    assert!(!app_mut(&mut h).session.signed_in());
    h.get_by_label("Sign in");
}

#[test]
fn play_without_a_connection_explains_how_to_continue() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    app_mut(&mut h).go(book_route(0));
    h.run_steps(2);
    h.get_by_label("Play").click();
    h.run_steps(2);
    h.get_by_label("Not connected. Sign in to play this book.");
}

#[test]
fn the_player_exposes_speed_and_listening_preferences() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    playing(app_mut(&mut h), true);
    app_mut(&mut h).go(book_route(0));
    h.run_steps(3);
    h.get_by_label_contains("Resume ·");
    h.get_by_label("Previous chapter (⇧←)");
    h.get_by_label("Next chapter (⇧→)");
    h.get_by_label("Playback speed, 1.5×").click();
    h.run_steps(2);
    assert_eq!(h.query_all_by_label("Speed").count(), 2);
    for speed in ["1×", "1.25×", "1.5×", "2×"] {
        h.get_by_label(speed);
    }
    h.get_by_label("Stop playback");
    h.check_accessibility();
}

#[test]
fn the_player_controls_fit_inside_the_bottom_panel() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    playing(app_mut(&mut h), true);
    for size in [FULL, [crate::MIN_SIZE[0], 560.0]] {
        h.set_size(egui::vec2(size[0], size[1]));
        h.run_steps(3);
        for label in ["Previous chapter (⇧←)", "Back 15 seconds", "Play", "Forward 30 seconds", "Next chapter (⇧→)", "Chapter position", "Playback speed, 1.5×"]
        {
            let rect = h.get_by_label(label).rect();
            assert!(rect.min.y >= size[1] - 160.0 && rect.max.y <= size[1], "{label}: {rect:?}");
            assert!(rect.min.x >= 0.0 && rect.max.x <= size[0], "{label}: {rect:?}");
        }
    }
}

#[test]
fn the_player_bar_buttons_enqueue_their_controls() {
    use crate::player::Control;
    // Only the bar, with nothing applying what it asks for, so the test
    // sees the actions themselves.
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(FULL[0], FULL[1])).build_ui_state(
        |ui, app: &mut Option<RedeApp>| {
            if let Some(app) = app {
                app.player_panel(ui);
            }
        },
        None,
    );
    let ctx = h.ctx.clone();
    let mut app = app(&ctx, dir.path(), BOOKS.len());
    playing(&mut app, true);
    *h.state_mut() = Some(app);
    h.run_steps(2);
    for (label, expected) in [
        ("Previous chapter (⇧←)", Control::PreviousChapter),
        ("Back 15 seconds", Control::Skip(-15.0)),
        ("Play", Control::Toggle),
        ("Forward 30 seconds", Control::Skip(30.0)),
        ("Next chapter (⇧→)", Control::NextChapter),
    ] {
        h.get_by_label(label).click();
        h.run_steps(2);
        let actions = std::mem::take(&mut app_mut(&mut h).actions);
        assert!(matches!(actions.as_slice(), [Action::Playback(control)] if *control == expected), "{label}: {} actions", actions.len());
    }
}

#[test]
fn playback_shortcuts_enqueue_the_same_controls_as_the_buttons() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path(), BOOKS.len());
    playing(&mut app, true);
    for (key, modifiers, expected) in [
        (egui::Key::Space, egui::Modifiers::NONE, crate::player::Control::Toggle),
        (egui::Key::ArrowLeft, egui::Modifiers::NONE, crate::player::Control::Skip(-15.0)),
        (egui::Key::ArrowRight, egui::Modifiers::NONE, crate::player::Control::Skip(30.0)),
        (egui::Key::ArrowLeft, egui::Modifiers::SHIFT, crate::player::Control::PreviousChapter),
        (egui::Key::ArrowRight, egui::Modifiers::SHIFT, crate::player::Control::NextChapter),
        (egui::Key::OpenBracket, egui::Modifiers::NONE, crate::player::Control::Speed(1.45)),
        (egui::Key::CloseBracket, egui::Modifiers::NONE, crate::player::Control::Speed(1.55)),
    ] {
        let input =
            egui::RawInput { events: vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }], ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| app.playback_keys(ui.ctx()));
        out.textures_delta.clear();
        assert!(matches!(app.actions.pop(), Some(Action::Playback(control)) if control == expected));
    }
}

#[test]
fn window_buttons_and_shortcuts_work_without_a_loaded_book() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    h.run_steps(3);
    h.get_by_label("Player window (⌥⌘2)").click();
    h.run_steps(3);
    assert_eq!(app_mut(&mut h).mode, crate::window::Mode::Player);
    h.get_by_label("Mini player (⇧⌘M)").click();
    h.run_steps(3);
    assert_eq!(app_mut(&mut h).mode, crate::window::Mode::Mini);
    h.get_by_label("Full window (⌥⌘1)").click();
    h.run_steps(3);
    assert_eq!(app_mut(&mut h).mode, crate::window::Mode::Full);
    h.get_by_label("Keyboard shortcuts (⌘/)").click();
    h.run_steps(3);
    h.get_by_label("Close shortcuts").click();
    h.run_steps(3);
    assert!(!app_mut(&mut h).shortcuts);
}

#[test]
fn sleep_options_and_custom_defaults_are_accessible_from_the_bar() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    playing(app_mut(&mut h), true);
    h.run_steps(3);
    h.get_by_label("Sleep timer").click();
    h.run_steps(3);
    h.check_accessibility();
    for label in ["5 minutes", "120 minutes", "End of chapter", "Custom minutes", "Start 30 minute timer"] {
        h.get_by_label(label);
    }
}

#[test]
fn the_window_shortcuts_match_onds_bindings_without_a_loaded_book() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path(), BOOKS.len());
    let alt = egui::Modifiers::COMMAND | egui::Modifiers::ALT;
    let shift = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
    for (key, modifiers, expected) in [
        (egui::Key::Num1, alt, crate::window::Mode::Full),
        (egui::Key::Num2, alt, crate::window::Mode::Player),
        (egui::Key::Num3, alt, crate::window::Mode::Mini),
        (egui::Key::N, shift, crate::window::Mode::Player),
        (egui::Key::M, shift, crate::window::Mode::Mini),
    ] {
        let input =
            egui::RawInput { events: vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }], ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| app.window_keys(ui.ctx()));
        out.textures_delta.clear();
        assert!(matches!(app.actions.pop(), Some(Action::Mode(mode)) if mode == expected));
    }
    let input = egui::RawInput {
        events: vec![egui::Event::Key { key: egui::Key::Slash, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND }],
        ..Default::default()
    };
    let mut out = ctx.run_ui(input, |ui| app.window_keys(ui.ctx()));
    out.textures_delta.clear();
    assert!(matches!(app.actions.pop(), Some(Action::Shortcuts)));
}
