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
                shell::chrome::stand_in_lights(ui.ctx(), Some(super::BAR_H));
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
    for (name, paused, book) in [("player-playing", false, false), ("player-paused", true, false), ("book-midway", false, true)] {
        scene(h, s, name, || {
            let mut app = app(&ctx, dir.path(), BOOKS.len());
            playing(&mut app, paused);
            if book {
                app.go(book_route(0));
            }
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
    h.set_size(egui::vec2(FULL[0], FULL[1]));
    // Dropping the app (and its runtime) before the next theme.
    *h.state_mut() = None;
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
