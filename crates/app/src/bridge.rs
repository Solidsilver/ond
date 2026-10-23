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
    /// Every stored track, grouped by album; sent after load and after a sync.
    /// Every stored track, grouped and indexed (built here, off the UI thread).
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
        subsonic::Error::Url(_) => "That doesn't look like a server address (e.g. https://music.example.com).".into(),
        subsonic::Error::Http(h) if h.is_connect() || h.is_timeout() => "Couldn't reach the server. Check the address and that it's running.".into(),
        subsonic::Error::Http(h) if h.status().is_some() => format!("The server answered with an error ({}).", h.status().map(|s| s.to_string()).unwrap_or_default()),
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

#[derive(Clone)]
pub struct Bridge {
    rt: tokio::runtime::Handle,
    tx: Sender<Msg>,
    notify: Notify,
    library: Arc<Library>,
    /// Stops the running track sync, if any.
    sync_cancel: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    /// A reconnect loop is running (one is enough).
    reconnecting: Arc<AtomicBool>,
    /// Bumped on sign-out: connects and reconnect loops started before it stop
    /// and their results are ignored.
    session: Arc<AtomicU64>,
    /// The last request sent per heart or rating: the next one for the same
    /// item waits for it, so the server sees them in the order they were made.
    in_order: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
}

