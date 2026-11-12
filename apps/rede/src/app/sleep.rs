//! The sleep timer's pill, popover and notice. They read the timer and ask for
//! actions; the engine clock owns time.
//!
//! The pill is the speed pill's sibling: a moon, then what's left once a
//! timer runs, then the chevron of a menu. The notice, shortly before the
//! timer stops playback, is a card in the manner of Ond's toasts, floating
//! over the bottom of whatever it's given, with the one button that matters.

use super::controls::{Pill, pill};
use super::{Action, PrefsAction, view_ctx::ViewCtx};
use crate::{player::Control, sleep::Choice};
use egui::{FontId, Rect, Response, Sense, Ui, pos2, vec2};
use shell::{
    icons::{self, Icon},
    kit,
    theme::{Tokens, semibold},
};
use std::time::Instant;

/// The presets, in minutes, three to a row; the custom cell has the rest.
const PRESETS: [u32; 6] = [5, 15, 30, 45, 60, 90];
/// The lengths the custom cell steps through: the timer takes 5 to 120.
const SLEEP_LENGTHS: [u32; 10] = [5, 10, 15, 20, 30, 45, 60, 75, 90, 120];
/// The popover's width, and the gap between its chips.
const SLEEP_W: f32 = 264.0;
const GRID_GAP: f32 = 6.0;
/// What the running timer's button says: the engine's extension, 10 minutes.
const EXTEND_SHORT: &str = "Add 10 min";
/// What a timer waiting for the chapter's end offers instead.
const ONE_MORE_CHAPTER: &str = "One more chapter";
/// The custom cell's name for a screen reader: "custom timer, 40 min".
const CUSTOM: &str = "custom timer";
/// The notice shows this long before the timer stops playback.
pub(super) const NOTICE_SECS: f64 = 60.0;

