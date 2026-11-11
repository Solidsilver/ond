//! Behaviour tests on the real views, driven through egui_kittest: what a
//! click does, what a control is called, and where the text lands. These run in
//! CI; the PNGs in `snapshots.rs` are for looking at.

use super::fixture::*;
use super::*;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use std::path::Path;

/// A window with the app in it, its state left for the test to look at.
fn window(dir: &Path, books: usize) -> Harness<'static, Option<RedeApp>> {
    let mut h = Harness::builder().with_size(egui::vec2(FULL[0], FULL[1])).with_pixels_per_point(2.0).with_os(egui::os::OperatingSystem::Mac).build_ui_state(
        |ui, app: &mut Option<RedeApp>| {
            if let Some(app) = app {
                whole_window(ui, |ui| if app.login.is_some() { app.login_ui(ui) } else { app.main_ui(ui) });
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

/// A store the test drives by hand: its changes are read with `refresh`,
/// as a change notice would.
fn open_store(app: &mut RedeApp, dir: &Path) -> offline::OfflineStore {
    let config = offline::OfflineConfig { dir: dir.join("downloads"), limit: None, reply_check: Default::default() };
    let store = offline::OfflineStore::open(config, app._rt.0.as_ref().unwrap().handle().clone(), std::sync::Arc::new(|| {})).unwrap();
    app.downloads.store.install(store.clone());
    app.downloads.refresh();
    store
}

#[test]
fn a_books_download_button_offers_what_its_state_allows() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    app_mut(&mut h).go(book_route(1));
    h.run_steps(3);
    h.get_by_label("Download");
    let store = open_store(app_mut(&mut h), dir.path());
    let pin = offline::Pin::Book(sources::ItemKey::new(account().id, "book-1").to_string());
    store.pin(pin.clone(), "Dune", vec![offline::TrackRef::new("dune-1", Some("m4b".into()), Some(100))]);
    app_mut(&mut h).downloads.refresh();
    h.run_steps(3);
    h.check_accessibility();
    // On its way: how far, with pause and remove behind it.
    h.get_by_label("Downloading 0%").click();
    h.run_steps(2);
    h.check_accessibility();
    h.get_by_label("Pause downloads").click();
    h.run_steps(2);
    assert!(store.snapshot().paused);
    app_mut(&mut h).downloads.refresh();
    h.run_steps(3);
    h.get_by_label("Paused · 0%").click();
    h.run_steps(2);
    h.get_by_label("Remove download").click();
    h.run_steps(2);
    assert!(!store.is_pinned(&pin));
    h.run_steps(2);
    h.get_by_label("Download");
    // Offline, a book that isn't here can't play or be fetched.
    app_mut(&mut h).sync.offline = true;
    h.run_steps(3);
    h.check_accessibility();
    h.get_by_label("Not downloaded");
    assert!(h.query_by_label("Download").is_none(), "nothing can be fetched offline");
    // The cover in the library says so too, and its menu offers no download.
    app_mut(&mut h).go(Route::Library);
    h.run_steps(3);
    h.get_by_label("Dune by Frank Herbert, not downloaded").click_secondary();
    h.run_steps(2);
    h.check_accessibility();
    h.get_by_label("Mark as finished");
    assert!(h.query_by_label("Download").is_none());
}

#[test]
fn a_covers_menu_downloads_the_book_and_the_place_lists_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    app_mut(&mut h).go(Route::Downloads);
    h.run_steps(3);
    h.get_by_label("Nothing downloaded");
    app_mut(&mut h).go(Route::Library);
    h.run_steps(3);
    h.get_by_label("Kindred by Octavia E. Butler").click_secondary();
    h.run_steps(2);
    h.get_by_label("Download").click();
    h.run_steps(2);
    // No store is open in the fixture, so it says why it can't.
    assert_eq!(app_mut(&mut h).ui.notice.as_ref().map(|n| n.text.as_str()), Some("Downloads aren't ready. Try again."));
    downloaded(app_mut(&mut h));
    h.run_steps(3);
    h.get_by_label("Kindred by Octavia E. Butler, downloaded");
    h.get_by_label("Piranesi by Susanna Clarke, downloading");
    app_mut(&mut h).go(Route::Downloads);
    h.run_steps(3);
    h.check_accessibility();
    h.get_by_label_contains("Kindred by Octavia E. Butler, 298 MB");
    h.get_by_label_contains("Piranesi by Susanna Clarke, Downloading 41%");
    h.get_by_label_contains("The Fifth Season by N. K. Jemisin, Download failed · Can't reach the server.");
    h.get_by_label("Pause all");
    h.get_by_label("Retry failed");
    h.get_by_label("More for Wolf Hall").click();
    h.run_steps(2);
    h.check_accessibility();
    h.get_by_label("Retry download");
    h.get_by_label("Remove download").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("Wolf Hall by").is_none());
    h.get_by_label_contains("Station Eleven by Emily St. John Mandel, Preparing…");
}

#[test]
fn settings_change_the_download_preferences_and_remove_everything_after_asking() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    downloaded(app_mut(&mut h));
    app_mut(&mut h).apply(Action::OpenSettings(super::settings_page::Tab::Downloads));
    h.run_steps(3);
    h.check_accessibility();
    h.get_by_label("storage limit, No limit");
    h.get_by_label("Less storage limit").click();
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).prefs.get(|s| s.downloads.limit), Some(500_000_000_000));
    h.get_by_role_and_label(egui::accesskit::Role::Switch, "Remove downloads when finished").click();
    h.run_steps(2);
    assert!(app_mut(&mut h).prefs.get(|s| s.downloads.remove_when_finished));
    h.get_by_label("Remove all…").click();
    h.run_steps(3);
    h.check_accessibility();
    h.get_by_label("Remove all 5 downloads?");
    h.get_by_label("Remove").click();
    h.run_steps(3);
    assert_eq!(app_mut(&mut h).sheet, Some(super::sheets::Sheet::Settings), "back to Settings");
    assert!(app_mut(&mut h).downloads.preparing.is_empty() && app_mut(&mut h).downloads.errors.is_empty());
}

