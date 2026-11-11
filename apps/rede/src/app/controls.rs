//! The playback controls the title bar, the Player and Mini share: the round
//! play/pause, the speed and sleep pills and their menus, the chapter
//! scrubber, and the hairline for the whole book under it, broken where
//! chapters begin, so the two can't be mistaken for each other.
//!
//! The scrubber and the book line paint into a rect they're given, so the
//! title bar's capsule can draw them at its own small size and the Player at
//! its large one.

use super::Action;
use super::view_ctx::ViewCtx;
use crate::player::Control;
use crate::timeline::Timeline;
use egui::{Color32, FontId, Rect, Response, RichText, Sense, Ui, pos2, vec2};
use shell::{
    icons::{self, Icon},
    kit,
    theme::{Tokens, semibold},
};

/// The Player's scrubber strip, easy to grab, and the 4 pt line in it.
const SCRUB_H: f32 = 16.0;
const SCRUB_LINE: f32 = 4.0;
const SCRUB_KNOB: f32 = 6.0;
/// The book line's hover strip, and the line inside it.
const BOOK_H: f32 = 8.0;
const BOOK_LINE: f32 = 2.0;

/// The speeds the Playback popover offers at a click; the slider and its
/// steppers have the rest, in [`SPEED_STEP`]s across [`SPEED_RANGE`].
const SPEEDS: [f32; 6] = [0.8, 1.0, 1.25, 1.5, 1.75, 2.0];
const SPEED_RANGE: (f32, f32) = (0.5, 3.0);
const SPEED_STEP: f32 = 0.05;
/// The Playback popover's width: room for the six presets.
const PLAYBACK_W: f32 = 296.0;
/// The volume row's height, the switches' below it, and the speaker's button.
const VOLUME_ROW: f32 = 30.0;
const SPEAKER: f32 = 22.0;
const SPEAKER_GLYPH: f32 = 15.0;

impl ViewCtx<'_> {
    /// The Playback popover the speed chip and pill open: the speed, large,
    /// with its presets, a slider and its steppers; the volume; voice boost
    /// and skip silence, with what skipping has saved in this book; then the
    /// way to Settings' Playback tab. The skip lengths and rewinds are
    /// settings, not controls, so they live there; Stop playback is a
    /// command, not a setting, so it's in the Player's "…" menu and the
    /// capsule's.
    pub(super) fn playback_menu(&mut self, anchor: &Response) {
        kit::popover(anchor, PLAYBACK_W, |ui| {
            kit::popover_title(ui, "Playback", None);
            ui.add_space(10.0);
            kit::padded(ui, |ui| self.speed_controls(ui));
            kit::popover_rule(ui, 8.0);
            kit::padded(ui, |ui| self.volume_row(ui));
            kit::popover_rule(ui, 8.0);
            self.speech_switches(ui);
            kit::popover_rule(ui, 8.0);
            ui.horizontal(|ui| {
                ui.add_space(kit::POPOVER_PAD - 8.0);
                if kit::quiet_button(ui, Some(Icon::Gear), "Settings…").clicked() {
                    self.actions.push(Action::OpenSettings(super::settings_page::Tab::Playback));
                    ui.close();
                }
            });
        });
    }

    /// The volume: a speaker that mutes, and the level itself from silence to
    /// full. The slider keeps its place while muted, so unmuting is a click.
    fn volume_row(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let playback = self.playback;
        let width = ui.available_width();
        ui.allocate_ui_with_layout(vec2(width, VOLUME_ROW), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let (icon, label) = if playback.muted { (Icon::Muted, "Unmute") } else { (Icon::Volume, "Mute") };
            if icons::sized_button(ui, icon, label, vec2(SPEAKER, SPEAKER), SPEAKER_GLYPH, Some(if playback.muted { t.muted } else { t.text })).clicked() {
                self.actions.push(Action::Prefs(super::PrefsAction::Muted(!playback.muted)));
            }
            // Room for the readout and the spacing either side of the slider,
            // so "65%" ends where the switches under it do.
            let readout = 60.0;
            let level_w = (ui.available_width() - readout).max(80.0);
            let mut level = playback.volume;
            if kit::thin_slider(ui, &mut level, level_w, "Volume").changed() {
                self.actions.push(Action::Prefs(super::PrefsAction::Volume(level)));
            }
            kit::readout(ui, &format!("{:.0}%", level * 100.0));
        });
    }

    /// Voice boost and skip silence, the two switches you'd flip while
    /// listening; under them, once skipping has cut something from this
    /// book, how much.
    fn speech_switches(&mut self, ui: &mut Ui) {
        let mut voice = self.playback.voice_boost;
        if kit::switch_row(ui, "Voice boost", &mut voice) {
            self.actions.push(Action::Prefs(super::PrefsAction::VoiceBoost(voice)));
        }
        let mut skip = self.playback.skip_silence;
        if kit::switch_row(ui, "Skip silence", &mut skip) {
            self.actions.push(Action::Prefs(super::PrefsAction::SkipSilence(skip)));
        }
        if let Some(saved) = saved_line(self.playback.skip_silence, self.player.silence_saved) {
            let t = Tokens::of(ui);
            kit::padded(ui, |ui| ui.label(RichText::new(saved).size(11.5).color(t.muted)));
            ui.add_space(4.0);
        }
    }

    /// The speed: its value large, the `[` and `]` that step it, the presets
    /// in a segmented pill (the one playing lit), and the slider between −
    /// and +.
    fn speed_controls(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let speed = crate::timeline::normalize_speed(self.player.speed);
        let width = ui.available_width();
        ui.horizontal(|ui| {
            ui.set_height(30.0);
            let value = crate::timeline::speed_label(speed);
            ui.label(RichText::new(&value).font(FontId::new(24.0, semibold())).color(t.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                kit::key_cap(ui, "]");
                kit::key_cap(ui, "[");
                ui.add_space(2.0);
                ui.label(RichText::new("Step").size(11.0).color(t.muted));
            })
            .response
            .on_hover_text("[ and ] step the speed by 0.05");
        });
        ui.add_space(8.0);
        let labels: Vec<String> = SPEEDS.iter().map(|s| crate::timeline::speed_label(*s)).collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        // None lit when the speed is between presets.
        let lit = SPEEDS.iter().position(|s| (*s - speed).abs() < 0.001).unwrap_or(usize::MAX);
        if let Some(i) = kit::segmented_wide(ui, &labels, lit, width) {
            self.actions.push(Action::Playback(Control::Speed(SPEEDS[i])));
        }
        ui.add_space(10.0);
        let (lo, hi) = SPEED_RANGE;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if kit::step_button(ui, Icon::Minus, "Slower", speed > lo).clicked() {
                self.actions.push(Action::Playback(Control::Speed(snap(speed - SPEED_STEP))));
            }
            let slider_w = ui.available_width() - 26.0 - 4.0;
            let mut v = (speed - lo) / (hi - lo);
            if kit::thin_slider(ui, &mut v, slider_w, "Speed").changed() {
                self.actions.push(Action::Playback(Control::Speed(snap(lo + v * (hi - lo)))));
            }
            if kit::step_button(ui, Icon::Plus, "Faster", speed < hi).clicked() {
                self.actions.push(Action::Playback(Control::Speed(snap(speed + SPEED_STEP))));
            }
        });
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(width, 14.0), Sense::hover());
            let ends = [(rect.left() + 30.0, egui::Align2::LEFT_CENTER, "0.5×"), (rect.right() - 30.0, egui::Align2::RIGHT_CENTER, "3×")];
            for (x, align, text) in ends {
                ui.painter().text(pos2(x, rect.center().y), align, text, FontId::proportional(10.5), t.muted);
            }
        });
    }
}

