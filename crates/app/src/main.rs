//! Listener: a native Navidrome client.
//!
//! Login → sync into the local db → browse → play through the audio engine,
//! with the UI thread never blocking.

mod app;
mod art;
mod bridge;
mod icons;
mod nav;
mod now_playing;
mod output;
mod queue;
mod search;
mod settings;
mod theme;
mod window;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,wgpu=warn,naga=warn".into()),
        )
        .init();

    // Reopen in the mode (and place) the app was left in.
    let windows = settings::Paths::resolve().ok().and_then(|p| settings::Settings::load(&p)).map(|s| s.window).unwrap_or_default();
    let options = eframe::NativeOptions {
        viewport: windows.plan().builder(egui::ViewportBuilder::default()),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native("Listener", options, Box::new(|cc| Ok(Box::new(app::ListenerApp::new(cc)?))))
}
