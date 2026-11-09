//! The sheets over the window, one at a time, in the frame Ond's dialogs
//! use: for now only the keyboard shortcuts (⌘/), with its title and a × at
//! the top; Esc or a click outside closes it too. Preferences are a page,
//! not a sheet: see `settings_page`.

use super::view_ctx::ViewCtx;
use super::{Action, RedeApp};
use egui::Ui;
use shell::kit;

/// Which sheet is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sheet {
    Shortcuts,
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
    (
        "Navigation",
        &[("Collapse / expand sidebar", &["⌥⌘S"]), ("Settings", &["⌘,"]), ("Keyboard shortcuts", &["⌘/"]), ("Back, or close a sheet or menu", &["esc"])],
    ),
];

impl RedeApp {
    /// The open sheet, if any, over everything else.
    pub(super) fn sheet_ui(&mut self, ctx: &egui::Context) {
        let Some(sheet) = self.sheet else { return };
        let modal = egui::Modal::new(egui::Id::unique("sheet").with(sheet)).frame(kit::sheet_frame(ctx)).show(ctx, |ui| {
            let mut view = self.view();
            let close = match sheet {
                Sheet::Shortcuts => view.shortcuts_sheet(ui),
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
}
