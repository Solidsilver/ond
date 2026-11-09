//! The title bar, which is also the player, in Ond's toolbar idiom: the
//! transport at the left past the traffic lights; the now-playing capsule in
//! the middle; the speed and sleep pills to its right; the mode buttons, the
//! account and the chapters column's toggle at the right end.
//!
//! The capsule is Ond's now-playing display made for books: the cover, the
//! book over the chapter playing, and a scrubber for the chapter with its
//! times at the ends, the book's hairline under it broken where chapters
//! begin. Unlike Ond's it grows with the window, since a wider scrubber is a
//! finer one for an hour-long chapter: a share of the bar's width up to a
//! width past which it would look stretched, kept as near the window's
//! centre as the controls either side allow.
//!
//! Narrow windows give it the room first: the time left beside the speed
//! goes (the speed pill's tooltip and the book line's still say it), then
//! the account's name, then the chapter buttons (⇧← and ⇧→, and the column,
//! still do their work). Inside the capsule, the book's title goes first,
//! then the cover, and when the scrubber would get short its times move up
//! beside the chapter's name so the line can have the whole width.
//!
//! The bar is the window's title bar too: whatever isn't a control moves the
//! window, and a double-click zooms it, as on Ond's.

use super::controls::{self, Pill, Scrub};
use super::panel::{PanelAction, Tab};
use super::view_ctx::ViewCtx;
use super::{Action, RedeApp, Route};
use crate::player::Control;
use egui::{Align, FontId, Layout, Rect, Sense, Ui, UiBuilder, pos2, vec2};
use shell::icons::{self, Icon};
use shell::theme::{Tokens, semibold};

/// Without the traffic lights (full screen, other platforms), the
/// transport's smallest inset from the window's edge.
const TRANSPORT_INSET: f32 = 14.0;
/// The least room between the capsule and the controls either side, and
/// the window controls' spacing: tighter in a narrow window.
const GAP: f32 = 12.0;
const NARROW_GAP: f32 = 8.0;

/// The transport: Ond's toolbar sizes, the skips large enough for their
/// seconds to read, the chapter buttons smaller and quiet outside them.
const SKIP: f32 = 32.0;
const SKIP_GLYPH: f32 = 22.0;
const PLAY_W: f32 = 36.0;
const PLAY_GLYPH: f32 = 17.0;
const CHAPTER_W: f32 = 24.0;
const CHAPTER_GLYPH: f32 = 11.0;
const BUTTON_H: f32 = 32.0;
const TRANSPORT_SPACING: f32 = 4.0;

/// The capsule: a little taller than Ond's display, for the book line.
const CAPSULE_H: f32 = 46.0;
const CAPSULE_RADIUS: f32 = 7.0;
/// Its share of the bar's width, and the widths it keeps within. Judged on
/// windows 1440, 1920 and 2560 wide: 0.44 of the first is about Ond's
/// display at its widest plus a third, and by the last it has reached the
/// most a scrubber needs and the title still sits comfortably centred.
const CAPSULE_SHARE: f32 = 0.44;
const CAPSULE_MIN: f32 = 180.0;
const CAPSULE_MAX: f32 = 960.0;
/// From this width the capsule has room for the cover, and from the next
/// for the book's title over the chapter.
const COVER_FROM: f32 = 280.0;
const TWO_LINES_FROM: f32 = 360.0;
/// The shortest scrubber with its times beside it; shorter, and the times
/// move up to the chapter's row.
const SCRUB_MIN: f32 = 140.0;
/// The least room for the chapter's name between the two times above the
/// scrubber; with less, only the time left stays.
const FLANKED_MIN: f32 = 70.0;
/// The scrubber and the book line, up from the capsule's bottom edge, and
/// how thick each is drawn: the scrubber in the text colour and thicker, the
/// book a muted hairline, as the Player draws them larger.
const SCRUB_UP: f32 = 12.0;
const BOOK_UP: f32 = 6.0;
const SCRUB_LINE: f32 = 3.0;
const SCRUB_KNOB: f32 = 4.5;
const BOOK_LINE: f32 = 1.5;
const TIME_SIZE: f32 = 9.5;

