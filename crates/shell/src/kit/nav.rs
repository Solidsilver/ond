//! The row above a page's heading: back and forward, and the breadcrumb of
//! the places that led here. The app keeps the history and names its places;
//! the row draws them and says what was clicked.

use crate::icons::{self, Icon};
use crate::theme::Tokens;
use egui::{FontId, Sense, Ui, vec2};

/// What was clicked in a [`nav_row`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavClick {
    Back,
    Forward,
    /// An earlier crumb: go back this many steps.
    BackBy(usize),
}

/// Back and forward (each dimmed when there's nowhere to go), then `trail`,
/// the names of the places that led here, oldest first and ending with this
/// one. Every crumb but the last goes back to its place. An empty trail draws
/// the buttons alone. The tooltips name the keys both apps bind, ⌘[ and ⌘].
pub fn nav_row(ui: &mut Ui, can_back: bool, can_forward: bool, trail: &[String]) -> Option<NavClick> {
    let t = Tokens::of(ui);
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let back = ui.add_enabled_ui(can_back, |ui| icons::sized_button(ui, Icon::ChevronLeft, "Back (⌘[)", vec2(26.0, 26.0), 10.0, Some(t.text))).inner;
        if back.clicked() {
            clicked = Some(NavClick::Back);
        }
        let fwd = ui.add_enabled_ui(can_forward, |ui| icons::sized_button(ui, Icon::ChevronRight, "Forward (⌘])", vec2(26.0, 26.0), 10.0, Some(t.text))).inner;
        if fwd.clicked() {
            clicked = Some(NavClick::Forward);
        }
        ui.add_space(8.0);
        ui.spacing_mut().item_spacing.x = 6.0;
        let n = trail.len();
        for (i, label) in trail.iter().enumerate() {
            if i + 1 == n {
                ui.label(egui::RichText::new(label).size(12.0).color(t.text));
            } else {
                if super::link(ui, label, FontId::proportional(12.0), t.muted).clicked() {
                    clicked = Some(NavClick::BackBy(n - 1 - i));
                }
                let (r, _) = ui.allocate_exact_size(vec2(8.0, 12.0), Sense::hover());
                icons::paint(ui.painter(), r.shrink(1.5), Icon::ChevronRight, t.muted);
            }
        }
    });
    ui.add_space(6.0);
    clicked
}
