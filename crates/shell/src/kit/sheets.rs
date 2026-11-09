//! Sheets over the window: the [`sheet_frame`] they're drawn in and the
//! [`sheet_header`] with its ×.

use crate::icons::Icon;
use crate::theme::{Tokens, semibold};
use egui::{FontId, Ui, vec2};

/// The frame every sheet over the window is drawn in (a dialog, the
/// keyboard shortcuts): the popup's fill, stroke and shadow, rounder, with
/// room inside.
pub fn sheet_frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::popup(&ctx.global_style()).inner_margin(egui::Margin::same(20)).corner_radius(12)
}

/// A sheet's heading: its title at 15 pt, and a × at the right end that
/// closes it (as Esc does). True when the × was clicked.
pub fn sheet_header(ui: &mut Ui, title: &str) -> bool {
    let t = Tokens::of(ui);
    let mut close = false;
    ui.horizontal(|ui| {
        ui.set_height(24.0);
        ui.label(egui::RichText::new(title).font(FontId::new(15.0, semibold())).color(t.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            close = crate::icons::sized_button(ui, Icon::Close, &crate::keys::shown("Close (esc)"), vec2(24.0, 24.0), 10.0, Some(t.muted)).clicked();
        });
    });
    close
}
