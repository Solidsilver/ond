//! Headless frames of the real views: catches layout panics, and checks the
//! views wire up to the actions and the messages the app handles. No server,
//! no keychain: the fixture writes the shelf the app reads.

use super::fixture::*;
use super::*;
use crate::settings::LibrarySort;

#[test]
fn every_view_draws() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path(), BOOKS.len());
    frame(&ctx, &mut app);
    // The library in each order, then a book's page, then one the shelf no
    // longer has (a stale link, or a listing that dropped it).
    for sort in LibrarySort::ALL {
        app.apply_prefs(PrefsAction::Sort(sort));
        frame(&ctx, &mut app);
    }
    let mut routes = vec![Route::Library, book_route(0), book_route(5), Route::Book("book-gone".into())];
    for route in routes.drain(..) {
        app.go(route);
        for _ in 0..2 {
            frame(&ctx, &mut app);
        }
    }
    playing(&mut app, false);
    frame(&ctx, &mut app);
    app.player.paused = true;
    frame(&ctx, &mut app);
    for mode in [crate::window::Mode::Player, crate::window::Mode::Mini] {
        app.mode = mode;
        frame(&ctx, &mut app);
    }
    app.mode = crate::window::Mode::Full;
    // The trail went Library → book → book → gone; back walks it.
    assert!(app.ui.nav.can_back());
    app.back();
    frame(&ctx, &mut app);
    assert_eq!(app.ui.nav.current(), &book_route(5));
    // And the sign-in screen, with and without an error.
    let mut login = signed_out(&ctx, dir.path());
    frame(&ctx, &mut login);
    login.login.as_mut().unwrap().error = Some("Wrong username or password.".into());
    frame(&ctx, &mut login);
}

#[test]
fn a_book_page_asks_for_its_details_once_and_keeps_them() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path(), BOOKS.len());
    app.books.details.clear();
    app.go(book_route(0));
    // The page asks the shelf (and the source) and says it's waiting.
    assert!(matches!(app.books.details.get("book-0"), Some(Details::Loading)));
    // What comes back is kept, and opening the page again doesn't ask anew.
    let item = super::fixture::details(0);
    app.handle(crate::bridge::LibraryMsg::Book { key: sources::ItemKey::new(super::fixture::account().id, "book-0"), item: Box::new(item) });
    assert!(matches!(app.books.details.get("book-0"), Some(Details::Loaded(_))));
    app.go(Route::Library);
    app.go(book_route(0));
    assert!(matches!(app.books.details.get("book-0"), Some(Details::Loaded(_))));
}

#[test]
fn signing_in_with_an_empty_form_says_what_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = signed_out(&ctx, dir.path());
    // No server, no username, no password: the first empty field is named, and
    // nothing is sent.
    app.sign_in();
    assert_eq!(app.login.as_ref().unwrap().error.as_deref(), Some("Enter the server's address, for example https://books.example.com."));
    app.login.as_mut().unwrap().server = "https://books.example.com".into();
    app.sign_in();
    assert_eq!(app.login.as_ref().unwrap().error.as_deref(), Some("Enter your username."));
    app.login.as_mut().unwrap().username = "sam".into();
    app.sign_in();
    assert_eq!(app.login.as_ref().unwrap().error.as_deref(), Some("Enter your password."));
}

#[test]
fn signing_out_forgets_the_tokens_and_keeps_the_books() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path(), BOOKS.len());
    let session = app.bridge.session();
    let shelf = app.session.shelf.clone();
    app.sign_out();
    assert!(app.login.is_some(), "the sign-in screen comes back");
    assert_eq!(app.login.as_ref().unwrap().username, "sam", "filled in with who was signed in");
    assert!(!app.session.signed_in());
    assert_eq!(app.books.len(), 0, "the shelf's books are no longer shown");
    assert!(app.bridge.session() > session, "work for the old session is dropped");
    assert!(shelf.is_some(), "the shelf itself was kept");
    frame(&ctx, &mut app);
}

#[test]
fn a_rate_limited_server_is_waited_out_without_a_word() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path(), BOOKS.len());
    let before = app.books.len();
    app.sync.running = true;
    app.sync_done(crate::sync::Outcome::Busy { retry_after: Some(std::time::Duration::from_secs(30)) });
    assert!(app.ui.notice.is_none(), "a rate limit is not an error to show");
    assert!(app.sync.retry_at.is_some(), "and it's tried again later");
    assert_eq!(app.books.len(), before, "the books already there stay");
}

