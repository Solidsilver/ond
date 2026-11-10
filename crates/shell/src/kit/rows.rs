//! Rows: the settings rows every page is built from (a [`group`] of
//! [`setting`]s, or [`row_with`] and its [`Head`], hairlines between them,
//! [`managed`] to lock one); the facts a panel lists, a [`kv_section`] of
//! [`kv_row`]s (or [`kv_text`]); a list row's [`row_fill`]; and the
//! [`sidebar_row`] a place in a sidebar is.

use super::focus_ring;
use super::text::{elide, elide_middle, section};
use crate::theme::{Tokens, semibold};
use egui::{Color32, FontId, Rect, Response, Sense, Ui, pos2, vec2};

/// The widest a list of rows grows (tracks, downloads), however wide the
/// window: past it a row's end drifts so far from its name that the two stop
/// reading as one line. Wider than a column of prose; a row holds a name, a
/// few short facts and its buttons.
pub const LIST_W: f32 = 680.0;

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
/// (a song's or a book's facts, an album's details).
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
