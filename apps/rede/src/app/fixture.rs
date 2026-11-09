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
    let mut account = Account::new(SERVER, "sam");
    account.id = sources::AccountId::from_uuid(uuid::Uuid::from_u128(42));
    account.device_id = "fixture".into();
    account.libraries = vec![crate::settings::KnownLibrary { id: LIBRARY.into(), name: "Books".into() }];
    account
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
    let title = BOOKS[n].title;
    // The details carry what the listing does (narrators, series, length).
    let mut item = Item { summary: summaries().swap_remove(n), ..Item::default() };
    item.description = Some(if n == 1 {
        // Audiobookshelf keeps a description as HTML. One of the fixture's
        // books has the markup a real one does, so the page shows how it reads.
        html_description(title)
    } else {
        format!(
            "{title} is one of the fixture's books, here to show what a book's page looks like. \
             The description runs to a few lines of prose, which is all the page needs to lay out: \
             a paragraph that wraps in its column, a little longer than the metadata above it."
        )
    });
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

/// A book's description as the server stores it: paragraphs with bold and
/// italic runs, entities, a list, a quote and a link.
fn html_description(title: &str) -> String {
    format!(
        "<p><b>“{title} is a landmark.”</b> — <i>The Saturday Review</i><br />\
         <b>Winner</b> of the Hugo Award &amp; the Nebula Award, and the first of a series that \
         changed what science fiction could do.</p>\
         <p>On the desert planet Arrakis, young Paul Atreides&nbsp;&mdash; heir to a noble house \
         &mdash; is caught between two empires and a spice only his world makes.</p>\
         <h3>What the critics said</h3>\
         <ul><li>“A wonderful, lifelike tapestry.”</li><li>“Its power and vision are extraordinary.”</li></ul>\
         <blockquote>A beginning is the most important part of the work.</blockquote>\
         <p>The reading group guide is on <a href=\"https://example.com/guide\">the publisher's site</a>.</p>"
    )
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
    app.bridge.use_openid_store(crate::openid::TestStore::default());
    app.login = None;
    let shelf = shelf(dir);
    let all = summaries();
    let listed: Vec<ItemSummary> = all.iter().take(books).cloned().collect();
    let signed_in = account();
    // The settings file is shared by every app a test builds in `dir`; the
    // column starts hidden and the sidebar open whatever the last one left.
    app.prefs.edit(|s| {
        s.put_account(signed_in.clone());
        s.show_panel = false;
        s.sidebar_collapsed = false;
    });
    app.shown = PrefsView::read(&app.prefs);
    app.session = Session { account: Some(signed_in), source: None, shelf: Some(shelf.clone()), connecting: false };
    app.handle(crate::bridge::LibraryMsg::Books(listed));
    for (n, book) in all.iter().enumerate().take(books) {
        app.handle(crate::bridge::LibraryMsg::Book { key: ItemKey::new(account().id, book.id.clone()), item: Box::new(details(n)) });
    }
    // Square, portrait and landscape art exercise the fit; the remaining
    // books keep their title placeholders, including a cover still loading.
    for (n, size, color) in [
        (0, [128, 192], egui::Color32::from_rgb(123, 146, 155)),
        (1, [192, 192], egui::Color32::from_rgb(190, 157, 101)),
        (2, [192, 128], egui::Color32::from_rgb(126, 149, 123)),
    ] {
        let mut image = egui::ColorImage::filled(size, color);
        for y in 0..size[1] {
            for x in 0..size[0] {
                if x < 4 || y < 4 || x >= size[0] - 4 || y >= size[1] - 4 {
                    image.pixels[y * size[0] + x] = egui::Color32::from_rgb(225, 218, 198);
                }
            }
        }
        app.art.preload(&format!("book-{n}"), image);
    }
    for book in &all {
        app.art.preload_missing(&book.id);
    }
    app
}

/// An app at its sign-in screen, with nothing saved.
pub fn signed_out(ctx: &egui::Context, dir: &Path) -> RedeApp {
    let mut app = RedeApp::build(ctx, paths(dir)).expect("the app builds");
    app.bridge.use_openid_store(crate::openid::TestStore::default());
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

/// Places the real player policy in a stable mid-book scene without opening
/// an audio device. The same scene is used by accessibility and PNG tests.
pub fn playing(app: &mut RedeApp, paused: bool) {
    struct Preview;
    impl crate::player::Transport for Preview {
        fn send(&self, _: audio::Command) {}
        fn state(&self) -> audio::PlayState {
            audio::PlayState::Paused
        }
        fn position(&self) -> f64 {
            0.0
        }
        fn silence_saved(&self) -> f64 {
            0.0
        }
    }
    let item = details(0);
    let session = sources::PlaySession {
        id: "fixture".into(),
        item: "book-0".into(),
        start: sources::Position::from_secs(14_200.0),
        duration: Some(40_920.0),
        tracks: item
            .tracks
            .iter()
            .map(|t| sources::SessionTrack { index: t.index, start_offset: t.start_offset, duration: t.duration, ..Default::default() })
            .collect(),
    };
    app.player
        .load(crate::player::Opened { key: ItemKey::new(account().id, "book-0"), item, session, position: 14_200.0 }, 1.5, &Preview, std::time::Instant::now())
        .unwrap();
    app.player.paused = paused;
    app.prefs.edit(|p| p.playback.speed = 1.5);
}

/// Server discovery and pending-keyring replies for sign-in screenshots and
/// UI tests. These are wire fixtures, never real browser credentials.
pub fn signin(ctx: &egui::Context, dir: &Path, both: bool, waiting: bool) -> RedeApp {
    let methods: &[&str] = if both { &["local", "openid"] } else { &["openid"] };
    signin_with(ctx, dir, methods, waiting.then(pending_openid))
}

/// The sign-in screen after the server said which `methods` it offers, and
/// waiting for the browser when there's a `waiting` sign-in.
pub fn signin_with(ctx: &egui::Context, dir: &Path, methods: &[&str], waiting: Option<crate::openid::Pending>) -> RedeApp {
    let mut app = signed_out(ctx, dir);
    {
        let form = app.login.as_mut().unwrap();
        form.server = "https://books.example.com".into();
        form.checked_server = form.server.clone();
    }
    app.handle(crate::bridge::AccountMsg::Status {
        session: app.bridge.session(),
        status: abs::ServerStatus {
            auth_methods: methods.iter().map(|m| (*m).to_owned()).collect(),
            auth_form_data: abs::AuthFormData { auth_open_id_button_text: Some("Sign in with Book Club".into()), ..Default::default() },
            ..Default::default()
        },
        warning: None,
    });
    if let Some(pending) = waiting {
        app.handle(crate::bridge::AccountMsg::OpenIdRestored { session: 0, pending: Some(Box::new(pending)) });
    } else {
        app.openid.restored = true;
    }
    app
}

/// A browser sign-in on a server that hasn't allowed Rede's own redirect, so
/// it went through Audiobookshelf's.
pub fn pending_openid_fallback() -> crate::openid::Pending {
    let mut value = serde_json::to_value(pending_openid()).unwrap();
    value["flow"]["redirect"] = "audiobookshelf://oauth".into();
    serde_json::from_value(value).unwrap()
}

pub fn pending_openid() -> crate::openid::Pending {
    crate::openid::test_pending()
}

/// Draws `body` over the whole window, edge to edge as the app does, rather
/// than inside the margin the test harness leaves around its ui: what sits at
/// the window's edges (the sidebar's foot) is then where it would be.
pub fn whole_window(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui)) {
    let rect = ui.ctx().content_rect();
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.set_clip_rect(rect);
        body(ui);
    });
}
