//! Room-sized listening controls and the small floating player share the same
//! actions and timeline as the bottom bar.

use super::{Action, RedeApp, player_bar, view_ctx::ViewCtx};
use crate::player::Control;
use egui::{Align, FontId, Layout, RichText, Sense, Ui, UiBuilder, vec2};
use shell::{
    icons::{self, Icon},
    theme::Tokens,
};

impl RedeApp {
    pub(super) fn player_window(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg).inner_margin(24)).show(ui, |ui| self.view().large_player(ui));
    }
    pub(super) fn mini_window(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.panel)).show(ui, |ui| self.view().mini_player(ui));
    }
}

impl ViewCtx<'_> {
    fn large_player(&mut self, ui: &mut Ui) {
        let Some(book) = &self.player.book else {
            ui.label("Choose a book from the library.");
            return;
        };
        let t = Tokens::of(ui);
        let side = ui.available_width().min((ui.available_height() - 300.0).max(120.0));
        ui.vertical_centered(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
            self.art.paint(ui, rect, book.item.summary.cover.as_deref(), 8);
            ui.add_space(16.0);
            ui.label(RichText::new(&book.item.summary.title).size(25.0).strong());
            let chapter = book.timeline.chapter(self.player.position).map(|i| book.timeline.chapters[i].title.as_str()).unwrap_or("Audiobook");
            ui.label(RichText::new(chapter).size(21.0).color(t.muted));
        });
        ui.add_space(16.0);
        let (start, end) = book.timeline.chapter_range(self.player.position);
        let (at, seek) = player_bar::scrubber(ui, self.player.position, start, end, &t);
        if let Some(at) = seek {
            self.actions.push(Action::Playback(Control::Seek(at)));
        }
        player_bar::book_line(ui, &book.timeline, at, &t);
        ui.horizontal(|ui| {
            ui.label(crate::fmt::clock(at - start));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(crate::timeline::span_left(book.timeline.remaining(at, self.player.speed).0));
            });
        });
        ui.add_space(12.0);
        self.large_transport(ui, true);
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let speed = player_bar::speed_pill(ui, &t, self.player.speed);
            self.speed_menu(&speed);
            self.sleep_menu(ui);
        });
        self.sleep_notice(ui);
    }

    fn large_transport(&mut self, ui: &mut Ui, chapters: bool) {
        let t = Tokens::of(ui);
        let width = if chapters { 286.0 } else { 176.0 };
        let rect = egui::Rect::from_center_size(egui::pos2(ui.available_rect_before_wrap().center().x, ui.cursor().top() + 28.0), vec2(width, 56.0));
        ui.scope_builder(UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Center)), |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if chapters && icons::sized_button(ui, Icon::Previous, "Previous chapter (⇧←)", vec2(40.0, 48.0), 16.0, Some(t.muted)).clicked() {
                self.actions.push(Action::Playback(Control::PreviousChapter));
            }
            let back = self.playback.skip_back;
            if icons::sized_button(ui, Icon::SkipBack(back), &format!("Back {back} seconds"), vec2(48.0, 48.0), 32.0, Some(t.text)).clicked() {
                self.actions.push(Action::Playback(Control::Skip(-f64::from(back))));
            }
            let paused = self.player.paused || !self.player.active;
            if icons::sized_button(
                ui,
                if paused { Icon::Play } else { Icon::Pause },
                if paused { "Play" } else { "Pause" },
                vec2(56.0, 56.0),
                32.0,
                Some(t.text),
            )
            .clicked()
            {
                self.actions.push(Action::Playback(Control::Toggle));
            }
            let forward = self.playback.skip_forward;
            if icons::sized_button(ui, Icon::SkipForward(forward), &format!("Forward {forward} seconds"), vec2(48.0, 48.0), 32.0, Some(t.text)).clicked() {
                self.actions.push(Action::Playback(Control::Skip(f64::from(forward))));
            }
            if chapters && icons::sized_button(ui, Icon::Next, "Next chapter (⇧→)", vec2(40.0, 48.0), 16.0, Some(t.muted)).clicked() {
                self.actions.push(Action::Playback(Control::NextChapter));
            }
        });
        ui.advance_cursor_after_rect(rect);
    }

    fn mini_player(&mut self, ui: &mut Ui) {
        let full = ui.max_rect();
        let drag = ui.interact(full, ui.scope_id().with("mini-drag"), Sense::click_and_drag());
        drag.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Move Mini player; double-click to return"));
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            self.actions.push(Action::ToggleMini);
        }
        let notice = self.player.sleep_remaining(std::time::Instant::now()).is_some_and(|left| left <= 60.0);
        let side = (full.height() - if notice { 164.0 } else { 114.0 }).max(60.0).min(full.width());
        let cover = egui::Rect::from_center_size(egui::pos2(full.center().x, full.top() + side / 2.0), vec2(side, side));
        let book = self.player.book.as_ref();
        self.art.paint(ui, cover, book.and_then(|b| b.item.summary.cover.as_deref()), 0);
        ui.scope_builder(UiBuilder::new().max_rect(full.shrink(6.0)).layout(Layout::right_to_left(Align::Min)), |ui| self.mode_buttons(ui));
        let bottom = egui::Rect::from_min_max(egui::pos2(full.left() + 8.0, cover.bottom() + 4.0), full.max - vec2(8.0, 0.0));
        ui.scope_builder(UiBuilder::new().max_rect(bottom).layout(Layout::top_down(Align::Center)), |ui| {
            let chapter = book.and_then(|b| b.timeline.chapter(self.player.position).map(|i| b.timeline.chapters[i].title.as_str())).unwrap_or("Choose a book");
            let t = Tokens::of(ui);
            let label = crate::widgets::elide(ui, chapter, FontId::proportional(14.0), t.text, ui.available_width());
            ui.label(label);
            self.large_transport(ui, false);
            self.sleep_notice(ui);
        });
    }
}
