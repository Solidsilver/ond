//! A book's page, in Ond's album-page idiom: the cover with the title,
//! authors, a line of facts (narrators, series and number, length) and Play
//! set on its bottom edge; under them the description and the chapters, side
//! by side in reading-width columns when the window is wide, one above the
//! other when it isn't. The details are fetched when the page opens.
//!

use super::library::{COVER_RADIUS, authors_line};
use super::progress::Listened;
use super::view_ctx::ViewCtx;
use super::{Action, Details};
use crate::fmt;
use crate::widgets::elide;
use egui::{FontId, RichText, Sense, Ui, pos2, vec2};
use shell::icons::{self, Icon};
use shell::theme::{Tokens, semibold};
use sources::ItemSummary;

/// Below this width the cover sits above the text instead of beside it.
const HEADER_STACK_BELOW: f32 = 480.0;
/// The cover's side beside the text: 30% of the width, within these. A little
/// larger than Ond's album art, since the page is about this one book.
const COVER_MIN: f32 = 176.0;
const COVER_MAX: f32 = 260.0;
/// How far along the book is: the gap under Play, and the bar's widest.
const PROGRESS_GAP: f32 = 14.0;
const PROGRESS_W: f32 = 320.0;
/// The … beside Play: as tall as the button it sits by, and its menu's width.
const MORE_SIDE: f32 = 32.0;
const MORE_MENU_W: f32 = 190.0;
/// The header text's height before it's first measured.
const HEADER_TEXT_H_GUESS: f32 = 140.0;
/// From this width the description and the chapters sit side by side.
const SPLIT_FROM: f32 = 760.0;
const COLUMN_GAP: f32 = 48.0;
/// The widest a column gets: prose stays readable, and a chapter's length
/// stays near its title instead of drifting to the far edge of the window.
const READING_W: f32 = 600.0;
pub(super) const CHAPTER_ROW_H: f32 = 32.0;
/// A chapter's number ends here (right-aligned, as Ond numbers tracks), and
/// its title starts at `CHAPTER_TITLE_X`.
pub(super) const CHAPTER_NUMBER_RIGHT: f32 = 18.0;
pub(super) const CHAPTER_TITLE_X: f32 = 34.0;
/// The lengths' column, whose numbers end at the row's right edge; the
/// starts' column ends left of it. A row narrower than `CHAPTER_STARTS_FROM`
/// shows the start only under the pointer.
const CHAPTER_LENGTH_W: f32 = 56.0;
const CHAPTER_STARTS_FROM: f32 = 340.0;

