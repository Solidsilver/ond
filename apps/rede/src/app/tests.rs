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
    for filter in crate::settings::LibraryFilter::ALL {
        app.apply_prefs(PrefsAction::Filter(filter));
        frame(&ctx, &mut app);
    }
    app.apply_prefs(PrefsAction::Filter(crate::settings::LibraryFilter::All));
    let mut routes = vec![
        Route::Home,
        Route::Library,
        Route::Downloads,
        Route::Stats,
        Route::AllSeries,
        Route::Series("Hainish".into()),
        Route::Series("A series gone".into()),
        book_route(0),
        book_route(5),
        Route::Book("book-gone".into()),
    ];
    for route in routes.drain(..) {
        app.go(route);
        for _ in 0..2 {
            frame(&ctx, &mut app);
        }
    }
    // Stats with a year of listening, then with this computer's alone.
    listening_history(&mut app);
    app.go(Route::Stats);
    frame(&ctx, &mut app);
    app.stats.history = None;
    app.stats.arrived(Vec::new(), Some(Err(crate::failure::Failure::Server("The server answered with an error (500).".into()))));
    frame(&ctx, &mut app);
    app.back();
    // Settings on each tab.
    for tab in super::settings_page::Tab::ALL {
        app.apply(Action::OpenSettings(tab));
        frame(&ctx, &mut app);
    }
    // The question before signing out, asked from Settings.
    app.apply(Action::AskSignOut);
    frame(&ctx, &mut app);
    app.apply(Action::CloseSheet);
    assert_eq!(app.sheet, Some(super::sheets::Sheet::Settings));
    app.apply(Action::CloseSheet);
    playing(&mut app, false);
    frame(&ctx, &mut app);
    app.player.paused = true;
    frame(&ctx, &mut app);
    // The chapters column on each tab, the bookmarks empty and then not.
    app.apply_panel(super::panel::PanelAction::Show(super::panel::Tab::Chapters));
    for tab in [super::panel::Tab::Chapters, super::panel::Tab::Bookmarks, super::panel::Tab::Info] {
        app.apply_panel(super::panel::PanelAction::Tab(tab));
        frame(&ctx, &mut app);
    }
    bookmarks(&mut app);
    app.apply_panel(super::panel::PanelAction::Tab(super::panel::Tab::Bookmarks));
    frame(&ctx, &mut app);
    // The toasts: a bookmark made, and a position from another device.
    app.bookmarked("Chapter 5", 14_200.0);
    reconciled(&mut app);
    frame(&ctx, &mut app);
    for mode in [crate::window::Mode::Player, crate::window::Mode::Mini] {
        app.mode = mode;
        frame(&ctx, &mut app);
    }
    app.reconciliation = None;
    app.mode = crate::window::Mode::Full;
    // The sidebar as its rail, and the account syncing, connecting and
    // offline.
    app.apply_prefs(PrefsAction::CollapseSidebar(true));
    frame(&ctx, &mut app);
    app.apply_prefs(PrefsAction::CollapseSidebar(false));
    for state in 0..3 {
        (app.sync.running, app.session.connecting, app.sync.offline) = (state == 0, state == 1, state == 2);
        frame(&ctx, &mut app);
    }
    (app.sync.running, app.session.connecting, app.sync.offline) = (false, false, false);
    // The palette: empty, a query, a name chosen, and nothing found.
    app.apply(Action::OpenPalette);
    frame(&ctx, &mut app);
    for query in ["le guin", "zzqx"] {
        if let Some(p) = app.ui.palette.as_mut() {
            p.typed(query);
        }
        frame(&ctx, &mut app);
    }
    app.apply(Action::PaletteName("Ursula K. Le Guin".into()));
    frame(&ctx, &mut app);
    app.apply(Action::ClosePalette);
    // The trail went Library → book → book → gone; back walks it.
    assert!(app.ui.nav.can_back());
    app.back();
    frame(&ctx, &mut app);
    assert_eq!(app.ui.nav.current(), &book_route(5));
    // Home and the library again, with books partway and one finished.
    listened(&mut app);
    for route in [Route::Home, Route::Library] {
        app.go(route);
        for _ in 0..2 {
            frame(&ctx, &mut app);
        }
    }
    // Downloads in every state: the place, each book's page, the library
    // narrowed to what's here, and all of it offline.
    downloaded(&mut app);
    for offline in [false, true] {
        app.sync.offline = offline;
        for route in [Route::Downloads, book_route(0), book_route(2), book_route(7), book_route(8), book_route(9), book_route(1), Route::Home, Route::Library] {
            app.go(route);
            frame(&ctx, &mut app);
        }
    }
    app.sync.offline = false;
    app.apply_prefs(PrefsAction::Filter(crate::settings::LibraryFilter::Downloaded));
    frame(&ctx, &mut app);
    app.apply_prefs(PrefsAction::Filter(crate::settings::LibraryFilter::All));
    // And the sign-in screen, with and without an error.
    let mut login = signed_out(&ctx, dir.path());
    frame(&ctx, &mut login);
    login.login.as_mut().unwrap().error = Some("Wrong username or password.".into());
    frame(&ctx, &mut login);
    drop(login);
    for (both, waiting) in [(false, false), (true, false), (false, true)] {
        let mut login = signin(&ctx, dir.path(), both, waiting);
        frame(&ctx, &mut login);
    }
    // Passwords only, a password on its way, and a browser sign-in through
    // Audiobookshelf's link.
    let mut login = signin_with(&ctx, dir.path(), &["local"], None);
    frame(&ctx, &mut login);
    login.login.as_mut().unwrap().busy = true;
    login.session.connecting = true;
    frame(&ctx, &mut login);
    drop(login);
    let mut login = signin_with(&ctx, dir.path(), &["openid"], Some(pending_openid_fallback()));
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
#[cfg(not(target_os = "macos"))]
fn unsupported_browser_sign_in_leaves_the_password_form_available() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = signed_out(&ctx, dir.path());
    let mut status = abs::ServerStatus { auth_methods: vec!["openid".into(), "local".into()], ..Default::default() };
    status.auth_form_data.auth_open_id_auto_launch = true;
    app.handle(crate::bridge::AccountMsg::Status { session: app.bridge.session(), status, warning: None, server: "https://books.example.com".into() });
    assert!(!app.login.as_ref().unwrap().busy, "the password form remains available");
    assert!(app.openid.pending.is_none());
    app.apply(Action::OpenId);
    assert!(app.login.as_ref().unwrap().error.as_deref().unwrap().contains("only available on macOS"));
    assert!(!app.login.as_ref().unwrap().busy);
}

