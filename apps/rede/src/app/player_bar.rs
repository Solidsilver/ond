//! A quiet bottom strip in the family's player idiom. The large line seeks
//! within the chapter; the thin line below keeps the whole book in view.

use super::view_ctx::ViewCtx;
use super::{Action, PrefsAction, RedeApp};
use crate::player::Control;
use egui::{Align, FontId, Layout, Rect, RichText, Sense, Ui, UiBuilder, pos2, vec2};
use shell::{
    icons::{self, Icon},
    theme::{Tokens, semibold},
};

impl RedeApp {
    pub(super) fn player_panel(&mut self, ui: &mut Ui) {
        if self.player.book.is_none() {
            return;
        }
        let t = Tokens::of(ui);
        egui::Panel::bottom("player-bar")
            .exact_size(138.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(t.panel).stroke(egui::Stroke::new(1.0, t.hairline)).inner_margin(egui::Margin::symmetric(24, 10)))
            .show(ui, |ui| self.view().player_bar(ui));
    }
}

impl ViewCtx<'_> {
    pub(super) fn player_bar(&mut self, ui: &mut Ui) {
        let Some(book) = &self.player.book else { return };
        let t = Tokens::of(ui);
        let at = self.player.position;
        let chapter = book.timeline.chapter(at).map(|i| book.timeline.chapters[i].title.as_str()).unwrap_or("Audiobook");
        ui.spacing_mut().item_spacing.y = 0.0;
        let full = ui.available_rect_before_wrap();
        let top = Rect::from_min_size(full.min, vec2(full.width(), 56.0));
        let cover = Rect::from_min_size(top.min, vec2(52.0, 52.0));
        self.art.paint(ui, cover, book.item.summary.cover.as_deref(), 4.0);
        let narrow = full.width() < 740.0;
        let transport_w = 180.0;
        let mut header = ui.new_child(UiBuilder::new().id_salt("player-header").max_rect(top));
        let controls_x = if narrow { top.right() - transport_w - 94.0 } else { top.center().x - transport_w / 2.0 };
        let text = Rect::from_min_max(pos2(cover.right() + 12.0, top.top() + 5.0), pos2(controls_x - 14.0, top.bottom()));
        header.scope_builder(UiBuilder::new().max_rect(text).layout(Layout::top_down(Align::Min)), |ui| {
            let title = crate::widgets::elide(ui, &book.item.summary.title, FontId::new(13.0, semibold()), t.text, text.width());
            let response = ui.add(egui::Label::new(title).sense(Sense::click()));
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Open playing book"));
            if response.clicked() {
                self.actions.push(Action::Go(super::Route::Book(book.key.item.clone())));
            }
            let chapter = crate::widgets::elide(ui, chapter, FontId::proportional(12.0), t.muted, text.width());
            ui.add(egui::Label::new(chapter));
        });
        let controls = Rect::from_min_size(pos2(controls_x, top.top() + 8.0), vec2(transport_w, 36.0));
        header.scope_builder(UiBuilder::new().max_rect(controls).layout(Layout::left_to_right(Align::Center)), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let back = format!("Skip back {} seconds", self.playback.skip_back);
            if icons::sized_button(ui, Icon::ChevronLeft, &back, vec2(38.0, 36.0), 17.0, Some(t.muted)).clicked() {
                self.actions.push(Action::Playback(Control::Skip(-f64::from(self.playback.skip_back))));
            }
            let (icon, label) = if self.player.paused || !self.player.active { (Icon::Play, "Resume playback") } else { (Icon::Pause, "Pause playback") };
            if shell::widgets::primary(ui, Some(icon), if self.player.paused || !self.player.active { "Play" } else { "Pause" }).on_hover_text(label).clicked()
            {
                self.actions.push(Action::Playback(Control::Toggle));
            }
            let forward = format!("Skip forward {} seconds", self.playback.skip_forward);
            if icons::sized_button(ui, Icon::ChevronRight, &forward, vec2(38.0, 36.0), 17.0, Some(t.muted)).clicked() {
                self.actions.push(Action::Playback(Control::Skip(f64::from(self.playback.skip_forward))));
            }
        });
        let right = Rect::from_min_max(pos2(top.right() - 88.0, top.top() + 10.0), top.max);
        header.scope_builder(UiBuilder::new().max_rect(right).layout(Layout::right_to_left(Align::Center)), |ui| {
            let speed = shell::widgets::secondary(ui, &format!("{}×", self.player.speed)).on_hover_text("Playback speed");
            egui::Popup::menu(&speed).show(|ui| {
                ui.set_min_width(230.0);
                ui.label(RichText::new("Playback speed").font(FontId::new(13.0, semibold())));
                let mut value = self.player.speed;
                if ui.add(egui::Slider::new(&mut value, 0.5..=3.0).step_by(0.05).suffix("×").text("Speed")).changed() {
                    self.actions.push(Action::Playback(Control::Speed(value)));
                }
                ui.separator();
                let mut prefs = self.playback;
                ui.horizontal(|ui| {
                    ui.label("Skip back");
                    ui.add(egui::DragValue::new(&mut prefs.skip_back).range(1..=300).suffix(" s")).labelled_by(ui.label("seconds back").id);
                });
                ui.horizontal(|ui| {
                    ui.label("Skip forward");
                    ui.add(egui::DragValue::new(&mut prefs.skip_forward).range(1..=300).suffix(" s")).labelled_by(ui.label("seconds forward").id);
                });
                ui.horizontal(|ui| {
                    ui.label("Rewind after 5 minutes paused");
                    ui.add(egui::DragValue::new(&mut prefs.rewind).range(0..=60).suffix(" s")).labelled_by(ui.label("rewind seconds (0 is off)").id);
                });
                if prefs != self.playback {
                    self.actions.push(Action::Prefs(PrefsAction::Playback(prefs)));
                }
                ui.separator();
                if ui.button("Previous chapter").clicked() {
                    self.actions.push(Action::Playback(Control::PreviousChapter));
                    ui.close();
                }
                if ui.button("Next chapter").clicked() {
                    self.actions.push(Action::Playback(Control::NextChapter));
                    ui.close();
                }
                if ui.button("Stop playback").clicked() {
                    self.actions.push(Action::Playback(Control::Stop));
                    ui.close();
                }
            });
        });
        ui.allocate_space(vec2(full.width(), 56.0));
        ui.add_space(6.0);
        let (start, end) = book.timeline.chapter_range(at);
        if let Some(position) = scrubber(ui, at, start, end, &t) {
            self.actions.push(Action::Playback(Control::Seek(position)));
        }
        let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 3.0), Sense::hover());
        paint_line(ui, line, at / book.timeline.duration.max(1.0), &t, 1.0);
        ui.add_space(4.0);
        let (chapter_left, book_left) = book.timeline.remaining(at, self.player.speed);
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Chapter · {} left", crate::fmt::clock(chapter_left))).size(11.0).color(t.muted));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(crate::timeline::left(book_left, self.player.speed)).size(11.0).color(t.muted));
            });
        });
    }
}