impl Bridge {
    pub fn new(rt: tokio::runtime::Handle, tx: Sender<Msg>, notify: Notify, library: Arc<Library>) -> Self {
        Self {
            rt,
            tx,
            notify,
            library,
            sync_cancel: Arc::default(),
            reconnecting: Arc::default(),
            session: Arc::default(),
            in_order: Arc::default(),
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
    /// anything they still deliver is recognised as stale.
    pub fn end_session(&self) {
        self.session.fetch_add(1, Ordering::AcqRel);
        self.reconnecting.store(false, Ordering::Relaxed);
    }

    fn live(&self, session: u64) -> bool {
        self.session() == session
    }

    /// The UI's wake-up, for other background work to share.
    pub fn notifier(&self) -> Notify {
        self.notify.clone()
    }

    fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
        (self.notify)();
    }

    /// Runs `f` on the library off the async workers (redb blocks).
    async fn db<T: Send + 'static>(&self, f: impl FnOnce(&Library) -> library::Result<T> + Send + 'static) -> Result<T, String> {
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
    pub fn build_search(&self, generation: u64, albums: Vec<Album>, artists: Vec<library::Artist>, tracks: Arc<Vec<Track>>) {
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
                Err(e) => return this.send(Msg::Error(format!("read library: {e}"))),
            }
            if let Ok(a) = this.library.artists() {
                this.send(Msg::Artists(a));
            }
            this.load_all_tracks();
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
            Err(e) => self.send(Msg::Error(format!("read tracks: {e}"))),
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
                        None => return this.send(Msg::ConnectFailed("no saved password".into())),
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
    /// mirror) and reports the sign-in.
    async fn connected(&self, session: u64, client: Client, settings: Settings, server: String) {
        let new_account = match settings.account_id() {
            Some(id) => self.db(move |l| l.claim_account(id)).await.unwrap_or_else(|e| {
                tracing::warn!("recording the account: {e}");
                false
            }),
            None => false,
        };
        self.send(Msg::Connected { session, client, settings: Box::new(settings), server, new_account });
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

    /// Pages through every album on the server into the local db.
    pub fn sync_albums(&self, client: Client) {
        let this = self.clone();
        self.rt.spawn(async move {
            let progress = |n| this.send(Msg::SyncProgress(n));
            match crate::sync::albums(&this.library, &client, progress).await {
                Ok(albums) => {
                    let n = albums.len();
                    this.send(Msg::Albums(albums));
                    this.send(Msg::AlbumSyncDone(Ok(n)));
                }
                Err(e) => return this.send(Msg::AlbumSyncDone(Err(e))),
            }
            match crate::sync::artists(&this.library, &client).await {
                Ok(stored) => this.send(Msg::Artists(stored)),
                Err(e) => this.send(Msg::Error(e)),
            }
            this.sync_tracks(client);
        });
    }

    /// Fetches the tracks of every album that changed since the last sync, in
    /// the background. Replaces (cancels) a sync already running.
    pub fn sync_tracks(&self, client: Client) {
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(old) = self.sync_cancel.lock().unwrap_or_else(|e| e.into_inner()).replace(cancel.clone()) {
            old.store(true, Ordering::Relaxed);
        }
        let this = self.clone();
        self.rt.spawn(async move {
            // At most ~10 progress repaints a second, however fast albums arrive.
            let last = Mutex::new(Instant::now() - Duration::from_secs(1));
            let outcome = sync::run(this.library.clone(), client, cancel.clone(), |p| {
                let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
                if p.done == p.total || last.elapsed() >= Duration::from_millis(100) {
                    *last = Instant::now();
                    this.send(Msg::TrackSync(p));
                }
            })
            .await;
            // Show what was stored, unless nothing changed. A cancelled sync
            // still stored something worth showing.
            if outcome != (sync::Outcome::Done { fetched: 0, bulk: false }) {
                let me = this.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    if let Ok(albums) = me.library.albums() {
                        me.send(Msg::Albums(albums));
                    }
                    me.load_all_tracks();
                })
                .await;
            }
            // A newer sync took over: it reports for itself.
            if this.is_current(&cancel) {
                this.send(Msg::TrackSyncDone(outcome));
            }
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
        let this = self.clone();
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
                Err(e) => return this.send(Msg::LoadFailed { key, error: Some(format!("load album: {e}")) }),
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
                Err(e) => this.send(Msg::LoadFailed { key, error: Some(format!("store album: {e}")) }),
            }
        });
    }

    /// Recently added albums and the user's playlists; both small, fetched on connect.
    pub fn fetch_home(&self, client: Client) {
        let this = self.clone();
        self.rt.spawn(async move {
            match client.album_list(AlbumListType::Newest, 120, 0).await {
                Ok(albums) => {
                    let ids = albums.iter().map(|a| a.id.clone()).collect();
                    // Newest albums may predate the last full sync.
                    let _ = this.db(move |l| l.upsert_albums(albums)).await;
                    this.send(Msg::Newest(ids));
                }
                Err(e) => this.send(Msg::Error(format!("recently added: {e}"))),
            }
            match client.playlists().await {
                Ok(p) => this.send(Msg::Playlists(p)),
                Err(e) => this.send(Msg::Error(format!("playlists: {e}"))),
            }
        });
    }

    pub fn open_playlist(&self, client: Option<Client>, id: String) {
        let Some(client) = client else {
            return self.send(Msg::LoadFailed { key: ListKey::Playlist(id), error: None });
        };
        let this = self.clone();
        self.rt.spawn(async move {
            match client.playlist(&id).await {
                Ok(p) => {
                    let meta = song_meta(&p.entry);
                    let tracks: Vec<Track> = p.entry.into_iter().map(song_track).collect();
                    this.send(Msg::UserMeta { meta, tracks: tracks.clone() });
                    this.send(Msg::PlaylistTracks { id, tracks });
                }
                Err(e) => this.send(Msg::LoadFailed { key: ListKey::Playlist(id), error: Some(format!("load playlist: {e}")) }),
            }
        });
    }

    /// Everything the user starred, and the ratings that come with it.
    pub fn fetch_starred(&self, client: Option<Client>) {
        let Some(client) = client else { return };
        let this = self.clone();
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
                Err(e) => this.send(Msg::Error(format!("favorites: {e}"))),
            }
        });
    }

    /// Lyrics for one track: structured (synced where possible) first, then
    /// plain `getLyrics` by artist and title. Failures read as "no lyrics":
    /// they're a nicety, never worth an error banner.
    pub fn fetch_lyrics(&self, client: Option<Client>, track_id: String, artist: Option<String>, title: String) {
        let Some(client) = client else { return };
        let this = self.clone();
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
        let this = self.clone();
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
        let this = self.clone();
        self.after_previous(format!("star:{kind:?}:{id}"), async move {
            let error = match &client {
                Some(c) => c.set_starred(kind, &id, on).await.err().map(|e| e.to_string()),
                None => Some("not connected".into()),
            };
            this.send(Msg::StarDone { kind, id, on, seq, error });
        });
    }

    pub fn set_rating(&self, client: Option<Client>, id: String, rating: u8, previous: u8, seq: u64) {
        let this = self.clone();
        self.after_previous(format!("rating:{id}"), async move {
            let error = match &client {
                Some(c) => c.set_rating(&id, rating).await.err().map(|e| e.to_string()),
                None => Some("not connected".into()),
            };
            this.send(Msg::RatingDone { id, previous, seq, error });
        });
    }

    /// Makes a playlist of `song_ids`; the app opens it when it's there.
    pub fn create_playlist(&self, client: Option<Client>, name: String, song_ids: Vec<String>) {
        let Some(client) = client else { return self.send(Msg::Error("not connected".into())) };
        let this = self.clone();
        self.rt.spawn(async move {
            match client.create_playlist(&name, &song_ids).await {
                Ok(p) => {
                    this.refresh_playlists(&client).await;
                    this.send(Msg::PlaylistCreated { id: p.playlist.id, name: p.playlist.name });
                }
                Err(e) => this.send(Msg::Error(format!("new playlist: {e}"))),
            }
        });
    }

    /// Sends one edit, then reloads the playlist list and the playlist (which
    /// also undoes the app's optimistic copy if the edit failed).
    pub fn edit_playlist(&self, client: Option<Client>, id: String, edit: PlaylistEdit) {
        let Some(client) = client else { return self.send(Msg::Error("not connected".into())) };
        let this = self.clone();
        self.rt.spawn(async move {
            let result = match &edit {
                PlaylistEdit::Rename(name) => client.update_playlist(&id, Some(name), &[], &[]).await,
                PlaylistEdit::Add(songs) => client.update_playlist(&id, None, songs, &[]).await,
                PlaylistEdit::Remove(at) => client.update_playlist(&id, None, &[], at).await,
                PlaylistEdit::Replace(songs) => client.replace_playlist(&id, songs).await,
                PlaylistEdit::Delete => client.delete_playlist(&id).await,
            };
            if let Err(e) = result {
                this.send(Msg::Error(format!("playlist: {e}")));
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
            Err(e) => self.send(Msg::Error(format!("playlists: {e}"))),
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
        let this = self.clone();
        self.rt.spawn(async move {
            let mut all = Vec::new();
            for id in album_ids {
                let lookup = id.clone();
                let stored = this.db(move |l| l.album_tracks(&lookup)).await.unwrap_or_default();
                if !stored.is_empty() {
                    all.extend(stored);
                    continue;
                }
                let Some(client) = &client else { continue };
                let Ok(album) = client.album(&id).await else { continue };
                if let Ok(tracks) = this.db(move |l| l.upsert_album_tracks(album).and_then(|_| l.album_tracks(&id))).await {
                    all.extend(tracks);
                }
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
        Err(_) => Err(PingError::Unreachable("no answer from the server".into())),
        Ok(Err(e @ subsonic::Error::Http(_))) => Err(PingError::Unreachable(e.to_string())),
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

    #[test]
    fn connect_errors_read_plainly() {
        let wrong = subsonic::Error::Api { code: 40, message: "Wrong username or password".into() };
        assert_eq!(connect_error(&wrong), "Wrong username or password.");
        let other = subsonic::Error::Api { code: 50, message: "Not allowed".into() };
        assert_eq!(connect_error(&other), "The server said: Not allowed");
        let url = subsonic::Client::new("not a url", subsonic::Credentials::new("a", "b")).err().unwrap();
        assert!(connect_error(&url).contains("server address"));
    }
}
