//! The Series place and a series' page.
//!
//! The place is every series the shelf's books are in, by name, as tiles: a
//! small stack of the first books' covers (spreading upward under the pointer), the name, how many books and who
//! wrote them, and how far through it the listener is. Only the rows on
//! screen are laid out, as in the library; the list itself is worked out
//! once per listing ([`Books::all_series`](super::Books::all_series)).
//!
//! A series' page: its name, who wrote it, how far through it the listener
//! is, a button that goes on with the next book not finished, and its books
//! as the library's tiles in their order, each with its number. Reached from
//! the place, a book's facts (its page, and the Info tab) and the palette. A series is known only by its name ([`Route::Series`]), and its
//! books are worked out once per listing ([`Books::series`](super::Books::series)).
//! A series is a handful of books, so the tiles are laid out whole rather
//! than through a virtual list.

use super::Action;
use super::library::{COVER_RADIUS, SeriesEntry, author_of, grid_metrics, stand_in};
use super::nav::Route;
use super::progress::Listened;
use super::view_ctx::ViewCtx;
use super::virtual_list::{VirtualList, near};
use crate::fmt;
use crate::widgets::elide;
use egui::{Color32, FontId, Rect, RichText, Sense, Ui, pos2, vec2};
use shell::icons::Icon;
use shell::theme::Tokens;
use sources::ItemSummary;

/// The longest a title gets on the button before it's cut.
const BUTTON_TITLE_CHARS: usize = 36;
/// At most this many covers in a stack.
const STACK_DEPTH: usize = 3;
/// How much more of each cover behind shows above the one in front, at
/// rest and spread out under the pointer (or Tab), and how much narrower it
/// is, as a share of the tile; the spread eases in over `SPREAD_TIME`.
const PEEK: f32 = 9.0;
const PEEK_SPREAD: f32 = 15.0;
const SHRINK: f32 = 0.07;
const SPREAD_TIME: f32 = 0.2;
/// The room every tile keeps above its front cover: the deepest stack,
/// spread out. Covers and names line up along a row whatever each series
/// holds, and spreading never moves the tile.
const HEAD: f32 = PEEK_SPREAD * (STACK_DEPTH - 1) as f32;
/// The name, the books and the standing under a tile's stack, as the
/// library's tiles keep room for three lines.
const LABEL_H: f32 = 58.0;
const GAP_X: f32 = 20.0;
const GAP_Y: f32 = 24.0;

