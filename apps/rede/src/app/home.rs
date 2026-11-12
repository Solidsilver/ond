//! Home: where the listening is. "Continue listening" first, the books in
//! progress as cards (cover, title, author, how far along, the time left at
//! the speed, and a button that resumes), the one listened to last first;
//! then "Recently added", the library's newest in a row or two of its own
//! tiles.
//!
//! Both lists are worked out once per change to the library or the
//! positions ([`Books::in_progress`](super::Books::in_progress),
//! [`Books::recent`](super::Books::recent)), so a frame only draws.

use super::Action;
use super::library::{COVER_RADIUS, author_of, authors_line, grid_metrics, stand_in};
use super::nav::Route;
use super::progress::Listened;
use super::view_ctx::ViewCtx;
use crate::widgets::elide;
use egui::text::{LayoutJob, TextWrapping};
use egui::{FontId, Rect, RichText, Sense, Ui, pos2, vec2};
use shell::icons::Icon;
use shell::theme::{Tokens, semibold};

/// A card is never narrower than this; as many fit across as the width
/// allows, each as wide as it then can be up to [`CARD_MAX`].
const CARD_MIN: f32 = 280.0;
const CARD_MAX: f32 = 420.0;
const CARD_PAD: f32 = 12.0;
/// The cover's side on a card: larger than nothing else on the page needs,
/// smaller than a grid tile's, so the text beside it has room.
const CARD_COVER: f32 = 104.0;
const CARD_H: f32 = CARD_COVER + 2.0 * CARD_PAD;
const CARD_GAP: f32 = 16.0;
const CARD_RADIUS: f32 = 10.0;
/// Rows of cards at most; the rest are in the library, with their progress
/// on their covers.
const CARD_ROWS: usize = 2;
/// A narrow window's single column still shows this many: two would hide
/// most of what's in progress.
const CARDS_AT_LEAST: usize = 4;
/// Rows of the library's tiles under "Recently added".
const RECENT_ROWS: usize = 2;
/// The resume button's side.
const PLAY: f32 = 32.0;

