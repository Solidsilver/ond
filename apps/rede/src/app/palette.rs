//! ⌘K (or ⌘F, or / outside a text field): find a book and open or play it
//! without leaving the keyboard. The sheet is the kit's, Ond's palette;
//! this says what the rows are (`search` finds them) and what the keys do.
//!
//! ⏎ opens a book's page and ⌘⏎ plays or resumes it, as its page's button
//! would. ⏎ on a series opens its page; an author or narrator has no page
//! yet, so choosing one puts its name in the field, which lists its books.

use super::nav::Route;
use super::search::{BookIndex, Hit, Results};
use super::view_ctx::ViewCtx;
use super::{Action, library};
use egui::Rect;
use shell::icons::Icon;
use shell::kit::{self, PaletteRow, PaletteState};
use shell::search::Matcher;
use sources::ItemSummary;
use std::sync::Arc;

#[derive(Default)]
pub struct Palette {
    /// What's typed, and the row selected.
    pub state: PaletteState,
    results: Results,
    matcher: Matcher,
    /// The index the results came from. Held, so a new listing (a new
    /// index) is told apart from this one and searched again.
    index: Option<Arc<BookIndex>>,
    /// A name was chosen: once its books are listed, the first is selected,
    /// so ⏎ again opens it.
    to_book: bool,
}

impl Palette {
    /// Puts a name in the field; its books are listed next frame.
    pub fn show_name(&mut self, name: String) {
        self.state.replace_query(name);
        self.index = None;
        self.to_book = true;
    }

    /// `query` in the field as if typed: searched next frame, the first row
    /// selected.
    #[cfg(test)]
    pub fn typed(&mut self, query: &str) {
        self.state.replace_query(query.into());
        self.index = None;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Verb {
    Open,
    Play,
}

const BOOK_KEYS: &str = "⏎ open    ⌘⏎ play";
const NAME_KEYS: &str = "⏎ show books";
const SERIES_KEYS: &str = "⏎ open";

const TEXT: kit::PaletteText = kit::PaletteText {
    placeholder: "Search books, authors, narrators and series",
    footer: "⏎ open    ⌘⏎ play or resume    ⏎ on a person lists their books",
    no_matches: "No books match",
};

impl ViewCtx<'_> {
    pub(super) fn palette_ui(&mut self, ctx: &egui::Context) {
        let Some(palette) = self.palette.as_mut() else { return };
        let mut verb = palette_keys(ctx);
        let index = self.books.index();
        let books = Arc::clone(self.books.all());
        let Palette { state, results, matcher, index: searched, to_book } = palette;
        let mut rows = Rows {
            index: &index,
            books: &books,
            positions: self.positions,
            player: self.player,
            speed: self.playback.speed,
            art: &mut *self.art,
            results,
            matcher,
        };
        // Opened, a name chosen, or a new listing: search what's typed.
        if !searched.as_ref().is_some_and(|i| Arc::ptr_eq(i, &index)) {
            *searched = Some(Arc::clone(&index));
            kit::PaletteRows::search(&mut rows, &state.query);
            state.selected = state.selected.min(rows.results.len().saturating_sub(1));
            if std::mem::take(to_book) {
                let first = (0..rows.results.len()).find(|&n| matches!(rows.results.hit(n), Some(Hit::Book(_))));
                state.selected = first.unwrap_or(0);
            }
        }
        let out = kit::palette(ctx, "rede-palette", &TEXT, state, &mut rows);
        if let Some((_, play)) = out.clicked {
            verb = Some(if play { Verb::Play } else { Verb::Open });
        }
        match (verb, rows.results.hit(state.selected)) {
            (Some(verb), Some(Hit::Book(i))) => {
                let id = books[i].id.clone();
                self.actions.push(Action::ClosePalette);
                self.actions.push(if verb == Verb::Play { Action::PlayBook(id) } else { Action::Go(Route::Book(id)) });
            }
            (Some(_), Some(Hit::Name(super::search::Kind::Series, i))) => {
                if let Some(name) = index.name(super::search::Kind::Series, i) {
                    self.actions.push(Action::ClosePalette);
                    self.actions.push(Action::Go(Route::Series(name.name.clone())));
                }
            }
            (Some(_), Some(Hit::Name(kind, i))) => {
                if let Some(name) = index.name(kind, i) {
                    self.actions.push(Action::PaletteName(name.name.clone()));
                }
            }
            _ if out.close => self.actions.push(Action::ClosePalette),
            _ => {}
        }
    }
}

/// The palette's keys this frame, consumed: ⏎, or ⌘⏎. ↑ and ↓ are the
/// kit's.
fn palette_keys(ctx: &egui::Context) -> Option<Verb> {
    use egui::{Key, Modifiers};
    let (play, open) = ctx.input_mut(|i| (i.consume_key(Modifiers::COMMAND, Key::Enter), i.consume_key(Modifiers::NONE, Key::Enter)));
    if play {
        Some(Verb::Play)
    } else if open {
        Some(Verb::Open)
    } else {
        None
    }
}

/// What the rows draw from: the listing and its index, where each book is,
/// and the covers.
struct Rows<'r> {
    index: &'r BookIndex,
    books: &'r [ItemSummary],
    positions: &'r super::progress::Positions,
    player: &'r crate::player::Player,
    speed: f32,
    art: &'r mut crate::art::Art,
    results: &'r mut Results,
    matcher: &'r mut Matcher,
}

