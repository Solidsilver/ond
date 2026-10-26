//! Visuals. Some are settled: the app's colors (the chosen accent, or the
//! album art's when colors adapt to it; see [`colors`]), how controls drawn
//! over artwork stay readable ([`over_art`]), and, each behind a switch in
//! Settings › Appearance, the blurred artwork ([`backdrop`]) and the waveform
//! seek bar ([`waveform`]). The rest are experiments, each behind its own
//! switch in Settings › Experimental. Every switch is off by default, and
//! with them off none of those runs: no audio tap, no extra frames.
//!
//! The ones that move (a live spectrum, the visualizer) ask for frames through
//! [`Live`], and only while they're on screen and something plays; otherwise
//! the app keeps its usual pace (a few frames a second while playing, none
//! when idle).

pub mod backdrop;
pub mod colors;
pub mod flow;
pub mod gpu;
pub mod over_art;
pub mod particles;
pub mod pulse;
pub mod ribbons;
pub mod spectrogram;
pub mod spectrum;
pub mod visualizer;
pub mod waveform;


use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Which experimental visuals are on (and which visualizer the visualizer shows).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Experimental {
    /// Read from older settings only: colors from the album art are a real
    /// setting now (`Settings::adapt_colors`, see `Settings::migrate`).
    #[serde(skip_serializing)]
    pub art_colors: bool,
    /// A small live spectrum in the now-playing display and the Player.
    pub spectrum: bool,
    /// The full-window visualizer (⌘T).
    pub visualizer: bool,
    /// Which one it shows, or a different one each track.
    #[serde(deserialize_with = "visualizer::lenient_pick")]
    pub visualizer_pick: visualizer::Pick,
    /// A scrolling spectrogram in the Info tab.
    pub spectrogram: bool,
}

impl Experimental {
    pub fn all_off(&self) -> bool {
        *self == Self::default()
    }
}

/// Frames for visuals that move. Whatever draws one asks with [`Live::want`]
/// during the frame; at the end of it [`Live::next`] says how soon to draw
/// again, and only while something plays (paused or stopped, a still frame
/// costs nothing).
#[derive(Debug, Default)]
pub struct Live {
    every: Option<Duration>,
}

impl Live {
    /// Something on screen moves with the music and wants a frame every `every`.
    pub fn want(&mut self, every: Duration) {
        self.every = Some(self.every.map_or(every, |e| e.min(every)));
    }

    /// Ends the frame: the shortest interval asked for, if `playing`. Clears
    /// the request either way, so a visual that's gone stops costing frames.
    pub fn next(&mut self, playing: bool) -> Option<Duration> {
        self.every.take().filter(|_| playing)
    }
}

/// Every frame the display shows (vsync paces it), for the visualizer.
pub const EVERY_FRAME: Duration = Duration::ZERO;
/// About 30 frames a second, for small meters.
pub const METER: Duration = Duration::from_millis(33);

/// Bars standing on `floor` (the bottom of `area`), one per value (0..=1),
/// for a small spectrum.
pub fn paint_bars(painter: &egui::Painter, area: egui::Rect, values: &[f32], color: egui::Color32) {
    let step = area.width() / values.len().max(1) as f32;
    let half_w = (step * 0.62).max(1.0) / 2.0;
    let mut mesh = egui::Mesh::default();
    for (i, v) in values.iter().enumerate() {
        let cx = area.left() + step * (i as f32 + 0.5);
        let h = area.height() * v.clamp(0.0, 1.0);
        if h < 0.5 {
            continue;
        }
        mesh.add_colored_rect(egui::Rect::from_min_max(egui::pos2(cx - half_w, area.bottom() - h), egui::pos2(cx + half_w, area.bottom())), color);
    }
    painter.add(egui::Shape::mesh(mesh));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_is_off_by_default_and_old_settings_read_as_off() {
        assert!(Experimental::default().all_off());
        let read: Experimental = serde_json::from_str("{}").unwrap();
        assert!(read.all_off());
        // A newer file with a switch this build doesn't know still reads.
        let read: Experimental = serde_json::from_str(r#"{"spectrum": true, "hologram": true}"#).unwrap();
        assert!(read.spectrum && !read.visualizer);
        // The visualizer's choice reads, and one this build doesn't know is the default.
        let read: Experimental = serde_json::from_str(r#"{"visualizer": true, "visualizer_pick": "ribbons"}"#).unwrap();
        assert_eq!(read.visualizer_pick, visualizer::Pick::Ribbons);
        let read: Experimental = serde_json::from_str(r#"{"visualizer_pick": "lasers"}"#).unwrap();
        assert_eq!(read.visualizer_pick, visualizer::Pick::default());
    }

    #[test]
    fn live_frames_only_while_playing_and_only_when_asked() {
        let mut live = Live::default();
        assert_eq!(live.next(true), None, "nothing on screen asked");
        live.want(METER);
        live.want(EVERY_FRAME);
        assert_eq!(live.next(true), Some(EVERY_FRAME), "the most demanding visual sets the pace");
        assert_eq!(live.next(true), None, "a request lasts one frame");
        live.want(EVERY_FRAME);
        assert_eq!(live.next(false), None, "paused: no extra frames");
        assert_eq!(live.next(true), None, "and the request didn't linger");
    }
}
