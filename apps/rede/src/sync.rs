//! The library sync: list the account's libraries, page each book library's
//! listing into the shelf, and remember where the listing got to.
//!
//! Audiobookshelf has no "changed since" filter, so [`Source::items`] pages the
//! whole minified listing and keeps what is newer than the stamp it was given;
//! the shelf stores that stamp per library. It also has no deletion signal, so
//! only a listing fetched from the start can say what is gone: the sync runs one
//! of those [`FULL_LISTING_EVERY`], and compares what it listed with what the
//! shelf holds.
//!
//! Nothing here talks to the UI: [`run`] writes the shelf and reports through
//! the closure it's given, and the app turns the outcome into a message. That's
//! what lets the tests drive it against a fake [`Source`].

use crate::failure::Failure;
use shelf::{Shelf, SyncState};
use sources::{ItemSummary, Library, LibraryKind, Source};
use std::time::Duration;

/// How often a listing runs from the start. Only that can report removals, and
/// once a day is often enough for a library that changes when someone adds a
/// book.
pub const FULL_LISTING_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// The width of a stamp in milliseconds, for the arithmetic above.
const DAY_MS: i64 = FULL_LISTING_EVERY.as_millis() as i64;

/// What a sync wrote.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    /// The book libraries it listed, as the account remembers them, so the
    /// shelf can be read back before the next listing.
    pub libraries: Vec<crate::settings::KnownLibrary>,
    /// Items the listing had changed, written to the shelf.
    pub items: usize,
    /// Items a full listing no longer has, removed from the shelf.
    pub removed: usize,
}

/// How a sync ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Done(Report),
    /// The server is rate limiting (its sign-in and refresh limiter, which a
    /// token refresh counts against). The shelf is untouched: nothing is shown,
    /// and the sync is tried again after the wait.
    Busy {
        retry_after: Option<Duration>,
    },
    /// The refresh token is gone: only the user can fix it.
    SignedOut,
    Failed(Failure),
}

/// Syncs every book library of `account` into `shelf`, one at a time.
///
/// `now` decides whether this run lists from the start: one is due when the
/// last one was more than [`FULL_LISTING_EVERY`] ago, or when there never was
/// one. `progress` is told how many items have been written so far.
pub async fn run(source: &dyn Source, shelf: &Shelf, now: sources::Stamp, progress: impl Fn(usize)) -> Outcome {
    let libraries = match source.libraries().await {
        Ok(l) => l,
        Err(e) => return ended(e),
    };
    let books: Vec<Library> = libraries.into_iter().filter(|l| l.kind == LibraryKind::Books).collect();
    let mut report = Report { libraries: book_libraries(&books), ..Report::default() };
    for library in &books {
        let state = match shelf.sync_state(&library.id) {
            Ok(state) => state.unwrap_or_default(),
            Err(e) => return Outcome::Failed(Failure::Local(format!("Couldn't read the shelf: {e}"))),
        };
        // The last full listing's time is the cursor: the shelf keeps it
        // without knowing what it means (`SyncState`'s docs).
        let last_full = state.cursor.as_deref().and_then(|c| c.parse::<i64>().ok());
        let full = last_full.is_none_or(|last| (now.millis() - last).max(0) >= DAY_MS);
        let since = if full { None } else { state.newest };
        let mut changes = match source.items(&library.id, since).await {
            Ok(changes) => changes,
            Err(e) => return ended(e),
        };
        if full {
            // What the shelf holds and the listing doesn't is gone. A library
            // never listed before has nothing to compare with.
            match shelf.items(&library.id) {
                Ok(stored) => {
                    if changes.items.is_empty() && !stored.is_empty() {
                        tracing::warn!(library = %library.id, "empty full listing for a populated library; keeping its books and retrying next sync");
                        continue;
                    }
                    let listed: std::collections::HashSet<&str> = changes.items.iter().map(|i| i.id.as_str()).collect();
                    changes.removed = stored.iter().filter(|s| !listed.contains(s.id.as_str())).map(|s| s.id.clone()).collect();
                }
                Err(e) => return Outcome::Failed(Failure::Local(format!("Couldn't read the shelf: {e}"))),
            }
        }
        if let Err(e) = shelf.apply_changes(library, &changes) {
            return Outcome::Failed(Failure::Local(format!("Couldn't save the library: {e}")));
        }
        report.items += changes.items.len();
        report.removed += changes.removed.len();
        let next = SyncState { cursor: if full { Some(now.millis().to_string()) } else { state.cursor }, newest: state.newest.max(changes.stamp) };
        if let Err(e) = shelf.set_sync_state(&library.id, &next) {
            return Outcome::Failed(Failure::Local(format!("Couldn't save the sync's place: {e}")));
        }
        progress(report.items);
    }
    Outcome::Done(report)
}

