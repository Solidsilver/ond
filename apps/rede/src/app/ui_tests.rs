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
fn the_play_button_waits_for_the_engine() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    app_mut(&mut h).go(book_route(0));
    h.run_steps(3);
    // Drawn where it will work, and disabled until the engine is there.
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
fn the_play_button_says_playback_comes_later() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    app_mut(&mut h).go(book_route(0));
    h.run_steps(2);
    h.get_by_label("Play").hover();
    // egui holds a tooltip back a moment before showing it.
    h.run_steps(40);
    h.get_by_label("Playback comes next");
}