#[test]
fn a_signed_out_server_returns_to_the_sign_in_screen_saying_so() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path(), BOOKS.len());
    app.sync.running = true;
    app.sync_done(crate::sync::Outcome::SignedOut);
    let form = app.login.as_ref().expect("the sign-in screen");
    assert_eq!(form.error.as_deref(), Some(crate::failure::SIGNED_OUT));
    assert!(!app.session.signed_in());
}

#[test]
fn a_network_failure_is_shown_and_retried_quietly() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path(), BOOKS.len());
    app.sync.running = true;
    app.sync_done(crate::sync::Outcome::Failed(crate::failure::Failure::Network("Couldn't reach the server.".into())));
    assert_eq!(app.ui.notice.as_ref().map(|n| n.text.as_str()), Some("Couldn't reach the server."));
    assert!(app.sync.retry_at.is_some());
    // What failed on this computer isn't retried: nothing would change.
    app.sync.retry_at = None;
    app.sync_done(crate::sync::Outcome::Failed(crate::failure::Failure::Local("Couldn't read the shelf: no room".into())));
    assert!(app.sync.retry_at.is_none());
}

#[test]
fn a_listing_remembers_the_libraries_for_the_next_start() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path(), 0);
    let report = crate::sync::Report { libraries: vec![crate::settings::KnownLibrary { id: LIBRARY.into(), name: "Books".into() }], items: 12, removed: 0 };
    app.sync.running = true;
    app.sync_done(crate::sync::Outcome::Done(report));
    assert_eq!(app.session.account.as_ref().unwrap().libraries.len(), 1);
    assert_eq!(app.prefs.get(|s| s.account().map(|a| a.libraries.len())), Some(1), "saved with the account");
}

/// A frame with nothing moving asks for no further frame: the window draws when
/// the system or the user gives it a reason, and not otherwise. (A spinner
/// that's spinning, a cover on its way or a loading page is a reason; this
/// fixture has none.)
#[test]
fn an_idle_window_asks_for_no_frames() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(FULL[0], FULL[1])).with_pixels_per_point(2.0).build_ui_state(
        |ui, app: &mut Option<RedeApp>| {
            if let Some(app) = app {
                app.main_ui(ui);
            }
        },
        None,
    );
    let ctx = h.ctx.clone();
    *h.state_mut() = Some(app(&ctx, dir.path(), BOOKS.len()));
    idle(&mut h, "an idle library");
    // A book page too, once its details are there.
    app_mut(&mut h).go(book_route(0));
    idle(&mut h, "an idle book page");
    playing(app_mut(&mut h), true);
    idle(&mut h, "a paused player");
}

/// Runs frames until nothing asks for another, then a few more, and says the
/// frames stayed quiet. Settling first is fair: a cover is looked for on disk
/// once, and the frame that miss lands on is a frame with something to do.
fn idle(h: &mut egui_kittest::Harness<'static, Option<RedeApp>>, what: &str) {
    let mut quiet = 0;
    for _ in 0..600 {
        h.step();
        quiet = if h.ctx.has_requested_repaint() { 0 } else { quiet + 1 };
        if quiet >= 3 {
            break;
        }
    }
    assert_eq!(quiet, 3, "{what} never settled");
    for _ in 0..20 {
        h.step();
        assert!(!h.ctx.has_requested_repaint(), "{what} keeps asking for repaints");
    }
}

fn app_mut<'a>(h: &'a mut egui_kittest::Harness<'static, Option<RedeApp>>) -> &'a mut RedeApp {
    h.state_mut().as_mut().expect("the fixture app")
}

#[test]
fn queued_playback_results_from_a_signed_out_account_are_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path(), BOOKS.len());
    let session = app.bridge.session();
    app.sign_out();
    app.handle(Msg::Playback {
        session,
        event: crate::listening::Event::Positions(vec![sources::Progress {
            item: sources::ItemKey::new(account().id, "book-0"),
            position: sources::Position::from_secs(500.0),
            finished: false,
            at: None,
        }]),
    });
    assert!(app.positions.is_empty());
    app.handle(Msg::Engine { session, event: audio::Event::Error { track_id: None, message: "old device".into() } });
    assert!(app.ui.notice.is_none());
}
