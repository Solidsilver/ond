//! A quiet bottom strip in the family's player idiom, after Ond's Player:
//! the cover with the book and its author, then the transport centred on the
//! window (chapter back, skip back, a round play/pause, skip forward, chapter
//! on). Under them the chapter scrubber, in the weight of Ond's big seek bar,
//! and a hairline for the whole book, broken where chapters begin, so the
//! two can't be mistaken for each other. The last line says where the
//! chapter is on the left and how long the book has to go on the right,
//! ending in the speed, which is the control that changes it.

use super::view_ctx::ViewCtx;
use super::{Action, PrefsAction, RedeApp};
use crate::player::Control;
use crate::timeline::Timeline;
use egui::{Align, FontId, Layout, Rect, Response, RichText, Sense, Ui, UiBuilder, pos2, vec2};
use shell::{
    icons::{self, Icon},
    theme::{Tokens, semibold},
};

const MARGIN_X: i8 = 24;
const MARGIN_Y: i8 = 10;
const HEADER_H: f32 = 52.0;
/// Ond's small artwork: the same side as the header, its corner radius.
const COVER_RADIUS: f32 = 6.0;
const COVER_GAP: f32 = 12.0;
const HEADER_GAP: f32 = 12.0;
const SCRUB_H: f32 = 16.0;
/// The book line's hover area, and the line inside it.
const BOOK_H: f32 = 8.0;
const BOOK_LINE: f32 = 2.0;
const READOUT_GAP: f32 = 6.0;
const READOUT_H: f32 = 22.0;
const BAR_H: f32 = 2.0 * MARGIN_Y as f32 + HEADER_H + HEADER_GAP + SCRUB_H + BOOK_H + READOUT_GAP + READOUT_H;

/// The round play/pause, the skip buttons and their glyphs, and the smaller
/// chapter buttons either side.
const PLAY_D: f32 = 40.0;
const SKIP: f32 = 36.0;
const SKIP_GLYPH: f32 = 26.0;
const CHAPTER: f32 = 28.0;
const CHAPTER_GLYPH: f32 = 11.0;
const TRANSPORT_GAP: f32 = 8.0;
const TRANSPORT_W: f32 = 2.0 * CHAPTER + 2.0 * SKIP + PLAY_D + 4.0 * TRANSPORT_GAP;
/// Below this the transport moves to the right end, out of the title's way.
const NARROW_W: f32 = 740.0;

/// The speeds the speed menu offers at a click; the slider has the rest.
const SPEEDS: [f32; 6] = [0.8, 1.0, 1.25, 1.5, 1.75, 2.0];

impl RedeApp {
    pub(super) fn player_panel(&mut self, ui: &mut Ui) {
        if self.player.book.is_none() {
            return;
        }
        let t = Tokens::of(ui);
        egui::Panel::bottom("player-bar")
            .exact_size(BAR_H)
            .resizable(false)
            .frame(egui::Frame::new().fill(t.panel).stroke(egui::Stroke::new(1.0, t.hairline)).inner_margin(egui::Margin::symmetric(MARGIN_X, MARGIN_Y)))
            .show(ui, |ui| self.view().player_bar(ui));
    }
}

