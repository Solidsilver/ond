//! Popovers and what goes in them: [`popover`] (or [`popover_at`]) opens one
//! in the theme's frame; [`padded`] lines a block up with its rows;
//! [`popover_title`], [`popover_rule`], [`menu_row`] (a command),
//! [`info_row`] (a fact) and [`switch_row`] (a setting that's on or off).

use super::controls::switch;
use super::focus_ring;
use crate::theme::{Tokens, semibold};
use egui::{FontId, Response, Sense, Ui, vec2};

/// A row in a popover menu, hovered as a sidebar row is. True when chosen,
/// and the menu closes.
pub fn menu_row(ui: &mut Ui, label: &str) -> bool {
    let t = Tokens::of(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
    if ui.is_rect_visible(rect) {
        if resp.hovered() {
            ui.painter().rect_filled(rect, 5.0, t.hover);
        }
        focus_ring(ui, &resp, rect.shrink(2.0), 3.0);
        ui.painter().text(egui::pos2(rect.left() + 8.0, rect.center().y), egui::Align2::LEFT_CENTER, label, FontId::proportional(13.0), t.text);
    }
    let label = label.to_owned();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::MenuItem, true, &label));
    if resp.clicked() {
        ui.close();
        return true;
    }
    false
}

/// How far a popover's text sits in from its frame's edge, on top of the
/// frame's own margin: Ond's popovers line their text up 8 in, and their menu
/// rows run the full width with their labels 8 in.
pub const POPOVER_PAD: f32 = 8.0;

/// A popover under `anchor`, `width` wide: the frame the theme gives every
/// menu, closed by a click outside it, its rows set tight (they space
/// themselves). Rows ([`menu_row`], [`info_row`], [`popover_title`]) inset
/// their own text; anything else goes in [`padded`].
pub fn popover<R>(anchor: &Response, width: f32, body: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    popover_at(anchor, width, egui::RectAlign::BOTTOM_START, body)
}

/// [`popover`] placed by `align`: `TOP_START` opens up from a control at the
/// foot of the window.
pub fn popover_at<R>(anchor: &Response, width: f32, align: egui::RectAlign, body: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    egui::Popup::menu(anchor)
        .align(align)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(width)
        .show(|ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add_space(6.0);
            let out = body(ui);
            ui.add_space(4.0);
            out
        })
        .map(|r| r.inner)
}

/// `body` in a column inset by [`POPOVER_PAD`] on both sides, so it lines up
/// with a popover's rows.
pub fn padded<R>(ui: &mut Ui, body: impl FnOnce(&mut Ui) -> R) -> R {
    let width = ui.available_width() - 2.0 * POPOVER_PAD;
    ui.horizontal(|ui| {
        ui.add_space(POPOVER_PAD);
        ui.vertical(|ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 0.0;
            body(ui)
        })
        .inner
    })
    .inner
}

/// A popover's title: 13 pt semibold, inset as its rows are, with
/// `trailing` (a status, a value) muted at the right end when there is one.
pub fn popover_title(ui: &mut Ui, title: &str, trailing: Option<&str>) {
    let t = Tokens::of(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
    let rect = rect.shrink2(vec2(POPOVER_PAD, 0.0));
    let p = ui.painter();
    p.text(rect.left_center(), egui::Align2::LEFT_CENTER, title, FontId::new(13.0, semibold()), t.text);
    if let Some(trailing) = trailing {
        p.text(rect.right_center(), egui::Align2::RIGHT_CENTER, trailing, FontId::proportional(12.0), t.muted);
    }
}

/// A hairline across a popover, with `space` above and below it.
pub fn popover_rule(ui: &mut Ui, space: f32) {
    let t = Tokens::of(ui);
    ui.add_space(space);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, egui::Stroke::new(1.0, t.hairline));
    ui.add_space(space);
}

/// A label and value in a popover (the server's): the label muted in a
/// fixed column, so values line up.
pub fn info_row(ui: &mut Ui, label: &str, value: &str, color: egui::Color32) {
    let t = Tokens::of(ui);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let (r, _) = ui.allocate_exact_size(vec2(64.0, 20.0), Sense::hover());
        ui.painter().text(r.left_center(), egui::Align2::LEFT_CENTER, label, FontId::proportional(12.0), t.muted);
        ui.add(egui::Label::new(egui::RichText::new(value).size(12.0).color(color)).truncate());
    });
}

/// A row in a popover with `label` at the left and a [`switch`] at the right,
/// both inset as the popover's rows are, the whole row a target. True when it changed.
pub fn switch_row(ui: &mut Ui, label: &str, on: &mut bool) -> bool {
    let t = Tokens::of(ui);
    let (rect, row) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    let rect = rect.shrink2(vec2(POPOVER_PAD, 0.0));
    ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, label, FontId::proportional(13.0), t.text);
    let mut sw = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::right_to_left(egui::Align::Center)));
    let flipped = switch(&mut sw, on, label).changed();
    if row.clicked() {
        *on = !*on;
        return true;
    }
    flipped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::themed;

    #[test]
    fn a_switch_row_flips_from_its_label_as_well_as_its_switch() {
        use egui_kittest::kittest::Queryable;
        let mut h = themed((false, 0), |ui, (on, changes): &mut (bool, u32)| {
            if switch_row(ui, "Skip silence", on) {
                *changes += 1;
            }
        });
        h.run();
        h.get_by_label("Skip silence").click();
        h.run();
        assert_eq!(h.state().1, (true, 1), "the switch flips once");
        let row = h.get_by_label("Skip silence").rect();
        let at = egui::pos2(row.left() - 120.0, row.center().y);
        h.hover_at(at);
        h.drag_at(at);
        h.drop_at(at);
        h.run();
        assert_eq!(h.state().1, (false, 2), "and so does the row's text");
    }
}