/// The window controls' buttons and the room between them.
const BUTTON: f32 = 28.0;
const WINDOW_SPACING: f32 = 8.0;
const NARROW_WINDOW_SPACING: f32 = 2.0;
/// Below this window width the account shows as its icon (its name is in
/// the tooltip and the menu), and the gaps tighten.
const ACCOUNT_NAME_FROM: f32 = 900.0;
const PILL_SPACING: f32 = 6.0;

/// How wide the parts that come and go are.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Widths {
    /// The speed and sleep pills; nothing when nothing plays.
    pub pills: f32,
    /// The "4h 57m left at" before the speed pill, with its spacing.
    pub left_text: f32,
}

/// Where the bar's parts go, and which of the optional ones fit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Placement {
    pub capsule: Rect,
    pub transport: Rect,
    pub pills: Rect,
    pub chapters: bool,
    pub left_text: bool,
}

fn transport_w(chapters: bool) -> f32 {
    2.0 * SKIP + PLAY_W + 2.0 * TRANSPORT_SPACING + if chapters { 2.0 * (CHAPTER_W + TRANSPORT_SPACING) } else { 0.0 }
}

/// The capsule's width in a bar `width` wide, before the controls either
/// side have their say.
pub(super) fn capsule_target(width: f32) -> f32 {
    (width * CAPSULE_SHARE).clamp(CAPSULE_MIN, CAPSULE_MAX)
}

/// Lays out the bar: `bar` is the whole bar (the window's width),
/// `free_left` where the transport may start (past the traffic lights) and
/// `window_left` where the window controls begin. The capsule takes its
/// target width when the room allows, centred on the window if it can be
/// and otherwise as near it as the sides let it; the transport and the pills
/// sit midway in the space either side of it, as Ond's transport and output
/// control do.
pub(super) fn place(bar: Rect, free_left: f32, window_left: f32, w: Widths) -> Placement {
    let target = capsule_target(bar.width());
    let gap = if bar.width() < ACCOUNT_NAME_FROM { NARROW_GAP } else { GAP };
    let pills = |text: bool| if w.pills > 0.0 { w.pills + gap + if text { w.left_text } else { 0.0 } } else { 0.0 };
    let room = |chapters: bool, text: bool| (window_left - gap - pills(text)) - (free_left + transport_w(chapters) + gap);
    // The chapter buttons stay while the capsule can keep its cover; the time
    // left stays while it can have all it wants.
    let chapters = room(true, false) >= COVER_FROM.min(target);
    let left_text = w.pills > 0.0 && room(chapters, true) >= target;
    let left_end = free_left + transport_w(chapters) + gap;
    let right_start = window_left - gap - pills(left_text);
    let width = target.min(right_start - left_end).max(0.0);
    let cx =
        if right_start - left_end <= width { (left_end + right_start) / 2.0 } else { bar.center().x.clamp(left_end + width / 2.0, right_start - width / 2.0) };
    let capsule = Rect::from_center_size(pos2(cx, bar.center().y), vec2(width, CAPSULE_H));
    let tw = transport_w(chapters);
    let tx = ((free_left + capsule.left()) / 2.0 - tw / 2.0).clamp(free_left, (capsule.left() - gap - tw).max(free_left));
    let transport = Rect::from_min_max(pos2(tx, bar.top()), pos2(tx + tw, bar.bottom()));
    let pw = pills(left_text) - if w.pills > 0.0 { gap } else { 0.0 };
    let px = ((capsule.right() + window_left) / 2.0 - pw / 2.0).clamp(capsule.right() + gap, (window_left - gap - pw).max(capsule.right() + gap));
    let pills = Rect::from_min_max(pos2(px, bar.top()), pos2(px + pw, bar.bottom()));
    Placement { capsule, transport, pills, chapters, left_text }
}