impl ViewCtx<'_> {
    pub(super) fn player_bar(&mut self, ui: &mut Ui) {
        let Some(book) = &self.player.book else { return };
        let t = Tokens::of(ui);
        let at = self.player.position;
        ui.spacing_mut().item_spacing.y = 0.0;
        let full = ui.available_rect_before_wrap();
        let top = Rect::from_min_size(full.min, vec2(full.width(), HEADER_H));
        let cover = Rect::from_min_size(top.min, vec2(HEADER_H, HEADER_H));
        self.art.paint(ui, cover, book.item.summary.cover.as_deref(), COVER_RADIUS);
        let narrow = full.width() < NARROW_W;
        let controls_x = if narrow { top.right() - TRANSPORT_W } else { top.center().x - TRANSPORT_W / 2.0 };
        let controls = Rect::from_min_size(pos2(controls_x, top.top()), vec2(TRANSPORT_W, HEADER_H));
        let mut header = ui.new_child(UiBuilder::new().id_salt("player-header").max_rect(top));

        // The title over the author, centred on the cover as Ond sets its
        // titles beside the art.
        let text = Rect::from_min_max(pos2(cover.right() + COVER_GAP, top.top()), pos2(controls.left() - 16.0, top.bottom()));
        let title = crate::widgets::elide(ui, &book.item.summary.title, FontId::new(13.0, semibold()), t.text, text.width());
        let author = crate::widgets::elide(ui, &super::library::authors_line(&book.item.summary), FontId::proportional(12.0), t.muted, text.width());
        let block = title.size().y + 2.0 + author.size().y;
        let text = Rect::from_min_size(pos2(text.left(), top.center().y - block / 2.0), vec2(text.width(), block));
        header.scope_builder(UiBuilder::new().max_rect(text).layout(Layout::top_down(Align::Min)), |ui| {
            let response = ui.add(egui::Label::new(title).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand);
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Open playing book"));
            if response.clicked() {
                self.actions.push(Action::Go(super::Route::Book(book.key.item.clone())));
            }
            ui.add_space(2.0);
            ui.add(egui::Label::new(author));
        });

        header.scope_builder(UiBuilder::new().max_rect(controls).layout(Layout::left_to_right(Align::Center)), |ui| {
            ui.spacing_mut().item_spacing.x = TRANSPORT_GAP;
            let (back, forward) = (self.playback.skip_back, self.playback.skip_forward);
            if icons::sized_button(ui, Icon::Previous, "Previous chapter (⇧←)", vec2(CHAPTER, CHAPTER), CHAPTER_GLYPH, Some(t.muted)).clicked() {
                self.actions.push(Action::Playback(Control::PreviousChapter));
            }
            let label = format!("Back {back} seconds");
            if icons::sized_button(ui, Icon::SkipBack(back), &label, vec2(SKIP, SKIP), SKIP_GLYPH, Some(t.text)).clicked() {
                self.actions.push(Action::Playback(Control::Skip(-f64::from(back))));
            }
            if play_pause(ui, &t, self.player.paused || !self.player.active).clicked() {
                self.actions.push(Action::Playback(Control::Toggle));
            }
            let label = format!("Forward {forward} seconds");
            if icons::sized_button(ui, Icon::SkipForward(forward), &label, vec2(SKIP, SKIP), SKIP_GLYPH, Some(t.text)).clicked() {
                self.actions.push(Action::Playback(Control::Skip(f64::from(forward))));
            }
            if icons::sized_button(ui, Icon::Next, "Next chapter (⇧→)", vec2(CHAPTER, CHAPTER), CHAPTER_GLYPH, Some(t.muted)).clicked() {
                self.actions.push(Action::Playback(Control::NextChapter));
            }
        });

        ui.allocate_space(vec2(full.width(), HEADER_H));
        ui.add_space(HEADER_GAP);
        let (start, end) = book.timeline.chapter_range(at);
        let (shown, seek) = scrubber(ui, at, start, end, &t);
        if let Some(position) = seek {
            self.actions.push(Action::Playback(Control::Seek(position)));
        }
        book_line(ui, &book.timeline, shown, &t);
        ui.add_space(READOUT_GAP);

        // The readouts follow a drag, so the times say where a release lands.
        let speed = self.player.speed;
        let (chapter_left, book_left) = book.timeline.remaining(shown, speed);
        let chapter = book.timeline.chapter(shown).map(|i| book.timeline.chapters[i].title.as_str()).unwrap_or("Audiobook");
        let (row, row_resp) = ui.allocate_exact_size(vec2(full.width(), READOUT_H), Sense::hover());
        let elapsed = crate::fmt::clock((shown - start).max(0.0));
        let left = crate::fmt::clock(chapter_left);
        row_resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, format!("{chapter}, {elapsed} in, {left} left")));
        let mut readout = ui.new_child(UiBuilder::new().id_salt("player-readout").max_rect(row).layout(Layout::right_to_left(Align::Center)));
        readout.spacing_mut().item_spacing.x = 6.0;
        let pill = speed_pill(&mut readout, &t, speed);
        self.speed_menu(&pill);
        readout.label(RichText::new(format!("{} at", crate::timeline::span_left(book_left))).size(12.0).color(t.muted));
        self.sleep_menu(&mut readout);
        let room = readout.available_width() - 24.0;
        let times = format!("  {elapsed} · {left} left");
        let times_w = ui.painter().layout_no_wrap(times.clone(), FontId::proportional(12.0), t.muted).size().x;
        let name = crate::widgets::elide(ui, chapter, FontId::proportional(12.0), t.text, (room - times_w).max(40.0));
        let name_w = name.size().x;
        let y = row.center().y;
        ui.painter().galley(pos2(row.left(), y - name.size().y / 2.0), name, t.text);
        if name_w + times_w <= room {
            ui.painter().text(pos2(row.left() + name_w, y), egui::Align2::LEFT_CENTER, times, FontId::proportional(12.0), t.muted);
        }
    }

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
                for (label, value, range) in [
                    ("Skip back", &mut prefs.skip_back, 1..=300),
                    ("Skip forward", &mut prefs.skip_forward, 1..=300),
                    ("Rewind after a long pause", &mut prefs.rewind, 0..=60),
                    ("Sleep timer duration", &mut prefs.sleep_minutes, 5..=120),
                    ("Rewind after sleep", &mut prefs.sleep_rewind, 0..=60),
                ] {
                    let name = ui.label(label).id;
                    ui.add(egui::DragValue::new(value).range(range).suffix(" s")).labelled_by(name);
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

/// The primary control, as Ond's Player draws it: a round button in the
/// text colour with the glyph knocked out in the background's.
pub(super) fn play_pause(ui: &mut Ui, t: &Tokens, paused: bool) -> Response {
    let (icon, label) = if paused { (Icon::Play, "Play") } else { (Icon::Pause, "Pause") };
    let (slot, resp) = ui.allocate_exact_size(vec2(PLAY_D, PLAY_D), Sense::click());
    let fill = if resp.hovered() { t.text.gamma_multiply(0.85) } else { t.text };
    ui.painter().circle_filled(slot.center(), PLAY_D / 2.0, fill);
    shell::widgets::focus_ring(ui, &resp, slot, PLAY_D / 2.0);
    // A triangle's weight sits left of its box's centre; nudged, it looks centred.
    let nudge = if paused { vec2(1.5, 0.0) } else { vec2(0.0, 0.0) };
    icons::paint(ui.painter(), Rect::from_center_size(slot.center() + nudge, vec2(16.0, 16.0)), icon, t.bg);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(format!("{label} (Space)"))
}

/// The speed as a small pill with a chevron, so it reads as the menu it opens.
pub(super) fn speed_pill(ui: &mut Ui, t: &Tokens, speed: f32) -> Response {
    let text = crate::timeline::speed_label(speed);
    let galley = ui.painter().layout_no_wrap(text.clone(), FontId::new(12.0, semibold()), t.text);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 32.0, READOUT_H), Sense::click());
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    ui.painter().rect_filled(rect, rect.height() / 2.0, if resp.hovered() || open { t.selected } else { t.field });
    shell::widgets::focus_ring(ui, &resp, rect, rect.height() / 2.0);
    ui.painter().galley(pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0), galley, t.text);
    icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.right() - 12.0, rect.center().y + 0.5), vec2(8.0, 8.0)), Icon::ChevronDown, t.muted);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Playback speed, {text}")));
    resp.on_hover_text("Playback speed ([ and ])")
}