impl ViewCtx<'_> {
    /// What the pill says while a timer runs: the time left, or that it
    /// waits for the chapter to end. `None` while there's no timer.
    pub(super) fn sleep_text(&self) -> Option<String> {
        let left = self.player.sleep_remaining(Instant::now())?;
        Some(if self.sleeps_at_chapter_end() { "End of chapter".into() } else { crate::fmt::clock(left.ceil()) })
    }

    /// What the capsule's sleep chip says while a timer runs, short enough
    /// for the chip beside the moon: whole minutes, seconds in the last one,
    /// and "End" while it waits for the chapter's end (its tooltip and menu
    /// say which end). `None` while there's no timer.
    pub(super) fn sleep_chip_text(&self) -> Option<String> {
        let left = self.player.sleep_remaining(Instant::now())?;
        Some(if self.sleeps_at_chapter_end() { "End".into() } else { short_left(left) })
    }

    pub(super) fn sleeps_at_chapter_end(&self) -> bool {
        self.player.sleep.as_ref().is_some_and(crate::sleep::Timer::at_chapter_end)
    }

    /// The sleep pill at `size`, and its menu.
    pub(super) fn sleep_control(&mut self, ui: &mut Ui, size: Pill) {
        let t = Tokens::of(ui);
        let left = self.player.sleep_remaining(Instant::now());
        let text = self.sleep_text();
        let response = pill(ui, &t, size, Some(Icon::Moon), text.as_deref(), left.is_some());
        response.widget_info(|| {
            let name = text.as_ref().map_or_else(|| "Sleep timer".to_owned(), |text| format!("Sleep timer, {text}"));
            egui::WidgetInfo::labeled(egui::Role::Button, true, name)
        });
        let response = response.on_hover_text(if left.is_some() { "Sleep timer" } else { "Sleep timer: off" });
        self.sleep_menu_with(&response, left);
    }

    /// The Sleep timer popover: while a timer runs, when it pauses, with the
    /// way to have more of it (ten minutes, or the next chapter) and Cancel;
    /// then the one grid of lengths: the presets, the chapter's end, and the
    /// length of the user's own, whose − and + set it without starting it.
    pub(super) fn sleep_menu(&mut self, pill: &Response) {
        let left = self.player.sleep_remaining(Instant::now());
        self.sleep_menu_with(pill, left);
    }

    fn sleep_menu_with(&mut self, pill: &Response, left: Option<f64>) {
        kit::popover(pill, SLEEP_W, |ui| {
            kit::popover_title(ui, "Sleep timer", left.is_none().then_some("Off"));
            ui.add_space(10.0);
            let at_chapter_end = self.sleeps_at_chapter_end();
            if let Some(left) = left {
                kit::padded(ui, |ui| self.running_timer(ui, left, at_chapter_end));
                kit::popover_rule(ui, 10.0);
            }
            kit::padded(ui, |ui| {
                kit::section(ui, if left.is_some() { "Change to" } else { "Pause after" });
                ui.add_space(6.0);
                self.sleep_grid(ui, at_chapter_end);
            });
        });
    }

    /// The lengths, three to a row, then the chapter's end and the custom
    /// length side by side under them.
    fn sleep_grid(&mut self, ui: &mut Ui, at_chapter_end: bool) {
        let w = ui.available_width();
        let cell = (w - 2.0 * GRID_GAP) / 3.0;
        let start = |actions: &mut Vec<Action>, choice| actions.push(Action::Playback(Control::Sleep(Some(choice))));
        for row in PRESETS.chunks(3) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GRID_GAP;
                for &minutes in row {
                    if kit::choice_chip(ui, &format!("{minutes} min"), false, Some(cell)).clicked() {
                        start(self.actions, Choice::Minutes(minutes));
                        ui.close();
                    }
                }
            });
            ui.add_space(GRID_GAP);
        }
        let half = (w - GRID_GAP) / 2.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GRID_GAP;
            if kit::choice_chip(ui, "End of chapter", at_chapter_end, Some(half)).clicked() && !at_chapter_end {
                start(self.actions, Choice::Chapter);
                ui.close();
            }
            let mut prefs = self.playback;
            let custom = kit::stepper_chip(ui, &mut prefs.sleep_minutes, &SLEEP_LENGTHS, half, CUSTOM, |m| format!("{m} min"));
            if custom.stepped {
                self.actions.push(Action::Prefs(PrefsAction::Playback(prefs)));
            }
            if custom.chosen {
                start(self.actions, Choice::Minutes(prefs.sleep_minutes));
                ui.close();
            }
        });
    }

    /// A running timer: when it pauses, the time in the accent as the chip is
    /// lit, and the two things to do about it. A timer waiting for the
    /// chapter's end offers the next chapter rather than ten minutes, unless
    /// this is the last.
    fn running_timer(&mut self, ui: &mut Ui, left: f64, at_chapter_end: bool) {
        let t = Tokens::of(ui);
        let clock = crate::fmt::clock(left.ceil());
        let lead = if at_chapter_end { "Pauses when this chapter ends, in " } else { "Pauses in " };
        let mut job = egui::text::LayoutJob::default();
        job.append(lead, 0.0, egui::TextFormat::simple(FontId::new(14.0, semibold()), t.text));
        job.append(&clock, 0.0, egui::TextFormat::simple(FontId::new(14.0, semibold()), t.accent));
        job.wrap.max_width = ui.available_width();
        ui.add(egui::Label::new(job).wrap().selectable(false));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if !at_chapter_end {
                if shell::kit::secondary(ui, EXTEND_SHORT).clicked() {
                    self.actions.push(Action::Playback(Control::ExtendSleep));
                }
            } else if self.player.next_chapter_end().is_some() && shell::kit::secondary(ui, ONE_MORE_CHAPTER).clicked() {
                self.actions.push(Action::Playback(Control::OneMoreChapter));
            }
            if kit::quiet_button(ui, None, "Cancel timer").clicked() {
                self.actions.push(Action::Playback(Control::Sleep(None)));
                ui.close();
            }
        });
    }

    /// The notice's height at `width` when it shows, so a layout can make
    /// room for it; `None` when it doesn't.
    pub(super) fn sleep_notice_height(&self, ui: &Ui, width: f32) -> Option<f32> {
        let left = self.player.sleep_remaining(Instant::now()).filter(|left| *left <= NOTICE_SECS)?;
        Some(Notice::new(ui, left, width).size.y)
    }

    /// The notice over the bottom of `within`, centred, while the timer is in
    /// its last minute.
    pub(super) fn sleep_notice(&mut self, ui: &Ui, within: Rect) {
        let Some(left) = self.player.sleep_remaining(Instant::now()).filter(|left| *left <= NOTICE_SECS) else { return };
        let t = Tokens::of(ui);
        let notice = Notice::new(ui, left, within.width() - 2.0 * NOTICE_INSET);
        let rect = Rect::from_min_size(pos2(within.center().x - notice.size.x / 2.0, within.bottom() - NOTICE_INSET - notice.size.y), notice.size);
        let id = egui::Id::unique("sleep-notice");
        egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(rect.min).show(ui.ctx(), |ui| {
            let (_, body) = ui.allocate_exact_size(rect.size(), Sense::hover());
            body.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Alert, true, notice.text.text()));
            let painter = ui.painter();
            let shadow =
                egui::Shadow { offset: [0, 6], blur: 20, spread: 0, color: egui::Color32::from_black_alpha(if ui.visuals().dark_mode { 110 } else { 34 }) };
            painter.add(shadow.as_shape(rect, 10.0));
            painter.rect(rect, 10.0, t.panel, egui::Stroke::new(1.0, t.hairline), egui::StrokeKind::Inside);
            let top = if notice.stacked { rect.top() + PAD_Y } else { rect.center().y - notice.text.size().y / 2.0 };
            let text_at = pos2(rect.left() + PAD_X + MOON + MOON_GAP, top);
            let first = notice.text.rows.first().map_or(16.0, |r| r.height());
            icons::paint(
                painter,
                Rect::from_center_size(pos2(rect.left() + PAD_X + MOON / 2.0, text_at.y + first / 2.0), vec2(MOON, MOON)),
                Icon::Moon,
                t.muted,
            );
            let button = notice.button_rect(rect);
            painter.galley(text_at, notice.text.clone(), t.text);
            let extend = ui.interact(button, id.with("extend"), Sense::click());
            extend.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, EXTEND));
            if extend.hovered() {
                ui.painter().rect_filled(button, 6.0, t.hover);
            }
            shell::kit::focus_ring(ui, &extend, button, 6.0);
            ui.painter().galley(button.center() - notice.button.size() / 2.0, notice.button.clone(), t.accent);
            if extend.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.actions.push(Action::Playback(Control::ExtendSleep));
            }
        });
    }
}