/// Every control a route paints is named for the accessibility tree, which is
/// also how a test (or a screen reader) finds it.
#[test]
fn every_view_names_its_controls() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    for route in [
        Route::Home,
        Route::Library,
        Route::Downloads,
        Route::Stats,
        book_route(0),
        Route::Book("book-gone".into()),
        Route::AllSeries,
        Route::Series("Hainish".into()),
        Route::Series("A series gone".into()),
    ] {
        app_mut(&mut h).go(route);
        h.run_steps(3);
        h.check_accessibility();
    }
    // Stats with a year of listening: the tiles, the charts and the
    // rankings, the books among them opening their pages.
    listening_history(app_mut(&mut h));
    app_mut(&mut h).go(Route::Stats);
    h.run_steps(3);
    h.check_accessibility();
    // Downloads in each state: the place, the pages, offline too.
    downloaded(app_mut(&mut h));
    for offline in [false, true] {
        app_mut(&mut h).sync.offline = offline;
        for route in [Route::Downloads, book_route(0), book_route(2), book_route(7), book_route(9), book_route(1), Route::Library] {
            app_mut(&mut h).go(route);
            h.run_steps(3);
            h.check_accessibility();
        }
    }
    app_mut(&mut h).sync.offline = false;
    app_mut(&mut h).downloads = Default::default();
    // Home and the library with books partway and one finished: the cards,
    // their resume buttons, and the tiles say how far along each book is.
    listened(app_mut(&mut h));
    app_mut(&mut h).go(Route::Home);
    h.run_steps(3);
    h.check_accessibility();
    h.get_by_label("Resume Piranesi");
    h.get_by_label_contains("Piranesi by Susanna Clarke, 2h 10m left at 1×");
    app_mut(&mut h).go(Route::Library);
    h.run_steps(3);
    h.check_accessibility();
    h.get_by_label("A Wizard of Earthsea by Ursula K. Le Guin, finished");
    h.get_by_label("Piranesi by Susanna Clarke, 2h 10m left");
    h.get_by_label("The Dispossessed by Ursula K. Le Guin");
    // The order's menu, open, and the library narrowed to nothing.
    h.get_by_label("Sort: Title").click();
    h.run_steps(2);
    h.check_accessibility();
    h.key_press(egui::Key::Escape);
    app_mut(&mut h).apply_prefs(PrefsAction::Filter(crate::settings::LibraryFilter::Finished));
    app_mut(&mut h).positions.clear();
    h.run_steps(3);
    h.check_accessibility();
    h.get_by_label("Nothing finished yet");
    app_mut(&mut h).apply_prefs(PrefsAction::Filter(crate::settings::LibraryFilter::All));
    listened(app_mut(&mut h));
    h.run_steps(2);
    for tab in super::settings_page::Tab::ALL {
        app_mut(&mut h).apply(Action::OpenSettings(tab));
        h.run_steps(3);
        h.check_accessibility();
    }
    app_mut(&mut h).apply(Action::AskSignOut);
    h.run_steps(3);
    h.check_accessibility();
    app_mut(&mut h).sheet = None;
    for paused in [false, true] {
        playing(app_mut(&mut h), paused);
        app_mut(&mut h).go(book_route(0));
        h.run_steps(3);
        h.check_accessibility();
        for label in ["Previous chapter (⇧←)", "Back 15 seconds", "Forward 30 seconds", "Next chapter (⇧→)", "Chapter position", "Playback speed, 1.5×"]
        {
            h.get_by_label(label);
        }
        h.get_by_label(if paused { "Play (Space)" } else { "Pause (Space)" });
        h.get_by_label("Chapter 5, show chapters");
        h.get_by_label_contains("Now playing: ");
    }
    // The chapters column on each of its tabs, playing and not, and the
    // bookmarks with rows in them.
    app_mut(&mut h).apply_prefs(PrefsAction::ShowPanel(true));
    for tab in [super::panel::Tab::Chapters, super::panel::Tab::Bookmarks, super::panel::Tab::Info] {
        app_mut(&mut h).ui.panel.tab = tab;
        h.run_steps(3);
        h.check_accessibility();
        h.get_by_label("Hide chapters (⌥⌘U)");
    }
    bookmarks(app_mut(&mut h));
    app_mut(&mut h).ui.panel.tab = super::panel::Tab::Bookmarks;
    h.run_steps(3);
    h.check_accessibility();
    h.get_by_label("Add bookmark");
    h.get_by_label("More for Genly meets the king");
    // The toasts, with their buttons (the confirmation offers the tab when
    // it isn't in view).
    app_mut(&mut h).ui.panel.tab = super::panel::Tab::Chapters;
    app_mut(&mut h).bookmarked("Chapter 5", 14_200.0);
    reconciled(app_mut(&mut h));
    h.run_steps(3);
    h.check_accessibility();
    for label in ["Show", "Undo"] {
        h.get_by_label(label);
    }
    app_mut(&mut h).reconciliation = None;
    app_mut(&mut h).ui.toasts = shell::kit::Toasts::default();
    app_mut(&mut h).ui.reconcile_toast = None;
    app_mut(&mut h).ui.panel.tab = super::panel::Tab::Info;
    h.run_steps(3);
    h.get_by_label("George Guidall");
    app_mut(&mut h).playback_control(crate::player::Control::Stop);
    app_mut(&mut h).player.book = None;
    h.run_steps(3);
    h.check_accessibility();
    app_mut(&mut h).apply_prefs(PrefsAction::ShowPanel(false));
    playing(app_mut(&mut h), true);
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
            h.get_by_label(&shell::keys::shown("Add bookmark (⌘B)"));
        }
    }
    app_mut(&mut h).mode = crate::window::Mode::Full;
    h.set_size(egui::vec2(FULL[0], FULL[1]));
    // The sidebar, full and as its rail, in each of the account's states,
    // and the account's menu.
    for collapsed in [false, true] {
        app_mut(&mut h).apply_prefs(PrefsAction::CollapseSidebar(collapsed));
        for state in 0..4 {
            let app = app_mut(&mut h);
            (app.sync.running, app.session.connecting, app.sync.offline) = (state == 1, state == 2, state == 3);
            h.run_steps(3);
            h.check_accessibility();
            h.get_by_label("Keyboard shortcuts (⌘/)");
            h.get_by_label("Settings (⌘,)");
            h.get_by_label("Search (⌘K)");
            // The sidebar's, and on a book's page the breadcrumb's too.
            assert!(h.query_all_by_label("Library").count() >= 1);
        }
        h.get_by_label_contains("sam, ").click();
        h.run_steps(4);
        h.check_accessibility();
        for label in ["Keyboard shortcuts", "Sign out…"] {
            h.get_by_label(label);
        }
        egui::Popup::close_all(&h.ctx);
        h.run_steps(2);
    }
    h.get_by_label("Expand sidebar (⌥⌘S)");
    app_mut(&mut h).apply_prefs(PrefsAction::CollapseSidebar(false));
    h.run_steps(3);
    h.get_by_label("Collapse sidebar (⌥⌘S)");
    // The palette: the books in progress, then what a query finds (books
    // and names).
    in_progress(app_mut(&mut h));
    app_mut(&mut h).apply(Action::OpenPalette);
    h.run_steps(3);
    h.check_accessibility();
    h.get_by_label("Piranesi");
    type_in_palette(&mut h, "le guin");
    h.check_accessibility();
    // Her name is also on the book page behind the palette.
    assert!(h.query_all_by_label("Ursula K. Le Guin").count() >= 2, "the author is a row of her own");
    app_mut(&mut h).apply(Action::ClosePalette);
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
    // The row above the page names the way here, and the chevrons retrace it.
    assert_eq!(h.query_all_by_label("Library").count(), 2, "the sidebar's, and the crumb");
    assert!(h.query_all_by_label("Dune").count() >= 2, "the crumb, as well as the page's title");
    h.get_by_label_contains("Back (").click();
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).ui.nav.current(), &Route::Library);
    h.get_by_label_contains("Forward (").click();
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).ui.nav.current(), &Route::Book("book-1".into()));
    // The crumb before the book goes back to it: the one right of the sidebar.
    let crumb = h.query_all_by_label("Library").max_by(|a, b| a.rect().left().total_cmp(&b.rect().left())).unwrap();
    crumb.click();
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).ui.nav.current(), &Route::Library);
}

#[test]
fn command_brackets_go_back_and_forward() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    app_mut(&mut h).go(Route::Home);
    app_mut(&mut h).go(book_route(0));
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::OpenBracket);
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).ui.nav.current(), &Route::Home);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::CloseBracket);
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).ui.nav.current(), &book_route(0));
    // A plain [ is still the speed's, not a step back.
    h.key_press(egui::Key::OpenBracket);
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).ui.nav.current(), &book_route(0));
}

#[test]
fn a_card_on_home_opens_its_book_and_its_button_resumes_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    listened(app_mut(&mut h));
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).ui.nav.current(), &Route::Home);
    // The fixture has no server, so resuming says so; it doesn't open the
    // book the card is for.
    h.get_by_label("Resume Dune").click();
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).ui.nav.current(), &Route::Home);
    assert!(app_mut(&mut h).ui.notice.as_ref().is_some_and(|n| n.text.starts_with("Not connected")), "the resume asked to play");
    h.get_by_label("Dune by Frank Herbert, 15h 29m left at 1×").click();
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).ui.nav.current(), &Route::Book("book-1".into()));
    // The sidebar's Library, and Home again.
    h.get_by_role_and_label(egui::accesskit::Role::ListBoxOption, "Library").click();
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).ui.nav.current(), &Route::Library);
    // The sidebar's row, not the page's own heading.
    h.get_by_role_and_label(egui::accesskit::Role::ListBoxOption, "Home").click();
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).ui.nav.current(), &Route::Home);
}

#[test]
fn home_with_nothing_in_progress_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    app_mut(&mut h).go(Route::Home);
    h.run_steps(2);
    h.get_by_label("Nothing in progress. Start a book and it shows up here.");
    h.get_by_label("Recently added");
    // The last app lets go of its shelf before the next one opens it.
    *h.state_mut() = None;
    *h.state_mut() = Some(app(&h.ctx.clone(), dir.path(), 0));
    app_mut(&mut h).go(Route::Home);
    h.run_steps(2);
    h.get_by_label("No books yet");
}

/// The covers on screen, by title, in the order drawn.
fn tiles(h: &Harness<'static, Option<RedeApp>>) -> Vec<String> {
    h.query_all_by_label_contains(" by ").filter_map(|n| n.accesskit_node().label().map(|l| l.split(" by ").next().unwrap_or_default().to_owned())).collect()
}

#[test]
fn the_sort_menu_reorders_the_shelf_and_is_saved() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    h.run_steps(2);
    h.get_by_label("Sort: Title").click();
    h.run_steps(2);
    // The orders are in the menu, the one in use checked.
    for sort in crate::settings::LibrarySort::ALL {
        let row = h.get_by_role_and_label(egui::accesskit::Role::MenuItem, sort.label());
        let checked = row.accesskit_node().toggled() == Some(egui::accesskit::Toggled::True);
        assert_eq!(checked, sort == crate::settings::LibrarySort::Title, "{}", sort.label());
    }
    h.get_by_role_and_label(egui::accesskit::Role::MenuItem, "Author").click();
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).prefs.get(|s| s.sort), crate::settings::LibrarySort::Author);
    assert!(h.query_by_role_and_label(egui::accesskit::Role::MenuItem, "Title").is_none(), "the menu closes");
    h.get_by_label("Sort: Author");
    // The first cover on screen is the first book by author: Brontë's.
    assert_eq!(tiles(&h).first().map(String::as_str), Some("Jane Eyre"), "the shelf is in author order");
    // By listening: the book heard last first.
    listened(app_mut(&mut h));
    app_mut(&mut h).go(Route::Library);
    h.run_steps(2);
    h.get_by_label("Sort: Author").click();
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::MenuItem, "Recently listened").click();
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).prefs.get(|s| s.sort), crate::settings::LibrarySort::Listened);
    assert_eq!(tiles(&h).first().map(String::as_str), Some("Piranesi"), "the book heard two hours ago leads");
}