impl ViewCtx<'_> {
    pub(super) fn book_page(&mut self, ui: &mut Ui, id: &str) {
        let t = Tokens::of(ui);
        let Some(book) = self.books.by_id(id).cloned() else {
            shell::kit::empty_state(ui, Icon::Headphones, "This book isn't here", "It may have been removed from the server. Sync again to see.");
            return;
        };
        egui::ScrollArea::vertical().id_salt(("book", id)).auto_shrink(false).show(ui, |ui| {
            self.book_header(ui, &t, &book);
            ui.add_space(32.0);
            self.details(ui, id, &t);
            if let Some(Details::Loaded(item)) = self.books.details.get(id).cloned() {
                let at = if self.player.book.as_ref().is_some_and(|b| b.key.item == id) {
                    Some(self.player.position)
                } else {
                    // A finished book has every chapter behind it.
                    self.positions.get(id).map(|p| if p.finished { f64::INFINITY } else { p.position.secs() })
                };
                let description = item.description.as_deref().map(str::trim).filter(|d| !d.is_empty()).map(|text| self.books.description(id, text));
                book_body(ui, &t, description.as_ref().map(|blocks| blocks.as_slice()), &item.chapters, at);
            }
            ui.add_space(24.0);
        });
    }

    /// The cover beside the title, authors, facts, Play and how far along
    /// the book is, or above them when there's no room. The cover keeps its
    /// own shape within a square of `side`: a tall one is narrower, a wide
    /// one shorter, and the text sits on its bottom edge either way.
    fn book_header(&mut self, ui: &mut Ui, t: &Tokens, book: &ItemSummary) {
        let avail = ui.available_width();
        let stacked = avail < HEADER_STACK_BELOW;
        let side = if stacked { avail.min(COVER_MAX) } else { (avail * 0.3).clamp(COVER_MIN, COVER_MAX) };
        let shape = self.art.fitted(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(side, side)), book.cover.as_deref(), egui::Align2::LEFT_BOTTOM).size();
        // The text's height (it wraps) is known only once drawn, so this uses
        // last frame's and redoes the frame when it changed.
        let measured = ui.scope_id().with(("book-header-h", &book.id));
        let last_h = ui.data(|d| d.get_temp::<f32>(measured)).unwrap_or(HEADER_TEXT_H_GUESS);
        let layout = if stacked { egui::Layout::top_down(egui::Align::Min) } else { egui::Layout::left_to_right(egui::Align::Min) };
        ui.with_layout(layout, |ui| {
            // Beside the text, the row is as tall as the taller of the two,
            // and both sit on its foot.
            let row_h = if stacked { shape.y } else { shape.y.max(last_h) };
            let (slot, _) = ui.allocate_exact_size(vec2(shape.x, row_h), Sense::hover());
            let cover = egui::Align2::LEFT_BOTTOM.align_size_within_rect(shape, slot);
            self.art.paint(ui, cover, book.cover.as_deref(), COVER_RADIUS, &super::library::stand_in(book));
            ui.add_space(if stacked { 16.0 } else { 24.0 });
            let block = ui.vertical(|ui| {
                if !stacked {
                    ui.add_space((row_h - last_h).max(0.0));
                }
                let top = ui.cursor().top();
                header_text(ui, t, book);
                self.play_row(ui, book);
                self.book_progress(ui, t, book);
                ui.min_rect().bottom() - top
            });
            let h = block.inner.round();
            if (h - last_h).abs() > 0.5 {
                ui.data_mut(|d| d.insert_temp(measured, h));
                ui.ctx().request_discard("book header height changed");
            }
        });
    }

    /// Play, Resume or Pause. A row of its own, so the book's other commands
    /// can sit beside it.
    fn play_row(&mut self, ui: &mut Ui, book: &ItemSummary) {
        let position = if self.player.book.as_ref().is_some_and(|b| b.key.item == book.id) {
            self.player.position
        } else {
            self.positions.get(&book.id).filter(|p| !p.finished).map_or(0.0, |p| p.position.secs())
        };
        let left = crate::timeline::left(
            (book.duration.unwrap_or(0.0) - position).max(0.0) / f64::from(crate::timeline::normalize_speed(self.playback.speed)),
            self.playback.speed,
        );
        // While this book is the one sounding, the button pauses it, as the
        // title bar's play/pause would.
        let sounding = self.player.active && !self.player.paused && self.player.book.as_ref().is_some_and(|b| b.key.item == book.id);
        let (icon, label) = if sounding {
            (Icon::Pause, format!("Pause · {left}"))
        } else if position > 0.0 {
            (Icon::Play, format!("Resume · {left}"))
        } else {
            (Icon::Play, "Play".into())
        };
        ui.horizontal(|ui| {
            if self.opening {
                shell::kit::primary_unavailable(ui, None, "Opening…");
            } else if shell::kit::primary(ui, Some(icon), &label).clicked() {
                self.actions.push(if sounding { Action::Playback(crate::player::Control::Toggle) } else { Action::PlayBook(book.id.clone()) });
            }
            self.more_menu(ui, book);
        });
    }

    /// The book's other commands, behind a … beside Play: settling where the
    /// book stands by hand. Only what applies is offered: a finished book
    /// can't be finished again, and one not begun has nothing to start over.
    fn more_menu(&mut self, ui: &mut Ui, book: &ItemSummary) {
        let t = Tokens::of(ui);
        let more = icons::sized_button(ui, Icon::More, &format!("More for {}", book.title), vec2(MORE_SIDE, MORE_SIDE), 14.0, Some(t.muted));
        let listened = self.listened(book);
        let id = &book.id;
        let chosen = shell::kit::popover(&more, MORE_MENU_W, |ui| {
            let mut chosen = None;
            if matches!(listened, Listened::Finished) {
                if shell::kit::menu_row(ui, "Mark as unfinished") {
                    chosen = Some(Action::MarkUnfinished(id.clone()));
                }
            } else if shell::kit::menu_row(ui, "Mark as finished") {
                chosen = Some(Action::MarkFinished(id.clone()));
            }
            if !matches!(listened, Listened::NotStarted) && shell::kit::menu_row(ui, "Start over") {
                chosen = Some(Action::StartOver(id.clone()));
            }
            chosen
        });
        if let Some(action) = chosen.flatten() {
            self.actions.push(action);
        }
    }

    /// How far along the whole book is, under Play: a bar and "6h 25m of
    /// 11h 24m · 56%", or "Finished". Nothing for a book not started.
    fn book_progress(&mut self, ui: &mut Ui, t: &Tokens, book: &ItemSummary) {
        let listened = self.listened(book);
        let (fraction, line) = match listened {
            Listened::NotStarted => return,
            Listened::Finished => (1.0, "Finished".to_owned()),
            Listened::InProgress { fraction: Some(fraction), .. } => {
                let duration = book.duration.unwrap_or(0.0);
                let playing_at = self.player.book.as_ref().filter(|b| b.key.item == book.id).map(|_| self.player.position);
                let at = playing_at.or_else(|| self.positions.get(&book.id).map(|p| p.position.secs())).unwrap_or(0.0);
                (fraction, format!("{} · {}%", fmt::of_span(at, duration), (fraction * 100.0).floor() as u32))
            }
            Listened::InProgress { fraction: None, .. } => return,
        };
        ui.add_space(PROGRESS_GAP);
        let w = ui.available_width().min(PROGRESS_W);
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 4.0), Sense::hover());
        shell::kit::progress_bar(ui.painter(), rect, fraction, t);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ProgressIndicator, true, &line));
        ui.add_space(6.0);
        ui.label(RichText::new(line).size(12.0).color(t.muted));
    }

    /// What the page says while the details are on their way, or when they
    /// couldn't be had.
    fn details(&self, ui: &mut Ui, id: &str, t: &Tokens) {
        match self.books.details.get(id) {
            None | Some(Details::Loading) => {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(13.0, 13.0), Sense::hover());
                    shell::icons::paint(ui.painter(), rect, Icon::Spinner, t.muted);
                    ui.label(RichText::new("Loading the chapters…").size(12.0).color(t.muted));
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                });
            }
            Some(Details::Failed(why)) => {
                ui.label(RichText::new(why).size(12.0).color(t.danger));
            }
            Some(Details::Loaded(_)) => {}
        }
    }
}

