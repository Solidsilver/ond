//! What the window shows and how, apart from the library and playback: the
//! place and its selections, sheets and panels, toasts, the colors as they
//! ease, and the window's own bookkeeping.

use super::{Overlay, Palette, PrefsView, WindowState, favorites_page, settings_page, songs, toasts, view_ctx, visualizer_view};
use crate::nav::{Nav, Route};
use crate::theme;
use library::Track;
use std::collections::HashMap;

/// Tracks being dragged (onto a sidebar playlist, or within a playlist to
/// reorder: then `from` says where they were).
#[derive(Debug, Clone)]
pub struct DragTracks {
    pub tracks: Vec<Track>,
    pub from: Option<(String, usize)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueueTab {
    #[default]
    UpNext,
    History,
    /// The playing track's lyrics (the full window's sidebar only).
    Lyrics,
    /// The playing track's credits, album, file and analysis details.
    Info,
}

impl QueueTab {
    pub fn label(self) -> &'static str {
        match self {
            QueueTab::UpNext => "Up next",
            QueueTab::History => "History",
            QueueTab::Lyrics => "Lyrics",
            QueueTab::Info => "Info",
        }
    }
}

/// A song's plays as the server counts them, for the Info tab.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SongStats {
    pub plays: Option<u64>,
    /// RFC 3339, as the server sends it.
    pub last_played: Option<String>,
}

/// Which side keyboard navigation applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    Main,
    Queue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlbumSort {
    #[default]
    Artist,
    Title,
    Year,
    Added,
}

impl AlbumSort {
    pub const ALL: [AlbumSort; 4] = [AlbumSort::Artist, AlbumSort::Title, AlbumSort::Year, AlbumSort::Added];

    pub fn label(self) -> &'static str {
        match self {
            AlbumSort::Artist => "Artist",
            AlbumSort::Title => "Title",
            AlbumSort::Year => "Year",
            AlbumSort::Added => "Recently added",
        }
    }
}

/// What Artists is sorted by: names A to Z, the most albums first, or the
/// earliest first album first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ArtistSort {
    #[default]
    Name,
    Albums,
    Year,
}

impl ArtistSort {
    pub const ALL: [ArtistSort; 3] = [ArtistSort::Name, ArtistSort::Albums, ArtistSort::Year];

    pub fn label(self) -> &'static str {
        match self {
            ArtistSort::Name => "Name",
            ArtistSort::Albums => "Albums",
            ArtistSort::Year => "Year",
        }
    }
}

#[derive(Default)]
pub struct QueueUi {
    pub tab: QueueTab,
    /// Per-group open/closed choices, by [`Source::key`].
    pub open: HashMap<String, bool>,
    pub selected: Option<u64>,
}