#[test]
fn the_filter_narrows_the_shelf_counts_what_it_shows_and_is_saved() {
    use crate::settings::LibraryFilter;
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    listened(app_mut(&mut h));
    app_mut(&mut h).go(Route::Library);
    h.run_steps(2);
    h.get_by_label("12 books");
    h.get_by_label("In progress").click();
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).prefs.get(|s| s.filter), LibraryFilter::InProgress);
    let shown = tiles(&h);
    assert_eq!(shown.len(), 4, "the four books partway: {shown:?}");
    assert!(shown.iter().any(|t| t == "Piranesi") && !shown.iter().any(|t| t == "A Wizard of Earthsea"), "{shown:?}");
    h.get_by_label("4 books");
    h.get_by_label("Finished").click();
    h.run_steps(2);
    assert_eq!(tiles(&h), ["A Wizard of Earthsea"]);
    h.get_by_label("1 book");
    h.get_by_label("Not started").click();
    h.run_steps(2);
    assert!(!tiles(&h).iter().any(|t| t == "Piranesi" || t == "A Wizard of Earthsea"));
    h.get_by_label("7 books");
    // With nothing begun, In progress says so rather than showing a blank
    // page, and not as an empty library would.
    app_mut(&mut h).positions.clear();
    h.get_by_label("In progress").click();
    h.run_steps(2);
    assert!(tiles(&h).is_empty());
    h.get_by_label("Nothing in progress");
    assert!(h.query_by_label("No books yet").is_none());
    h.get_by_label("All").click();
    h.run_steps(2);
    h.get_by_label("12 books");
    assert_eq!(app_mut(&mut h).prefs.get(|s| s.filter), LibraryFilter::All);
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

/// One book's page alone, so what it asks for stays in `actions` to be read
/// instead of being applied.
fn page(dir: &Path, n: usize) -> Harness<'static, Option<RedeApp>> {
    let mut h = Harness::builder().with_size(egui::vec2(900.0, 700.0)).with_pixels_per_point(2.0).with_os(egui::os::OperatingSystem::Mac).build_ui_state(
        move |ui, app: &mut Option<RedeApp>| {
            if let Some(app) = app {
                app.view().book_page(ui, &format!("book-{n}"));
            }
        },
        None,
    );
    let ctx = h.ctx.clone();
    let mut app = app(&ctx, dir, BOOKS.len());
    listened(&mut app);
    app.actions.clear();
    *h.state_mut() = Some(app);
    h.run_steps(3);
    h
}

#[test]
fn a_books_more_menu_offers_only_what_applies_and_asks_for_it() {
    let asked = |h: &mut Harness<'static, Option<RedeApp>>| -> Vec<&'static str> {
        std::mem::take(&mut app_mut(h).actions)
            .into_iter()
            .filter_map(|a| match a {
                Action::MarkFinished(_) => Some("finished"),
                Action::MarkUnfinished(_) => Some("unfinished"),
                Action::StartOver(_) => Some("over"),
                _ => None,
            })
            .collect()
    };
    // A shelf of its own for each page: one database opens once.
    let open = |n: usize| {
        let dir = tempfile::tempdir().unwrap();
        let mut h = page(dir.path(), n);
        h.get_by_label_contains("More for ").click();
        h.run_steps(2);
        (h, dir)
    };
    // Not begun: it can be finished, and there is nothing to start over.
    let (mut h, _dir) = open(0);
    h.check_accessibility();
    assert!(h.query_by_label("Start over").is_none());
    assert!(h.query_by_label("Mark as unfinished").is_none());
    h.get_by_label("Mark as finished").click();
    h.run_steps(2);
    assert_eq!(asked(&mut h), ["finished"]);
    // Partway: finished, or back to the start.
    let (mut h, _dir) = open(2);
    h.get_by_label("Mark as finished");
    h.get_by_label("Start over").click();
    h.run_steps(2);
    assert_eq!(asked(&mut h), ["over"]);
    // Finished: un-finished, or back to the start.
    let (mut h, _dir) = open(4);
    assert!(h.query_by_label("Mark as finished").is_none());
    h.get_by_label("Start over");
    h.get_by_label("Mark as unfinished").click();
    h.run_steps(2);
    assert_eq!(asked(&mut h), ["unfinished"]);
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
fn a_books_series_links_to_its_page_with_the_series_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    // The Dispossessed is the sixth Hainish book; the fixture has the first too.
    app_mut(&mut h).go(book_route(3));
    h.run_steps(3);
    h.get_by_label("Hainish #6").click();
    h.run_steps(3);
    assert_eq!(app_mut(&mut h).ui.nav.current(), &Route::Series("Hainish".into()));
    h.get_by_label_contains("By Ursula K. Le Guin");
    let first = h.get_by_label("The Left Hand of Darkness by Ursula K. Le Guin, book 1").rect().min;
    let sixth = h.get_by_label("The Dispossessed by Ursula K. Le Guin, book 6").rect().min;
    assert!(first.x < sixth.x, "in the series' order, not the library's");
    assert!(h.query_by_label_contains("Piranesi").is_none(), "only the series' books");
    // With the first finished, the button goes on with the next.
    let item = sources::ItemKey::new(account().id, "book-0");
    let done = sources::Progress { item, position: sources::Position::from_secs(40_920.0), finished: true, at: Some(std::time::SystemTime::now()) };
    app_mut(&mut h).positions.insert("book-0".into(), done);
    h.run_steps(3);
    h.get_by_label_contains("1 of 2 finished");
    h.get_by_label("Play · The Dispossessed").click();
    h.run_steps(3);
    let notice = app_mut(&mut h).ui.notice.clone().map(|n| n.text).unwrap_or_default();
    assert!(notice.contains("Sign in to play"), "it plays that book: {notice}");
}

#[test]
fn the_series_place_lists_every_series_and_opens_each_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    h.run_steps(2);
    h.get_by_label("Series").click();
    h.run_steps(3);
    assert_eq!(app_mut(&mut h).ui.nav.current(), &Route::AllSeries);
    h.get_by_label("5 series");
    // By name, so Dune comes before Hainish whatever the library's order.
    let dune = h.get_by_label("Dune, 1 book by Frank Herbert, not started").rect().min;
    let hainish = h.get_by_label("Hainish, 2 books by Ursula K. Le Guin, not started").rect().min;
    assert!((dune.y, dune.x) < (hainish.y, hainish.x));
    h.get_by_label("Hainish, 2 books by Ursula K. Le Guin, not started").click();
    h.run_steps(3);
    assert_eq!(app_mut(&mut h).ui.nav.current(), &Route::Series("Hainish".into()));
    // The page belongs to the place it was opened from.
    assert_eq!(app_mut(&mut h).ui.nav.place(), &Route::AllSeries);
    h.get_by_label("The Left Hand of Darkness by Ursula K. Le Guin, book 1");
}

#[test]
fn choosing_a_series_in_the_palette_opens_its_page() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    h.run_steps(2);
    app_mut(&mut h).apply(Action::OpenPalette);
    h.run_steps(3);
    type_in_palette(&mut h, "hainish");
    h.get_by_label("Hainish").click();
    h.run_steps(3);
    assert!(app_mut(&mut h).ui.palette.is_none());
    assert_eq!(app_mut(&mut h).ui.nav.current(), &Route::Series("Hainish".into()));
}

#[test]
fn an_html_description_reads_as_prose_with_its_markup_folded_away() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    // The fixture's second book gives its description as Audiobookshelf does.
    app_mut(&mut h).go(book_route(1));
    h.run_steps(3);
    h.get_by_label_contains("is a landmark");
    h.get_by_label_contains("What the critics said");
    h.get_by_label_contains("A wonderful, lifelike tapestry.");
    for markup in ["<b>", "</p>", "&amp;", "&mdash;", "<li>", "<a href", "<blockquote>"] {
        assert_eq!(h.query_all_by_label_contains(markup).count(), 0, "{markup} shows through");
    }
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
    h.get_by_label("sam, books.example.com").click();
    // The menu opens up from the foot, placed once its size is known.
    h.run_steps(4);
    // It asks first; Esc keeps you signed in, and doesn't open Settings.
    h.get_by_label("Sign out…").click();
    h.run_steps(3);
    assert_eq!(app_mut(&mut h).sheet, Some(super::sheets::Sheet::SignOut { from_settings: false }));
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).sheet, None);
    assert!(app_mut(&mut h).login.is_none());
    h.get_by_label("sam, books.example.com").click();
    h.run_steps(4);
    h.get_by_label("Sign out…").click();
    h.run_steps(3);
    h.get_by_label("Sign out of books.example.com?");
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
    for label in ["Speed", "Slower", "Faster", "Settings…"] {
        h.get_by_label(label);
    }
    assert_eq!(h.query_all_by_label("Stop playback").count(), 0, "stopping is a command, not a playback setting");
    // The speed playing shows large over its lit preset.
    for speed in ["0.8×", "1×", "1.25×", "1.75×", "2×"] {
        h.get_by_label(speed);
    }
    assert_eq!(h.query_all_by_label("1.5×").count(), 2);
    h.check_accessibility();
    // The skips and rewinds are settings, a click away on Settings' Playback
    // tab, whichever tab it was last on.
    app_mut(&mut h).ui.settings = super::settings_page::Tab::About;
    h.get_by_label("Settings…").click();
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).sheet, Some(super::sheets::Sheet::Settings));
    assert_eq!(app_mut(&mut h).ui.settings, super::settings_page::Tab::Playback);
}

