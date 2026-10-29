//! The preferences a frame draws with, copied out once at its start instead
//! of locking the preferences for each one as it's needed.

use super::{Action, OndApp, layout};
use crate::prefs::Prefs;
use crate::theme::{Accent, ThemePref};
use crate::visuals::Experimental;
use crate::visuals::waveform::Bars;
use crate::window::WindowMode;

/// The preferences the app reads, copied out under one lock. A frame reads
/// them from [`UiState::prefs`](super::UiState::prefs), taken as it starts
/// (in `logic` and `main_ui`): views ask for changes as actions, which apply
/// after the frame, so the copy holds for the whole frame. Code that applies
/// actions reads them as they are now, with [`OndApp::prefs_now`], since an
/// action just applied may have changed them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PrefsView {
    /// Full, Player or Mini.
    pub mode: WindowMode,
    /// The Player's lyrics drawer is open.
    pub lyrics_open: bool,
    /// The system's title bar instead of our own window buttons (Linux).
    pub system_title_bar: bool,
    /// Whether the queue column is open (in windows wide enough to dock it).
    pub show_queue: bool,
    /// Sidebar and queue column widths, as saved (dragged by their inner edges).
    pub columns: layout::Columns,
    pub theme: ThemePref,
    /// The accent color the user chose.
    pub accent: Accent,
    /// Whether colors adapt to the album art.
    pub adapt_colors: bool,
    /// The blurred cover behind the Player and Mini.
    pub backdrop: bool,
    /// The waveform seek bar, and how its bars look.
    pub waveform: bool,
    pub waveform_bars: Bars,
    /// The alphabet rail beside long lists.
    pub alpha_rail: bool,
    /// Artists' photos in the Artists list.
    pub artist_photos: bool,
    /// Grid or list, for the pages of albums and for Artists.
    pub album_view: crate::settings::ViewMode,
    pub artist_view: crate::settings::ViewMode,
    pub scrobble: bool,
    /// The diagnostics bar is shown.
    pub diagnostics_bar: bool,
    /// Which experimental visuals are on.
    pub experimental: Experimental,
    /// Software volume, 0..=1.
    pub volume: f32,
    /// Analyse each track as it plays (it's in the playback cache by then).
    pub analyze_playing: bool,
    /// A library-wide analysis should be running (resumed after a restart).
    pub analysis_resume: bool,
}

impl PrefsView {
    pub fn read(prefs: &Prefs) -> Self {
        prefs.get(|s| Self {
            mode: s.window.mode,
            lyrics_open: s.window.lyrics_open,
            system_title_bar: s.window.system_title_bar,
            show_queue: s.show_queue,
            columns: layout::Columns::restore(s.sidebar_width, s.queue_width),
            theme: s.theme,
            accent: s.accent,
            adapt_colors: s.adapt_colors,
            backdrop: s.backdrop,
            waveform: s.waveform,
            waveform_bars: s.waveform_bars,
            alpha_rail: s.alpha_rail,
            artist_photos: s.artist_photos,
            album_view: s.album_view,
            artist_view: s.artist_view,
            scrobble: s.scrobble,
            diagnostics_bar: s.diagnostics_bar,
            experimental: s.experimental,
            volume: s.volume,
            analyze_playing: s.analyze_playing,
            analysis_resume: s.analysis_resume,
        })
    }
}

/// A change to an appearance or behaviour preference.
pub enum PrefsAction {
    Theme(ThemePref),
    /// The accent color used without (or instead of) the album art's.
    Accent(Accent),
    /// Whether colors adapt to the album art.
    AdaptColors(bool),
    /// The blurred cover behind the Player and Mini.
    Backdrop(bool),
    /// The waveform seek bar, and how its bars look.
    Waveform(bool),
    WaveformBars(Bars),
    /// The alphabet rail beside long lists.
    AlphaRail(bool),
    ArtistPhotos(bool),
    AlbumView(crate::settings::ViewMode),
    ArtistView(crate::settings::ViewMode),
    Scrobble(bool),
    /// Show or hide the diagnostics bar.
    Diagnostics(bool),
    /// Turn experimental visuals on or off.
    Experimental(Experimental),
    /// How the queue behaves.
    Queue(crate::queue::QueuePrefs),
}

