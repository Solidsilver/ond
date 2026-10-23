//! Ond: a native Navidrome client.
//!
//! Login → sync into the local db → browse → play through the audio engine,
//! with the UI thread never blocking.

mod analyze;
mod app;
mod art;
mod bridge;
mod chrome;
mod folder;
mod favorites;
mod fmt;
mod icons;
mod keys;
mod lyrics;
mod memo;
mod menu;
mod nav;
mod now_playing;
mod output;
mod playback;
mod player;
mod prefs;
mod queue;
mod search;
mod settings;
mod sync;
mod theme;
mod tracks;
mod window;

fn main() -> eframe::Result {
    init_logging();

    // Reopen in the mode (and place) the app was left in.
    let windows = settings::Paths::resolve().ok().and_then(|p| settings::Settings::read(&p.config_file)).map(|s| s.window).unwrap_or_default();
    let options = eframe::NativeOptions {
        viewport: windows.plan().builder(egui::ViewportBuilder::default().with_icon(window_icon())),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native("Ond", options, Box::new(|cc| Ok(Box::new(app::OndApp::new(cc)?))))
}

/// Logs to stderr from a terminal; launched as an app (Finder, Dock) there is
/// no terminal, so logs go to `~/Library/Logs/Ond/ond.log`, rotated
/// once it passes 5 MB.
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
    let dir = directories::BaseDirs::new()?.home_dir().join("Library/Logs/Ond");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("ond.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX) {
        let _ = std::fs::rename(&path, dir.join("ond.log.1"));
    }
    std::fs::OpenOptions::new().create(true).append(true).open(path).ok()
}

/// The Dock and window icon when run outside the app bundle (`cargo run`).
fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../../../packaging/macos/icon-256.png")).unwrap_or_default()
}