impl ViewCtx<'_> {
    /// The Series place: every series as a tile, by name.
    pub(super) fn all_series_page(&mut self, ui: &mut Ui) {
        let all = self.books.all_series();
        let count = match all.len() {
            0 => None,
            1 => Some("1 series".to_owned()),
            n => Some(format!("{n} series")),
        };
        shell::kit::page_heading(ui, "Series", count);
        ui.add_space(2.0);
        if self.books.len() == 0 {
            self.no_books(ui);
            return;
        }
        if all.is_empty() {
            shell::kit::empty_state(ui, Icon::List, "No series yet", "Books the server files under a series show up here, together and in order.");
            return;
        }
        let (cols, tile) = grid_metrics(ui.available_width());
        let row_h = HEAD + tile + LABEL_H + GAP_Y;
        let rows = all.len().div_ceil(cols);
        let row_entries = |row: usize| &all[row * cols..((row + 1) * cols).min(all.len())];
        VirtualList::grid("all-series", row_h, rows).show(ui, |ui, range| {
            for row in range.clone() {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP_X;
                    for entry in row_entries(row) {
                        self.series_tile(ui, entry, tile);
                    }
                });
                ui.add_space(GAP_Y - ui.spacing().item_spacing.y);
            }
            for row in near(&range, 1, rows) {
                for entry in row_entries(row) {
                    for &i in entry.books.iter().take(STACK_DEPTH) {
                        if let Some(cover) = self.books.all()[i].cover.as_deref() {
                            self.art.prefetch(cover, tile);
                        }
                    }
                }
            }
        });
    }

    /// One series: its first books' covers stacked, the front one its first
    /// book's, then its name, how many books and by whom, and how far
    /// through it the listener is. A click opens its page.
    fn series_tile(&mut self, ui: &mut Ui, entry: &SeriesEntry, tile: f32) {
        let t = Tokens::of(ui);
        let (rect, resp) = ui.allocate_exact_size(vec2(tile, HEAD + tile + LABEL_H), Sense::click());
        let books: Vec<ItemSummary> = entry.books.iter().map(|&i| self.books.all()[i].clone()).collect();
        let listened: Vec<Listened> = books.iter().map(|b| self.listened(b)).collect();
        let stage = stage(books.len(), &listened);
        let count = if books.len() == 1 { "1 book".to_owned() } else { format!("{} books", books.len()) };
        let authors = authors(&books);
        let byline = if authors.is_empty() { count.clone() } else { format!("{count} · {authors}") };
        if ui.is_rect_visible(rect) {
            let spread = ui.ctx().animate_bool_with_time_and_easing(
                resp.id.with("spread"),
                resp.hovered() || resp.has_focus(),
                SPREAD_TIME,
                egui::emath::easing::cubic_out,
            );
            let dark = ui.visuals().dark_mode;
            let base = Rect::from_min_size(pos2(rect.left(), rect.top() + HEAD), vec2(tile, tile));
            // Back to front: each a step higher and narrower, solid and
            // shaded rather than see-through, so the stack reads as covers
            // and not as a grey frame. Spread, the stack loosens upward and
            // the shade eases, so more of the books behind shows.
            let peek = PEEK + (PEEK_SPREAD - PEEK) * spread;
            for back in (1..books.len().min(STACK_DEPTH)).rev() {
                let k = back as f32;
                let side = tile * (1.0 - SHRINK * k);
                let r = Rect::from_min_size(pos2(base.center().x - side / 2.0, base.top() - peek * k), vec2(side, side));
                let shade = if dark { 35.0 + 35.0 * k } else { 25.0 + 25.0 * k } * (1.0 - 0.5 * spread);
                ui.painter().add(shadow(spread, dark).as_shape(r, COVER_RADIUS));
                self.art.paint(ui, r, books[back].cover.as_deref(), COVER_RADIUS, &stand_in(&books[back]));
                ui.painter().rect_filled(r, COVER_RADIUS, Color32::from_black_alpha(shade as u8));
            }
            let front = base.translate(vec2(0.0, -2.0 * spread));
            if books.len() > 1 {
                ui.painter().add(shadow(spread, dark).as_shape(front, COVER_RADIUS));
            }
            self.art.paint(ui, front, books[0].cover.as_deref(), COVER_RADIUS, &stand_in(&books[0]));
            shell::kit::focus_ring(ui, &resp, front, COVER_RADIUS);
            match through(&listened) {
                f if f >= 1.0 => shell::kit::cover_badge(ui.painter(), front, Icon::Check, &t),
                f if f > 0.0 => shell::kit::cover_progress(ui.painter(), front, COVER_RADIUS, f, &t),
                _ => {}
            }
            let name = elide(ui, &entry.name, FontId::proportional(13.0), t.text, tile);
            ui.painter().galley(pos2(rect.left(), base.bottom() + 7.0), name, t.text);
            let line = elide(ui, &byline, FontId::proportional(12.0), t.muted, tile);
            ui.painter().galley(pos2(rect.left(), base.bottom() + 24.0), line, t.muted);
            let line = elide(ui, &stage, FontId::proportional(11.5), t.muted, tile);
            ui.painter().galley(pos2(rect.left(), base.bottom() + 40.0), line, t.muted);
        }
        let label = if authors.is_empty() {
            format!("{}, {count}, {}", entry.name, stage.to_lowercase())
        } else {
            format!("{}, {count} by {authors}, {}", entry.name, stage.to_lowercase())
        };
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.actions.push(Action::Go(Route::Series(entry.name.clone())));
        }
    }

    pub(super) fn series_page(&mut self, ui: &mut Ui, name: &str) {
        let t = Tokens::of(ui);
        let ids = self.books.series(name);
        if ids.is_empty() {
            shell::kit::page_heading(ui, name, None);
            shell::kit::empty_state(ui, Icon::List, "This series isn't here", "It may have been renamed or removed on the server. Sync again to see.");
            return;
        }
        let books: Vec<ItemSummary> = ids.iter().map(|&i| self.books.all()[i].clone()).collect();
        let listened: Vec<Listened> = books.iter().map(|b| self.listened(b)).collect();
        egui::ScrollArea::vertical().id_salt(("series", name)).auto_shrink(false).show(ui, |ui| {
            let count = if books.len() == 1 { "1 book".to_owned() } else { format!("{} books", books.len()) };
            shell::kit::page_heading(ui, name, Some(count));
            let authors = authors(&books);
            if !authors.is_empty() {
                ui.add(egui::Label::new(RichText::new(format!("By {authors}")).size(15.0).color(t.text)).wrap());
                ui.add_space(2.0);
            }
            ui.add(egui::Label::new(RichText::new(standing(&books, &listened)).size(12.0).color(t.muted)).wrap());
            ui.add_space(12.0);
            if let Some(next) = next_up(&listened) {
                self.continue_button(ui, &books[next], &listened[next]);
                ui.add_space(24.0);
            } else {
                ui.add_space(12.0);
            }
            let (cols, tile) = grid_metrics(ui.available_width());
            for row in ids.chunks(cols) {
                self.book_row(ui, row, tile, tile, Some(name));
            }
        });
    }

    /// Goes on with the series: Resume the book partway, or Play the next
    /// one, named; Pause while it's the one sounding. Offline, a book that
    /// isn't all here says so instead, as its page does.
    fn continue_button(&mut self, ui: &mut Ui, book: &ItemSummary, listened: &Listened) {
        let loaded = self.player.book.as_ref().is_some_and(|b| b.key.item == book.id);
        let sounding = self.player.active && !self.player.paused && loaded;
        let title = cut(&book.title, BUTTON_TITLE_CHARS);
        let (icon, label) = if sounding {
            (Icon::Pause, format!("Pause · {title}"))
        } else if matches!(listened, Listened::InProgress { .. }) || loaded {
            (Icon::Play, format!("Resume · {title}"))
        } else {
            (Icon::Play, format!("Play · {title}"))
        };
        ui.horizontal(|ui| {
            if self.opening {
                shell::kit::primary_unavailable(ui, None, "Opening…");
            } else if self.unplayable(&book.id) {
                shell::kit::primary_unavailable(ui, Some(Icon::Offline), &format!("Not downloaded · {title}"))
                    .on_disabled_hover_text("Can't reach the server. Downloaded books play without it.");
            } else if shell::kit::primary(ui, Some(icon), &label).clicked() {
                self.actions.push(if sounding { Action::Playback(crate::player::Control::Toggle) } else { Action::PlayBook(book.id.clone()) });
            }
        });
    }
}