fn paint_line(ui: &Ui, rect: Rect, fraction: f64, t: &Tokens, radius: f32) {
    ui.painter().rect_filled(rect, radius, t.hairline);
    let right = rect.left() + rect.width() * fraction.clamp(0.0, 1.0) as f32;
    ui.painter().rect_filled(Rect::from_min_max(rect.min, pos2(right, rect.bottom())), radius, t.accent);
}

fn scrubber(ui: &mut Ui, at: f64, start: f64, end: f64, t: &Tokens) -> Option<f64> {
    let (slot, response) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::click_and_drag());
    response.widget_info(|| egui::WidgetInfo::slider(end > start, at - start, "Chapter position"));
    shell::widgets::focus_ring(ui, &response, slot, 4.0);
    let line = Rect::from_center_size(slot.center(), vec2(slot.width(), 4.0));
    let pointer = response.interact_pointer_pos().map(|p| start + f64::from(((p.x - line.left()) / line.width()).clamp(0.0, 1.0)) * (end - start));
    let shown = if response.dragged() { pointer.unwrap_or(at) } else { at };
    let fraction = (shown - start) / (end - start).max(1.0);
    paint_line(ui, line, fraction, t, 2.0);
    if response.hovered() || response.dragged() {
        ui.painter().circle_filled(pos2(line.left() + line.width() * fraction.clamp(0.0, 1.0) as f32, line.center().y), 5.0, t.accent);
    }
    if response.has_focus() {
        if ui.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
            return Some((at - 5.0).max(start));
        }
        if ui.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
            return Some((at + 5.0).min(end));
        }
    }
    if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Home)) {
        return Some(start);
    }
    if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::End)) {
        return Some(end);
    }
    if response.clicked() || response.drag_stopped() { pointer } else { None }
}