/// A request that didn't answer well, as an outcome: rate limiting and a dead
/// session are not errors to show.
fn ended(e: sources::Error) -> Outcome {
    match e {
        sources::Error::Busy { retry_after } => Outcome::Busy { retry_after },
        sources::Error::SignedOut => Outcome::SignedOut,
        e => Outcome::Failed(Failure::from(e)),
    }
}

/// The book libraries the shelf was synced from, for reading it back. The
/// account keeps them (see [`crate::settings::KnownLibrary`]).
pub fn book_libraries(libraries: &[Library]) -> Vec<crate::settings::KnownLibrary> {
    libraries.iter().filter(|l| l.kind == LibraryKind::Books).map(|l| crate::settings::KnownLibrary { id: l.id.clone(), name: l.name.clone() }).collect()
}

/// The shelf's books across `libraries`, sorted by title so a caller can rely
/// on the order it's given.
pub fn stored_books(shelf: &Shelf, libraries: &[crate::settings::KnownLibrary]) -> Result<Vec<ItemSummary>, Failure> {
    let mut books = Vec::new();
    for library in libraries {
        match shelf.items(&library.id) {
            Ok(items) => books.extend(items),
            // A library the source has dropped has no rows, not an error.
            Err(e) => return Err(Failure::Local(format!("Couldn't read the shelf: {e}"))),
        }
    }
    books.sort_by_cached_key(|b| (b.title.to_lowercase(), b.id.clone()));
    Ok(books)
}

