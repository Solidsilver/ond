//! The books the shelf has, and the page that draws them: a grid of covers,
//! narrowed to those not started, in progress or finished, and sorted by
//! title, author, when they were added, when they were last heard, or how far
//! along they are.
//!
//! The rows are laid out only while they're on screen ([`VirtualList`]), so a
//! library of any size draws a screenful and asks for the covers it shows.
//! The order and the filtered list are worked out once per library version,
//! order and filter, and per positions version where they depend on where the
//! books are ([`crate::memo::Memo`]), so a frame lays out what it can rather
//! than sorting.

use super::Action;
use super::downloads_page::BookDownload;
use super::nav::Route;
use super::prefs_view::PrefsAction;
use super::progress::{Listened, Positions, Stage};
use super::view_ctx::ViewCtx;
use super::virtual_list::{VirtualList, near};
use crate::memo::Memo;
use crate::settings::{LibraryFilter, LibrarySort};
use crate::widgets::elide;
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};
use shell::icons::Icon;
use shell::theme::Tokens;
use sources::{Item, ItemSummary};
use std::collections::HashMap;
use std::sync::Arc;

/// The narrowest a cover may be drawn; the grid fits as many columns as the
/// width allows.
const TILE_MIN: f32 = 172.0;
const GAP_X: f32 = 20.0;
const GAP_Y: f32 = 24.0;
/// A cover's corners, as Ond rounds an album's.
pub const COVER_RADIUS: f32 = 6.0;
/// Title, author, and the time left or "Finished" under the art. Every tile
/// keeps room for the third line, so the rows line up whatever is begun.
const LABEL_H: f32 = 58.0;
/// The rows just past either end whose covers load ahead.
const PREFETCH_ROWS: usize = 1;
/// Between the filter and the order's menu at the right, at the least: what
/// decides when the heading's row runs out of room.
const HEADING_GAP: f32 = 24.0;
/// The sort menu: its longest order and the check's gutter.
const SORT_MENU_W: f32 = 176.0;
/// A book's menu, on a right-click or behind its page's …: its longest
/// command.
pub const BOOK_MENU_W: f32 = 190.0;
/// Room the download's mark takes at the end of a tile's author line.
const MARK_ROOM: f32 = 16.0;

/// The shelf's books, in the order the sync stored them, and what has been
/// fetched about each one since.
#[derive(Default)]
pub struct Books {
    /// Bumped by every new listing, so derived data can be memoised on it.
    version: u64,
    all: Arc<Vec<ItemSummary>>,
    /// Indices into `all`, in the order drawn, per library version and sort
    /// (and positions version, for an order that follows them).
    order: Memo<OrderKey, Vec<usize>>,
    /// That order narrowed by the filter: per the same, the positions
    /// version (whatever the order), the filter, and the loaded book's stage
    /// while the player is ahead of its saved position.
    shown: Memo<ShownKey, Vec<usize>>,
    /// Home's lists: the books in progress, per library and positions
    /// version, and the newest, per library version.
    in_progress: Memo<(u64, u64), Vec<usize>>,
    recent: Memo<u64, Vec<usize>>,
    /// A book's details once its page asked for them. By the book's own id:
    /// one account is shown for now.
    pub details: HashMap<String, Details>,
    /// Descriptions read as prose, by the book and the text it was given: a
    /// page drawn every frame parses its markup once.
    descriptions: Memo<(String, String), Vec<shell::rich::Block>>,
    /// What the palette searches, per library version.
    index: Memo<u64, super::search::BookIndex>,
    /// Each book's download, and the Downloads page's rows, per library
    /// version and downloads version.
    downloads: Memo<(u64, u64), HashMap<String, BookDownload>>,
    download_rows: Memo<(u64, u64), Vec<super::downloads_page::DownloadRow>>,
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
    pub fn order(&mut self, sort: LibrarySort, positions: &Positions) -> Arc<Vec<usize>> {
        let key = self.order_key(sort, positions);
        let books = &self.all;
        self.order.get(key, || sorted_ids(books, sort, positions))
    }

