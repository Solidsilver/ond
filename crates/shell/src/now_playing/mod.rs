//! System playback metadata and commands, independent of either app's player.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{install, intervals, publish, set_artwork};

/// A command from Control Center, headphones or media keys.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Remote {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Seek(f64),
    SkipBy(f64),
}

/// The metadata an app publishes, in the same time base as its seek command.
#[derive(Debug, Clone, PartialEq)]
pub struct Info {
    pub track_id: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_secs: Option<f64>,
    pub rate: f64,
    /// One-based chapter number and total count.
    pub chapter: Option<(u64, u64)>,
}

/// Buffering keeps the play button engaged while stopping the system clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Playing,
    Buffering,
    Paused,
    Stopped,
}
