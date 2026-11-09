//! The playback controls the title bar, the Player and Mini share: the round
//! play/pause, the speed and sleep pills and their menus, the chapter
//! scrubber, and the hairline for the whole book under it, broken where
//! chapters begin, so the two can't be mistaken for each other.
//!
//! The scrubber and the book line paint into a rect they're given, so the
//! title bar's capsule can draw them at its own small size and the Player at
//! its large one.

use super::view_ctx::ViewCtx;
use super::{Action, PrefsAction};
use crate::player::Control;
use crate::timeline::Timeline;
use egui::{Color32, FontId, Rect, Response, RichText, Sense, Ui, pos2, vec2};
use shell::{
    icons::{self, Icon},
    theme::{Tokens, semibold},
};

/// The Player's scrubber strip, easy to grab, and the 4 pt line in it.
const SCRUB_H: f32 = 16.0;
const SCRUB_LINE: f32 = 4.0;
const SCRUB_KNOB: f32 = 6.0;
/// The book line's hover strip, and the line inside it.
const BOOK_H: f32 = 8.0;
const BOOK_LINE: f32 = 2.0;
/// The title bar's pill height.
const BAR_PILL_H: f32 = 22.0;

/// The speeds the speed menu offers at a click; the slider has the rest.
const SPEEDS: [f32; 6] = [0.8, 1.0, 1.25, 1.5, 1.75, 2.0];

impl ViewCtx<'_> {
    /// The speed pill's menu: common speeds at a click, the slider between
    /// them, then the playback preferences and Stop.
    pub(super) fn speed_menu(&mut self, pill: &Response) {
        egui::Popup::menu(pill).show(|ui| {
            let t = Tokens::of(ui);
            ui.set_min_width(250.0);
            ui.label(RichText::new("Playback speed").font(FontId::new(13.0, semibold())));
            ui.add_space(4.0);
            let current = crate::timeline::normalize_speed(self.player.speed);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for speed in SPEEDS {
                    if ui.selectable_label(speed == current, crate::timeline::speed_label(speed)).clicked() {
                        self.actions.push(Action::Playback(Control::Speed(speed)));
                    }
                }
            });
            let mut value = self.player.speed;
            if ui.add(egui::Slider::new(&mut value, 0.5..=3.0).step_by(0.05).suffix("×").text("Speed")).changed() {
                self.actions.push(Action::Playback(Control::Speed(value)));
            }
            ui.label(RichText::new("[ and ] step it by 0.05.").size(11.0).color(t.muted));
            ui.separator();
            let mut prefs = self.playback;
            // Each value is named by the label beside it, for a screen reader too.
            egui::Grid::new("playback-prefs").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                for (label, value, range, unit) in [
                    ("Skip back", &mut prefs.skip_back, 1..=300, " s"),
                    ("Skip forward", &mut prefs.skip_forward, 1..=300, " s"),
                    ("Rewind after a long pause", &mut prefs.rewind, 0..=60, " s"),
                    ("Sleep timer duration", &mut prefs.sleep_minutes, 5..=120, " min"),
                    ("Rewind after sleep", &mut prefs.sleep_rewind, 0..=60, " s"),
                ] {
                    let name = ui.label(label).id;
                    ui.add(egui::DragValue::new(value).range(range).suffix(unit)).labelled_by(name);
                    ui.end_row();
                }
            });
            ui.label(RichText::new("A pause longer than 5 minutes resumes a little earlier; 0 turns it off.").size(11.0).color(t.muted));
            if prefs != self.playback {
                self.actions.push(Action::Prefs(PrefsAction::Playback(prefs)));
            }
            ui.separator();
            if ui.button("Stop playback").clicked() {
                self.actions.push(Action::Playback(Control::Stop));
                ui.close();
            }
        });
    }
}

/// The round play/pause at any diameter `d`: filled with `fill`, the glyph
/// knocked out in `knock`, the glyph in proportion to the circle.
pub(super) fn round_play(ui: &mut Ui, d: f32, fill: Color32, knock: Color32, paused: bool) -> Response {
    let (icon, label) = if paused { (Icon::Play, "Play") } else { (Icon::Pause, "Pause") };
    let (slot, resp) = ui.allocate_exact_size(vec2(d, d), Sense::click());
    let fill = if resp.hovered() { fill.gamma_multiply(0.85) } else { fill };
    ui.painter().circle_filled(slot.center(), d / 2.0, fill);
    shell::widgets::focus_ring(ui, &resp, slot, d / 2.0);
    // A triangle's weight sits left of its box's centre; nudged, it looks centred.
    let nudge = if paused { vec2(d * 0.0375, 0.0) } else { vec2(0.0, 0.0) };
    icons::paint(ui.painter(), Rect::from_center_size(slot.center() + nudge, vec2(d * 0.4, d * 0.4)), icon, knock);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(format!("{label} (Space)"))
}

