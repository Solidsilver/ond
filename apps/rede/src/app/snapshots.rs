//! Review snapshots: renders the real views, offscreen through wgpu, to
//! `target/snapshots/rede-<scene>-<theme>.png`. Not a regression test (nothing
//! is compared); it's for looking at the UI without running the app:
//!
//! ```sh
//! cargo test -p rede --release snapshots -- --ignored --nocapture
//! ```
//!
//! Set `OND_SNAPSHOT_DIR` to keep independent render runs in separate folders.
//!
//! The data is the fixture shelf (`app::fixture`): twelve books, generated
//! covers in three shapes, missing art, titles, authors and chapters. No server, no keychain;
//! the settings and caches live in a temp dir.

use super::fixture::*;
use super::*;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use std::path::PathBuf;

struct Shooter {
    out: PathBuf,
    theme: &'static str,
}

impl Shooter {
    fn shoot(&self, h: &mut Harness<'_, Option<RedeApp>>, scene: &str) {
        // The app installs the saved preference (the system's) when it starts;
        // the picture is taken in the theme the run asked for.
        let pref = if self.theme == "dark" { shell::theme::ThemePref::Dark } else { shell::theme::ThemePref::Light };
        shell::theme::set_pref(&h.ctx, pref);
        h.run_steps(4);
        let path = self.out.join(format!("rede-{scene}-{}.png", self.theme));
        match h.render() {
            Ok(image) => image.save(&path).unwrap_or_else(|e| panic!("saving {}: {e}", path.display())),
            Err(e) => panic!("rendering {scene}: {e}"),
        }
        println!("  {}", path.display());
    }
}

/// The window as the app draws it, at `FULL`, with stand-in traffic lights on
/// macOS (where the system's own aren't in an offscreen render).
fn window(theme: egui::Theme) -> Harness<'static, Option<RedeApp>> {
    Harness::builder()
        .with_size(egui::vec2(FULL[0], FULL[1]))
        .with_pixels_per_point(2.0)
        .with_theme(theme)
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .with_os(egui::os::OperatingSystem::Mac)
        .build_ui_state(
            |ui, app: &mut Option<RedeApp>| {
                let Some(app) = app else { return };
                whole_window(ui, |ui| if app.login.is_some() { app.login_ui(ui) } else { app.main_ui(ui) });
                shell::chrome::stand_in_lights(ui.ctx(), (app.mode != crate::window::Mode::Mini).then_some(super::BAR_H));
            },
            None,
        )
}