/// The whole book as a hairline under the scrubber: muted, not the text
/// colour, and broken where each chapter begins, so it reads as the book's
/// chapters rather than a second seek bar. It doesn't seek.
pub(super) fn book_line(ui: &mut Ui, timeline: &Timeline, at: f64, t: &Tokens) {
    let (slot, resp) = ui.allocate_exact_size(vec2(ui.available_width(), BOOK_H), Sense::hover());
    let line = Rect::from_center_size(slot.center(), vec2(slot.width(), BOOK_LINE));
    let duration = timeline.duration.max(1.0);
    let fraction = (at / duration).clamp(0.0, 1.0);
    let painter = ui.painter();
    painter.rect_filled(line, 1.0, t.hairline);
    painter.rect_filled(Rect::from_min_max(line.min, pos2(line.left() + line.width() * fraction as f32, line.bottom())), 1.0, t.muted);
    // Breaks only where they'd stand apart; a book of very short chapters
    // would otherwise turn the line into dashes.
    if timeline.chapters.len() > 1 && line.width() / timeline.chapters.len() as f32 >= 6.0 {
        for chapter in &timeline.chapters[1..] {
            let x = line.left() + line.width() * (chapter.start / duration) as f32;
            painter.rect_filled(Rect::from_center_size(pos2(x, line.center().y), vec2(2.0, BOOK_LINE + 1.0)), 0.0, t.panel);
        }
    }
    resp.on_hover_text(format!("{}% of the book", (fraction * 100.0).floor()));
}

/// The chapter scrubber, after Ond's big seek bar: a 4 pt line in a taller
/// strip that's easy to grab, played part in the text colour, a knob while
/// it's hovered or held. Returns the position to show (the pointer's while
/// dragging) and where to seek, if anywhere.
pub(super) fn scrubber(ui: &mut Ui, at: f64, start: f64, end: f64, t: &Tokens) -> (f64, Option<f64>) {
    let (slot, response) = ui.allocate_exact_size(vec2(ui.available_width(), SCRUB_H), Sense::click_and_drag());
    shell::widgets::focus_ring(ui, &response, slot, 4.0);
    let line = Rect::from_center_size(slot.center(), vec2(slot.width(), 4.0));
    let pointer = response.interact_pointer_pos().map(|p| start + f64::from(((p.x - line.left()) / line.width()).clamp(0.0, 1.0)) * (end - start));
    let shown = if response.dragged() { pointer.unwrap_or(at) } else { at };
    let fraction = ((shown - start) / (end - start).max(1.0)).clamp(0.0, 1.0);
    let x = line.left() + line.width() * fraction as f32;
    ui.painter().rect_filled(line, 2.0, t.hairline);
    ui.painter().rect_filled(Rect::from_min_max(line.min, pos2(x, line.bottom())), 2.0, t.text);
    if response.hovered() || response.dragged() {
        ui.painter().circle_filled(pos2(x, line.center().y), 6.0, t.text);
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
    (shown, seek)
}
