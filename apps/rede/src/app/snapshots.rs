//! Review snapshots: renders the real views, offscreen through wgpu, to
//! `target/snapshots/rede-<scene>-<theme>.png`. Not a regression test (nothing
//! is compared); it's for looking at the UI without running the app:
//!
//! ```sh
//! cargo test -p rede --release snapshots -- --ignored --nocapture
//! ```
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
                if app.login.is_some() {
                    app.login_ui(ui);
                } else {
                    app.main_ui(ui);
                }
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
    for (name, both, waiting) in [("signin-openid", false, false), ("signin-both", true, false), ("signin-waiting", false, true)] {
        scene(h, s, name, || signin(&ctx, dir.path(), both, waiting));
    }
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
    for (name, paused) in [("player-playing", false), ("player-paused", true)] {
        scene(h, s, name, || {
            let mut app = app(&ctx, dir.path(), BOOKS.len());
            playing(&mut app, paused);
            app
        });
    }
    // The speed menu, open over the bar.
    *h.state_mut() = None;
    *h.state_mut() = Some({
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        playing(&mut app, false);
        app
    });
    h.run_steps(2);
    h.get_by_label("Playback speed, 1.5×").click();
    h.run_steps(2);
    h.event(egui::Event::PointerGone);
    s.shoot(h, "player-speed");
    // The playing book's page, scrolled down to the chapter playing. The
    // speed menu's open state lives in the shared context, not the app, so
    // it would follow into this scene unless closed.
    egui::Popup::close_all(&ctx);
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
    // The same page at the smallest window Rede opens: the cover goes
    // above the text and the columns stack.
    h.set_size(egui::vec2(crate::MIN_SIZE[0], 560.0));
    scene(h, s, "book-narrow", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        app.go(book_route(0));
        app
    });
    scene(h, s, "player-narrow", || {
        let mut app = app(&ctx, dir.path(), BOOKS.len());
        playing(&mut app, true);
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
    h.set_size(egui::vec2(FULL[0], FULL[1]));
    // The bar with a timer running, and with its notice over the page.
    for (timer, name) in [(Sleep::Running, "player-sleep"), (Sleep::Ending, "player-notice")] {
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
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let out = out.canonicalize().unwrap();
    for (theme, name) in [(egui::Theme::Light, "light"), (egui::Theme::Dark, "dark")] {
        let mut h = window(theme);
        println!("{name} pictures:");
        scenes(&mut h, &Shooter { out: out.clone(), theme: name });
    }
}