/// "12m", or "48s" in the last minute: a countdown that fits a chip.
fn short_left(seconds: f64) -> String {
    let seconds = seconds.max(0.0).ceil() as u64;
    if seconds >= 60 { format!("{}m", seconds.div_ceil(60)) } else { format!("{seconds}s") }
}

const EXTEND: &str = "Extend 10 minutes";
const NOTICE_INSET: f32 = 12.0;
const PAD_X: f32 = 14.0;
const PAD_Y: f32 = 9.0;
const MOON: f32 = 12.0;
const MOON_GAP: f32 = 8.0;
const BUTTON_H: f32 = 22.0;
const BUTTON_PAD: f32 = 6.0;
const NOTICE_W: f32 = 380.0;

/// The notice laid out for a width: on one line when it fits, else the button
/// under the text, so it reads in Mini too.
struct Notice {
    text: std::sync::Arc<egui::Galley>,
    button: std::sync::Arc<egui::Galley>,
    stacked: bool,
    size: egui::Vec2,
}

impl Notice {
    fn new(ui: &Ui, left: f64, width: f32) -> Self {
        let t = Tokens::of(ui);
        let font = FontId::proportional(12.5);
        let width = width.min(NOTICE_W);
        let text = format!("Playback pauses in {}", crate::fmt::clock(left.ceil()));
        let button = ui.painter().layout_no_wrap(EXTEND.into(), FontId::new(12.5, semibold()), t.accent);
        let lead = PAD_X + MOON + MOON_GAP;
        let button_w = button.size().x + 2.0 * BUTTON_PAD;
        let line = ui.painter().layout_no_wrap(text.clone(), font.clone(), t.text);
        // Sized for the widest time it can say, so it doesn't twitch each second.
        let widest = ui.painter().layout_no_wrap("Playback pauses in 0:00".into(), font.clone(), t.text).size().x.max(line.size().x);
        let one_line = lead + widest + 12.0 + button_w + PAD_X - BUTTON_PAD;
        if one_line <= width {
            let h = BUTTON_H.max(line.size().y) + 2.0 * PAD_Y - 4.0;
            return Self { text: line, button, stacked: false, size: vec2(one_line, h) };
        }
        let text = ui.painter().layout(text, font, t.text, width - lead - PAD_X);
        let w = (lead + text.size().x.max(button_w - BUTTON_PAD) + PAD_X).min(width);
        let h = PAD_Y + text.size().y + 4.0 + BUTTON_H + PAD_Y - 4.0;
        Self { text, button, stacked: true, size: vec2(w, h) }
    }

    fn button_rect(&self, card: Rect) -> Rect {
        let w = self.button.size().x + 2.0 * BUTTON_PAD;
        if self.stacked {
            // Under the text, its label lined up with the text's left edge.
            let x = card.left() + PAD_X + MOON + MOON_GAP - BUTTON_PAD;
            Rect::from_min_size(pos2(x, card.bottom() - PAD_Y + 4.0 - BUTTON_H), vec2(w, BUTTON_H))
        } else {
            Rect::from_min_size(pos2(card.right() - PAD_X + BUTTON_PAD - w, card.center().y - BUTTON_H / 2.0), vec2(w, BUTTON_H))
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_chip_counts_down_in_minutes_then_seconds() {
        assert_eq!(super::short_left(23.0 * 60.0 + 41.0), "24m");
        assert_eq!(super::short_left(120.0 * 60.0), "120m");
        assert_eq!(super::short_left(60.0), "1m");
        assert_eq!(super::short_left(48.2), "49s");
        assert_eq!(super::short_left(0.0), "0s");
    }
}
