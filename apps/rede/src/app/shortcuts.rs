//! Window shortcuts remain available without a loaded book or keyboard focus.

use super::{Action, RedeApp};
use crate::window::Mode;
use egui::{Key, Modifiers};

impl RedeApp {
    pub(super) fn window_keys(&mut self, ctx: &egui::Context) {
        let mode = ctx.input_mut(|i| {
            let shift = Modifiers::COMMAND | Modifiers::SHIFT;
            let alt = Modifiers::COMMAND | Modifiers::ALT;
            if i.consume_key(Modifiers::COMMAND, Key::Slash) {
                self.actions.push(Action::Shortcuts);
            }
            if i.consume_key(Modifiers::COMMAND, Key::Comma) {
                self.actions.push(Action::OpenSettings(self.ui.settings));
            }
            // Esc is an open sheet's (Settings is one); otherwise it closes
            // whatever else is open (the chapters sheet, a menu).
            if self.sheet.is_some() && i.consume_key(Modifiers::NONE, Key::Escape) {
                self.actions.push(Action::CloseSheet);
            }
            if i.consume_key(shift, Key::N) {
                Some(self.prefs.get(|s| s.window.player()))
            } else if i.consume_key(shift, Key::M) {
                Some(self.prefs.get(|s| s.window.mini()))
            } else if i.consume_key(alt, Key::Num1) {
                Some(Mode::Full)
            } else if i.consume_key(alt, Key::Num2) {
                Some(Mode::Player)
            } else if i.consume_key(alt, Key::Num3) {
                Some(Mode::Mini)
            } else if self.mode == Mode::Full && i.consume_key(alt, Key::S) {
                // The left column's partner to ⌥⌘U for the right one.
                let collapsed = self.prefs.get(|s| s.sidebar_collapsed);
                self.actions.push(Action::Prefs(super::PrefsAction::CollapseSidebar(!collapsed)));
                None
            } else if self.mode == Mode::Full && i.consume_key(alt, Key::U) {
                // Ond's binding for its queue column, which this one replaces.
                self.actions.push(Action::Panel(super::panel::PanelAction::Toggle));
                None
            } else {
                None
            }
        });
        if let Some(mode) = mode {
            self.actions.push(Action::Mode(mode));
        }
    }
}
