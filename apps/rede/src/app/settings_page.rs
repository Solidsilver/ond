//! Settings' tabs (⌘,), in the kit's Settings window over the app, Ond's:
//! its tabs down the grey side and one calm pane beside them. Esc or a click
//! outside closes it (see `sheets`).
//!
//! Appearance has the zoom (⌘+, ⌘−, ⌘0 do the same).
//!
//! Downloads has the storage used and its limit, the account's folder,
//! pausing them, removing a finished book's download, and removing them all
//! (after asking).
//!
//! Playback holds the preferences that used to be a sheet: the skips, the
//! rewinds, the sleep timer's length, and the two speech switches. The speed
//! isn't here: the one saved is the one playing, and the Playback popover
//! sets it. Sources lists every account with the kit's source rows, each
//! with its menu (rename, colour, Sync now for the one in view, Sign out…,
//! Remove…; see `manage`), and Add source…, which opens the first-run
//! sheet as the switcher's does; the switcher's Manage sources… opens it.
//! Under them, the account in view has its own group: its full address,
//! who's signed in and how, and the last sync with Sync now (the account
//! menu keeps Sync now and Sign out… too, a click from the sidebar). Its
//! state is on its row. About is Ond's: the version, then the kit's
//! Updates group (Check for updates, the channel, Check now with how the
//! last look went, or a newer version with Download and Skip, and its
//! release notes), then the links. A copy from a package manager, or one
//! built from source, says so instead.

use super::manage::SourceAction;
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
    Downloads,
    Appearance,
    Sources,
    About,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::Playback, Tab::Downloads, Tab::Appearance, Tab::Sources, Tab::About];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Playback => "Playback",
            Tab::Downloads => "Downloads",
            Tab::Appearance => "Appearance",
            Tab::Sources => "Sources",
            Tab::About => "About",
        }
    }
}

/// The skips' lengths, and the rewinds' (0 is off), in seconds; a new
/// timer's, in minutes.
const SKIPS: [u32; 11] = [5, 10, 15, 20, 30, 45, 60, 90, 120, 180, 300];
const REWINDS: [u32; 10] = [0, 1, 2, 3, 5, 10, 15, 20, 30, 60];
const TIMER: [u32; 10] = [5, 10, 15, 20, 30, 45, 60, 75, 90, 120];
/// The storage limits offered, in GB; past the last, no limit.
const LIMITS_GB: [u64; 9] = [1, 2, 5, 10, 25, 50, 100, 250, 500];
const GB: u64 = 1_000_000_000;