    /// Indices into [`all`](Self::all) of the books `filter` lets through,
    /// in the order `sort` draws them. `loaded` is the book in the player
    /// with its stage, when that's ahead of its saved position.
    /// Downloaded keeps the books whose every file is here, by `downloads`.
    pub fn shown(
        &mut self,
        sort: LibrarySort,
        filter: LibraryFilter,
        positions: &Positions,
        loaded: Option<(String, Stage)>,
        downloads: &crate::downloads::State,
    ) -> Arc<Vec<usize>> {
        let order = self.order(sort, positions);
        if filter == LibraryFilter::All {
            return order;
        }
        // The downloads matter only to their own filter, so a download's
        // progress doesn't filter the shelf again by stage.
        let downloads_version = if filter == LibraryFilter::Downloaded { downloads.version } else { 0 };
        let here = (filter == LibraryFilter::Downloaded).then(|| self.downloads(downloads));
        let key = (self.order_key(sort, positions), positions.version(), filter, loaded, downloads_version);
        let books = &self.all;
        let loaded = key.3.clone();
        self.shown.get(key, || {
            if let Some(here) = here {
                return order.iter().copied().filter(|&i| matches!(here.get(&books[i].id), Some(BookDownload::Done { .. }))).collect();
            }
            let stage = |i: usize| match &loaded {
                Some((id, stage)) if *id == books[i].id => *stage,
                _ => saved(&books[i], positions).stage(),
            };
            order.iter().copied().filter(|&i| admits(filter, stage(i))).collect()
        })
    }

    /// Each book's download, by its own id, worked out once per change to
    /// the downloads.
    pub fn downloads(&mut self, state: &crate::downloads::State) -> Arc<HashMap<String, BookDownload>> {
        self.downloads.get((self.version, state.version), || super::downloads_page::book_downloads(state))
    }

    /// The Downloads page's rows, worked out once per change to the library
    /// or the downloads.
    pub fn download_rows(&mut self, state: &crate::downloads::State) -> Arc<Vec<super::downloads_page::DownloadRow>> {
        let downloads = self.downloads(state);
        let books = &self.all;
        self.download_rows.get((self.version, state.version), || super::downloads_page::download_rows(books, state, &downloads))
    }

    fn order_key(&self, sort: LibrarySort, positions: &Positions) -> OrderKey {
        (self.version, sort, if sort.by_position() { positions.version() } else { 0 })
    }

    /// Indices into [`all`](Self::all) of the books in progress, the one
    /// listened to last first.
    pub fn in_progress(&mut self, positions: &Positions) -> Arc<Vec<usize>> {
        let books = &self.all;
        self.in_progress.get((self.version, positions.version()), || super::progress::in_progress(books, positions))
    }

    /// Indices into [`all`](Self::all), newest first. Its own memo, so Home
    /// doesn't undo the library's order.
    pub fn recent(&mut self) -> Arc<Vec<usize>> {
        let books = &self.all;
        self.recent.get(self.version, || sorted_ids(books, LibrarySort::Added, &Positions::default()))
    }

    /// The palette's haystack for this listing, built once per version: a
    /// keystroke only scores it.
    pub fn index(&mut self) -> Arc<super::search::BookIndex> {
        let books = &self.all;
        self.index.get(self.version, || super::search::BookIndex::new(books))
    }

    pub fn loaded(&mut self, id: &str, item: Item) {
        self.details.insert(id.to_owned(), Details::Loaded(Arc::new(item)));
    }

    pub fn failed(&mut self, id: &str, error: crate::failure::Failure) {
        self.details.insert(id.to_owned(), Details::Failed(error.into_message()));
    }

    /// A book's description as blocks, parsed once for each text it has: a
    /// frame that only redraws (a hover, a repaint while playing) reads the
    /// markup no further.
    pub fn description(&mut self, id: &str, text: &str) -> Arc<Vec<shell::rich::Block>> {
        self.descriptions.get((id.to_owned(), text.to_owned()), || shell::rich::parse(text))
    }
}

