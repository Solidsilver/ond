//! Rede: a native audiobook player for Audiobookshelf and audiobook folders.
//!
//! For now an empty window in the family's look, built from `shell`; the
//! design and the plan to fill it are in `docs/rede/design.md`.

// A release build on Windows is a GUI app: no console window behind it.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod view;

use shell::window::Plan;

/// The window's size and smallest size, in points.
const SIZE: [f32; 2] = [1100.0, 720.0];
const MIN_SIZE: [f32; 2] = [640.0, 420.0];

fn main() -> eframe::Result {
    let plan = window();
    let options = eframe::NativeOptions {
        // The app id names the window for Wayland and X11 desktops.
        viewport: plan.builder(egui::ViewportBuilder::default().with_title("Rede").with_icon(icon()).with_app_id("dev.solidsilver.rede")),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "Rede",
        options,
        Box::new(move |cc| {
            shell::theme::install(&cc.egui_ctx, shell::theme::ThemePref::System);
            // The window opened from this plan; our title bar follows it.
            plan.apply_frame(&cc.egui_ctx);
            Ok(Box::new(Rede))
        }),
    )
}

/// One window, one mode, until there's a player to make smaller.
fn window() -> Plan<()> {
    Plan::framed((), SIZE, MIN_SIZE)
}

struct Rede;

impl eframe::App for Rede {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        view::root(ui);
    }
}

/// A placeholder until Rede has its own artwork: a ring in the dark theme's
/// accent on its sidebar color, drawn here so no image ships with it.
fn icon() -> egui::IconData {
    use shell::theme::Tokens;
    const SIDE: usize = 256;
    let t = Tokens::DARK;
    let (bg, ring) = (t.sidebar, t.accent);
    let c = SIDE as f32 / 2.0;
    let mut rgba = Vec::with_capacity(SIDE * SIDE * 4);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let (dx, dy) = (x as f32 + 0.5 - c, y as f32 + 0.5 - c);
            // A rounded square: inside when the corner distance is within the radius.
            let (qx, qy) = ((dx.abs() - (c - 56.0)).max(0.0), (dy.abs() - (c - 56.0)).max(0.0));
            let inside = (qx * qx + qy * qy).sqrt() <= 48.0;
            let r = (dx * dx + dy * dy).sqrt();
            let color = if !inside {
                egui::Color32::TRANSPARENT
            } else if (r - 56.0).abs() <= 12.0 {
                ring
            } else {
                bg
            };
            rgba.extend_from_slice(&color.to_array());
        }
    }
    egui::IconData { rgba, width: SIDE as u32, height: SIDE as u32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_placeholder_icon_is_a_square_with_clear_corners() {
        let icon = icon();
        assert_eq!((icon.width, icon.height, icon.rgba.len()), (256, 256, 256 * 256 * 4));
        assert_eq!(icon.rgba[3], 0, "the corner is transparent");
        let mid = (128 * 256 + 128) * 4;
        assert_eq!(icon.rgba[mid + 3], 255, "the middle is opaque");
    }
}
