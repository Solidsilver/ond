//! Background work for the UI. Every function here returns immediately; results
//! arrive later as [`Msg`]s, each followed by a repaint request.

use crate::favorites::{StarKind, Starred, UserMeta};
use crate::settings::Settings;
use crate::sync;
use library::{Album, Library, Track};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use subsonic::{AlbumListType, Client, Credentials};

pub enum Msg {
    /// `session`: the bridge's session when the connect began; the app drops
    /// it if the user signed out since.
    /// Signed in. `new_account` means the local library belonged to another
    /// server or user and was cleared for this one.
    Connected { session: u64, client: Client, settings: Box<Settings>, server: String, new_account: bool },
    ConnectFailed(String),
    /// The server can't be reached (saved login, so the app opens offline);
    /// reconnecting continues in the background and ends with `Connected`.
    Unreachable { session: u64, username: String, error: String },
    /// A ping's round trip, or `None` when it got no answer.
    Latency(Option<Duration>),
    /// A search index built off the UI thread, for the request numbered `generation`.
    SearchIndex { generation: u64, index: Box<crate::search::SearchIndex> },
    Albums(Vec<Album>),
    SyncProgress(usize),
    /// The album list sync finished (`Ok`: how many albums) or failed.
    AlbumSyncDone(Result<usize, String>),
    /// An album's tracks. `fresh`: straight from the server's `getAlbum` (the
    /// complete list), not the local library (which may be partial).
    Tracks { album_id: String, tracks: Vec<Track>, fresh: bool },
    /// An album row refreshed from `getAlbum` (labels, genres).
    AlbumUpdated(Album),
    /// Ids of the most recently added albums, newest first.
    Newest(Vec<String>),
    Playlists(Vec<subsonic::Playlist>),
    PlaylistTracks { id: String, tracks: Vec<Track> },
    /// Albums whose stored tracks credit `name`.
    Credited { name: String, album_ids: Vec<String> },
    /// Tracks of several albums, gathered for one queue action (`key` says which).
    TrackBatch { key: ListKey, tracks: Vec<Track> },
    /// An album's or playlist's tracks couldn't be loaded (offline with
    /// nothing stored, or the server failed). Whatever waited for them gives
    /// up. `error` is worth showing; `None` when offline explains it already.
    LoadFailed { key: ListKey, error: Option<String> },
    /// The server's artists (split from joined credits), sorted.
    Artists(Vec<library::Artist>),
    /// Every stored track, grouped by album and indexed (built here, off the
    /// UI thread); sent after load and after a sync.
    AllTracks(Box<crate::tracks::TrackSet>),
    TrackSync(sync::Progress),
    TrackSyncDone(sync::Outcome),
    /// A track's analysis just finished.
    Analyzed { track_id: String, record: analysis::Record },
    /// Every stored analysis, read at startup.
    AnalysesLoaded(Vec<(String, analysis::Record)>),
    Engine(audio::Event),
    /// Media keys, Control Center, headphone buttons.
    Remote(crate::now_playing::Remote),
    /// A folder chosen to keep offline downloads in.
    OfflineFolder(std::path::PathBuf),
    /// An offline store opened off the UI thread, for the open numbered
    /// `generation` (a newer one may have replaced it since).
    OfflineStore { generation: u64, store: Result<offline::OfflineStore, String> },
    /// The downloads folder is moving: how far it got.
    OfflineMoving(offline::MoveProgress),
    /// The move finished, or stopped (it's tried again at the next start).
    OfflineMoved(Result<(), String>),
    /// `getStarred2`: everything loved, and the ratings seen with it.
    Starred { starred: Starred, ratings: Vec<(String, u8)> },
    /// A track's lyrics arrived (`None`: the server has none).
    Lyrics { track_id: String, lyrics: Option<crate::lyrics::Lyrics> },
    /// A song's play count and last-played time (`None` when the server
    /// couldn't say).
    SongStats { track_id: String, stats: Option<crate::app::SongStats> },
    /// Hearts and ratings seen while loading an album or playlist.
    UserMeta { meta: UserMeta, tracks: Vec<Track> },
    /// A heart change reached the server (`error` is `None`) or didn't.
    StarDone { kind: StarKind, id: String, on: bool, seq: u64, error: Option<String> },
    RatingDone { id: String, previous: u8, seq: u64, error: Option<String> },
    /// A playlist was made; the app opens it.
    PlaylistCreated { id: String, name: String },
    /// Selected from the native menu bar.
    Menu(crate::keys::Cmd),
    Error(String),
}

/// A track list something can wait for: an album's, a playlist's, or all of
/// an artist's albums together.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ListKey {
    Album(String),
    Playlist(String),
    Artist(String),
}

/// A change to a playlist, sent to the server.
#[derive(Debug, Clone)]
pub enum PlaylistEdit {
    Rename(String),
    Add(Vec<String>),
    /// Indices into the playlist as the server has it.
    Remove(Vec<usize>),
    /// The whole song list, in its new order.
    Replace(Vec<String>),
    Delete,
}