/// The controls the title bar holds while a book plays.
const BAR_CONTROLS: [&str; 6] = ["Back 15 seconds", "Play (Space)", "Forward 30 seconds", "Chapter position", "Playback speed, 1.5×", "Sleep timer"];

#[test]
fn the_title_bar_holds_the_player_at_every_width_without_overlaps() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    playing(app_mut(&mut h), true);
    for width in [crate::MIN_SIZE[0], FULL[0], 1440.0, 1920.0, 2560.0] {
        h.set_size(egui::vec2(width, FULL[1]));
        h.run_steps(3);
        let toggle = h.get_by_label_contains("chapters (⌥⌘U)").rect();
        let mut rects = vec![("toggle", toggle)];
        for label in BAR_CONTROLS {
            rects.push((label, h.get_by_label(label).rect()));
        }
        // The cover, the way to the Player, and the chapter buttons stay at
        // every width.
        rects.push(("cover", h.get_by_label("Open the Player").rect()));
        rects.push(("previous", h.get_by_label("Previous chapter (⇧←)").rect()));
        rects.push(("next", h.get_by_label("Next chapter (⇧→)").rect()));
        for (label, rect) in &rects {
            assert!(rect.min.y >= 0.0 && rect.max.y <= super::BAR_H, "{label} at {width}: {rect:?}");
            assert!(rect.min.x >= 0.0 && rect.max.x <= width, "{label} at {width}: {rect:?}");
        }
        for (i, (a, ra)) in rects.iter().enumerate() {
            for (b, rb) in &rects[i + 1..] {
                assert!(!ra.shrink(0.5).intersects(rb.shrink(0.5)), "{a} and {b} overlap at {width}: {ra:?} {rb:?}");
            }
        }
        assert!(h.get_by_label("Chapter position").rect().width() >= 100.0, "the scrubber stays usable at {width}");
    }
}

#[test]
fn the_title_bar_buttons_enqueue_their_controls() {
    use crate::player::Control;
    // Only the bar, with nothing applying what it asks for, so the test
    // sees the actions themselves.
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(FULL[0], FULL[1])).build_ui_state(
        |ui, app: &mut Option<RedeApp>| {
            if let Some(app) = app {
                egui::Panel::top("bar").exact_size(super::BAR_H).show(ui, |ui| app.title_bar(ui));
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
        ("Play (Space)", Control::Toggle),
        ("Forward 30 seconds", Control::Skip(30.0)),
        ("Next chapter (⇧→)", Control::NextChapter),
    ] {
        h.get_by_label(label).click();
        h.run_steps(2);
        let actions = std::mem::take(&mut app_mut(&mut h).actions);
        assert!(matches!(actions.as_slice(), [Action::Playback(control)] if *control == expected), "{label}: {} actions", actions.len());
    }
    // The chips open their menus rather than asking for anything.
    for label in ["Playback speed, 1.5×", "Sleep timer"] {
        h.get_by_label(label).click();
        h.run_steps(2);
        assert!(app_mut(&mut h).actions.is_empty(), "{label} only opens its menu");
        egui::Popup::close_all(&h.ctx);
        h.run_steps(2);
    }
    // The cover opens the Player.
    h.get_by_label("Open the Player").click();
    h.run_steps(2);
    let actions = std::mem::take(&mut app_mut(&mut h).actions);
    assert!(matches!(actions.as_slice(), [Action::Mode(crate::window::Mode::Player)]), "the cover opens the Player: {} actions", actions.len());
    // The rest of the capsule (here its title, over the chapter's link)
    // opens the book's page.
    let capsule = h.get_by_label_contains("Now playing: The Left Hand of Darkness").rect();
    let title = capsule.center_top() + egui::vec2(0.0, 10.0);
    h.event(egui::Event::PointerMoved(title));
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: title, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
    let actions = std::mem::take(&mut app_mut(&mut h).actions);
    assert!(matches!(actions.as_slice(), [Action::Go(Route::Book(id))] if id == "book-0"), "the capsule opens the playing book's page");
}

#[test]
fn stop_playback_is_in_the_capsules_menu_and_the_players_more_menu() {
    use crate::player::Control;
    let dir = tempfile::tempdir().unwrap();
    let mut h = bar_only(dir.path());
    // A right-click anywhere on the capsule: its body, or the chapter on it.
    for part in ["Now playing: The Left Hand of Darkness", "Chapter 5, show chapters"] {
        h.get_by_label_contains(part).click_secondary();
        h.run_steps(2);
        h.check_accessibility();
        h.get_by_label("Stop playback").click();
        h.run_steps(2);
        let actions = std::mem::take(&mut app_mut(&mut h).actions);
        assert!(matches!(actions.as_slice(), [Action::Playback(Control::Stop)]), "{part}: {} actions", actions.len());
    }
    // In the Player and Mini, the "…" in the corner.
    for mode in [crate::window::Mode::Player, crate::window::Mode::Mini] {
        app_mut(&mut h).mode = mode;
        h.run_steps(2);
        h.get_by_label("More").click();
        h.run_steps(2);
        h.get_by_label("Stop playback").click();
        h.run_steps(2);
        let actions = std::mem::take(&mut app_mut(&mut h).actions);
        assert!(matches!(actions.as_slice(), [Action::Playback(Control::Stop)]), "{mode:?}: {} actions", actions.len());
    }
}

#[test]
fn nothing_playing_leaves_a_quiet_capsule_and_an_unavailable_transport() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    h.run_steps(3);
    h.get_by_label("Not playing");
    assert_eq!(h.query_all_by_label("Playback speed, 1.5×").count(), 0, "no pills without a book");
    h.get_by_label("Play (Space)").click();
    h.run_steps(2);
    assert!(app_mut(&mut h).player.book.is_none());
    assert!(app_mut(&mut h).ui.notice.is_none(), "an unavailable button does nothing, not even complain");
}

#[test]
fn the_chapter_in_the_title_bar_opens_the_column_at_the_chapters() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    playing(app_mut(&mut h), true);
    app_mut(&mut h).ui.panel.tab = super::panel::Tab::Info;
    h.run_steps(3);
    h.get_by_label("Chapter 5, show chapters").click();
    h.run_steps(3);
    let app = app_mut(&mut h);
    assert!(app.prefs.get(|s| s.show_panel), "the column is shown, and that's remembered");
    assert_eq!(app.ui.panel.tab, super::panel::Tab::Chapters);
    // The chapter playing is in view, marked as playing.
    let row = h.get_by_label("Chapter 5, 56:40, playing").rect();
    assert!(row.min.y >= super::BAR_H && row.max.y <= FULL[1], "{row:?}");
}

#[test]
fn a_chapter_in_the_column_seeks_to_its_start() {
    use crate::player::Control;
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(super::panel::PANEL_W, FULL[1])).build_ui_state(
        |ui, app: &mut Option<RedeApp>| {
            if let Some(app) = app {
                app.view().panel(ui);
            }
        },
        None,
    );
    let ctx = h.ctx.clone();
    let mut app = app(&ctx, dir.path(), BOOKS.len());
    playing(&mut app, true);
    *h.state_mut() = Some(app);
    h.run_steps(2);
    // Over the chapters, when the book ends: past midnight, so with its day.
    h.get_by_label_contains("ends Wed ");
    assert_eq!(h.query_all_by_label_contains("left at").count(), 0, "the time left is the capsule's to say");
    h.get_by_label("Chapter 2, 56:40").click();
    h.run_steps(2);
    let actions = std::mem::take(&mut app_mut(&mut h).actions);
    assert!(matches!(actions.as_slice(), [Action::Playback(Control::Seek(at))] if *at == 3_400.0), "{} actions", actions.len());
    h.get_by_label("Info").click();
    h.run_steps(2);
    let actions = std::mem::take(&mut app_mut(&mut h).actions);
    assert!(matches!(actions.as_slice(), [Action::Panel(super::panel::PanelAction::Tab(super::panel::Tab::Info))]));
}

