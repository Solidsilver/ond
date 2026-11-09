//! Buttons that paint themselves: the filled ones ([`primary`],
//! [`secondary`], [`danger`] and their kin), the [`quiet_button`], and the
//! [`focus_ring`] every self-painted control shows when Tab lands on it.

use crate::icons::Icon;
use crate::theme::{Tokens, semibold};
use egui::{Color32, FontId, Rect, Response, Sense, Ui, vec2};

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

/// A quiet button: muted text (and an icon) that fills on hover, for a
/// command that shouldn't shout, as a popover's footer has.
pub fn quiet_button(ui: &mut Ui, icon: Option<Icon>, text: &str) -> Response {
    let t = Tokens::of(ui);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(12.5), t.text);
    let icon_w = if icon.is_some() { 16.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + icon_w + 16.0, 26.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let ink = if resp.hovered() { t.text } else { t.muted };
        if resp.hovered() {
            ui.painter().rect_filled(rect, 6.0, t.hover);
        }
        focus_ring(ui, &resp, rect, 6.0);
        let mut x = rect.left() + 8.0;
        if let Some(icon) = icon {
            crate::icons::paint(ui.painter(), Rect::from_center_size(egui::pos2(x + 5.0, rect.center().y), vec2(10.0, 10.0)), icon, ink);
            x += icon_w;
        }
        ui.painter().galley_with_override_text_color(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, ink);
    }
    let (label, enabled) = (text.to_owned(), ui.is_enabled());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &label));
    resp
}
