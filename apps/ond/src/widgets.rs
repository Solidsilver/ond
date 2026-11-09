//! Small shared pieces: elided text, quiet links, the accent button, a
//! segmented control, a thin slider, a switch and a menu label, and the rows
//! Settings is built from (a [`group`] of [`setting`]s).

use crate::icons::{self, Icon};
use crate::theme::{Tokens, semibold};
use egui::text::{LayoutJob, TextWrapping};
use egui::{Color32, FontId, Galley, Rect, Response, Sense, Ui, pos2, vec2};
pub use shell::widgets::{danger, empty_state, focus_ring, menu_row, primary, secondary, secondary_danger, sidebar_row};
use std::sync::Arc;

/// One line of text cut to `width` with an ellipsis. The one way the app
/// shortens a name to fit: lists, headers, the display, the visualizer's
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

/// A heart that loves / unloves: filled accent when on, a quiet outline when off.
pub fn heart_button(ui: &mut Ui, on: bool, size: f32, glyph: f32) -> Response {
    let t = Tokens::of(ui);
    let (icon, color, label) = if on { (crate::icons::Icon::HeartFilled, t.accent, "Unlove") } else { (crate::icons::Icon::Heart, t.muted, "Love") };
    crate::icons::sized_button(ui, icon, label, vec2(size, size), glyph, Some(color))
}

/// The heart over a row or display: drawn filled when loved, as an outline when
/// `reveal` (its row is hovered) or when it's hovered itself, and always
/// clickable. `label` names it for accessibility ("Love", "Unlove (⇧⌘L)").
pub fn hover_heart(ui: &mut Ui, rect: Rect, id: egui::Id, loved: bool, reveal: bool, glyph: f32, label: &str) -> Response {
    let t = Tokens::of(ui);
    let resp = ui.interact(rect, id, Sense::click());
    // Focused, it shows (as when hovered), or Tab would land on nothing.
    let hot = resp.hovered() || resp.has_focus();
    if loved || reveal || hot {
        let (icon, color) = if loved { (crate::icons::Icon::HeartFilled, t.accent) } else { (crate::icons::Icon::Heart, if hot { t.text } else { t.muted }) };
        crate::icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(glyph, glyph)), icon, color);
    }
    let side = glyph + 6.0;
    focus_ring(ui, &resp, Rect::from_center_size(rect.center(), vec2(side, side)), side / 2.0);
    let label = crate::keys::shown(label).into_owned();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    resp
}

/// One choice in a rating menu: `rating` stars drawn as icons, filled up to
/// it, with a check when it's the current rating.
pub fn rating_choice(ui: &mut Ui, rating: u8, current: bool) -> Response {
    let t = Tokens::of(ui);
    let star = 12.0;
    // A fixed width: a menu sizes itself to its widest row, so taking the room
    // offered would stretch the menu across the window.
    let width = 26.0 + 5.0 * (star + 3.0) + 8.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 22.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, t.selected);
    }
    if current {
        crate::icons::paint(
            ui.painter(),
            Rect::from_center_size(egui::pos2(rect.left() + 10.0, rect.center().y), vec2(10.0, 10.0)),
            crate::icons::Icon::Check,
            t.text,
        );
    }
    for k in 0..5u8 {
        let c = egui::pos2(rect.left() + 26.0 + f32::from(k) * (star + 3.0), rect.center().y);
        let (icon, color) = if k < rating { (crate::icons::Icon::StarFilled, t.text) } else { (crate::icons::Icon::Star, t.muted) };
        crate::icons::paint(ui.painter(), Rect::from_center_size(c, vec2(star, star)), icon, color);
    }
    let label = crate::fmt::stars(rating);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::MenuItem, true, current, &label));
    resp
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

/// Grid or list: an icon for each in a segmented pill. Writes the pick into
/// `current` and says whether it changed.
pub fn view_picker(ui: &mut Ui, current: &mut crate::settings::ViewMode) -> bool {
    use crate::settings::ViewMode;
    let t = Tokens::of(ui);
    let seg = vec2(30.0, 22.0);
    let (rect, pill) = ui.allocate_exact_size(vec2(seg.x * 2.0 + 4.0, 26.0), Sense::hover());
    ui.painter().rect_filled(rect, 7.0, t.field);
    let mut changed = false;
    for (i, (mode, icon, label)) in [(ViewMode::Grid, Icon::Grid, "Grid"), (ViewMode::List, Icon::List, "List")].into_iter().enumerate() {
        let r = Rect::from_min_size(egui::pos2(rect.left() + 2.0 + seg.x * i as f32, rect.top() + 2.0), seg);
        let resp = ui.interact(r, pill.id.with(("view", i)), Sense::click());
        let on = *current == mode;
        if on {
            ui.painter().rect_filled(r, 5.0, t.bg);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 5.0, t.hover);
        }
        icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(13.0, 13.0)), icon, if on { t.text } else { t.muted });
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, on, label));
        if resp.on_hover_text(label).clicked() && !on {
            *current = mode;
            changed = true;
        }
    }
    changed
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

