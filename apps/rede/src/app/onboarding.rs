//! The first-run sheet: where Rede opens without an account to resume, and
//! where signing out leads. It's the kit's [`onboarding`](fn@kit::onboarding)
//! with Audiobookshelf as its one kind, so it opens on the sign-in form
//! (`session`) with no tiles to pick from. Once signed in, the sheet closes
//! on the library.
//!
//! The form opened on its own, when a saved login stops working, is drawn
//! in the same sheet.

use super::{Action, RedeApp};
use accounts::SourceColor;
use shell::icons::Icon;
use shell::kit::{self, OnboardingEvent, OnboardingStep, SourceKind};
use shell::theme::Tokens;

/// The kinds of source the sheet offers.
const KINDS: [SourceKind<'static>; 1] =
    [SourceKind { name: "Audiobookshelf", icon: Icon::Server, blurb: "Your own server, at home or away.", available: true }];
/// Audiobookshelf's place in [`KINDS`]: the sign-in form's kind.
const AUDIOBOOKSHELF: usize = 0;

/// Whether the sheet offers to add another account once one is signed in.
/// Off while Rede signs in to one account at a time; on once it holds
/// several.
const ADD_ANOTHER: bool = false;

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
            let account = self.session.account.as_ref().filter(|_| self.session.signed_in()).map(|a| (a.id, a.username.clone()));
            let state = accounts::SourceState::Online;
            let added: Vec<kit::SourceInfo<'_>> = account
                .iter()
                .map(|(id, name)| kit::SourceInfo {
                    id: *id,
                    name,
                    kind: KINDS[AUDIOBOOKSHELF].name,
                    icon: KINDS[AUDIOBOOKSHELF].icon,
                    color: SourceColor::default(),
                    state: &state,
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
                cancel: false,
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

    /// What was done on the sheet.
    pub(super) fn onboarding_event(&mut self, event: OnboardingEvent) {
        let Some(step) = self.onboarding.as_mut() else { return };
        match event {
            OnboardingEvent::Picked(kind) if KINDS.get(kind).is_some_and(|k| k.available) => {
                *step = OnboardingStep::Form(kind);
                if self.login.is_none() {
                    let account = self.prefs.get(|s| s.account().cloned());
                    self.login = Some(super::LoginForm::for_account(account.as_ref()));
                }
            }
            OnboardingEvent::Picked(_) => {}
            // Not while the form waits: the sheet doesn't offer it then.
            OnboardingEvent::Back if self.login.as_ref().is_some_and(|f| f.busy) || self.openid.pending.is_some() => {}
            OnboardingEvent::Back | OnboardingEvent::AddAnother => *step = OnboardingStep::Kinds,
            OnboardingEvent::Done | OnboardingEvent::Open(_) if self.session.signed_in() => self.onboarding = None,
            OnboardingEvent::Done | OnboardingEvent::Open(_) | OnboardingEvent::Cancel => {}
        }
    }

    /// Signed in: the sheet, if it's up, moves on as the kit says, which
    /// without Add another is to close on the library.
    pub(super) fn source_added(&mut self) {
        if self.onboarding.is_some() {
            self.onboarding = OnboardingStep::after_adding(ADD_ANOTHER);
        }
    }
}
