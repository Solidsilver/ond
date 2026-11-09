//! The Player and Mini window modes, in the title bar player's language at
//! other sizes.
//!
//! The Player is for reading across the room: the cover as large as the
//! window allows, the book and chapter under it, the chapter scrubber over
//! the whole-book hairline with the chapter's times at its ends, the
//! transport, and a last row with the sleep timer and the book's time left
//! at the speed it plays. Everything under the cover has a fixed height, so
//! the cover takes what's left and nothing moves as the readouts change.
//!
//! Mini is Ond's Mini: the cover fills the square window, the mode buttons
//! sit on a dark capsule in its top corner and the chapter and transport on a
//! scrim along its foot, in light colours that read on any cover. Unlike
//! Ond's they don't wait for the pointer: they are all of Mini's controls.

use super::controls::{self, Pill};
use super::{Action, RedeApp, view_ctx::ViewCtx};
use crate::player::Control;
use egui::{Align, Color32, FontId, Layout, Rect, Sense, Ui, UiBuilder, pos2, vec2};
use shell::{
    icons::{self, Icon},
    over_art::{self, OnArt},
    theme::{Tokens, semibold},
};

/// The Player window's margins and the gaps down its column.
const PLAYER_MARGIN_X: i8 = 28;
const PLAYER_MARGIN_TOP: i8 = 14;
const PLAYER_MARGIN_BOTTOM: i8 = 20;
const COVER_GAP: f32 = 22.0;
const TITLE_SIZE: f32 = 24.0;
const CHAPTER_SIZE: f32 = 17.0;
const TITLE_GAP: f32 = 4.0;
const SCRUB_GAP: f32 = 20.0;
/// The chapter's times under the scrubber, large enough to read from a chair.
const TIMES_SIZE: f32 = 15.0;
const TIMES_GAP: f32 = 4.0;
const TIMES_H: f32 = 20.0;
const TRANSPORT_GAP: f32 = 16.0;
const TRANSPORT_H: f32 = 64.0;
const CONTROLS_GAP: f32 = 18.0;
const NOTICE_GAP: f32 = 14.0;
/// The scrubber's and the book line's strips, as `controls` draws them.
const SCRUB_H: f32 = 16.0 + 8.0;
const COVER_MIN: f32 = 120.0;

/// The Player's transport: as Ond's Player sets its own, a round play/pause
/// with the skips beside it and the chapter buttons, smaller, outside them.
const PLAY_D: f32 = 64.0;
const SKIP: f32 = 52.0;
const SKIP_GLYPH: f32 = 36.0;
const CHAPTER: f32 = 40.0;
const CHAPTER_GLYPH: f32 = 15.0;
const PLAYER_TRANSPORT_GAP: f32 = 16.0;

/// Mini's scrim along the foot of the cover, its padding, and the transport
/// on it; the capsule of mode buttons in the corner.
const MINI_PANEL_H: f32 = 112.0;
const MINI_PAD_X: f32 = 14.0;
const MINI_PAD_Y: f32 = 10.0;
const MINI_PLAY_D: f32 = 40.0;
const MINI_SKIP: f32 = 36.0;
const MINI_SKIP_GLYPH: f32 = 25.0;
const MINI_TRANSPORT_GAP: f32 = 14.0;
const MINI_CORNER: f32 = 8.0;
const MINI_BUTTON: f32 = 26.0;

/// Text and icons over the cover, on the scrim, whatever the theme: Ond's.
const ON_ART: Color32 = over_art::LIGHT;
const ON_ART_MUTED: Color32 = Color32::from_rgb(0xC9, 0xCC, 0xD2);
/// Beside a cover that isn't square: dark, as the window around a picture.
const MINI_BACKDROP: Color32 = Color32::from_gray(22);

fn scrim() -> Color32 {
    Color32::from_rgba_unmultiplied(12, 13, 15, 204)
}

