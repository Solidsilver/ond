//! Background track sync: every album's tracks into the local db, so search,
//! the Songs view and credits cover the whole library, not just albums opened.
//!
//! First run pages through `search3` with an empty query, which Navidrome
//! answers with the whole library, 500 songs a request (~40 requests for 20k
//! tracks instead of one `getAlbum` per album). Albums whose stored track count
//! then matches the album list are stamped as current. After that, and for any
//! album the bulk pass missed, sync is per album: an album is fetched again only
//! when its [`sync_stamp`](library::Album::sync_stamp) changed.
//!
//! Progress is saved as it goes (the bulk offset in the db's meta table, stamps
//! per album), so a sync cut short by quitting picks up where it stopped.

use library::{Album, Library};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use subsonic::{AlbumWithSongs, Client, Song};
use tokio::task::JoinSet;

pub const BULK_PAGE: u32 = 500;
/// `getAlbum` requests in flight at once.
pub const CONCURRENCY: usize = 4;
/// Albums stored per write transaction during per-album sync.
const WRITE_BATCH: usize = 16;
/// More stale albums than this, on a library never bulk-synced: bulk is faster.
pub const BULK_THRESHOLD: usize = 200;
/// Consecutive failed requests before giving up (server down, auth lost).
const MAX_FAILURES: usize = 8;
/// Tries per bulk page before giving up; the saved offset resumes it next time.
const BULK_RETRIES: usize = 3;

const BULK_DONE: &str = "tracks_bulk_done";
const BULK_OFFSET: &str = "tracks_bulk_offset";

/// Where the tracks come from; [`Client`] in the app, a fake in tests.
pub trait Fetch: Clone + Send + Sync + 'static {
    fn album(&self, id: String) -> impl Future<Output = Result<AlbumWithSongs, String>> + Send;
    /// One page of every song in the library.
    fn all_songs(&self, query: &'static str, count: u32, offset: u32) -> impl Future<Output = Result<Vec<Song>, String>> + Send;
}

impl Fetch for Client {
    async fn album(&self, id: String) -> Result<AlbumWithSongs, String> {
        Client::album(self, &id).await.map_err(|e| e.to_string())
    }

