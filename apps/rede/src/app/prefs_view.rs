//! The preferences a frame reads: one copy, taken once when the frame starts,
//! so a view never locks [`crate::prefs::Prefs`] while drawing. Changes
//! are asked for as actions and applied after the frame.

use crate::prefs::Prefs;
use crate::settings::{LibraryFilter, LibrarySort};

pub struct PrefsView {
    pub sort: LibrarySort,
    /// Which books the library shows.
    pub filter: LibraryFilter,
    /// How large the interface is drawn, in percent.
    pub zoom: u16,
    pub playback: crate::settings::PlaybackSettings,
    /// The chapters column is docked at the right (when the window has room).
    pub show_panel: bool,
    /// The sidebar is folded to its rail.
    pub sidebar_collapsed: bool,
}

impl PrefsView {
    pub fn read(prefs: &Prefs) -> Self {
        prefs.get(|s| Self {
            sort: s.sort,
            filter: s.filter,
            zoom: s.zoom,
            playback: s.playback,
            show_panel: s.show_panel,
            sidebar_collapsed: s.sidebar_collapsed,
        })
    }
}

/// A change to a preference. Each arm is applied by
/// [`RedeApp::apply_prefs`](super::RedeApp).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PrefsAction {
    /// The library's order.
    Sort(LibrarySort),
    /// Which books the library shows.
    Filter(LibraryFilter),
    /// The interface's size, in percent.
    Zoom(u16),
    Playback(crate::settings::PlaybackSettings),
    /// Voice boost or skip silence on its own, from a switch, so a switch
    /// never writes back the rest of the playback settings as it last saw
    /// them.
    VoiceBoost(bool),
    SkipSilence(bool),
    /// The chapters column shown or hidden.
    ShowPanel(bool),
    /// The sidebar folded to its rail, or opened out again.
    CollapseSidebar(bool),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_speech_action_updates_the_saved_and_frame_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = crate::app::fixture::app(&ctx, dir.path(), 0);
        app.apply_prefs(PrefsAction::VoiceBoost(true));
        app.apply_prefs(PrefsAction::SkipSilence(true));
        assert!(app.prefs.get(|s| s.playback.voice_boost && s.playback.skip_silence));
        assert!(app.shown.playback.voice_boost && app.shown.playback.skip_silence);
        app.apply_prefs(PrefsAction::VoiceBoost(false));
        assert!(!app.shown.playback.voice_boost && app.shown.playback.skip_silence);
        app.apply_prefs(PrefsAction::SkipSilence(false));
        assert!(!app.prefs.get(|s| s.playback.voice_boost || s.playback.skip_silence));
    }
}
