//! The fixture the tests and the review snapshots share: a shelf of books in a
//! temp folder, and an app showing it.
//!
//! No server, no keychain, no audio device: the shelf is written directly, the
//! same way a sync would, and the app reads it back. A book's details are
//! stored the way `open_book` stores them, so the page draws the chapters it
//! would draw for real.

use super::*;
use crate::settings::{Account, Paths};
use sources::{Changes, Chapter, Item, ItemKey, ItemSummary, Library, LibraryKind, Series, Stamp, TrackFile};
use std::path::Path;

/// The account the fixture is signed in to. Nothing talks to it.
pub const SERVER: &str = "https://books.example.com";
pub const LIBRARY: &str = "lib-books";
/// The first of January 2026, in milliseconds: the day the fixture's oldest
/// book was added.
const ADDED: i64 = 1_767_225_600_000;

pub fn account() -> Account {
    Account::new(SERVER, "sam")
}

/// One of the fixture's books, as a listing would describe it.
pub struct FixtureBook {
    pub title: &'static str,
    pub author: &'static str,
    pub narrator: &'static str,
    /// The series, and the book's number in it.
    pub series: Option<(&'static str, &'static str)>,
    /// Seconds.
    pub duration: f64,
    /// Days after the oldest book that it was added.
    pub days: i64,
}

const fn fixture(
    title: &'static str,
    author: &'static str,
    narrator: &'static str,
    series: Option<(&'static str, &'static str)>,
    duration: f64,
    days: i64,
) -> FixtureBook {
    FixtureBook { title, author, narrator, series, duration, days }
}

/// Twelve books, enough to fill a grid and to have two of each author.
pub const BOOKS: [FixtureBook; 12] = [
    fixture("The Left Hand of Darkness", "Ursula K. Le Guin", "George Guidall", Some(("Hainish", "1")), 40_920.0, 11),
    fixture("Dune", "Frank Herbert", "Scott Brick, Simon Vance", Some(("Dune", "1")), 75_720.0, 10),
    fixture("Piranesi", "Susanna Clarke", "Chiwetel Ejiofor", None, 25_800.0, 9),
    fixture("The Dispossessed", "Ursula K. Le Guin", "Don Leslie", Some(("Hainish", "6")), 48_480.0, 8),
    fixture("A Wizard of Earthsea", "Ursula K. Le Guin", "Rob Inglis", Some(("Earthsea", "1")), 25_500.0, 7),
    fixture("Kindred", "Octavia E. Butler", "Kim Staunton", None, 39_300.0, 6),
    fixture("The Remains of the Day", "Kazuo Ishiguro", "Simon Prebble", None, 32_280.0, 5),
    fixture("Wolf Hall", "Hilary Mantel", "Simon Slater", Some(("Thomas Cromwell", "1")), 87_120.0, 4),
    fixture("Station Eleven", "Emily St. John Mandel", "Kirsten Potter", None, 38_460.0, 3),
    fixture("The Fifth Season", "N. K. Jemisin", "Robin Miles", Some(("The Broken Earth", "1")), 55_440.0, 2),
    fixture("Jane Eyre", "Charlotte Brontë", "Thandiwe Newton", None, 69_120.0, 1),
    fixture("The Ocean at the End of the Lane", "Neil Gaiman", "Neil Gaiman", None, 20_820.0, 0),
];

/// The fixture's books as a listing has them.
pub fn summaries() -> Vec<ItemSummary> {
    BOOKS
        .iter()
        .enumerate()
        .map(|(n, fixture)| {
            let mut book = summary(n);
            book.title = fixture.title.into();
            book.authors = vec![fixture.author.into()];
            book.narrators = vec![fixture.narrator.into()];
            book.series = fixture.series.map(|(name, number)| vec![Series { name: name.into(), sequence: Some(number.into()) }]).unwrap_or_default();
            book.duration = Some(fixture.duration);
            book.added = Some(Stamp::from_millis(ADDED + fixture.days * 86_400_000));
            book
        })
        .collect()
}

/// The `n`th fixture book as the listing has it, before the fixture's own
/// titles are put on it.
fn summary(n: usize) -> ItemSummary {
    ItemSummary {
        id: format!("book-{n}"),
        library: LIBRARY.into(),
        title: format!("Book {n}"),
        authors: vec!["Ursula K. Le Guin".into()],
        cover: Some(format!("book-{n}")),
        updated: Some(Stamp::from_millis(ADDED)),
        added: Some(Stamp::from_millis(ADDED)),
        duration: Some(40_000.0),
        ..ItemSummary::default()
    }
}