    async fn all_songs(&self, query: &'static str, count: u32, offset: u32) -> Result<Vec<Song>, String> {
        self.search_songs(query, count, offset).await.map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Tracks,
    Albums,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub unit: Unit,
}

impl Progress {
    pub fn label(&self) -> String {
        match self.unit {
            Unit::Tracks => format!("Syncing tracks {}/{}", self.done, self.total),
            Unit::Albums => format!("Syncing tracks · album {}/{}", self.done, self.total),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// `fetched` albums were (re)loaded; `bulk` when the search3 pass ran.
    Done { fetched: usize, bulk: bool },
    Cancelled,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Page through every song from this offset, then fix up per album.
    Bulk { from: u32 },
    PerAlbum(Vec<String>),
}

/// Albums whose stored tracks are missing or older than what the album list says.
pub fn stale(albums: &[Album], stamps: &HashMap<String, u64>) -> Vec<String> {
    albums.iter().filter(|a| stamps.get(&a.id) != Some(&a.sync_stamp())).map(|a| a.id.clone()).collect()
}

pub fn plan(albums: &[Album], stamps: &HashMap<String, u64>, bulk_done: bool, bulk_offset: Option<u32>) -> Plan {
    let stale = stale(albums, stamps);
    match bulk_offset {
        // A bulk pass was interrupted: carry on with it.
        Some(from) if !bulk_done => Plan::Bulk { from },
        _ if !bulk_done && stale.len() > BULK_THRESHOLD => Plan::Bulk { from: 0 },
        _ => Plan::PerAlbum(stale),
    }
}

/// After a bulk pass: stamps for albums whose stored track count matches the
/// album list. The rest (changed mid-pass, or songs the pass missed) stay stale
/// and get fetched one by one.
pub fn bulk_stamps(albums: &[Album], counts: &HashMap<String, u32>) -> Vec<(String, u64)> {
    albums
        .iter()
        .filter(|a| counts.get(&a.id).copied().unwrap_or(0) == a.song_count)
        .map(|a| (a.id.clone(), a.sync_stamp()))
        .collect()
}

async fn blocking<T: Send + 'static>(lib: &Arc<Library>, f: impl FnOnce(&Library) -> library::Result<T> + Send + 'static) -> Result<T, String> {
    let lib = lib.clone();
    match tokio::task::spawn_blocking(move || f(&lib)).await {
        Ok(r) => r.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// Runs a full sync pass. `progress` is called after every page or album.
pub async fn run<F: Fetch>(lib: Arc<Library>, fetch: F, cancel: Arc<AtomicBool>, progress: impl Fn(Progress)) -> Outcome {
    match run_inner(&lib, fetch, &cancel, &progress).await {
        Ok(o) => o,
        Err(e) => Outcome::Failed(e),
    }
}

async fn run_inner<F: Fetch>(lib: &Arc<Library>, fetch: F, cancel: &AtomicBool, progress: &impl Fn(Progress)) -> Result<Outcome, String> {
    let (albums, stamps, done, offset) =
        blocking(lib, |l| Ok((l.albums()?, l.album_stamps()?, l.meta(BULK_DONE)?.is_some(), l.meta(BULK_OFFSET)?))).await?;
    let mut bulk = false;
    if let Plan::Bulk { from } = plan(&albums, &stamps, done, offset) {
        let total = albums.iter().map(|a| a.song_count as usize).sum();
        match bulk_pass(lib, &fetch, cancel, progress, from, total).await? {
            BulkEnd::Cancelled => return Ok(Outcome::Cancelled),
            BulkEnd::Unsupported => {}
            BulkEnd::Done => {
                let albums2 = albums.clone();
                blocking(lib, move |l| {
                    l.set_album_stamps(bulk_stamps(&albums2, &l.track_counts()?))?;
                    l.set_meta(BULK_DONE, Some(1))?;
                    l.set_meta(BULK_OFFSET, None)
                })
                .await?;
                bulk = true;
            }
        }
    }
    // Whatever is still stale: everything when bulk isn't supported, else the
    // few the bulk pass couldn't vouch for.
    let stamps = blocking(lib, |l| l.album_stamps()).await?;
    let todo = stale(&albums, &stamps);
    match per_album(lib, &fetch, cancel, progress, todo).await? {
        Some(fetched) => Ok(Outcome::Done { fetched, bulk }),
        None => Ok(Outcome::Cancelled),
    }
}

enum BulkEnd {
    Done,
    Cancelled,
    /// The server returned nothing for an empty query; use per-album sync.
    Unsupported,
}

async fn bulk_pass<F: Fetch>(
    lib: &Arc<Library>,
    fetch: &F,
    cancel: &AtomicBool,
    progress: &impl Fn(Progress),
    from: u32,
    total: usize,
) -> Result<BulkEnd, String> {
    // Navidrome takes an empty query; some versions want a literal `""`.
    let mut query = "";
    let mut offset = from;
    let mut failures = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(BulkEnd::Cancelled);
        }
        let page = match fetch.all_songs(query, BULK_PAGE, offset).await {
            Ok(p) => {
                failures = 0;
                p
            }
            Err(e) => {
                failures += 1;
                if failures >= BULK_RETRIES {
                    return Err(format!("track sync: {e}"));
                }
                continue;
            }
        };
        if page.is_empty() && offset == 0 && total > 0 {
            if query.is_empty() {
                query = "\"\"";
                continue;
            }
            return Ok(BulkEnd::Unsupported);
        }
        let n = page.len() as u32;
        let next = offset + n;
        blocking(lib, move |l| {
            l.store_songs(page)?;
            l.set_meta(BULK_OFFSET, Some(next))
        })
        .await?;
        offset = next;
        progress(Progress { done: offset as usize, total: total.max(offset as usize), unit: Unit::Tracks });
        if n < BULK_PAGE {
            return Ok(BulkEnd::Done);
        }
    }
}

/// Fetches `ids` with [`CONCURRENCY`] requests in flight, storing them in
/// batches. `None` when cancelled (what was fetched so far is kept).
async fn per_album<F: Fetch>(lib: &Arc<Library>, fetch: &F, cancel: &AtomicBool, progress: &impl Fn(Progress), ids: Vec<String>) -> Result<Option<usize>, String> {
    let total = ids.len();
    let mut queue = ids.into_iter();
    let mut running = JoinSet::new();
    let mut batch = Vec::new();
    let mut done = 0;
    let mut stored = 0;
    let mut failures = 0;
    let mut last_error = String::new();
    loop {
        while running.len() < CONCURRENCY && !cancel.load(Ordering::Relaxed) {
            let Some(id) = queue.next() else { break };
            let f = fetch.clone();
            running.spawn(async move { f.album(id).await });
        }
        let Some(joined) = running.join_next().await else { break };
        done += 1;
        match joined.map_err(|e| e.to_string()).and_then(|r| r) {
            Ok(album) => {
                failures = 0;
                batch.push(album);
            }
            Err(e) => {
                failures += 1;
                last_error = e;
            }
        }
        if batch.len() >= WRITE_BATCH {
            stored += batch.len();
            let b = std::mem::take(&mut batch);
            blocking(lib, move |l| l.upsert_albums_tracks(b)).await?;
        }
        progress(Progress { done, total, unit: Unit::Albums });
        if failures >= MAX_FAILURES {
            running.abort_all();
            return Err(format!("track sync: {last_error}"));
        }
    }
    stored += batch.len();
    if !batch.is_empty() {
        blocking(lib, move |l| l.upsert_albums_tracks(batch)).await?;
    }
    Ok(if cancel.load(Ordering::Relaxed) && stored < total { None } else { Some(stored) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    fn album(id: &str, songs: u32) -> subsonic::Album {
        subsonic::Album {
            id: id.into(),
            name: format!("Album {id}"),
            artist: Some("Artist".into()),
            artist_id: None,
            cover_art: Some(format!("al-{id}_1")),
            song_count: songs,
            duration: 100 * songs,
            year: Some(2000),
            genre: None,
            created: None,
            starred: None,
            play_count: None,
            user_rating: None,
            sort_name: None,
            record_labels: Vec::new(),
            genres: Vec::new(),
            artists: Vec::new(),
        }
    }

    fn song(id: String, album_id: &str, n: u32) -> Song {
        Song {
            id,
            title: format!("t{n}"),
            album: None,
            album_id: Some(album_id.into()),
            artist: None,
            artist_id: None,
            track: Some(n),
            disc_number: Some(1),
            year: None,
            genre: None,
            cover_art: None,
            size: None,
            content_type: None,
            suffix: None,
            duration: Some(100),
            bit_rate: None,
            bit_depth: None,
            sampling_rate: None,
            channel_count: None,
            replay_gain: None,
            display_composer: None,
            contributors: Vec::new(),
            artists: Vec::new(),
            starred: None,
            user_rating: None,
            play_count: None,
            played: None,
        }
    }

    /// A server with `albums`, each with `song_count` songs. Records calls.
    #[derive(Clone)]
    struct Fake {
        albums: Arc<Mutex<Vec<subsonic::Album>>>,
        bulk: bool,
        /// search3 skips songs of this album (as if added mid-pass).
        bulk_skips: Option<String>,
        album_calls: Arc<Mutex<Vec<String>>>,
        page_calls: Arc<AtomicUsize>,
        /// Cancel after this many pages.
        cancel_after_pages: Option<(usize, Arc<AtomicBool>)>,
    }

    impl Fake {
        fn new(albums: Vec<subsonic::Album>, bulk: bool) -> Self {
            Self {
                albums: Arc::new(Mutex::new(albums)),
                bulk,
                bulk_skips: None,
                album_calls: Arc::default(),
                page_calls: Arc::default(),
                cancel_after_pages: None,
            }
        }

        fn songs_of(a: &subsonic::Album) -> Vec<Song> {
            (1..=a.song_count).map(|n| song(format!("{}-{n}", a.id), &a.id, n)).collect()
        }
    }

    impl Fetch for Fake {
        async fn album(&self, id: String) -> Result<AlbumWithSongs, String> {
            self.album_calls.lock().unwrap().push(id.clone());
            let a = self.albums.lock().unwrap().iter().find(|a| a.id == id).cloned().ok_or("not found")?;
            Ok(AlbumWithSongs { song: Fake::songs_of(&a), album: a })
        }

        async fn all_songs(&self, _query: &'static str, count: u32, offset: u32) -> Result<Vec<Song>, String> {
            let pages = self.page_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if let Some((after, cancel)) = &self.cancel_after_pages
                && pages > *after
            {
                cancel.store(true, Ordering::SeqCst);
            }
            if !self.bulk {
                return Ok(Vec::new());
            }
            let all: Vec<Song> = self
                .albums
                .lock()
                .unwrap()
                .iter()
                .filter(|a| Some(&a.id) != self.bulk_skips.as_ref())
                .flat_map(Fake::songs_of)
                .collect();
            Ok(all.into_iter().skip(offset as usize).take(count as usize).collect())
        }
    }

    fn library(albums: &[subsonic::Album]) -> Arc<Library> {
        let lib = Library::in_memory().unwrap();
        lib.upsert_albums(albums.to_vec()).unwrap();
        Arc::new(lib)
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
    }

    fn many(n: usize, songs: u32) -> Vec<subsonic::Album> {
        (0..n).map(|i| album(&format!("a{i:03}"), songs)).collect()
    }

    #[test]
    fn plan_picks_bulk_for_a_fresh_big_library_and_resumes_it() {
        let albums: Vec<Album> = many(300, 10).into_iter().map(Album::from).collect();
        let none = HashMap::new();
        assert_eq!(plan(&albums, &none, false, None), Plan::Bulk { from: 0 });
        assert_eq!(plan(&albums, &none, false, Some(1500)), Plan::Bulk { from: 1500 });
        // Once bulk has run, never again: per album, only what's stale.
        let mut stamps: HashMap<String, u64> = albums.iter().map(|a| (a.id.clone(), a.sync_stamp())).collect();
        assert_eq!(plan(&albums, &stamps, true, None), Plan::PerAlbum(Vec::new()));
        stamps.insert("a007".into(), 1);
        stamps.remove("a008");
        assert_eq!(plan(&albums, &stamps, true, None), Plan::PerAlbum(vec!["a007".into(), "a008".into()]));
        // Small libraries skip bulk.
        assert!(matches!(plan(&albums[..20], &none, false, None), Plan::PerAlbum(v) if v.len() == 20));
    }

    #[test]
    fn bulk_stamps_only_complete_albums() {
        let albums: Vec<Album> = vec![album("a", 2), album("b", 3), album("c", 0)].into_iter().map(Album::from).collect();
        let counts: HashMap<String, u32> = [("a".into(), 2), ("b".into(), 2)].into_iter().collect();
        let ids: Vec<String> = bulk_stamps(&albums, &counts).into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, ["a", "c"]);
    }

    #[test]
    fn first_run_bulk_then_fix_up_then_incremental() {
        let albums = many(250, 3);
        let lib = library(&albums);
        let mut fake = Fake::new(albums.clone(), true);
        fake.bulk_skips = Some("a100".into());
        let seen = Mutex::new(Vec::new());
        let out = rt().block_on(run(lib.clone(), fake.clone(), Arc::default(), |p| seen.lock().unwrap().push(p)));
        // 747 songs: two pages; the skipped album is fetched on its own.
        assert_eq!(out, Outcome::Done { fetched: 1, bulk: true });
        assert_eq!(fake.page_calls.load(Ordering::SeqCst), 2);
        assert_eq!(*fake.album_calls.lock().unwrap(), ["a100"]);
        assert_eq!(lib.track_count().unwrap(), 750);
        assert_eq!(seen.lock().unwrap()[0], Progress { done: 500, total: 750, unit: Unit::Tracks });
        assert!(lib.check_indexes().unwrap().is_empty());

        // Nothing changed: nothing fetched.
        fake.album_calls.lock().unwrap().clear();
        let out = rt().block_on(run(lib.clone(), fake.clone(), Arc::default(), |_| {}));
        assert_eq!(out, Outcome::Done { fetched: 0, bulk: false });
        assert_eq!(fake.page_calls.load(Ordering::SeqCst), 2);

        // An album gains a track on the server (and its list entry says so).
        let mut changed = albums[5].clone();
        changed.song_count = 4;
        fake.albums.lock().unwrap()[5] = changed.clone();
        lib.upsert_albums([changed]).unwrap();
        let out = rt().block_on(run(lib.clone(), fake.clone(), Arc::default(), |_| {}));
        assert_eq!(out, Outcome::Done { fetched: 1, bulk: false });
        assert_eq!(*fake.album_calls.lock().unwrap(), ["a005"]);
        assert_eq!(lib.album_tracks("a005").unwrap().len(), 4);
    }

    #[test]
    fn interrupted_bulk_resumes_from_its_offset() {
        let albums = many(400, 3); // 1200 songs: three pages
        let lib = library(&albums);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut fake = Fake::new(albums.clone(), true);
        fake.cancel_after_pages = Some((1, cancel.clone()));
        let out = rt().block_on(run(lib.clone(), fake.clone(), cancel, |_| {}));
        assert_eq!(out, Outcome::Cancelled);
        assert_eq!(lib.meta(BULK_OFFSET).unwrap(), Some(1000));
        assert_eq!(lib.track_count().unwrap(), 1000);

        // Next launch: picks up at 1000, one more page, done.
        let fake = Fake::new(albums, true);
        let out = rt().block_on(run(lib.clone(), fake.clone(), Arc::default(), |_| {}));
        assert_eq!(out, Outcome::Done { fetched: 0, bulk: true });
        assert_eq!(fake.page_calls.load(Ordering::SeqCst), 1);
        assert_eq!(lib.track_count().unwrap(), 1200);
        assert_eq!(lib.meta(BULK_OFFSET).unwrap(), None);
        assert_eq!(lib.album_stamps().unwrap().len(), 400);
    }

    #[test]
    fn falls_back_to_per_album_when_search_returns_nothing() {
        let albums = many(220, 2);
        let lib = library(&albums);
        let fake = Fake::new(albums, false);
        let out = rt().block_on(run(lib.clone(), fake.clone(), Arc::default(), |_| {}));
        assert_eq!(out, Outcome::Done { fetched: 220, bulk: false });
        // Tried "" and then `""` before giving up on bulk.
        assert_eq!(fake.page_calls.load(Ordering::SeqCst), 2);
        assert_eq!(fake.album_calls.lock().unwrap().len(), 220);
        assert_eq!(lib.track_count().unwrap(), 440);
        assert_eq!(lib.album_stamps().unwrap().len(), 220);
    }

    #[test]
    fn cancelled_per_album_keeps_what_it_fetched() {
        let albums = many(40, 1);
        let lib = library(&albums);
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let fake = Fake::new(albums, false);
        let out = rt().block_on(run(lib.clone(), fake, cancel, move |p| {
            if p.done == 20 {
                c.store(true, Ordering::SeqCst);
            }
        }));
        assert_eq!(out, Outcome::Cancelled);
        let stamped = lib.album_stamps().unwrap().len();
        assert!((20..40).contains(&stamped), "kept {stamped}");
        assert_eq!(lib.track_count().unwrap() as usize, stamped);
    }

    #[test]
    fn gives_up_after_repeated_failures() {
        let albums = many(30, 1);
        let lib = library(&albums);
        // The fake knows none of these albums: every getAlbum fails.
        let fake = Fake::new(Vec::new(), false);
        let out = rt().block_on(run(lib, fake, Arc::default(), |_| {}));
        assert!(matches!(out, Outcome::Failed(e) if e.contains("not found")));
    }
}
