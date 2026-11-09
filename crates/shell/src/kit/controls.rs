//! Controls that set a value: the segmented pill ([`segmented`],
//! [`segmented_wide`], [`choice`], and [`icon_choice`] for icons), the [`thin_slider`], the [`switch`], the
//! [`stepper`] and its [`step_button`], and the [`choice_chip`].

use super::focus_ring;
use super::text::elide;
use crate::icons::Icon;
use crate::theme::{Tokens, semibold};
use egui::{Color32, FontId, Rect, Response, Sense, Ui, vec2};

/// Mutually exclusive choices in a pill (Up Next / History, sort order).
/// Returns the index clicked, if any.
pub fn segmented(ui: &mut Ui, options: &[&str], selected: usize) -> Option<usize> {
    let font = FontId::proportional(12.0);
    let natural: Vec<f32> = options.iter().map(|o| ui.painter().layout_no_wrap((*o).to_owned(), font.clone(), Color32::WHITE).size().x).collect();
    segments(ui, options, &fit_segments(&natural, ui.available_width()), selected)
}

/// One of `all` in a segmented control, each labelled by `label`. With a
/// `width`, equal segments fill it; otherwise each fits its label. Writes the
/// pick into `current` and says whether it changed.
pub fn choice<T: Copy + PartialEq>(ui: &mut Ui, all: &[T], current: &mut T, width: Option<f32>, label: impl Fn(T) -> String) -> bool {
    let labels: Vec<String> = all.iter().map(|v| label(*v)).collect();
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    let at = all.iter().position(|v| v == current).unwrap_or(0);
    let picked = match width {
        Some(w) => segmented_wide(ui, &labels, at, w),
        None => segmented(ui, &labels, at),
    };
    match picked.map(|i| all[i]) {
        Some(v) if v != *current => {
            *current = v;
            true
        }
        _ => false,
    }
}

/// Segment widths for labels of `natural` width: comfortable padding when it
/// fits in `room`, tighter padding next, and finally the room shared out (the
/// labels then elide) — the control never grows past its column.
pub fn fit_segments(natural: &[f32], room: f32) -> Vec<f32> {
    let room = (room - 4.0).max(0.0);
    for pad in [22.0, 14.0, 8.0] {
        let w: Vec<f32> = natural.iter().map(|n| n + pad).collect();
        if w.iter().sum::<f32>() <= room {
            return w;
        }
    }
    vec![room / natural.len().max(1) as f32; natural.len()]
}

/// [`segmented`] with equal segments filling `width`.
pub fn segmented_wide(ui: &mut Ui, options: &[&str], selected: usize, width: f32) -> Option<usize> {
    let each = (width - 4.0) / options.len().max(1) as f32;
    segments(ui, options, &vec![each; options.len()], selected)
}

fn segments(ui: &mut Ui, options: &[&str], widths: &[f32], selected: usize) -> Option<usize> {
    let t = Tokens::of(ui);
    let font = FontId::proportional(12.0);
    let widths = widths.to_vec();
    let size = vec2(widths.iter().sum::<f32>() + 4.0, 26.0);
    // Segments take their ids from the pill's own: two controls in one scope
    // can share a label (ReplayGain and Streaming both have "Auto").
    let (rect, pill) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, 7.0, t.field);
    let mut x = rect.left() + 2.0;
    let mut clicked = None;
    for (i, (opt, w)) in options.iter().zip(widths).enumerate() {
        let r = Rect::from_min_size(egui::pos2(x, rect.top() + 2.0), vec2(w, rect.height() - 4.0));
        let resp = ui.interact(r, pill.id.with(("seg", i)), Sense::click());
        if i == selected {
            ui.painter().rect_filled(r, 5.0, t.bg);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 5.0, t.hover);
        }
        focus_ring(ui, &resp, r.shrink(2.0), 3.0);
        let color = if i == selected { t.text } else { t.muted };
        let g = elide(ui, opt, font.clone(), color, w - 6.0);
        ui.painter().galley(r.center() - g.size() / 2.0, g, color);
        let label = (*opt).to_owned();
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, i == selected, &label));
        if resp.clicked() {
            clicked = Some(i);
        }
        x += w;
    }
    clicked
}

