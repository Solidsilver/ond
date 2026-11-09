//! Widgets every app in the family draws the same way.

use crate::icons::Icon;
use crate::theme::{Tokens, semibold};
use egui::{Color32, FontId, Rect, Response, Sense, Ui, vec2};

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

/// How a filled button looks: text colour, fill at rest and on hover, and the
/// horizontal padding (both sides together).
struct Fill {
    text: Color32,
    rest: Color32,
    hover: Color32,
    pad: f32,
}

/// An accent ring just outside `rect` while `resp` has keyboard focus, for
/// the controls that paint themselves. A click never focuses them, so the
/// ring shows only when Tab got there.
pub fn focus_ring(ui: &Ui, resp: &Response, rect: Rect, radius: f32) {
    if resp.has_focus() {
        let ring = egui::Stroke::new(1.5, Tokens::of(ui).accent);
        ui.painter().rect_stroke(rect.expand(2.0), radius + 2.0, ring, egui::StrokeKind::Inside);
    }
}

/// The body every filled button shares: an optional icon, then the label.
fn filled(ui: &mut Ui, icon: Option<Icon>, text: &str, f: Fill) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(13.0, semibold()), f.text);
    let icon_w = if icon.is_some() { 16.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + icon_w + f.pad, 30.0), Sense::click());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, 6.0, if resp.hovered() { f.hover } else { f.rest });
        focus_ring(ui, &resp, rect, 6.0);
        let mut x = rect.left() + f.pad / 2.0;
        if let Some(icon) = icon {
            crate::icons::paint(ui.painter(), Rect::from_center_size(egui::pos2(x + 5.0, rect.center().y), vec2(10.0, 10.0)), icon, f.text);
            x += icon_w;
        }
        ui.painter().galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, f.text);
    }
    // A button in a disabled scope says so where a screen reader looks.
    let (label, enabled) = (text.to_owned(), ui.is_enabled());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &label));
    resp
}

/// The filled accent button (Play).
pub fn primary(ui: &mut Ui, icon: Option<Icon>, text: &str) -> Response {
    let t = Tokens::of(ui);
    filled(ui, icon, text, Fill { text: t.on_accent, rest: t.accent, hover: t.accent.gamma_multiply(0.88), pad: 32.0 })
}

/// [`primary`] for an action that isn't there yet: the same shape, in the
/// quiet field fill with muted ink, so it reads as waiting rather than broken
/// in either theme. It takes hovers, so a tooltip can say why
/// (`on_disabled_hover_text`), but not clicks.
pub fn primary_unavailable(ui: &mut Ui, icon: Option<Icon>, text: &str) -> Response {
    let t = Tokens::of(ui);
    ui.scope(|ui| {
        // These colours are already the disabled look; egui's own fade on
        // top would wash them out.
        ui.visuals_mut().disabled_alpha = 1.0;
        ui.add_enabled_ui(false, |ui| filled(ui, icon, text, Fill { text: t.muted, rest: t.field, hover: t.field, pad: 32.0 })).inner
    })
    .inner
}

/// A filled button for a destructive choice (Delete in a confirmation).
pub fn danger(ui: &mut Ui, text: &str) -> Response {
    let t = Tokens::of(ui);
    filled(ui, None, text, Fill { text: Color32::WHITE, rest: t.danger, hover: t.danger.gamma_multiply(0.88), pad: 32.0 })
}

/// A quiet button with a tinted fill (Shuffle next to Play).
pub fn secondary(ui: &mut Ui, text: &str) -> Response {
    let t = Tokens::of(ui);
    filled(ui, None, text, Fill { text: t.accent, rest: t.field, hover: t.selected, pad: 28.0 })
}

/// [`secondary`] for something that can't be taken back (Sign out): red
/// text, the same quiet fill.
pub fn secondary_danger(ui: &mut Ui, text: &str) -> Response {
    let t = Tokens::of(ui);
    filled(ui, None, text, Fill { text: t.danger, rest: t.field, hover: t.selected, pad: 28.0 })
}

/// A place in a sidebar: its name, highlighted when `on`, `trailing` points
/// kept clear at the right end for a mark the caller paints there. `name` is
/// what the accessibility tree calls it (the label, and whatever the mark
/// says).
pub fn sidebar_row(ui: &mut Ui, label: &str, on: bool, trailing: f32, name: &str) -> Response {
    let t = Tokens::of(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if on {
            t.selected
        } else if resp.hovered() {
            t.hover
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 6.0, fill);
        focus_ring(ui, &resp, rect.shrink(2.0), 4.0);
        let font = if on { FontId::new(13.0, semibold()) } else { FontId::proportional(13.0) };
        let mut job = egui::text::LayoutJob::simple_singleline(label.to_owned(), font, t.text);
        job.wrap =
            egui::text::TextWrapping {
                max_width: (rect.width() - 16.0 - trailing).max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…')
            };
        let g = ui.painter().layout_job(job);
        ui.painter().galley(egui::pos2(rect.left() + 8.0, rect.center().y - g.size().y / 2.0), g, t.text);
    }
    let name = name.to_owned();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::ListBoxOption, true, on, &name));
    resp
}

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
