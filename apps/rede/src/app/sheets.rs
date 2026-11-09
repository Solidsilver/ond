//! The sheets over the window, one at a time, in the frame Ond's dialogs
//! use: the keyboard shortcuts (⌘/) and the playback settings. Each has its
//! title and a × at the top; Esc or a click outside closes it too.
//!
//! The playback settings are a sheet of their own because they're
//! preferences, not controls: how far the skips go, the rewinds after a long
//! pause and after the sleep timer, and a new timer's length. The Playback
//! popover has the speed (and, with M3, voice boost and skip silence); it and
//! the account menu both lead here. When Rede has a Settings window, this
//! sheet's groups become its Playback page.

use super::view_ctx::ViewCtx;
use super::{Action, PrefsAction, RedeApp};
use egui::Ui;
use shell::kit;

/// Which sheet is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sheet {
    Shortcuts,
    PlaybackSettings,
}

/// A group of shortcuts: its name, and each row's description and keys.
type Group = (&'static str, &'static [(&'static str, &'static [&'static str])]);

/// The keyboard shortcuts, grouped, each with the keys that do it. The sheet
/// balances them into two columns: Playback and Chapters, then Window and
/// Navigation.
const SHORTCUTS: [Group; 4] = [
    ("Playback", &[("Play / pause", &["Space"]), ("Skip back", &["←"]), ("Skip forward", &["→"]), ("Slower", &["["]), ("Faster", &["]"])]),
    ("Chapters", &[("Previous chapter", &["⇧←"]), ("Next chapter", &["⇧→"]), ("Show / hide chapters", &["⌥⌘U"])]),
    ("Window", &[("Player, and back", &["⇧⌘N"]), ("Mini, and back", &["⇧⌘M"]), ("Full, Player, Mini", &["⌥⌘1", "⌥⌘2", "⌥⌘3"])]),
    ("Navigation", &[("Collapse / expand sidebar", &["⌥⌘S"]), ("Keyboard shortcuts", &["⌘/"]), ("Close a sheet or menu", &["esc"])]),
];

/// The playback settings sheet's width.
const SETTINGS_W: f32 = 380.0;
/// The skips' lengths, and the rewinds' (0 is off), in seconds; a new
/// timer's, in minutes.
const SKIPS: [u32; 11] = [5, 10, 15, 20, 30, 45, 60, 90, 120, 180, 300];
const REWINDS: [u32; 10] = [0, 1, 2, 3, 5, 10, 15, 20, 30, 60];
const TIMER: [u32; 10] = [5, 10, 15, 20, 30, 45, 60, 75, 90, 120];

impl RedeApp {
    /// The open sheet, if any, over everything else.
    pub(super) fn sheet_ui(&mut self, ctx: &egui::Context) {
        let Some(sheet) = self.sheet else { return };
        let modal = egui::Modal::new(egui::Id::unique("sheet").with(sheet)).frame(kit::sheet_frame(ctx)).show(ctx, |ui| {
            let mut view = self.view();
            let close = match sheet {
                Sheet::Shortcuts => view.shortcuts_sheet(ui),
                Sheet::PlaybackSettings => view.playback_settings(ui),
            };
            if close {
                view.actions.push(Action::CloseSheet);
            }
        });
        if modal.should_close() {
            self.actions.push(Action::CloseSheet);
        }
    }
}

impl ViewCtx<'_> {
    /// The keyboard shortcuts in the kit's sheet, Ond's: two columns, or one
    /// in a narrow window (the Player). True when its × was clicked.
    fn shortcuts_sheet(&mut self, ui: &mut Ui) -> bool {
        let groups: Vec<kit::ShortcutGroup<'_>> = SHORTCUTS.iter().map(|(name, rows)| kit::ShortcutGroup { name, rows: rows.to_vec() }).collect();
        kit::shortcuts_sheet(ui, &groups, 1..=2, true)
    }

    /// The playback settings, in Ond's settings rows: each a name and a hint
    /// at the left, a stepper at the right. True when its × was clicked.
    fn playback_settings(&mut self, ui: &mut Ui) -> bool {
        ui.set_width(SETTINGS_W);
        let close = kit::sheet_header(ui, "Playback settings");
        ui.add_space(14.0);
        ui.spacing_mut().item_spacing.y = 0.0;
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
        close
    }
}
