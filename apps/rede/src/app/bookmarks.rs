//! State and actions only. The Bookmarks tab (`bookmarks_tab`), ⌘B and the
//! toasts draw from these snapshots; drawing never reads the database or
//! performs a server call.
//!
//! The position another device left behind is offered as a toast with Undo
//! for as long as `reconciliation` holds it: Undo takes it, and so does
//! clicking the toast away.

use super::{Action, RedeApp};
use crate::bridge::BookmarkMsg;
use crate::failure::Failure;
use crate::listening::{Reconciled, Request};
use sources::{Bookmark, ItemKey, Position, ProgressUpdate};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

#[derive(Default)]
pub struct State {
    version: u64,
    received: Option<u64>,
    rows: Vec<Bookmark>,
    sorted: crate::memo::Memo<(u64, ItemKey), Vec<Bookmark>>,
}
impl State {
    pub fn replace(&mut self, snapshot: shelf::BookmarkSnapshot) {
        if self.received.is_some_and(|version| version > snapshot.revision) {
            return;
        }
        self.received = Some(snapshot.revision);
        if self.rows != snapshot.bookmarks {
            self.rows = snapshot.bookmarks;
            self.version = self.version.wrapping_add(1);
        }
    }
    /// A stable snapshot, rebuilt only for a new item or bookmark generation.
    pub fn for_book(&mut self, key: &ItemKey) -> Arc<Vec<Bookmark>> {
        self.sorted.get((self.version, key.clone()), || {
            let mut rows: Vec<_> = self.rows.iter().filter(|b| &b.item == key).cloned().collect();
            rows.sort_by(|a, b| a.position.secs().total_cmp(&b.position.secs()));
            rows
        })
    }

    /// The title of the bookmark at exactly `time` in `item`, if there is one.
    pub fn title(&self, item: &ItemKey, time: f64) -> Option<String> {
        self.rows.iter().find(|b| &b.item == item && b.position.secs().to_bits() == time.to_bits()).map(|b| b.title.clone())
    }
}

/// Both the keyboard shortcut and the panel button use chapter metadata here;
/// only the label is formatted, never the bookmark's identity/time.
pub fn at_current_position(player: &crate::player::Player) -> Option<Action> {
    let book = player.book.as_ref()?;
    let time = player.position;
    let clock = crate::fmt::clock(time);
    let title = book.timeline.chapter(time).map_or_else(|| clock.clone(), |i| format!("{} · {clock}", book.timeline.chapters[i].title));
    Some(Action::AddBookmark { item: book.key.clone(), time, title })
}

impl RedeApp {
    pub(super) fn bookmark_event(&mut self, event: BookmarkMsg) {
        match event {
            BookmarkMsg::Updated(rows) => self.bookmarks.replace(rows),
            BookmarkMsg::Failed(e) => self.show_failure(e),
        }
    }

    pub fn current_bookmarks(&mut self) -> Arc<Vec<Bookmark>> {
        match &self.player.book {
            Some(book) => self.bookmarks.for_book(&book.key),
            None => Arc::new(Vec::new()),
        }
    }

    /// Sends one edit to the listening worker. True when it went; false when
    /// it was refused, and the reason is shown.
    pub(super) fn edit_bookmark(&mut self, item: ItemKey, time: f64, title: String, change: shelf::BookmarkChange) -> bool {
        let title = title.trim().to_owned();
        if !time.is_finite() || time < 0.0 || (change != shelf::BookmarkChange::Delete && title.is_empty()) {
            self.show_failure(Failure::Local("Give the bookmark a title and a valid time.".into()));
            return false;
        }
        if self.session.account.as_ref().is_none_or(|a| a.id != item.account) {
            self.show_failure(Failure::Local("Not connected. Sign in to save bookmarks.".into()));
            return false;
        }
        if self.session.source.as_ref().is_none_or(|s| !s.capabilities().bookmarks) {
            self.show_failure(Failure::Local("This source cannot sync bookmarks.".into()));
            return false;
        }
        if let Some(listening) = &self.listening {
            listening.send(Request::Bookmark { bookmark: Bookmark { item, position: Position::from_secs(time), title, created: SystemTime::now() }, change });
            true
        } else {
            self.show_failure(Failure::Local("Not connected. Sign in to save bookmarks.".into()));
            false
        }
    }

