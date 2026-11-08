//! The books the shelf has, and the page that draws them: a grid of covers,
//! sorted by title, author or when they were added.
//!
//! The rows are laid out only while they're on screen ([`VirtualList`]), so a
//! library of any size draws a screenful and asks for the covers it shows.
//! Sorting is worked out once per library version and order
//! ([`crate::memo::Memo`]), so a frame lays out what it can rather than sorting.

use super::Action;
use super::nav::Route;
use super::prefs_view::PrefsAction;
use super::view_ctx::ViewCtx;
use super::virtual_list::{VirtualList, near};
use crate::memo::Memo;
use crate::settings::LibrarySort;
use crate::widgets::elide;
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};
use shell::icons::Icon;
use shell::theme::{Tokens, semibold};
use sources::{Item, ItemSummary};
use std::collections::HashMap;
use std::sync::Arc;

/// The narrowest a cover may be drawn; the grid fits as many columns as the
/// width allows.
const TILE_MIN: f32 = 172.0;
const GAP_X: f32 = 20.0;
const GAP_Y: f32 = 24.0;
/// Title and author under the art.
const LABEL_H: f32 = 42.0;
/// The rows just past either end whose covers load ahead.
const PREFETCH_ROWS: usize = 1;

/// The shelf's books, in the order the sync stored them, and what has been
/// fetched about each one since.
#[derive(Default)]
pub struct Books {
    /// Bumped by every new listing, so derived data can be memoised on it.
    version: u64,
    all: Arc<Vec<ItemSummary>>,
    /// Indices into `all`, in the order drawn, per library version and sort.
    order: Memo<(u64, LibrarySort), Vec<usize>>,
    /// A book's details once its page asked for them. By the book's own id:
    /// one account is shown for now.
    pub details: HashMap<String, Details>,
}

/// What a book page knows about a book beyond its listing.
#[derive(Debug, Clone)]
pub enum Details {
    /// Asked for, not here yet.
    Loading,
    Loaded(Arc<Item>),
    /// Couldn't be loaded; the page still shows the listing's view.
    Failed(String),
}

impl Books {
    /// A new listing from the shelf. What was fetched about the books that are
    /// still there stays.
    pub fn set(&mut self, books: Vec<ItemSummary>) {
        self.version += 1;
        self.all = Arc::new(books);
        let kept: std::collections::HashSet<String> = self.all.iter().map(|b| b.id.clone()).collect();
        self.details.retain(|id, _| kept.contains(id));
    }

    /// Nothing is on file any more (signed out, or another account).
    pub fn clear(&mut self) {
        self.version += 1;
        self.all = Arc::new(Vec::new());
        self.details.clear();
    }

    pub fn all(&self) -> &Arc<Vec<ItemSummary>> {
        &self.all
    }

    pub fn len(&self) -> usize {
        self.all.len()
    }

    pub fn by_id(&self, id: &str) -> Option<&ItemSummary> {
        self.all.iter().find(|b| b.id == id)
    }

    /// Indices into [`all`](Self::all), in the order `sort` draws them.
    pub fn order(&mut self, sort: LibrarySort) -> Arc<Vec<usize>> {
        let key = (self.version, sort);
        let books = &self.all;
        self.order.get(key, || sorted_ids(books, sort))
    }

    pub fn loaded(&mut self, id: &str, item: Item) {
        self.details.insert(id.to_owned(), Details::Loaded(Arc::new(item)));
    }

    pub fn failed(&mut self, id: &str, error: crate::failure::Failure) {
        self.details.insert(id.to_owned(), Details::Failed(error.into_message()));
    }
}