/// The title, any subtitle, the authors, the facts and Play.
fn header_text(ui: &mut Ui, t: &Tokens, book: &ItemSummary) {
    ui.add(egui::Label::new(RichText::new(&book.title).font(FontId::new(26.0, semibold())).color(t.text)).wrap());
    if let Some(subtitle) = book.subtitle.as_deref().filter(|s| !s.is_empty()) {
        ui.add(egui::Label::new(RichText::new(subtitle).size(15.0).color(t.muted)).wrap());
    }
    ui.add_space(2.0);
    ui.add(egui::Label::new(RichText::new(authors_line(book)).size(17.0).color(t.text)).wrap());
    let facts = facts_line(book);
    if !facts.is_empty() {
        ui.add_space(2.0);
        ui.add(egui::Label::new(RichText::new(facts).size(12.0).color(t.muted)).wrap());
    }
    ui.add_space(12.0);
}

/// The description and the chapters: two columns from [`SPLIT_FROM`], else
/// one above the other, each no wider than [`READING_W`].
fn book_body(ui: &mut Ui, t: &Tokens, description: Option<&[shell::rich::Block]>, chapters: &[sources::Chapter], at: Option<f64>) {
    let avail = ui.available_width();
    let column = |ui: &mut Ui, w: f32, draw: &dyn Fn(&mut Ui)| {
        ui.allocate_ui_with_layout(vec2(w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_width(w);
            draw(ui);
        });
    };
    if avail >= SPLIT_FROM && description.is_some() && !chapters.is_empty() {
        let w = ((avail - COLUMN_GAP) / 2.0).min(READING_W);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            column(ui, w, &|ui| about(ui, t, description));
            ui.add_space(COLUMN_GAP);
            column(ui, w, &|ui| chapter_list(ui, t, chapters, at));
        });
    } else {
        let w = avail.min(READING_W);
        if description.is_some() {
            column(ui, w, &|ui| about(ui, t, description));
            ui.add_space(24.0);
        }
        column(ui, w, &|ui| chapter_list(ui, t, chapters, at));
    }
}