#[test]
fn a_bare_address_becomes_the_full_one_the_server_answered_at() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = signed_out(&ctx, dir.path());
    app.login.as_mut().unwrap().server = "nas.local:13378".into();
    app.handle(crate::bridge::AccountMsg::Status {
        session: app.bridge.session(),
        status: abs::ServerStatus { auth_methods: vec!["local".into()], ..Default::default() },
        warning: None,
        server: "http://nas.local:13378".into(),
    });
    let form = app.login.as_ref().unwrap();
    assert_eq!(form.server, "http://nas.local:13378", "the field shows the address that worked");
    assert_eq!(form.checked_server, form.server, "and signing in uses it as it stands");
    assert!(form.status.is_some());
}

#[test]
fn signing_in_with_an_empty_form_says_what_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = signed_out(&ctx, dir.path());
    // No server, no username, no password: the first empty field is named, and
    // nothing is sent.
    app.sign_in();
    assert_eq!(app.login.as_ref().unwrap().error.as_deref(), Some("Enter the server's address, for example books.example.com."));
    let form = app.login.as_mut().unwrap();
    form.server = "https://books.example.com".into();
    form.checked_server = form.server.clone();
    form.status = Some(abs::ServerStatus { auth_methods: vec!["local".into()], ..Default::default() });
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
    app_mut(&mut h).apply_panel(super::panel::PanelAction::Show(super::panel::Tab::Chapters));
    idle(&mut h, "the chapters column, paused");
    app_mut(&mut h).apply_prefs(PrefsAction::CollapseSidebar(true));
    idle(&mut h, "the sidebar's rail");
    app_mut(&mut h).sync.offline = true;
    idle(&mut h, "an account offline");
    // Downloads on their way move only when the store says so.
    downloaded(app_mut(&mut h));
    app_mut(&mut h).go(Route::Downloads);
    idle(&mut h, "the Downloads place with a book on its way");
    app_mut(&mut h).go(book_route(2));
    idle(&mut h, "a book page downloading");
    // A running sync is something moving: its spinner turns (for at most
    // half a minute, which is more frames than this test steps).
    app_mut(&mut h).sync.running = true;
    h.step();
    assert!(h.ctx.has_requested_repaint(), "the spinner turns at first");
}

