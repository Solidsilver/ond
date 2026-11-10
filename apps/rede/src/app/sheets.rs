//! The sheets over the window, one at a time, as Ond's: the keyboard
//! shortcuts (⌘/), with its title and a × at the top; Settings (⌘,), the
//! kit's window with Rede's tabs (see `settings_page`); and the question
//! before signing out. Esc or a click outside closes each.

use super::settings_page::Tab;
use super::view_ctx::ViewCtx;
use super::{Action, RedeApp};
use egui::Ui;
use shell::kit;

/// Which sheet is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sheet {
    Shortcuts,
    /// On the tab [`super::UiState`] keeps.
    Settings,
    /// Sign out? Asked from Settings, a No goes back there.
    SignOut {
        from_settings: bool,
    },
}

/// A group of shortcuts: its name, and each row's description and keys.
type Group = (&'static str, &'static [(&'static str, &'static [&'static str])]);

/// The keyboard shortcuts, grouped, each with the keys that do it. The sheet
/// balances them into two columns: Playback and Chapters, then Window and
/// Navigation.
const SHORTCUTS: [Group; 4] = [
    ("Playback", &[("Play / pause", &["Space"]), ("Skip back", &["←"]), ("Skip forward", &["→"]), ("Slower", &["["]), ("Faster", &["]"])]),
    ("Chapters", &[("Previous chapter", &["⇧←"]), ("Next chapter", &["⇧→"]), ("Show / hide chapters", &["⌥⌘U"]), ("Add bookmark", &["⌘B"])]),
    ("Window", &[("Player, and back", &["⇧⌘N"]), ("Mini, and back", &["⇧⌘M"]), ("Full, Player, Mini", &["⌥⌘1", "⌥⌘2", "⌥⌘3"])]),
    (
        "Navigation",
        &[
            ("Search", &["⌘K", "⌘F", "/"]),
            ("Collapse / expand sidebar", &["⌥⌘S"]),
            ("Settings", &["⌘,"]),
            ("Keyboard shortcuts", &["⌘/"]),
            ("Back, or close a sheet or menu", &["esc"]),
        ],
    ),
];

impl RedeApp {
    /// The open sheet, if any, over everything else.
    pub(super) fn sheet_ui(&mut self, ctx: &egui::Context) {
        let Some(sheet) = self.sheet else { return };
        if sheet == Sheet::Settings {
            return self.settings_window(ctx);
        }
        let modal = egui::Modal::new(egui::Id::unique("sheet").with(sheet)).frame(kit::sheet_frame(ctx)).show(ctx, |ui| {
            let mut view = self.view();
            let close = match sheet {
                Sheet::Shortcuts => view.shortcuts_sheet(ui),
                Sheet::Settings => false,
                Sheet::SignOut { .. } => view.sign_out_sheet(ui),
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

impl RedeApp {
    /// Settings in the kit's window, Ond's, on the tab last shown.
    fn settings_window(&mut self, ctx: &egui::Context) {
        let labels = Tab::ALL.map(Tab::label);
        let at = Tab::ALL.iter().position(|tab| *tab == self.ui.settings).unwrap_or(0);
        let window = kit::settings_window(ctx, super::BAR_H, &labels, at, "rede-settings", |ui, i| self.view().settings_tab(ui, Tab::ALL[i]));
        if let Some(i) = window.clicked {
            self.ui.settings = Tab::ALL[i];
        }
        if window.close {
            self.actions.push(Action::CloseSheet);
        }
    }
}

impl ViewCtx<'_> {
    /// Sign out of the server? Its Cancel closes the sheet, as the
    /// caller's × would. False otherwise: the answer is an action.
    fn sign_out_sheet(&mut self, ui: &mut Ui) -> bool {
        let host = self.session.account.as_ref().map(|a| super::sidebar::host(&a.server).to_owned()).unwrap_or_default();
        let title = if host.is_empty() { "Sign out?".to_owned() } else { format!("Sign out of {host}?") };
        let body = "Playback stops and Rede forgets the sign-in. Your books and your place in each stay on this computer.";
        match kit::confirm(ui, &title, body, "Sign out") {
            Some(true) => {
                self.actions.push(Action::SignOut);
                false
            }
            Some(false) => true,
            None => false,
        }
    }

    /// The keyboard shortcuts in the kit's sheet, Ond's: two columns, or one
    /// in a narrow window (the Player). True when its × was clicked.
    fn shortcuts_sheet(&mut self, ui: &mut Ui) -> bool {
        let groups: Vec<kit::ShortcutGroup<'_>> = SHORTCUTS.iter().map(|(name, rows)| kit::ShortcutGroup { name, rows: rows.to_vec() }).collect();
        kit::shortcuts_sheet(ui, &groups, 1..=2, true)
    }
}
