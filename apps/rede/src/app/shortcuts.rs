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
            if i.consume_key(Modifiers::NONE, Key::Escape) && self.shortcuts {
                self.actions.push(Action::Shortcuts);
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
            } else {
                None
            }
        });
        if let Some(mode) = mode {
            self.actions.push(Action::Mode(mode));
        }
    }

    pub(super) fn shortcuts_ui(&mut self, ctx: &egui::Context) {
        if !self.shortcuts {
            return;
        }
        egui::Window::new("Keyboard shortcuts").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO).show(ctx, |ui| {
            egui::Grid::new("shortcuts").spacing([28.0, 8.0]).show(ui, |ui| {
                for (keys, label) in [
                    ("Space", "Play / pause"),
                    ("← / →", "Skip back / forward"),
                    ("⇧← / ⇧→", "Previous / next chapter"),
                    ("[ / ]", "Speed down / up"),
                    ("⇧⌘N", "Switch to Player and back"),
                    ("⇧⌘M", "Switch to Mini and back"),
                    ("⌥⌘1 / 2 / 3", "Full / Player / Mini"),
                    ("⌘/", "Keyboard shortcuts"),
                ] {
                    ui.label(shell::keys::shown(keys));
                    ui.label(label);
                    ui.end_row();
                }
            });
            ui.separator();
            if ui.button("Close shortcuts").clicked() {
                self.actions.push(Action::Shortcuts);
            }
        });
    }
}
