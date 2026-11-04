//! Review snapshots: renders the real views, offscreen through wgpu, to
//! `target/snapshots/<scene>-<theme>.png`. Not a regression test (nothing is
//! compared); it's for looking at the UI without running the app:
//!
//! ```sh
//! cargo test -p ond-player --release snapshots -- --ignored --nocapture
//! ```
//!
//! The data is a fixed, realistic library with generated cover art, a queue
//! with several sources and a track part-way through. No server, no audio
//! device, no keychain; settings and caches live in a temp dir.
//!
//! The scenes are in `scenes`, the README's screenshots in `sleeves` and
//! the frame-time budget in `perf`; the fixture they share is
//! `app::fixture`.

use super::fixture::*;
use super::*;
use egui::{Color32, ColorImage, Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use std::path::PathBuf;

mod perf;
mod scenes;
mod sleeves;

struct Shooter {
    out: PathBuf,
    theme: &'static str,
}

impl Shooter {
    fn shoot(&self, h: &mut Harness<'_, Option<OndApp>>, scene: &str) {
        h.run_steps(4);
        self.shoot_now(h, scene);
    }

    /// Renders the frame as it is, without letting anything settle first
    /// (for the middle of a fade).
    fn shoot_now(&self, h: &mut Harness<'_, Option<OndApp>>, scene: &str) {
        // SNAPSHOTS=settings renders only the scenes with "settings" in their name.
        if std::env::var("SNAPSHOTS").is_ok_and(|only| !scene.contains(&only)) {
            return;
        }
        match h.render() {
            Ok(img) => {
                let path = self.out.join(format!("{scene}-{}.png", self.theme));
                img.save(&path).unwrap_or_else(|e| panic!("saving {}: {e}", path.display()));
                println!("  {}", path.display());
            }
            Err(e) => panic!("rendering {scene}: {e}"),
        }
    }
}

/// Renders `scenes` in each of `themes`, offscreen through wgpu, into
/// `target/snapshots`, with `managed` as managed.json when given.
fn shoot_scenes(themes: &[(egui::Theme, &'static str)], managed: Option<&str>, scenes: fn(&mut Harness<'_, Option<OndApp>>, &Shooter)) {
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let out = out.canonicalize().unwrap();
    for &(theme, name) in themes {
        let dir = tempfile::tempdir().unwrap();
        // Our own render state (the harness's default one, made here), so the
        // app knows what it renders to, for the visualizer's pipelines.
        let rs = egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), eframe::egui_wgpu::RendererOptions::PREDICTABLE);
        let target = crate::visuals::gpu::Target::new(rs.target_format, &rs.adapter);
        RENDERER.set(Some(std::sync::Arc::clone(&rs.renderer)));
        let mut h = Harness::builder()
            .with_size(FULL)
            .with_pixels_per_point(2.0)
            .with_theme(theme)
            .with_accessibility_check(false)
            .allow_missing_glyphs()
            .with_os(egui::os::OperatingSystem::Mac)
            .renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(rs))
            .build_ui_state(
                |ui, app: &mut Option<OndApp>| {
                    if let Some(app) = app {
                        if app.login.is_some() {
                            app.login_ui(ui);
                        } else {
                            app.main_ui(ui);
                        }
                        traffic_lights(ui.ctx(), title_bar_h(app.prefs_now().mode));
                    }
                },
                None,
            );
        let ctx = h.ctx.clone();
        if let Some(json) = managed {
            std::fs::write(dir.path().join("managed.json"), json).unwrap();
        }
        let mut app = populated(&ctx, dir.path());
        app.gpu = Some(target);
        // `build` installs the theme with the saved (system) preference.
        theme::set_pref(&ctx, if theme == egui::Theme::Dark { ThemePref::Dark } else { ThemePref::Light });
        *h.state_mut() = Some(app);
        println!("{name} ({:?} pictures):", target.offscreen);
        scenes(&mut h, &Shooter { out: out.clone(), theme: name });
        // Drop the app (and its engine/runtime) before the next theme.
        *h.state_mut() = None;
    }
}

thread_local! {
    /// The renderer [`shoot_scenes`] made, so a scene can look at what's
    /// held in its callback resources.
    static RENDERER: std::cell::RefCell<Option<std::sync::Arc<egui::mutex::RwLock<eframe::egui_wgpu::Renderer>>>> = const { std::cell::RefCell::new(None) };
}
