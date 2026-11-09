//! The Settings page (⌘,): Ond's Settings in the page's place, its tabs down
//! the left and one calm pane beside them. A page rather than a window over
//! the library, so the sidebar and the player stay where they are and Back
//! (or Esc) returns to where you were.
//!
//! Playback holds the preferences that used to be a sheet: the skips, the
//! rewinds, the sleep timer's length, and the two speech switches. The speed
//! isn't here: the one saved is the one playing, and the Playback popover
//! sets it. Account says who's signed in where, and has Sync now and Sign
//! out (the account menu keeps them too, a click from the sidebar). About is
//! Ond's About without the updates, which Rede doesn't look for yet.

use super::view_ctx::ViewCtx;
use super::{Action, PrefsAction};
use egui::{FontId, Rect, Ui, pos2, vec2};
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

/// Below this page width the tab list narrows, so the pane keeps room for a
/// row's name beside its control.
const NARROW_BELOW: f32 = 620.0;
const NAV_NARROW_W: f32 = 140.0;
/// The widest the pane gets: in a wide window a row's control would drift
/// far from its name.
const PANE_MAX_W: f32 = 720.0;

/// Where the source lives, and where problems go.
const REPO: &str = "https://github.com/Solidsilver/ond";

impl ViewCtx<'_> {
    pub(super) fn settings_page(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        if crate::widgets::link(ui, "← Back", FontId::proportional(12.0), t.muted).clicked() {
            self.actions.push(Action::Back);
        }
        ui.add_space(6.0);
        let area = ui.available_rect_before_wrap();
        let nav_w = if area.width() < NARROW_BELOW { NAV_NARROW_W } else { kit::SETTINGS_NAV_W };
        let side = Rect::from_min_size(area.min, vec2(nav_w, area.height()));
        ui.painter().vline(side.right(), side.y_range(), egui::Stroke::new(1.0, t.hairline));
        let labels = Tab::ALL.map(Tab::label);
        let at = Tab::ALL.iter().position(|tab| *tab == self.settings).unwrap_or(0);
        if let Some(i) = kit::settings_nav(ui, side, &labels, at) {
            self.actions.push(Action::OpenSettings(Tab::ALL[i]));
        }
        let pane = Rect::from_min_max(pos2(side.right() + 1.0, area.top()), pos2(area.right().min(side.right() + 1.0 + PANE_MAX_W), area.bottom()));
        let tab = self.settings;
        kit::settings_pane(ui, pane, ("rede-settings", tab), tab.label(), |ui| match tab {
            Tab::Playback => self.playback_settings(ui),
            Tab::Account => self.account_settings(ui),
            Tab::About => about(ui),
        });
        ui.advance_cursor_after_rect(area);
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
            kit::setting(ui, "User name", "", |ui| value(ui, &account.username, t.muted));
            if let Some(method) = account.sign_in {
                kit::setting(ui, "Sign-in method", "", |ui| value(ui, method.label(), t.muted));
            }
            let hint = "Signing out forgets the saved sign-in. Your books and your place in each stay on this computer.";
            kit::setting(ui, "Sign out of this server", hint, |ui| {
                if kit::secondary_danger(ui, "Sign out").clicked() {
                    self.actions.push(Action::SignOut);
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