/// Indices into `books`, ordered by `sort`. Title and author compare without
/// case; ties fall back to the title and then the id, so the order is stable.
pub fn sorted_ids(books: &[ItemSummary], sort: LibrarySort) -> Vec<usize> {
    let mut ids: Vec<usize> = (0..books.len()).collect();
    match sort {
        // A book the server gave no title or author for goes last, where it's
        // found rather than in front of everything.
        LibrarySort::Title => ids.sort_by_cached_key(|&i| {
            let b = &books[i];
            (b.title.trim().is_empty(), b.title.to_lowercase(), author_of(b).to_lowercase(), b.id.clone())
        }),
        LibrarySort::Author => ids.sort_by_cached_key(|&i| {
            let b = &books[i];
            (author_of(b).trim().is_empty(), author_of(b).to_lowercase(), b.title.to_lowercase(), b.id.clone())
        }),
        // Newest first; books the server never stamped go last, by title.
        LibrarySort::Added => ids.sort_by(|&a, &b| {
            let (x, y) = (books[a].added, books[b].added);
            y.cmp(&x).then_with(|| books[a].title.to_lowercase().cmp(&books[b].title.to_lowercase())).then_with(|| books[a].id.cmp(&books[b].id))
        }),
    }
    ids
}

/// The name a book is filed under: its first author, spelled as the server
/// spells it.
pub fn author_of(book: &ItemSummary) -> &str {
    book.authors.first().map_or("", String::as_str)
}

/// Every author, as a book page lists them: "Ursula K. Le Guin and 2 others".
pub fn authors_line(book: &ItemSummary) -> String {
    match book.authors.as_slice() {
        [] => "Unknown author".into(),
        [one] => one.clone(),
        [first, rest @ ..] => format!("{first} and {} other{}", rest.len(), if rest.len() == 1 { "" } else { "s" }),
    }
}

/// Columns and tile width that fill `width`.
pub fn grid_metrics(width: f32) -> (usize, f32) {
    let cols = (((width + GAP_X) / (TILE_MIN + GAP_X)).floor() as usize).max(1);
    let tile = ((width - GAP_X * (cols - 1) as f32) / cols as f32).max(80.0);
    (cols, tile)
}