#[test]
fn the_column_toggle_is_remembered_and_opening_it_finds_the_chapter_playing() {
    use super::panel::{PanelAction, Tab};
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path(), BOOKS.len());
    frame(&ctx, &mut app);
    assert!(!app.prefs.get(|s| s.show_panel), "hidden until it's asked for");
    app.apply_panel(PanelAction::Toggle);
    assert!(app.prefs.get(|s| s.show_panel), "saved with the preferences");
    assert!(app.ui.panel.reveal, "the chapter playing is brought into view");
    frame(&ctx, &mut app);
    assert!(!app.ui.panel.reveal, "once");
    app.apply_panel(PanelAction::Tab(Tab::Info));
    assert!(!app.ui.panel.reveal, "another tab is just another tab");
    app.apply_panel(PanelAction::Toggle);
    assert!(!app.prefs.get(|s| s.show_panel));
    // A narrow window opens the sheet instead, and leaves the preference be.
    app.ui.panel.narrow = true;
    app.apply_panel(PanelAction::Show(Tab::Chapters));
    assert!(app.ui.panel.sheet && !app.prefs.get(|s| s.show_panel));
    assert_eq!(app.ui.panel.tab, Tab::Chapters);
}

/// The sign-in screen is still too (once nothing's being typed), and so is one waiting on the browser once
/// its spinner has had its few seconds: the browser may take minutes.
#[test]
fn a_sign_in_screen_asks_for_no_frames_even_while_the_browser_takes_its_time() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(FULL[0], FULL[1])).with_pixels_per_point(2.0).build_ui_state(
        |ui, app: &mut Option<RedeApp>| {
            if let Some(app) = app {
                app.login_ui(ui);
            }
        },
        None,
    );
    let ctx = h.ctx.clone();
    *h.state_mut() = Some(signin(&ctx, dir.path(), true, false));
    // A caret blinking in the server's field is a reason to draw; with
    // nothing focused, there's none.
    h.step();
    h.ctx.memory_mut(|m| m.stop_text_input());
    idle(&mut h, "the sign-in form");
    *h.state_mut() = Some(signin(&ctx, dir.path(), false, true));
    h.step();
    assert!(h.ctx.has_requested_repaint(), "the spinner turns at first");
    idle(&mut h, "a sign-in waiting on the browser");
    assert!(app_mut(&mut h).openid.pending.is_some(), "still waiting, only still");
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