/// A section's name, with a quiet count or note beside it when there is one.
fn section_heading(ui: &mut Ui, t: &Tokens, title: &str, note: Option<String>) {
    ui.horizontal(|ui| {
        // Its own spacing: the columns it sits in set theirs to nothing.
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(RichText::new(title).font(FontId::new(15.0, semibold())).color(t.text));
        if let Some(note) = note {
            ui.label(RichText::new(note).size(12.0).color(t.muted));
        }
    });
    ui.add_space(6.0);
}

/// "About this book" and the description, set for reading.
fn about(ui: &mut Ui, t: &Tokens, description: Option<&[shell::rich::Block]>) {
    let Some(description) = description else { return };
    section_heading(ui, t, "About this book", None);
    shell::rich::draw(ui, description, &shell::rich::Style::reading(ui));
}

/// The line under the authors: narrators, series and number, and how long the
/// book is, each only when the server said.
pub fn facts_line(book: &ItemSummary) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !book.narrators.is_empty() {
        parts.push(format!("Read by {}", book.narrators.join(", ")));
    }
    for series in &book.series {
        parts.push(match &series.sequence {
            Some(n) => format!("{} #{n}", series.name),
            None => series.name.clone(),
        });
    }
    if let Some(duration) = book.duration {
        parts.push(fmt::span(duration));
    }
    parts.join(" · ")
}

/// The chapter list: each chapter's number, title, where it starts in the
/// book and how long it runs.
fn chapter_list(ui: &mut Ui, t: &Tokens, chapters: &[sources::Chapter], at: Option<f64>) {
    if chapters.is_empty() {
        return;
    }
    let count = if chapters.len() == 1 { "1 chapter".to_owned() } else { format!("{} chapters", chapters.len()) };
    section_heading(ui, t, "Chapters", Some(count));
    ui.spacing_mut().item_spacing.y = 0.0;
    // A book rarely has more than a few hundred chapters, and this list is
    // drawn only while the book's page is open.
    for (n, chapter) in chapters.iter().enumerate() {
        let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), CHAPTER_ROW_H), Sense::hover());
        let length = fmt::clock((chapter.end - chapter.start).max(0.0));
        let start = fmt::clock(chapter.start);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, format!("{} · starts at {start} · {length}", chapter.title)));
        if !ui.is_rect_visible(row) {
            continue;
        }
        chapter_row(ui, t, row, n, chapter, at);
    }
}