#[test]
fn the_column_is_a_sheet_in_a_narrow_window_and_the_preference_stays_put() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    playing(app_mut(&mut h), true);
    h.set_size(egui::vec2(crate::MIN_SIZE[0], 560.0));
    h.run_steps(3);
    h.get_by_label("Show chapters (⌥⌘U)").click();
    h.run_steps(3);
    let app = app_mut(&mut h);
    assert!(app.ui.panel.sheet, "the sheet opens");
    assert!(!app.prefs.get(|s| s.show_panel), "the docked column's preference isn't touched");
    h.get_by_label("Chapter 5, 56:40, playing");
    h.check_accessibility();
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(!app_mut(&mut h).ui.panel.sheet, "Esc closes it");
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
    // The full window's bar has no mode buttons: the Player is a key away
    // (and a click on the cover, with a book), and its own bar has the rest.
    assert_eq!(h.query_all_by_label_contains("window (⌥⌘").count(), 0);
    assert_eq!(h.query_all_by_label("Mini player (⇧⌘M)").count(), 0);
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::N);
    h.run_steps(3);
    assert_eq!(app_mut(&mut h).mode, crate::window::Mode::Player);
    // The Player's "…" has the shortcuts, and nothing to stop.
    h.get_by_label("More").click();
    h.run_steps(3);
    assert_eq!(h.query_all_by_label("Stop playback").count(), 0, "nothing to stop");
    h.get_by_label_contains("Keyboard shortcuts").click();
    h.run_steps(3);
    assert_eq!(app_mut(&mut h).sheet, Some(super::sheets::Sheet::Shortcuts));
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    h.get_by_label("Mini player (⇧⌘M)").click();
    h.run_steps(3);
    assert_eq!(app_mut(&mut h).mode, crate::window::Mode::Mini);
    h.get_by_label("Full window (⌥⌘1)").click();
    h.run_steps(3);
    assert_eq!(app_mut(&mut h).mode, crate::window::Mode::Full);
    h.get_by_label("Keyboard shortcuts (⌘/)").click();
    h.run_steps(3);
    h.get_by_label("Close (esc)").click();
    h.run_steps(3);
    assert_eq!(app_mut(&mut h).sheet, None);
}

#[test]
fn sleep_options_and_custom_defaults_are_accessible_from_the_title_bar() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    playing(app_mut(&mut h), true);
    h.run_steps(3);
    h.get_by_label("Sleep timer").click();
    h.run_steps(3);
    h.check_accessibility();
    for label in ["5 min", "15 min", "30 min", "45 min", "60 min", "90 min", "End of chapter", "custom timer, 30 min", "Less custom timer"] {
        h.get_by_label(label);
    }
    assert_eq!(h.query_all_by_label("Start").count(), 0, "the custom length is a cell of the grid, not a row with its own Start");
    // The custom length steps, and is saved for next time.
    h.get_by_label("More custom timer").click();
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).prefs.get(|s| s.playback.sleep_minutes), 45);
    h.get_by_label("custom timer, 45 min");
}

/// The title bar alone, with nothing applying what it asks for.
fn bar_only(dir: &std::path::Path) -> Harness<'static, Option<RedeApp>> {
    let mut h = Harness::builder().with_size(egui::vec2(FULL[0], FULL[1])).build_ui_state(
        |ui, app: &mut Option<RedeApp>| {
            if let Some(app) = app {
                egui::Panel::top("bar").exact_size(super::BAR_H).show(ui, |ui| app.title_bar(ui));
            }
        },
        None,
    );
    let ctx = h.ctx.clone();
    let mut app = app(&ctx, dir, BOOKS.len());
    playing(&mut app, true);
    *h.state_mut() = Some(app);
    h.run_steps(2);
    h
}

#[test]
fn the_custom_sleep_cell_sets_its_length_without_starting_and_starts_when_chosen() {
    use crate::player::Control;
    let dir = tempfile::tempdir().unwrap();
    let mut h = bar_only(dir.path());
    h.get_by_label("Sleep timer").click();
    h.run_steps(2);
    h.get_by_label("More custom timer").click();
    h.run_steps(2);
    let actions = std::mem::take(&mut app_mut(&mut h).actions);
    assert!(
        matches!(actions.as_slice(), [Action::Prefs(PrefsAction::Playback(p))] if p.sleep_minutes == 45),
        "a step saves the length and starts nothing: {} actions",
        actions.len()
    );
    for a in actions {
        app_mut(&mut h).apply(a);
    }
    h.run_steps(2);
    h.get_by_label("custom timer, 45 min").click();
    h.run_steps(2);
    let actions = std::mem::take(&mut app_mut(&mut h).actions);
    assert!(matches!(actions.as_slice(), [Action::Playback(Control::Sleep(Some(crate::sleep::Choice::Minutes(45))))]), "{} actions", actions.len());
}

#[test]
fn a_running_timer_shows_what_is_left_and_how_to_change_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    playing(app_mut(&mut h), false);
    let now = std::time::Instant::now();
    app_mut(&mut h).player.sleep = Some(crate::sleep::Timer::new(crate::sleep::Choice::Minutes(30), 0.0, now));
    h.run_steps(3);
    h.get_by_label_contains("Sleep timer, ").click();
    h.run_steps(3);
    h.check_accessibility();
    for label in ["Add 10 min", "Cancel timer", "Change to"] {
        h.get_by_label(label);
    }
    h.get_by_label_contains("Pauses in ");
    assert_eq!(h.query_all_by_label("One more chapter").count(), 0);
}

#[test]
fn a_timer_at_the_chapters_end_offers_one_more_chapter_instead_of_ten_minutes() {
    use crate::player::Control;
    let dir = tempfile::tempdir().unwrap();
    let mut h = bar_only(dir.path());
    let app = app_mut(&mut h);
    let end = app.player.book.as_ref().unwrap().timeline.chapter_range(app.player.position).1;
    app.player.sleep = Some(crate::sleep::Timer::new(crate::sleep::Choice::Chapter, end, std::time::Instant::now()));
    h.run_steps(2);
    h.get_by_label_contains("Sleep timer, ").click();
    h.run_steps(2);
    h.get_by_label_contains("Pauses when this chapter ends, in ");
    assert_eq!(h.query_all_by_label("Add 10 min").count(), 0, "ten minutes means nothing to a timer waiting for a chapter");
    h.get_by_label("One more chapter").click();
    h.run_steps(2);
    let actions = std::mem::take(&mut app_mut(&mut h).actions);
    assert!(matches!(actions.as_slice(), [Action::Playback(Control::OneMoreChapter)]), "{} actions", actions.len());
    // In the book's last chapter there's no next one to wait for.
    let app = app_mut(&mut h);
    let last = app.player.book.as_ref().unwrap().timeline.duration;
    app.player.position = last - 30.0;
    app.player.sleep = Some(crate::sleep::Timer::new(crate::sleep::Choice::Chapter, last, std::time::Instant::now()));
    h.run_steps(2);
    assert_eq!(h.query_all_by_label("One more chapter").count(), 0);
    h.get_by_label("Cancel timer");
}

#[test]
fn the_settings_window_steps_each_playback_setting_and_closes_over_the_page() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    app_mut(&mut h).go(book_route(0));
    h.run_steps(2);
    h.get_by_label("sam, books.example.com").click();
    h.run_steps(2);
    h.get_by_label("Settings…").click();
    h.run_steps(3);
    assert_eq!(app_mut(&mut h).sheet, Some(super::sheets::Sheet::Settings));
    assert_eq!(app_mut(&mut h).ui.settings, super::settings_page::Tab::Playback);
    // A window over the book, not a page in its place.
    assert_eq!(app_mut(&mut h).ui.nav.current(), &book_route(0));
    h.check_accessibility();
    for (label, more) in [
        ("skip back, 15 s", "More skip back"),
        ("skip forward, 30 s", "More skip forward"),
        ("rewind after a pause, 5 s", "More rewind after a pause"),
        ("rewind after sleep, 10 s", "More rewind after sleep"),
        ("timer length, 30 min", "More timer length"),
    ] {
        h.get_by_label(label);
        h.get_by_label(more).click();
        h.run_steps(2);
    }
    let p = app_mut(&mut h).prefs.get(|s| s.playback);
    assert_eq!((p.skip_back, p.skip_forward, p.rewind, p.sleep_rewind, p.sleep_minutes), (20, 45, 10, 15, 45));
    h.get_by_label("Less rewind after a pause").click();
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).prefs.get(|s| s.playback.rewind), 5);
    // The speech switches, each on its own.
    h.get_by_role_and_label(egui::accesskit::Role::Switch, "Voice boost").click();
    h.run_steps(2);
    assert!(app_mut(&mut h).prefs.get(|s| s.playback.voice_boost && !s.playback.skip_silence));
    h.get_by_role_and_label(egui::accesskit::Role::Switch, "Skip silence").click();
    h.run_steps(2);
    assert!(app_mut(&mut h).prefs.get(|s| s.playback.voice_boost && s.playback.skip_silence));
    // Esc closes it, leaving the book where it was; ⌘, brings it back, and a
    // click outside closes it too.
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).sheet, None);
    assert_eq!(app_mut(&mut h).ui.nav.current(), &book_route(0));
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Comma);
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).sheet, Some(super::sheets::Sheet::Settings));
    h.hover_at(egui::pos2(8.0, FULL[1] - 8.0));
    h.event(egui::Event::PointerButton {
        pos: egui::pos2(8.0, FULL[1] - 8.0),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    h.event(egui::Event::PointerButton {
        pos: egui::pos2(8.0, FULL[1] - 8.0),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).sheet, None, "a click outside closes it");
}

#[test]
fn the_sidebar_gear_opens_settings_in_the_column_and_the_rail() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    for collapsed in [false, true] {
        app_mut(&mut h).apply_prefs(PrefsAction::CollapseSidebar(collapsed));
        app_mut(&mut h).ui.settings = super::settings_page::Tab::Account;
        h.run_steps(3);
        h.get_by_label("Settings (⌘,)").click();
        h.run_steps(3);
        assert_eq!(app_mut(&mut h).sheet, Some(super::sheets::Sheet::Settings));
        assert_eq!(app_mut(&mut h).ui.settings, super::settings_page::Tab::Account, "on the tab it was last on");
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        assert_eq!(app_mut(&mut h).sheet, None);
    }
}