impl RedeApp {
    pub(super) fn player_window(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let margin = egui::Margin { left: PLAYER_MARGIN_X, right: PLAYER_MARGIN_X, top: PLAYER_MARGIN_TOP, bottom: PLAYER_MARGIN_BOTTOM };
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg).inner_margin(margin)).show(ui, |ui| self.view().large_player(ui));
    }
    pub(super) fn mini_window(&mut self, ui: &mut Ui) {
        egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ui, |ui| self.view().mini_player(ui));
    }
}

impl ViewCtx<'_> {
    fn large_player(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let Some(book) = &self.player.book else {
            ui.centered_and_justified(|ui| ui.label(egui::RichText::new("Choose a book from the library.").size(14.0).color(t.muted)));
            return;
        };
        let full = ui.available_rect_before_wrap();
        let notice = self.sleep_notice_height(ui, full.width()).map_or(0.0, |h| h + NOTICE_GAP);
        let title = crate::widgets::elide(ui, &book.item.summary.title, FontId::new(TITLE_SIZE, semibold()), t.text, full.width());
        let chapter_name = book.timeline.chapter(self.player.position).map(|i| book.timeline.chapters[i].title.as_str()).unwrap_or("Audiobook");
        let chapter = crate::widgets::elide(ui, chapter_name, FontId::proportional(CHAPTER_SIZE), t.muted, full.width());
        let below = COVER_GAP
            + title.size().y
            + TITLE_GAP
            + chapter.size().y
            + SCRUB_GAP
            + SCRUB_H
            + TIMES_GAP
            + TIMES_H
            + TRANSPORT_GAP
            + TRANSPORT_H
            + CONTROLS_GAP
            + Pill::ROOM.height
            + notice;
        let side = full.width().min(full.height() - below).max(COVER_MIN).floor();
        // Whatever the cover can't use is shared above and below the column,
        // so a wide window keeps it centred rather than piling space at the foot.
        let spare = (full.height() - below - side).max(0.0);
        ui.add_space(spare / 2.0);
        ui.spacing_mut().item_spacing.y = 0.0;

        ui.vertical_centered(|ui| {
            let (cover, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
            let shadow = egui::Shadow {
                offset: [0, (side / 40.0) as i8],
                blur: (side / 14.0).min(28.0) as u8,
                spread: 0,
                color: Color32::from_black_alpha(if ui.visuals().dark_mode { 110 } else { 46 }),
            };
            ui.painter().add(shadow.as_shape(cover, 8));
            self.art.paint(ui, cover, book.item.summary.cover.as_deref(), 8);
            ui.add_space(COVER_GAP);
            ui.add(egui::Label::new(title).selectable(false));
            ui.add_space(TITLE_GAP);
            ui.add(egui::Label::new(chapter).selectable(false));
        });
        ui.add_space(SCRUB_GAP);

        let at = self.player.position;
        let (start, end) = book.timeline.chapter_range(at);
        let (shown, seek) = controls::scrubber(ui, at, start, end, &t);
        if let Some(at) = seek {
            self.actions.push(Action::Playback(Control::Seek(at)));
        }
        controls::book_line(ui, &book.timeline, shown, &t);
        ui.add_space(TIMES_GAP);

        // The chapter's times at the scrubber's ends, in the capsule's words; the
        // book's time left goes by the speed it depends on, below.
        let (chapter_left, book_left) = book.timeline.remaining(shown, self.player.speed);
        let elapsed = crate::fmt::clock((shown - start).max(0.0));
        let left = format!("{} left", crate::fmt::clock(chapter_left));
        let (row, resp) = ui.allocate_exact_size(vec2(full.width(), TIMES_H), Sense::hover());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, format!("{chapter_name}, {elapsed} in, {left}")));
        let font = FontId::proportional(TIMES_SIZE);
        ui.painter().text(row.left_center(), egui::Align2::LEFT_CENTER, elapsed, font.clone(), t.muted);
        ui.painter().text(row.right_center(), egui::Align2::RIGHT_CENTER, left, font, t.muted);

        ui.add_space(TRANSPORT_GAP);
        self.large_transport(ui, &t);
        ui.add_space(CONTROLS_GAP);
        self.room_controls(ui, &t, book_left);
        if notice > 0.0 {
            let within = Rect::from_min_max(full.min, pos2(full.right(), full.bottom() + f32::from(PLAYER_MARGIN_BOTTOM) - 2.0));
            self.sleep_notice(ui, within);
        }
    }

    /// The sleep pill, then the book's time left at the speed in its pill,
    /// centred as one group under the transport.
    fn room_controls(&mut self, ui: &mut Ui, t: &Tokens, book_left: f64) {
        let size = Pill::ROOM;
        let speed = self.player.speed;
        let text = format!("{} at", crate::timeline::span_left(book_left));
        let font = FontId::proportional(size.font);
        let text_w = ui.painter().layout_no_wrap(text.clone(), font.clone(), t.muted).size().x;
        let sleep_w = controls::pill_width(ui, size, Some(Icon::Moon), self.sleep_text().as_deref());
        let speed_w = controls::pill_width(ui, size, None, Some(&crate::timeline::speed_label(speed)));
        let (group_gap, gap) = (14.0, 6.0);
        let total = sleep_w + group_gap + text_w + gap + speed_w;
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), size.height), Sense::hover());
        let start = row.left() + ((row.width() - total) / 2.0).max(0.0);
        let mut line = ui.new_child(
            UiBuilder::new()
                .id_salt("room-controls")
                .max_rect(Rect::from_min_max(pos2(start, row.top()), row.max))
                .layout(Layout::left_to_right(Align::Center)),
        );
        line.spacing_mut().item_spacing.x = gap;
        self.sleep_control(&mut line, size);
        line.add_space(group_gap - gap);
        line.label(egui::RichText::new(text).font(font).color(t.muted));
        let pill = controls::speed_pill(&mut line, t, speed, size);
        self.speed_menu(&pill);
    }

    fn large_transport(&mut self, ui: &mut Ui, t: &Tokens) {
        let width = 2.0 * CHAPTER + 2.0 * SKIP + PLAY_D + 4.0 * PLAYER_TRANSPORT_GAP;
        let (slot, _) = ui.allocate_exact_size(vec2(ui.available_width(), TRANSPORT_H), Sense::hover());
        let rect = Rect::from_center_size(slot.center(), vec2(width, TRANSPORT_H));
        ui.scope_builder(UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Center)), |ui| {
            ui.spacing_mut().item_spacing.x = PLAYER_TRANSPORT_GAP;
            if icons::sized_button(ui, Icon::Previous, "Previous chapter (⇧←)", vec2(CHAPTER, CHAPTER), CHAPTER_GLYPH, Some(t.muted)).clicked() {
                self.actions.push(Action::Playback(Control::PreviousChapter));
            }
            self.skips_and_play(ui, t.text, t.bg, PLAY_D, SKIP, SKIP_GLYPH);
            if icons::sized_button(ui, Icon::Next, "Next chapter (⇧→)", vec2(CHAPTER, CHAPTER), CHAPTER_GLYPH, Some(t.muted)).clicked() {
                self.actions.push(Action::Playback(Control::NextChapter));
            }
        });
    }

    /// Skip back, the round play/pause in `fg` with its glyph knocked out in
    /// `knock`, and skip forward.
    fn skips_and_play(&mut self, ui: &mut Ui, fg: Color32, knock: Color32, play: f32, skip: f32, glyph: f32) {
        let back = self.playback.skip_back;
        if icons::sized_button(ui, Icon::SkipBack(back), &format!("Back {back} seconds"), vec2(skip, skip), glyph, Some(fg)).clicked() {
            self.actions.push(Action::Playback(Control::Skip(-f64::from(back))));
        }
        if controls::round_play(ui, play, fg, knock, self.player.paused || !self.player.active).clicked() {
            self.actions.push(Action::Playback(Control::Toggle));
        }
        let forward = self.playback.skip_forward;
        if icons::sized_button(ui, Icon::SkipForward(forward), &format!("Forward {forward} seconds"), vec2(skip, skip), glyph, Some(fg)).clicked() {
            self.actions.push(Action::Playback(Control::Skip(f64::from(forward))));
        }
    }

    fn mini_player(&mut self, ui: &mut Ui) {
        let full = ui.max_rect();
        let book = self.player.book.as_ref();
        self.art.paint_on(ui, full, book.and_then(|b| b.item.summary.cover.as_deref()), 0, MINI_BACKDROP);
        // No title bar: the cover moves the window, and a double-click goes
        // back. The controls drawn after it take the pointer where they are.
        let drag = ui.interact(full, ui.scope_id().with("mini-drag"), Sense::click_and_drag());
        drag.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Move Mini player; double-click to return"));
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            self.actions.push(Action::ToggleMini);
        }

        let mut ui = ui.new_child(UiBuilder::new().max_rect(full));
        OnArt::plain(ON_ART).apply(&mut ui);
        let panel = Rect::from_min_max(pos2(full.left(), full.bottom() - MINI_PANEL_H), full.max);
        ui.painter().rect_filled(panel, 0.0, scrim());
        let inner = panel.shrink2(vec2(MINI_PAD_X, MINI_PAD_Y));

        // The chapter over the book, as Ond's Mini sets the title over the artist.
        let (title, subtitle, progress) = match book {
            Some(b) => {
                let at = self.player.position;
                let chapter = b.timeline.chapter(at).map(|i| b.timeline.chapters[i].title.as_str()).unwrap_or("Audiobook");
                let (start, end) = b.timeline.chapter_range(at);
                (chapter, b.item.summary.title.as_str(), Some(((at - start) / (end - start).max(1.0)).clamp(0.0, 1.0) as f32))
            }
            None => ("Choose a book", "", None),
        };
        let title = crate::widgets::elide(&ui, title, FontId::new(13.0, semibold()), ON_ART, inner.width());
        let subtitle = crate::widgets::elide(&ui, subtitle, FontId::proportional(11.5), ON_ART_MUTED, inner.width());
        let title_h = title.size().y;
        ui.painter().galley(inner.left_top(), title, ON_ART);
        ui.painter().galley(pos2(inner.left(), inner.top() + title_h + 1.0), subtitle, ON_ART_MUTED);
        // Where the chapter is, as a line Ond's Mini shows at rest; the
        // scrubber is the Player's.
        if let Some(fraction) = progress {
            let y = inner.top() + 46.0;
            let line = Rect::from_min_max(pos2(inner.left(), y - 1.0), pos2(inner.right(), y + 1.0));
            ui.painter().rect_filled(line, 1.0, ON_ART.gamma_multiply(0.22));
            ui.painter().rect_filled(Rect::from_min_max(line.min, pos2(line.left() + line.width() * fraction, line.bottom())), 1.0, ON_ART.gamma_multiply(0.9));
        }
        let width = 2.0 * MINI_SKIP + MINI_PLAY_D + 2.0 * MINI_TRANSPORT_GAP;
        let row = Rect::from_center_size(pos2(inner.center().x, inner.bottom() - MINI_PLAY_D / 2.0 + 2.0), vec2(width, MINI_PLAY_D));
        ui.scope_builder(UiBuilder::new().max_rect(row).layout(Layout::left_to_right(Align::Center)), |ui| {
            ui.spacing_mut().item_spacing.x = MINI_TRANSPORT_GAP;
            self.skips_and_play(ui, ON_ART, scrim().to_opaque(), MINI_PLAY_D, MINI_SKIP, MINI_SKIP_GLYPH);
        });

        // The mode buttons on a capsule in the corner, clear of the window's edge.
        let count = 3.0;
        let capsule = Rect::from_min_size(
            pos2(full.right() - MINI_CORNER - count * MINI_BUTTON - 4.0, full.top() + MINI_CORNER),
            vec2(count * MINI_BUTTON + 4.0, MINI_BUTTON + 4.0),
        );
        ui.painter().rect_filled(capsule, capsule.height() / 2.0, scrim());
        ui.scope_builder(UiBuilder::new().max_rect(capsule.shrink(2.0)).layout(Layout::right_to_left(Align::Center)), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            self.mode_buttons_sized(ui, MINI_BUTTON, 11.0, Some(ON_ART));
        });

        self.sleep_notice(&ui, Rect::from_min_max(full.min, pos2(full.right(), panel.top() + 4.0)));
    }
}