impl OndApp {
    /// Whether `action` would change a setting set in managed.json. Settings
    /// shows those controls locked; this stops menus and shortcuts too.
    pub(super) fn locked(&self, action: &Action) -> bool {
        use super::{AnalysisAction, OfflineAction, WindowAction};
        let path = match action {
            Action::Prefs(p) => match p {
                PrefsAction::Theme(_) => "theme",
                PrefsAction::Accent(_) => "accent",
                PrefsAction::AdaptColors(_) => "adapt_colors",
                PrefsAction::Backdrop(_) => "backdrop",
                PrefsAction::Waveform(_) => "waveform",
                PrefsAction::AlphaRail(_) => "alpha_rail",
                PrefsAction::ArtistPhotos(_) => "artist_photos",
                PrefsAction::Scrobble(_) => "scrobble",
                PrefsAction::Diagnostics(_) => "diagnostics_bar",
                // Partly managed groups: the managed parts are put back.
                PrefsAction::WaveformBars(_) | PrefsAction::Experimental(_) | PrefsAction::Queue(_) | PrefsAction::AlbumView(_) | PrefsAction::ArtistView(_) => return false,
            },
            Action::Window(WindowAction::SetSystemTitleBar(_)) => "window.system_title_bar",
            Action::Analysis(AnalysisAction::AnalyzePlaying(_)) => "analyze_playing",
            Action::Offline(OfflineAction::SetLimit(_)) => "offline_limit_gb",
            Action::Offline(OfflineAction::Move) => "offline_dir",
            _ => return false,
        };
        self.prefs.managed(path)
    }

    pub(super) fn apply_prefs(&mut self, action: PrefsAction) {
        match action {
            PrefsAction::Theme(pref) => {
                crate::theme::set_pref(&self.ctx, pref);
                self.prefs.edit(|s| s.theme = pref);
            }
            PrefsAction::Accent(accent) => self.prefs.edit(|s| s.accent = accent),
            PrefsAction::AdaptColors(on) => self.prefs.edit(|s| s.adapt_colors = on),
            PrefsAction::Backdrop(on) => self.prefs.edit(|s| s.backdrop = on),
            PrefsAction::Waveform(on) => self.prefs.edit(|s| s.waveform = on),
            PrefsAction::WaveformBars(bars) => self.prefs.edit(|s| s.waveform_bars = bars),
            PrefsAction::AlphaRail(on) => self.prefs.edit(|s| s.alpha_rail = on),
            PrefsAction::ArtistPhotos(on) => self.prefs.edit(|s| s.artist_photos = on),
            PrefsAction::AlbumView(v) => self.prefs.edit(|s| s.album_view = v),
            PrefsAction::ArtistView(v) => self.prefs.edit(|s| s.artist_view = v),
            PrefsAction::Scrobble(on) => {
                self.now_playing.set_scrobbling(on);
                self.prefs.edit(|s| s.scrobble = on);
            }
            PrefsAction::Diagnostics(on) => self.prefs.edit(|s| s.diagnostics_bar = on),
            PrefsAction::Experimental(x) => self.prefs.edit(|s| s.experimental = x),
            PrefsAction::Queue(q) => {
                self.prefs.edit(|s| s.queue = q);
                // As saved: managed parts may have been put back.
                self.player.queue.prefs = self.prefs.get(|s| s.queue);
            }
        }
    }

    /// The preferences as they are now (not as this frame started).
    pub(super) fn prefs_now(&self) -> PrefsView {
        PrefsView::read(&self.prefs)
    }

    /// Takes this frame's copy of the preferences.
    pub(super) fn read_prefs(&mut self) {
        self.ui.prefs = self.prefs_now();
    }
}