/// Mutually exclusive choices drawn as icons in the segmented pill (grid or
/// list), each named by its label for a tooltip and a screen reader.
/// Returns the index clicked, unless it was the one already chosen.
pub fn icon_choice(ui: &mut Ui, options: &[(Icon, &str)], selected: usize) -> Option<usize> {
    let t = Tokens::of(ui);
    let seg = vec2(30.0, 22.0);
    let (rect, pill) = ui.allocate_exact_size(vec2(seg.x * options.len() as f32 + 4.0, 26.0), Sense::hover());
    ui.painter().rect_filled(rect, 7.0, t.field);
    let mut picked = None;
    for (i, (icon, label)) in options.iter().enumerate() {
        let r = Rect::from_min_size(egui::pos2(rect.left() + 2.0 + seg.x * i as f32, rect.top() + 2.0), seg);
        let resp = ui.interact(r, pill.id.with(("seg", i)), Sense::click());
        let on = i == selected;
        if on {
            ui.painter().rect_filled(r, 5.0, t.bg);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 5.0, t.hover);
        }
        crate::icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(13.0, 13.0)), *icon, if on { t.text } else { t.muted });
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, on, *label));
        if resp.on_hover_text(*label).clicked() && !on {
            picked = Some(i);
        }
    }
    picked
}

/// A thin horizontal slider for `value` in 0..=1 (volume): a 3 px track, the
/// filled part in the accent, a small round knob that grows on hover.
/// Dragging, clicking, the scroll wheel and ←/→ (when focused) all adjust it.
pub fn thin_slider(ui: &mut Ui, value: &mut f32, width: f32, label: &str) -> Response {
    thin_slider_marked(ui, value, width, label, None)
}

/// [`thin_slider`] with a small dot of `mark` in its knob (it says what the
/// slider sets, e.g. a device's own volume).
pub fn thin_slider_marked(ui: &mut Ui, value: &mut f32, width: f32, label: &str, mark: Option<Color32>) -> Response {
    let t = Tokens::of(ui);
    let fill = t.accent;
    let enabled = ui.is_enabled();
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, 18.0), Sense::click_and_drag());
    let knob_r = 6.0;
    let track = Rect::from_min_max(egui::pos2(rect.left() + knob_r, rect.center().y - 1.5), egui::pos2(rect.right() - knob_r, rect.center().y + 1.5));
    let before = *value;
    if enabled {
        if let Some(p) = resp.interact_pointer_pos()
            && (resp.dragged() || resp.clicked() || resp.drag_started())
        {
            *value = ((p.x - track.left()) / track.width()).clamp(0.0, 1.0);
        }
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta);
            let d = if scroll.x.abs() > scroll.y.abs() { scroll.x } else { scroll.y };
            if d != 0.0 {
                *value = (*value + d / 400.0).clamp(0.0, 1.0);
            }
        }
        if resp.has_focus() {
            let step = ui.input(|i| {
                let mut s = 0.0;
                if i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::ArrowUp) {
                    s += 0.05;
                }
                if i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowDown) {
                    s -= 0.05;
                }
                s
            });
            *value = (*value + step).clamp(0.0, 1.0);
        }
    }
    if *value != before {
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let fill = if enabled { fill } else { t.muted.gamma_multiply(0.5) };
        ui.painter().rect_filled(track, 1.5, t.hairline);
        let x = track.left() + track.width() * value.clamp(0.0, 1.0);
        ui.painter().rect_filled(Rect::from_min_max(track.min, egui::pos2(x, track.bottom())), 1.5, fill);
        if enabled {
            let hot = resp.hovered() || resp.dragged() || resp.has_focus();
            let r = if hot { knob_r + 1.0 } else { knob_r };
            let c = egui::pos2(x, track.center().y);
            let knob = if ui.visuals().dark_mode { t.text } else { Color32::WHITE };
            ui.painter().circle_filled(c + vec2(0.0, 0.5), r + 0.5, Color32::from_black_alpha(40));
            ui.painter().circle(c, r, knob, egui::Stroke::new(1.0, t.hairline));
            if let Some(mark) = mark {
                ui.painter().circle_filled(c, r * 0.45, mark);
            }
        }
    }
    let v = f64::from(*value);
    let label = label.to_owned();
    resp.widget_info(|| egui::WidgetInfo::slider(enabled, v, &label));
    resp
}