fn scenes(h: &mut Harness<'_, Option<RedeApp>>, s: &Shooter) {
    let ctx = h.ctx.clone();
    /// Draws one scene: a fresh app, and the picture.
    fn scene(h: &mut Harness<'_, Option<RedeApp>>, s: &Shooter, name: &str, build: impl FnOnce() -> RedeApp) {
        // The last app lets go of its shelf before the next one opens it.
        *h.state_mut() = None;
        *h.state_mut() = Some(build());
        s.shoot(h, name);
    }
    // The sign-in screen, as a first start shows it.
    let dir = tempfile::tempdir().unwrap();
    scene(h, s, "signin", || signed_out(&ctx, dir.path()));
    // Once the server has said how it signs people in: passwords, its
    // provider, both, and waiting on the browser (through Rede's own link, and
    // through Audiobookshelf's when the server hasn't allowed Rede's).
    for (name, methods, waiting) in [
        ("signin-password", &["local"][..], None),
        ("signin-openid", &["openid"], None),
        ("signin-both", &["local", "openid"], None),
        ("signin-waiting", &["openid"], Some(pending_openid())),
        ("signin-fallback", &["openid"], Some(pending_openid_fallback())),
    ] {
        scene(h, s, name, || signin_with(&ctx, dir.path(), methods, waiting));
    }
    // Waiting on the server for a password, and the ways it can go wrong: a
    // wrong password with a warning about the server's version, an address
    // that isn't one, and a server too old to use.
    scene(h, s, "signin-busy", || {
        let mut app = signin_with(&ctx, dir.path(), &["local", "openid"], None);
        let form = app.login.as_mut().unwrap();
        (form.username, form.busy) = ("sam".into(), true);
        app.session.connecting = true;
        app
    });
    let wrong_password = |app: &mut RedeApp| {
        let form = app.login.as_mut().unwrap();
        form.username = "sam".into();
        form.error = Some(crate::bridge::sign_in_error(&abs::Error::Auth(abs::AuthError::BadCredentials)));
        form.warning = Some("Audiobookshelf 2.35.1 has a known security issue. Update the server to 2.36 or newer.".into());
    };
    scene(h, s, "signin-error", || {
        let mut app = signin_with(&ctx, dir.path(), &["local"], None);
        wrong_password(&mut app);
        app
    });
    scene(h, s, "signin-bad-address", || {
        let mut app = signed_out(&ctx, dir.path());
        let form = app.login.as_mut().unwrap();
        form.server = "books example".into();
        form.error = Some(crate::bridge::sign_in_error(&abs::Client::new("books example", account().device()).err().unwrap()));
        app
    });
    scene(h, s, "signin-too-old", || {
        let mut app = signed_out(&ctx, dir.path());
        let form = app.login.as_mut().unwrap();
        form.server = "https://books.example.com".into();
        form.error = Some("This server is version 2.19.0. Rede needs Audiobookshelf 2.26 or newer.".into());
        app
    });
    // Both ways in, gone wrong, in the smallest window Rede opens.
    h.set_size(egui::vec2(crate::MIN_SIZE[0], crate::MIN_SIZE[1]));
    scene(h, s, "signin-narrow", || {
        let mut app = signin_with(&ctx, dir.path(), &["local", "openid"], None);
        wrong_password(&mut app);
        app
    });
    scene(h, s, "signin-waiting-narrow", || signin_with(&ctx, dir.path(), &["openid"], Some(pending_openid_fallback())));
    h.set_size(egui::vec2(FULL[0], FULL[1]));
    // A library with nothing in it yet.
    scene(h, s, "library-empty", || app(&ctx, dir.path(), 0));
    // Twelve books, by title and by author.
    for (sort, name) in [(crate::settings::LibrarySort::Title, "title"), (crate::settings::LibrarySort::Author, "author")] {
        scene(h, s, &format!("library-{name}"), || {
            let app = app(&ctx, dir.path(), BOOKS.len());
            app.prefs.edit(|p| p.sort = sort);
            app
        });
    }
    // A book's page, with its description and chapters.
    scene(h, s, "book", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        app.go(book_route(0));
        app
    });
    for (n, name) in [(1, "book-square"), (2, "book-landscape"), (3, "book-missing-art")] {
        scene(h, s, name, || {
            let mut app = app(&ctx, dir.path(), BOOKS.len());
            app.go(book_route(n));
            app
        });
    }
    // A book partway, opened from Home (the breadcrumb says so), with how
    // far along it is under Resume; and a finished one.
    scene(h, s, "book-started", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        listened(&mut app);
        app.go(book_route(2));
        app
    });
    scene(h, s, "book-finished", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        listened(&mut app);
        app.go(book_route(4));
        app
    });
    // The … beside Play, on a book partway: finish it, or start it over.
    scene(h, s, "book-more-menu", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        listened(&mut app);
        app.go(book_route(2));
        app
    });
    h.get_by_label_contains("More for ").click();
    h.run_steps(2);
    h.event(egui::Event::PointerGone);
    s.shoot(h, "book-more-menu");
    // Home with books partway (the one heard last first) and one finished,
    // as the window opens with something in progress; Home with nothing
    // begun; and the library with the same mix on its covers.
    scene(h, s, "home", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        listened(&mut app);
        app
    });
    scene(h, s, "home-nothing", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        app.go(Route::Home);
        app
    });
    scene(h, s, "library-progress", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        listened(&mut app);
        app.go(Route::Library);
        app
    });
    // The library narrowed to each stage, by the books heard last, with the
    // order's menu open, and narrowed to nothing. A folder of their own, so
    // the saved order and filter don't carry into another scene's app.
    use crate::settings::{LibraryFilter, LibrarySort};
    let shelf_dir = tempfile::tempdir().unwrap();
    let library = |filter: LibraryFilter, sort: LibrarySort, begun: bool| {
        let mut app = app(&ctx, shelf_dir.path(), BOOKS.len());
        if begun {
            listened(&mut app);
        }
        app.apply_prefs(PrefsAction::Filter(filter));
        app.apply_prefs(PrefsAction::Sort(sort));
        app.go(Route::Library);
        app
    };
    for (filter, name) in [(LibraryFilter::InProgress, "in-progress"), (LibraryFilter::NotStarted, "not-started"), (LibraryFilter::Finished, "finished")] {
        scene(h, s, &format!("library-{name}"), || library(filter, LibrarySort::Title, true));
    }
    scene(h, s, "library-listened", || library(LibraryFilter::All, LibrarySort::Listened, true));
    scene(h, s, "library-filter-empty", || library(LibraryFilter::InProgress, LibrarySort::Title, false));
    scene(h, s, "library-sort-menu", || library(LibraryFilter::All, LibrarySort::Progress, true));
    h.get_by_label("Sort: Progress").click();
    h.run_steps(2);
    h.event(egui::Event::PointerGone);
    s.shoot(h, "library-sort-menu");
    // Where the heading has less room: beside the chapters column, and with
    // the sidebar folded to its rail.
    scene(h, s, "library-panel-in-progress", || {
        let app = library(LibraryFilter::InProgress, LibrarySort::Listened, true);
        app.prefs.edit(|p| p.show_panel = true);
        app
    });
    scene(h, s, "library-collapsed-finished", || {
        let app = library(LibraryFilter::Finished, LibrarySort::Title, true);
        app.prefs.edit(|p| p.sidebar_collapsed = true);
        app
    });
    // Nothing playing: the capsule's quiet line, and the column saying so.
    scene(h, s, "nothing-playing", || {
        let app = app(&ctx, dir.path(), BOOKS.len());
        app.prefs.edit(|p| p.show_panel = true);
        app
    });
    // The library and a book's page while a book plays (the page's button
    // pauses it), and paused.
    for (name, route, paused) in [("library-playing", None, false), ("library-paused", None, true), ("book-playing", Some(0), false)] {
        scene(h, s, name, || {
            let mut app = app(&ctx, dir.path(), BOOKS.len());
            playing(&mut app, paused);
            if let Some(n) = route {
                app.go(book_route(n));
            }
            app
        });
    }
    // The chapters column on each tab, beside the library.
    for (tab, name) in
        [(super::panel::Tab::Chapters, "panel-chapters"), (super::panel::Tab::Bookmarks, "panel-bookmarks"), (super::panel::Tab::Info, "panel-info")]
    {
        scene(h, s, name, || {
            let mut app = app(&ctx, dir.path(), BOOKS.len());
            playing(&mut app, false);
            app.apply_panel(super::panel::PanelAction::Show(tab));
            app
        });
    }
    // The same book without its cover: the capsule's thumbnail and the info
    // column's are generated.
    scene(h, s, "panel-info-missing-art", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        playing(&mut app, false);
        without_art(&mut app);
        app.apply_panel(super::panel::PanelAction::Show(super::panel::Tab::Info));
        app
    });
    // The bookmarks tab with rows (the third, just behind where the book
    // is, marked), and one being renamed.
    scene(h, s, "panel-bookmarks-rows", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        playing(&mut app, true);
        bookmarks(&mut app);
        app.apply_panel(super::panel::PanelAction::Show(super::panel::Tab::Bookmarks));
        app
    });
    *h.state_mut() = None;
    *h.state_mut() = Some({
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        playing(&mut app, true);
        bookmarks(&mut app);
        app.apply_panel(super::panel::PanelAction::Show(super::panel::Tab::Bookmarks));
        app
    });
    h.run_steps(2);
    h.get_by_label("More for Genly meets the king").click();
    h.run_steps(2);
    h.event(egui::Event::PointerGone);
    s.shoot(h, "panel-bookmarks-menu");
    h.get_by_label("Rename").click();
    h.run_steps(3);
    h.get_by_label("Bookmark title").type_text("The king's fire");
    h.event(egui::Event::PointerGone);
    s.shoot(h, "panel-bookmarks-rename");
    egui::Popup::close_all(&ctx);
    // The toasts: a bookmark made with ⌘B, and a position picked up from
    // another device; then both, stacked.
    scene(h, s, "toast-bookmarked", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        playing(&mut app, true);
        app.bookmarked("Chapter 5", 14_200.0);
        app
    });
    scene(h, s, "toast-reconciled", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        playing(&mut app, true);
        reconciled(&mut app);
        app
    });
    scene(h, s, "toast-both", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        playing(&mut app, true);
        reconciled(&mut app);
        app.bookmarked("Chapter 5", 14_200.0);
        app
    });
    // The Playback and Sleep timer popovers, open from the capsule's chips:
    // the timer off, then running; the Playback one with the volume row, its
    // level moved and muted.
    for (label, name, timer) in [
        ("Playback speed, 1.5×", "title-speed-menu", None),
        ("Sleep timer", "title-sleep-menu", None),
        ("Sleep timer, ", "title-sleep-running", Some(Sleep::Running)),
        ("Playback speed, 1.5×", "title-speed-menu-switches", None),
        ("Playback speed, 1.5×", "title-speed-menu-volume", None),
        ("Playback speed, 1.5×", "title-speed-menu-muted", None),
    ] {
        *h.state_mut() = None;
        *h.state_mut() = Some({
            let mut app = app(&ctx, dir.path(), BOOKS.len());
            playing(&mut app, false);
            if let Some(timer) = timer {
                sleep(&mut app, timer);
            }
            // Both switches on, and three minutes cut from the book.
            if name.ends_with("switches") {
                app.prefs.edit(|p| (p.playback.voice_boost, p.playback.skip_silence) = (true, true));
                app.player.silence_saved = 192.0;
            }
            if name.ends_with("volume") {
                app.prefs.edit(|p| p.playback.volume = 0.65);
            }
            if name.ends_with("muted") {
                app.prefs.edit(|p| (p.playback.volume, p.playback.muted) = (0.4, true));
            }
            app
        });
        h.run_steps(2);
        h.get_by_label_contains(label).click();
        h.run_steps(2);
        h.event(egui::Event::PointerGone);
        s.shoot(h, name);
        // The volume scenes move a preference every later picture's app reads
        // from the same settings file; put it back. (The switches are on in
        // every picture that shows them, so they stay.)
        if (name.ends_with("volume") || name.ends_with("muted"))
            && let Some(app) = h.state_mut().as_mut()
        {
            app.prefs.edit(|p| (p.playback.volume, p.playback.muted) = (1.0, false));
        }
        egui::Popup::close_all(&ctx);
    }
    // The sidebar folded to its rail; the account syncing, connecting and
    // out of reach; an account with several libraries on one shelf; and
    // the account's menu open.
    scene(h, s, "sidebar-collapsed", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        playing(&mut app, false);
        app.prefs.edit(|p| p.sidebar_collapsed = true);
        app
    });
    for (name, state) in [("sidebar-syncing", 0), ("sidebar-connecting", 1), ("sidebar-offline", 2)] {
        scene(h, s, name, || {
            let mut app = app(&ctx, dir.path(), BOOKS.len());
            (app.sync.running, app.sync.applied, app.session.connecting, app.sync.offline) = (state == 0, 40, state == 1, state == 2);
            app
        });
    }
    scene(h, s, "sidebar-collapsed-offline", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        app.prefs.edit(|p| p.sidebar_collapsed = true);
        app.sync.offline = true;
        app
    });
    scene(h, s, "sidebar-libraries", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        if let Some(account) = app.session.account.as_mut() {
            account.libraries.push(crate::settings::KnownLibrary { id: "lib-kids".into(), name: "Kids".into() });
        }
        app
    });
    *h.state_mut() = None;
    *h.state_mut() = Some(app(&ctx, dir.path(), BOOKS.len()));
    h.run_steps(2);
    h.get_by_label("sam, books.example.com").click();
    h.run_steps(4);
    h.event(egui::Event::PointerGone);
    s.shoot(h, "account-menu");
    egui::Popup::close_all(&ctx);
    // The keyboard shortcuts' sheet.
    scene(h, s, "sheet-shortcuts", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        playing(&mut app, false);
        app.sheet = Some(super::sheets::Sheet::Shortcuts);
        app
    });
    // The palette (⌘K): the books in progress for an empty query, what a
    // query finds (books, then the author as a name of her own), and a query
    // that finds nothing.
    for (name, query) in [("palette-empty", ""), ("palette-query", "le guin"), ("palette-none", "zzqx")] {
        scene(h, s, name, || palette(&ctx, dir.path(), query));
    }
    // The question before signing out.
    scene(h, s, "sheet-sign-out", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        playing(&mut app, false);
        app.apply(Action::AskSignOut);
        app
    });
    // Settings, Ond's window, on each tab, then in the narrowest window (the
    // sidebar a rail).
    for (size, suffix) in [(FULL, ""), (crate::MIN_SIZE, "-narrow")] {
        h.set_size(egui::vec2(size[0], size[1]));
        for tab in super::settings_page::Tab::ALL {
            scene(h, s, &format!("settings-{}{suffix}", tab.label().to_lowercase()), || {
                let mut app = app(&ctx, dir.path(), BOOKS.len());
                playing(&mut app, false);
                app.sync.synced = Some(std::time::SystemTime::now() - std::time::Duration::from_secs(12 * 60));
                app.apply(Action::OpenSettings(tab));
                app
            });
        }
    }
    h.set_size(egui::vec2(FULL[0], FULL[1]));
    // The capsule's cover under the pointer: the way to the Player.
    *h.state_mut() = None;
    *h.state_mut() = Some({
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        playing(&mut app, false);
        app
    });
    h.run_steps(2);
    let cover = h.get_by_label("Open the Player").rect().center();
    h.hover_at(cover);
    s.shoot(h, "capsule-cover-hover");
    h.event(egui::Event::PointerGone);
    // A right-click on the capsule: what its parts do, and Stop playback.
    h.get_by_label_contains("Now playing: ").click_secondary();
    h.run_steps(2);
    h.event(egui::Event::PointerGone);
    s.shoot(h, "capsule-menu");
    egui::Popup::close_all(&ctx);
    // The capsule at the window's smallest width, then as it grows with the
    // window: Ond's 1440, a 1920 display, a 2560 one full screen.
    for width in [crate::MIN_SIZE[0], 1440.0, 1920.0, 2560.0] {
        h.set_size(egui::vec2(width, 420.0));
        scene(h, s, &format!("bar-{width}"), || {
            let mut app = app(&ctx, dir.path(), BOOKS.len());
            playing(&mut app, false);
            app
        });
    }
    h.set_size(egui::vec2(FULL[0], FULL[1]));
    // The playing book's page, scrolled down to the chapter playing.
    *h.state_mut() = None;
    *h.state_mut() = Some({
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        playing(&mut app, false);
        app.go(book_route(0));
        app
    });
    h.run_steps(2);
    h.hover_at(egui::pos2(FULL[0] / 2.0, FULL[1] / 2.0));
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -240.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
        source: egui::MouseWheelSource::Wheel,
    });
    h.run_steps(30);
    h.event(egui::Event::PointerGone);
    s.shoot(h, "book-midway");
    // A description that arrived as HTML, scrolled until the prose fills the
    // page: bold and italic runs, decoded entities, a list, a quote and a
    // link.
    *h.state_mut() = None;
    *h.state_mut() = Some({
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        app.go(book_route(1));
        app
    });
    h.run_steps(2);
    h.hover_at(egui::pos2(FULL[0] / 2.0, FULL[1] / 2.0));
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -290.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
        source: egui::MouseWheelSource::Wheel,
    });
    h.run_steps(30);
    h.event(egui::Event::PointerGone);
    s.shoot(h, "book-html");
    // The same page at the smallest window Rede opens: the cover goes
    // above the text and the columns stack.
    h.set_size(egui::vec2(crate::MIN_SIZE[0], 560.0));
    scene(h, s, "book-narrow", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        app.go(book_route(0));
        app
    });
    scene(h, s, "book-started-narrow", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        listened(&mut app);
        app.go(book_route(2));
        app
    });
    scene(h, s, "home-narrow", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        listened(&mut app);
        app
    });
    scene(h, s, "palette-narrow", || palette(&ctx, dir.path(), "earthsea"));
    scene(h, s, "library-playing-narrow", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        playing(&mut app, true);
        app
    });
    scene(h, s, "library-in-progress-narrow", || library(LibraryFilter::InProgress, LibrarySort::Listened, true));
    scene(h, s, "library-filter-empty-narrow", || library(LibraryFilter::Finished, LibrarySort::Title, false));
    scene(h, s, "library-sort-menu-narrow", || library(LibraryFilter::NotStarted, LibrarySort::Added, true));
    h.get_by_label("Sort: Recently added").click();
    h.run_steps(2);
    h.event(egui::Event::PointerGone);
    s.shoot(h, "library-sort-menu-narrow");
    // There the column is a sheet over the page.
    scene(h, s, "panel-sheet-narrow", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        playing(&mut app, true);
        app.ui.panel.narrow = true;
        app.apply_panel(super::panel::PanelAction::Show(super::panel::Tab::Chapters));
        app
    });
    scene(h, s, "panel-sheet-narrow-bookmarks", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        playing(&mut app, true);
        bookmarks(&mut app);
        app.ui.panel.narrow = true;
        app.apply_panel(super::panel::PanelAction::Show(super::panel::Tab::Bookmarks));
        app
    });
    scene(h, s, "toast-reconciled-narrow", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        playing(&mut app, true);
        reconciled(&mut app);
        app
    });
    // The Player and Mini at their usual sizes and their smallest, with no
    // timer, with one running, and in its last minute with the notice up.
    use crate::window::Mode;
    for (mode, size, name) in [
        (Mode::Player, Mode::Player.size(), "mode-player"),
        (Mode::Player, Mode::Player.min_size(), "mode-player-narrow"),
        (Mode::Mini, Mode::Mini.size(), "mode-mini"),
        (Mode::Mini, Mode::Mini.min_size(), "mode-mini-narrow"),
    ] {
        h.set_size(egui::vec2(size[0], size[1]));
        for (timer, suffix) in [(None, ""), (Some(Sleep::Running), "-sleep"), (Some(Sleep::Chapter), "-sleep-chapter"), (Some(Sleep::Ending), "-notice")] {
            // Mini shows no timer but its notice.
            if mode == Mode::Mini && matches!(timer, Some(Sleep::Running | Sleep::Chapter)) {
                continue;
            }
            scene(h, s, &format!("{name}{suffix}"), || {
                let mut app = app(&ctx, dir.path(), BOOKS.len());
                playing(&mut app, true);
                if let Some(timer) = timer {
                    sleep(&mut app, timer);
                }
                app.mode = mode;
                app
            });
        }
    }
    // A book with no cover playing: the Player and Mini draw its generated one.
    for (mode, name) in [(Mode::Player, "mode-player-missing-art"), (Mode::Mini, "mode-mini-missing-art")] {
        let size = mode.size();
        h.set_size(egui::vec2(size[0], size[1]));
        scene(h, s, name, || {
            let mut app = app(&ctx, dir.path(), BOOKS.len());
            playing(&mut app, true);
            without_art(&mut app);
            app.mode = mode;
            app
        });
    }
    // The sleep menu, open over the Player with a timer waiting for the
    // chapter's end.
    let size = Mode::Player.size();
    h.set_size(egui::vec2(size[0], size[1]));
    *h.state_mut() = None;
    *h.state_mut() = Some({
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        playing(&mut app, true);
        sleep(&mut app, Sleep::Chapter);
        app.mode = Mode::Player;
        app
    });
    h.run_steps(2);
    h.get_by_label("Sleep timer, End of chapter").click();
    h.run_steps(2);
    h.event(egui::Event::PointerGone);
    s.shoot(h, "mode-player-sleep-menu");
    egui::Popup::close_all(&ctx);
    // The Playback popover over the Player, and the shortcuts in one column.
    h.get_by_label_contains("Playback speed, ").click();
    h.run_steps(2);
    h.event(egui::Event::PointerGone);
    s.shoot(h, "mode-player-speed-menu");
    egui::Popup::close_all(&ctx);
    // Its "…": the shortcuts, and Stop playback.
    h.get_by_label("More").click();
    h.run_steps(2);
    h.event(egui::Event::PointerGone);
    s.shoot(h, "mode-player-more-menu");
    egui::Popup::close_all(&ctx);
    // A bookmark made in the Player: the confirmation over its foot.
    if let Some(app) = h.state_mut().as_mut() {
        app.player.sleep = None;
        app.bookmarked("Chapter 5", 14_200.0);
        reconciled(app);
    }
    s.shoot(h, "mode-player-bookmarked");
    if let Some(app) = h.state_mut().as_mut() {
        app.reconciliation = None;
    }
    if let Some(app) = h.state_mut().as_mut() {
        app.sheet = Some(super::sheets::Sheet::Shortcuts);
    }
    s.shoot(h, "mode-player-shortcuts");
    if let Some(app) = h.state_mut().as_mut() {
        app.sheet = None;
    }
    h.set_size(egui::vec2(FULL[0], FULL[1]));
    // The title bar with a timer running (the sleep chip lit), one waiting
    // for the chapter's end, and the notice over the page.
    for (timer, name) in [(Sleep::Running, "playing-sleep"), (Sleep::Chapter, "playing-sleep-chapter"), (Sleep::Ending, "playing-notice")] {
        scene(h, s, name, || {
            let mut app = app(&ctx, dir.path(), BOOKS.len());
            playing(&mut app, false);
            sleep(&mut app, timer);
            app
        });
    }
    // Zoomed to 150%. Sizes are in points: a 1100×720 window is 733×480 of
    // them, and the narrowest library window is its minimum in points at any
    // zoom. The Player's and Mini's usual sizes are under their minimums at
    // 150%, so they open at those. A folder of their own, so the saved zoom
    // doesn't carry into another scene's app.
    let zoomed = tempfile::tempdir().unwrap();
    let at_150 = |mut app: RedeApp| {
        app.apply_prefs(PrefsAction::Zoom(150));
        app.ui.toasts = shell::kit::Toasts::default();
        app
    };
    for (size, mode, name) in [
        ([FULL[0] / 1.5, FULL[1] / 1.5], Mode::Full, "zoom-150-library"),
        (crate::MIN_SIZE, Mode::Full, "zoom-150-library-narrow"),
        (Mode::Player.min_size(), Mode::Player, "zoom-150-player"),
        (Mode::Mini.min_size(), Mode::Mini, "zoom-150-mini"),
    ] {
        h.set_size(egui::vec2(size[0], size[1]));
        scene(h, s, name, || {
            let mut app = at_150(app(&ctx, zoomed.path(), BOOKS.len()));
            playing(&mut app, mode != Mode::Full);
            app.mode = mode;
            app
        });
    }
    h.set_size(egui::vec2(FULL[0] / 1.5, FULL[1] / 1.5));
    scene(h, s, "zoom-150-settings-appearance", || {
        let mut app = at_150(app(&ctx, zoomed.path(), BOOKS.len()));
        app.apply(Action::OpenSettings(super::settings_page::Tab::Appearance));
        app
    });
    h.set_size(egui::vec2(FULL[0], FULL[1]));
    // Dropping the app (and its runtime) before the next theme.
    *h.state_mut() = None;
}