/// What the user sees when connecting fails: the gist, without the plumbing.
pub fn connect_error(e: &subsonic::Error) -> String {
    match e {
        // 40: wrong credentials; 41: token auth not allowed for LDAP users.
        subsonic::Error::Api { code: 40 | 41, .. } => "Wrong username or password.".into(),
        subsonic::Error::Api { message, .. } if !message.is_empty() => format!("The server said: {message}"),
        subsonic::Error::Url(_) => "That doesn't look like a server address. Try something like https://music.example.com.".into(),
        subsonic::Error::Http(h) if h.is_connect() || h.is_timeout() => "Couldn't reach the server. Check the address and that it's running.".into(),
        subsonic::Error::Http(h) if let Some(status) = h.status() => {
            // What the server (or a proxy in front of it) said, unless it only
            // repeats the status, as "502 Bad Gateway" pages do.
            let said = h.message().map(str::trim).filter(|m| {
                let m = m.trim_end_matches('.');
                !m.is_empty() && !m.eq_ignore_ascii_case(&status.to_string()) && !status.canonical_reason().is_some_and(|r| m.eq_ignore_ascii_case(r)) && m != status.as_str()
            });
            match said {
                Some(said) => format!("The server answered with an error ({status}): {said}"),
                None => format!("The server answered with an error ({status})."),
            }
        }
        subsonic::Error::Decode(_) => "That address answered, but not like a Navidrome server.".into(),
        other => other.to_string(),
    }
}

/// Hearts and ratings carried by a list of songs.
fn song_meta(songs: &[subsonic::Song]) -> UserMeta {
    UserMeta {
        starred: songs.iter().map(|s| (StarKind::Song, s.id.clone(), s.starred.is_some())).collect(),
        ratings: songs.iter().map(|s| (s.id.clone(), s.user_rating.unwrap_or(0))).collect(),
    }
}

/// Wakes the UI so it reads what was sent (a repaint, in the app).
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// The [`Bridge::after_previous`] chain offline store jobs run on.
const OFFLINE_JOBS: &str = "offline";

#[derive(Clone)]
pub struct Bridge {
    rt: tokio::runtime::Handle,
    tx: Sender<Msg>,
    notify: Notify,
    library: Arc<Library>,
    /// Stops the running track sync, if any.
    sync_cancel: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    /// Bumped by each album sync: an older pass still running (from before a
    /// reconnect) stops storing and reporting once a newer one starts.
    album_pass: Arc<AtomicU64>,
    /// A track sync that was replaced stored something it didn't show; the
    /// next one to finish reloads the library even if it fetched nothing.
    reload_owed: Arc<AtomicBool>,
    /// A reconnect loop is running (one is enough).
    reconnecting: Arc<AtomicBool>,
    /// Bumped on sign-out: connects and reconnect loops started before it stop
    /// and their results are ignored.
    session: Arc<AtomicU64>,
    /// The last request sent per heart or rating: the next one for the same
    /// item waits for it, so the server sees them in the order they were made.
    in_order: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    /// Set on a copy made by [`scoped`](Self::scoped): the session its work
    /// belongs to. Once that session ends, the copy's library writes and
    /// messages are dropped.
    scope: Option<u64>,
}

impl Bridge {
    pub fn new(rt: tokio::runtime::Handle, tx: Sender<Msg>, notify: Notify, library: Arc<Library>) -> Self {
        Self {
            rt,
            tx,
            notify,
            library,
            sync_cancel: Arc::default(),
            album_pass: Arc::default(),
            reload_owed: Arc::default(),
            reconnecting: Arc::default(),
            session: Arc::default(),
            in_order: Arc::default(),
            scope: None,
        }
    }

    /// Runs `job` after the previous job queued under `key` has finished.
    fn after_previous<F>(&self, key: String, job: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let mut chains = self.in_order.lock().unwrap_or_else(|e| e.into_inner());
        chains.retain(|_, h| !h.is_finished());
        let previous = chains.remove(&key);
        let handle = self.rt.spawn(async move {
            if let Some(p) = previous {
                let _ = p.await;
            }
            job.await;
        });
        chains.insert(key, handle);
    }

    /// The current login session (see [`end_session`](Self::end_session)).
    pub fn session(&self) -> u64 {
        self.session.load(Ordering::Acquire)
    }

    /// Signing out: connects and reconnect loops in flight give up, and
    /// anything they still deliver is recognised as stale. Server requests
    /// still running store nothing more: the next sign-in may be another
    /// account, whose library they'd mix into.
    pub fn end_session(&self) {
        self.session.fetch_add(1, Ordering::AcqRel);
        self.reconnecting.store(false, Ordering::Relaxed);
        self.cancel_sync();
    }

    fn live(&self, session: u64) -> bool {
        self.session() == session
    }

    /// A copy for work on the current session's behalf (see `scope`).
    fn scoped(&self) -> Self {
        self.scoped_to(self.scope.unwrap_or_else(|| self.session()))
    }

    /// A copy for work on `session`'s behalf, which may have ended already.
    fn scoped_to(&self, session: u64) -> Self {
        Self { scope: Some(session), ..self.clone() }
    }

    /// False once the session this copy works for has ended.
    fn current(&self) -> bool {
        self.scope.is_none_or(|s| self.live(s))
    }

    /// The UI's wake-up, for other background work to share.
    pub fn notifier(&self) -> Notify {
        self.notify.clone()
    }

    fn send(&self, msg: Msg) {
        if !self.current() {
            return;
        }
        let _ = self.tx.send(msg);
        (self.notify)();
    }