/// An on/off switch on its own; `label` names it for accessibility (the
/// row it sits in shows the name).
pub fn switch(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let accent = Tokens::of(ui).accent;
    switch_tinted(ui, on, label, accent)
}

/// [`switch`] whose track is `on_color` when on (bit-perfect's green).
pub fn switch_tinted(ui: &mut Ui, on: &mut bool, label: &str, on_color: Color32) -> Response {
    let t = Tokens::of(ui);
    let (rect, mut resp) = ui.allocate_exact_size(vec2(30.0, 26.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let sw = Rect::from_center_size(rect.center(), vec2(30.0, 18.0));
        let how_on = ui.ctx().animate_bool_responsive(resp.id, *on);
        let track = if *on { on_color } else { t.selected };
        ui.painter().rect_filled(sw, 9.0, track);
        let x = egui::lerp(sw.left() + 9.0..=sw.right() - 9.0, how_on);
        ui.painter().circle_filled(egui::pos2(x, sw.center().y), 7.0, Color32::WHITE);
        focus_ring(ui, &resp, sw, 9.0);
    }
    let (on, label) = (*on, label.to_owned());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Switch, true, on, &label));
    resp
}

/// A choice shown as a chip: a rounded cell in the field's fill, lit with
/// the accent when `on`, deeper on hover. `width` sets its width (a grid of
/// them lines up); `None` fits the text.
pub fn choice_chip(ui: &mut Ui, text: &str, on: bool, width: Option<f32>) -> Response {
    let t = Tokens::of(ui);
    let font = if on { FontId::new(12.5, semibold()) } else { FontId::proportional(12.5) };
    let ink = if on { t.accent } else { t.text };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, ink);
    let w = width.unwrap_or(galley.size().x + 20.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, CHIP_H), Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = match (on, resp.hovered()) {
            (true, hot) => t.accent.gamma_multiply(if hot { 0.24 } else { 0.16 }),
            (false, true) => t.selected,
            (false, false) => t.field,
        };
        ui.painter().rect_filled(rect, 6.0, fill);
        focus_ring(ui, &resp, rect, 6.0);
        ui.painter().galley(rect.center() - galley.size() / 2.0, galley, ink);
    }
    let label = text.to_owned();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), on, &label));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A [`choice_chip`]'s height.
pub const CHIP_H: f32 = 28.0;

/// One of `steps` (ascending) with a − and a + either side of its value as
/// `text` writes it, the value in a fixed width so the buttons stay put. A
/// value between steps (an old setting) moves to its neighbour. `label`
/// names it for a screen reader. True when it changed.
pub fn stepper<T: Copy + PartialOrd>(ui: &mut Ui, value: &mut T, steps: &[T], label: &str, text: impl Fn(T) -> String) -> bool {
    let t = Tokens::of(ui);
    if steps.is_empty() {
        return false;
    }
    let down = steps.iter().rev().find(|s| **s < *value).copied();
    let up = steps.iter().find(|s| **s > *value).copied();
    let mut changed = false;
    // Laid out left to right whatever the row's direction, so − comes first.
    let size = vec2(2.0 * STEP_BUTTON + STEP_VALUE_W, STEP_BUTTON);
    ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.painter().rect_filled(Rect::from_min_size(ui.cursor().min, size), 7.0, t.field);
        if step_button(ui, Icon::Minus, &format!("Less {label}"), down.is_some()).clicked()
            && let Some(v) = down
        {
            *value = v;
            changed = true;
        }
        let (r, resp) = ui.allocate_exact_size(vec2(STEP_VALUE_W, STEP_BUTTON), Sense::hover());
        let shown = text(*value);
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, &shown, FontId::proportional(12.5), t.text);
        let enabled = ui.is_enabled();
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::SpinButton, enabled, format!("{label}, {shown}")));
        if step_button(ui, Icon::Plus, &format!("More {label}"), up.is_some()).clicked()
            && let Some(v) = up
        {
            *value = v;
            changed = true;
        }
    });
    changed
}