#[test]
fn settings_lists_its_tabs_and_each_names_its_controls() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    app_mut(&mut h).apply(Action::OpenSettings(super::settings_page::Tab::Playback));
    h.run_steps(3);
    h.get_by_label("Account").click();
    h.run_steps(3);
    assert_eq!(app_mut(&mut h).ui.settings, super::settings_page::Tab::Account);
    h.check_accessibility();
    for label in ["https://books.example.com", "sam", "Sync now", "Sign out…"] {
        h.get_by_label(label);
    }
    h.get_by_label_contains("Signed in with a password.");
    // Sign out… asks first; Cancel goes back to Settings.
    h.get_by_label("Sign out…").click();
    h.run_steps(3);
    assert_eq!(app_mut(&mut h).sheet, Some(super::sheets::Sheet::SignOut { from_settings: true }));
    h.get_by_label("Sign out of books.example.com?");
    h.check_accessibility();
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(app_mut(&mut h).sheet, Some(super::sheets::Sheet::Settings), "Settings is back");
    assert!(app_mut(&mut h).login.is_none());
    h.get_by_label("Sign out…").click();
    h.run_steps(3);
    h.get_by_label("Sign out").click();
    h.run_steps(2);
    assert!(app_mut(&mut h).login.is_some(), "signed out, to the sign-in screen");
    assert_eq!(app_mut(&mut h).sheet, None);
    let mut h = window(dir.path(), BOOKS.len());
    app_mut(&mut h).apply(Action::OpenSettings(super::settings_page::Tab::About));
    h.run_steps(3);
    h.check_accessibility();
    for label in ["Rede", "Source code", "Report a problem"] {
        h.get_by_label(label);
    }
    h.get_by_label(&format!("Version {}", env!("CARGO_PKG_VERSION")));
}

/// ⌘+, ⌘− and ⌘0 step the zoom once a press, one toast says where it is,
/// and Settings › Appearance shows the same level and sets it.
#[test]
fn the_zoom_keys_step_the_zoom_and_settings_follows() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    h.run_steps(2);
    let cmd = egui::Modifiers::COMMAND;
    h.key_press_modifiers(cmd | egui::Modifiers::SHIFT, egui::Key::Plus);
    h.run_steps(2);
    assert_eq!(h.ctx.zoom_factor(), 1.1);
    h.key_press_modifiers(cmd, egui::Key::Equals);
    h.run_steps(2);
    assert_eq!(h.ctx.zoom_factor(), 1.25, "one step a press");
    let app = app_mut(&mut h);
    assert_eq!(app.prefs.get(|s| s.zoom), 125);
    let zoom_toasts: Vec<_> = app.ui.toasts.texts().into_iter().filter(|t| t.starts_with("Zoom")).map(str::to_owned).collect();
    assert_eq!(zoom_toasts, ["Zoom 125%"], "the last toast replaces the one before");
    h.key_press_modifiers(cmd, egui::Key::Minus);
    h.run_steps(2);
    assert_eq!(h.ctx.zoom_factor(), 1.1);
    app_mut(&mut h).apply(Action::OpenSettings(super::settings_page::Tab::Appearance));
    h.run_steps(3);
    h.check_accessibility();
    h.get_by_label("zoom, 110%");
    h.get_by_label("More zoom").click();
    h.run_steps(2);
    assert_eq!((app_mut(&mut h).prefs.get(|s| s.zoom), h.ctx.zoom_factor()), (125, 1.25));
    h.get_by_label("zoom, 125%");
    h.key_press_modifiers(cmd, egui::Key::Num0);
    h.run_steps(2);
    assert_eq!((app_mut(&mut h).prefs.get(|s| s.zoom), h.ctx.zoom_factor()), (100, 1.0));
}

#[test]
fn opening_settings_from_the_player_returns_to_the_full_window() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    playing(app_mut(&mut h), true);
    for mode in [crate::window::Mode::Player, crate::window::Mode::Mini] {
        app_mut(&mut h).set_mode(mode);
        assert_eq!(app_mut(&mut h).mode, mode);
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Comma);
        h.run_steps(2);
        assert_eq!(app_mut(&mut h).mode, crate::window::Mode::Full);
        assert_eq!(app_mut(&mut h).sheet, Some(super::sheets::Sheet::Settings));
        app_mut(&mut h).sheet = None;
    }
}

#[test]
fn the_volume_keys_step_five_points_and_leave_a_typed_field_alone() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    playing(app_mut(&mut h), false);
    h.run_steps(2);
    let cmd = egui::Modifiers::COMMAND;
    h.key_press_modifiers(cmd, egui::Key::ArrowDown);
    h.run_steps(2);
    let level = app_mut(&mut h).prefs.get(|s| s.playback.volume);
    assert!((level - 0.95).abs() < 0.001, "five points a press: {level}");
    h.key_press_modifiers(cmd, egui::Key::ArrowUp);
    h.run_steps(2);
    assert!((app_mut(&mut h).prefs.get(|s| s.playback.volume) - 1.0).abs() < 0.001, "and no higher than full");
    // Muted, a press is the way back to hearing it.
    app_mut(&mut h).apply_prefs(PrefsAction::Muted(true));
    h.run_steps(2);
    h.key_press_modifiers(cmd, egui::Key::ArrowDown);
    h.run_steps(2);
    let playback = app_mut(&mut h).prefs.get(|s| s.playback);
    assert!(!playback.muted, "moving the level unmutes");
    assert!((playback.volume - 0.95).abs() < 0.001);
    // A field with the keys keeps its arrows.
    *h.state_mut() = Some(signin_with(&h.ctx.clone(), dir.path(), &["local"], None));
    h.run_steps(3);
    let before = app_mut(&mut h).prefs.get(|s| s.playback.volume);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Username").click();
    h.run_steps(2);
    h.key_press_modifiers(cmd, egui::Key::ArrowUp);
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).prefs.get(|s| s.playback.volume), before, "typing is not listening");
}

#[test]
fn the_shortcuts_sheet_lists_each_group_with_its_keys() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    app_mut(&mut h).sheet = Some(super::sheets::Sheet::Shortcuts);
    h.run_steps(3);
    h.check_accessibility();
    for label in ["Playback", "Chapters", "Window", "Navigation", "Play / pause", "Show / hide chapters", "Add bookmark", "Full, Player, Mini"] {
        h.get_by_label(label);
    }
    h.get_by_label("Space");
    h.get_by_label(&shell::keys::shown("⌥⌘3"));
}

#[test]
fn the_sleep_notice_names_its_extension_in_every_mode() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    playing(app_mut(&mut h), false);
    // The fixture has no engine, so this checks the button is there and named;
    // what Extend does is the player's, tested there.
    for mode in [crate::window::Mode::Full, crate::window::Mode::Player, crate::window::Mode::Mini] {
        let app = app_mut(&mut h);
        let now = std::time::Instant::now();
        let started = now.checked_sub(std::time::Duration::from_secs(5 * 60 - 40)).unwrap_or(now);
        app.player.sleep = Some(crate::sleep::Timer::new(crate::sleep::Choice::Minutes(5), 0.0, started));
        app.mode = mode;
        let size = mode.size();
        h.set_size(egui::vec2(size[0], size[1]));
        h.run_steps(3);
        h.check_accessibility();
        h.get_by_label("Extend 10 minutes");
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
        events: vec![egui::Event::Key { key: egui::Key::U, physical_key: None, pressed: true, repeat: false, modifiers: alt }],
        ..Default::default()
    };
    let mut out = ctx.run_ui(input, |ui| app.window_keys(ui.ctx()));
    out.textures_delta.clear();
    assert!(matches!(app.actions.pop(), Some(Action::Panel(super::panel::PanelAction::Toggle))), "⌥⌘U toggles the column, as it does Ond's queue");
    let input = egui::RawInput {
        events: vec![egui::Event::Key { key: egui::Key::Slash, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND }],
        ..Default::default()
    };
    let mut out = ctx.run_ui(input, |ui| app.window_keys(ui.ctx()));
    out.textures_delta.clear();
    assert!(matches!(app.actions.pop(), Some(Action::Shortcuts)));
}