/// The library version, the order, and the positions version when the order
/// follows them (0 when it doesn't, so a position saved while playing doesn't
/// sort the shelf by title again).
type OrderKey = (u64, LibrarySort, u64);

/// The order's key, the positions version, the filter, the loaded book
/// with its stage while the player is ahead of what was saved for it, and
/// the downloads version when the filter is Downloaded (0 otherwise).
type ShownKey = (OrderKey, u64, LibraryFilter, Option<(String, Stage)>, u64);

/// A book's listening as last saved: what the orders and the filter go by.
fn saved(book: &ItemSummary, positions: &Positions) -> Listened {
    Listened::of(book, positions.get(&book.id), None, 1.0)
}

/// Whether `filter` shows a book at `stage`.
pub fn admits(filter: LibraryFilter, stage: Stage) -> bool {
    match filter {
        LibraryFilter::All => true,
        LibraryFilter::InProgress => stage == Stage::InProgress,
        LibraryFilter::NotStarted => stage == Stage::NotStarted,
        LibraryFilter::Finished => stage == Stage::Finished,
        // Not a stage: `Books::shown` asks the downloads instead.
        LibraryFilter::Downloaded => true,
    }
}

/// Indices into `books`, ordered by `sort`. Title and author compare without
/// case; ties fall back to the title and then the id, so the order is stable.
/// The orders by listening read `positions`; the others ignore it.
pub fn sorted_ids(books: &[ItemSummary], sort: LibrarySort, positions: &Positions) -> Vec<usize> {
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
        // The book heard last first, whether it was finished or not; a book
        // never heard, or heard at a time the source didn't say, goes last.
        LibrarySort::Listened => ids.sort_by_cached_key(|&i| {
            let b = &books[i];
            let at = positions.get(&b.id).and_then(|p| p.at);
            (std::cmp::Reverse(at), b.title.to_lowercase(), b.id.clone())
        }),
        // Finished books first, then the rest by how much is behind them,
        // those whose length isn't known after those it is, and the unstarted
        // last.
        LibrarySort::Progress => ids.sort_by_cached_key(|&i| {
            let b = &books[i];
            let rank = match saved(b, positions) {
                Listened::Finished => (0, 0),
                Listened::InProgress { fraction: Some(f), .. } => (1, u32::MAX - (f.clamp(0.0, 1.0) * 1_000_000.0) as u32),
                Listened::InProgress { fraction: None, .. } => (2, 0),
                Listened::NotStarted => (3, 0),
            };
            (rank, b.title.to_lowercase(), b.id.clone())
        }),
    }
    ids
}

/// What the library says when the filter leaves nothing to show: what
/// would put books there.
fn filter_empty(filter: LibraryFilter) -> (&'static str, &'static str) {
    match filter {
        LibraryFilter::All => ("No books yet", ""),
        LibraryFilter::InProgress => ("Nothing in progress", "Books you've started and haven't finished show up here."),
        LibraryFilter::NotStarted => ("Nothing left to start", "Every book has been started. New ones show up here as they're added."),
        LibraryFilter::Finished => ("Nothing finished yet", "Books you listen to the end show up here."),
        LibraryFilter::Downloaded => ("Nothing downloaded", "Books you download show up here, and play without a connection."),
    }
}

/// The name a book is filed under: its first author, spelled as the server
/// spells it.
pub fn author_of(book: &ItemSummary) -> &str {
    book.authors.first().map_or("", String::as_str)
}