const STEP_BUTTON: f32 = 26.0;
const STEP_VALUE_W: f32 = 56.0;

/// A − or + as a [`stepper`] draws it: quiet, filled on hover, faded when
/// `enabled` is false (the end of the range). For a control that steps
/// something else, as the speed slider's ends do.
pub fn step_button(ui: &mut Ui, icon: Icon, label: &str, enabled: bool) -> Response {
    let t = Tokens::of(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(STEP_BUTTON, STEP_BUTTON), if enabled { Sense::click() } else { Sense::hover() });
    if ui.is_rect_visible(rect) {
        if enabled && resp.hovered() {
            ui.painter().rect_filled(rect.shrink(2.0), 5.0, t.selected);
        }
        focus_ring(ui, &resp, rect.shrink(2.0), 5.0);
        let ink = if enabled { t.text } else { t.muted.gamma_multiply(0.5) };
        crate::icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(9.0, 9.0)), icon, ink);
    }
    let (label, on) = (label.to_owned(), enabled && ui.is_enabled());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, on, &label));
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::themed;

    #[test]
    fn two_controls_with_the_same_label_click_separately() {
        use egui_kittest::kittest::Queryable;
        let mut h = themed((0, 0), |ui, picks: &mut (usize, usize)| {
            choice(ui, &[0, 1, 2], &mut picks.0, Some(300.0), |i| ["Off", "Auto", "Album"][i].into());
            choice(ui, &[0, 1, 2], &mut picks.1, Some(300.0), |i| ["Original", "Auto", "Transcoded"][i].into());
        });
        h.run();
        h.get_all_by_label("Auto").next().expect("the first control's Auto").click();
        h.run();
        assert_eq!(h.state().1, (1, 0));
        h.get_all_by_label("Auto").nth(1).expect("the second control's Auto").click();
        h.run();
        assert_eq!(h.state().1, (1, 1));
    }

    #[test]
    fn a_stepper_moves_to_the_next_step_and_stops_at_the_ends() {
        use egui_kittest::kittest::Queryable;
        let mut h = themed(15u32, |ui, v| {
            stepper(ui, v, &[5, 10, 15, 30], "skip", |v| format!("{v} s"));
        });
        h.run();
        h.get_by_label("skip, 15 s");
        h.get_by_label("More skip").click();
        h.run();
        assert_eq!(h.state().1, 30);
        h.get_by_label("More skip").click();
        h.run();
        assert_eq!(h.state().1, 30, "the last step is the most");
        for _ in 0..5 {
            h.get_by_label("Less skip").click();
            h.run();
        }
        assert_eq!(h.state().1, 5, "and the first the least");
    }

    #[test]
    fn a_value_between_steps_moves_to_its_neighbour() {
        use egui_kittest::kittest::Queryable;
        let mut h = themed(7u32, |ui, v| {
            stepper(ui, v, &[5, 10, 15], "skip", |v| format!("{v} s"));
        });
        h.run();
        h.get_by_label("skip, 7 s");
        h.get_by_label("More skip").click();
        h.run();
        assert_eq!(h.state().1, 10);
        h.state_mut().1 = 7;
        h.run();
        h.get_by_label("Less skip").click();
        h.run();
        assert_eq!(h.state().1, 5);
    }

    #[test]
    fn a_chip_says_whether_it_is_the_one_chosen() {
        use egui_kittest::kittest::{NodeT, Queryable};
        let mut h = themed(None::<usize>, |ui, picked| {
            for (i, text) in ["5 min", "15 min"].into_iter().enumerate() {
                if choice_chip(ui, text, i == 1, Some(60.0)).clicked() {
                    *picked = Some(i);
                }
            }
        });
        h.run();
        let toggled = |h: &egui_kittest::Harness<'_, _>, label| h.get_by_label(label).accesskit_node().toggled();
        assert_eq!(toggled(&h, "15 min"), Some(egui::accesskit::Toggled::True));
        assert_eq!(toggled(&h, "5 min"), Some(egui::accesskit::Toggled::False));
        h.get_by_label("5 min").click();
        h.run();
        assert_eq!(h.state().1, Some(0));
    }
}
