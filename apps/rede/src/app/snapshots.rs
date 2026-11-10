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
        form.warning = Some("This server version has a known security issue; update it.".into());
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
    // Twelve books, in each of the orders the pill offers.
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
    // The Playback and Sleep timer popovers, open from the capsule's chips:
    // the timer off, then running.
    for (label, name, timer) in [
        ("Playback speed, 1.5×", "title-speed-menu", None),
        ("Sleep timer", "title-sleep-menu", None),
        ("Sleep timer, ", "title-sleep-running", Some(Sleep::Running)),
    ] {
        *h.state_mut() = None;
        *h.state_mut() = Some({
            let mut app = app(&ctx, dir.path(), BOOKS.len());
            playing(&mut app, false);
            if let Some(timer) = timer {
                sleep(&mut app, timer);
            }
            app
        });
        h.run_steps(2);
        h.get_by_label_contains(label).click();
        h.run_steps(2);
        h.event(egui::Event::PointerGone);
        s.shoot(h, name);
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
    // Settings on each tab, then in the narrowest window (the sidebar a
    // rail, the tabs narrowed).
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
    scene(h, s, "library-playing-narrow", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        playing(&mut app, true);
        app
    });
    // There the column is a sheet over the page.
    scene(h, s, "panel-sheet-narrow", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        playing(&mut app, true);
        app.ui.panel.narrow = true;
        app.apply_panel(super::panel::PanelAction::Show(super::panel::Tab::Chapters));
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
    // Dropping the app (and its runtime) before the next theme.
    *h.state_mut() = None;
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
