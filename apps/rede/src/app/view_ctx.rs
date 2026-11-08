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
    /// Covers load as they're first drawn.
    pub art: &'a mut Art,
    pub actions: &'a mut Vec<Action>,
}

impl RedeApp {
    /// Lends the parts a view works with.
    pub(super) fn view(&mut self) -> ViewCtx<'_> {
        ViewCtx {
            books: &mut self.books,
            session: &self.session,
            sync: &self.sync,
            nav: &self.ui.nav,
            sort: self.shown.sort,
            art: &mut self.art,
            actions: &mut self.actions,
        }
    }
}