/// The series' authors, each once, in the order its books name them: "Ursula
/// K. Le Guin", or "Frank Herbert and Brian Herbert", or the first and how
/// many others when there are more.
fn authors(books: &[ItemSummary]) -> String {
    let mut names: Vec<&str> = Vec::new();
    for book in books {
        let author = author_of(book);
        if !author.trim().is_empty() && !names.contains(&author) {
            names.push(author);
        }
    }
    match names.as_slice() {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [a, b] => format!("{a} and {b}"),
        [first, rest @ ..] => format!("{first} and {} others", rest.len()),
    }
}

/// How far through the series the listener is, and how long it runs:
/// "1 of 2 finished · 24 hr 52 min", "Not started", or "All finished".
fn standing(books: &[ItemSummary], listened: &[Listened]) -> String {
    let mut parts = vec![stage(books.len(), listened)];
    let total: f64 = books.iter().filter_map(|b| b.duration).sum();
    if total > 0.0 {
        parts.push(fmt::span(total));
    }
    parts.join(" · ")
}

/// How far through `books` books the listener is: "Not started",
/// "Started", "1 of 2 finished" or "All finished".
/// The soft shadow under each cover in a stack, deeper as it spreads, so the
/// covers sit apart from one another.
fn shadow(spread: f32, dark: bool) -> egui::epaint::Shadow {
    let alpha = if dark { 90.0 + 40.0 * spread } else { 35.0 + 25.0 * spread };
    egui::epaint::Shadow { offset: [0, (2.0 + 3.0 * spread) as i8], blur: (8.0 + 8.0 * spread) as u8, spread: 0, color: Color32::from_black_alpha(alpha as u8) }
}

/// How far through a series the listener is, from nothing to all of it: each
/// book finished counts whole and one begun by how far into it they are.
fn through(listened: &[Listened]) -> f32 {
    if listened.is_empty() {
        return 0.0;
    }
    let sum: f32 = listened
        .iter()
        .map(|l| match l {
            Listened::Finished => 1.0,
            Listened::InProgress { fraction, .. } => fraction.unwrap_or(0.0).clamp(0.0, 1.0),
            Listened::NotStarted => 0.0,
        })
        .sum();
    sum / listened.len() as f32
}

fn stage(books: usize, listened: &[Listened]) -> String {
    let finished = listened.iter().filter(|l| matches!(l, Listened::Finished)).count();
    if finished == books {
        "All finished".to_owned()
    } else if finished > 0 {
        format!("{finished} of {books} finished")
    } else if listened.iter().any(|l| !matches!(l, Listened::NotStarted)) {
        "Started".to_owned()
    } else {
        "Not started".to_owned()
    }
}

/// The book to go on with: the first one partway, else the first not
/// finished. None once every book is finished.
fn next_up(listened: &[Listened]) -> Option<usize> {
    listened.iter().position(|l| matches!(l, Listened::InProgress { .. })).or_else(|| listened.iter().position(|l| !matches!(l, Listened::Finished)))
}

/// `text` cut to `chars` characters with an ellipsis.
fn cut(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars { text.to_owned() } else { format!("{}…", text.chars().take(chars - 1).collect::<String>().trim_end()) }
}

#[cfg(test)]
mod tests {
    use super::super::library::series_ids;
    use super::*;
    use sources::Series;

