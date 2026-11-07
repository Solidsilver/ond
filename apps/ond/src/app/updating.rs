//! Newer versions of Ond: the update checker's answers, what Settings ›
//! About shows of them, and the actions about them ([`UpdateAction`]).

use super::{Msg, OndApp};

/// About a newer Ond.
pub enum UpdateAction {
    /// Its release page, in the browser.
    Open(crate::updates::Version),
    /// Don't mention it again.
    Skip(crate::updates::Version),
    /// Look for new versions, or don't.
    Check(crate::settings::UpdateCheck),
    /// Look now (Settings › About).
    CheckNow,
    /// Fetch a release's notes, for Settings › About.
    Notes(crate::updates::Version),
}

/// What Ond knows about newer versions, for Settings › About.
#[derive(Default)]
pub struct Updates {
    /// The newest release, from the last look that worked.
    pub latest: Option<crate::updates::Version>,
    /// Check Now is waiting for an answer.
    pub checking: bool,
    /// When the last look worked.
    pub checked: Option<std::time::SystemTime>,
    /// Why the last look didn't work, if it didn't.
    pub error: Option<String>,
    /// A release's notes: `None` while they're on their way.
    pub notes: Option<(crate::updates::Version, Option<crate::updates::Notes>)>,
}

impl OndApp {
    pub(super) fn update_action(&mut self, action: UpdateAction) {
        match action {
            UpdateAction::Open(v) => self.ctx.open_url(egui::OpenUrl::new_tab(v.page())),
            UpdateAction::Skip(v) => {
                self.prefs.edit(|s| s.skipped_update = Some(v.to_string()));
                self.update = None;
            }
            UpdateAction::Check(check) => {
                self.prefs.edit(|s| s.updates = check);
                let on = self.prefs.get(|s| s.updates) != crate::settings::UpdateCheck::Off;
                self.update_check.store(on, std::sync::atomic::Ordering::Relaxed);
                if !on {
                    self.update = None;
                }
            }
            UpdateAction::CheckNow => {
                self.updates.checking = true;
                self.update_now.notify_one();
            }
            UpdateAction::Notes(v) => {
                if self.updates.notes.as_ref().is_none_or(|(n, _)| *n != v) {
                    self.updates.notes = Some((v, None));
                    let bridge = self.bridge.clone();
                    crate::updates::fetch_notes(self.bridge.runtime(), v, move |notes| bridge.post(Msg::ReleaseNotes(v, notes)));
                }
            }
        }
    }

    /// The checker's answer: a newer release shows in the sidebar unless it
    /// was skipped (Settings › About shows it either way).
    pub(super) fn update_checked(&mut self, result: Result<crate::updates::Version, String>) {
        self.updates.checking = false;
        match result {
            Ok(v) => {
                self.updates.latest = Some(v);
                self.updates.checked = Some(std::time::SystemTime::now());
                self.updates.error = None;
                let newer = v > crate::updates::Version::current();
                let skipped = self.prefs.get(|s| s.skipped_update.as_deref() == Some(&v.to_string()));
                if newer && !skipped {
                    self.update = Some(v);
                } else if !newer {
                    self.update = None;
                }
            }
            Err(e) => self.updates.error = Some(e),
        }
    }
}