impl Rows<'_> {
    /// How far into a book the listener is: "4h 57m left" at the speed
    /// they listen at, "Finished", or nothing for a book not started.
    fn progress(&self, book: &ItemSummary) -> Option<String> {
        let position = if self.player.book.as_ref().is_some_and(|b| b.key.item == book.id) {
            self.player.position
        } else {
            let p = self.positions.get(&book.id)?;
            if p.finished {
                return Some("Finished".into());
            }
            p.position.secs()
        };
        let duration = book.duration.filter(|_| position > 0.0)?;
        let speed = f64::from(crate::timeline::normalize_speed(self.speed));
        Some(crate::timeline::span_left((duration - position).max(0.0) / speed))
    }
}

impl kit::PaletteRows for Rows<'_> {
    fn search(&mut self, query: &str) {
        *self.results = self.index.search(query, self.matcher, self.books, self.positions);
    }

    fn groups(&self) -> Vec<(&'static str, usize)> {
        self.results.groups().to_vec()
    }

    /// A book: its cover, its title over its author and how far in it is.
    /// A name: a quiet circle, the name over what it is and how many books.
    fn row(&mut self, ui: &mut egui::Ui, tile: Rect, n: usize) -> PaletteRow {
        match self.results.hit(n) {
            Some(Hit::Book(i)) => {
                let book = &self.books[i];
                self.art.paint(ui, tile, book.cover.as_deref(), 4.0, &library::stand_in(book));
                let author = library::author_of(book);
                let author = if author.trim().is_empty() { "Unknown author" } else { author };
                let detail = match self.progress(book) {
                    Some(progress) => format!("{author} · {progress}"),
                    None => author.to_owned(),
                };
                PaletteRow { title: book.title.clone(), detail, keys: BOOK_KEYS }
            }
            Some(Hit::Name(kind, i)) => {
                let series = kind == super::search::Kind::Series;
                kit::palette_icon_tile(ui, tile, if series { Icon::List } else { Icon::Person });
                let keys = if series { SERIES_KEYS } else { NAME_KEYS };
                let Some(name) = self.index.name(kind, i) else { return PaletteRow { title: String::new(), detail: String::new(), keys } };
                let books = if name.books == 1 { "1 book".to_owned() } else { format!("{} books", name.books) };
                PaletteRow { title: name.name.clone(), detail: format!("{} · {books}", kind.label()), keys }
            }
            None => PaletteRow { title: String::new(), detail: String::new(), keys: BOOK_KEYS },
        }
    }
}