    fn book(id: &str, title: &str, author: &str, series: &[(&str, Option<&str>)]) -> ItemSummary {
        ItemSummary {
            id: id.into(),
            title: title.into(),
            authors: vec![author.into()],
            series: series.iter().map(|(name, n)| Series { name: (*name).into(), sequence: n.map(Into::into) }).collect(),
            duration: Some(36_000.0),
            ..ItemSummary::default()
        }
    }

    fn titles(books: &[ItemSummary], name: &str) -> Vec<String> {
        series_ids(books, name).into_iter().map(|i| books[i].title.clone()).collect()
    }

    #[test]
    fn a_series_lists_its_books_by_number_with_the_unnumbered_last() {
        let books = vec![
            book("a", "Children of Dune", "Frank Herbert", &[("Dune", Some("3"))]),
            book("b", "Dune", "Frank Herbert", &[("Dune", Some("1")), ("Dune Chronicles", Some("1"))]),
            book("c", "Tales of Dune", "Frank Herbert", &[("Dune", None)]),
            book("d", "God Emperor of Dune", "Frank Herbert", &[("Dune", Some("10"))]),
            book("e", "Dune Messiah", "Frank Herbert", &[("Dune", Some(" 2 "))]),
            book("f", "Hunters of Dune", "Brian Herbert", &[("Dune", Some("2.5"))]),
            book("g", "Piranesi", "Susanna Clarke", &[]),
        ];
        // As numbers, not text: 10 after 3, and the novella between 2 and 3.
        assert_eq!(titles(&books, "Dune"), ["Dune", "Dune Messiah", "Hunters of Dune", "Children of Dune", "God Emperor of Dune", "Tales of Dune"]);
        assert_eq!(titles(&books, "Dune Chronicles"), ["Dune"], "a book in two series is in each");
        assert!(titles(&books, "Earthsea").is_empty());
    }

    #[test]
    fn the_place_lists_each_series_once_by_name_with_its_books_in_order() {
        use super::super::library::all_series;
        let books = vec![
            book("a", "The Dispossessed", "Ursula K. Le Guin", &[("Hainish", Some("6"))]),
            book("b", "Dune", "Frank Herbert", &[("Dune", Some("1")), ("Dune Chronicles", Some("1"))]),
            book("c", "The Left Hand of Darkness", "Ursula K. Le Guin", &[("Hainish", Some("1")), ("Hainish", Some("1"))]),
            book("d", "Piranesi", "Susanna Clarke", &[]),
            book("e", "A Wizard of Earthsea", "Ursula K. Le Guin", &[("earthsea", Some("1"))]),
        ];
        let all = all_series(&books);
        let names: Vec<&str> = all.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Dune", "Dune Chronicles", "earthsea", "Hainish"], "by name, without case");
        assert_eq!(all[3].books, [2, 0], "each book once, in the series' order");
        assert_eq!(all[1].books, [1], "a book in two series is in each");
    }

    #[test]
    fn the_series_goes_on_with_the_book_partway_else_the_first_not_finished() {
        let partway = Listened::InProgress { fraction: Some(0.5), left: Some(10.0) };
        assert_eq!(next_up(&[Listened::Finished, Listened::NotStarted, partway]), Some(2), "a book partway comes before a new one");
        assert_eq!(next_up(&[Listened::Finished, Listened::NotStarted, Listened::NotStarted]), Some(1));
        assert_eq!(next_up(&[Listened::Finished, Listened::Finished]), None);
    }

    #[test]
    fn the_standing_line_says_how_far_through_and_how_long() {
        let books = [book("a", "A", "X", &[]), book("b", "B", "X", &[])];
        assert_eq!(standing(&books, &[Listened::NotStarted, Listened::NotStarted]), "Not started · 20 hr");
        assert_eq!(standing(&books, &[Listened::Finished, Listened::NotStarted]), "1 of 2 finished · 20 hr");
        assert_eq!(standing(&books, &[Listened::Finished, Listened::Finished]), "All finished · 20 hr");
    }

    #[test]
    fn the_authors_are_named_once_each() {
        let one = [book("a", "A", "Ursula K. Le Guin", &[]), book("b", "B", "Ursula K. Le Guin", &[])];
        assert_eq!(authors(&one), "Ursula K. Le Guin");
        let two = [book("a", "A", "Frank Herbert", &[]), book("b", "B", "Brian Herbert", &[])];
        assert_eq!(authors(&two), "Frank Herbert and Brian Herbert");
        let three = [two[0].clone(), two[1].clone(), book("c", "C", "Kevin J. Anderson", &[])];
        assert_eq!(authors(&three), "Frank Herbert and 2 others");
    }

    #[test]
    fn a_long_title_is_cut_on_the_button() {
        assert_eq!(cut("Dune", 36), "Dune");
        assert_eq!(cut("The Ocean at the End of the Lane", 12), "The Ocean a…");
    }
}