/// How large a pill is drawn: the title bar's, or the Player's, which is
/// read from across the room. Padding and glyphs scale with the type.
#[derive(Clone, Copy)]
pub(super) struct Pill {
    pub height: f32,
    pub font: f32,
}

impl Pill {
    pub const BAR: Self = Self { height: BAR_PILL_H, font: 12.0 };
    pub const ROOM: Self = Self { height: 30.0, font: 14.0 };

    fn scale(self) -> f32 {
        self.font / 12.0
    }
}

/// How wide [`pill`] draws with this content.
pub(super) fn pill_width(ui: &Ui, size: Pill, icon: Option<Icon>, text: Option<&str>) -> f32 {
    let text_w = text.map_or(0.0, |text| ui.painter().layout_no_wrap(text.to_owned(), FontId::new(size.font, semibold()), Color32::WHITE).size().x);
    pill_layout(size, icon.is_some(), text.is_some(), text_w).0
}

/// The pill's width, and where its icon and its text start from its left.
fn pill_layout(size: Pill, icon: bool, text: bool, text_w: f32) -> (f32, f32, f32) {
    let s = size.scale();
    let glyph = if icon { size.font + if text { 5.0 * s } else { 0.0 } } else { 0.0 };
    let pad = 10.0 * s;
    (pad + glyph + text_w + 22.0 * s, pad, pad + glyph)
}

/// A small pill with a chevron, so it reads as the menu it opens: an icon,
/// some text, or both. `lit` draws the icon in the text colour rather than
/// muted, for a setting that's on.
pub(super) fn pill(ui: &mut Ui, t: &Tokens, size: Pill, icon: Option<Icon>, text: Option<&str>, lit: bool) -> Response {
    let s = size.scale();
    let galley = text.map(|text| ui.painter().layout_no_wrap(text.to_owned(), FontId::new(size.font, semibold()), t.text));
    let (w, icon_x, text_x) = pill_layout(size, icon.is_some(), galley.is_some(), galley.as_ref().map_or(0.0, |g| g.size().x));
    let (rect, resp) = ui.allocate_exact_size(vec2(w, size.height), Sense::click());
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    ui.painter().rect_filled(rect, rect.height() / 2.0, if resp.hovered() || open { t.selected } else { t.field });
    shell::widgets::focus_ring(ui, &resp, rect, rect.height() / 2.0);
    if let Some(icon) = icon {
        let glyph = Rect::from_min_size(pos2(rect.left() + icon_x, rect.center().y - size.font / 2.0), vec2(size.font, size.font));
        icons::paint(ui.painter(), glyph, icon, if lit { t.text } else { t.muted });
    }
    if let Some(galley) = galley {
        ui.painter().galley(pos2(rect.left() + text_x, rect.center().y - galley.size().y / 2.0), galley, t.text);
    }
    let chevron = Rect::from_center_size(pos2(rect.right() - 12.0 * s, rect.center().y + 0.5), vec2(8.0 * s, 8.0 * s));
    icons::paint(ui.painter(), chevron, Icon::ChevronDown, t.muted);
    resp
}

/// The speed as a pill, in the title bar or the Player.
pub(super) fn speed_pill(ui: &mut Ui, t: &Tokens, speed: f32, size: Pill) -> Response {
    let text = crate::timeline::speed_label(speed);
    let resp = pill(ui, t, size, None, Some(&text), true);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Playback speed, {text}")));
    resp.on_hover_text("Playback speed ([ and ])")
}

/// The whole book as a hairline under the scrubber: muted, not the text
/// colour, and broken where each chapter begins, so it reads as the book's
/// chapters rather than a second seek bar. It doesn't seek. Allocated in the
/// ui, at the Player's size.
pub(super) fn book_line(ui: &mut Ui, timeline: &Timeline, at: f64, t: &Tokens) {
    let (slot, resp) = ui.allocate_exact_size(vec2(ui.available_width(), BOOK_H), Sense::hover());
    paint_book_line(ui, resp, slot, BOOK_LINE, timeline, at, t.panel, None, t);
}