/// The cover a book without art gets: its title and first author, on a
/// ground of its own.
pub fn stand_in(book: &ItemSummary) -> shell::kit::Generated<'_> {
    shell::kit::Generated { title: &book.title, byline: book.authors.first().map(String::as_str), seed: &book.id }
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
        if self.books.len() == 0 {
            shell::kit::page_heading(ui, "Books", None);
            ui.add_space(12.0);
            self.no_books(ui);
            return;
        }
        let loaded = if self.filter == LibraryFilter::All { None } else { self.loaded_stage() };
        let ids = self.books.shown(self.sort, self.filter, self.positions, loaded, self.downloads);
        self.library_heading(ui, ids.len());
        if ids.is_empty() {
            let (title, body) = filter_empty(self.filter);
            shell::kit::empty_state(ui, Icon::Headphones, title, body);
            return;
        }
        self.book_grid(ui, &ids, (self.sort, self.filter));
    }

    /// "Books", how many are shown, the filter, and the order's menu at the
    /// right end. Where they don't all fit on one line the count goes first,
    /// and then the filter moves to a line of its own under the heading.
    fn library_heading(&mut self, ui: &mut Ui, count: usize) {
        let t = Tokens::of(ui);
        let width = |ui: &Ui, text: &str, font: FontId| ui.painter().layout_no_wrap(text.to_owned(), font, t.text).size().x;
        let count_text = match count {
            0 => None,
            1 => Some("1 book".to_owned()),
            n => Some(format!("{n} books")),
        };
        let sort_text = format!("Sort: {}", self.sort.label());
        let spacing = ui.spacing().item_spacing.x;
        let title_w = width(ui, "Books", FontId::new(24.0, shell::theme::semibold()));
        let count_w = count_text.as_deref().map_or(0.0, |c| spacing + width(ui, c, FontId::proportional(13.0)));
        // The pill sits a gap past the heading as the old order pill did: the
        // spacing either side of the space before it.
        // The sort button and the pill as the kit sizes them: a label and a
        // chevron, and segments with their comfortable padding.
        let sort_w = width(ui, &sort_text, FontId::proportional(12.0)) + 26.0;
        let pill_w = 4.0 + LibraryFilter::ALL.iter().map(|f| width(ui, f.label(), FontId::proportional(12.0)) + 22.0).sum::<f32>();
        let room = ui.available_width();
        let one_line = title_w + 3.0 * spacing + pill_w + HEADING_GAP + sort_w;
        let (wrap, show_count) = if one_line + count_w <= room {
            (false, true)
        } else if one_line <= room {
            (false, false)
        } else {
            (true, title_w + count_w + HEADING_GAP + sort_w <= room)
        };
        let mut filter = self.filter;
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Books").font(FontId::new(24.0, shell::theme::semibold())).color(t.text));
            if let Some(count) = count_text.filter(|_| show_count) {
                ui.label(egui::RichText::new(count).size(13.0).color(t.muted));
            }
            if !wrap {
                ui.add_space(spacing);
                changed = crate::widgets::choice(ui, &LibraryFilter::ALL, &mut filter, |f| f.label().into());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.sort_button(ui, &sort_text));
        });
        if wrap {
            ui.add_space(8.0);
            ui.horizontal(|ui| changed = crate::widgets::choice(ui, &LibraryFilter::ALL, &mut filter, |f| f.label().into()));
        }
        if changed {
            self.actions.push(Action::Prefs(PrefsAction::Filter(filter)));
        }
        ui.add_space(12.0);
    }

    /// "Sort: Title" and a chevron, quiet at the heading's right end, opening
    /// the orders with the current one checked.
    fn sort_button(&mut self, ui: &mut Ui, text: &str) {
        let t = Tokens::of(ui);
        let resp = shell::kit::menu_label(ui, text, t.muted);
        shell::kit::popover_at(&resp, SORT_MENU_W, egui::RectAlign::BOTTOM_END, |ui| {
            for sort in LibrarySort::ALL {
                if shell::kit::check_row(ui, sort.label(), sort == self.sort) && sort != self.sort {
                    self.actions.push(Action::Prefs(PrefsAction::Sort(sort)));
                }
            }
        });
    }

    /// What a page of books says with none: loading, or how they come.
    pub(super) fn no_books(&self, ui: &mut Ui) {
        let (title, body) = if self.session.connecting || self.sync.running {
            ("Loading your library…", "")
        } else {
            ("No books yet", "Books show up here as they're added to the server's library.")
        };
        shell::kit::empty_state(ui, Icon::Headphones, title, body);
    }

    /// The grid: only the rows on screen are laid out, so only their covers are
    /// asked for.
    fn book_grid(&mut self, ui: &mut Ui, ids: &[usize], list: (LibrarySort, LibraryFilter)) {
        let (cols, tile) = grid_metrics(ui.available_width());
        let cover_h = tile;
        let row_h = cover_h + LABEL_H + GAP_Y;
        let rows = ids.len().div_ceil(cols);
        let row_ids = |row: usize| &ids[row * cols..((row + 1) * cols).min(ids.len())];
        VirtualList::grid(("books", list), row_h, rows).show(ui, |ui, range| {
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
    pub(super) fn book_row(&mut self, ui: &mut Ui, ids: &[usize], tile: f32, cover_h: f32) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = GAP_X;
            for &i in ids {
                self.book_tile(ui, i, tile, cover_h);
            }
        });
        ui.add_space(GAP_Y - ui.spacing().item_spacing.y);
    }

    /// Cover, title and author, and how far along the book is: a line along
    /// the cover's foot and the time left, or a check and "Finished"; the
    /// download's mark at the author line's end. Offline, a book that isn't
    /// all here is faded: it can't play. A click opens the book; a
    /// right-click has its commands.
    fn book_tile(&mut self, ui: &mut Ui, idx: usize, tile: f32, cover_h: f32) {
        let t = Tokens::of(ui);
        let (rect, resp) = ui.allocate_exact_size(vec2(tile, cover_h + LABEL_H), Sense::click());
        let id = self.books.all()[idx].id.clone();
        let download = self.download(&id);
        let dim = self.sync.offline && !matches!(download, Some(BookDownload::Done { .. }));
        let book = &self.books.all()[idx];
        let listened = self.listened(book);
        if ui.is_rect_visible(rect) {
            let opacity = ui.opacity();
            if dim {
                ui.multiply_opacity(super::downloads_page::DIMMED);
            }
            let art = Rect::from_min_size(rect.min, vec2(tile, cover_h));
            self.art.paint(ui, art, book.cover.as_deref(), COVER_RADIUS, &stand_in(book));
            match listened {
                Listened::InProgress { fraction: Some(f), .. } => shell::kit::cover_progress(ui.painter(), art, COVER_RADIUS, f, &t),
                Listened::Finished => shell::kit::cover_badge(ui.painter(), art, Icon::Check, &t),
                _ => {}
            }
            let title = elide(ui, &book.title, FontId::proportional(13.0), t.text, tile);
            ui.painter().galley(pos2(rect.left(), art.bottom() + 7.0), title, t.text);
            let author = author_of(book);
            let author = if author.is_empty() { "Unknown author" } else { author };
            let mark_room = if download.is_some() { MARK_ROOM } else { 0.0 };
            let line = elide(ui, author, FontId::proportional(12.0), t.muted, tile - mark_room);
            let line_y = art.bottom() + 24.0;
            let line_h = line.size().y;
            ui.painter().galley(pos2(rect.left(), line_y), line, t.muted);
            if let Some(d) = &download {
                shell::kit::download_mark(ui, pos2(rect.right() - 6.0, line_y + line_h / 2.0), 11.0, d.mark());
            }
            if let Some(state) = listened.line() {
                let line = elide(ui, &state, FontId::proportional(11.5), t.muted, tile);
                ui.painter().galley(pos2(rect.left(), art.bottom() + 40.0), line, t.muted);
            }
            ui.set_opacity(opacity);
        }
        let mut name = match listened.line() {
            Some(state) => format!("{} by {}, {}", book.title, authors_line(book), state.to_lowercase()),
            None => format!("{} by {}", book.title, authors_line(book)),
        };
        if dim {
            name.push_str(", not downloaded");
        } else if let Some(d) = &download {
            name.push_str(&format!(", {}", d.mark().hint().to_lowercase()));
        }
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
        let mut chosen = None;
        resp.context_menu(|ui| {
            ui.set_width(BOOK_MENU_W);
            chosen = self.book_commands(ui, &id);
        });
        if let Some(action) = chosen {
            self.actions.push(action);
        } else if resp.clicked() {
            self.actions.push(Action::Go(Route::Book(id)));
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
        titles_with(books, sort, &Positions::default())
    }

    fn titles_with(books: &[ItemSummary], sort: LibrarySort, positions: &Positions) -> Vec<String> {
        sorted_ids(books, sort, positions).into_iter().map(|i| books[i].title.clone()).collect()
    }

    fn named(books: &[ItemSummary], ids: &[usize]) -> Vec<String> {
        ids.iter().map(|&i| books[i].title.clone()).collect()
    }

    /// `secs` into a ten-hour book, `hours_ago` (or undated).
    fn heard(positions: &mut Positions, id: &str, secs: f64, finished: bool, hours_ago: Option<u64>) {
        let at = hours_ago.map(|h| std::time::SystemTime::now() - std::time::Duration::from_secs(h * 3_600));
        let item = sources::ItemKey::new(sources::AccountId::from_uuid(uuid::Uuid::nil()), id);
        positions.insert(id.into(), sources::Progress { item, position: sources::Position::from_secs(secs), finished, at });
    }

    /// Six ten-hour books: two partway, one finished, one begun at a time
    /// the source didn't say, and two never begun.
    fn listened_shelf() -> (Vec<ItemSummary>, Positions) {
        let books: Vec<ItemSummary> = [("a", "Alpha"), ("b", "Beta"), ("c", "Gamma"), ("d", "Delta"), ("e", "Epsilon"), ("f", "Zeta")]
            .into_iter()
            .map(|(id, title)| ItemSummary { duration: Some(36_000.0), ..book(id, title, "", 0) })
            .collect();
        let mut positions = Positions::default();
        heard(&mut positions, "a", 3_600.0, false, Some(30));
        heard(&mut positions, "b", 27_000.0, false, Some(2));
        heard(&mut positions, "c", 36_000.0, true, Some(10));
        heard(&mut positions, "d", 1_800.0, false, None);
        (books, positions)
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
    fn recently_listened_is_the_last_heard_first_and_the_undated_and_unheard_last() {
        let (books, positions) = listened_shelf();
        // Finished or not, by when; then by title, those with no date and
        // those never begun alike.
        assert_eq!(titles_with(&books, LibrarySort::Listened, &positions), ["Beta", "Gamma", "Alpha", "Delta", "Epsilon", "Zeta"]);
    }

    #[test]
    fn progress_is_the_most_complete_first_and_the_unstarted_last() {
        let (mut books, positions) = listened_shelf();
        assert_eq!(titles_with(&books, LibrarySort::Progress, &positions), ["Gamma", "Beta", "Alpha", "Delta", "Epsilon", "Zeta"]);
        // A book begun whose length isn't known can't say how far along it
        // is: after those that can, before those not begun.
        books[1].duration = None;
        assert_eq!(titles_with(&books, LibrarySort::Progress, &positions), ["Gamma", "Alpha", "Delta", "Beta", "Epsilon", "Zeta"]);
    }

    #[test]
    fn the_filter_keeps_the_order_and_is_worked_out_once_per_change() {
        let (list, mut positions) = listened_shelf();
        let mut books = Books::default();
        books.set(list.clone());
        let none = crate::downloads::State::default();
        let shown = |books: &mut Books, filter, positions: &Positions| books.shown(LibrarySort::Title, filter, positions, None, &none);
        assert_eq!(named(&list, &shown(&mut books, LibraryFilter::InProgress, &positions)), ["Alpha", "Beta", "Delta"]);
        assert_eq!(named(&list, &shown(&mut books, LibraryFilter::NotStarted, &positions)), ["Epsilon", "Zeta"]);
        assert_eq!(named(&list, &shown(&mut books, LibraryFilter::Finished, &positions)), ["Gamma"]);
        assert_eq!(shown(&mut books, LibraryFilter::All, &positions).len(), list.len());
        let first = shown(&mut books, LibraryFilter::Finished, &positions);
        assert!(Arc::ptr_eq(&first, &shown(&mut books, LibraryFilter::Finished, &positions)), "a frame that only redraws doesn't filter again");
        // A position saved moves a book across.
        heard(&mut positions, "a", 36_000.0, true, Some(0));
        assert_eq!(named(&list, &shown(&mut books, LibraryFilter::Finished, &positions)), ["Alpha", "Gamma"]);
        // The book in the player, started before its first report, counts as
        // begun.
        let loaded = Some(("e".to_owned(), Stage::InProgress));
        assert_eq!(named(&list, &books.shown(LibrarySort::Title, LibraryFilter::NotStarted, &positions, loaded, &none)), ["Zeta"]);
    }

    #[test]
    fn downloaded_keeps_the_books_all_here_and_follows_the_downloads_version() {
        let (list, positions) = listened_shelf();
        let mut books = Books::default();
        books.set(list.clone());
        let account = sources::AccountId::from_uuid(uuid::Uuid::nil());
        let summary = |id: &str, done: usize| offline::PinSummary {
            pin: offline::Pin::Book(sources::ItemKey::new(account, id).to_string()),
            label: id.into(),
            total: 2,
            done,
            failed: 0,
            bytes_done: 0,
            bytes_total: 0,
            downloading: done < 2,
        };
        let mut downloads = crate::downloads::State {
            snapshot: Arc::new(offline::Snapshot { pins: vec![summary("b", 2), summary("e", 1)], ..Default::default() }),
            version: 1,
            ..Default::default()
        };
        let shown =
            |books: &mut Books, downloads: &crate::downloads::State| books.shown(LibrarySort::Title, LibraryFilter::Downloaded, &positions, None, downloads);
        assert_eq!(named(&list, &shown(&mut books, &downloads)), ["Beta"], "a book still on its way isn't downloaded yet");
        let first = shown(&mut books, &downloads);
        assert!(Arc::ptr_eq(&first, &shown(&mut books, &downloads)));
        downloads.snapshot = Arc::new(offline::Snapshot { pins: vec![summary("b", 2), summary("e", 2)], ..Default::default() });
        downloads.version = 2;
        assert_eq!(named(&list, &shown(&mut books, &downloads)), ["Beta", "Epsilon"]);
    }

    #[test]
    fn an_order_by_title_is_not_sorted_again_when_a_position_changes_but_one_by_listening_is() {
        let (list, mut positions) = listened_shelf();
        let mut books = Books::default();
        books.set(list);
        let by_title = books.order(LibrarySort::Title, &positions);
        heard(&mut positions, "e", 60.0, false, Some(1));
        assert!(Arc::ptr_eq(&by_title, &books.order(LibrarySort::Title, &positions)));
        let by_listening = books.order(LibrarySort::Listened, &positions);
        heard(&mut positions, "f", 60.0, false, Some(0));
        let again = books.order(LibrarySort::Listened, &positions);
        assert!(!Arc::ptr_eq(&by_listening, &again));
        assert_eq!(again[0], 5, "the book just heard comes first");
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
    fn a_description_is_read_once_for_each_book_and_text() {
        let mut books = Books::default();
        let text = "<b>Praise</b> for the book &amp; its author.";
        let first = books.description("a", text);
        let again = books.description("a", text);
        assert!(Arc::ptr_eq(&first, &again), "the same description is not read twice");
        assert_eq!(first[0].runs[0].text, "Praise", "and it reads as prose");
        let other_text = "<i>Another</i> description.";
        let other = books.description("a", other_text);
        assert!(!Arc::ptr_eq(&first, &other), "a new text is read anew");
        assert_eq!(other[0].runs[0].text, "Another");
        assert!(!Arc::ptr_eq(&other, &books.description("b", other_text)), "and another book's text is its own");
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
