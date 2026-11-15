//! What Settings' Sources tab does to an account: rename it, give it
//! another colour, sign out of one that isn't in view, and remove one.
//!
//! Signing out of the account in view goes the usual way (`session`); one
//! that isn't in view has nothing running, so its saved sign-in is
//! forgotten and its row says it's signed out. Removing an account deletes
//! its shelf and its downloads (through the bridge, once its listening has
//! flushed and its downloads have closed) and forgets its sign-in. Removing
//! the one in view ends its session first, so nothing it had in flight
//! lands on the next; Rede then opens the next account, or the first-run
//! sheet when none is left. Cover art and streamed audio are shared caches
//! that evict what's no longer asked for, so they're left to do that.

use super::{Action, RedeApp, sheets::Sheet};
use accounts::{AccountId, SourceColor};
use shell::kit::OnboardingStep;

/// A change to one account, asked for on the Sources tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceAction {
    Rename(AccountId, String),
    Color(AccountId, SourceColor),
    /// Ask before signing out of an account that isn't in view.
    AskSignOut(AccountId),
    /// Sign out of an account that isn't in view.
    SignOut(AccountId),
    /// Ask before removing an account, saying what goes with it.
    AskRemove(AccountId),
    Remove(AccountId),
}

impl From<SourceAction> for Action {
    fn from(a: SourceAction) -> Self {
        Action::Source(a)
    }
}

impl RedeApp {
    pub(super) fn source_action(&mut self, action: SourceAction) {
        match action {
            SourceAction::Rename(id, name) => {
                self.prefs.edit(|s| s.rename_account(id, &name));
                self.refresh_account(id);
            }
            SourceAction::Color(id, color) => {
                self.prefs.edit(|s| s.set_color(id, color));
                self.refresh_account(id);
            }
            SourceAction::AskSignOut(id) => self.sheet = Some(Sheet::SignOutOf(id)),
            SourceAction::SignOut(id) => {
                self.sheet = Some(Sheet::Settings);
                self.sign_out_of(id);
            }
            SourceAction::AskRemove(id) => self.sheet = Some(Sheet::RemoveSource(id)),
            SourceAction::Remove(id) => {
                self.sheet = Some(Sheet::Settings);
                self.remove_account(id);
            }
        }
    }

    /// This frame's copy of the settings, and the session's copy of the
    /// account in view, after `id` changed.
    fn refresh_account(&mut self, id: AccountId) {
        if let Some(saved) = self.prefs.get(|s| s.account_by_id(id).cloned())
            && let Some(live) = self.session.account.as_mut().filter(|a| a.id == id)
        {
            live.name = saved.name;
            live.color = saved.color;
        }
        self.shown = super::PrefsView::read(&self.prefs);
        self.ctx.request_repaint();
    }

    /// The account in view, whether it's signed in yet or still connecting.
    fn in_view(&self) -> Option<AccountId> {
        self.session.account.as_ref().map(|a| a.id).or_else(|| self.prefs.get(|s| s.account().map(|a| a.id)))
    }

    /// Signs out of `id`: the usual way for the account in view, else by
    /// forgetting its tokens and saying so on its row.
    fn sign_out_of(&mut self, id: AccountId) {
        if self.in_view() == Some(id) {
            self.sheet = None;
            return self.sign_out();
        }
        let Some(account) = self.prefs.get(|s| s.account_by_id(id).cloned()) else { return };
        self.bridge.forget_tokens(account);
        self.prefs.edit(|s| s.set_signed_out(id, true));
        self.shown = super::PrefsView::read(&self.prefs);
    }

    /// Removes `id` and what this computer keeps of it. The account in view
    /// gives way to the next one, or to the first-run sheet.
    fn remove_account(&mut self, id: AccountId) {
        let Some(account) = self.prefs.get(|s| s.account_by_id(id).cloned()) else { return };
        let in_view = self.in_view() == Some(id);
        if in_view {
            let finished = self.teardown_session();
            self.openid.pending = None;
            self.openid.exchanging = false;
            self.bridge.end_session();
            self.bridge.track_finish(finished);
        }
        self.bridge.purge_account(account);
        self.prefs.edit(|s| {
            s.remove_account(id);
        });
        self.shown = super::PrefsView::read(&self.prefs);
        if !in_view {
            return;
        }
        match self.prefs.get(|s| s.account().map(|a| a.id)) {
            Some(next) => {
                self.switch_account(next);
                // A signed-out account opens on its sign-in form, over
                // everything.
                if self.login.is_some() {
                    self.sheet = None;
                    self.onboarding = Some(OnboardingStep::Kinds);
                }
            }
            None => {
                self.sheet = None;
                self.login = Some(super::LoginForm::default());
                self.onboarding = Some(OnboardingStep::default());
            }
        }
    }
}
