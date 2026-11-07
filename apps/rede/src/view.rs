//! The window's one screen: a title bar and, until there are sources to
//! read from, a calm empty state. Nothing here moves, so nothing asks for
//! frames; the window draws when the system or the user gives it a reason.

use egui::{Frame, Sense, Ui, ViewportCommand};
use shell::icons::Icon;
use shell::theme::Tokens;
use shell::{chrome, csd};

/// The title bar's height, as Ond's toolbar: the traffic lights (macOS) and
/// our own window buttons (Linux) sit centred in it.
pub const BAR_H: f32 = 52.0;

/// Roughly how tall the empty state is, to set it a little above the middle.
const EMPTY_H: f32 = 120.0;

pub fn root(ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let t = Tokens::of(ui);
    egui::Panel::top("title-bar").exact_size(BAR_H).resizable(false).frame(Frame::new().fill(t.bg)).show(ui, title_bar);
    egui::CentralPanel::default().frame(Frame::new().fill(t.bg)).show(ui, |ui| {
        ui.add_space(((ui.available_height() - EMPTY_H) * 0.4 - 48.0).max(0.0));
        shell::widgets::empty_state(ui, Icon::Headphones, "Rede", "No sources yet.");
    });
    // Window buttons and resize edges where the system draws none (Linux).
    csd::overlay(&ctx, Some(BAR_H));
    chrome::sync(&ctx, chrome::Bar { height: Some(BAR_H) });
}

/// The bar is the window's title bar: it moves the window, and a
/// double-click does what the system's title bar would.
fn title_bar(ui: &mut Ui) {
    let rect = ui.max_rect();
    csd::set_bar(ui.ctx(), rect, None);
    let bar = ui.interact(rect, ui.scope_id().with("title-bar"), Sense::click_and_drag());
    if bar.drag_started() {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if bar.double_clicked() {
        chrome::title_double_click(ui.ctx());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use shell::theme::{self, ThemePref};

    /// The root view in a window of `size`, its theme installed before the
    /// first frame that draws text (fonts arrive a frame after they're set).
    fn harness(size: egui::Vec2, os: egui::os::OperatingSystem, theme: egui::Theme) -> Harness<'static, bool> {
        let pref = if theme == egui::Theme::Dark { ThemePref::Dark } else { ThemePref::Light };
        Harness::builder().with_size(size).with_pixels_per_point(2.0).with_theme(theme).with_os(os).build_ui_state(
            move |ui, installed: &mut bool| {
                if !std::mem::replace(installed, true) {
                    theme::install(ui.ctx(), pref);
                    ui.ctx().request_repaint();
                    return;
                }
                root(ui);
                if ui.ctx().os() == egui::os::OperatingSystem::Mac {
                    chrome::stand_in_lights(ui.ctx(), Some(BAR_H));
                }
            },
            false,
        )
    }

    #[test]
    fn the_root_view_draws_the_empty_state_and_then_asks_for_no_frames() {
        let mut h = harness(egui::vec2(960.0, 640.0), egui::os::OperatingSystem::Mac, egui::Theme::Light);
        // Settle, then a few more frames: none of them may ask for another.
        h.run();
        h.run_steps(8);
        h.get_by_label("Rede");
        h.get_by_label("No sources yet.");
        assert!(!h.ctx.has_requested_repaint(), "an empty window keeps asking for repaints");
    }

    #[test]
    fn linux_window_buttons_sit_in_the_title_bar() {
        let mut h = harness(egui::vec2(960.0, 640.0), egui::os::OperatingSystem::Nix, egui::Theme::Dark);
        csd::preview(&h.ctx, true);
        csd::set_window(&h.ctx, true, true);
        h.run();
        let close = h.get_by_label("Close");
        let r = close.rect();
        assert!(r.max.y <= BAR_H, "the close button is in the bar, not below it");
        assert!(r.max.x > 900.0, "and at its right end, as most desktops have it");
    }

    /// Renders the window, offscreen through wgpu, to
    /// `target/snapshots/rede-<theme>.png` (and `rede-linux-<theme>.png`
    /// with the Linux window buttons), to look at:
    ///
    /// ```sh
    /// cargo test -p rede --release snapshots -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "renders PNGs for review; run with --ignored"]
    fn snapshots() {
        let out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
        std::fs::create_dir_all(&out).unwrap();
        let out = out.canonicalize().unwrap();
        for (theme, name) in [(egui::Theme::Light, "light"), (egui::Theme::Dark, "dark")] {
            for (os, scene) in [(egui::os::OperatingSystem::Mac, "rede"), (egui::os::OperatingSystem::Nix, "rede-linux")] {
                let mut h = harness(egui::vec2(960.0, 640.0), os, theme);
                let linux = os == egui::os::OperatingSystem::Nix;
                csd::preview(&h.ctx, linux);
                csd::set_window(&h.ctx, linux, linux);
                h.run_steps(4);
                let path = out.join(format!("{scene}-{name}.png"));
                h.render().unwrap_or_else(|e| panic!("rendering {scene}: {e}")).save(&path).unwrap_or_else(|e| panic!("saving {}: {e}", path.display()));
                println!("  {}", path.display());
            }
        }
    }
}
