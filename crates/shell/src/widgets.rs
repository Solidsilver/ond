//! Widgets every app in the family draws the same way.

use crate::icons::Icon;
use crate::theme::{Tokens, semibold};
use egui::{FontId, Sense, Ui, vec2};

/// The one empty-state layout: a muted glyph, a short title, a line or two
/// of how to fill it (none when `body` is empty, as while loading).
/// Centred, with the same spacing everywhere.
pub fn empty_state(ui: &mut Ui, icon: Icon, title: &str, body: &str) {
    let t = Tokens::of(ui);
    ui.add_space(48.0);
    ui.vertical_centered(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
        crate::icons::paint(ui.painter(), r, icon, t.muted);
        ui.add_space(12.0);
        ui.label(egui::RichText::new(title).font(FontId::new(15.0, semibold())).color(t.text));
        if body.is_empty() {
            return;
        }
        ui.add_space(6.0);
        ui.scope(|ui| {
            // Never wider than the column it sits in: a centred layout claims
            // the whole frame, and a panel's scroll area would grow to fit it.
            ui.set_max_width(440.0_f32.min(ui.available_width()));
            ui.add(egui::Label::new(egui::RichText::new(crate::keys::shown(body)).size(13.0).color(t.muted)).wrap());
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn an_empty_state_shows_its_title_and_its_body_when_it_has_one() {
        // The theme's fonts arrive a frame after they're installed.
        let mut h = egui_kittest::Harness::builder().build_ui_state(
            |ui, installed: &mut bool| {
                if !std::mem::replace(installed, true) {
                    crate::theme::install(ui.ctx(), crate::theme::ThemePref::Light);
                    ui.ctx().request_repaint();
                    return;
                }
                empty_state(ui, Icon::Download, "Nothing downloaded", "");
                empty_state(ui, Icon::Search, "No results", "Try fewer words.");
            },
            false,
        );
        h.run();
        h.get_by_label("Nothing downloaded");
        h.get_by_label("No results");
        h.get_by_label("Try fewer words.");
    }
}