    /// Runs `f` on the library off the async workers (redb blocks).
    async fn db<T: Send + 'static>(&self, f: impl FnOnce(&Library) -> library::Result<T> + Send + 'static) -> Result<T, String> {
        if !self.current() {
            return Err("Signed out".into());
        }
        crate::sync::blocking(&self.library, f).await
    }

    /// Delivers `msg` to the UI from any thread.
    pub fn post(&self, msg: Msg) {
        self.send(msg);
    }

    /// Forwards engine events into the UI channel so `Ended` is seen even when
    /// idle; `tap` sees each one first.
    pub fn forward_engine_events(&self, events: audio::EventReceiver<audio::Event>, tap: impl Fn(&audio::Event) + Send + 'static) {
        let this = self.clone();
        std::thread::Builder::new()
            .name("engine-events".into())
            .spawn(move || {
                while let Ok(ev) = events.recv() {
                    tap(&ev);
                    this.send(Msg::Engine(ev));
                }
            })
            .expect("spawn event forwarder");
    }

    /// Builds the search index on a worker (tens of ms on a big library) and
    /// sends it back tagged with `generation`, so a stale build can be dropped.
    pub fn build_search(&self, generation: u64, albums: Arc<Vec<Album>>, artists: Arc<Vec<library::Artist>>, tracks: Arc<Vec<Track>>) {
        let this = self.clone();
        self.rt.spawn_blocking(move || {
            let index = crate::search::SearchIndex::new(&albums, &artists, &tracks);
            this.send(Msg::SearchIndex { generation, index: Box::new(index) });
        });
    }

    /// Loads the cached library from disk without touching the network.
    pub fn load_cached_albums(&self) {
        let this = self.clone();
        self.rt.spawn_blocking(move || {
            match this.library.albums() {
                Ok(a) => this.send(Msg::Albums(a)),
                Err(e) => return this.send(Msg::Error(format!("Couldn't read the library: {e}"))),
            }
            if let Ok(a) = this.library.artists() {
                this.send(Msg::Artists(a));
            }
            this.load_all_tracks();
        });
    }

    /// Runs `job` on the blocking pool after every job queued here before it
    /// has finished: opening, closing and moving offline stores. One at a time
    /// and in order, so a store is never opened on a folder whose previous
    /// store is still shutting down (or still moving in). `job` delivers its
    /// own results through the bridge it's given.
    pub fn offline_job(&self, job: impl FnOnce(&Bridge) + Send + 'static) {
        let this = self.clone();
        self.after_previous(OFFLINE_JOBS.into(), async move {
            let _ = tokio::task::spawn_blocking(move || job(&this)).await;
        });
    }

    /// Reads the stored analysis results.
    pub fn load_analyses(&self, store: Arc<library::AnalysisStore>) {
        let this = self.clone();
        self.rt.spawn_blocking(move || match store.records() {
            Ok(rows) => {
                let records = rows.into_iter().filter_map(|(id, b)| analysis::Record::decode(&b).map(|r| (id, r))).collect();
                this.send(Msg::AnalysesLoaded(records));
            }
            Err(e) => tracing::warn!("reading analyses: {e}"),
        });
    }

    /// Blocking: reads every stored track and sends them to the UI.
    fn load_all_tracks(&self) {
        match self.library.all_tracks() {
            Ok(t) => self.send(Msg::AllTracks(Box::new(crate::tracks::TrackSet::new(t)))),
            Err(e) => self.send(Msg::Error(format!("Couldn't read the library: {e}"))),
        }
    }

    /// Pings the server; on success stores the password in the keychain.
    ///
    /// With a saved login (`password` is `None`) an unreachable server isn't a
    /// login failure: the app opens offline and keeps retrying until it's back.
    pub fn connect(&self, settings: Settings, password: Option<String>) {
        let this = self.clone();
        let session = self.session();
        self.rt.spawn(async move {
            let saved = password.is_none();
            let password = match password {
                Some(p) => p,
                None => {
                    let s = settings.clone();
                    match tokio::task::spawn_blocking(move || s.load_password()).await.ok().flatten() {
                        Some(p) => p,
                        None => return this.send(Msg::ConnectFailed("No saved password. Sign in again.".into())),
                    }
                }
            };
            let client = match Client::new(&settings.server, Credentials::new(&settings.username, &password)) {
                Ok(c) => c,
                Err(e) => return this.send(Msg::ConnectFailed(connect_error(&e))),
            };
            let pinged = ping(&client).await;
            if !this.live(session) {
                return;
            }
            match pinged {
                Ok(server) => {
                    // The account itself is recorded by the app (in its preferences)
                    // when `Connected` arrives for a live session.
                    let s = settings.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        if let Err(e) = s.store_password(&password) {
                            tracing::warn!("keychain: {e}");
                        }
                    })
                    .await;
                    this.connected(session, client, settings, server).await;
                }
                Err(PingError::Unreachable(e)) if saved => {
                    this.send(Msg::Unreachable { session, username: settings.username.clone(), error: e });
                    this.reconnect(client, settings, session);
                }
                Err(PingError::Unreachable(e) | PingError::Refused(e)) => this.send(Msg::ConnectFailed(e)),
            }
        });
    }

    /// Claims the local library for this account (clearing another account's
    /// mirror) and reports the sign-in. Nothing happens once `session` has
    /// ended (the keychain write before this can wait on a prompt for a while).
    async fn connected(&self, session: u64, client: Client, settings: Settings, server: String) {
        let this = self.scoped_to(session);
        if !this.current() {
            return;
        }
        let new_account = match settings.account_id() {
            Some(id) => match this.db(move |l| l.claim_account(id)).await {
                Ok(new) => new,
                Err(_) if !this.current() => return,
                Err(e) => {
                    tracing::warn!("recording the account: {e}");
                    false
                }
            },
            None => false,
        };
        this.send(Msg::Connected { session, client, settings: Box::new(settings), server, new_account });
    }

    /// Times one ping (for the server popover and the diagnostics bar).
    pub fn measure_latency(&self, client: Client) {
        let this = self.clone();
        self.rt.spawn(async move {
            let start = std::time::Instant::now();
            let ok = ping(&client).await.is_ok();
            this.send(Msg::Latency(ok.then(|| start.elapsed())));
        });
    }

    /// Checks the connection after something failed; if the server is gone,
    /// says so and reconnects in the background.
    pub fn probe(&self, client: Client, settings: Settings) {
        if self.reconnecting.load(Ordering::Relaxed) {
            return;
        }
        let this = self.clone();
        let session = self.session();
        self.rt.spawn(async move {
            if let Err(PingError::Unreachable(e)) = ping(&client).await
                && this.live(session)
            {
                this.send(Msg::Unreachable { session, username: settings.username.clone(), error: e });
                this.reconnect(client, settings, session);
            }
        });
    }

    /// Pings with backoff (5 s, doubling to a minute) until the server answers,
    /// or until the user signs out (`session` ends).
    fn reconnect(&self, client: Client, settings: Settings, session: u64) {
        if !self.live(session) || self.reconnecting.swap(true, Ordering::Relaxed) {
            return;
        }
        let this = self.clone();
        self.rt.spawn(async move {
            let mut delay = Duration::from_secs(5);
            loop {
                tokio::time::sleep(delay).await;
                if !this.live(session) {
                    // Signed out: `end_session` already cleared `reconnecting`
                    // (and a new loop may own it now).
                    return;
                }
                let pinged = ping(&client).await;
                if !this.live(session) {
                    return;
                }
                match pinged {
                    Ok(server) => {
                        this.reconnecting.store(false, Ordering::Relaxed);
                        return this.connected(session, client, settings, server).await;
                    }
                    Err(PingError::Unreachable(_)) => delay = (delay * 2).min(Duration::from_secs(60)),
                    Err(PingError::Refused(e)) => {
                        // Reachable but turning us away (password changed): back to the login form.
                        this.reconnecting.store(false, Ordering::Relaxed);
                        return this.send(Msg::ConnectFailed(e));
                    }
                }
            }
        });
    }

    /// Pages through every album on the server into the local db. Replaces a
    /// pass already running (e.g. from before the server went away): that one
    /// stores and reports nothing more.
    pub fn sync_albums<F: sync::Fetch>(&self, client: F) {
        let pass = self.album_pass.fetch_add(1, Ordering::AcqRel) + 1;
        let this = self.scoped();
        self.rt.spawn(async move {
            let live = || this.current() && this.album_pass.load(Ordering::Acquire) == pass;
            let progress = |n| {
                if live() {
                    this.send(Msg::SyncProgress(n));
                }
            };
            let listed = crate::sync::albums(&this.library, &client, &live, progress).await;
            if !live() {
                return;
            }
            match listed {
                Ok(albums) => {
                    let n = albums.len();
                    this.send(Msg::Albums(albums));
                    this.send(Msg::AlbumSyncDone(Ok(n)));
                }
                Err(e) => return this.send(Msg::AlbumSyncDone(Err(e))),
            }
            let artists = crate::sync::artists(&this.library, &client, &live).await;
            if !live() {
                return;
            }
            match artists {
                Ok(stored) => this.send(Msg::Artists(stored)),
                Err(e) => this.send(Msg::Error(e)),
            }
            this.sync_tracks(client);
        });
    }

    /// Fetches the tracks of every album that changed since the last sync, in
    /// the background. Replaces (cancels) a sync already running; from then on
    /// that one reports nothing (the new one does).
    pub fn sync_tracks<F: sync::Fetch>(&self, client: F) {
        // From a session that's over (an album pass finishing after sign-out):
        // it mustn't cancel the new session's sync, let alone store anything.
        if !self.current() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(old) = self.sync_cancel.lock().unwrap_or_else(|e| e.into_inner()).replace(cancel.clone()) {
            old.store(true, Ordering::Relaxed);
        }
        let this = self.scoped();
        self.rt.spawn(async move {
            // At most ~10 progress repaints a second, however fast albums arrive.
            let last = Mutex::new(Instant::now() - Duration::from_secs(1));
            let outcome = sync::run(this.library.clone(), client, cancel.clone(), || this.current(), |p| {
                let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
                if (p.done == p.total || last.elapsed() >= Duration::from_millis(100)) && this.is_current(&cancel) {
                    *last = Instant::now();
                    this.send(Msg::TrackSync(p));
                }
            })
            .await;
            let stored = outcome != (sync::Outcome::Done { fetched: 0, bulk: false });
            // A newer sync took over: it reports, and shows what this one
            // stored, for itself.
            if !this.is_current(&cancel) {
                if stored {
                    this.reload_owed.store(true, Ordering::Release);
                }
                return;
            }
            // Show what was stored, unless nothing changed. A cancelled sync
            // still stored something worth showing.
            if this.reload_owed.swap(false, Ordering::AcqRel) || stored {
                let me = this.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    if let Ok(albums) = me.library.albums() {
                        me.send(Msg::Albums(albums));
                    }
                    me.load_all_tracks();
                })
                .await;
            }
            this.send(Msg::TrackSyncDone(outcome));
        });
    }

    fn is_current(&self, cancel: &Arc<AtomicBool>) -> bool {
        self.sync_cancel.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|c| Arc::ptr_eq(c, cancel))
    }

    /// Stops the running track sync; what it fetched so far is kept.
    pub fn cancel_sync(&self) {
        if let Some(c) = self.sync_cancel.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            c.store(true, Ordering::Relaxed);
        }
    }

    /// Shows cached tracks immediately, then refreshes them from the server.
    pub fn open_album(&self, client: Option<Client>, album_id: String) {
        let this = self.scoped();
        self.rt.spawn(async move {
            let id = album_id.clone();
            let key = ListKey::Album(album_id.clone());
            let mut cached = false;
            if let Ok(tracks) = this.db(move |l| l.album_tracks(&id)).await
                && !tracks.is_empty()
            {
                cached = true;
                this.send(Msg::Tracks { album_id: album_id.clone(), tracks, fresh: false });
            }
            let Some(client) = client else {
                if !cached {
                    this.send(Msg::LoadFailed { key, error: None });
                }
                return;
            };
            let album = match client.album(&album_id).await {
                Ok(a) => a,
                Err(e) => return this.send(Msg::LoadFailed { key, error: Some(format!("Couldn't load the album: {e}")) }),
            };
            let mut meta = song_meta(&album.song);
            meta.starred.push((StarKind::Album, album_id.clone(), album.album.starred.is_some()));
            let id = album_id.clone();
            let stored = this
                .db(move |l| {
                    l.upsert_album_tracks(album)?;
                    Ok((l.album(&id)?, l.album_tracks(&id)?))
                })
                .await;
            match stored {
                Ok((row, tracks)) => {
                    if let Some(row) = row {
                        this.send(Msg::AlbumUpdated(row));
                    }
                    this.send(Msg::UserMeta { meta, tracks: tracks.clone() });
                    this.send(Msg::Tracks { album_id, tracks, fresh: true });
                }
                Err(e) => this.send(Msg::LoadFailed { key, error: Some(format!("Couldn't save the album: {e}")) }),
            }
        });
    }

    /// Recently added albums and the user's playlists; both small, fetched on connect.
    pub fn fetch_home(&self, client: Client) {
        let this = self.scoped();
        self.rt.spawn(async move {
            match client.album_list(AlbumListType::Newest, 120, 0).await {
                Ok(albums) => {
                    let ids = albums.iter().map(|a| a.id.clone()).collect();
                    // Newest albums may predate the last full sync; announced
                    // only once they're stored, or they'd show as missing.
                    match this.db(move |l| l.upsert_albums(albums)).await {
                        Ok(()) => this.send(Msg::Newest(ids)),
                        Err(e) => this.send(Msg::Error(format!("Couldn't save Recently added: {e}"))),
                    }
                }
                Err(e) => this.send(Msg::Error(format!("Couldn't load Recently added: {e}"))),
            }
            match client.playlists().await {
                Ok(p) => this.send(Msg::Playlists(p)),
                Err(e) => this.send(Msg::Error(format!("Couldn't load playlists: {e}"))),
            }
        });
    }

    pub fn open_playlist(&self, client: Option<Client>, id: String) {
        let Some(client) = client else {
            return self.send(Msg::LoadFailed { key: ListKey::Playlist(id), error: None });
        };
        let this = self.scoped();
        self.rt.spawn(async move {
            match client.playlist(&id).await {
                Ok(p) => {
                    let meta = song_meta(&p.entry);
                    let tracks: Vec<Track> = p.entry.into_iter().map(song_track).collect();
                    this.send(Msg::UserMeta { meta, tracks: tracks.clone() });
                    this.send(Msg::PlaylistTracks { id, tracks });
                }
                Err(e) => this.send(Msg::LoadFailed { key: ListKey::Playlist(id), error: Some(format!("Couldn't load the playlist: {e}")) }),
            }
        });
    }

    /// Everything the user starred, and the ratings that come with it.
    pub fn fetch_starred(&self, client: Option<Client>) {
        let Some(client) = client else { return };
        let this = self.scoped();
        self.rt.spawn(async move {
            match client.starred().await {
                Ok(s) => {
                    let mut ratings: Vec<(String, u8)> = s.song.iter().filter_map(|t| t.user_rating.filter(|r| *r > 0).map(|r| (t.id.clone(), r))).collect();
                    ratings.extend(s.album.iter().filter_map(|a| a.user_rating.filter(|r| *r > 0).map(|r| (a.id.clone(), r))));
                    let starred = Starred {
                        songs: s.song.into_iter().map(song_track).collect(),
                        albums: s.album.into_iter().map(|a| a.id).collect(),
                        artists: s.artist.into_iter().map(|a| (a.id, a.name)).collect(),
                    };
                    this.send(Msg::Starred { starred, ratings });
                }
                Err(e) => this.send(Msg::Error(format!("Couldn't load favorites: {e}"))),
            }
        });
    }

    /// Lyrics for one track: structured (synced where possible) first, then
    /// plain `getLyrics` by artist and title. Failures read as "no lyrics":
    /// they're a nicety, never worth an error banner.
    pub fn fetch_lyrics(&self, client: Option<Client>, track_id: String, artist: Option<String>, title: String) {
        let Some(client) = client else { return };
        let this = self.scoped();
        self.rt.spawn(async move {
            let mut lyrics = match client.lyrics_by_song(&track_id).await {
                Ok(versions) => crate::lyrics::pick(versions),
                Err(e) => {
                    tracing::debug!("getLyricsBySongId {track_id}: {e}");
                    None
                }
            };
            if lyrics.is_none()
                && let Some(artist) = artist.as_deref()
            {
                match client.lyrics(artist, &title).await {
                    Ok(text) => lyrics = text.as_deref().and_then(crate::lyrics::from_text),
                    Err(e) => tracing::debug!("getLyrics {title}: {e}"),
                }
            }
            this.send(Msg::Lyrics { track_id, lyrics });
        });
    }

    /// The playing song's play count and when it was last played, for the
    /// Info tab. Quietly nothing on failure.
    pub fn fetch_song_stats(&self, client: Option<Client>, track_id: String) {
        let Some(client) = client else { return };
        let this = self.scoped();
        self.rt.spawn(async move {
            let stats = match client.song(&track_id).await {
                Ok(s) => Some(crate::app::SongStats { plays: s.play_count, last_played: s.played }),
                Err(e) => {
                    tracing::debug!("getSong {track_id}: {e}");
                    None
                }
            };
            this.send(Msg::SongStats { track_id, stats });
        });
    }

    /// Loves or unloves; `seq` comes back in the reply so a stale one is recognised.
    pub fn set_starred(&self, client: Option<Client>, kind: StarKind, id: String, on: bool, seq: u64) {
        let this = self.scoped();
        self.after_previous(format!("star:{kind:?}:{id}"), async move {
            let error = match &client {
                Some(c) => c.set_starred(kind, &id, on).await.err().map(|e| e.to_string()),
                None => Some("Not connected to a server".into()),
            };
            this.send(Msg::StarDone { kind, id, on, seq, error });
        });
    }

    pub fn set_rating(&self, client: Option<Client>, id: String, rating: u8, previous: u8, seq: u64) {
        let this = self.scoped();
        self.after_previous(format!("rating:{id}"), async move {
            let error = match &client {
                Some(c) => c.set_rating(&id, rating).await.err().map(|e| e.to_string()),
                None => Some("Not connected to a server".into()),
            };
            this.send(Msg::RatingDone { id, previous, seq, error });
        });
    }

    /// Makes a playlist of `song_ids`; the app opens it when it's there.
    pub fn create_playlist(&self, client: Option<Client>, name: String, song_ids: Vec<String>) {
        let Some(client) = client else { return self.send(Msg::Error("Not connected to a server".into())) };
        let this = self.scoped();
        self.rt.spawn(async move {
            match client.create_playlist(&name, &song_ids).await {
                Ok(p) => {
                    this.refresh_playlists(&client).await;
                    this.send(Msg::PlaylistCreated { id: p.playlist.id, name: p.playlist.name });
                }
                Err(e) => this.send(Msg::Error(format!("Couldn't create the playlist: {e}"))),
            }
        });
    }

    /// Sends one edit, then reloads the playlist list and the playlist (which
    /// also undoes the app's optimistic copy if the edit failed).
    pub fn edit_playlist(&self, client: Option<Client>, id: String, edit: PlaylistEdit) {
        let Some(client) = client else { return self.send(Msg::Error("Not connected to a server".into())) };
        let this = self.scoped();
        self.rt.spawn(async move {
            let result = match &edit {
                PlaylistEdit::Rename(name) => client.update_playlist(&id, Some(name), &[], &[]).await,
                PlaylistEdit::Add(songs) => client.update_playlist(&id, None, songs, &[]).await,
                PlaylistEdit::Remove(at) => client.update_playlist(&id, None, &[], at).await,
                PlaylistEdit::Replace(songs) => client.replace_playlist(&id, songs).await,
                PlaylistEdit::Delete => client.delete_playlist(&id).await,
            };
            if let Err(e) = result {
                this.send(Msg::Error(format!("Couldn't change the playlist: {e}")));
            }
            this.refresh_playlists(&client).await;
            if !matches!(edit, PlaylistEdit::Delete) {
                this.open_playlist(Some(client), id);
            }
        });
    }

    async fn refresh_playlists(&self, client: &Client) {
        match client.playlists().await {
            Ok(p) => self.send(Msg::Playlists(p)),
            Err(e) => self.send(Msg::Error(format!("Couldn't load playlists: {e}"))),
        }
    }

    /// Looks up which stored albums credit `name` (composer, performer, ...).
    pub fn find_credited(&self, name: String) {
        let this = self.clone();
        self.rt.spawn_blocking(move || {
            if let Ok(album_ids) = this.library.albums_crediting(&name) {
                this.send(Msg::Credited { name, album_ids });
            }
        });
    }

    /// Gathers the tracks of `album_ids` in order: stored ones straight from the
    /// db, the rest from the server (and stored on the way).
    pub fn fetch_tracks(&self, client: Option<Client>, key: ListKey, album_ids: Vec<String>) {
        let this = self.scoped();
        self.rt.spawn(async move {
            let mut all = Vec::new();
            let mut missed = 0;
            for id in album_ids {
                let lookup = id.clone();
                let stored = this.db(move |l| l.album_tracks(&lookup)).await.unwrap_or_default();
                if !stored.is_empty() {
                    all.extend(stored);
                    continue;
                }
                // Offline, an album that isn't stored just isn't there to play.
                let Some(client) = &client else { continue };
                let fetched = match client.album(&id).await {
                    Ok(album) => this.db(move |l| l.upsert_album_tracks(album).and_then(|_| l.album_tracks(&id))).await,
                    Err(e) => Err(e.to_string()),
                };
                match fetched {
                    Ok(tracks) => all.extend(tracks),
                    Err(e) => {
                        tracing::warn!("fetching an album's tracks: {e}");
                        missed += 1;
                    }
                }
            }
            if missed > 0 {
                let albums = if missed == 1 { "1 album".to_owned() } else { format!("{missed} albums") };
                this.send(Msg::Error(format!("Couldn't load the songs of {albums}")));
            }
            this.send(Msg::TrackBatch { key, tracks: all });
        });
    }
}

