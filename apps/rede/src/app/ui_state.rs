//! What the window shows between messages: where the trail is, the one line at
//! the top that says what failed, the toasts over the foot, and how far the
//! sync is.
//!
//! Everything here changes only when a message or an action says so, so an idle
//! window draws nothing and asks for nothing.

use super::Nav;
use std::time::{Duration, Instant};

/// How often the library is listed while the app is in use: on start and when
/// the window comes to the front, at most this often.
pub const SYNC_EVERY: Duration = Duration::from_secs(5 * 60);

/// One thing worth reading at the top of the page: what failed, or something
/// the user should know. Plain, one line, dismissible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    /// Said in the error color, and louder than a note.
    pub error: bool,
}

impl Notice {
    pub fn error(text: impl Into<String>) -> Self {
        Self { text: text.into(), error: true }
    }
}

#[derive(Debug, Default)]
pub struct SyncState {
    /// A sync is running (until its outcome arrives, whatever it is).
    pub running: bool,
    /// Books the running sync has written.
    pub applied: usize,
    /// When a sync last started: the clock the [`SYNC_EVERY`] wait runs on.
    pub last: Option<Instant>,
    /// A sync the server asked us to hold off on, or one to retry after the
    /// network failed.
    pub retry_at: Option<Instant>,
    /// When a sync last finished and wrote what changed: the wall clock, so
    /// Settings can say how long ago.
    pub synced: Option<std::time::SystemTime>,
    /// The last sync couldn't reach the server. Cleared when one gets
    /// through.
    pub offline: bool,
}

impl SyncState {
    /// Whether a sync is due by the clock alone: one hasn't run, or the last
    /// one is old enough.
    pub fn due(&self) -> bool {
        self.last.is_none_or(|at| at.elapsed() >= SYNC_EVERY)
    }
}

#[derive(Default)]
pub struct UiState {
    pub nav: Nav,
    /// The line above the page, until it's dismissed or replaced.
    pub notice: Option<Notice>,
    /// The chapters column's tab, and whether it's a sheet.
    pub panel: super::panel::PanelState,
    /// Settings' tab, kept while the page is away.
    pub settings: super::settings_page::Tab,
    /// Confirmations and offers over the foot of the window: a bookmark
    /// made or deleted, a position picked up from another device.
    pub toasts: shell::kit::Toasts<super::Action>,
    /// The other-device notice the toasts are showing, so a new pick-up
    /// replaces it and an undone one takes it away.
    pub reconcile_toast: Option<String>,
}

impl UiState {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sync_is_due_on_the_first_listing_and_then_once_every_five_minutes() {
        let mut sync = SyncState::default();
        assert!(sync.due(), "nothing has been listed yet");
        sync.last = Some(Instant::now());
        assert!(!sync.due(), "just listed");
        sync.last = Some(Instant::now() - SYNC_EVERY - Duration::from_secs(1));
        assert!(sync.due());
    }
}