#[test]
fn discovered_methods_name_only_the_controls_the_server_offers() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), 0);
    for both in [false, true] {
        *h.state_mut() = Some(signin(&h.ctx.clone(), dir.path(), both, false));
        h.run_steps(3);
        h.check_accessibility();
        h.get_by_label("Sign in with Book Club");
        assert_eq!(h.query_all_by_label("Username").next().is_some(), both);
        assert_eq!(h.query_all_by_label("Password").next().is_some(), both);
    }
    *h.state_mut() = Some(signin(&h.ctx.clone(), dir.path(), false, true));
    h.run_steps(3);
    h.check_accessibility();
    h.get_by_label("Finish signing in in your browser");
    // Opening the page again opens the same attempt: the same page, the same
    // pending sign-in, no new request to the server.
    let session = app_mut(&mut h).bridge.session();
    let url = app_mut(&mut h).openid.pending.as_ref().unwrap().flow.browser_url().to_owned();
    h.get_by_label("Open the browser again").click();
    h.step();
    let opened: Vec<_> = h
        .output()
        .platform_output
        .commands
        .iter()
        .filter_map(|c| match c {
            egui::OutputCommand::OpenUrl(open) => Some(open.url.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(opened, std::slice::from_ref(&url));
    assert_eq!(app_mut(&mut h).bridge.session(), session);
    assert_eq!(app_mut(&mut h).openid.pending.as_ref().unwrap().flow.browser_url(), url);
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(app_mut(&mut h).openid.pending.is_none());
    assert!(!app_mut(&mut h).login.as_ref().unwrap().busy);
}

#[test]
fn the_sidebar_folds_to_its_rail_and_the_window_remembers() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    h.run_steps(3);
    h.get_by_label("Collapse sidebar (⌥⌘S)").click();
    h.run_steps(3);
    assert!(app_mut(&mut h).prefs.get(|s| s.sidebar_collapsed), "saved with the preferences");
    // The rail keeps every place and the account.
    h.get_by_label("Library");
    h.get_by_label_contains("sam, ");
    h.get_by_label("Expand sidebar (⌥⌘S)").click();
    h.run_steps(3);
    assert!(!app_mut(&mut h).prefs.get(|s| s.sidebar_collapsed));
    // ⌥⌘S does the same.
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::ALT, egui::Key::S);
    h.run_steps(3);
    assert!(app_mut(&mut h).prefs.get(|s| s.sidebar_collapsed));
    // A window too narrow for the full column shows the rail whatever the
    // preference, and doesn't offer to expand it.
    app_mut(&mut h).apply_prefs(PrefsAction::CollapseSidebar(false));
    h.set_size(egui::vec2(crate::MIN_SIZE[0], 560.0));
    h.run_steps(3);
    assert_eq!(h.query_all_by_label_contains("sidebar (⌥⌘S)").count(), 0);
    h.get_by_label("Library");
    h.get_by_label("Keyboard shortcuts (⌘/)");
}

#[test]
fn the_account_says_when_the_server_is_out_of_reach_and_when_it_is_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    h.run_steps(3);
    h.get_by_label("sam, books.example.com");
    app_mut(&mut h).sync_done(crate::sync::Outcome::Failed(crate::failure::Failure::Network("down".into())));
    h.run_steps(3);
    h.get_by_label("sam, Offline · retrying");
    app_mut(&mut h).sync_done(crate::sync::Outcome::Done(crate::sync::Report::default()));
    h.run_steps(3);
    h.get_by_label("sam, books.example.com");
}

#[test]
fn the_shortcuts_are_in_the_account_menu_too() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    h.run_steps(2);
    h.get_by_label("sam, books.example.com").click();
    h.run_steps(4);
    h.get_by_label("Keyboard shortcuts").click();
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).sheet, Some(super::sheets::Sheet::Shortcuts), "the sheet opens");
}

/// The account at the foot of the column, and the rail's, sit inside the
/// window with room under them: a row that grew past its layout once hung
/// its second line under the window's bottom edge.
#[test]
fn the_sidebar_foot_sits_inside_the_window() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    for collapsed in [false, true] {
        app_mut(&mut h).apply_prefs(PrefsAction::CollapseSidebar(collapsed));
        h.run_steps(3);
        let window = h.ctx.content_rect();
        for label in ["sam, books.example.com", "Keyboard shortcuts (⌘/)"] {
            let rect = h.get_by_label(label).rect();
            let room = window.bottom() - rect.bottom();
            assert!(room >= f32::from(super::sidebar::FOOT_MARGIN) - 0.5, "{label} (collapsed: {collapsed}) has {room} under it: {rect:?}");
        }
    }
}

/// The column alone, so what its controls ask for stays in `actions` to be
/// read rather than applied: the playing book, paused, with its bookmarks.
fn column(dir: &Path) -> Harness<'static, Option<RedeApp>> {
    let mut h = Harness::builder()
        .with_size(egui::vec2(super::panel::PANEL_W, 520.0))
        .with_pixels_per_point(2.0)
        .with_os(egui::os::OperatingSystem::Mac)
        .build_ui_state(
            |ui, app: &mut Option<RedeApp>| {
                if let Some(app) = app {
                    app.view().panel(ui);
                }
            },
            None,
        );
    let ctx = h.ctx.clone();
    let mut app = app(&ctx, dir, BOOKS.len());
    playing(&mut app, true);
    bookmarks(&mut app);
    app.ui.panel.tab = super::panel::Tab::Bookmarks;
    *h.state_mut() = Some(app);
    h
}

/// What the column asked for since the last look, as (kind, time, title).
fn asked(h: &mut Harness<'static, Option<RedeApp>>) -> Vec<(&'static str, f64, String)> {
    std::mem::take(&mut app_mut(h).actions)
        .into_iter()
        .map(|a| match a {
            Action::AddBookmark { time, title, .. } => ("add", time, title),
            Action::RenameBookmark { time, title, .. } => ("rename", time, title),
            Action::DeleteBookmark { time, .. } => ("delete", time, String::new()),
            Action::JumpToBookmark { time, .. } => ("jump", time, String::new()),
            _ => ("other", 0.0, String::new()),
        })
        .collect()
}

#[test]
fn a_bookmark_jumps_on_a_click_and_its_menu_renames_it_in_place_or_deletes_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = column(dir.path());
    h.run_steps(3);
    h.check_accessibility();
    h.get_by_label("5 bookmarks");
    // The chapter is in the name only when the title doesn't already say it.
    h.get_by_label("Chapter 5, 3:52:35, jump there");
    h.get_by_label("Genly meets the king, 1:57:00, Chapter 3, jump there").click();
    h.run_steps(2);
    assert_eq!(asked(&mut h), [("jump", 7_020.0, String::new())]);

    let rename = |h: &mut Harness<'static, Option<RedeApp>>| {
        h.get_by_label("More for Genly meets the king").click();
        h.run_steps(2);
        h.get_by_label("Rename").click();
        h.run_steps(3);
    };
    // The whole title is selected, so typing replaces it; Enter keeps it.
    rename(&mut h);
    h.check_accessibility();
    h.get_by_label("Bookmark title").type_text("The king's fire");
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(asked(&mut h), [("rename", 7_020.0, "The king's fire".to_owned())]);
    assert!(h.query_by_label("Bookmark title").is_none(), "the row is a row again");
    // Esc keeps the old title.
    rename(&mut h);
    h.get_by_label("Bookmark title").type_text("Not this");
    h.run_steps(1);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(asked(&mut h).is_empty());
    assert!(h.query_by_label("Bookmark title").is_none());
    // An empty title isn't one: Enter leaves the field open.
    rename(&mut h);
    h.key_press(egui::Key::Backspace);
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(asked(&mut h).is_empty());
    h.get_by_label("Bookmark title");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    // Delete, from the same menu.
    h.get_by_label("More for Estraven's warning, the whole speech").click();
    h.run_steps(2);
    h.get_by_label("Delete").click();
    h.run_steps(2);
    assert_eq!(asked(&mut h), [("delete", 14_480.0, String::new())]);
}

#[test]
fn the_bookmarks_tab_adds_where_the_book_is_and_says_how_when_there_are_none() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = column(dir.path());
    let app = app_mut(&mut h);
    app.bookmark_event(crate::bridge::BookmarkMsg::Updated(shelf::BookmarkSnapshot { revision: 2, bookmarks: vec![] }));
    h.run_steps(3);
    h.check_accessibility();
    h.get_by_label("No bookmarks yet");
    h.get_by_label_contains("to mark this spot.");
    h.get_by_label("Add bookmark").click();
    h.run_steps(2);
    assert_eq!(asked(&mut h), [("add", 14_200.0, "Chapter 5".to_owned())]);
}