impl RedeApp {
    /// The bar: in the full window the player and everything else; in the
    /// Player the mode buttons alone.
    pub(super) fn title_bar(&mut self, ui: &mut Ui) {
        let mut full = ui.available_rect_before_wrap();
        shell::csd::set_bar(ui.ctx(), full, None);
        // The bar is the window's title bar too: its empty parts move the
        // window, and a double-click zooms it. Widgets drawn after this take
        // the pointer where they are.
        let whole = full.expand2(vec2(12.0, 0.0));
        let bar = ui.interact(whole, ui.scope_id().with("titlebar"), Sense::click_and_drag());
        if bar.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if bar.double_clicked() {
            shell::chrome::title_double_click(ui.ctx());
        }
        // On macOS the traffic lights sit at the left of the bar; on Linux our
        // own window buttons at whichever end the desktop puts them.
        use shell::csd::{Side, reserved};
        full.max.x -= reserved(ui.ctx(), Side::Right);
        if self.mode != crate::window::Mode::Full {
            ui.scope_builder(UiBuilder::new().max_rect(full).layout(Layout::right_to_left(Align::Center)), |ui| self.view().mode_buttons(ui));
            return;
        }
        let free_left = if shell::chrome::folded(ui.ctx()) {
            whole.left() + shell::chrome::LIGHTS_W
        } else {
            (whole.left() + reserved(ui.ctx(), Side::Left)).max(full.left() + TRANSPORT_INSET)
        };
        let compact = whole.width() < ACCOUNT_NAME_FROM;
        let window_left = ui
            .scope_builder(UiBuilder::new().max_rect(full).layout(Layout::right_to_left(Align::Center)), |ui| {
                self.window_controls(ui, compact);
                ui.min_rect().left()
            })
            .inner;
        let widths = self.view().pill_widths(ui);
        let at = place(whole, free_left, window_left, widths);
        let mut view = self.view();
        ui.scope_builder(UiBuilder::new().max_rect(at.transport).layout(Layout::left_to_right(Align::Center)), |ui| view.transport(ui, at.chapters));
        if widths.pills > 0.0 {
            ui.scope_builder(UiBuilder::new().max_rect(at.pills).layout(Layout::left_to_right(Align::Center)), |ui| view.pills(ui, at.left_text));
        }
        view.capsule(ui, at.capsule);
    }

    /// The chapters column's toggle and the mode buttons, laid out right to
    /// left from the bar's end.
    fn window_controls(&mut self, ui: &mut Ui, compact: bool) {
        let t = Tokens::of(ui);
        ui.spacing_mut().item_spacing.x = if compact { NARROW_WINDOW_SPACING } else { WINDOW_SPACING };
        // In a narrow window the column is a sheet: the button shows whether
        // it's open.
        let shown = self.panel_shown();
        let label = if shown { "Hide chapters (⌥⌘U)" } else { "Show chapters (⌥⌘U)" };
        let toggle = icons::sized_button(ui, Icon::Queue, label, vec2(BUTTON, BUTTON), 14.0, Some(if shown { t.accent } else { t.muted }));
        if shown {
            ui.painter().rect_filled(toggle.rect, 6.0, t.accent.gamma_multiply(0.12));
        }
        if toggle.clicked() {
            self.actions.push(Action::Panel(PanelAction::Toggle));
        }
        self.view().mode_buttons(ui);
    }
}

