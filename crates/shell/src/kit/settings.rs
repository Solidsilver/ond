//! A Settings page's parts: the tab list down its side ([`settings_nav`])
//! and the pane beside it that scrolls under the tab's title
//! ([`settings_pane`]). Ond draws them in a window over the library, Rede
//! in the page's place; what frames them (a fill, a hairline, the "esc"
//! hint) is the app's.

use crate::theme::{Tokens, semibold};
use egui::{FontId, Rect, Ui, vec2};

/// The tab list's width.
pub const SETTINGS_NAV_W: f32 = 176.0;
/// The pane's side margins.
const PANE_PAD: f32 = 28.0;

/// "Settings" over a row per section, in `side`, the one at `selected` lit.
/// Returns the one clicked.
pub fn settings_nav(ui: &mut Ui, side: Rect, sections: &[&str], selected: usize) -> Option<usize> {
    let t = Tokens::of(ui);
    let mut nav = ui.new_child(egui::UiBuilder::new().max_rect(side.shrink2(vec2(10.0, 16.0))).layout(egui::Layout::top_down(egui::Align::Min)));
    nav.spacing_mut().item_spacing.y = 2.0;
    nav.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Settings").font(FontId::new(15.0, semibold())).color(t.text));
    });
    nav.add_space(12.0);
    let mut clicked = None;
    for (i, label) in sections.iter().enumerate() {
        if super::sidebar_row(&mut nav, label, i == selected, 0.0, label).clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

/// The pane in `pane`: `title` large, then `body`, scrolling when it's taller
/// than the pane. `id` names the scroll position, so each tab keeps its own.
pub fn settings_pane(ui: &mut Ui, pane: Rect, id: impl std::hash::Hash + std::fmt::Debug, title: &str, body: impl FnOnce(&mut Ui)) {
    let t = Tokens::of(ui);
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(pane).layout(egui::Layout::top_down(egui::Align::Min)));
    egui::ScrollArea::vertical().id_salt(id).auto_shrink(false).show(&mut ui, |ui| {
        ui.add_space(20.0);
        ui.horizontal(|ui| {
            ui.add_space(PANE_PAD);
            ui.vertical(|ui| {
                ui.set_width(pane.width() - PANE_PAD * 2.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(egui::RichText::new(title).font(FontId::new(20.0, semibold())).color(t.text));
                ui.add_space(16.0);
                body(ui);
                ui.add_space(24.0);
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::themed;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn the_tab_list_lights_the_tab_shown_and_says_which_was_clicked() {
        let mut h = themed(0usize, |ui, at: &mut usize| {
            let side = Rect::from_min_size(ui.cursor().min, vec2(SETTINGS_NAV_W, 300.0));
            if let Some(i) = settings_nav(ui, side, &["Playback", "Account", "About"], *at) {
                *at = i;
            }
            let pane = Rect::from_min_size(side.right_top(), vec2(300.0, 300.0));
            settings_pane(ui, pane, "pane", ["Playback", "Account", "About"][*at], |ui| {
                ui.label("body");
            });
        });
        h.run();
        assert_eq!(h.query_all_by_label("Playback").count(), 2, "the tab, and the pane's title");
        h.get_by_label("About").click();
        h.run();
        assert_eq!(h.state().1, 2);
        // The pane's title follows the tab.
        assert_eq!(h.query_all_by_label("About").count(), 2);
        h.get_by_label("body");
    }
}