/// "Saved 3m 12s in this book": what skip silence has cut, once it's on and
/// has cut a whole second.
fn saved_line(on: bool, saved: f64) -> Option<String> {
    if !on || saved < 1.0 {
        return None;
    }
    let s = saved as u64;
    let span = match (s / 3600, s / 60 % 60, s % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, s) => format!("{m}m {s}s"),
        (h, m, _) => format!("{h}h {m}m"),
    };
    Some(format!("Saved {span} in this book"))
}

/// A speed on the slider's 0.05 grid, within its range.
fn snap(speed: f32) -> f32 {
    ((speed / SPEED_STEP).round() * SPEED_STEP).clamp(SPEED_RANGE.0, SPEED_RANGE.1)
}

/// The round play/pause at any diameter `d`: filled with `fill`, the glyph
/// knocked out in `knock`, the glyph in proportion to the circle.
pub(super) fn round_play(ui: &mut Ui, d: f32, fill: Color32, knock: Color32, paused: bool) -> Response {
    let (icon, label) = if paused { (Icon::Play, "Play") } else { (Icon::Pause, "Pause") };
    let (slot, resp) = ui.allocate_exact_size(vec2(d, d), Sense::click());
    let fill = if resp.hovered() { fill.gamma_multiply(0.85) } else { fill };
    ui.painter().circle_filled(slot.center(), d / 2.0, fill);
    shell::kit::focus_ring(ui, &resp, slot, d / 2.0);
    // A triangle's weight sits left of its box's centre; nudged, it looks centred.
    let nudge = if paused { vec2(d * 0.0375, 0.0) } else { vec2(0.0, 0.0) };
    icons::paint(ui.painter(), Rect::from_center_size(slot.center() + nudge, vec2(d * 0.4, d * 0.4)), icon, knock);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(format!("{label} (Space)"))
}

/// How large a pill is drawn: the Player's is read from across the room. Padding and glyphs scale with the type.
#[derive(Clone, Copy)]
pub(super) struct Pill {
    pub height: f32,
    pub font: f32,
}

impl Pill {
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
    shell::kit::focus_ring(ui, &resp, rect, rect.height() / 2.0);
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

/// A round button the size of a pill, for a command rather than a menu: no
/// chevron, the glyph centred. The Player's bookmark.
pub(super) fn pill_button(ui: &mut Ui, t: &Tokens, size: Pill, icon: Icon, label: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size.height, size.height), Sense::click());
    ui.painter().rect_filled(rect, rect.height() / 2.0, if resp.hovered() { t.selected } else { t.field });
    shell::kit::focus_ring(ui, &resp, rect, rect.height() / 2.0);
    let glyph = size.font;
    icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(glyph, glyph)), icon, if resp.hovered() { t.text } else { t.muted });
    let label = shell::keys::shown(label).into_owned();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(label)
}

/// The speed as a pill, in the Player.
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
        shell::kit::focus_ring(ui, response, self.ring, line.height());
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

#[cfg(test)]
mod tests {
    #[test]
    fn the_saved_line_waits_for_a_whole_second_and_reads_in_minutes() {
        assert_eq!(super::saved_line(false, 192.0), None, "skip silence is off");
        assert_eq!(super::saved_line(true, 0.4), None, "nothing worth saying yet");
        assert_eq!(super::saved_line(true, 42.9).as_deref(), Some("Saved 42s in this book"));
        assert_eq!(super::saved_line(true, 192.0).as_deref(), Some("Saved 3m 12s in this book"));
        assert_eq!(super::saved_line(true, 3_900.0).as_deref(), Some("Saved 1h 5m in this book"));
    }
}