/// The app signed in to a server that knows no progress and takes every write,
/// so a test can follow a progress intent all the way out. What arrived is in
/// the returned fake.
fn listening_server(app: &mut RedeApp) -> testkit::JsonFake {
    let server = testkit::JsonFake::start(|seen| match seen.path() {
        "/api/me" => testkit::Answer::ok(r#"{"id":"user","mediaProgress":[]}"#),
        "/api/me/bookmarks" => testkit::Answer::ok(r#"{"bookmarks":[]}"#),
        _ => testkit::Answer::ok(""),
    });
    let client =
        abs::Client::new(server.base(), abs::DeviceInfo::new("fixture", "1")).expect("the fake's address").with_tokens(abs::Tokens::new("access", "refresh"));
    app.session.source = Some(std::sync::Arc::new(sources::AbsSource::new(account().id, client, Vec::new())));
    server
}

/// What the progress endpoints were sent, in order.
fn progress_bodies(server: &testkit::JsonFake) -> Vec<serde_json::Value> {
    server.seen().into_iter().filter(|seen| seen.method == "PATCH" && seen.path().starts_with("/api/me/progress")).map(|seen| seen.json()).collect()
}

/// A place a pull found, for the tests that start from one.
fn saved(app: &mut RedeApp, id: &str, secs: f64, finished: bool) {
    let progress = sources::Progress {
        item: sources::ItemKey::new(account().id, id),
        position: sources::Position::from_secs(secs),
        finished,
        at: Some(std::time::SystemTime::now() - Duration::from_secs(60)),
    };
    app.positions.insert(id.to_owned(), progress);
}

#[test]
fn a_book_marked_finished_shows_finished_at_once_and_keeps_the_stamp_it_was_written_with() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path(), BOOKS.len());
    let server = listening_server(&mut app);
    let key = sources::ItemKey::new(account().id, "book-0");
    saved(&mut app, "book-0", 1_200.0, false);
    let version = app.positions.version();
    app.apply(Action::MarkFinished("book-0".into()));
    let shown = app.positions.get("book-0").expect("the book is still shown");
    assert!(shown.finished, "the finish shows at once");
    assert_eq!(shown.position.secs(), 1_200.0, "a finished book stays where it got to");
    let stamp = sources::Stamp::from(shown.at.unwrap()).millis();
    assert!(app.positions.version() > version, "what's derived from the positions refreshes");
    testkit::wait_until("the finish to reach the server", || progress_bodies(&server).len() == 1);
    let bodies = progress_bodies(&server);
    let body = &bodies[0][0];
    assert_eq!(body["libraryItemId"], "book-0");
    assert_eq!(body["currentTime"], 1_200.0);
    assert_eq!(body["isFinished"], true);
    assert_eq!(body["lastUpdate"], stamp, "the server keeps the log's stamp, not its own clock");
    testkit::wait_until("the shelf to keep the finish", || app.session.shelf.as_ref().unwrap().position(&key).unwrap().is_some_and(|p| p.finished));
}

#[test]
fn a_book_marked_unfinished_goes_back_to_the_place_it_was_left_at() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path(), BOOKS.len());
    let server = listening_server(&mut app);
    // A phone finished the book a third of the way in.
    saved(&mut app, "book-0", 1_234.5, true);
    app.apply(Action::MarkUnfinished("book-0".into()));
    let shown = app.positions.get("book-0").expect("the book is still shown");
    assert!(!shown.finished);
    assert_eq!(shown.position.secs(), 1_234.5, "the known place is kept, not the start");
    testkit::wait_until("the un-finish to reach the server", || progress_bodies(&server).len() == 1);
    let bodies = progress_bodies(&server);
    assert_eq!(bodies[0][0]["currentTime"], 1_234.5);
    assert!(bodies[0][0].get("isFinished").is_none(), "the reset clears the finish; this writes the place");
}

#[test]
fn starting_a_book_over_moves_the_loaded_player_to_the_start_and_keeps_it_paused() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path(), BOOKS.len());
    let server = listening_server(&mut app);
    playing(&mut app, true);
    app.apply(Action::StartOver("book-0".into()));
    assert_eq!(app.player.position, 0.0, "the loaded book moves to the start");
    assert!(app.player.paused, "and stays as it was");
    let shown = app.positions.get("book-0").expect("the book is still shown");
    assert_eq!(shown.position.secs(), 0.0);
    assert!(!shown.finished);
    testkit::wait_until("the start to reach the server", || progress_bodies(&server).len() == 1);
    assert_eq!(progress_bodies(&server)[0][0]["currentTime"], 0.0);
}

