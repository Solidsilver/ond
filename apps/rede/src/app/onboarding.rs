//! The first-run sheet: where Rede opens without an account to resume, and
//! where signing out leads. It's the kit's [`onboarding`](fn@kit::onboarding)
//! with Audiobookshelf as its one kind, so it opens on the sign-in form
//! (`session`) with no tiles to pick from. Once signed in, the sheet lists
//! the accounts with Add another and Done, which opens the library on the
//! account just added.
//!
//! The switcher's Add source… opens the same sheet over the account in
//! view, which keeps running underneath: the form, under the accounts
//! there are, with Cancel back to the library as it was. Signing in there
//! goes the usual way, beside the account in view (see `session`), and
//! the new account takes over once it's in.
//!
//! Signing out shows the sheet too, the form filled in for that account,
//! under every account there is. While another is still signed in, picking
//! it or Done switches to it (`session`), so signing out of one account
//! never strands the others. With none signed in it's a first run, the
//! signed-out accounts listed.
//!
//! The form opened on its own, when a saved login stops working or on
//! switching to an account that's signed out, is drawn in the same sheet,
//! with the same list.

use super::{Action, RedeApp};
use accounts::{AccountId, SourceColor, SourceState};
use shell::icons::Icon;
use shell::kit::{self, OnboardingEvent, OnboardingStep, SourceKind};
use shell::theme::Tokens;

/// The kinds of source the sheet offers.
const KINDS: [SourceKind<'static>; 1] =
    [SourceKind { name: "Audiobookshelf", icon: Icon::Server, blurb: "Your own server, at home or away.", available: true }];
/// Audiobookshelf's place in [`KINDS`]: the sign-in form's kind.
const AUDIOBOOKSHELF: usize = 0;

/// Whether the sheet offers to add another account once one is signed in.
const ADD_ANOTHER: bool = true;

impl RedeApp {
    /// The window shows the sign-in screen: the first-run sheet, or the
    /// form on its own.
    pub(super) fn signing_in(&self) -> bool {
        self.login.is_some() || self.onboarding.is_some()
    }

    /// The sign-in screen, filling the window.
    pub(super) fn login_ui(&mut self, ui: &mut egui::Ui) {
        if !self.signing_in() {
            return;
        }
        let t = Tokens::of(ui);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg)).show(ui, |ui| {
            let strip = egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(ui.max_rect().width(), super::BAR_H));
            let bar = ui.interact(strip, ui.scope_id().with("login-titlebar"), egui::Sense::click_and_drag());
            if bar.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if bar.double_clicked() {
                shell::chrome::title_double_click(ui.ctx());
            }
            let waiting = self.login.as_ref().is_some_and(|f| f.busy) || self.openid.pending.is_some();
            let (step, back) = match self.onboarding {
                Some(step) => (step, true),
                None => (OnboardingStep::Form(AUDIOBOOKSHELF), false),
            };
            let form_height = self.login_form_height();
            // A short window keeps the buttons in view, as the sheet does.
            let compact = ui.available_height() < form_height + 220.0;
            // Copied out, so the form can borrow the app while it draws.
            let listed = self.listed_accounts();
            let added: Vec<kit::SourceInfo<'_>> = listed
                .iter()
                .map(|(id, name, color, state)| kit::SourceInfo {
                    id: *id,
                    name,
                    kind: KINDS[AUDIOBOOKSHELF].name,
                    icon: KINDS[AUDIOBOOKSHELF].icon,
                    color: *color,
                    state,
                    detail: None,
                })
                .collect();
            let sheet = kit::Onboarding {
                welcome: "Where are your books?",
                kinds: &KINDS,
                added: &added,
                step,
                add_another: ADD_ANOTHER,
                back,
                cancel: self.onboarding.is_some() && self.session.signed_in(),
                waiting,
                form_height,
            };
            if let Some(event) = kit::onboarding(ui, &sheet, |ui, _| self.login_form(ui, compact)) {
                self.actions.push(Action::Onboarding(event));
            }
        });
        for action in std::mem::take(&mut self.actions) {
            self.apply(action);
        }
    }

    /// The accounts the sheet lists, each signed in or out; the one in view,
    /// if any, is signed in.
    fn listed_accounts(&self) -> Vec<(AccountId, String, SourceColor, SourceState)> {
        let live = self.session.account.as_ref().filter(|_| self.session.signed_in()).map(|a| a.id);
        self.prefs.get(|s| {
            s.accounts
                .iter()
                .map(|a| {
                    let state = if Some(a.id) == live || !a.signed_out { SourceState::Online } else { SourceState::SignedOut };
                    (a.id, a.username.clone(), a.color, state)
                })
                .collect()
        })
    }

    /// Add source…: the sheet over the account in view, on a blank form.
    pub(super) fn add_source(&mut self) {
        if !self.session.signed_in() {
            return;
        }
        self.login = Some(super::LoginForm::default());
        self.onboarding = Some(OnboardingStep::Kinds);
    }

    /// What was done on the sheet.
    pub(super) fn onboarding_event(&mut self, event: OnboardingEvent) {
        // The sheet offers nothing but the form's own Cancel while the form
        // waits.
        let waiting = self.login.as_ref().is_some_and(|f| f.busy) || self.openid.pending.is_some();
        match event {
            OnboardingEvent::Picked(kind) if KINDS.get(kind).is_some_and(|k| k.available) => {
                self.onboarding = Some(OnboardingStep::Form(kind));
                if self.login.is_none() {
                    let account = self.prefs.get(|s| s.account().cloned());
                    self.login = Some(super::LoginForm::for_account(account.as_ref()));
                }
            }
            OnboardingEvent::Picked(_) => {}
            _ if waiting => {}
            OnboardingEvent::Back => self.onboarding = Some(OnboardingStep::Kinds),
            OnboardingEvent::AddAnother => {
                self.onboarding = Some(OnboardingStep::Kinds);
                self.login = Some(super::LoginForm::default());
            }
            OnboardingEvent::Done | OnboardingEvent::Cancel if self.session.signed_in() => self.leave_sheet(),
            // Signed out with another account still signed in: back to it.
            OnboardingEvent::Done => {
                if let Some(id) = self.prefs.get(|s| s.accounts.iter().find(|a| !a.signed_out).map(|a| a.id)) {
                    self.leave_sheet();
                    self.switch_account(id);
                }
            }
            OnboardingEvent::Cancel => {}
            OnboardingEvent::Open(id) => {
                self.leave_sheet();
                self.switch_account(id);
            }
        }
    }

    /// The sheet closes, and the form with it, on whatever's in view.
    fn leave_sheet(&mut self) {
        self.onboarding = None;
        self.login = None;
    }

    /// Signed in: the sheet, if it's up, moves on as the kit says: to the
    /// accounts, with Add another and Done.
    pub(super) fn source_added(&mut self) {
        if self.onboarding.is_some() {
            self.onboarding = OnboardingStep::after_adding(ADD_ANOTHER);
        }
    }
}