    /// The confirmation for a bookmark just made. In the full window, unless
    /// the Bookmarks tab is already in view, it offers to show it.
    pub(super) fn bookmarked(&mut self, title: &str) {
        let text = format!("Bookmarked {}", title.trim());
        let in_view = self.panel_shown() && self.ui.panel.tab == super::panel::Tab::Bookmarks;
        if self.mode == crate::window::Mode::Full && !in_view {
            self.ui.toasts.note(text, "Show", Action::Panel(super::panel::PanelAction::Show(super::panel::Tab::Bookmarks)));
        } else {
            self.ui.toasts.push(text, false);
        }
        self.ctx.request_repaint();
    }

    /// Keeps the other-device toast in step with `reconciliation`: shown
    /// while it holds a position, replaced when a newer one arrives, gone
    /// once it's undone or dismissed.
    pub(super) fn sync_reconcile_toast(&mut self) {
        let want = self.reconciliation.as_ref().map(Reconciled::message);
        if want == self.ui.reconcile_toast {
            return;
        }
        if let Some(old) = self.ui.reconcile_toast.take() {
            self.ui.toasts.remove(&old);
        }
        if let Some(text) = want {
            self.ui.toasts.offer_or(text.clone(), "Undo", Action::UndoReconcile, Action::DismissReconcile);
            self.ui.reconcile_toast = Some(text);
        }
    }

    pub(super) fn jump_to_bookmark(&mut self, item: ItemKey, time: f64) {
        if self.session.account.as_ref().is_none_or(|a| a.id != item.account) {
            return;
        }
        if self.player.book.as_ref().is_some_and(|b| b.key == item) {
            self.playback_control(crate::player::Control::Seek(time));
        } else {
            self.play_book(item.item, Some(time));
        }
    }

    pub(super) fn send_activity(&self) {
        if let Some(listening) = &self.listening {
            let item = self.player.book.as_ref().filter(|_| self.player.active && !self.player.paused).map(|b| b.key.clone());
            listening.send(Request::Activity { item });
        }
    }
    pub(super) fn pull_listening(&self) {
        self.send_activity();
        if let Some(listening) = &self.listening {
            listening.send(Request::Reconcile);
        }
    }