/// The book line along the middle of `slot`, `thickness` high, its breaks
/// cut in `ground` (whatever it's drawn on). `resp` is its hover strip's. Hovering says how far into the book,
/// and `left` after it when given (the time left at speed).
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_book_line(
    ui: &Ui,
    resp: Response,
    slot: Rect,
    thickness: f32,
    timeline: &Timeline,
    at: f64,
    ground: Color32,
    left: Option<&str>,
    t: &Tokens,
) -> Response {
    let line = Rect::from_center_size(slot.center(), vec2(slot.width(), thickness));
    let duration = timeline.duration.max(1.0);
    let fraction = (at / duration).clamp(0.0, 1.0);
    let painter = ui.painter();
    let radius = thickness / 2.0;
    painter.rect_filled(line, radius, t.hairline);
    painter.rect_filled(Rect::from_min_max(line.min, pos2(line.left() + line.width() * fraction as f32, line.bottom())), radius, t.muted);
    // Breaks only where they'd stand apart; a book of very short chapters
    // would otherwise turn the line into dashes.
    if timeline.chapters.len() > 1 && line.width() / timeline.chapters.len() as f32 >= 6.0 {
        for chapter in &timeline.chapters[1..] {
            let x = line.left() + line.width() * (chapter.start / duration) as f32;
            painter.rect_filled(Rect::from_center_size(pos2(x, line.center().y), vec2(2.0, thickness + 1.0)), 0.0, ground);
        }
    }
    let share = format!("{}% of the book", (fraction * 100.0).floor());
    resp.on_hover_text(match left {
        Some(left) => format!("{share} · {left}"),
        None => share,
    })
}

/// The chapter scrubber, after Ond's big seek bar: a 4 pt line in a taller
/// strip that's easy to grab, played part in the text colour, a knob while
/// it's hovered or held. Allocated in the ui, at the Player's size. Returns
/// the position to show (the pointer's while dragging) and where to seek, if
/// anywhere.
pub(super) fn scrubber(ui: &mut Ui, at: f64, start: f64, end: f64, t: &Tokens) -> (f64, Option<f64>) {
    let (slot, response) = ui.allocate_exact_size(vec2(ui.available_width(), SCRUB_H), Sense::click_and_drag());
    let line = Rect::from_center_size(slot.center(), vec2(slot.width(), SCRUB_LINE));
    let scrub = Scrub { line, knob: SCRUB_KNOB, ring: slot };
    let (shown, seek, _) = scrub.run(ui, &response, at, start, end, t);
    (shown, seek)
}

/// A scrubber laid out somewhere: its `line` (as long and thick as it's
/// drawn), the knob's radius, and the rect the focus ring goes round.
pub(super) struct Scrub {
    pub line: Rect,
    pub knob: f32,
    pub ring: Rect,
}

impl Scrub {
    /// Paints the scrubber for `response` (its strip's) and reads the pointer
    /// and keys. Returns the position to show, where to seek, and whether the
    /// pointer is on it.
    pub(super) fn run(&self, ui: &Ui, response: &Response, at: f64, start: f64, end: f64, t: &Tokens) -> (f64, Option<f64>, bool) {
        let line = self.line;
        shell::widgets::focus_ring(ui, response, self.ring, line.height());
        let pointer = response.interact_pointer_pos().map(|p| start + f64::from(((p.x - line.left()) / line.width()).clamp(0.0, 1.0)) * (end - start));
        let shown = if response.dragged() { pointer.unwrap_or(at) } else { at };
        let fraction = ((shown - start) / (end - start).max(1.0)).clamp(0.0, 1.0);
        let x = line.left() + line.width() * fraction as f32;
        let radius = line.height() / 2.0;
        ui.painter().rect_filled(line, radius, t.hairline);
        ui.painter().rect_filled(Rect::from_min_max(line.min, pos2(x, line.bottom())), radius, t.text);
        let live = response.hovered() || response.dragged();
        if live {
            ui.painter().circle_filled(pos2(x, line.center().y), self.knob, t.text);
        }
        // A screen reader hears where in the chapter, out of how long.
        response.widget_info(|| egui::WidgetInfo::slider(end > start, shown - start, "Chapter position"));
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_min_numeric_value(0.0);
            node.set_max_numeric_value((end - start).max(0.0));
            node.set_value(format!("{} of {}", crate::fmt::clock((shown - start).max(0.0)), crate::fmt::clock((end - start).max(0.0))));
        });
        let key = |k| response.has_focus() && ui.input(|i| i.key_pressed(k));
        let seek = if key(egui::Key::ArrowLeft) {
            Some((at - 5.0).max(start))
        } else if key(egui::Key::ArrowRight) {
            Some((at + 5.0).min(end))
        } else if key(egui::Key::Home) {
            Some(start)
        } else if key(egui::Key::End) {
            Some(end)
        } else if response.clicked() || response.drag_stopped() {
            pointer
        } else {
            None
        };
        (shown, seek, live)
    }
}