/// The palette open over the library with `query` typed, a book playing
/// and another half listened to.
fn palette(ctx: &egui::Context, dir: &std::path::Path, query: &str) -> RedeApp {
    let mut app = app(ctx, dir, BOOKS.len());
    playing(&mut app, true);
    in_progress(&mut app);
    let item = sources::ItemKey::new(account().id, "book-0");
    let yesterday = std::time::SystemTime::now() - std::time::Duration::from_secs(86_400);
    let paused = sources::Progress { item, position: sources::Position::from_secs(14_200.0), finished: false, at: Some(yesterday) };
    app.positions.insert("book-0".into(), paused);
    app.apply(Action::OpenPalette);
    if let Some(p) = app.ui.palette.as_mut() {
        p.typed(query);
    }
    app
}

/// The sleep timers the scenes show.
#[derive(Clone, Copy)]
enum Sleep {
    /// 23:41 to go.
    Running,
    /// Waiting for the chapter to end.
    Chapter,
    /// In its last minute, with the notice up.
    Ending,
}

fn sleep(app: &mut RedeApp, timer: Sleep) {
    use crate::sleep::{Choice, Timer};
    use std::time::{Duration, Instant};
    let now = Instant::now();
    // A timer started earlier, so it has the time left the scene wants.
    let started = |ago: u64| now.checked_sub(Duration::from_secs(ago)).unwrap_or(now);
    let chapter_end = app.player.book.as_ref().map_or(0.0, |b| b.timeline.chapter_range(app.player.position).1);
    app.player.sleep = Some(match timer {
        Sleep::Running => Timer::new(Choice::Minutes(30), chapter_end, started(30 * 60 - (23 * 60 + 41))),
        Sleep::Chapter => Timer::new(Choice::Chapter, chapter_end, now),
        Sleep::Ending => Timer::new(Choice::Minutes(5), chapter_end, started(5 * 60 - 48)),
    });
}

#[test]
#[ignore = "renders PNGs for review; run with --ignored"]
fn snapshots() {
    let out =
        std::env::var_os("OND_SNAPSHOT_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots"));
    std::fs::create_dir_all(&out).unwrap();
    let out = out.canonicalize().unwrap();
    for (theme, name) in [(egui::Theme::Light, "light"), (egui::Theme::Dark, "dark")] {
        let mut h = window(theme);
        println!("{name} pictures:");
        scenes(&mut h, &Shooter { out: out.clone(), theme: name });
    }
}