#[test]
fn a_new_bookmark_is_confirmed_with_a_way_to_the_tab() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    playing(app_mut(&mut h), true);
    app_mut(&mut h).bookmarked("Chapter 5", 14_200.0);
    h.run_steps(3);
    h.check_accessibility();
    h.get_by_label("Bookmarked Chapter 5 at 3:56:40");
    h.get_by_label("Show").click();
    h.run_steps(3);
    let app = app_mut(&mut h);
    assert!(app.panel_shown() && app.ui.panel.tab == super::panel::Tab::Bookmarks);
    // With the tab in view, there's nothing to show: the toast just says so.
    app.bookmarked("Here", 60.0);
    h.run_steps(2);
    h.get_by_label("Bookmarked Here at 1:00");
    assert!(h.query_by_label("Show").is_none());
}

#[test]
fn the_other_device_notice_offers_undo_and_either_way_it_goes() {
    let dir = tempfile::tempdir().unwrap();
    // The toasts alone, so what they ask for can be read before it's applied.
    let mut h = Harness::builder().with_size(egui::vec2(FULL[0], FULL[1])).with_pixels_per_point(2.0).build_ui_state(
        |ui, app: &mut Option<RedeApp>| {
            if let Some(app) = app {
                app.toasts_ui(ui, ui.ctx().content_rect());
            }
        },
        None,
    );
    let ctx = h.ctx.clone();
    *h.state_mut() = Some(app(&ctx, dir.path(), BOOKS.len()));
    playing(app_mut(&mut h), true);
    reconciled(app_mut(&mut h));
    let text = "Picked up where you left off on another device, 3h 56m in.";
    h.run_steps(3);
    h.check_accessibility();
    h.get_by_label(text);
    h.get_by_label("Undo").click();
    h.run_steps(2);
    let actions = std::mem::take(&mut app_mut(&mut h).actions);
    assert!(matches!(actions[..], [Action::UndoReconcile]));
    for a in actions {
        app_mut(&mut h).apply(a);
    }
    h.run_steps(2);
    assert!(app_mut(&mut h).reconciliation.is_none(), "Undo takes the offer");
    assert!(h.query_by_label(text).is_none());
    // Clicked away instead, the position stays as picked up.
    reconciled(app_mut(&mut h));
    h.run_steps(3);
    h.get_by_label(text).click();
    h.run_steps(2);
    let actions = std::mem::take(&mut app_mut(&mut h).actions);
    assert!(matches!(actions[..], [Action::DismissReconcile]));
    for a in actions {
        app_mut(&mut h).apply(a);
    }
    h.run_steps(2);
    let app = app_mut(&mut h);
    assert!(app.reconciliation.is_none());
    assert_eq!(app.player.position, 14_200.0);
    assert!(app.ui.toasts.texts().is_empty());
}

#[test]
fn the_playback_popover_has_the_speech_switches_and_what_skipping_saved() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    playing(app_mut(&mut h), true);
    h.run_steps(2);
    h.get_by_label_contains("Playback speed, ").click();
    h.run_steps(2);
    h.check_accessibility();
    assert!(h.query_by_label_contains("Saved ").is_none(), "skip silence is off");
    // The volume: the speaker mutes with the level kept, and the slider sets
    // the level (a click in its middle is about half).
    h.get_by_label("Volume");
    h.get_by_label("Mute").click();
    h.run_steps(2);
    assert!(app_mut(&mut h).prefs.get(|s| s.playback.muted));
    h.get_by_label("Unmute").click();
    h.run_steps(2);
    assert!(!app_mut(&mut h).prefs.get(|s| s.playback.muted));
    h.get_by_label("Volume").click();
    h.run_steps(2);
    let level = app_mut(&mut h).prefs.get(|s| s.playback.volume);
    assert!((level - 0.5).abs() < 0.08, "the slider's middle is about half: {level}");
    h.get_by_label("Voice boost").click();
    h.run_steps(2);
    assert!(app_mut(&mut h).prefs.get(|s| s.playback.voice_boost));
    h.get_by_label("Skip silence").click();
    h.run_steps(2);
    assert!(app_mut(&mut h).prefs.get(|s| s.playback.skip_silence));
    app_mut(&mut h).player.silence_saved = 192.0;
    h.run_steps(2);
    h.get_by_label("Saved 3m 12s in this book");
}

#[test]
fn a_rename_left_behind_comes_back_as_a_row() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = column(dir.path());
    h.run_steps(2);
    h.get_by_label("More for Genly meets the king").click();
    h.run_steps(2);
    h.get_by_label("Rename").click();
    h.run_steps(3);
    h.get_by_label("Bookmark title");
    // The tab goes away mid-rename, and comes back.
    app_mut(&mut h).ui.panel.tab = super::panel::Tab::Info;
    h.run_steps(3);
    app_mut(&mut h).ui.panel.tab = super::panel::Tab::Bookmarks;
    h.run_steps(3);
    assert!(h.query_by_label("Bookmark title").is_none());
    assert!(asked(&mut h).is_empty(), "nothing was kept");
}

/// Types `text` into the open palette's field, which has the keyboard.
fn type_in_palette(h: &mut Harness<'static, Option<RedeApp>>, text: &str) {
    h.event(egui::Event::Text(text.into()));
    h.run_steps(3);
    assert_eq!(app_mut(h).ui.palette.as_ref().map(|p| p.state.query.as_str()), Some(text));
}

#[test]
fn command_k_command_f_and_slash_open_the_palette_and_esc_closes_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    h.run_steps(2);
    for (modifiers, key) in [(egui::Modifiers::COMMAND, egui::Key::K), (egui::Modifiers::COMMAND, egui::Key::F), (egui::Modifiers::NONE, egui::Key::Slash)] {
        h.key_press_modifiers(modifiers, key);
        h.run_steps(3);
        let palette = app_mut(&mut h).ui.palette.as_ref().map(|p| p.state.query.clone());
        assert_eq!(palette.as_deref(), Some(""), "{key:?} opens it, with nothing typed");
        h.key_press(egui::Key::Escape);
        h.run_steps(3);
        assert!(app_mut(&mut h).ui.palette.is_none(), "esc closes it");
    }
    // The sidebar's field opens it too.
    h.get_by_label("Search (⌘K)").click();
    h.run_steps(3);
    assert!(app_mut(&mut h).ui.palette.is_some());
}

#[test]
fn slash_in_a_text_field_is_typed_not_a_search() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    playing(app_mut(&mut h), true);
    bookmarks(app_mut(&mut h));
    app_mut(&mut h).apply_panel(super::panel::PanelAction::Show(super::panel::Tab::Bookmarks));
    h.run_steps(2);
    h.get_by_label("More for Genly meets the king").click();
    h.run_steps(2);
    h.get_by_label("Rename").click();
    h.run_steps(3);
    h.key_press(egui::Key::Slash);
    h.run_steps(2);
    assert!(app_mut(&mut h).ui.palette.is_none());
}

#[test]
fn enter_opens_the_book_found_and_command_enter_plays_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    h.run_steps(2);
    app_mut(&mut h).apply(Action::OpenPalette);
    h.run_steps(3);
    type_in_palette(&mut h, "piranesi");
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert!(app_mut(&mut h).ui.palette.is_none());
    assert_eq!(app_mut(&mut h).ui.nav.current(), &book_route(2));
    // ⌘⏎ plays, as the page's button does: the fixture has no server, so
    // playing says so.
    app_mut(&mut h).apply(Action::OpenPalette);
    h.run_steps(3);
    type_in_palette(&mut h, "dune");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Enter);
    h.run_steps(3);
    assert!(app_mut(&mut h).ui.palette.is_none());
    let notice = app_mut(&mut h).ui.notice.clone().map(|n| n.text).unwrap_or_default();
    assert!(notice.contains("Sign in to play"), "{notice}");
}

#[test]
fn choosing_an_author_lists_their_books_with_the_first_selected() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    h.run_steps(2);
    app_mut(&mut h).apply(Action::OpenPalette);
    h.run_steps(3);
    type_in_palette(&mut h, "octavia");
    h.get_by_label("Octavia E. Butler").click();
    h.run_steps(4);
    let palette = app_mut(&mut h).ui.palette.as_ref().map(|p| (p.state.query.clone(), p.state.selected));
    let (query, selected) = palette.expect("still open");
    assert_eq!(query, "Octavia E. Butler");
    // ⏎ now opens her book rather than choosing her again.
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(app_mut(&mut h).ui.nav.current(), &book_route(5), "selected row {selected}");
}

#[test]
fn an_empty_query_offers_the_books_in_progress_and_none_found_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = window(dir.path(), BOOKS.len());
    in_progress(app_mut(&mut h));
    app_mut(&mut h).apply(Action::OpenPalette);
    h.run_steps(3);
    h.get_by_label("Piranesi");
    assert!(h.query_by_label("Dune").is_none(), "a finished book isn't in progress");
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(app_mut(&mut h).ui.nav.current(), &book_route(2), "⌘K ⏎ goes back to it");
    app_mut(&mut h).apply(Action::OpenPalette);
    h.run_steps(3);
    type_in_palette(&mut h, "zzqx");
    h.get_by_label("No books match");
}
