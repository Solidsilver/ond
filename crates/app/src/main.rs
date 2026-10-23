//! Listener: a native Navidrome client.
//!
//! The UI here is a deliberately throwaway spike. It proves the plumbing:
//! login → sync into the local db → browse → play through the audio engine,
//! with the UI thread never blocking.

mod app;
mod bridge;
mod icons;
mod output;
mod search;
mod settings;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,wgpu=warn,naga=warn".into()),
        )
        .init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 720.0]).with_min_inner_size([640.0, 400.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native("Listener", options, Box::new(|cc| Ok(Box::new(app::ListenerApp::new(cc)?))))
}