enum PingError {
    /// No answer: network down, server off, timeout, or a proxy's error page.
    Unreachable(String),
    /// The server answered and said no (bad credentials, unsupported).
    Refused(String),
}

/// Pings with a timeout, so a black-holed server can't hang the connect.
async fn ping(client: &Client) -> Result<String, PingError> {
    match tokio::time::timeout(Duration::from_secs(10), client.ping()).await {
        Err(_) => Err(PingError::Unreachable("The server didn't answer".into())),
        Ok(Err(e @ subsonic::Error::Http(_))) => {
            tracing::debug!("ping: {e}");
            Err(PingError::Unreachable(connect_error(&e)))
        }
        Ok(Err(e)) => Err(PingError::Refused(connect_error(&e))),
        Ok(Ok(info)) => Ok(format!("{} {}", info.server_type.as_deref().unwrap_or("subsonic"), info.server_version.as_deref().unwrap_or(&info.version))),
    }
}

fn song_track(s: subsonic::Song) -> Track {
    let album = s.album_id.clone().unwrap_or_default();
    Track::from_song(s, &album)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::tests::{album, song};
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc::Receiver;
    use tokio::sync::Semaphore;

    /// A server whose requests can be held until the test lets them go.
    #[derive(Clone, Default)]
    struct Server {
        albums: Vec<subsonic::Album>,
        /// Requests wait for a permit here (none: they answer at once).
        gate: Option<Arc<Semaphore>>,
        /// The album list fails (after the gate).
        list_fails: bool,
        /// Requests that reached the server, and ones it answered.
        asked: Arc<AtomicUsize>,
        answered: Arc<AtomicUsize>,
    }

    impl Server {
        async fn wait(&self) {
            self.asked.fetch_add(1, Ordering::SeqCst);
            if let Some(g) = &self.gate {
                g.acquire().await.unwrap().forget();
            }
        }
    }

    impl sync::Fetch for Server {
        async fn album_page(&self, _: u32, offset: u32) -> Result<Vec<subsonic::Album>, String> {
            self.wait().await;
            self.answered.fetch_add(1, Ordering::SeqCst);
            if self.list_fails {
                return Err("server went away".into());
            }
            Ok(if offset == 0 { self.albums.clone() } else { Vec::new() })
        }
        async fn artists(&self) -> Result<Vec<subsonic::Artist>, String> {
            Ok(Vec::new())
        }
        async fn album(&self, id: String) -> Result<subsonic::AlbumWithSongs, String> {
            self.wait().await;
            let a = self.albums.iter().find(|a| a.id == id).cloned().ok_or("not found")?;
            let songs = (1..=a.song_count).map(|n| song(format!("{id}-{n}"), &id, n)).collect();
            self.answered.fetch_add(1, Ordering::SeqCst);
            Ok(subsonic::AlbumWithSongs { album: a, song: songs })
        }
        async fn all_songs(&self, _: &'static str, _: u32, _: u32) -> Result<Vec<subsonic::Song>, String> {
            Ok(Vec::new())
        }
    }

    /// One thread runs every task, and only while the test waits in `until`:
    /// when `until` sees a condition, every task has run as far as it can.
    fn rig(albums: &[subsonic::Album]) -> (tokio::runtime::Runtime, Bridge, Receiver<Msg>) {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let lib = Library::in_memory().unwrap();
        lib.upsert_albums(albums.to_vec()).unwrap();
        let bridge = Bridge::new(rt.handle().clone(), tx, Arc::new(|| {}), Arc::new(lib));
        (rt, bridge, rx)
    }

    /// Runs the bridge's tasks until `done`, collecting what they send.
    fn until(rt: &tokio::runtime::Runtime, rx: &Receiver<Msg>, got: &mut Vec<Msg>, mut done: impl FnMut(&[Msg]) -> bool) {
        rt.block_on(async {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                got.extend(rx.try_iter());
                if done(got) {
                    return;
                }
                assert!(Instant::now() < deadline, "timed out");
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
    }

    fn names(msgs: &[Msg]) -> Vec<&'static str> {
        msgs.iter()
            .map(|m| match m {
                Msg::TrackSync(_) => "TrackSync",
                Msg::TrackSyncDone(_) => "TrackSyncDone",
                Msg::AllTracks(_) => "AllTracks",
                Msg::Albums(_) => "Albums",
                Msg::Artists(_) => "Artists",
                Msg::SyncProgress(_) => "SyncProgress",
                Msg::AlbumSyncDone(Ok(_)) => "AlbumSyncDone",
                Msg::AlbumSyncDone(Err(_)) => "AlbumSyncFailed",
                Msg::Error(_) => "Error",
                _ => "other",
            })
            .collect()
    }

    #[test]
    fn a_replaced_track_sync_reports_nothing_more() {
        let albums: Vec<_> = (0..3).map(|i| album(&format!("a{i}"), 2)).collect();
        let (rt, bridge, rx) = rig(&albums);
        let gate = Arc::new(Semaphore::new(0));
        let slow = Server { albums: albums.clone(), gate: Some(gate.clone()), ..Default::default() };
        let mut got = Vec::new();
        bridge.sync_tracks(slow.clone());
        // Its three album requests are out, waiting on the server.
        until(&rt, &rx, &mut got, |_| slow.asked.load(Ordering::SeqCst) == 3);
        // Sync now: a new sync takes over and finishes.
        bridge.sync_tracks(Server { albums, ..Default::default() });
        until(&rt, &rx, &mut got, |m| matches!(m.last(), Some(Msg::TrackSyncDone(_))));
        assert!(matches!(got.last(), Some(Msg::TrackSyncDone(sync::Outcome::Done { fetched: 3, .. }))));
        got.clear();
        // The old one's answers come in now: it stores them, but says nothing,
        // and leaves the reload to the next sync.
        gate.add_permits(3);
        until(&rt, &rx, &mut got, |_| bridge.reload_owed.load(Ordering::Acquire));
        assert_eq!(slow.answered.load(Ordering::SeqCst), 3);
        assert!(got.is_empty(), "the replaced sync sent {:?}", names(&got));
    }

    #[test]
    fn a_newer_album_sync_replaces_one_still_running() {
        let albums: Vec<_> = (0..2).map(|i| album(&format!("a{i}"), 1)).collect();
        let (rt, bridge, rx) = rig(&albums);
        let gate = Arc::new(Semaphore::new(0));
        // Started before the server went away; its request hangs.
        let old = Server { albums: albums.clone(), gate: Some(gate.clone()), list_fails: true, ..Default::default() };
        let mut got = Vec::new();
        bridge.sync_albums(old.clone());
        until(&rt, &rx, &mut got, |_| old.asked.load(Ordering::SeqCst) == 1);
        // Reconnected: a new pass runs to the end.
        bridge.sync_albums(Server { albums, ..Default::default() });
        until(&rt, &rx, &mut got, |m| matches!(m.last(), Some(Msg::TrackSyncDone(_))));
        assert!(names(&got).contains(&"AlbumSyncDone"));
        got.clear();
        // The old request finally fails: no "Sync failed" after reconnecting.
        gate.add_permits(1);
        until(&rt, &rx, &mut got, |_| old.answered.load(Ordering::SeqCst) == 1);
        // (Nothing in that task awaits after the answer: it has finished.)
        until(&rt, &rx, &mut got, |_| true);
        assert!(got.is_empty(), "the replaced pass sent {:?}", names(&got));
    }

    #[test]
    fn work_for_a_session_that_ended_sends_nothing() {
        let (rt, bridge, rx) = rig(&[]);
        let scoped = bridge.scoped();
        scoped.send(Msg::Error("before".into()));
        bridge.end_session();
        scoped.send(Msg::Error("after".into()));
        // Copies of a copy keep the session they were made for.
        scoped.scoped().send(Msg::Error("after, again".into()));
        bridge.scoped().send(Msg::Error("the new session".into()));
        let texts: Vec<String> = rx.try_iter().filter_map(|m| if let Msg::Error(e) = m { Some(e) } else { None }).collect();
        assert_eq!(texts, ["before", "the new session"]);
        let stored = rt.block_on(scoped.db(|l| l.albums()));
        assert_eq!(stored.unwrap_err(), "Signed out", "nor reads or writes the library");
    }

    #[test]
    fn requests_for_one_item_run_in_the_order_made() {
        let (rt, bridge, _rx) = rig(&[]);
        let order = Arc::new(Mutex::new(Vec::new()));
        let gate = Arc::new(Semaphore::new(0));
        let job = |n: u32, wait: bool| {
            let (order, gate) = (order.clone(), gate.clone());
            async move {
                if wait {
                    gate.acquire().await.unwrap().forget();
                }
                order.lock().unwrap().push(n);
            }
        };
        bridge.after_previous("star:a".into(), job(1, true));
        bridge.after_previous("star:a".into(), job(2, false));
        // Another item doesn't wait for the first.
        bridge.after_previous("star:b".into(), job(3, false));
        rt.block_on(async {
            while order.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        });
        assert_eq!(*order.lock().unwrap(), [3], "2 waits for 1, which is held");
        gate.add_permits(1);
        rt.block_on(async {
            while order.lock().unwrap().len() < 3 {
                tokio::task::yield_now().await;
            }
        });
        assert_eq!(*order.lock().unwrap(), [3, 1, 2]);
    }

    #[test]
    fn offline_jobs_run_one_after_another_off_the_async_workers() {
        let (rt, bridge, rx) = rig(&[]);
        let (gate_tx, gate) = std::sync::mpsc::channel::<()>();
        // The first holds its blocking thread (closing a store, say) until let go.
        bridge.offline_job(move |b| {
            let _ = gate.recv();
            b.post(Msg::Error("first".into()));
        });
        bridge.offline_job(|b| b.post(Msg::Error("second".into())));
        let mut got = Vec::new();
        rt.block_on(async { tokio::time::sleep(Duration::from_millis(20)).await });
        got.extend(rx.try_iter());
        assert!(got.is_empty(), "the second waits for the first");
        gate_tx.send(()).unwrap();
        until(&rt, &rx, &mut got, |m| m.len() == 2);
        let texts: Vec<&str> = got.iter().map(|m| if let Msg::Error(e) = m { e.as_str() } else { "" }).collect();
        assert_eq!(texts, ["first", "second"]);
    }

    #[test]
    fn connect_errors_read_plainly() {
        let wrong = subsonic::Error::Api { code: 40, message: "Wrong username or password".into() };
        assert_eq!(connect_error(&wrong), "Wrong username or password.");
        let other = subsonic::Error::Api { code: 50, message: "Not allowed".into() };
        assert_eq!(connect_error(&other), "The server said: Not allowed");
        let url = subsonic::Client::new("not a url", subsonic::Credentials::new("a", "b")).err().unwrap();
        assert!(connect_error(&url).contains("server address"));
    }

    /// Answers one request on a local port with `status` and an HTML page
    /// titled `title`. Returns the base URL.
    fn answer_once(status: &'static str, title: &'static str) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let (mut req, mut buf) = (Vec::new(), [0u8; 1024]);
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => req.extend_from_slice(&buf[..n]),
                }
            }
            let body = format!("<html><head><title>{title}</title></head><body>proxy</body></html>");
            let _ = write!(s, "HTTP/1.1 {status}\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        });
        format!("http://{addr}/")
    }

    /// Signing in to a server behind a reverse proxy that answers with an
    /// error page shows what the page says, in the friendly sentence, and
    /// without repeating the status when that's all the page says.
    #[test]
    fn a_failed_sign_in_shows_what_the_server_said() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let sign_in = |base: String| {
            let client = Client::new(&base, Credentials::new("joe", "sesame")).unwrap();
            match rt.block_on(ping(&client)) {
                Err(PingError::Unreachable(e)) => e,
                Err(PingError::Refused(e)) => panic!("refused: {e}"),
                Ok(s) => panic!("connected to {s}"),
            }
        };
        assert_eq!(
            sign_in(answer_once("502 Bad Gateway", "Navidrome is restarting, try again in a minute")),
            "The server answered with an error (502 Bad Gateway): Navidrome is restarting, try again in a minute"
        );
        assert_eq!(sign_in(answer_once("502 Bad Gateway", "502 Bad Gateway")), "The server answered with an error (502 Bad Gateway).");
        assert_eq!(sign_in(answer_once("503 Service Unavailable", "Service Unavailable")), "The server answered with an error (503 Service Unavailable).");
    }
}