impl ViewCtx<'_> {
    /// How wide the pills and the time left beside them draw this frame.
    fn pill_widths(&self, ui: &Ui) -> Widths {
        let Some(book) = &self.player.book else { return Widths { pills: 0.0, left_text: 0.0 } };
        let speed = crate::timeline::speed_label(self.player.speed);
        let pills = controls::pill_width(ui, Pill::BAR, None, Some(&speed))
            + PILL_SPACING
            + controls::pill_width(ui, Pill::BAR, Some(Icon::Moon), self.sleep_text().as_deref());
        let text = self.left_text(book);
        let text_w = ui.painter().layout_no_wrap(text, FontId::proportional(12.0), egui::Color32::PLACEHOLDER).size().x;
        Widths { pills, left_text: text_w + PILL_SPACING }
    }

    /// "4h 57m left at", which the speed pill finishes.
    fn left_text(&self, book: &crate::player::Book) -> String {
        let (_, book_left) = book.timeline.remaining(self.player.position, self.player.speed);
        format!("{} at", crate::timeline::span_left(book_left))
    }

    /// Previous chapter (when `chapters`), skip back, play/pause, skip
    /// forward, next chapter. Quiet and unavailable while nothing plays.
    fn transport(&mut self, ui: &mut Ui, chapters: bool) {
        let t = Tokens::of(ui);
        ui.spacing_mut().item_spacing.x = TRANSPORT_SPACING;
        ui.add_enabled_ui(self.player.book.is_some(), |ui| {
            let (back, forward) = (self.playback.skip_back, self.playback.skip_forward);
            let chapter = vec2(CHAPTER_W, BUTTON_H);
            if chapters && icons::sized_button(ui, Icon::Previous, "Previous chapter (⇧←)", chapter, CHAPTER_GLYPH, Some(t.muted)).clicked() {
                self.actions.push(Action::Playback(Control::PreviousChapter));
            }
            if icons::sized_button(ui, Icon::SkipBack(back), &format!("Back {back} seconds"), vec2(SKIP, BUTTON_H), SKIP_GLYPH, Some(t.text)).clicked() {
                self.actions.push(Action::Playback(Control::Skip(-f64::from(back))));
            }
            let (icon, label) = if self.player.paused || !self.player.active { (Icon::Play, "Play (Space)") } else { (Icon::Pause, "Pause (Space)") };
            if icons::sized_button(ui, icon, label, vec2(PLAY_W, BUTTON_H), PLAY_GLYPH, Some(t.text)).clicked() {
                self.actions.push(Action::Playback(Control::Toggle));
            }
            let label = format!("Forward {forward} seconds");
            if icons::sized_button(ui, Icon::SkipForward(forward), &label, vec2(SKIP, BUTTON_H), SKIP_GLYPH, Some(t.text)).clicked() {
                self.actions.push(Action::Playback(Control::Skip(f64::from(forward))));
            }
            if chapters && icons::sized_button(ui, Icon::Next, "Next chapter (⇧→)", chapter, CHAPTER_GLYPH, Some(t.muted)).clicked() {
                self.actions.push(Action::Playback(Control::NextChapter));
            }
        });
    }

    /// The book's time left (when `left_text`), the speed pill that finishes
    /// the sentence, and the sleep pill.
    fn pills(&mut self, ui: &mut Ui, left_text: bool) {
        let Some(book) = &self.player.book else { return };
        let t = Tokens::of(ui);
        ui.spacing_mut().item_spacing.x = PILL_SPACING;
        let text = self.left_text(book);
        if left_text {
            ui.label(egui::RichText::new(&text).size(12.0).color(t.muted));
        }
        let pill = controls::speed_pill(ui, &t, self.player.speed, Pill::BAR);
        // The time left is in the tooltip too, for when there's no room for it.
        let pill = pill.on_hover_text(format!("{text} {}", crate::timeline::speed_label(self.player.speed)));
        self.speed_menu(&pill);
        self.sleep_control(ui, Pill::BAR);
    }

    /// The now-playing capsule in `rect`: what's playing and where in the
    /// chapter, or a quiet line when nothing is.
    fn capsule(&mut self, ui: &mut Ui, rect: Rect) {
        let t = Tokens::of(ui);
        let painter = ui.painter().clone();
        painter.rect(rect, CAPSULE_RADIUS, t.sidebar, egui::Stroke::new(1.0, t.hairline), egui::StrokeKind::Inside);
        let id = ui.scope_id().with("capsule");
        let Some(book) = &self.player.book else {
            let text = if self.opening { "Opening…" } else { "Not playing" };
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, text, FontId::proportional(12.0), t.muted);
            let quiet = ui.interact(rect, id, Sense::hover());
            quiet.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text));
            return;
        };
        let body = ui.interact(rect, id, Sense::click());
        let at = self.player.position;
        let (start, end) = book.timeline.chapter_range(at);
        let chapter = book.timeline.chapter(at).map_or("Audiobook", |i| book.timeline.chapters[i].title.as_str());
        let title = book.item.summary.title.as_str();

        let cover = rect.width() >= COVER_FROM;
        if cover {
            let art = Rect::from_min_size(rect.min, vec2(rect.height(), rect.height()));
            self.art.paint(ui, art.shrink(1.0), book.item.summary.cover.as_deref(), 6.0);
        }
        // The text and the scrubber sit on the capsule's centre, like Ond's,
        // when the same margin fits on both sides of the cover.
        let col_left = if cover { rect.left() + rect.height() + 10.0 } else { rect.left() + 12.0 };
        let col_right = rect.right() - 12.0;
        let reserve = (col_left - rect.left()).max(rect.right() - col_right);
        let (left, right) = if rect.width() - 2.0 * reserve >= 200.0 { (rect.left() + reserve, rect.right() - reserve) } else { (col_left, col_right) };
        let time_font = FontId::proportional(TIME_SIZE);
        // Sized for the widest time, so the line doesn't shift as digits change.
        let widest = if end - start >= 3600.0 { "−8:88:88" } else { "−88:88" };
        let time_w = painter.layout_no_wrap(widest.into(), time_font.clone(), t.muted).size().x;
        let beside = right - left - 2.0 * (time_w + 6.0) >= SCRUB_MIN;
        let (bar_left, bar_right) = if beside { (left + time_w + 6.0, right - time_w - 6.0) } else { (left, right) };
        let scrub_y = rect.bottom() - SCRUB_UP;
        let book_y = rect.bottom() - BOOK_UP;

        // The scrubber's strip reaches up to the text and down to the book
        // line, so it's easy to grab.
        let strip = Rect::from_min_max(pos2(bar_left - 4.0, scrub_y - 6.0), pos2(bar_right + 4.0, (scrub_y + book_y) / 2.0));
        let scrub_resp = ui.interact(strip, id.with("scrub"), Sense::click_and_drag());
        let line = Rect::from_min_max(pos2(bar_left, scrub_y - SCRUB_LINE / 2.0), pos2(bar_right, scrub_y + SCRUB_LINE / 2.0));
        let (shown, seek, _) = Scrub { line, knob: SCRUB_KNOB, ring: strip }.run(ui, &scrub_resp, at, start, end, &t);
        if let Some(position) = seek {
            self.actions.push(Action::Playback(Control::Seek(position)));
        }
        // The readouts follow a drag, so the times say where a release lands.
        let (chapter_left, book_left) = book.timeline.remaining(shown, self.player.speed);
        let left_at = crate::timeline::left(book_left, self.player.speed);
        let book_strip = Rect::from_min_max(pos2(bar_left, strip.bottom()), pos2(bar_right, rect.bottom() - 1.0));
        let book_resp = ui.interact(book_strip, id.with("book"), Sense::hover());
        let book_line = Rect::from_center_size(pos2(book_strip.center().x, book_y), vec2(book_strip.width(), BOOK_LINE));
        controls::paint_book_line(ui, book_resp, book_line, BOOK_LINE, &book.timeline, shown, t.sidebar, Some(&left_at), &t);
        let elapsed = crate::fmt::clock((shown - start).max(0.0));
        let remaining = format!("−{}", crate::fmt::clock(chapter_left));

        // The text above the scrubber: the book over the chapter when there's
        // room, else the chapter alone; when the times couldn't sit beside
        // the scrubber, they flank the chapter here.
        let floor = scrub_y - 4.0;
        let text_w = (right - left).max(40.0);
        let link_font;
        let link_ink;
        let link_y;
        let mut link_w = text_w;
        let mut link_left = None;
        if beside && rect.width() >= TWO_LINES_FROM {
            let title_g = crate::widgets::elide(ui, title, FontId::new(12.5, semibold()), t.text, text_w);
            link_font = FontId::proportional(11.0);
            link_ink = t.muted;
            let sub_h = painter.layout_no_wrap("Ag".into(), link_font.clone(), link_ink).size().y;
            let (y0, sub_y) = title_rows(rect.top(), floor, title_g.size().y, sub_h);
            painter.galley(pos2((left + right) / 2.0 - title_g.size().x / 2.0, y0), title_g, t.text);
            link_y = sub_y;
        } else {
            link_font = FontId::proportional(12.0);
            link_ink = t.text;
            let h = painter.layout_no_wrap("Ag".into(), link_font.clone(), link_ink).size().y;
            link_y = rect.top() + ((floor - rect.top()) - h) / 2.0;
            if !beside {
                let mid = link_y + h / 2.0;
                painter.text(pos2(right, mid), egui::Align2::RIGHT_CENTER, &remaining, time_font.clone(), t.muted);
                if text_w - 2.0 * (time_w + 8.0) >= FLANKED_MIN {
                    painter.text(pos2(left, mid), egui::Align2::LEFT_CENTER, &elapsed, time_font.clone(), t.muted);
                    link_w = text_w - 2.0 * (time_w + 8.0);
                } else {
                    // Tighter still, the chapter keeps its name and the time
                    // it has left; the time it's been playing goes.
                    link_w = text_w - time_w - 8.0;
                    link_left = Some(left);
                }
            }
        }
        if beside {
            painter.text(pos2(bar_left - 6.0, scrub_y), egui::Align2::RIGHT_CENTER, &elapsed, time_font.clone(), t.muted);
            painter.text(pos2(bar_right + 6.0, scrub_y), egui::Align2::LEFT_CENTER, &remaining, time_font, t.muted);
        }
        // The chapter's name opens the chapters column at the one playing.
        let name = crate::widgets::elide(ui, chapter, link_font, link_ink, link_w);
        let name_x = link_left.unwrap_or((left + right) / 2.0 - name.size().x / 2.0);
        let name_rect = Rect::from_min_size(pos2(name_x, link_y), name.size());
        let link = ui.interact(name_rect, id.with("chapter"), Sense::click());
        let ink = if link.hovered() { t.text } else { link_ink };
        painter.galley(name_rect.min, name, ink);
        if link.hovered() {
            let y = name_rect.bottom() - 0.5;
            painter.line_segment([pos2(name_rect.left(), y), pos2(name_rect.right(), y)], egui::Stroke::new(1.0, ink));
        }
        link.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{chapter}, show chapters")));
        if link.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Show chapters").clicked() {
            self.actions.push(Action::Panel(PanelAction::Show(Tab::Chapters)));
        }

        // The rest of the capsule opens the book's page.
        let label = format!("Now playing: {title}, {chapter}, {elapsed} in, {} left", crate::fmt::clock(chapter_left));
        body.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
        if body.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.actions.push(Action::Go(Route::Book(book.key.item.clone())));
        }
    }
}

