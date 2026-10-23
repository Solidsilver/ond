//! Small shared pieces: elided text, quiet links, the accent button, a
//! segmented control.

use crate::theme::{Tokens, semibold};
use egui::text::{LayoutJob, TextWrapping};
use egui::{Align2, Color32, FontId, Galley, Rect, Response, Sense, Ui, vec2};
use std::sync::Arc;

/// One line of text cut to `width` with an ellipsis.
pub fn elide(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = TextWrapping { max_width: width.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    ui.painter().layout_job(job)
}

/// Text that reads as plain until hovered, then underlines: links where they
/// earn it, without turning the page blue.
pub fn link(ui: &mut Ui, text: &str, font: FontId, color: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let (rect, resp) = ui.allocate_exact_size(galley.size(), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        ui.painter().galley(rect.min, galley, color);
        if hovered {
            let y = rect.bottom() - 1.0;
            ui.painter().line_segment([egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)], egui::Stroke::new(1.0, color));
        }
    }
    let label = text.to_owned();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, &label));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The filled accent button (Play).
pub fn primary(ui: &mut Ui, icon: Option<crate::icons::Icon>, text: &str) -> Response {
    let t = Tokens::of(ui);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(13.0, semibold()), t.on_accent);
    let icon_w = if icon.is_some() { 16.0 } else { 0.0 };
    let size = vec2(galley.size().x + icon_w + 32.0, 30.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if resp.hovered() { t.accent.gamma_multiply(0.88) } else { t.accent };
        ui.painter().rect_filled(rect, 6.0, fill);
        let mut x = rect.left() + 16.0;
        if let Some(icon) = icon {
            crate::icons::paint(ui.painter(), Rect::from_center_size(egui::pos2(x + 5.0, rect.center().y), vec2(10.0, 10.0)), icon, t.on_accent);
            x += icon_w;
        }
        ui.painter().galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, t.on_accent);
    }
    let label = text.to_owned();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    resp
}

/// A quiet button with a tinted fill (Shuffle next to Play).
pub fn secondary(ui: &mut Ui, text: &str) -> Response {
    let t = Tokens::of(ui);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(13.0, semibold()), t.accent);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 28.0, 30.0), Sense::click());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, 6.0, if resp.hovered() { t.selected } else { t.field });
        ui.painter().galley(rect.center() - galley.size() / 2.0, galley, t.accent);
    }
    let label = text.to_owned();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    resp
}

/// Mutually exclusive choices in a pill (Up Next / History, sort order).
/// Returns the index clicked, if any.
pub fn segmented(ui: &mut Ui, options: &[&str], selected: usize) -> Option<usize> {
    let t = Tokens::of(ui);
    let font = FontId::proportional(12.0);
    let widths: Vec<f32> = options.iter().map(|o| ui.painter().layout_no_wrap((*o).to_owned(), font.clone(), t.text).size().x + 22.0).collect();
    let size = vec2(widths.iter().sum::<f32>() + 4.0, 26.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, 7.0, t.field);
    let mut x = rect.left() + 2.0;
    let mut clicked = None;
    for (i, (opt, w)) in options.iter().zip(widths).enumerate() {
        let r = Rect::from_min_size(egui::pos2(x, rect.top() + 2.0), vec2(w, rect.height() - 4.0));
        let resp = ui.interact(r, ui.scope_id().with(("seg", i, *opt)), Sense::click());
        if i == selected {
            ui.painter().rect_filled(r, 5.0, t.bg);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 5.0, t.hover);
        }
        let color = if i == selected { t.text } else { t.muted };
        ui.painter().text(r.center(), Align2::CENTER_CENTER, *opt, font.clone(), color);
        let label = (*opt).to_owned();
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, i == selected, &label));
        if resp.clicked() {
            clicked = Some(i);
        }
        x += w;
    }
    clicked
}

/// Muted small-caps-ish section label.
pub fn section(ui: &mut Ui, text: &str) {
    let t = Tokens::of(ui);
    ui.label(egui::RichText::new(text).font(FontId::new(11.0, semibold())).color(t.muted));
}