impl ViewCtx<'_> {
    pub(super) fn library_page(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let count = self.books.len();
        let mut sort = self.sort;
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Books").font(FontId::new(24.0, semibold())).color(t.text));
            if count > 0 {
                let sub = if count == 1 { "1 book".to_owned() } else { format!("{count} books") };
                ui.label(egui::RichText::new(sub).size(13.0).color(t.muted));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    changed = crate::widgets::choice(ui, &LibrarySort::ALL, &mut sort, |s| s.label().into());
                });
            });
        });
        if changed {
            self.actions.push(Action::Prefs(PrefsAction::Sort(sort)));
        }
        ui.add_space(12.0);
        if count == 0 {
            let (title, body) = if self.session.connecting || self.sync.running {
                ("Loading your library…", "")
            } else {
                ("No books yet", "Books show up here as they're added to the server's library.")
            };
            shell::widgets::empty_state(ui, Icon::Headphones, title, body);
            return;
        }
        let ids = self.books.order(sort);
        self.book_grid(ui, &ids, sort);
    }

    /// The grid: only the rows on screen are laid out, so only their covers are
    /// asked for.
    fn book_grid(&mut self, ui: &mut Ui, ids: &[usize], sort: LibrarySort) {
        let (cols, tile) = grid_metrics(ui.available_width());
        let cover_h = tile;
        let row_h = cover_h + LABEL_H + GAP_Y;
        let rows = ids.len().div_ceil(cols);
        let row_ids = |row: usize| &ids[row * cols..((row + 1) * cols).min(ids.len())];
        VirtualList::grid(("books", sort), row_h, rows).show(ui, |ui, range| {
            for row in range.clone() {
                self.book_row(ui, row_ids(row), tile, cover_h);
            }
            // The rows just above and below load next, after what's on screen.
            for row in near(&range, PREFETCH_ROWS, rows) {
                for &i in row_ids(row) {
                    if let Some(cover) = self.books.all()[i].cover.as_deref() {
                        self.art.prefetch(cover, tile);
                    }
                }
            }
        });
    }

    /// One grid row of covers, followed by the row gap.
    fn book_row(&mut self, ui: &mut Ui, ids: &[usize], tile: f32, cover_h: f32) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = GAP_X;
            for &i in ids {
                self.book_tile(ui, i, tile, cover_h);
            }
        });
        ui.add_space(GAP_Y - ui.spacing().item_spacing.y);
    }

    /// Cover, title and author. A click opens the book.
    fn book_tile(&mut self, ui: &mut Ui, idx: usize, tile: f32, cover_h: f32) {
        let t = Tokens::of(ui);
        let (rect, resp) = ui.allocate_exact_size(vec2(tile, cover_h + LABEL_H), Sense::click());
        let book = &self.books.all()[idx];
        if ui.is_rect_visible(rect) {
            let art = Rect::from_min_size(rect.min, vec2(tile, cover_h));
            self.art.paint(ui, art, book.cover.as_deref(), &book.title, 5.0);
            let title = elide(ui, &book.title, FontId::proportional(13.0), t.text, tile);
            ui.painter().galley(pos2(rect.left(), art.bottom() + 7.0), title, t.text);
            let author = author_of(book);
            let author = if author.is_empty() { "Unknown author" } else { author };
            let line = elide(ui, author, FontId::proportional(12.0), t.muted, tile);
            ui.painter().galley(pos2(rect.left(), art.bottom() + 25.0), line, t.muted);
        }
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{} by {}", book.title, authors_line(book))));
        if resp.clicked() {
            self.actions.push(Action::Go(Route::Book(book.id.clone())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sources::Stamp;

    fn book(id: &str, title: &str, author: &str, added: i64) -> ItemSummary {
        ItemSummary {
            id: id.into(),
            library: "books".into(),
            title: title.into(),
            authors: if author.is_empty() { Vec::new() } else { vec![author.into()] },
            added: Some(Stamp::from_millis(added)),
            ..ItemSummary::default()
        }
    }

    fn titles(books: &[ItemSummary], sort: LibrarySort) -> Vec<String> {
        sorted_ids(books, sort).into_iter().map(|i| books[i].title.clone()).collect()
    }

    fn shelf() -> Vec<ItemSummary> {
        vec![
            book("c", "the Dispossessed", "Le Guin, Ursula K.", 3_000),
            book("a", "Dune", "Herbert, Frank", 1_000),
            book("b", "A Wizard of Earthsea", "Le Guin, Ursula K.", 2_000),
            book("d", "Untitled", "", 0),
        ]
    }

    #[test]
    fn titles_and_authors_sort_without_case() {
        assert_eq!(titles(&shelf(), LibrarySort::Title), ["A Wizard of Earthsea", "Dune", "the Dispossessed", "Untitled"]);
        // The author as the server spells it, so "Le Guin, Ursula K." files
        // under L, and the nameless book last.
        assert_eq!(titles(&shelf(), LibrarySort::Author), ["Dune", "A Wizard of Earthsea", "the Dispossessed", "Untitled"]);
    }

    #[test]
    fn recently_added_is_newest_first_and_unstamped_last() {
        assert_eq!(titles(&shelf(), LibrarySort::Added), ["the Dispossessed", "A Wizard of Earthsea", "Dune", "Untitled"]);
    }

    #[test]
    fn a_listing_keeps_what_was_fetched_about_books_that_stay() {
        let mut books = Books::default();
        books.set(shelf());
        books.loaded("a", Item::default());
        assert!(matches!(books.details.get("a"), Some(Details::Loaded(_))));
        // A new listing without that book drops what was fetched for it.
        books.set(vec![book("b", "A Wizard of Earthsea", "Le Guin, Ursula K.", 2_000)]);
        assert!(books.details.is_empty());
        assert_eq!(books.len(), 1);
        // Another account: nothing of this one stays.
        books.clear();
        assert_eq!(books.len(), 0);
    }

    #[test]
    fn authors_read_as_a_sentence() {
        let mut b = book("a", "Dune", "Frank Herbert", 0);
        assert_eq!(authors_line(&b), "Frank Herbert");
        b.authors = vec!["Frank Herbert".into(), "Brian Herbert".into()];
        assert_eq!(authors_line(&b), "Frank Herbert and 1 other");
        b.authors.push("Kevin J. Anderson".into());
        assert_eq!(authors_line(&b), "Frank Herbert and 2 others");
        b.authors.clear();
        assert_eq!(authors_line(&b), "Unknown author");
    }

    #[test]
    fn the_grid_fills_its_width() {
        assert_eq!(grid_metrics(100.0).0, 1);
        let (cols, tile) = grid_metrics(900.0);
        assert_eq!(cols, 4);
        assert!((tile * cols as f32 + GAP_X * (cols - 1) as f32 - 900.0).abs() < 0.01);
    }
}
