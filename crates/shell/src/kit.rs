//! The kit popovers, sheets and settings rows are built from, shared so both
//! apps draw them alike. Most of it came from Ond as Rede needed it.
//!
//! - Text: [`elide`], [`elide_middle`], [`elide_job`], and the quiet
//!   [`link`] and [`link_line`] that underline on hover.
//! - Choices: the segmented pill, [`segmented`] (each segment fits its
//!   label), [`segmented_wide`] (equal segments) and [`choice`] (over values).
//! - Controls: [`thin_slider`] (and [`thin_slider_marked`]), [`switch`]
//!   (and [`switch_tinted`]).
//! - Labels: [`section`] (a muted group label), [`info_row`] (a label and
//!   its value in a popover), [`key_cap`] (a combo drawn as a key).
//! - Settings rows: a [`group`] of [`setting`]s (or [`row_with`] and its
//!   [`Head`]), hairlines between them ([`divider`]), [`managed`] to lock one.
//! - Popovers: [`popover`] (or [`popover_at`]) opens one in the theme's
//!   frame; [`padded`] lines a block up with its rows; [`popover_title`], [`popover_rule`], [`menu_row`] (a
//!   command), [`switch_row`] (a setting that's on or off), and
//!   [`quiet_button`] for a command in its footer.
//! - Choosing: [`choice_chip`] (one of a grid of choices, lit when on) and
//!   [`stepper`] (a value with − and +; [`step_button`] is one of those).
//! - Sheets: [`sheet_frame`] (the frame a sheet over the window is drawn
//!   in) and [`sheet_header`] (its title, and a × that closes it).

use crate::icons::Icon;
use crate::theme::{Tokens, semibold};
use crate::widgets::focus_ring;
use egui::text::{LayoutJob, TextWrapping};
use egui::{Color32, FontId, Galley, Rect, Response, Sense, Ui, pos2, vec2};
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

/// Muted small-caps-ish section label.
pub fn section(ui: &mut Ui, text: &str) {
    let t = Tokens::of(ui);
    ui.label(egui::RichText::new(text).font(FontId::new(11.0, semibold())).color(t.muted));
}

/// A key combo drawn as a small rounded key: `keys` as the platform writes
/// them, on the field's fill with a hairline edge. Named as its keys, so a
/// screen reader reads the combo beside its description.
pub fn key_cap(ui: &mut egui::Ui, keys: &str) {
    let t = Tokens::of(ui);
    let g = ui.painter().layout_no_wrap(keys.to_owned(), FontId::proportional(12.0), t.text);
    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 12.0, 20.0), egui::Sense::hover());
    ui.painter().rect(r, 5.0, t.field, egui::Stroke::new(1.0, t.hairline), egui::StrokeKind::Inside);
    ui.painter().galley(r.center() - g.size() / 2.0, g, t.text);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, keys));
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

/// A titled block of rows with hairlines between them: the title, 4, the
/// rows, 20 to the next.
pub fn group(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    section(ui, title);
    ui.add_space(4.0);
    ui.data_mut(|d| d.insert_temp(first_row(), true));
    body(ui);
    ui.add_space(20.0);
}

/// Set by a [`group`] and cleared by its first row, which has no hairline above.
fn first_row() -> egui::Id {
    egui::Id::unique("settings-first-row")
}

/// The hairline above a row, unless it's the first in its group.
pub fn divider(ui: &mut Ui) {
    let first = ui.data_mut(|d| std::mem::replace(d.get_temp_mut_or(first_row(), true), false));
    if !first {
        let t = Tokens::of(ui);
        let (x, y) = (ui.cursor().left(), ui.cursor().top());
        ui.painter().hline(x..=x + ui.available_width(), y, egui::Stroke::new(1.0, t.hairline));
    }
}

/// Every control fits this height (a button is the tallest).
const CONTROL_H: f32 = 30.0;
/// Space above and below a row's content.
pub const ROW_PAD: f32 = 6.0;
/// The least room between a row's text and its control.
const ROW_GAP: f32 = 24.0;
/// The most of a name a row keeps beside its control before putting the
/// control under it (longer names wrap).
const NAME_W: f32 = 160.0;

/// What a row says on its left.
#[derive(Default, Clone, Copy)]
pub struct Head<'a> {
    pub label: &'a str,
    /// A small accent note after the label ("in use").
    pub tag: Option<&'a str>,
    /// Muted text under the label, wrapped; empty for none.
    pub hint: &'a str,
    /// The hint is a path: one line, elided, never wrapped.
    pub path: bool,
}

/// One setting: its name and a short hint on the left, its control on the
/// right, level with the name. Every page is built from these, so controls
/// share one right edge and hints never run under them. The control is added
/// right to left (its last part first).
pub fn setting<R>(ui: &mut Ui, label: &str, hint: &str, control: impl FnOnce(&mut Ui) -> R) -> R {
    row_with(ui, Head { label, hint, ..Default::default() }, control)
}

