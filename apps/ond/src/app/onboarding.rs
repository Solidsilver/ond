//! The first-run sheet: where Ond opens without a source to resume, and
//! where signing out leads. It's the kit's [`onboarding`](fn@kit::onboarding)
//! with the kinds of source Ond knows of, of which Navidrome and a folder of
//! music can be added yet. Picking Navidrome shows the login form
//! (`session`); once the server answers, the sheet closes on the library.
//! Picking a folder shows the folder form (`sources`); once its first scan
//! has run (or the user carries on without waiting), the sheet closes on it.
//! Ond holds one source live at a time, so there's no Add another and no
//! step that would only say Done.
//!
//! The login form opened on its own, to sign in again over the library, is
//! drawn in the same sheet, on the form with no way back to the tiles.

use super::{Action, OndApp, toolbar};
use crate::icons::Icon;
use crate::theme::Tokens;
use accounts::{AccountId, SourceColor, SourceState};
use shell::kit::{self, OnboardingEvent, OnboardingStep, SourceKind};

/// The kinds of source the sheet offers, Navidrome first.
pub(super) const KINDS: [SourceKind<'static>; 4] = [
    SourceKind { name: "Navidrome", icon: Icon::Server, blurb: "Your own server, at home or away.", available: true },
    SourceKind { name: "Music folder", icon: Icon::Folder, blurb: "Music on this computer. No server needed.", available: true },
    SourceKind { name: "Jellyfin", icon: Icon::Server, blurb: "A Jellyfin server.", available: false },
    SourceKind { name: "Plex", icon: Icon::Server, blurb: "A Plex server.", available: false },
];
/// Navidrome's place in [`KINDS`]: the login form's kind.
const NAVIDROME: usize = 0;
/// A folder of music's place in [`KINDS`]: the folder form's kind.
pub(super) const FOLDER: usize = 1;

/// Ond signs in to one server at a time until it holds several, so the
/// sheet doesn't offer to add another.
const ADD_ANOTHER: bool = false;

/// The sheet's own state while it's up.
#[derive(Debug, Default)]
pub(super) struct Onboard {
    /// This sheet was opened over a configured library by Add source.
    pub(super) adding: bool,
    pub(super) step: OnboardingStep,
    /// The server signed in to from the sheet: its account and its
    /// address.
    pub(super) added: Option<(AccountId, String)>,
    /// The folder form, kept while the sheet is up.
    pub(super) folders: super::sources::FolderForm,
}

impl OndApp {
    /// The window shows the sign-in screen: the first-run sheet, or the
    /// login form on its own.
    pub(super) fn signing_in(&self) -> bool {
        self.login.is_some() || self.onboarding.is_some()
    }