/// One chapter's row in `row`: its number, title, start and length, as the
/// page and the chapters column both draw it. Where the row is narrow (the
/// column) the start shows only under the pointer, so the title keeps its
/// room. `at` is where the book is, if it's
/// been started: the chapter there is tinted, with how far into it along
/// the row's foot, and the ones before it are dimmed.
pub(super) fn chapter_row(ui: &Ui, t: &Tokens, row: egui::Rect, n: usize, chapter: &sources::Chapter, at: Option<f64>) {
    let length = fmt::clock((chapter.end - chapter.start).max(0.0));
    let current = at.is_some_and(|at| at >= chapter.start && at < chapter.end);
    let finished = at.is_some_and(|at| at >= chapter.end);
    let ink = if current {
        t.accent
    } else if finished {
        t.muted
    } else {
        t.text
    };
    if current && let Some(at) = at {
        // The chapter playing: tinted, with how far into it along the
        // foot of the row, as the title bar's scrubber shows it.
        ui.painter().rect_filled(row, 4.0, t.accent.gamma_multiply(0.1));
        let fraction = ((at - chapter.start) / (chapter.end - chapter.start).max(1.0)).clamp(0.0, 1.0) as f32;
        let foot = egui::Rect::from_min_max(pos2(row.left() + 4.0, row.bottom() - 2.0), pos2(row.right() - 4.0, row.bottom()));
        ui.painter().rect_filled(egui::Rect::from_min_max(foot.min, pos2(foot.left() + foot.width() * fraction, foot.bottom())), 1.0, t.accent);
    }
    let cy = row.center().y;
    if current {
        let icon = egui::Rect::from_center_size(pos2(row.left() + CHAPTER_NUMBER_RIGHT - 5.0, cy), vec2(10.0, 10.0));
        shell::icons::paint(ui.painter(), icon, Icon::Play, t.accent);
    } else {
        ui.painter().text(pos2(row.left() + CHAPTER_NUMBER_RIGHT, cy), egui::Align2::RIGHT_CENTER, format!("{}", n + 1), FontId::proportional(12.0), t.muted);
    }
    let time = elide(ui, &length, FontId::proportional(12.0), t.muted, 72.0);
    let time_w = time.size().x;
    ui.painter().galley(pos2(row.right() - time_w, cy - time.size().y / 2.0), time, t.muted);
    // The start, quieter, right-aligned in a column of its own left of the
    // lengths, so both columns of numbers line up down the list.
    let mut text_right = row.right() - time_w;
    if row.width() >= CHAPTER_STARTS_FROM || ui.rect_contains_pointer(row) {
        let quiet = t.muted.gamma_multiply(0.75);
        let start = elide(ui, &fmt::clock(chapter.start), FontId::proportional(12.0), quiet, CHAPTER_LENGTH_W);
        let right = row.right() - CHAPTER_LENGTH_W - 12.0;
        text_right = right - start.size().x;
        ui.painter().galley(pos2(text_right, cy - start.size().y / 2.0), start, quiet);
    }
    let font = if current { FontId::new(13.0, semibold()) } else { FontId::proportional(13.0) };
    let title = elide(ui, &chapter.title, font, ink, text_right - row.left() - CHAPTER_TITLE_X - 16.0);
    ui.painter().galley(pos2(row.left() + CHAPTER_TITLE_X, cy - title.size().y / 2.0), title, ink);
}

#[cfg(test)]
mod tests {
    use super::*;
    use sources::Series;

    fn book() -> ItemSummary {
        ItemSummary { id: "b".into(), title: "The Left Hand of Darkness".into(), authors: vec!["Ursula K. Le Guin".into()], ..ItemSummary::default() }
    }

    #[test]
    fn the_facts_line_says_only_what_the_server_said() {
        assert_eq!(facts_line(&book()), "", "no narrator, series or length: nothing to say");
        let mut b = book();
        b.narrators = vec!["George Guidall".into()];
        b.duration = Some(41_000.0);
        assert_eq!(facts_line(&b), "Read by George Guidall · 11 hr 24 min");
        b.series = vec![Series { name: "Hainish".into(), sequence: Some("2".into()) }];
        assert_eq!(facts_line(&b), "Read by George Guidall · Hainish #2 · 11 hr 24 min");
        b.series[0].sequence = None;
        assert!(facts_line(&b).contains("Hainish ·"), "a book with no number in its series says so plainly");
    }
}
