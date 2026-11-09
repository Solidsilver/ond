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
mod listening;
mod memo;
#[cfg(any(target_os = "macos", test))]
mod now_playing;
mod player;
mod prefs;
mod settings;
mod sleep;
mod sync;
mod timeline;
mod tokens;
mod widgets;
mod window;

/// The smallest library window stacks the book cover above its description.
pub(crate) const MIN_SIZE: [f32; 2] = [640.0, 420.0];

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
fn window() -> shell::window::Plan<window::Mode> {
    let settings = settings::Paths::resolve().ok().map(|p| prefs::Prefs::open(&p.config_file).0.snapshot());
    settings.map_or_else(window::Windows::default, |s| s.window).plan()
}

/// The window icon uses the platform's padding, as Ond's does.
fn icon() -> egui::IconData {
    let png: &[u8] = if cfg!(target_os = "macos") {
        include_bytes!("../../../packaging/rede/macos/icon-256.png")
    } else {
        include_bytes!("../../../packaging/rede/linux/hicolor/256x256/apps/dev.solidsilver.rede.png")
    };
    eframe::icon_data::from_png_bytes(png).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_packaged_window_icon_decodes_at_256_pixels() {
        let icon = icon();
        assert_eq!((icon.width, icon.height, icon.rgba.len()), (256, 256, 256 * 256 * 4));
        let packaged = if cfg!(target_os = "macos") {
            include_bytes!("../../../packaging/rede/macos/icon-256.png").as_slice()
        } else {
            include_bytes!("../../../packaging/rede/linux/hicolor/256x256/apps/dev.solidsilver.rede.png").as_slice()
        };
        assert_eq!(icon.rgba, image::load_from_memory(packaged).unwrap().to_rgba8().into_raw());
        let mid = (128 * 256 + 128) * 4;
        assert_eq!(icon.rgba[mid + 3], 255, "the middle is opaque");
    }
}
