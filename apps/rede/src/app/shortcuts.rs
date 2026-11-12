//! Window shortcuts remain available without a loaded book or keyboard focus;
//! ⌘B is here too, since it works in every mode, and needs only a book. So is
//! search: ⌘K and ⌘F anywhere, and / when it wouldn't be typed into a field.

use super::{Action, RedeApp};
use crate::window::Mode;
use egui::{Key, Modifiers};

impl RedeApp {
    pub(super) fn window_keys(&mut self, ctx: &egui::Context) {
        let typing = ctx.egui_wants_keyboard_input();
        let mode = ctx.input_mut(|i| {
            let shift = Modifiers::COMMAND | Modifiers::SHIFT;
            let alt = Modifiers::COMMAND | Modifiers::ALT;
            if i.consume_key(Modifiers::COMMAND, Key::Slash) {
                self.actions.push(Action::Shortcuts);
            }
            let slash = !typing && self.ui.palette.is_none() && self.sheet.is_none() && i.consume_key(Modifiers::NONE, Key::Slash);
            if slash {
                // Not typed into the field the palette is about to focus.
                i.events.retain(|e| !matches!(e, egui::Event::Text(t) if t == "/"));
            }
            if i.consume_key(Modifiers::COMMAND, Key::K) | i.consume_key(Modifiers::COMMAND, Key::F) | slash {
                self.actions.push(Action::OpenPalette);
            }
            if let Some(step) = shell::zoom::take(i) {
                let now = self.prefs.get(|s| s.zoom);
                self.actions.push(Action::Prefs(super::PrefsAction::Zoom(step.from(now))));
            }
            // Back and forward through the pages, as in Ond; only the full
            // window has pages.
            if self.mode == Mode::Full && self.ui.palette.is_none() && self.sheet.is_none() {
                if i.consume_key(Modifiers::COMMAND, Key::OpenBracket) {
                    self.actions.push(Action::Back);
                }
                if i.consume_key(Modifiers::COMMAND, Key::CloseBracket) {
                    self.actions.push(Action::Forward);
                }
            }
            // The volume, as Ond's ⌘↑ and ⌘↓: five points a press, and not
            // while a field has the keys. Moving it unmutes.
            let louder = !typing && i.consume_key(Modifiers::COMMAND, Key::ArrowUp);
            let softer = !typing && i.consume_key(Modifiers::COMMAND, Key::ArrowDown);
            if louder || softer {
                let now = self.prefs.get(|s| s.playback.volume);
                self.actions.push(Action::Prefs(super::PrefsAction::Volume(if louder { now + 0.05 } else { now - 0.05 })));
            }
            if i.consume_key(Modifiers::COMMAND, Key::Comma) {
                self.actions.push(Action::OpenSettings(self.ui.settings));
            }
            // A bookmark where the book is, in any mode; with nothing
            // loaded the key does nothing.
            if i.consume_key(Modifiers::COMMAND, Key::B)
                && let Some(add) = super::bookmarks::at_current_position(&self.player)
            {
                self.actions.push(add);
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