/// Where the capsule's two lines start, `title_h` and `sub_h` tall, from
/// `top`: 4 below it, unless that runs past `floor`. Then they move up, as
/// far as 1 below the top, and the second line moves up under the title by
/// up to 2. Ond's display sets its titles the same way, for the same reason:
/// Linux's UI font is about a tenth taller than macOS's.
fn title_rows(top: f32, floor: f32, title_h: f32, sub_h: f32) -> (f32, f32) {
    let over = top + 4.0 + title_h + sub_h - floor;
    let lift = over.clamp(0.0, 3.0);
    let tuck = (over - lift).clamp(0.0, 2.0);
    let y0 = top + 4.0 - lift;
    (y0, y0 + title_h - tuck)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bar as drawn in a window `w` wide, the traffic lights at its left
    /// and the window controls taking `controls` at its right.
    fn placed(w: f32, controls: f32, widths: Widths) -> (Rect, Placement) {
        let bar = Rect::from_min_size(pos2(0.0, 0.0), vec2(w, 52.0));
        let free_left = shell::chrome::LIGHTS_W;
        (bar, place(bar, free_left, w - 12.0 - controls, widths))
    }

    const PLAYING: Widths = Widths { pills: 92.0, left_text: 92.0 };
    const CONTROLS: f32 = 230.0;
    /// The account as its icon and the buttons closer together.
    const NARROW_CONTROLS: f32 = 163.0;

    #[test]
    fn the_capsule_grows_with_the_window_up_to_its_widest() {
        let widths: Vec<f32> = [1100.0, 1440.0, 1920.0, 2560.0, 3840.0].iter().map(|&w| placed(w, CONTROLS, PLAYING).1.capsule.width()).collect();
        assert!(widths.windows(2).all(|w| w[0] <= w[1]), "never narrower in a wider window: {widths:?}");
        assert!((widths[1] - 1440.0 * CAPSULE_SHARE).abs() < 0.5, "a share of a 1440 window: {widths:?}");
        assert!((widths[2] - 1920.0 * CAPSULE_SHARE).abs() < 0.5);
        assert_eq!(widths[3], CAPSULE_MAX, "and no wider than it looks good");
        assert_eq!(widths[4], CAPSULE_MAX);
    }

    #[test]
    fn a_wide_window_centres_the_capsule_on_it() {
        for w in [1920.0, 2560.0] {
            let (bar, at) = placed(w, CONTROLS, PLAYING);
            assert!((at.capsule.center().x - bar.center().x).abs() < 0.5, "{w}: {:?}", at.capsule);
            assert!(at.chapters && at.left_text, "everything fits at {w}");
        }
    }

    #[test]
    fn nothing_overlaps_at_any_width() {
        for w in [640.0, 760.0, 900.0, 1100.0, 1440.0, 1920.0, 2560.0] {
            for widths in [PLAYING, Widths { pills: 0.0, left_text: 0.0 }] {
                let controls = if w < ACCOUNT_NAME_FROM { NARROW_CONTROLS } else { CONTROLS };
                let gap = if w < ACCOUNT_NAME_FROM { NARROW_GAP } else { GAP };
                let (_, at) = placed(w, controls, widths);
                let window_left = w - 12.0 - controls;
                assert!(at.transport.left() >= shell::chrome::LIGHTS_W, "{w}: the transport clears the lights");
                assert!(at.transport.right() + gap <= at.capsule.left() + 0.5, "{w}: {at:?}");
                let right = if widths.pills > 0.0 { at.pills.left() } else { window_left };
                assert!(at.capsule.right() + gap <= right + 0.5, "{w}: {at:?}");
                assert!(at.pills.right() <= window_left - gap + 0.5 || widths.pills == 0.0, "{w}: {at:?}");
            }
        }
    }

    #[test]
    fn a_narrow_window_lets_the_extras_go_before_the_capsule_shrinks() {
        let (_, at) = placed(1100.0, CONTROLS, PLAYING);
        assert!(at.chapters, "the chapter buttons stay at the default size");
        assert!(!at.left_text, "the time left goes to the speed's tooltip");
        let (_, at) = placed(640.0, NARROW_CONTROLS, PLAYING);
        assert!(!at.chapters, "the smallest window keeps the chapter buttons to the keyboard and the column");
        assert!(at.capsule.width() >= 150.0, "and the capsule keeps a usable scrubber: {:?}", at.capsule);
    }

    #[test]
    fn the_titles_move_up_only_as_far_as_they_need() {
        assert_eq!(title_rows(0.0, 34.0, 15.0, 13.0), (4.0, 19.0));
        assert_eq!(title_rows(0.0, 31.0, 15.0, 13.0), (3.0, 18.0));
        let (y0, sub_y) = title_rows(0.0, 20.0, 17.0, 15.0);
        assert_eq!((y0, sub_y), (1.0, 16.0));
    }
}
