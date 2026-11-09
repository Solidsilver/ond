//! The kit popovers, sheets and settings rows are built from, shared so both
//! apps draw them alike. Most of it came from Ond as Rede needed it.
//!
//! - Text: [`elide`], [`elide_middle`], [`elide_job`], and the quiet
//!   [`link`] and [`link_line`] that underline on hover.
//! - Choices: the segmented pill, [`segmented`] (each segment fits its
//!   label), [`segmented_wide`] (equal segments) and [`choice`] (over values).
//! - Controls: [`thin_slider`] (and [`thin_slider_marked`]), [`switch`]
//!   (and [`switch_tinted`]).

use crate::theme::Tokens;
use crate::widgets::focus_ring;
use egui::text::{LayoutJob, TextWrapping};
use egui::{Color32, FontId, Galley, Rect, Response, Sense, Ui, vec2};
use std::sync::Arc;

/// One line of text cut to `width` with an ellipsis. The one way the apps
/// shorten a name to fit: lists, headers, the display, the visualizer's
/// caption (through [`elide_job`]) and [`link_line`].
pub fn elide(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    ui.painter().layout_job(elide_job(text, font, color, width))
}

/// `text` cut in the middle to fit `width`, keeping its start and (more of) its
/// end. For paths, whose last components are the part that tells them apart.
pub fn elide_middle(ui: &Ui, text: &str, font: &FontId, width: f32) -> String {
    let fits = |s: &str| ui.painter().layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x <= width;
    if fits(text) {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let cut = |keep: usize| {
        let head = keep / 3;
        let tail = keep - head;
        format!("{}…{}", chars[..head].iter().collect::<String>(), chars[chars.len() - tail..].iter().collect::<String>())
    };
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(&cut(mid)) { lo = mid } else { hi = mid - 1 }
    }
    cut(lo)
}

/// The layout [`elide`] lays out, for a painter without a `Ui`.
pub fn elide_job(text: &str, font: FontId, color: Color32, width: f32) -> LayoutJob {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = elide_wrap(width);
    job
}

/// One row of at most `width`, cut anywhere, ending in an ellipsis.
fn elide_wrap(width: f32) -> TextWrapping {
    TextWrapping { max_width: width.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') }
}

/// Text that reads as plain until hovered, then underlines: links where they
/// earn it, without turning the page blue.
pub fn link(ui: &mut Ui, text: &str, font: FontId, color: Color32) -> Response {
    // Never wider than the room it's in: a long name wraps (in a wrapping row,
    // across the whole row) instead of widening its column.
    let cap = if ui.layout().main_wrap { ui.max_rect().width() } else { ui.available_width() };
    let wrap = TextWrapping { max_width: cap.max(24.0), ..Default::default() };
    link_with(ui, text, font, color, wrap).0
}

/// A link kept to one line of at most `width`, cut with an ellipsis; the whole
/// text shows on hover when it was cut.
pub fn link_line(ui: &mut Ui, text: &str, font: FontId, color: Color32, width: f32) -> Response {
    let (resp, cut) = link_with(ui, text, font, color, elide_wrap(width));
    if cut { resp.on_hover_text(text) } else { resp }
}

/// The link, and whether its text was cut to fit (the layout says so, so the
/// whole text needn't be laid out again to find out).
fn link_with(ui: &mut Ui, text: &str, font: FontId, color: Color32, wrap: TextWrapping) -> (Response, bool) {
    let job = |underline: bool| {
        let mut job = LayoutJob::single_section(
            text.to_owned(),
            egui::TextFormat {
                font_id: font.clone(),
                color,
                underline: if underline { egui::Stroke::new(1.0, color) } else { egui::Stroke::NONE },
                ..Default::default()
            },
        );
        job.wrap = wrap.clone();
        job
    };
    let galley = ui.painter().layout_job(job(false));
    let cut = galley.elided;
    let (rect, resp) = ui.allocate_exact_size(galley.size(), Sense::click());
    if ui.is_rect_visible(rect) {
        let galley = if resp.hovered() { ui.painter().layout_job(job(true)) } else { galley };
        ui.painter().galley(rect.min, galley, color);
    }
    let label = text.to_owned();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, &label));
    (resp.on_hover_cursor(egui::CursorIcon::PointingHand), cut)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A harness whose first frame installs the theme (its fonts arrive a
    /// frame later) and whose later frames draw `body`.
    fn themed<S: 'static>(state: S, mut body: impl FnMut(&mut Ui, &mut S) + 'static) -> egui_kittest::Harness<'static, (bool, S)> {
        egui_kittest::Harness::builder().build_ui_state(
            move |ui, (installed, state): &mut (bool, S)| {
                if !std::mem::replace(installed, true) {
                    crate::theme::install(ui.ctx(), crate::theme::ThemePref::Light);
                    ui.ctx().request_repaint();
                    return;
                }
                body(ui, state);
            },
            (false, state),
        )
    }

    #[test]
    fn a_one_line_link_knows_when_its_text_was_cut() {
        let mut h = themed(Vec::new(), |ui, cuts: &mut Vec<bool>| {
            let font = FontId::proportional(13.0);
            cuts.push(link_with(ui, "Short", font.clone(), Color32::WHITE, elide_wrap(400.0)).1);
            cuts.push(link_with(ui, "A title far too long for forty points", font, Color32::WHITE, elide_wrap(40.0)).1);
        });
        h.run();
        assert_eq!(h.state().1.last_chunk::<2>(), Some(&[false, true]));
    }

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
}