/// A small pill of tinted text (a finding worth a glance, not an alarm).
pub fn chip(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(11.0), color);
    let size = galley.size() + vec2(14.0, 4.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, rect.height() / 2.0, color.gamma_multiply(0.13));
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text));
    resp
}

/// Muted small-caps-ish section label.
pub fn section(ui: &mut Ui, text: &str) {
    let t = Tokens::of(ui);
    ui.label(egui::RichText::new(text).font(FontId::new(11.0, semibold())).color(t.muted));
}

/// A list row's background: the selection, else the hover tint, else none.
pub fn row_fill(t: &Tokens, selected: bool, hovered: bool) -> Color32 {
    if selected {
        t.selected
    } else if hovered {
        t.hover
    } else {
        Color32::TRANSPARENT
    }
}

/// A track title's colour in a list: the accent while it plays, dimmed when it
/// can't play right now (offline and not downloaded).
pub fn track_color(t: &Tokens, playing: bool, playable: bool) -> Color32 {
    if playing {
        t.accent
    } else if playable {
        t.text
    } else {
        t.muted.gamma_multiply(0.75)
    }
}

/// The label column of [`kv_row`]s, so values line up down a section.
const KV_LABEL_W: f32 = 96.0;
/// Value text size in [`kv_row`]s.
pub const KV_VALUE: f32 = 12.5;

/// A [`section`] label with room above it, heading a run of [`kv_row`]s.
pub fn kv_section(ui: &mut Ui, title: &str) {
    ui.add_space(18.0);
    section(ui, title);
    ui.add_space(6.0);
}

/// A muted label in a fixed column, then the value laid out by `value`
/// (Info tab, album details, the Player's details).
pub fn kv_row(ui: &mut Ui, label: &str, value: impl FnOnce(&mut Ui)) {
    let t = Tokens::of(ui);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (r, _) = ui.allocate_exact_size(vec2(KV_LABEL_W, KV_VALUE + 4.0), Sense::hover());
        let g = elide(ui, label, FontId::proportional(12.0), t.muted, KV_LABEL_W - 4.0);
        ui.painter().galley(r.min + vec2(0.0, 1.0), g, t.muted);
        let rest = ui.available_width();
        ui.allocate_ui_with_layout(vec2(rest, 0.0), egui::Layout::top_down(egui::Align::Min), value);
    });
    ui.add_space(6.0);
}

/// A [`kv_row`] whose value is plain, wrapping text.
pub fn kv_text(ui: &mut Ui, label: &str, value: &str) {
    let t = Tokens::of(ui);
    kv_row(ui, label, |ui| {
        ui.add(egui::Label::new(egui::RichText::new(value).size(KV_VALUE).color(t.text)).wrap());
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

/// A row for a setting set in managed.json when `locked`: greyed, tagged
/// as set there, and its control can't be used.
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
pub fn row_with<R>(ui: &mut Ui, mut head: Head<'_>, control: impl FnOnce(&mut Ui) -> R) -> R {
    // Rows are only ever disabled whole by `managed`.
    if !ui.is_enabled() && head.tag.is_none() {
        head.tag = Some(crate::managed::LOCKED);
    }
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

/// Bit-perfect's switch for the device `name`, green when on: the one the
/// output panel and Settings › Output both show. `mode` comes back as chosen;
/// each caller acts on the change its own way.
pub fn bit_perfect_switch(ui: &mut Ui, name: &str, mode: &mut audio::PlaybackMode) -> Response {
    let mut on = *mode == audio::PlaybackMode::BitPerfect;
    let resp = switch_tinted(ui, &mut on, &format!("Bit-perfect on {name}"), Tokens::of(ui).ok);
    *mode = if on { audio::PlaybackMode::BitPerfect } else { audio::PlaybackMode::Normal };
    resp
}

/// A quiet button that opens a menu: its text and a small chevron, hugging
/// its content.
pub fn menu_label(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let t = Tokens::of(ui);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(12.0), color);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 26.0, 24.0), Sense::click());
    if ui.is_rect_visible(rect) {
        if resp.hovered() {
            ui.painter().rect_filled(rect, 6.0, t.hover);
        }
        ui.painter().galley(egui::pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0), galley, color);
        let chev = Rect::from_center_size(egui::pos2(rect.right() - 10.0, rect.center().y + 1.0), vec2(8.0, 8.0));
        crate::icons::paint(ui.painter(), chev, crate::icons::Icon::ChevronDown, t.muted);
    }
    let label = text.to_owned();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    resp
}