#[test]
fn a_progress_intent_from_a_signed_out_account_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path(), BOOKS.len());
    let _server = listening_server(&mut app);
    let session = app.bridge.session();
    app.apply(Action::MarkFinished("book-0".into()));
    assert!(app.positions.get("book-0").is_some_and(|p| p.finished));
    app.apply(Action::SignOut);
    let late = sources::Progress {
        item: sources::ItemKey::new(account().id, "book-0"),
        position: sources::Position::from_secs(1_200.0),
        finished: true,
        at: Some(std::time::SystemTime::now()),
    };
    app.handle(Msg::Playback { session, event: crate::listening::Event::Positions(vec![late]) });
    assert!(app.positions.is_empty(), "the signed-out account's positions stay gone");
    assert!(app.ui.notice.is_none());
}

#[test]
fn needing_credentials_returns_a_small_window_to_full_size() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path(), BOOKS.len());
    app.set_mode(crate::window::Mode::Mini);
    app.needs_sign_in(None);
    assert_eq!(app.mode, crate::window::Mode::Full);
    assert_eq!(app.prefs.get(|s| s.window.mode), crate::window::Mode::Full);
    assert!(app.login.is_some());
    frame(&ctx, &mut app);
}

#[test]
fn a_mismatched_browser_redirect_cannot_finish_or_cancel_sign_in() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = signin(&ctx, dir.path(), false, true);
    let session = app.bridge.session();
    app.handle(AccountMsg::Redirect("rede://oauth?state=wrong&code=untrusted".into()));
    app.handle(AccountMsg::Redirect("rede://oauth?state=wrong&error=access_denied".into()));
    app.handle(AccountMsg::Redirect("audiobookshelf://oauth?state=fixture-state&code=untrusted".into()));
    assert!(app.openid.pending.is_some());
    assert!(!app.openid.exchanging);
    assert_eq!(app.bridge.session(), session);
    assert!(app.login.as_ref().unwrap().error.is_none());
}

#[test]
fn cancel_and_timeout_drop_the_attempt_and_ignore_late_redirects() {
    for timeout in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = signin(&ctx, dir.path(), false, true);
        let session = app.bridge.session();
        if timeout {
            let deadline = app.openid.pending.as_ref().unwrap().started + crate::openid::TIMEOUT;
            app.expire_openid(deadline - Duration::from_secs(1));
            assert!(app.openid.pending.is_some());
            app.expire_openid(deadline);
        } else {
            app.apply(Action::CancelSignIn);
        }
        assert!(app.openid.pending.is_none());
        assert!(!app.login.as_ref().unwrap().busy);
        assert!(app.login.as_ref().unwrap().error.as_ref().unwrap().contains(if timeout { "timed out" } else { "cancelled" }));
        assert!(app.bridge.session() > session);
        app.handle(AccountMsg::Redirect("rede://oauth?state=fixture-state&code=late".into()));
        assert!(!app.openid.exchanging);
        assert!(app.openid.pending.is_none());
    }
}

#[test]
fn a_launch_redirect_waits_for_keyring_recovery_and_is_then_validated() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = signed_out(&ctx, dir.path());
    app.handle(AccountMsg::Redirect("rede://oauth?state=wrong&code=launch".into()));
    assert!(app.openid.early_redirect.is_some());
    app.handle(AccountMsg::OpenIdRestored { session: 0, pending: Some(Box::new(pending_openid())) });
    assert!(app.openid.restored);
    assert!(app.openid.early_redirect.is_none());
    assert!(app.openid.pending.is_some());
    assert!(!app.openid.exchanging, "an unmatched launch URL leaves the real attempt available");
}

#[test]
fn a_cancelled_discovery_cannot_reopen_the_browser_or_restore_a_login() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = signed_out(&ctx, dir.path());
    let session = app.bridge.session();
    app.apply(Action::CancelSignIn);
    app.handle(AccountMsg::OpenIdReady { session, pending: Box::new(pending_openid()) });
    app.handle(AccountMsg::OpenIdRestored { session, pending: Some(Box::new(pending_openid())) });
    app.handle(AccountMsg::NoTokens { session });
    assert!(app.openid.pending.is_none());
    assert!(!app.login.as_ref().unwrap().busy);
    assert!(app.login.as_ref().unwrap().error.as_ref().unwrap().contains("cancelled"));
}