/// A book's details: a description and a chapter list, as `Source::item` would
/// bring them.
pub fn details(n: usize) -> Item {
    let fixture = &BOOKS[n];
    let (title, author) = (fixture.title, fixture.author);
    let mut item = Item { summary: ItemSummary { title: title.into(), authors: vec![author.into()], ..summary(n) }, ..Item::default() };
    item.description = Some(format!(
        "{title} is one of the fixture's books, here to show what a book's page looks like. \
         The description runs to a few lines of prose, which is all the page needs to lay out: \
         a paragraph that wraps in its column, a little longer than the metadata above it."
    ));
    item.genres = vec!["Science Fiction".into()];
    item.published_year = Some("1969".into());
    item.publisher = Some("Ace Books".into());
    item.tracks = vec![
        TrackFile { index: 1, start_offset: 0.0, duration: Some(20_000.0), suffix: Some("m4b".into()), content_id: format!("{n}-1"), ..TrackFile::default() },
        TrackFile {
            index: 2,
            start_offset: 20_000.0,
            duration: Some(20_460.0),
            suffix: Some("m4b".into()),
            content_id: format!("{n}-2"),
            ..TrackFile::default()
        },
    ];
    // Twelve chapters, a little over 56 minutes each: enough rows to see the
    // list as a list.
    item.chapters = (0..12)
        .map(|c| {
            let start = f64::from(c) * 3_400.0;
            let title = if c == 0 { "Prologue".to_owned() } else { format!("Chapter {}", c + 1) };
            Chapter { start, end: start + 3_400.0, title }
        })
        .collect();
    item
}

/// A shelf holding the fixture's books, with the first book's details stored.
pub fn shelf(dir: &Path) -> std::sync::Arc<shelf::Shelf> {
    let shelf = shelf::Shelf::open(dir.join("books.redb")).expect("a fresh shelf opens");
    let library = Library { id: LIBRARY.into(), name: "Books".into(), kind: LibraryKind::Books };
    let mut changes = Changes::new(account().id);
    changes.items = summaries();
    changes.stamp = Some(Stamp::from_millis(ADDED));
    shelf.apply_changes(&library, &changes).expect("the fixture writes");
    for n in 0..BOOKS.len() {
        shelf.set_details(&ItemKey::new(account().id, format!("book-{n}")), &details(n)).expect("the fixture writes");
    }
    std::sync::Arc::new(shelf)
}

/// The app's files in `dir`, as a start would find them.
pub fn paths(dir: &Path) -> Paths {
    Paths { config_file: dir.join("settings.json"), shelves: dir.join("accounts"), art_cache: dir.join("art") }
}

/// An app signed in to the fixture shelf, showing its books. `books` is how
/// many of the fixture's books it has (all of them unless a test says
/// otherwise), so an empty library is the same app with none.
pub fn app(ctx: &egui::Context, dir: &Path, books: usize) -> RedeApp {
    let mut app = RedeApp::build(ctx, paths(dir)).expect("the app builds");
    app.login = None;
    let shelf = shelf(dir);
    let all = summaries();
    let listed: Vec<ItemSummary> = all.iter().take(books).cloned().collect();
    let signed_in = account();
    app.prefs.edit(|s| s.put_account(signed_in.clone()));
    app.session = Session { account: Some(signed_in), source: None, shelf: Some(shelf.clone()), connecting: false };
    app.handle(crate::bridge::LibraryMsg::Books(listed));
    for (n, book) in all.iter().enumerate().take(books) {
        app.handle(crate::bridge::LibraryMsg::Book { key: ItemKey::new(account().id, book.id.clone()), item: Box::new(details(n)) });
    }
    app
}

/// An app at its sign-in screen, with nothing saved.
pub fn signed_out(ctx: &egui::Context, dir: &Path) -> RedeApp {
    let mut app = RedeApp::build(ctx, paths(dir)).expect("the app builds");
    app.login = Some(LoginForm::default());
    app.session = Session::default();
    app
}

/// The `n`th fixture book's page.
pub fn book_route(n: usize) -> Route {
    Route::Book(format!("book-{n}"))
}

/// Draws a frame of the app, as the harness does: nothing is rendered, only
/// laid out.
pub fn frame(ctx: &egui::Context, app: &mut RedeApp) {
    let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(FULL[0], FULL[1]))), ..Default::default() };
    let mut out = ctx.run_ui(input, |ui| {
        if app.login.is_some() {
            app.login_ui(ui);
        } else {
            app.main_ui(ui);
        }
    });
    // No renderer here to upload textures to.
    out.textures_delta.clear();
}

/// The window the snapshots and the frames are drawn at.
pub const FULL: [f32; 2] = [1100.0, 720.0];