/// A row for a setting the app doesn't let the user change when `locked`
/// (Ond's managed.json): greyed, and its control can't be used. The app
/// tags the row to say why.
pub fn managed<R>(ui: &mut Ui, locked: bool, row: impl FnOnce(&mut Ui) -> R) -> R {
    if !locked {
        return row(ui);
    }
    ui.scope(|ui| {
        ui.disable();
        row(ui)
    })
    .inner
}

/// [`setting`] with the whole [`Head`].
pub fn row_with<R>(ui: &mut Ui, head: Head<'_>, control: impl FnOnce(&mut Ui) -> R) -> R {
    let t = Tokens::of(ui);
    divider(ui);
    let (left, top, width) = (ui.cursor().left(), ui.cursor().top(), ui.available_width());
    // The name's first line level with the control, and as much room below
    // the text as above it.
    let line = ui.ctx().fonts_mut(|f| f.row_height(&FontId::proportional(13.0)));
    let inset = (CONTROL_H - line) / 2.0;
    // The control goes beside the name when both fit (it did last frame), and
    // under it in a narrow window.
    let key = ui.scope_id().with(("setting", head.label));
    let name_w = ui.painter().layout_no_wrap(head.label.to_owned(), FontId::proportional(13.0), t.text).size().x.min(NAME_W);
    let fits = |w: f32| w + ROW_GAP + name_w <= width;
    let beside = fits(ui.data(|d| d.get_temp::<f32>(key)).unwrap_or(0.0));
    let (out, used, bottom) = if beside {
        let strip = Rect::from_min_size(pos2(left, top + ROW_PAD), vec2(width, CONTROL_H));
        let (out, used) = row_control(ui, strip, control);
        let room = (width - if used > 0.0 { used + ROW_GAP } else { 0.0 }).max(80.0);
        let text = row_text(ui, head, pos2(left, strip.top() + inset), room);
        (out, used, (text + inset).max(strip.bottom()) + ROW_PAD)
    } else {
        let text = row_text(ui, head, pos2(left, top + ROW_PAD + inset), width);
        let strip = Rect::from_min_size(pos2(left, text + 8.0), vec2(width, CONTROL_H));
        let (out, used) = row_control(ui, strip, control);
        (out, used, strip.bottom() + ROW_PAD)
    };
    ui.data_mut(|d| d.insert_temp(key, used));
    if fits(used) != beside {
        ui.ctx().request_discard("a setting's control moved beside or under its name");
    }
    ui.advance_cursor_after_rect(Rect::from_min_max(pos2(left, top), pos2(left + width, bottom)));
    out
}

/// A row's control, right-aligned in `strip`, and how wide it came out.
fn row_control<R>(ui: &mut Ui, strip: Rect, control: impl FnOnce(&mut Ui) -> R) -> (R, f32) {
    let mut right = ui.new_child(egui::UiBuilder::new().max_rect(strip).layout(egui::Layout::right_to_left(egui::Align::Center)));
    right.spacing_mut().item_spacing.x = 6.0;
    let out = control(&mut right);
    (out, right.min_rect().width())
}

/// A row's name (and tag) and hint from `at`, `room` wide. Returns where they end.
fn row_text(ui: &mut Ui, head: Head<'_>, at: egui::Pos2, room: f32) -> f32 {
    let t = Tokens::of(ui);
    let mut text =
        ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(at, vec2(room, f32::INFINITY))).layout(egui::Layout::top_down(egui::Align::Min)));
    text.spacing_mut().item_spacing.y = 3.0;
    let mut job = egui::text::LayoutJob::default();
    job.append(head.label, 0.0, egui::TextFormat::simple(FontId::proportional(13.0), t.text));
    if let Some(tag) = head.tag {
        job.append(tag, 6.0, egui::TextFormat::simple(FontId::proportional(11.0), t.accent));
    }
    job.wrap.max_width = room;
    let label = text.painter().layout_job(job);
    text.add(egui::Label::new(label));
    if !head.hint.is_empty() {
        let hint = crate::keys::shown(head.hint);
        if head.path {
            let shown = elide_middle(&text, &hint, &FontId::proportional(12.0), room);
            let resp = text.add(egui::Label::new(egui::RichText::new(shown).size(12.0).color(t.muted)).sense(Sense::hover()));
            resp.on_hover_text(hint.as_ref());
        } else {
            text.add(egui::Label::new(egui::RichText::new(hint).size(12.0).color(t.muted)).wrap());
        }
    }
    text.min_rect().bottom()
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

/// The frame every sheet over the window is drawn in (a dialog, the
/// keyboard shortcuts): the popup's fill, stroke and shadow, rounder, with
/// room inside.
pub fn sheet_frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::popup(&ctx.global_style()).inner_margin(egui::Margin::same(20)).corner_radius(12)
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

#[cfg(test)]
mod tests {
    use super::*;

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
