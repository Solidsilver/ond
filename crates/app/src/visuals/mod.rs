//! Experimental visuals, each behind its own switch in Settings › Experimental
//! and all off by default. With every switch off, nothing here runs: no
//! decoding, no audio tap, no extra frames.
//!
//! The ones that move (a live spectrum, the visualizer) ask for frames through
//! [`Live`], and only while they're on screen and something plays; otherwise
//! the app keeps its usual pace (a few frames a second while playing, none
//! when idle).

pub mod backdrop;
pub mod colors;
pub mod waveform;


use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Which experimental visuals are on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Experimental {
    /// Accent, now-playing display and Player tinted from the cover.
    pub art_colors: bool,
    /// A blurred copy of the cover behind the Player and Mini.
    pub backdrop: bool,
    /// The track's waveform as the seek bar.
    pub waveform: bool,
    /// A small live spectrum in the now-playing display and the Player.
    pub spectrum: bool,
    /// The full-window visualizer (⌘T).
    pub visualizer: bool,
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

// Nothing moves yet; the live visuals that use this follow.
#[allow(dead_code)]
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

/// About 60 frames a second, for the visualizer.
#[allow(dead_code)]
pub const SMOOTH: Duration = Duration::from_millis(16);
/// About 30 frames a second, for small meters.
#[allow(dead_code)]
pub const METER: Duration = Duration::from_millis(33);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_is_off_by_default_and_old_settings_read_as_off() {
        assert!(Experimental::default().all_off());
        let read: Experimental = serde_json::from_str("{}").unwrap();
        assert!(read.all_off());
        // A newer file with a switch this build doesn't know still reads.
        let read: Experimental = serde_json::from_str(r#"{"waveform": true, "hologram": true}"#).unwrap();
        assert!(read.waveform && !read.spectrum);
    }

    #[test]
    fn live_frames_only_while_playing_and_only_when_asked() {
        let mut live = Live::default();
        assert_eq!(live.next(true), None, "nothing on screen asked");
        live.want(METER);
        live.want(SMOOTH);
        assert_eq!(live.next(true), Some(SMOOTH), "the most demanding visual sets the pace");
        assert_eq!(live.next(true), None, "a request lasts one frame");
        live.want(SMOOTH);
        assert_eq!(live.next(false), None, "paused: no extra frames");
        assert_eq!(live.next(true), None, "and the request didn't linger");
    }
}