    /// The sign-in screen, filling the window: the first-run sheet, or the
    /// login form on its own (in the same sheet, without the tiles).
    pub(super) fn login_ui(&mut self, ui: &mut egui::Ui) {
        if !self.signing_in() {
            return;
        }
        let t = Tokens::of(ui);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg)).show(ui, |ui| {
            // The top strip is the title bar here too.
            let strip = egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(ui.max_rect().width(), toolbar::TOOLBAR_H));
            let bar = ui.interact(strip, ui.scope_id().with("login-titlebar"), egui::Sense::click_and_drag());
            if bar.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if bar.double_clicked() {
                crate::chrome::title_double_click(ui.ctx());
            }
            let folder_busy = self.onboarding.as_ref().is_some_and(|o| o.folders.busy);
            let waiting = self.login.as_ref().is_some_and(|f| f.busy) || folder_busy;
            let (step, back) = match &self.onboarding {
                Some(o) if o.step == OnboardingStep::Form(FOLDER) => (o.step, true),
                // The form needs its fields; without them (the server went
                // away before it answered) the tiles lead back to it.
                Some(o) if matches!(o.step, OnboardingStep::Form(_)) && self.login.is_none() => (OnboardingStep::Kinds, false),
                Some(o) => (o.step, true),
                None => (OnboardingStep::Form(NAVIDROME), false),
            };
            let host = self.onboarding.as_ref().and_then(|o| o.added.as_ref()).map(|(id, server)| (*id, super::diagnostics::server_host(server)));
            let added: Vec<kit::SourceInfo<'_>> = host
                .iter()
                .map(|(id, host)| kit::SourceInfo {
                    id: *id,
                    name: host,
                    kind: KINDS[NAVIDROME].name,
                    icon: KINDS[NAVIDROME].icon,
                    color: SourceColor::default(),
                    state: &SourceState::Online,
                    detail: None,
                })
                .collect();
            let sheet = kit::Onboarding {
                welcome: "Where's your music?",
                kinds: &KINDS,
                added: &added,
                step,
                add_another: ADD_ANOTHER,
                back,
                cancel: self.onboarding.as_ref().is_some_and(|o| o.adding),
                waiting,
                form_height: if step == OnboardingStep::Form(FOLDER) { super::sources::FOLDER_FORM_H } else { FORM_H },
            };
            let folder = step == OnboardingStep::Form(FOLDER);
            let drawn = kit::onboarding(ui, &sheet, |ui, _| if folder { self.folder_form(ui) } else { self.login_form(ui) });
            if let Some(event) = drawn {
                self.actions.push(Action::Onboarding(event));
            }
        });
    }

    /// What was done on the sheet.
    pub(super) fn onboarding_event(&mut self, event: OnboardingEvent) {
        if event == OnboardingEvent::AddAnother && self.onboarding.is_none() {
            self.onboarding = Some(Onboard { adding: true, ..Default::default() });
        }
        if event == OnboardingEvent::Cancel && !self.multi.bridges.is_empty() {
            self.onboarding = None;
            self.login = None;
            return;
        }
        let Some(sheet) = self.onboarding.as_mut() else { return };
        match event {
            OnboardingEvent::Picked(FOLDER) => sheet.step = OnboardingStep::Form(FOLDER),
            OnboardingEvent::Picked(kind) if KINDS.get(kind).is_some_and(|k| k.available) => {
                sheet.step = OnboardingStep::Form(kind);
                // The form is kept while the sheet is up; this is for one
                // the server took away (going unreachable closes it).
                if self.login.is_none() {
                    let saved = self.prefs.snapshot();
                    self.login =
                        Some(super::session::LoginForm { server: saved.server().to_owned(), username: saved.username().to_owned(), ..Default::default() });
                }
            }
            OnboardingEvent::Picked(_) => {}
            // Not while the form waits on the server: the sheet doesn't
            // offer it then.
            OnboardingEvent::Back if self.login.as_ref().is_some_and(|f| f.busy) || sheet.folders.busy => {}
            OnboardingEvent::Back | OnboardingEvent::AddAnother => sheet.step = OnboardingStep::Kinds,
            OnboardingEvent::Done | OnboardingEvent::Open(_) if sheet.added.is_some() => self.onboarding = None,
            // Ond's sheet is never over a server in view, so it offers no
            // Cancel.
            OnboardingEvent::Done | OnboardingEvent::Open(_) | OnboardingEvent::Cancel => {}
        }
    }

    /// A source connected: the sheet, if it's up, moves on as the kit says,
    /// which without Add another is to close on the library.
    pub(super) fn source_added(&mut self, server: &str, account: AccountId) {
        let Some(sheet) = self.onboarding.as_mut() else { return };
        // A folder just added stays on its form while its first scan runs.
        if sheet.folders.busy && sheet.folders.account == Some(account) {
            return;
        }
        match OnboardingStep::after_adding(ADD_ANOTHER) {
            Some(step) => {
                sheet.added = Some((account, server.to_owned()));
                sheet.step = step;
            }
            None => self.onboarding = None,
        }
    }
}

/// Roughly how tall the login form is.
const FORM_H: f32 = 250.0;