    pub(super) fn reconciled(&mut self, reconciled: Reconciled) {
        // The state may have changed since the pull started. Never move audio
        // that resumed while the server was answering.
        if self.player.book.as_ref().is_some_and(|b| b.key == reconciled.current.item) {
            if self.player.active && !self.player.paused {
                return;
            }
            self.move_to_reconciled(reconciled.current.position.secs());
        }
        self.reconciliation = Some(reconciled);
    }
    pub(super) fn move_to_reconciled(&mut self, position: f64) {
        if self.player.active
            && let Some(engine) = &self.engine
        {
            // This is the accepted server snapshot, not new local listening.
            self.player.control(crate::player::Control::Seek(position), 0.0, engine, Instant::now(), &mut Vec::new());
        } else {
            self.player.position = position;
        }
    }
    pub(super) fn undo_reconcile(&mut self) {
        let Some(reconciled) = self.reconciliation.take() else { return };
        let Some(listening) = self.listening.clone() else { return };
        let previous = reconciled.previous;
        // A phone clock ahead of ours must not win again on the next pull.
        let at =
            SystemTime::now().max(reconciled.current.at.unwrap_or(SystemTime::UNIX_EPOCH).checked_add(Duration::from_millis(1)).unwrap_or(SystemTime::now()));
        if self.player.book.as_ref().is_some_and(|b| b.key == previous.item) {
            self.move_to_reconciled(previous.position.secs());
        }
        self.positions.insert(previous.item.item.clone(), sources::Progress { at: Some(at), ..previous.clone() });
        listening.send(Request::Undo(ProgressUpdate {
            item: previous.item,
            position: previous.position,
            finished: previous.finished,
            at,
            listened: Duration::ZERO,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sources::Progress;

    fn bookmark(time: f64, title: &str) -> Bookmark {
        Bookmark {
            item: ItemKey::new("2f1a5c3e-8a4b-4f6d-9e0a-1c2b3d4e5f60".parse().unwrap(), "book"),
            position: Position::from_secs(time),
            title: title.into(),
            created: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn a_bookmark_edit_needs_a_title_a_known_book_and_a_signed_in_account() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = super::super::fixture::app(&ctx, dir.path(), 1);
        let item = super::super::fixture::account().id;
        let key = ItemKey::new(item, "book-0");
        let refused = |app: &RedeApp| app.ui.notice.as_ref().map(|n| (n.text.clone(), n.error));
        app.apply(Action::AddBookmark { item: key.clone(), time: 60.0, title: "   ".into() });
        assert_eq!(refused(&app), Some(("Give the bookmark a title and a valid time.".into(), true)));
        app.apply(Action::AddBookmark { item: key.clone(), time: f64::NAN, title: "Here".into() });
        assert_eq!(refused(&app).map(|n| n.0), Some("Give the bookmark a title and a valid time.".into()));
        let other = ItemKey::new("2f1a5c3e-8a4b-4f6d-9e0a-1c2b3d4e5f60".parse().unwrap(), "book-0");
        app.apply(Action::DeleteBookmark { item: other, time: 60.0 });
        assert_eq!(refused(&app).map(|n| n.0), Some("Not connected. Sign in to save bookmarks.".into()));
        // The fixture has no source, so even a good edit has nowhere to go; it
        // is refused rather than kept for later.
        app.apply(Action::AddBookmark { item: key, time: 60.0, title: "Here".into() });
        assert_eq!(refused(&app).map(|n| n.0), Some("This source cannot sync bookmarks.".into()));
    }

    #[test]
    fn a_phone_position_moves_a_paused_book_and_leaves_one_playing_alone() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = super::super::fixture::app(&ctx, dir.path(), 1);
        super::super::fixture::playing(&mut app, true);
        let item = app.player.book.as_ref().unwrap().key.clone();
        let previous = Progress { item: item.clone(), position: Position::from_secs(14_200.0), finished: false, at: Some(SystemTime::UNIX_EPOCH) };
        let current = Progress {
            item: item.clone(),
            position: Position::from_secs(18_600.0),
            finished: false,
            at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(600)),
        };
        let event = || crate::listening::Event::Reconciled(Reconciled { previous: previous.clone(), current: current.clone() });
        app.handle(crate::bridge::Msg::Playback { session: app.bridge.session(), event: event() });
        assert_eq!(app.player.position, 18_600.0, "a paused book follows the phone");
        assert_eq!(app.reconciliation.as_ref().map(|r| r.current.position.secs()), Some(18_600.0), "and the toast has its numbers");
        // Playing here is this device's own place: pulling must not interrupt it.
        super::super::fixture::playing(&mut app, false);
        app.reconciliation = None;
        app.handle(crate::bridge::Msg::Playback { session: app.bridge.session(), event: event() });
        assert_eq!(app.player.position, 14_200.0);
        assert!(app.reconciliation.is_none(), "nothing was picked up, so nothing is offered");
    }

    #[test]
    fn the_current_bookmark_action_names_the_chapter_without_rounding_its_position() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = super::super::fixture::app(&ctx, dir.path(), 1);
        super::super::fixture::playing(&mut app, true);
        app.player.position = 1_234.000_000_000_000_2;
        let Some(Action::AddBookmark { item, time, title }) = at_current_position(&app.player) else {
            panic!("a book is loaded");
        };
        assert_eq!(item, app.player.book.as_ref().unwrap().key);
        assert_eq!(time.to_bits(), app.player.position.to_bits());
        assert!(title.contains("20:34"));
        assert!(title.contains('·'));
        app.bookmark_event(BookmarkMsg::Updated(shelf::BookmarkSnapshot {
            revision: 1,
            bookmarks: vec![Bookmark { item: item.clone(), position: Position::from_secs(time), title, created: SystemTime::UNIX_EPOCH }],
        }));
        assert_eq!(app.current_bookmarks()[0].item, item);
    }

    #[test]
    fn stale_bookmark_results_are_scoped_to_the_signed_in_session() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = super::super::fixture::app(&ctx, dir.path(), 1);
        app.handle(crate::bridge::Msg::Bookmarks {
            session: app.bridge.session() + 1,
            event: BookmarkMsg::Updated(shelf::BookmarkSnapshot { revision: 5, bookmarks: vec![bookmark(1.0, "Old account")] }),
        });
        assert!(app.bookmarks.for_book(&bookmark(1.0, "").item).is_empty());
    }

    #[test]
    fn bookmark_snapshots_are_sorted_once_per_version_and_late_messages_are_ignored() {
        let mut state = State::default();
        let early = bookmark(1.25, "Early");
        let late = bookmark(90.000_000_000_000_01, "Late");
        state.replace(shelf::BookmarkSnapshot { revision: 2, bookmarks: vec![late.clone(), early.clone()] });
        let first = state.for_book(&early.item);
        let again = state.for_book(&early.item);
        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(*first, [early.clone(), late]);
        state.replace(shelf::BookmarkSnapshot { revision: 1, bookmarks: vec![] });
        assert!(Arc::ptr_eq(&first, &state.for_book(&early.item)));
        state.replace(shelf::BookmarkSnapshot { revision: 3, bookmarks: vec![early.clone()] });
        assert_eq!(*state.for_book(&early.item), [early]);
    }
}