/// What the window shows and how: the place and its selections, open sheets
/// and panels, toasts, and the window's own bookkeeping. Nothing here is
/// library data or playback state.
pub struct UiState {
    pub nav: Nav,
    /// How Albums is sorted, and whether that order is turned around (by
    /// clicking its column's header in the list).
    pub sort: AlbumSort,
    pub sort_desc: bool,
    /// The album list for the current view, cached by (route, sort, data version).
    pub grid: crate::memo::Memo<(Route, AlbumSort, bool, u64), Vec<usize>>,
    /// Where each letter starts in that list, for the alphabet rail.
    pub grid_marks: crate::memo::Memo<(Route, AlbumSort, bool, u64), super::alpha::Marks>,
    /// How Artists is sorted, the order that gives (by the artists' list's
    /// address), and where each letter starts in it.
    pub artist_sort: ArtistSort,
    pub artist_desc: bool,
    pub artist_order: crate::memo::Memo<(usize, ArtistSort, bool), Vec<usize>>,
    pub artist_marks: crate::memo::Memo<(usize, bool), super::alpha::Marks>,
    /// The album page's "Album details" is unfolded (for this session).
    pub album_details_open: bool,
    /// Narrow windows: the queue opens as a sheet over the content instead.
    pub queue_sheet: bool,
    /// The window is too narrow to dock the queue column (set each frame).
    pub queue_narrow: bool,
    pub queue_ui: QueueUi,
    pub focus: Focus,
    /// Keyboard selection in the album/playlist track list.
    pub track_sel: Option<usize>,
    /// Where the seek bar is being dragged to, in seconds.
    pub seek_preview: Option<f64>,
    pub songs: songs::SongsView,
    /// Errors and notices, shown as toasts.
    pub toasts: toasts::Toasts,
    pub palette: Option<Palette>,
    /// A sheet over the window, if one is open.
    pub overlay: Option<Overlay>,
    /// ⌘L: select (and scroll to) this track once its album is shown.
    pub reveal: Option<String>,
    /// Playlists added to lately, first in "Add to playlist".
    pub recent_playlists: Vec<String>,
    pub fav_tab: favorites_page::FavTab,
    /// The Settings window's section.
    pub settings_section: settings_page::Section,
    /// Whether the lyrics column follows the sung line.
    pub lyrics_follow: crate::lyrics::Follow,
    /// The Player's details strip is open.
    pub np_details: bool,
    /// The window's own bookkeeping: mode switches under way, Mini held square.
    pub window: WindowState,
    /// When the last scroll happened (frame clock), for showing scroll bars.
    pub last_scroll: f64,
    pub bars_shown: bool,
    /// Edit menu commands (paste, undo…) waiting to reach egui as input.
    pub edit_input: Vec<egui::Event>,
    /// Trackpad coasting where the system doesn't do it (Wayland).
    pub momentum: crate::momentum::Momentum,
    /// Frames asked for by visuals that move (experimental; see `visuals`).
    pub live: crate::visuals::Live,
    /// The colors of the now-playing display, the Player, Mini and the
    /// window around the page: the chosen accent, or the playing cover's.
    pub tint: crate::visuals::colors::Easing,
    /// The page's colors: an album page's own cover's, else the same as [`Self::tint`].
    pub page_tint: crate::visuals::colors::Easing,
    /// [`Self::page_tint`] as drawn this frame.
    pub page_tints: Option<crate::visuals::colors::Tints>,
    /// Each cover's tints for the chosen accent.
    pub tint_cache: crate::visuals::colors::TintCache,
    /// The accent egui's own widgets were last given.
    pub accent_applied: Option<theme::Accent>,
    /// The blurred cover behind the Player and Mini, fading between covers (experimental).
    pub backdrop: crate::visuals::backdrop::Crossfade,
    /// The playing audio's spectrum, for the live visuals (experimental).
    pub listener: crate::visuals::spectrum::Listener,
    /// The Info tab's spectrogram (experimental).
    pub spectrogram: crate::visuals::spectrogram::Spectrogram,
    /// The visualizer, while it's open (experimental).
    pub visualizer: Option<visualizer_view::VisualizerView>,
    /// What views work out from the library, kept between frames.
    pub memos: view_ctx::Memos,
    /// The preferences as of this frame (see [`PrefsView`]).
    pub prefs: PrefsView,
}

impl UiState {
    pub(super) fn new(notices: Vec<String>, prefs: PrefsView) -> Self {
        Self {
            prefs,
            nav: Nav::default(),
            sort: AlbumSort::default(),
            sort_desc: false,
            grid: Default::default(),
            grid_marks: Default::default(),
            artist_sort: ArtistSort::default(),
            artist_desc: false,
            artist_order: Default::default(),
            artist_marks: Default::default(),
            album_details_open: false,
            queue_sheet: false,
            queue_narrow: false,
            queue_ui: QueueUi::default(),
            focus: Focus::default(),
            track_sel: None,
            seek_preview: None,
            songs: songs::SongsView::default(),
            memos: Default::default(),
            // Something worth knowing at start-up (e.g. the library was rebuilt).
            toasts: toasts::Toasts::with_notices(notices),
            palette: None,
            overlay: None,
            reveal: None,
            recent_playlists: Vec::new(),
            fav_tab: favorites_page::FavTab::default(),
            settings_section: settings_page::Section::default(),
            lyrics_follow: crate::lyrics::Follow::default(),
            np_details: false,
            window: WindowState::default(),
            last_scroll: f64::NEG_INFINITY,
            bars_shown: false,
            edit_input: Vec::new(),
            momentum: Default::default(),
            live: Default::default(),
            tint: Default::default(),
            page_tint: Default::default(),
            page_tints: None,
            tint_cache: Default::default(),
            accent_applied: None,
            backdrop: Default::default(),
            listener: Default::default(),
            visualizer: None,
            spectrogram: Default::default(),
        }
    }
}
