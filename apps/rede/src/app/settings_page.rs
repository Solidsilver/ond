//! Settings' tabs (⌘,), in the kit's Settings window over the app, Ond's:
//! its tabs down the grey side and one calm pane beside them. Esc or a click
//! outside closes it (see `sheets`).
//!
//! Playback holds the preferences that used to be a sheet: the skips, the
//! rewinds, the sleep timer's length, and the two speech switches. The speed
//! isn't here: the one saved is the one playing, and the Playback popover
//! sets it. Account says who's signed in where, and has Sync now and Sign
//! out… (the account menu keeps them too, a click from the sidebar). About is
//! Ond's About without the updates, which Rede doesn't look for yet.

use super::view_ctx::ViewCtx;
use super::{Action, PrefsAction};
use egui::{FontId, Ui};
use shell::kit;
use shell::theme::Tokens;

/// Settings' tabs, in the order they're listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Tab {
    #[default]
    Playback,
    Account,
    About,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Playback, Tab::Account, Tab::About];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Playback => "Playback",
            Tab::Account => "Account",
            Tab::About => "About",
        }
    }
}

/// The skips' lengths, and the rewinds' (0 is off), in seconds; a new
/// timer's, in minutes.
const SKIPS: [u32; 11] = [5, 10, 15, 20, 30, 45, 60, 90, 120, 180, 300];
const REWINDS: [u32; 10] = [0, 1, 2, 3, 5, 10, 15, 20, 30, 60];
const TIMER: [u32; 10] = [5, 10, 15, 20, 30, 45, 60, 75, 90, 120];

/// Where the source lives, and where problems go.
const REPO: &str = "https://github.com/Solidsilver/ond";

impl ViewCtx<'_> {
    /// What's on `tab`, in the window's pane.
    pub(super) fn settings_tab(&mut self, ui: &mut Ui, tab: Tab) {
        match tab {
            Tab::Playback => self.playback_settings(ui),
            Tab::Account => self.account_settings(ui),
            Tab::About => about(ui),
        }
    }

    /// The rows the Playback settings sheet had, each a stepper, then voice
    /// boost and skip silence.
    fn playback_settings(&mut self, ui: &mut Ui) {
        let mut prefs = self.playback;
        let seconds = |s: u32| format!("{s} s");
        let or_off = |s: u32| if s == 0 { "Off".to_owned() } else { format!("{s} s") };
        kit::group(ui, "Skipping", |ui| {
            kit::setting(ui, "Skip back", "", |ui| kit::stepper(ui, &mut prefs.skip_back, &SKIPS, "skip back", seconds));
            kit::setting(ui, "Skip forward", "", |ui| kit::stepper(ui, &mut prefs.skip_forward, &SKIPS, "skip forward", seconds));
        });
        kit::group(ui, "Resuming", |ui| {
            let hint = "After a pause of 5 minutes or more, so you hear where you left off.";
            kit::setting(ui, "Rewind after a pause", hint, |ui| kit::stepper(ui, &mut prefs.rewind, &REWINDS, "rewind after a pause", or_off));
            let hint = "When the sleep timer stops playback.";
            kit::setting(ui, "Rewind after sleep", hint, |ui| kit::stepper(ui, &mut prefs.sleep_rewind, &REWINDS, "rewind after sleep", or_off));
        });
        kit::group(ui, "Sleep timer", |ui| {
            let hint = "What Custom starts with; starting a timer remembers its length.";
            kit::setting(ui, "Timer length", hint, |ui| kit::stepper(ui, &mut prefs.sleep_minutes, &TIMER, "timer length", |m| format!("{m} min")));
        });
        if prefs != self.playback {
            self.actions.push(Action::Prefs(PrefsAction::Playback(prefs)));
        }
        kit::group(ui, "Voice", |ui| {
            let mut on = self.playback.voice_boost;
            let hint = "Brings voices forward and evens out loud and quiet passages.";
            if kit::setting(ui, "Voice boost", hint, |ui| kit::switch(ui, &mut on, "Voice boost")).changed() {
                self.actions.push(Action::Prefs(PrefsAction::VoiceBoost(on)));
            }
            let mut on = self.playback.skip_silence;
            let hint = "Shortens long pauses. Your place in the book stays exact.";
            if kit::setting(ui, "Skip silence", hint, |ui| kit::switch(ui, &mut on, "Skip silence")).changed() {
                self.actions.push(Action::Prefs(PrefsAction::SkipSilence(on)));
            }
        });
    }

    /// The server and its state, the last sync, and who's signed in.
    fn account_settings(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let Some(account) = self.session.account.clone() else {
            kit::group(ui, "Server", |ui| kit::setting(ui, "Not signed in", "", |_| ()));
            return;
        };
        let state = self.account_state();
        kit::group(ui, "Server", |ui| {
            kit::setting(ui, "Address", "", |ui| value(ui, &account.server, t.muted));
            let (status, color) = match &state {
                super::sidebar::AccountState::Connected => ("Connected", t.ok),
                super::sidebar::AccountState::Syncing(_) => ("Connected", t.ok),
                super::sidebar::AccountState::Connecting => ("Connecting…", t.muted),
                super::sidebar::AccountState::Offline => ("Can't reach the server", t.warn),
            };
            kit::setting(ui, "Status", "", |ui| value(ui, status, color));
        });
        kit::group(ui, "Library", |ui| {
            let label = match (&state, self.sync.synced) {
                (super::sidebar::AccountState::Syncing(line), _) => line.clone(),
                (_, Some(at)) => format!("Synced {}", crate::fmt::ago(at)),
                (_, None) => "Not synced yet".to_owned(),
            };
            let hint = "Rede looks for new books and changes when it starts and when you come back to it.";
            let can_sync = self.session.source.is_some() && !self.sync.running;
            kit::setting(ui, &label, hint, |ui| {
                if ui.add_enabled_ui(can_sync, |ui| kit::secondary(ui, "Sync now")).inner.clicked() {
                    self.actions.push(Action::SyncNow);
                }
            });
        });
        kit::group(ui, "Signed in", |ui| {
            let how = account.sign_in.map(|m| format!("Signed in with {}. ", m.phrase())).unwrap_or_default();
            let hint = format!("{how}Signing out forgets the saved sign-in. Your books and your place in each stay on this computer.");
            kit::setting(ui, &account.username, &hint, |ui| {
                if kit::secondary_danger(ui, "Sign out…").clicked() {
                    self.actions.push(Action::AskSignOut);
                }
            });
        });
    }
}

/// The app and its version, and where it's made.
fn about(ui: &mut Ui) {
    let t = Tokens::of(ui);
    let icon = kit::app_icon(ui.ctx(), "rede", include_bytes!("../../../../packaging/rede/linux/hicolor/256x256/apps/dev.solidsilver.rede.png"));
    kit::about_header(ui, &icon, "Rede", &format!("Version {}", env!("CARGO_PKG_VERSION")));
    ui.add_space(20.0);
    kit::group(ui, "Links", |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            for (text, url) in [("Source code", REPO.to_owned()), ("Report a problem", format!("{REPO}/issues"))] {
                if crate::widgets::link(ui, text, FontId::proportional(13.0), t.accent).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
            }
        });
    });
}

/// A value read out on the right of a row.
fn value(ui: &mut Ui, text: &str, color: egui::Color32) {
    ui.label(egui::RichText::new(text).size(13.0).color(color));
}
