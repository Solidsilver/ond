//! The timer menu reads policy and asks for actions; the engine clock owns time.

use super::{Action, view_ctx::ViewCtx};
use crate::{player::Control, sleep::Choice};
use std::time::Instant;

impl ViewCtx<'_> {
    pub(super) fn sleep_menu(&mut self, ui: &mut egui::Ui) {
        let left = self.player.sleep_remaining(Instant::now());
        let label = left.map_or_else(|| "Sleep timer".into(), |left| format!("Sleep {}", crate::fmt::clock(left.ceil())));
        let response = ui.button(label);
        egui::Popup::menu(&response).show(|ui| {
            ui.label("Sleep timer");
            for minutes in [5, 15, 30, 45, 60, 90, 120] {
                if ui.button(format!("{minutes} minutes")).clicked() {
                    self.actions.push(Action::Playback(Control::Sleep(Some(Choice::Minutes(minutes)))));
                    ui.close();
                }
            }
            if ui.button("End of chapter").clicked() {
                self.actions.push(Action::Playback(Control::Sleep(Some(Choice::Chapter))));
                ui.close();
            }
            let mut prefs = self.playback;
            let name = ui.label("Custom minutes").id;
            if ui.add(egui::DragValue::new(&mut prefs.sleep_minutes).range(5..=120)).labelled_by(name).changed() {
                self.actions.push(Action::Prefs(super::PrefsAction::Playback(prefs)));
            }
            if ui.button(format!("Start {} minute timer", prefs.sleep_minutes)).clicked() {
                self.actions.push(Action::Playback(Control::Sleep(Some(Choice::Minutes(prefs.sleep_minutes)))));
                ui.close();
            }
            if left.is_some() {
                ui.separator();
                if ui.button("Extend 10 minutes").clicked() {
                    self.actions.push(Action::Playback(Control::ExtendSleep));
                    ui.close();
                }
                if ui.button("Cancel sleep timer").clicked() {
                    self.actions.push(Action::Playback(Control::Sleep(None)));
                    ui.close();
                }
            }
        });
    }

    pub(super) fn sleep_notice(&mut self, ui: &mut egui::Ui) {
        if let Some(left) = self.player.sleep_remaining(Instant::now())
            && left <= 60.0
        {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Playback pauses in {}", crate::fmt::clock(left.ceil())));
                if ui.button("Extend 10 minutes").clicked() {
                    self.actions.push(Action::Playback(Control::ExtendSleep));
                }
            });
        }
    }
}
