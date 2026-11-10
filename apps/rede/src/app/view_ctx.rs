//! What a view gets to work with: the books, the session and the sync to read,
//! the artwork cache to draw from, and a list of [`Action`]s to ask for
//! everything else. A view that takes a `ViewCtx` never changes state behind the
//! app's back; the app applies the actions after the frame.

use super::session::Session;
use super::{Action, Books, Nav, RedeApp, SyncState};
use crate::art::Art;
use crate::settings::LibrarySort;

pub struct ViewCtx<'a> {
    pub books: &'a mut Books,
    pub session: &'a Session,
    pub sync: &'a SyncState,
    /// Where the window is, and whether it can go back.
    pub nav: &'a Nav,
    /// The library's order, as this frame's copy of the preferences has it.
    pub sort: LibrarySort,
    pub playback: crate::settings::PlaybackSettings,
    /// The zoom, in percent.
    pub zoom: u16,
    pub opening: bool,
    pub mode: crate::window::Mode,
    pub player: &'a crate::player::Player,
    /// The chapters column's tab, and whether to bring the chapter playing
    /// into view.
    pub panel: super::panel::PanelState,
    /// Settings' tab.
    pub settings: super::settings_page::Tab,
    pub positions: &'a std::collections::HashMap<String, sources::Progress>,
    /// The loaded book's bookmarks, sorted by time.
    pub bookmarks: std::sync::Arc<Vec<sources::Bookmark>>,
    /// Covers load as they're first drawn.
    pub art: &'a mut Art,
    pub actions: &'a mut Vec<Action>,
}

impl RedeApp {
    /// Lends the parts a view works with.
    pub(super) fn view(&mut self) -> ViewCtx<'_> {
        let bookmarks = self.current_bookmarks();
        ViewCtx {
            bookmarks,
            books: &mut self.books,
            session: &self.session,
            sync: &self.sync,
            nav: &self.ui.nav,
            sort: self.shown.sort,
            playback: self.shown.playback,
            zoom: self.shown.zoom,
            opening: self.opening,
            mode: self.mode,
            player: &self.player,
            panel: self.ui.panel,
            settings: self.ui.settings,
            positions: &self.positions,
            art: &mut self.art,
            actions: &mut self.actions,
        }
    }
}