impl ViewCtx<'_> {
    pub(super) fn home_page(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        shell::kit::page_heading(ui, "Home", None);
        ui.add_space(12.0);
        if self.books.len() == 0 {
            self.no_books(ui);
            return;
        }
        egui::ScrollArea::vertical().id_salt("home").auto_shrink(false).show(ui, |ui| {
            let in_progress = self.books.in_progress(self.positions);
            let note = (!in_progress.is_empty()).then(|| format!("{} in progress", in_progress.len()));
            section_heading(ui, &t, "Continue listening", note);
            if in_progress.is_empty() {
                ui.label(RichText::new("Nothing in progress. Start a book and it shows up here.").size(13.0).color(t.muted));
            } else {
                self.continue_cards(ui, &in_progress);
            }
            ui.add_space(32.0);
            section_heading(ui, &t, "Recently added", None);
            let recent = self.books.recent();
            let (cols, tile) = grid_metrics(ui.available_width());
            for row in recent.chunks(cols).take(RECENT_ROWS) {
                self.book_row(ui, row, tile, tile, None);
            }
        });
    }

    /// The books in progress as cards, as many across as fit, in up to
    /// [`CARD_ROWS`] rows (or [`CARDS_AT_LEAST`] cards, where fewer fit across).
    fn continue_cards(&mut self, ui: &mut Ui, ids: &[usize]) {
        let width = ui.available_width();
        let (cols, card_w) = card_metrics(width);
        let shown = (cols * CARD_ROWS).max(CARDS_AT_LEAST).min(ids.len());
        for row in ids[..shown].chunks(cols) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = CARD_GAP;
                for &i in row {
                    self.continue_card(ui, i, card_w);
                }
            });
            ui.add_space(CARD_GAP - ui.spacing().item_spacing.y);
        }
    }

    /// One book in progress: its cover, title and author (with the
    /// download's mark), the bar and the time left at the speed, and a
    /// button that resumes it (or pauses it while it's the one playing).
    /// Offline, a book that isn't all here is faded. A click elsewhere opens
    /// the book; a right-click has its commands.
    fn continue_card(&mut self, ui: &mut Ui, idx: usize, w: f32) {
        let t = Tokens::of(ui);
        let (rect, resp) = ui.allocate_exact_size(vec2(w, CARD_H), Sense::click());
        let book = self.books.all()[idx].clone();
        let listened = self.listened(&book);
        let left = match listened {
            Listened::InProgress { left: Some(left), .. } => crate::timeline::left(left, self.playback.speed),
            _ => listened.line().unwrap_or_default(),
        };
        let sounding = self.player.active && !self.player.paused && self.player.book.as_ref().is_some_and(|b| b.key.item == book.id);
        let download = self.download(&book.id);
        let dim = self.unplayable(&book.id);
        let opacity = ui.opacity();
        if dim {
            ui.multiply_opacity(super::downloads_page::DIMMED);
        }
        if ui.is_rect_visible(rect) {
            let fill = if resp.is_pointer_button_down_on() {
                t.selected
            } else if resp.hovered() {
                t.hover.lerp_to_gamma(t.selected, 0.5)
            } else {
                t.field.gamma_multiply(0.6)
            };
            ui.painter().rect_filled(rect, CARD_RADIUS, fill);
            shell::kit::focus_ring(ui, &resp, rect, CARD_RADIUS);
            let cover = Rect::from_min_size(rect.min + vec2(CARD_PAD, CARD_PAD), vec2(CARD_COVER, CARD_COVER));
            self.art.paint(ui, cover, book.cover.as_deref(), COVER_RADIUS, &stand_in(&book));
            let x = cover.right() + 14.0;
            let room = (rect.right() - CARD_PAD - x).max(20.0);
            let title = ui.painter().layout_job(two_lines(&book.title, FontId::new(14.0, semibold()), t.text, room));
            let title_h = title.size().y;
            ui.painter().galley(pos2(x, cover.top() + 2.0), title, t.text);
            let author = author_of(&book);
            let author = if author.is_empty() { "Unknown author" } else { author };
            let mark_room = if download.is_some() { 18.0 } else { 0.0 };
            let author = elide(ui, author, FontId::proportional(12.0), t.muted, room - mark_room);
            let author_y = cover.top() + 2.0 + title_h + 3.0;
            let author_h = author.size().y;
            ui.painter().galley(pos2(x, author_y), author, t.muted);
            if let Some(d) = &download {
                shell::kit::download_mark(ui, pos2(rect.right() - CARD_PAD - 6.0, author_y + author_h / 2.0), 11.0, d.mark());
            }
            // The foot: the bar over the time left, beside the button.
            let bar_room = (room - PLAY - 12.0).max(20.0);
            let text = elide(ui, &left, FontId::proportional(11.5), t.muted, bar_room);
            let text_y = cover.bottom() - text.size().y;
            let bar = Rect::from_min_size(pos2(x, text_y - 10.0), vec2(bar_room, 4.0));
            if let Listened::InProgress { fraction: Some(f), .. } = listened {
                shell::kit::progress_bar(ui.painter(), bar, f, &t);
            }
            ui.painter().galley(pos2(x, text_y), text, t.muted);
        }
        let mut name = format!("{} by {}, {left}", book.title, authors_line(&book));
        if dim {
            name.push_str(", not downloaded");
        } else if let Some(d) = &download {
            name.push_str(&format!(", {}", d.mark().hint().to_lowercase()));
        }
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
        let play = Rect::from_center_size(pos2(rect.right() - CARD_PAD - PLAY / 2.0, rect.bottom() - CARD_PAD - PLAY / 2.0 + 4.0), vec2(PLAY, PLAY));
        let (icon, label) = if sounding { (Icon::Pause, format!("Pause {}", book.title)) } else { (Icon::Play, format!("Resume {}", book.title)) };
        // Always shown: it's what the card is for, and a hover-only button
        // can't be found with the keyboard.
        // Offline, a book that isn't here still takes the press, and says
        // why it can't play.
        let resume = !self.opening && shell::kit::tile_button(ui, &t, play, ui.scope_id().with(("resume", &book.id)), true, icon, &label);
        ui.set_opacity(opacity);
        let mut chosen = None;
        resp.context_menu(|ui| {
            ui.set_width(super::library::BOOK_MENU_W);
            chosen = self.book_commands(ui, &book.id);
        });
        if let Some(action) = chosen {
            self.actions.push(action);
        } else if resume {
            self.actions.push(if sounding { Action::Playback(crate::player::Control::Toggle) } else { Action::PlayBook(book.id.clone()) });
        } else if resp.clicked() {
            self.actions.push(Action::Go(Route::Book(book.id.clone())));
        }
    }
}

/// Columns and card width for `width`: as many cards as fit at
/// [`CARD_MIN`], none wider than [`CARD_MAX`] beside another. A lone column
/// takes the whole width, so a narrow window has no ragged gap at the right.
pub fn card_metrics(width: f32) -> (usize, f32) {
    let cols = (((width + CARD_GAP) / (CARD_MIN + CARD_GAP)).floor() as usize).max(1);
    if cols == 1 {
        return (1, width.max(120.0));
    }
    let w = ((width - CARD_GAP * (cols - 1) as f32) / cols as f32).min(CARD_MAX);
    (cols, w)
}

/// `text` in up to two lines of `width`, the second ending in an ellipsis
/// when it doesn't fit.
fn two_lines(text: &str, font: FontId, color: egui::Color32, width: f32) -> LayoutJob {
    let mut job = LayoutJob::simple(text.to_owned(), font, color, width);
    job.wrap = TextWrapping { max_width: width, max_rows: 2, break_anywhere: false, overflow_character: Some('…') };
    job
}

/// A section's name, with a quiet note beside it when there is one.
fn section_heading(ui: &mut Ui, t: &Tokens, title: &str, note: Option<String>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(RichText::new(title).font(FontId::new(17.0, semibold())).color(t.text));
        if let Some(note) = note {
            ui.label(RichText::new(note).size(12.0).color(t.muted));
        }
    });
    ui.add_space(10.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_fill_the_width_and_stop_growing() {
        assert_eq!(card_metrics(532.0), (1, 532.0), "a lone column takes the width");
        let (cols, w) = card_metrics(828.0);
        assert_eq!(cols, 2);
        assert!((w * 2.0 + CARD_GAP - 828.0).abs() < 0.01);
        for width in (600..3000).step_by(7) {
            let (cols, w) = card_metrics(width as f32);
            if cols > 1 {
                assert!((CARD_MIN..=CARD_MAX).contains(&w), "{w} at {width}");
            }
        }
    }
}
