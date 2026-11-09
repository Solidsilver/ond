//! The sleep timer's pill, menu and notice. They read the timer and ask for
//! actions; the engine clock owns time.
//!
//! The pill is the speed pill's sibling: a moon, then what's left once a
//! timer runs, then the chevron of a menu. The notice, shortly before the
//! timer stops playback, is a card in the manner of Ond's toasts, floating
//! over the bottom of whatever it's given, with the one button that matters.

use super::controls::{Pill, pill};
use super::{Action, PrefsAction, view_ctx::ViewCtx};
use crate::{player::Control, sleep::Choice};
use egui::{FontId, Rect, Response, RichText, Sense, Ui, pos2, vec2};
use shell::{
    icons::{self, Icon},
    theme::{Tokens, semibold},
};
use std::time::Instant;

/// The presets, in minutes; the custom row has the rest.
const PRESETS: [u32; 7] = [5, 15, 30, 45, 60, 90, 120];
/// The notice shows this long before the timer stops playback.
pub(super) const NOTICE_SECS: f64 = 60.0;

impl ViewCtx<'_> {
    /// What the pill says while a timer runs: the time left, or that it
    /// waits for the chapter to end. `None` while there's no timer.
    pub(super) fn sleep_text(&self) -> Option<String> {
        let left = self.player.sleep_remaining(Instant::now())?;
        Some(if self.sleeps_at_chapter_end() { "End of chapter".into() } else { crate::fmt::clock(left.ceil()) })
    }

    fn sleeps_at_chapter_end(&self) -> bool {
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
        self.sleep_menu(&response, left);
    }

    /// Laid out as the speed menu is: a heading, the common choices at a
    /// click, the custom length, then what changes a running timer.
    fn sleep_menu(&mut self, pill: &Response, left: Option<f64>) {
        egui::Popup::menu(pill).show(|ui| {
            let t = Tokens::of(ui);
            // Fixed, or the time at the heading's right end would stretch
            // the menu to the window's width.
            ui.set_width(260.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Sleep timer").font(FontId::new(13.0, semibold())));
                if let Some(left) = left {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let when =
                            if self.sleeps_at_chapter_end() { "At the chapter's end".to_owned() } else { format!("{} left", crate::fmt::clock(left.ceil())) };
                        ui.label(RichText::new(when).size(12.0).color(t.muted));
                    });
                }
            });
            ui.add_space(6.0);
            ui.label(RichText::new("Pause after").size(11.0).color(t.muted));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for minutes in PRESETS {
                    let chip = ui.selectable_label(false, minutes.to_string());
                    chip.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{minutes} minutes")));
                    if chip.clicked() {
                        self.actions.push(Action::Playback(Control::Sleep(Some(Choice::Minutes(minutes)))));
                        ui.close();
                    }
                }
                ui.add_space(4.0);
                ui.label(RichText::new("min").size(11.0).color(t.muted));
            });
            ui.add_space(2.0);
            if ui.selectable_label(self.sleeps_at_chapter_end(), "End of chapter").clicked() {
                self.actions.push(Action::Playback(Control::Sleep(Some(Choice::Chapter))));
                ui.close();
            }
            ui.separator();
            let mut prefs = self.playback;
            ui.horizontal(|ui| {
                let name = ui.label("Custom minutes").id;
                if ui.add(egui::DragValue::new(&mut prefs.sleep_minutes).range(5..=120).suffix(" min")).labelled_by(name).changed() {
                    self.actions.push(Action::Prefs(PrefsAction::Playback(prefs)));
                }
            });
            if ui.button(format!("Start {} minute timer", prefs.sleep_minutes)).clicked() {
                self.actions.push(Action::Playback(Control::Sleep(Some(Choice::Minutes(prefs.sleep_minutes)))));
                ui.close();
            }
            if left.is_some() {
                ui.separator();
                if ui.button("Extend 10 minutes").clicked() {
                    self.actions.push(Action::Playback(Control::ExtendSleep));
                    ui.close();
                }
                if ui.button("Cancel sleep timer").clicked() {
                    self.actions.push(Action::Playback(Control::Sleep(None)));
                    ui.close();
                }
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
            shell::widgets::focus_ring(ui, &extend, button, 6.0);
            ui.painter().galley(button.center() - notice.button.size() / 2.0, notice.button.clone(), t.accent);
            if extend.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.actions.push(Action::Playback(Control::ExtendSleep));
            }
        });
    }
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