/// The round button on a tile's art (play on an album, shuffle on an
/// artist), 32 points across at `at`. It's there while the tile or the
/// button itself is hovered (on the button, the tile doesn't read as
/// hovered), filled with the accent under the pointer. True when clicked.
pub fn tile_button(ui: &Ui, t: &Tokens, at: Rect, id: egui::Id, tile_hovered: bool, icon: Icon, label: &str) -> bool {
    let resp = ui.interact(at, id, Sense::click());
    if !(tile_hovered || resp.hovered()) {
        return false;
    }
    let fill = if resp.hovered() { t.accent } else { t.bg.gamma_multiply(0.92) };
    let fg = if resp.hovered() { t.on_accent } else { t.text };
    ui.painter().circle_filled(at.center(), 16.0, fill);
    // The play triangle sits a point right of centre so it looks centred.
    let glyph = if matches!(icon, Icon::Play) {
        Rect::from_center_size(at.center() + vec2(1.0, 0.0), vec2(12.0, 12.0))
    } else {
        Rect::from_center_size(at.center(), vec2(13.0, 13.0))
    };
    icons::paint(ui.painter(), glyph, icon, fg);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    resp.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_shows_which_painted_control_has_focus() {
        // One of each control that paints itself, and whether its ring is drawn.
        let mut h = egui_kittest::Harness::builder().build_ui_state(
            |ui, on: &mut bool| {
                segmented_wide(ui, &["Off", "On"], 0, 120.0);
                switch(ui, on, "Shuffle");
                hover_heart(ui, Rect::from_min_size(ui.cursor().min, vec2(24.0, 24.0)), ui.scope_id().with("heart"), false, false, 14.0, "Love");
            },
            false,
        );
        let rings = |h: &egui_kittest::Harness<'_, bool>| {
            let accent = Tokens::of_ctx(&h.ctx).accent;
            h.output().shapes.iter().filter(|c| matches!(&c.shape, egui::Shape::Rect(r) if r.stroke.color == accent && r.stroke.width == 1.5)).count()
        };
        h.run();
        assert_eq!(rings(&h), 0, "nothing focused, no ring");
        for _ in 0..4 {
            h.key_press(egui::Key::Tab);
            h.run();
            assert_eq!(rings(&h), 1, "one control focused, one ring");
        }
    }

    #[test]
    fn two_controls_with_the_same_label_click_separately() {
        use egui_kittest::kittest::Queryable;
        let mut h = egui_kittest::Harness::builder().build_ui_state(
            |ui, picks: &mut (usize, usize)| {
                choice(ui, &[0, 1, 2], &mut picks.0, Some(300.0), |i| ["Off", "Auto", "Album"][i].into());
                choice(ui, &[0, 1, 2], &mut picks.1, Some(300.0), |i| ["Original", "Auto", "Transcoded"][i].into());
            },
            (0, 0),
        );
        h.run();
        h.get_all_by_label("Auto").next().expect("the first control's Auto").click();
        h.run();
        assert_eq!(*h.state(), (1, 0));
        h.get_all_by_label("Auto").nth(1).expect("the second control's Auto").click();
        h.run();
        assert_eq!(*h.state(), (1, 1));
    }

    #[test]
    fn a_one_line_link_knows_when_its_text_was_cut() {
        let mut cuts = Vec::new();
        let mut h = egui_kittest::Harness::builder().build_ui(|ui| {
            let font = FontId::proportional(13.0);
            cuts.push(link_with(ui, "Short", font.clone(), Color32::WHITE, elide_wrap(400.0)).1);
            cuts.push(link_with(ui, "A title far too long for forty points", font, Color32::WHITE, elide_wrap(40.0)).1);
        });
        h.run();
        drop(h);
        assert_eq!(cuts.last_chunk::<2>(), Some(&[false, true]));
    }

    #[test]
    fn the_bit_perfect_switch_hands_back_the_mode_chosen() {
        use egui_kittest::kittest::Queryable;
        let mut h = egui_kittest::Harness::builder()
            .build_ui_state(|ui, mode: &mut audio::PlaybackMode| _ = bit_perfect_switch(ui, "DAC", mode), audio::PlaybackMode::Normal);
        h.run();
        h.get_by_label("Bit-perfect on DAC").click();
        h.run();
        assert_eq!(*h.state(), audio::PlaybackMode::BitPerfect);
        h.get_by_label("Bit-perfect on DAC").click();
        h.run();
        assert_eq!(*h.state(), audio::PlaybackMode::Normal);
    }
}
