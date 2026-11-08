//! Rede: a native audiobook player for Audiobookshelf and audiobook folders.
//!
//! The window, its title bar and the app's frame; the plan and the design are
//! in `docs/rede/design.md`.

// A release build on Windows is a GUI app: no console window behind it.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod art;
mod bridge;
mod failure;
mod fmt;
mod memo;
mod prefs;
mod settings;
mod sync;
mod tokens;
mod widgets;

use shell::window::Plan;

/// The library's books need room; the book page has one column of its own.
const SIZE: [f32; 2] = [1100.0, 720.0];
const MIN_SIZE: [f32; 2] = [640.0, 420.0];

fn main() -> eframe::Result {
    init_logging();
    let plan = window();
    let options = eframe::NativeOptions {
        // The app id names the window for Wayland and X11 desktops.
        viewport: plan.builder(egui::ViewportBuilder::default().with_title("Rede").with_icon(icon()).with_app_id("dev.solidsilver.rede")),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native("Rede", options, Box::new(|cc| Ok(Box::new(app::RedeApp::new(cc)?))))
}

/// Logs to stderr from a terminal; launched as an app there is no terminal, so
/// logs go to `rede.log` in the user's log folder, rotated once it passes 5 MB.
fn init_logging() {
    use std::io::IsTerminal;
    let filter = || tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,wgpu=warn,naga=warn".into());
    if !std::io::stderr().is_terminal()
        && let Some(file) = log_file()
    {
        tracing_subscriber::fmt().with_env_filter(filter()).with_ansi(false).with_writer(std::sync::Mutex::new(file)).init();
        return;
    }
    tracing_subscriber::fmt().with_env_filter(filter()).init();
}

fn log_file() -> Option<std::fs::File> {
    const MAX: u64 = 5 * 1024 * 1024;
    let dir = log_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("rede.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX) {
        let _ = std::fs::rename(&path, dir.join("rede.log.1"));
    }
    std::fs::OpenOptions::new().create(true).append(true).open(path).ok()
}

/// `~/Library/Logs/Rede` on macOS, `~/.local/state/rede/logs` on Linux,
/// `%LOCALAPPDATA%\solidsilver\rede\data\logs` on Windows.
fn log_dir() -> Option<std::path::PathBuf> {
    if cfg!(target_os = "macos") {
        return Some(directories::BaseDirs::new()?.home_dir().join("Library/Logs/Rede"));
    }
    let dirs = directories::ProjectDirs::from("dev", "solidsilver", "rede")?;
    Some(dirs.state_dir().unwrap_or(dirs.data_local_dir()).join("logs"))
}

/// One window, one mode, and what a framed window draws its title bar with.
fn window() -> Plan<()> {
    Plan::framed((), SIZE, MIN_SIZE)
}

/// A placeholder until Rede has artwork: a ring in the dark theme's accent on
/// its sidebar color, drawn here so no image ships with it.
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