impl ViewCtx<'_> {
    /// What's on `tab`, in the window's pane.
    pub(super) fn settings_tab(&mut self, ui: &mut Ui, tab: Tab) {
        match tab {
            Tab::Playback => self.playback_settings(ui),
            Tab::Downloads => self.download_settings(ui),
            Tab::Appearance => self.appearance_settings(ui),
            Tab::Sources => self.source_settings(ui),
            Tab::About => self.about(ui),
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

    /// What the downloads take and where, and how they behave.
    fn download_settings(&mut self, ui: &mut Ui) {
        let snapshot = self.downloads.snapshot.clone();
        kit::group(ui, "Storage", |ui| {
            let used = super::downloads_page::storage_line(&snapshot);
            // No limit is the step past the largest.
            let mut steps: Vec<u64> = LIMITS_GB.iter().map(|gb| gb * GB).collect();
            steps.push(u64::MAX);
            let mut limit = self.download_settings.limit.unwrap_or(u64::MAX);
            let text = |bytes: u64| if bytes == u64::MAX { "No limit".to_owned() } else { crate::fmt::bytes(bytes) };
            let hint = format!("{used}. Books wait to download once the limit is reached.");
            if kit::setting(ui, "Storage limit", &hint, |ui| kit::stepper(ui, &mut limit, &steps, "storage limit", text)) {
                self.actions.push(Action::Prefs(PrefsAction::DownloadLimit((limit != u64::MAX).then_some(limit))));
            }
            let folder = self.downloads.dir.as_ref().map_or_else(|| "Opens when you sign in".to_owned(), |d| d.display().to_string());
            kit::row_with(ui, kit::Head { label: "Folder", hint: &folder, path: self.downloads.dir.is_some(), ..Default::default() }, |_| ());
        });
        kit::group(ui, "Downloading", |ui| {
            let mut paused = snapshot.paused;
            let hint = "Books on their way wait until you resume.";
            if kit::setting(ui, "Pause downloads", hint, |ui| kit::switch(ui, &mut paused, "Pause downloads")).changed() {
                self.actions.push(Action::PauseDownloads(paused));
            }
            let mut on = self.download_settings.remove_when_finished;
            let hint = "When you finish a book, its download goes. Your place and bookmarks stay.";
            if kit::setting(ui, "Remove downloads when finished", hint, |ui| kit::switch(ui, &mut on, "Remove downloads when finished")).changed() {
                self.actions.push(Action::Prefs(PrefsAction::RemoveDownloadsWhenFinished(on)));
            }
        });
        kit::group(ui, "Remove", |ui| {
            let hint = "Frees the space. Books stay in your library, and can be downloaded again.";
            let any = !snapshot.pins.is_empty() || !self.downloads.preparing.is_empty();
            kit::setting(ui, "Remove all downloads", hint, |ui| {
                if ui.add_enabled_ui(any, |ui| kit::secondary_danger(ui, "Remove all…")).inner.clicked() {
                    self.actions.push(Action::AskRemoveDownloads);
                }
            });
        });
    }

    /// How large Rede is drawn.
    fn appearance_settings(&mut self, ui: &mut Ui) {
        kit::group(ui, "Size", |ui| {
            let mut zoom = self.zoom;
            let hint = "⌘+ and ⌘− also zoom; ⌘0 goes back to 100%.";
            if kit::setting(ui, "Zoom", hint, |ui| kit::stepper(ui, &mut zoom, &shell::zoom::STEPS, "zoom", shell::zoom::label)) {
                self.actions.push(Action::Prefs(PrefsAction::Zoom(zoom)));
            }
        });
    }

    /// Every account, each with its menu, then Add source…, then the
    /// account in view.
    fn source_settings(&mut self, ui: &mut Ui) {
        let can_sync = self.session.source.is_some() && !self.sync.running;
        kit::group(ui, "Sources", |ui| {
            let entries = self.source_entries();
            if entries.is_empty() {
                kit::setting(ui, "Not signed in", "", |_| ());
            }
            for entry in &entries {
                let menu = kit::SourceRowMenu {
                    rename: true,
                    color: true,
                    sync: (entry.live && can_sync).then_some("Sync now"),
                    sign_out: !entry.signed_out,
                    remove: true,
                };
                let id = entry.id;
                let action = match kit::source_row(ui, &kit::SourceRow { source: entry.info(), menu, locked: None }) {
                    Some(kit::SourceRowEvent::Rename(name)) => SourceAction::Rename(id, name).into(),
                    Some(kit::SourceRowEvent::Color(color)) => SourceAction::Color(id, color).into(),
                    Some(kit::SourceRowEvent::Sync) => Action::SyncNow,
                    Some(kit::SourceRowEvent::SignOut) if entry.live => Action::AskSignOut,
                    Some(kit::SourceRowEvent::SignOut) => SourceAction::AskSignOut(id).into(),
                    Some(kit::SourceRowEvent::Remove) => SourceAction::AskRemove(id).into(),
                    None => continue,
                };
                self.actions.push(action);
            }
        });
        // Another account is added beside the one in view, so only once
        // it's in.
        let hint = "Sign in to another Audiobookshelf account. Rede shows one at a time; the switcher at the foot of the sidebar moves between them.";
        kit::setting(ui, "Add a source", hint, |ui| {
            if ui.add_enabled_ui(self.session.signed_in(), |ui| kit::secondary(ui, "Add source…")).inner.clicked() {
                self.actions.push(Action::AddSource);
            }
        });
        ui.add_space(20.0);
        self.account_settings(ui);
    }

    /// The account in view, under its name: its full address, who's signed
    /// in and how, and the last sync with Sync now. Its state is on its
    /// row, and Sign out… in the row's menu.
    fn account_settings(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let Some(account) = self.session.account.clone() else { return };
        let state = self.account_state();
        kit::group(ui, account.display_name(), |ui| {
            kit::setting(ui, "Address", "", |ui| value(ui, &account.server, t.muted));
            let how = account.sign_in.map(|m| format!("With {}.", m.phrase())).unwrap_or_default();
            kit::setting(ui, "Signed in as", &how, |ui| value(ui, &account.username, t.muted));
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
    }
}

impl ViewCtx<'_> {
    /// The app and its version, whether Rede looks for newer ones and on
    /// which channel, a newer release's notes, and where it's made.
    fn about(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let current = crate::updates::current();
        let icon = kit::app_icon(ui.ctx(), "rede", include_bytes!("../../../../packaging/rede/linux/hicolor/256x256/apps/dev.solidsilver.rede.png"));
        kit::about_header(ui, &icon, "Rede", &format!("Version {current}"));
        ui.add_space(20.0);
        let about = kit::AboutUpdates {
            app: "Rede",
            current: &current,
            distribution: crate::updates::channel(),
            updates: self.updates,
            check: self.update_check,
            channel: self.update_channel,
            skipped: self.skipped_update,
            hint: "Once a day, Rede looks on GitHub for a newer version and tells you when there is one.",
            check_locked: None,
            channel_locked: None,
        };
        let asked = kit::about_updates(ui, &about);
        self.actions.extend(asked.into_iter().map(Action::Update));
        kit::group(ui, "Links", |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 18.0;
                for (text, url) in [("Source code", crate::updates::REPO.to_owned()), ("Report a problem", format!("{}/issues", crate::updates::REPO))] {
                    if crate::widgets::link(ui, text, FontId::proportional(13.0), t.accent).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                    }
                }
            });
        });
    }
}

/// A value read out on the right of a row.
fn value(ui: &mut Ui, text: &str, color: egui::Color32) {
    ui.label(egui::RichText::new(text).size(13.0).color(color));
}