/// ⌘B in the window's keys, as a frame reads them.
fn command_b(ctx: &egui::Context, app: &mut RedeApp) {
    let modifiers = egui::Modifiers::COMMAND;
    let key = egui::Event::Key { key: egui::Key::B, physical_key: None, pressed: true, repeat: false, modifiers };
    let input = egui::RawInput { events: vec![key], ..Default::default() };
    let mut out = ctx.run_ui(input, |ui| app.window_keys(ui.ctx()));
    out.textures_delta.clear();
}

#[test]
fn command_b_asks_for_a_bookmark_where_the_book_is_named_for_its_chapter() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path(), BOOKS.len());
    command_b(&ctx, &mut app);
    assert!(app.actions.is_empty(), "with nothing loaded there's nowhere to mark");
    playing(&mut app, true);
    for mode in [crate::window::Mode::Full, crate::window::Mode::Player, crate::window::Mode::Mini] {
        app.mode = mode;
        command_b(&ctx, &mut app);
        let Some(Action::AddBookmark { item, time, title }) = app.actions.pop() else { panic!("⌘B asks for a bookmark in {mode:?}") };
        assert_eq!(item, sources::ItemKey::new(account().id, "book-0"));
        assert_eq!(time, 14_200.0);
        assert_eq!(title, "Chapter 5");
    }
}

/// A confirmation fades on its own, and once it's gone the window is still
/// again; the other-device notice waits for an answer without drawing.
#[test]
fn toasts_stop_asking_for_frames() {
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
    playing(app_mut(&mut h), true);
    app_mut(&mut h).bookmarked("Chapter 5", 14_200.0);
    h.step();
    assert!(h.ctx.has_requested_repaint(), "a confirmation fades");
    idle(&mut h, "a faded confirmation");
    assert!(app_mut(&mut h).ui.toasts.texts().is_empty());
    reconciled(app_mut(&mut h));
    idle(&mut h, "the other-device notice");
    assert_eq!(app_mut(&mut h).ui.toasts.texts().len(), 1, "still offering Undo");
}

#[test]
fn the_window_opens_on_home_only_when_a_book_is_in_progress() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    // Nothing begun: the library, as before, and a later pull doesn't move it.
    let mut app = super::fixture::app(&ctx, dir.path(), BOOKS.len());
    app.playback_event(crate::listening::Event::Positions(vec![sources::Progress {
        item: sources::ItemKey::new(account().id, "book-3"),
        position: sources::Position::from_secs(0.0),
        finished: false,
        at: None,
    }]));
    assert_eq!(app.ui.nav.current(), &Route::Library);
    listened(&mut app);
    assert_eq!(app.ui.nav.current(), &Route::Library, "the first answer decided; a later one never moves the window");
    drop(app);
    // A book partway: Home, with no step back to the library.
    let mut app = super::fixture::app(&ctx, dir.path(), BOOKS.len());
    listened(&mut app);
    assert_eq!(app.ui.nav.current(), &Route::Home);
    assert!(!app.ui.nav.can_back());
    drop(app);
    // An empty first answer (a fresh shelf) waits for the server's, but not
    // past the user going somewhere.
    let mut app = super::fixture::app(&ctx, dir.path(), BOOKS.len());
    app.playback_event(crate::listening::Event::Positions(Vec::new()));
    app.go(book_route(3));
    listened(&mut app);
    assert_eq!(app.ui.nav.current(), &book_route(3));
}

#[test]
fn home_lists_the_books_in_progress_last_heard_first_and_the_newest() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, dir.path(), BOOKS.len());
    listened(&mut app);
    let titles = |ids: &[usize], app: &RedeApp| ids.iter().map(|&i| app.books.all()[i].title.clone()).collect::<Vec<_>>();
    let ids = app.books.in_progress(&app.positions);
    assert_eq!(titles(&ids, &app), ["Piranesi", "Dune", "Kindred", "Wolf Hall"]);
    assert!(std::sync::Arc::ptr_eq(&ids, &app.books.in_progress(&app.positions)), "worked out once, not every frame");
    let recent = app.books.recent();
    assert_eq!(titles(&recent[..2], &app), ["The Left Hand of Darkness", "Dune"]);
}
