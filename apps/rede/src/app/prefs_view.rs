//! The preferences a frame reads: one copy, taken once when the frame starts,
//! so a view never locks [`crate::prefs::Prefs`] while drawing. Changes
//! are asked for as actions and applied after the frame.

use crate::prefs::Prefs;
use crate::settings::LibrarySort;

pub struct PrefsView {
    pub sort: LibrarySort,
    pub playback: crate::settings::PlaybackSettings,
    /// The chapters column is docked at the right (when the window has room).
    pub show_panel: bool,
}

impl PrefsView {
    pub fn read(prefs: &Prefs) -> Self {
        prefs.get(|s| Self { sort: s.sort, playback: s.playback, show_panel: s.show_panel })
    }
}

/// A change to a preference. Each arm is applied by
/// [`RedeApp::apply_prefs`](super::RedeApp).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PrefsAction {
    /// The library's order.
    Sort(LibrarySort),
    Playback(crate::settings::PlaybackSettings),
    /// The chapters column shown or hidden.
    ShowPanel(bool),
}
