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
        h.get_by_label(if paused { "Play (Space)" } else { "Pause (Space)" });
        h.get_by_label("Chapter 5, show chapters");
        h.get_by_label_contains("Now playing: ");
    }
    // The chapters column on each of its tabs, playing and not.
    app_mut(&mut h).apply_prefs(PrefsAction::ShowPanel(true));
    for tab in [super::panel::Tab::Chapters, super::panel::Tab::Bookmarks, super::panel::Tab::Info] {
        app_mut(&mut h).ui.panel.tab = tab;
        h.run_steps(3);
        h.check_accessibility();
        h.get_by_label("Hide chapters (⌥⌘U)");
    }
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
        // The cover, the way to the Player, stays at every width. So do the
        // chapter buttons in a real window (the placement's own tests); the
        // harness's margin takes 16 pt from the smallest, enough to let them
        // go there.
        rects.push(("cover", h.get_by_label("Open the Player").rect()));
        let chapters = h.query_all_by_label("Previous chapter (⇧←)").count() == 1;
        assert!(chapters || width <= crate::MIN_SIZE[0], "the chapter buttons stay at {width}");
        if chapters {
            rects.push(("previous", h.get_by_label("Previous chapter (⇧←)").rect()));
            rects.push(("next", h.get_by_label("Next chapter (⇧→)").rect()));
        }
        // The harness draws the window inside an 8 pt margin.
        for (label, rect) in &rects {
            assert!(rect.min.y >= 8.0 && rect.max.y <= 8.0 + super::BAR_H, "{label} at {width}: {rect:?}");
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
fn sleep_options_and_custom_defaults_are_accessible_from_the_title_bar() {
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