/// Serialized by the listening worker, including full-sync, focus and open
/// pulls. ABS has no bookmark revision stamp: a changed title or a deletion
/// compared with the queued baseline wins over the local edit. Matching the
/// desired title also makes a lost write response safe to retry.
pub async fn bookmarks(source: &dyn Source, shelf: &Shelf) -> Result<shelf::BookmarkSnapshot, Failure> {
    let local = |e| Failure::Local(format!("Couldn't save bookmarks: {e}"));
    if !source.capabilities().bookmarks {
        return shelf.bookmark_snapshot().map_err(local);
    }
    let mut remote = source.bookmarks().await.map_err(Failure::from)?;
    let mut wrote = false;
    for pending in shelf.pending_bookmarks().map_err(local)? {
        let desired = &pending.bookmark;
        let found = remote.iter().position(|b| b.item == desired.item && b.position == desired.position);
        let current = found.map(|i| &remote[i]);
        let same = |a: Option<&sources::Bookmark>, b: Option<&sources::Bookmark>| a.map(|b| &b.title) == b.map(|b| &b.title);
        let already = match pending.change {
            shelf::BookmarkChange::Delete => current.is_none(),
            _ => current.is_some_and(|b| b.title == desired.title),
        };
        if !already && same(current, pending.base.as_ref()) {
            wrote = true;
            let result = match pending.change {
                shelf::BookmarkChange::Create => source.create_bookmark(&desired.item.item, desired.position, &desired.title).await.map(Some),
                shelf::BookmarkChange::Rename => source.rename_bookmark(&desired.item.item, desired.position, &desired.title).await.map(Some),
                shelf::BookmarkChange::Delete => source.delete_bookmark(&desired.item.item, desired.position).await.map(|()| None),
            };
            match result {
                Ok(bookmark) => {
                    if let Some(i) = found {
                        remote.remove(i);
                    }
                    if let Some(bookmark) = bookmark {
                        remote.push(bookmark);
                    }
                }
                Err(e) if e.status() == Some(404) => {
                    remote = source.bookmarks().await.map_err(Failure::from)?;
                }
                Err(e) => return Err(Failure::from(e)),
            }
        }
        shelf.acknowledge_bookmark(pending.id).map_err(local)?;
    }
    // Fetch again after writing: a phone edit concurrent with the replay is
    // authoritative too. A pull that sent nothing already holds the server's
    // list, and pulls happen on every focus, so it isn't asked twice.
    let remote = if wrote { source.bookmarks().await.map_err(Failure::from)? } else { remote };
    shelf.replace_bookmarks(&remote).map_err(local)?;
    shelf.bookmark_snapshot().map_err(local)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use sources::{
        AccountId, Changes, DeviceInfo, Error, Item, LibraryId, PlaySession, Position, Progress, ProgressUpdate, Result as SourceResult, Stamp, TrackFile,
    };
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// An account that answers with what the test put in it, and records what
    /// it was asked.
    struct Fake {
        libraries: Vec<Library>,
        /// The listing each library answers with.
        listings: HashMap<String, Vec<ItemSummary>>,
        /// What it was asked for: library id and the `since` stamp.
        asked: Mutex<Vec<(String, Option<Stamp>)>>,
        /// Fails every listing with this instead of answering.
        fails: Option<Error>,
        /// The stamp its listings carry.
        stamp: Option<Stamp>,
    }

    impl Fake {
        fn new(items: &[(&str, &str)]) -> Self {
            let summaries: Vec<ItemSummary> = items.iter().map(|(id, title)| summary(id, title)).collect();
            Self {
                libraries: vec![
                    Library { id: "books".into(), name: "Books".into(), kind: LibraryKind::Books },
                    Library { id: "podcasts".into(), name: "Podcasts".into(), kind: LibraryKind::Podcasts },
                ],
                listings: HashMap::from([("books".to_owned(), summaries)]),
                asked: Mutex::new(Vec::new()),
                fails: None,
                stamp: Some(Stamp::from_millis(2_000)),
            }
        }

        fn asked(&self) -> Vec<(String, Option<Stamp>)> {
            self.asked.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Source for Fake {
        fn capabilities(&self) -> sources::Capabilities {
            sources::Capabilities::default()
        }

        async fn libraries(&self) -> SourceResult<Vec<Library>> {
            if let Some(e) = &self.fails {
                return Err(e.clone());
            }
            Ok(self.libraries.clone())
        }

        async fn items(&self, library: &LibraryId, since: Option<Stamp>) -> SourceResult<Changes<ItemSummary>> {
            self.asked.lock().unwrap().push((library.clone(), since));
            if let Some(e) = &self.fails {
                return Err(e.clone());
            }
            let mut changes = Changes::new(account());
            changes.items = self.listings.get(library).cloned().unwrap_or_default();
            changes.stamp = self.stamp;
            Ok(changes)
        }

        async fn item(&self, _: &str) -> SourceResult<Item> {
            Err(Error::NotSupported("not in this test".into()))
        }
        async fn cover(&self, _: &str, _: Option<u32>) -> SourceResult<bytes::Bytes> {
            Err(Error::NotSupported("not in this test".into()))
        }
        async fn open(&self, _: &str, _: &DeviceInfo) -> SourceResult<PlaySession> {
            Err(Error::NotSupported("not in this test".into()))
        }
        async fn report(&self, _: &PlaySession, _: Position, _: Duration) -> SourceResult<()> {
            Ok(())
        }
        async fn close(&self, _: &PlaySession, _: Position) -> SourceResult<()> {
            Ok(())
        }
        async fn progress(&self) -> SourceResult<Vec<Progress>> {
            Ok(Vec::new())
        }
        async fn set_progress(&self, _: &[ProgressUpdate]) -> SourceResult<()> {
            Ok(())
        }
    }

    fn account() -> AccountId {
        "2f1a5c3e-8a4b-4f6d-9e0a-1c2b3d4e5f60".parse().unwrap()
    }

    fn summary(id: &str, title: &str) -> ItemSummary {
        ItemSummary {
            id: id.into(),
            library: "books".into(),
            title: title.into(),
            authors: vec!["Ursula K. Le Guin".into()],
            cover: Some(id.into()),
            added: Some(Stamp::from_millis(1_000)),
            updated: Some(Stamp::from_millis(2_000)),
            duration: Some(3600.0),
            ..ItemSummary::default()
        }
    }

    fn shelf(items: &[(&str, &str)]) -> (tempfile::TempDir, Shelf) {
        let dir = tempfile::tempdir().unwrap();
        let shelf = Shelf::open(dir.path().join("books.redb")).unwrap();
        let library = Library { id: "books".into(), name: "Books".into(), kind: LibraryKind::Books };
        let mut changes = Changes::new(account());
        changes.items = items.iter().map(|(id, title)| summary(id, title)).collect();
        shelf.apply_changes(&library, &changes).unwrap();
        (dir, shelf)
    }

    fn titles(shelf: &Shelf) -> Vec<String> {
        let mut out: Vec<String> = shelf.items("books").unwrap().into_iter().map(|b| b.title).collect();
        out.sort();
        out
    }

    fn now() -> Stamp {
        Stamp::from_millis(100 * DAY_MS)
    }

    /// A finished sync's report for the fake's one book library.
    fn report(items: usize, removed: usize) -> Report {
        Report { libraries: vec![crate::settings::KnownLibrary { id: "books".into(), name: "Books".into() }], items, removed }
    }

    #[test]
    fn an_incremental_listing_asks_since_the_last_stamp_and_writes_what_changed() {
        let (_dir, shelf) = shelf(&[("old", "Old")]);
        shelf.set_sync_state("books", &SyncState { cursor: Some(now().millis().to_string()), newest: Some(Stamp::from_millis(1_500)) }).unwrap();
        let fake = Fake::new(&[("new", "New")]);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let outcome = rt.block_on(run(&fake, &shelf, now(), |_| {}));
        assert_eq!(outcome, Outcome::Done(report(1, 0)));
        assert_eq!(fake.asked(), [("books".to_string(), Some(Stamp::from_millis(1_500)))], "only book libraries are listed, from the last stamp");
        assert_eq!(titles(&shelf), ["New", "Old"], "an incremental listing removes nothing: it doesn't know what's gone");
        let state = shelf.sync_state("books").unwrap().unwrap();
        assert_eq!(state.newest, Some(Stamp::from_millis(2_000)), "the next sync starts from what this one saw");
        assert_eq!(state.cursor, Some(now().millis().to_string()), "and a full listing isn't due yet");
    }

    #[test]
    fn a_full_listing_removes_what_it_no_longer_lists() {
        let (_dir, shelf) = shelf(&[("kept", "Kept"), ("gone", "Gone"), ("stale", "Stale")]);
        // No full listing on record: one is due.
        let fake = Fake::new(&[("kept", "Kept, revised")]);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let outcome = rt.block_on(run(&fake, &shelf, now(), |_| {}));
        assert_eq!(outcome, Outcome::Done(report(1, 2)));
        assert_eq!(fake.asked(), [("books".to_string(), None)], "from the start, so it can see what's missing");
        assert_eq!(titles(&shelf), ["Kept, revised"]);
    }

    #[test]
    fn an_empty_full_listing_keeps_the_books_and_retries_from_the_same_cursor() {
        let (_dir, shelf) = shelf(&[("kept", "Kept")]);
        let state = SyncState { cursor: Some("5".into()), newest: Some(Stamp::from_millis(1_500)) };
        shelf.set_sync_state("books", &state).unwrap();
        let fake = Fake::new(&[]);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        for _ in 0..2 {
            assert_eq!(rt.block_on(run(&fake, &shelf, now(), |_| {})), Outcome::Done(report(0, 0)));
            assert_eq!(titles(&shelf), ["Kept"]);
            assert_eq!(shelf.sync_state("books").unwrap(), Some(state.clone()));
        }
        assert_eq!(fake.asked(), [("books".to_string(), None), ("books".to_string(), None)]);
    }

    #[test]
    fn an_empty_full_listing_for_a_new_library_records_its_cursor() {
        let (_dir, shelf) = shelf(&[]);
        let fake = Fake::new(&[]);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        assert_eq!(rt.block_on(run(&fake, &shelf, now(), |_| {})), Outcome::Done(report(0, 0)));
        assert_eq!(shelf.sync_state("books").unwrap().unwrap().cursor, Some(now().millis().to_string()));
    }

    #[test]
    fn a_full_listing_is_due_once_a_day() {
        let (_dir, shelf) = shelf(&[("a", "A")]);
        let recent = Stamp::from_millis(now().millis() - 60_000);
        shelf.set_sync_state("books", &SyncState { cursor: Some(recent.millis().to_string()), newest: Some(Stamp::from_millis(1_500)) }).unwrap();
        let fake = Fake::new(&[("a", "A")]);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        assert_eq!(rt.block_on(run(&fake, &shelf, now(), |_| {})), Outcome::Done(report(1, 0)));
        assert_eq!(fake.asked(), [("books".to_string(), Some(Stamp::from_millis(1_500)))], "yesterday's full listing still counts");
        // A day later it runs from the start again.
        let later = Stamp::from_millis(now().millis() + DAY_MS);
        rt.block_on(run(&fake, &shelf, later, |_| {}));
        assert_eq!(fake.asked().last(), Some(&("books".to_string(), None)));
    }

    #[test]
    fn a_busy_server_leaves_the_shelf_and_its_stamp_as_they_were() {
        let (_dir, shelf) = shelf(&[("kept", "Kept")]);
        let state = SyncState { cursor: Some("5".into()), newest: Some(Stamp::from_millis(1_500)) };
        shelf.set_sync_state("books", &state).unwrap();
        let fake = Fake { fails: Some(Error::Busy { retry_after: Some(Duration::from_secs(30)) }), ..Fake::new(&[]) };
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let outcome = rt.block_on(run(&fake, &shelf, now(), |_| {}));
        assert_eq!(outcome, Outcome::Busy { retry_after: Some(Duration::from_secs(30)) });
        assert_eq!(titles(&shelf), ["Kept"], "the books already there stay");
        assert_eq!(shelf.sync_state("books").unwrap(), Some(state), "and the next sync asks from the same place");
    }

    #[test]
    fn a_dead_session_is_signed_out_and_a_refusal_is_shown() {
        let (_dir, shelf) = shelf(&[]);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let gone = Fake { fails: Some(Error::SignedOut), ..Fake::new(&[]) };
        assert_eq!(rt.block_on(run(&gone, &shelf, now(), |_| {})), Outcome::SignedOut);
        let refused = Fake { fails: Some(Error::Server { status: 500, message: Some("no room".into()) }), ..Fake::new(&[]) };
        let outcome = rt.block_on(run(&refused, &shelf, now(), |_| {}));
        assert!(matches!(outcome, Outcome::Failed(Failure::Server(m)) if m.contains("no room")));
    }

    #[test]
    fn the_shelf_reads_back_what_the_sync_wrote() {
        let (dir, shelf) = shelf(&[("b", "Beta"), ("a", "Alpha")]);
        let books = stored_books(&shelf, &[crate::settings::KnownLibrary { id: "books".into(), name: "Books".into() }]).unwrap();
        assert_eq!(books.iter().map(|b| b.title.as_str()).collect::<Vec<_>>(), ["Alpha", "Beta"], "sorted by title");
        // A library that isn't there is empty, not an error.
        let none = stored_books(&shelf, &[crate::settings::KnownLibrary { id: "gone".into(), name: "Gone".into() }]).unwrap();
        assert!(none.is_empty());
        drop((dir, shelf));
    }

    /// Against a real Audiobookshelf server, when one is configured:
    ///
    /// ```sh
    /// ABS_TEST_URL=... ABS_TEST_USER=... ABS_TEST_PASSWORD=... \
    ///     cargo test -p rede -- --ignored a_real_server
    /// ```
    ///
    /// One sign-in, then one sync into a fresh shelf in a temp folder: enough to
    /// prove the mapping, the paging and the shelf writes meet a server, and
    /// nothing that costs more of the sign-in limiter than it must. Nothing here
    /// prints the address, the account or a token.
    #[test]
    #[ignore = "needs a real server: set ABS_TEST_URL, ABS_TEST_USER and ABS_TEST_PASSWORD"]
    fn a_real_server_signs_in_once_and_lists_its_books_into_a_shelf() {
        let get = |key: &str| std::env::var(key).ok().filter(|v| !v.is_empty());
        let (Some(url), Some(user), Some(password)) = (get("ABS_TEST_URL"), get("ABS_TEST_USER"), get("ABS_TEST_PASSWORD")) else {
            eprintln!("skipping: ABS_TEST_URL, ABS_TEST_USER and ABS_TEST_PASSWORD aren't all set");
            return;
        };
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async move {
            let client = abs::Client::new(&url, abs::DeviceInfo::new("rede-live-test", env!("CARGO_PKG_VERSION"))).expect("the test server's address");
            let status = client.status().await.expect("the server answers its status");
            let version = status.version().expect("the server says its version");
            println!("server {version}");
            assert!(version >= abs::Version::TOKENS, "the test server is new enough for refresh tokens");
            client.login(&user, &password).await.expect("one sign-in");
            let source = sources::AbsSource::new(account(), client, ["audio/mpeg", "audio/mp4", "audio/flac"].map(String::from).to_vec());

            let dir = tempfile::tempdir().unwrap();
            let shelf = Shelf::open(dir.path().join("live.redb")).unwrap();
            let outcome = run(&source, &shelf, Stamp::from(std::time::SystemTime::now()), |n| println!("  {n} items written so far")).await;
            let Outcome::Done(report) = outcome else { panic!("the sync ended as {outcome:?}") };
            println!("{} book libraries, {} items, {} removed", report.libraries.len(), report.items, report.removed);
            assert!(!report.libraries.is_empty(), "the account has a book library");
            assert!(report.items > 0, "the library listed something");
            let books = stored_books(&shelf, &report.libraries).expect("the shelf reads back");
            println!("{} books in the shelf", books.len());
            assert!(!books.is_empty(), "and the shelf holds them");
        });
    }

    #[test]
    fn track_files_and_chapters_survive_a_details_fetch() {
        // The book page reads these back from the shelf after storing them.
        let (dir, shelf) = shelf(&[("b", "Beta")]);
        let mut item = Item { summary: summary("b", "Beta"), ..Item::default() };
        item.description = Some("A book.".into());
        item.tracks = vec![TrackFile { index: 1, start_offset: 0.0, duration: Some(60.0), suffix: Some("m4b".into()), ..TrackFile::default() }];
        item.chapters = vec![sources::Chapter { start: 0.0, end: 60.0, title: "One".into() }];
        shelf.set_details(&sources::ItemKey::new(account(), "b"), &item).unwrap();
        let back = shelf.item(&sources::ItemKey::new(account(), "b")).unwrap().unwrap();
        assert_eq!(back.tracks, item.tracks);
        assert_eq!(back.chapters, item.chapters);
        assert_eq!(back.description.as_deref(), Some("A book."));
        drop(dir);
    }
}
