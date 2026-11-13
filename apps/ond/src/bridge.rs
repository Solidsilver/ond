//! Background work for the UI. Every function here returns immediately; results
//! arrive later as [`Msg`]s, each followed by a repaint request.

mod local_watch;

use crate::failure::Failure;
use crate::favorites::{SavedStarred, StarKind, Starred, UserMeta};
use crate::settings::Settings;
use crate::source::{Playlist, Src, subsonic::SubsonicSource};
use crate::sync;
use library::{Album, Library, Track};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use subsonic::{Client, Credentials};

/// What background work tells the UI. Messages for one part of the app are
/// grouped under it ([`LibraryMsg`], [`AnalysisMsg`], [`OfflineMsg`]), and
/// that part's handler matches its own group whole, so a message added to a
/// group can't be left unhandled.
pub enum Msg {
    OndPlaylistChanged,
    MixedPlaylistReady {
        id: String,
        name: String,
        existing: Vec<Track>,
        added: Vec<Track>,
    },
    OndPlaylists {
        generation: u64,
        playlists: Vec<Playlist>,
        tracks: HashMap<String, Arc<Vec<Track>>>,
    },
    /// Both enqueue and inbox checks use this account's independent session.
    Source {
        account: accounts::AccountId,
        session: u64,
        message: Box<Msg>,
    },
    /// Keeps a result scoped even while it waits in the UI inbox. Checking
    /// only before enqueueing leaves a gap if an account switches meanwhile.
    Scoped {
        session: u64,
        message: Box<Msg>,
    },
    /// Signed in. `new_account` means another server or user took over the
    /// local mirror. `password_saved` is false
    /// when the keychain wouldn't take the password just typed, so the next
    /// launch will ask for it again.
    Connected {
        /// The bridge's session when the connect began; the app drops the
        /// message if the user signed out since.
        session: u64,
        source: Src,
        stores: Option<crate::storage::Stores>,
        settings: Box<Settings>,
        server: String,
        new_account: bool,
        password_saved: bool,
    },
    /// The sign-in didn't work, and `error` says why.
    ConnectFailed {
        /// As for `Connected`: a failure from an ended session is dropped,
        /// so it can't knock the one signed in since offline.
        session: u64,
        error: String,
    },
    /// The server can't be reached (saved login, so the app opens offline);
    /// reconnecting continues in the background and ends with `Connected`.
    Unreachable {
        session: u64,
        username: String,
        error: String,
    },
    /// A ping's round trip, or `None` when it got no answer.
    Latency(Option<Duration>),
    /// The library, its sync, and track lists that loaded.
    Library(LibraryMsg),
    /// Library analysis.
    Analysis(AnalysisMsg),
    /// The offline store and its folder.
    Offline(OfflineMsg),
    Engine(audio::Event),
    /// Media keys, Control Center, headphone buttons.
    Remote(crate::now_playing::Remote),
    /// `getStarred2`: everything loved, and the ratings seen with it.
    Starred {
        starred: Starred,
        ratings: Vec<(String, u8)>,
    },
    /// A track's lyrics arrived (`None`: the server has none).
    Lyrics {
        track_id: String,
        lyrics: Option<crate::lyrics::Lyrics>,
    },
    /// A song's play count and last-played time (`None` when the server
    /// couldn't say).
    SongStats {
        track_id: String,
        stats: Option<crate::lyrics::SongStats>,
    },
    /// Hearts and ratings seen while loading an album or playlist.
    UserMeta {
        meta: UserMeta,
        tracks: Vec<Track>,
    },
    /// A heart change reached the server (`error` is `None`) or didn't.
    StarDone {
        kind: StarKind,
        id: String,
        on: bool,
        seq: u64,
        error: Option<Failure>,
    },
    RatingDone {
        id: String,
        previous: u8,
        seq: u64,
        error: Option<Failure>,
    },
    /// A playlist was made; the app opens it.
    PlaylistCreated {
        id: String,
        name: String,
    },
    /// A save location was picked for an M3U playlist export.
    M3uPathChosen {
        id: String,
        path: std::path::PathBuf,
    },
    /// Exporting a playlist to M3U finished (or failed).
    M3uExported {
        path: std::path::PathBuf,
        error: Option<Failure>,
    },
    /// Selected from the native menu bar.
    Menu(crate::keys::Cmd),
    /// The folder picker's answer, and what it was asked for.
    FolderChosen {
        target: crate::app::FolderTarget,
        path: std::path::PathBuf,
    },
    /// The update checker's latest release, or why it couldn't tell.
    UpdateChecked(Result<crate::updates::Version, String>),
    /// A release's notes, for Settings › About.
    ReleaseNotes(crate::updates::Version, crate::updates::Notes),
    Error(Failure),
}

/// About the library, its sync, the search index, and track lists that
/// loaded (`app/data.rs` handles them).
pub enum LibraryMsg {
    ScopeTracks {
        key: ListKey,
        parts: Vec<(accounts::AccountId, u64, Vec<Track>)>,
    },
    ArtistMbids(Vec<(String, String)>),
    /// A search index built off the UI thread, for the request numbered `generation`.
    SearchIndex {
        generation: u64,
        index: Box<crate::search::SearchIndex>,
    },
    Albums(Vec<Album>),
    SyncProgress(usize),
    /// The album list sync finished (`Ok`: how many albums) or failed.
    AlbumSyncDone(Result<usize, Failure>),
    /// Configured local folders that were missing during the latest scan (empty
    /// when every configured folder was present). A nonempty list also ends
    /// the scan when no roots are available; it is source state, not an error.
    FolderMissing(Vec<std::path::PathBuf>),
    /// Files in the local folders that looked like audio but couldn't be
    /// read, on the latest scan.
    Skipped(usize),
    /// An album's tracks. `fresh`: straight from the server's `getAlbum` (the
    /// complete list), not the local library (which may be partial).
    Tracks {
        album_id: String,
        tracks: Vec<Track>,
        fresh: bool,
    },
    /// An album row refreshed from `getAlbum` (labels, genres).
    AlbumUpdated(Box<Album>),
    /// Ids of the most recently added albums, newest first.
    Newest(Vec<String>),
    Playlists(Vec<Playlist>),
    /// A playlist's songs. `fresh`: the server's listing, not the saved copy.
    PlaylistTracks {
        id: String,
        tracks: Vec<Track>,
        fresh: bool,
    },
    /// The copies saved last time of what the server lists outside the
    /// library, read at startup so they show before it answers (or offline).
    Saved(Box<Saved>),
    /// Albums whose stored tracks credit `name`.
    Credited {
        name: String,
        album_ids: Vec<String>,
    },
    /// Tracks of several albums, gathered for one queue action (`key` says which).
    TrackBatch {
        key: ListKey,
        tracks: Vec<Track>,
    },
    /// An album's or playlist's tracks couldn't be loaded (offline with
    /// nothing stored, or the server failed). Whatever waited for them gives
    /// up. `error` is worth showing; `None` when offline explains it already.
    LoadFailed {
        key: ListKey,
        error: Option<Failure>,
    },
    /// The server's artists (split from joined credits), sorted.
    Artists(Vec<library::Artist>),
    /// Every stored track, grouped by album and indexed (built here, off the
    /// UI thread); sent after load and after a sync.
    AllTracks(Box<crate::tracks::TrackSet>),
    TrackSync(sync::Progress),
    TrackSyncDone(sync::Outcome),
}

/// About library analysis (`app/analyzing.rs` handles them).
pub enum AnalysisMsg {
    Store(Arc<library::AnalysisStore>),
    /// A track's analysis just finished.
    Analyzed {
        key: crate::source::TrackKey,
        record: analysis::Record,
    },
    /// Every stored analysis, read at startup.
    Loaded(Vec<(String, analysis::Record)>),
}

/// About the offline store (`app/pins.rs` handles them).
pub enum OfflineMsg {
    /// A folder chosen to keep offline downloads in.
    Folder(std::path::PathBuf),
    /// An offline store opened off the UI thread, for the open numbered
    /// `generation` (a newer one may have replaced it since).
    Store { generation: u64, store: Result<offline::OfflineStore, String> },
    /// The downloads folder is moving: how far it got.
    Moving(offline::MoveProgress),
    /// The move finished, or stopped (it's tried again at the next start).
    Moved(Result<(), String>),
}

impl From<LibraryMsg> for Msg {
    fn from(m: LibraryMsg) -> Self {
        Msg::Library(m)
    }
}

impl From<AnalysisMsg> for Msg {
    fn from(m: AnalysisMsg) -> Self {
        Msg::Analysis(m)
    }
}

impl From<OfflineMsg> for Msg {
    fn from(m: OfflineMsg) -> Self {
        Msg::Offline(m)
    }
}

/// What the server lists outside the library, as saved last time; `None`
/// where nothing was.
#[derive(Default)]
pub struct Saved {
    pub newest: Option<Vec<String>>,
    pub playlists: Option<Vec<Playlist>>,
    pub starred: Option<SavedStarred>,
}

/// Names of the saved copies (see [`library::Library::save`]).
const SAVED_NEWEST: &str = "newest";
const SAVED_PLAYLISTS: &str = "playlists";
const SAVED_STARRED: &str = "starred";

fn saved_playlist_name(id: &str) -> String {
    format!("playlist/{id}")
}

/// Frozen playlist layout: changing the live source model must not change
/// the copies already saved in existing libraries.
#[derive(serde::Serialize, serde::Deserialize)]
struct SavedPlaylist {
    id: String,
    name: String,
    song_count: u32,
    duration: u32,
    cover_art: Option<String>,
    owner: Option<String>,
}

impl From<&Playlist> for SavedPlaylist {
    fn from(p: &Playlist) -> Self {
        Self { id: p.id.clone(), name: p.name.clone(), song_count: p.song_count, duration: p.duration, cover_art: p.cover_art.clone(), owner: p.owner.clone() }
    }
}

impl From<SavedPlaylist> for Playlist {
    fn from(p: SavedPlaylist) -> Self {
        Self { id: p.id, name: p.name, song_count: p.song_count, duration: p.duration, cover_art: p.cover_art, owner: p.owner }
    }
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

/// More time passed on the wall clock than on the monotonic one: the
/// computer slept.
const SLEPT: Duration = Duration::from_secs(5);

/// What the user sees when connecting fails: the gist, without the plumbing.
pub fn connect_error(e: &subsonic::Error) -> String {
    match e {
        subsonic::Error::Api { code: 40, .. } => "Wrong username or password.".into(),
        // Token sign-in turned away (LDAP-backed accounts): Ond signs in no
        // other way, so asking for the password again won't help.
        subsonic::Error::Api { code: 41, .. } => "This server doesn't allow the way Ond signs in.".into(),
        subsonic::Error::Api { message, .. } if !message.is_empty() => format!("The server said: {message}"),
        subsonic::Error::Url(_) => "That doesn't look like a server address. Try something like https://music.example.com.".into(),
        subsonic::Error::Http(h) if h.is_connect() || h.is_timeout() => "Couldn't reach the server. Check the address and that it's running.".into(),
        subsonic::Error::Http(h) if let Some(status) = h.status() => {
            // What the server (or a proxy in front of it) said, unless it only
            // repeats the status, as "502 Bad Gateway" pages do.
            let said = h.message().map(str::trim).filter(|m| {
                let m = m.trim_end_matches('.');
                !m.is_empty()
                    && !m.eq_ignore_ascii_case(&status.to_string())
                    && !status.canonical_reason().is_some_and(|r| m.eq_ignore_ascii_case(r))
                    && m != status.as_str()
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

/// How the stream cache and offline downloads tell a Subsonic error reply
/// (or a proxy's page) from a track: the server answers `200` either way.
pub fn reply_check() -> streamcache::ReplyCheck {
    streamcache::ReplyCheck::new(subsonic::is_error_reply, subsonic::error_text)
}

/// Wakes the UI so it reads what was sent (a repaint, in the app).
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// What a server request the user asked for says when there's no server.
const NOT_CONNECTED: &str = "Not connected to a server";

/// The [`Bridge::after_previous`] chain offline store jobs run on.
const OFFLINE_JOBS: &str = "offline";

#[derive(Clone)]
pub struct Bridge {
    pub(super) account_tag: Option<accounts::AccountId>,
    rt: tokio::runtime::Handle,
    tx: Sender<Msg>,
    notify: Notify,
    libraries: Arc<Mutex<LibrarySlot>>,
    scoped_library: Option<Arc<Library>>,
    ond_store: Arc<crate::store::OndStore>,
    /// Stops the running track sync, if any.
    sync_cancel: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    /// One event-driven scan queue for the live local source.
    local_watch: Arc<Mutex<Option<local_watch::LocalWatch>>>,
    /// Keeps a cancelled blocking scan from overlapping its replacement.
    local_scan: Arc<tokio::sync::Mutex<()>>,
    /// Bumped by each album sync: an older pass still running (from before a
    /// reconnect) stops storing and reporting once a newer one starts.
    album_pass: Arc<AtomicU64>,
    /// A track sync that was replaced stored something it didn't show; the
    /// next one to finish reloads the library even if it fetched nothing.
    reload_owed: Arc<AtomicBool>,
    /// A reconnect loop is running (one is enough).
    reconnecting: Arc<AtomicBool>,
    /// Wakes the reconnect loop to try now (the window came to the front).
    retry_now: Arc<tokio::sync::Notify>,
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

/// Scoped work holds its own library, so even a write already queued on the
/// blocking pool can never follow a later account into its database.
struct LibrarySlot {
    library: Arc<Library>,
    account: Option<accounts::AccountId>,
    data: Option<std::path::PathBuf>,
    art_cache: Option<std::path::PathBuf>,
    analyses: std::sync::Weak<library::AnalysisStore>,
}

impl Bridge {
    pub fn new(rt: tokio::runtime::Handle, tx: Sender<Msg>, notify: Notify, library: Arc<Library>, ond_store: Arc<crate::store::OndStore>) -> Self {
        Self {
            account_tag: None,
            rt,
            tx,
            notify,
            libraries: Arc::new(Mutex::new(LibrarySlot { library, account: None, data: None, art_cache: None, analyses: std::sync::Weak::new() })),
            scoped_library: None,
            ond_store,
            sync_cancel: Arc::default(),
            local_watch: Arc::default(),
            local_scan: Arc::default(),
            album_pass: Arc::default(),
            reload_owed: Arc::default(),
            reconnecting: Arc::default(),
            retry_now: Arc::default(),
            session: Arc::default(),
            in_order: Arc::default(),
            scope: None,
        }
    }

    pub fn fork_source(&self, account: accounts::AccountId, library: Arc<Library>) -> Self {
        let mut bridge = Self::new(self.rt.clone(), self.tx.clone(), self.notify.clone(), library, self.ond_store.clone());
        bridge.account_tag = Some(account);
        let old = self.libraries.lock().unwrap_or_else(|e| e.into_inner());
        let mut slot = bridge.libraries.lock().unwrap_or_else(|e| e.into_inner());
        slot.data = old.data.clone();
        slot.art_cache = old.art_cache.clone();
        if old.account == Some(account) {
            slot.account = old.account;
            slot.analyses = old.analyses.clone();
        }
        drop(slot);
        bridge
    }

    pub fn storage_account(&self) -> Option<accounts::AccountId> {
        self.libraries.lock().unwrap_or_else(|e| e.into_inner()).account
    }

    /// Opens a source independently, then reads its mirror before connecting.
    pub fn open_source(&self, settings: Settings, keep: Option<crate::local::Playable>, connect: bool) {
        let mut this = self.scoped();
        self.rt.spawn(async move {
            let account = settings.account().expect("configured source");
            match this.prepare_storage(account).await {
                Ok(Some(stores)) if this.current() => {
                    this.scoped_library = Some(stores.library.clone());
                    let analyses = this.install_storage(account, stores, &[account]);
                    this.load_analyses(analyses);
                }
                Ok(None) => {
                    let analyses = this.libraries.lock().unwrap_or_else(|e| e.into_inner()).analyses.upgrade();
                    if let Some(analyses) = analyses {
                        this.load_analyses(analyses);
                    }
                }
                Ok(Some(_)) => return,
                Err(e) => return this.send(Msg::ConnectFailed { session: this.session(), error: format!("Couldn't open the library: {e}") }),
            }
            if !this.current() {
                return;
            }
            this.load_local(keep);
            if connect {
                this.connect(settings, None);
            }
        });
    }

    pub fn library_handle(&self) -> Arc<Library> {
        self.library()
    }

    /// A lightweight callback that posts a message without keeping the stores
    /// or runtime handles inside [`Bridge`] alive (for global OS hooks).
    pub fn poster(&self) -> impl Fn(Msg) + Clone + Send + Sync + 'static {
        let tx = self.tx.clone();
        let notify = self.notify.clone();
        move |msg| {
            let _ = tx.send(msg);
            (notify)();
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
        let _slot = self.libraries.lock().unwrap_or_else(|e| e.into_inner());
        self.session.fetch_add(1, Ordering::AcqRel);
        self.reconnecting.store(false, Ordering::Relaxed);
        self.cancel_sync();
        self.local_watch.lock().unwrap_or_else(|e| e.into_inner()).take();
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
        let library = self.scoped_library.clone().unwrap_or_else(|| self.library());
        Self { scope: Some(session), scoped_library: Some(library), ..self.clone() }
    }

    fn library(&self) -> Arc<Library> {
        self.scoped_library.clone().unwrap_or_else(|| self.libraries.lock().unwrap_or_else(|e| e.into_inner()).library.clone())
    }

    pub fn configure_storage(
        &self,
        data: std::path::PathBuf,
        art_cache: std::path::PathBuf,
        account: Option<accounts::AccountId>,
        analyses: &Arc<library::AnalysisStore>,
    ) {
        let mut slot = self.libraries.lock().unwrap_or_else(|e| e.into_inner());
        slot.data = Some(data);
        slot.art_cache = Some(art_cache);
        slot.account = account;
        slot.analyses = Arc::downgrade(analyses);
    }

    /// Installs a prepared mirror only after the UI accepts its live sign-in.
    /// A cancelled connect can therefore never replace the current library.
    /// The previous account's folder and cached art are removed unless it's
    /// one of `keep` (still configured, only not the one in use).
    pub fn install_storage(&self, account: accounts::AccountId, stores: crate::storage::Stores, keep: &[accounts::AccountId]) -> Arc<library::AnalysisStore> {
        let old = {
            let mut slot = self.libraries.lock().unwrap_or_else(|e| e.into_inner());
            let old = (slot.account, Arc::downgrade(&slot.library), slot.analyses.clone(), slot.data.clone(), slot.art_cache.clone());
            slot.library = stores.library;
            slot.account = Some(account);
            slot.analyses = Arc::downgrade(&stores.analyses);
            old
        };
        if let Some(account) = old.0.filter(|a| !keep.contains(a)) {
            if let Err(e) = self.ond_store.remove_account(account) {
                tracing::warn!("removing ond.redb data for {account}: {e}");
            }
            self.remove_account_files(account, old.1, old.2, old.3, old.4);
        }
        stores.analyses
    }

    /// A source was removed: its folder (library, scan index, analyses),
    /// its cached art and its entries in `ond.redb` go. When it's the one in
    /// use, an empty library in memory takes its place, whose analysis store
    /// is returned for the app to hand on.
    pub fn forget_account(&self, account: accounts::AccountId) -> Option<Arc<library::AnalysisStore>> {
        if let Err(e) = self.ond_store.remove_account(account) {
            tracing::warn!("removing ond.redb data for {account}: {e}");
        }
        let (old, fresh) = {
            let mut slot = self.libraries.lock().unwrap_or_else(|e| e.into_inner());
            if slot.account == Some(account) {
                let fresh = match crate::storage::Stores::in_memory() {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("opening an empty library: {e}");
                        return None;
                    }
                };
                let old = (Arc::downgrade(&slot.library), slot.analyses.clone());
                slot.library = fresh.library;
                slot.account = None;
                slot.analyses = Arc::downgrade(&fresh.analyses);
                (old, Some(fresh.analyses))
            } else {
                ((std::sync::Weak::new(), std::sync::Weak::new()), None)
            }
        };
        let (data, art) = {
            let slot = self.libraries.lock().unwrap_or_else(|e| e.into_inner());
            (slot.data.clone(), slot.art_cache.clone())
        };
        self.remove_account_files(account, old.0, old.1, data, art);
        fresh
    }

    /// Removes `account`'s folder and cached art once the old library and
    /// analysis handles have all been dropped.
    fn remove_account_files(
        &self,
        account: accounts::AccountId,
        library: std::sync::Weak<Library>,
        analyses: std::sync::Weak<library::AnalysisStore>,
        data: Option<std::path::PathBuf>,
        art: Option<std::path::PathBuf>,
    ) {
        let Some(data) = data else { return };
        let dir = crate::storage::account_dir(&data, account);
        let art_dir = art.as_ref().map(|root| crate::art::account_dir(root, account));
        self.rt.spawn(async move {
            // Old requests may still be returning from the network. Their
            // handles keep the old database alive until they have stopped.
            while library.strong_count() > 0 || analyses.strong_count() > 0 {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let _ = tokio::task::spawn_blocking(move || {
                for (what, path) in [("library", Some(dir)), ("art cache", art_dir)] {
                    if let Some(path) = path
                        && let Err(e) = std::fs::remove_dir_all(&path)
                        && e.kind() != std::io::ErrorKind::NotFound
                    {
                        tracing::warn!("removing previous {what} {}: {e}", path.display());
                    }
                }
            })
            .await;
        });
    }

    async fn prepare_storage(&self, account: accounts::AccountId) -> Result<Option<crate::storage::Stores>, String> {
        let data = {
            let slot = self.libraries.lock().unwrap_or_else(|e| e.into_inner());
            if slot.account == Some(account) {
                return Ok(None);
            }
            slot.data.clone()
        };
        let Some(data) = data else {
            return Ok(None);
        };
        let (stores, notices) = tokio::task::spawn_blocking(move || {
            let mut notices = Vec::new();
            crate::storage::open(&data, Some(account), &mut notices).map(|s| (s, notices))
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        for notice in notices {
            self.send(Msg::Error(Failure::Local(notice)));
        }
        Ok(Some(stores))
    }

    /// False once the session this copy works for has ended.
    fn current(&self) -> bool {
        self.scope.is_none_or(|s| self.live(s))
    }

    /// The runtime its work runs on, for other background work to share.
    pub fn runtime(&self) -> &tokio::runtime::Handle {
        &self.rt
    }

    /// The UI's wake-up, for other background work to share.
    pub fn notifier(&self) -> Notify {
        self.notify.clone()
    }

    fn send(&self, msg: impl Into<Msg>) {
        if !self.current() {
            return;
        }
        let msg = msg.into();
        let msg = if let Some(account) = self.account_tag { crate::source::identity::message(account, msg) } else { msg };
        let msg = match (self.scope, msg) {
            (_, msg @ (Msg::Connected { .. } | Msg::ConnectFailed { .. } | Msg::Unreachable { .. } | Msg::Scoped { .. })) => msg,
            (Some(session), msg) => Msg::Scoped { session, message: Box::new(msg) },
            (None, msg) => msg,
        };
        let msg = if let Some(account) = self.account_tag {
            Msg::Source { account, session: self.scope.unwrap_or_else(|| self.session()), message: Box::new(msg) }
        } else {
            msg
        };
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
        crate::sync::blocking(&self.library(), f).await
    }

    /// Delivers `msg` to the UI from any thread.
    pub fn post(&self, msg: impl Into<Msg>) {
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
    pub fn build_search(&self, generation: u64, albums: Arc<Vec<Album>>, artists: Arc<Vec<library::Artist>>, tracks: crate::tracks::Searchable) {
        let this = self.clone();
        self.rt.spawn_blocking(move || {
            let index = crate::search::SearchIndex::new(&albums, &artists, tracks.iter());
            this.send(LibraryMsg::SearchIndex { generation, index: Box::new(index) });
        });
    }

    /// Loads the cached library from disk without touching the network.
    /// Reads the stored library and the saved copies of the playlists,
    /// favorites and Recently added. With `keep` (working offline), only
    /// what can play: the rest isn't sent. Scoped to the session: once
    /// another account signs in, a read still running sends nothing.
    pub fn load_local(&self, keep: Option<crate::local::Playable>) {
        let this = self.scoped();
        self.rt.spawn_blocking(move || {
            if !this.current() {
                return;
            }
            let l = this.library();
            if let Ok(Some(mbids)) = l.saved::<std::collections::BTreeMap<String, String>>("artist_mbids_v1") {
                this.send(LibraryMsg::ArtistMbids(mbids.into_iter().collect()));
            }
            let mut saved = Saved {
                newest: l.saved(SAVED_NEWEST).ok().flatten(),
                playlists: l.saved::<Vec<SavedPlaylist>>(SAVED_PLAYLISTS).ok().flatten().map(|p| p.into_iter().map(Into::into).collect()),
                starred: l.saved(SAVED_STARRED).ok().flatten(),
            };
            let Some(keep) = keep else {
                match l.albums() {
                    Ok(a) => this.send(LibraryMsg::Albums(a)),
                    Err(e) => return this.send(Msg::Error(Failure::Local(format!("Couldn't read the library: {e}")))),
                }
                if let Ok(a) = l.artists() {
                    this.send(LibraryMsg::Artists(a));
                }
                this.send(LibraryMsg::Saved(Box::new(saved)));
                return this.load_all_tracks();
            };
            let read = || -> library::Result<_> { Ok((l.albums()?, l.artists()?, l.all_tracks()?)) };
            let (albums, artists, tracks) = match read() {
                Ok(r) => r,
                Err(e) => return this.send(Msg::Error(Failure::Local(format!("Couldn't read the library: {e}")))),
            };
            let kept = crate::local::Kept::new(&keep, albums, artists, tracks);
            if let Some(ids) = &mut saved.newest {
                kept.newest(ids);
            }
            if let Some(s) = &mut saved.starred {
                kept.starred(&keep, s);
            }
            // A playlist shows if any of its songs (as saved) can play.
            if let Some(p) = &mut saved.playlists {
                p.retain(|p| l.saved::<Vec<Track>>(&saved_playlist_name(&p.id)).ok().flatten().is_some_and(|t| t.iter().any(|t| keep.track(t))));
            }
            this.send(LibraryMsg::Albums(kept.albums));
            this.send(LibraryMsg::Artists(kept.artists));
            this.send(LibraryMsg::Saved(Box::new(saved)));
            this.send(LibraryMsg::AllTracks(Box::new(crate::tracks::TrackSet::new(kept.tracks))));
        });
    }

    /// Saves a copy of `value` for starting offline. A failure costs only
    /// the copy, so it's logged, not shown.
    async fn keep<T: serde::Serialize + Send + 'static>(&self, name: String, value: T) {
        let what = name.clone();
        if let Err(e) = self.db(move |l| l.save(&name, &value)).await {
            tracing::warn!("saving {what}: {e}");
        }
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
        if self.account_tag.is_some() {
            self.scoped().send(AnalysisMsg::Store(store.clone()));
        }
        let this = self.scoped();
        self.rt.spawn_blocking(move || match store.records() {
            Ok(rows) => {
                let records = rows.into_iter().filter_map(|(id, b)| analysis::Record::decode(&b).map(|r| (id, r))).collect();
                this.send(AnalysisMsg::Loaded(records));
            }
            Err(e) => tracing::warn!("reading analyses: {e}"),
        });
    }

    /// Blocking: reads every stored track and sends them to the UI.
    fn load_all_tracks(&self) {
        match self.library().all_tracks() {
            Ok(t) => self.send(LibraryMsg::AllTracks(Box::new(crate::tracks::TrackSet::new(t)))),
            Err(e) => self.send(Msg::Error(Failure::Local(format!("Couldn't read the library: {e}")))),
        }
    }

    /// Pings the server; on success stores the password in the keychain.
    ///
    /// With a saved login (`password` is `None`) an unreachable server isn't a
    /// login failure: the app opens offline and keeps retrying until it's back.
    pub fn connect(&self, settings: Settings, password: Option<String>) {
        if settings.primary().is_some_and(|e| e.kind == crate::settings::SourceKind::Local) {
            let folders = settings.primary().map(|e| e.scan_roots()).unwrap_or_default();
            self.connect_local(settings, folders);
            return;
        }
        let session = self.session();
        // Scoped: the keychain can hold this up behind a prompt for as long
        // as the user likes, and a sign-out and new sign-in may come first.
        let this = self.scoped_to(session);
        self.rt.spawn(async move {
            let saved = password.is_none();
            let failed = |error| Msg::ConnectFailed { session, error };
            let password = match password {
                Some(p) => p,
                None => {
                    let s = settings.clone();
                    match tokio::task::spawn_blocking(move || s.load_password()).await.ok().flatten() {
                        Some(p) => p,
                        None => return this.send(failed("No saved password. Sign in again.".into())),
                    }
                }
            };
            let client = match Client::new(settings.server(), Credentials::new(settings.username(), &password)) {
                Ok(c) => c,
                Err(e) => return this.send(failed(connect_error(&e))),
            };
            let pinged = sign_in_ping(&client).await;
            if !this.live(session) {
                return;
            }
            match pinged {
                Ok(server) => {
                    // The account itself is recorded by the app (in its preferences)
                    // when `Connected` arrives for a live session.
                    let s = settings.clone();
                    let me = this.clone();
                    let stored = tokio::task::spawn_blocking(move || {
                        // Checked again here, just before the write: the
                        // blocking pool may have queued it behind other work,
                        // and a signed-out account's password mustn't be kept.
                        if !me.live(session) {
                            return false;
                        }
                        match s.store_password(&password) {
                            Ok(()) => true,
                            Err(e) => {
                                tracing::warn!("keychain: {e}");
                                false
                            }
                        }
                    })
                    .await;
                    // A saved login's password came from the keychain: nothing to store.
                    let password_saved = saved || stored.unwrap_or(false);
                    this.connected(
                        session,
                        Arc::new(SubsonicSource::new(settings.account().expect("a configured sign-in has an account"), client)),
                        settings,
                        server,
                        password_saved,
                    )
                    .await;
                }
                Err(PingError::Unreachable(e)) if saved => {
                    this.send(Msg::Unreachable { session, username: settings.username().to_owned(), error: e });
                    this.reconnect(Arc::new(SubsonicSource::new(settings.account().expect("a configured sign-in has an account"), client)), settings, session);
                }
                Err(PingError::Unreachable(e) | PingError::Refused(e)) => this.send(failed(e)),
            }
        });
    }

    /// Prepares a local-folder source and announces it as the live session.
    pub fn connect_local(&self, settings: Settings, folders: Vec<scan::ScanRoot>) {
        let session = self.session();
        let this = self.scoped_to(session);
        self.rt.spawn(async move {
            let Some(account) = settings.account() else {
                return this.send(Msg::ConnectFailed { session, error: "Local source is missing an account id.".into() });
            };
            let data = this.libraries.lock().unwrap_or_else(|e| e.into_inner()).data.clone();
            let source: Src = Arc::new(crate::source::local::LocalSource::new(account, folders, data.as_deref()));
            // A folder that's away (a drive unplugged) still opens: its
            // stored library shows, and the scan says the folder is missing.
            this.connected(session, source, settings, "Local folders".into(), true).await;
        });
    }

    /// Prepares this account's mirror and reports the sign-in. Nothing happens once `session` has
    /// ended (the keychain write before this can wait on a prompt for a while).
    async fn connected(&self, session: u64, source: Src, settings: Settings, server: String, password_saved: bool) {
        let mut this = self.scoped_to(session);
        if !this.current() {
            return;
        }
        let stores = match this.prepare_storage(source.account()).await {
            Ok(stores) => stores,
            Err(e) => return this.send(Msg::ConnectFailed { session, error: format!("Couldn't open the library: {e}") }),
        };
        let switched = stores.is_some() && this.libraries.lock().unwrap_or_else(|e| e.into_inner()).account.is_some();
        if let Some(stores) = &stores {
            this.scoped_library = Some(stores.library.clone());
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
        this.send(Msg::Connected { session, source, stores, settings: Box::new(settings), server, new_account: switched || new_account, password_saved });
    }

    /// Times one ping (for the server popover and the diagnostics bar).
    pub fn measure_latency(&self, source: Src) {
        if !source.capabilities().network {
            self.send(Msg::Latency(None));
            return;
        }
        let this = self.scoped();
        self.rt.spawn(async move {
            let start = std::time::Instant::now();
            let ok = ping(&source).await.is_ok();
            this.send(Msg::Latency(ok.then(|| start.elapsed())));
        });
    }

    /// Checks the connection after something failed; if the server is gone,
    /// says so and reconnects in the background.
    pub fn probe(&self, source: Src, settings: Settings) {
        if !source.capabilities().network || self.reconnecting.load(Ordering::Relaxed) {
            return;
        }
        let session = self.session();
        let this = self.scoped_to(session);
        self.rt.spawn(async move {
            if let Err(PingError::Unreachable(e)) = ping(&source).await
                && this.live(session)
            {
                this.send(Msg::Unreachable { session, username: settings.username().to_owned(), error: e });
                this.reconnect(source, settings, session);
            }
        });
    }

    /// Has the reconnect loop, if one runs, try right away.
    pub fn retry_now(&self) {
        if self.reconnecting.load(Ordering::Relaxed) {
            self.retry_now.notify_one();
        }
    }

    /// Waits `delay` before the next ping, or less: when asked to retry now,
    /// or when the clock jumped (the computer slept, and the network may be
    /// back since). The monotonic clock stops during sleep, so a wait begun
    /// before it would otherwise run on for its full length after waking.
    async fn backoff(&self, delay: Duration) {
        let (start, wall) = (Instant::now(), std::time::SystemTime::now());
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        loop {
            tokio::select! {
                () = tokio::time::sleep_until((start + delay).into()) => return,
                () = self.retry_now.notified() => return,
                _ = tick.tick() => {
                    let slept = wall.elapsed().unwrap_or_default().saturating_sub(start.elapsed());
                    if slept > SLEPT {
                        return;
                    }
                }
            }
        }
    }

    /// Pings with backoff (5 s, doubling to half a minute) until the server
    /// answers, or until the user signs out (`session` ends).
    fn reconnect(&self, source: Src, settings: Settings, session: u64) {
        if !self.live(session) || self.reconnecting.swap(true, Ordering::Relaxed) {
            return;
        }
        let this = self.scoped_to(session);
        self.rt.spawn(async move {
            let mut delay = Duration::from_secs(5);
            loop {
                this.backoff(delay).await;
                if !this.live(session) {
                    // Signed out: `end_session` already cleared `reconnecting`
                    // (and a new loop may own it now).
                    return;
                }
                let pinged = ping(&source).await;
                if !this.live(session) {
                    return;
                }
                match pinged {
                    Ok(server) => {
                        this.reconnecting.store(false, Ordering::Relaxed);
                        return this.connected(session, source, settings, server, true).await;
                    }
                    Err(PingError::Unreachable(_)) => delay = (delay * 2).min(Duration::from_secs(30)),
                    Err(PingError::Refused(e)) => {
                        // Reachable but turning us away (password changed): back to the login form.
                        this.reconnecting.store(false, Ordering::Relaxed);
                        return this.send(Msg::ConnectFailed { session, error: e });
                    }
                }
            }
        });
    }

    /// Starts the appropriate library sync for `source`.
    pub fn sync_source(&self, source: &Src) {
        if let Some(paged) = source.paged() {
            self.sync_albums(paged);
        } else if source.local().is_some() {
            self.sync_local(source.clone());
        }
    }

    /// Requests an incremental rescan, starting this session's native folder
    /// watch on the first call. Requests during a scan coalesce into one more.
    pub fn sync_local(&self, source: Src) {
        if !self.current() || source.local().is_none() {
            return;
        }
        let this = self.scoped();
        let mut watch = self.local_watch.lock().unwrap_or_else(|e| e.into_inner());
        if watch.as_ref().is_some_and(|w| w.rescan(&source)) {
            return;
        }
        self.cancel_sync();
        *watch = Some(local_watch::LocalWatch::start(this, source));
    }

    /// One pass, awaited by the local scan queue before it accepts another.
    async fn scan_local(&self, source: Src) {
        let serial = self.local_scan.clone().lock_owned().await;
        if !self.current() {
            return;
        }
        let pass = self.album_pass.fetch_add(1, Ordering::AcqRel) + 1;
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(old) = self.sync_cancel.lock().unwrap_or_else(|e| e.into_inner()).replace(cancel.clone()) {
            old.store(true, Ordering::Relaxed);
        }
        let this = self.scoped();
        let work = self.rt.spawn_blocking(move || {
            let _serial = serial;
            let Some(local) = source.local() else { return };
            let live = || this.current() && this.album_pass.load(Ordering::Acquire) == pass && this.is_current(&cancel);
            this.send(LibraryMsg::SyncProgress(0));
            let mut last = Instant::now() - Duration::from_secs(1);
            let outcome = local.sync(&this.library(), &cancel, |p| {
                if (p.scanned == p.total || last.elapsed() >= Duration::from_millis(100)) && live() {
                    last = Instant::now();
                    this.send(LibraryMsg::TrackSync(sync::Progress { done: p.scanned, total: p.total, unit: sync::Unit::Tracks }));
                }
            });
            if !live() {
                return;
            }
            match outcome {
                Ok(synced) => {
                    tracing::debug!(
                        albums = synced.album_count,
                        tracks = synced.track_count,
                        reused = synced.reused,
                        rescanned = synced.rescanned,
                        skipped = synced.skipped,
                        missing = synced.missing_roots.len(),
                        moves = synced.moves.len(),
                        stamp = %synced.stamp,
                        "local scan complete"
                    );
                    if !synced.moves.is_empty()
                        && let Err(e) = this.ond_store.apply_moves(source.account(), &synced.moves)
                    {
                        tracing::warn!("migrating ond.redb entries across moved files: {e}");
                    }
                    this.send(LibraryMsg::FolderMissing(synced.missing_roots));
                    this.send(LibraryMsg::Skipped(synced.skipped));
                    this.send(LibraryMsg::Albums(synced.albums));
                    if let Ok(Some(mbids)) = this.library().saved::<std::collections::BTreeMap<String, String>>("artist_mbids_v1") {
                        this.send(LibraryMsg::ArtistMbids(mbids.into_iter().collect()));
                    }
                    this.send(LibraryMsg::Artists(synced.artists));
                    this.send(LibraryMsg::Newest(synced.newest_ids));
                    this.send(LibraryMsg::AlbumSyncDone(Ok(synced.album_count)));
                    this.load_all_tracks();
                    if !source.capabilities().stars || !source.capabilities().ratings {
                        match this.read_ond_starred(source.account()) {
                            Ok((starred, ratings)) => this.send(Msg::Starred { starred, ratings }),
                            Err(e) => tracing::warn!("reloading ond.redb stars after scan: {e}"),
                        }
                    }
                    if !source.capabilities().playlists {
                        match this.read_ond_playlists(source.account()) {
                            Ok(playlists) => this.send(LibraryMsg::Playlists(playlists)),
                            Err(e) => tracing::warn!("reloading ond.redb playlists after scan: {e}"),
                        }
                    }
                    this.send(LibraryMsg::TrackSyncDone(sync::Outcome::Done { fetched: synced.rescanned, bulk: false }));
                }
                Err(crate::source::local::LocalSyncError::Cancelled) => {
                    this.send(LibraryMsg::AlbumSyncDone(Ok(0)));
                    this.send(LibraryMsg::TrackSyncDone(sync::Outcome::Cancelled));
                }
                Err(crate::source::local::LocalSyncError::FolderMissing(paths)) => {
                    this.send(LibraryMsg::FolderMissing(paths));
                }
                Err(err) => {
                    this.send(LibraryMsg::AlbumSyncDone(Err(err.into())));
                }
            }
        });
        let _ = work.await;
    }

    /// Pages through every album on the server into the local db. Replaces a
    /// pass already running (e.g. from before the server went away): that one
    /// stores and reports nothing more.
    pub fn sync_albums<F: sync::Fetch + Clone>(&self, source: F) {
        let pass = self.album_pass.fetch_add(1, Ordering::AcqRel) + 1;
        let this = self.scoped();
        self.rt.spawn(async move {
            let live = || this.current() && this.album_pass.load(Ordering::Acquire) == pass;
            let progress = |n| {
                if live() {
                    this.send(LibraryMsg::SyncProgress(n));
                }
            };
            let listed = crate::sync::albums(&this.library(), &source, &live, progress).await;
            if !live() {
                return;
            }
            match listed {
                Ok(albums) => {
                    let n = albums.len();
                    this.send(LibraryMsg::Albums(albums));
                    this.send(LibraryMsg::AlbumSyncDone(Ok(n)));
                }
                Err(e) => return this.send(LibraryMsg::AlbumSyncDone(Err(e))),
            }
            let artists = crate::sync::artists(&this.library(), &source, &live).await;
            if !live() {
                return;
            }
            match artists {
                Ok(stored) => {
                    let mbids = source.artist_mbids();
                    let saved: std::collections::BTreeMap<_, _> = mbids.iter().cloned().collect();
                    this.keep("artist_mbids_v1".into(), saved).await;
                    this.send(LibraryMsg::ArtistMbids(mbids));
                    this.send(LibraryMsg::Artists(stored));
                }
                Err(e) => this.send(Msg::Error(e)),
            }
            this.sync_tracks(source);
        });
    }

    /// Fetches the tracks of every album that changed since the last sync, in
    /// the background. Replaces (cancels) a sync already running; from then on
    /// that one reports nothing (the new one does).
    pub fn sync_tracks<F: sync::Fetch + Clone>(&self, source: F) {
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
            // Whether the pass got anywhere before it was cancelled or failed.
            // Progress follows each page of songs stored and each album
            // answered, so a pass that never got that far stored nothing.
            // (Albums that only failed count too: `Outcome` doesn't say how
            // many were kept, and a needless reload is the safe way to be wrong.)
            let progressed = AtomicBool::new(false);
            let outcome = sync::run(
                this.library(),
                source,
                cancel.clone(),
                || this.current(),
                |p| {
                    progressed.store(true, Ordering::Relaxed);
                    let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
                    if (p.done == p.total || last.elapsed() >= Duration::from_millis(100)) && this.is_current(&cancel) {
                        *last = Instant::now();
                        this.send(LibraryMsg::TrackSync(p));
                    }
                },
            )
            .await;
            // Whether the library may have changed. A pass that stopped early
            // keeps what it fetched, but one that died before anything came
            // back stored nothing and isn't worth reloading every track for.
            let stored = match &outcome {
                sync::Outcome::Done { fetched, bulk } => *fetched > 0 || *bulk,
                sync::Outcome::Cancelled | sync::Outcome::Failed(_) => progressed.load(Ordering::Relaxed),
            };
            // A newer sync took over: it reports, and shows what this one
            // stored, for itself.
            if !this.is_current(&cancel) {
                if stored {
                    this.reload_owed.store(true, Ordering::Release);
                }
                return;
            }
            // Show what was stored, unless nothing changed.
            if this.reload_owed.swap(false, Ordering::AcqRel) || stored {
                let me = this.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    if let Ok(albums) = me.library().albums() {
                        me.send(LibraryMsg::Albums(albums));
                    }
                    me.load_all_tracks();
                })
                .await;
            }
            this.send(LibraryMsg::TrackSyncDone(outcome));
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
    pub fn open_album(&self, source: Option<Src>, album_id: String) {
        if let Some(account) = crate::source::identity::account(&album_id)
            && (self.account_tag.is_some_and(|a| a != account) || source.as_ref().is_some_and(|s| s.account() != account))
        {
            return self.send(Msg::Error(Failure::Local("This source is no longer available".into())));
        }
        let album_id = crate::source::identity::bare(&album_id).to_owned();
        let this = self.scoped();
        self.rt.spawn(async move {
            let id = album_id.clone();
            let key = ListKey::Album(album_id.clone());
            let mut cached = false;
            if let Ok(tracks) = this.db(move |l| l.album_tracks(&id)).await
                && !tracks.is_empty()
            {
                cached = true;
                this.send(LibraryMsg::Tracks { album_id: album_id.clone(), tracks, fresh: false });
            }
            let Some(source) = source else {
                if !cached {
                    this.send(LibraryMsg::LoadFailed { key, error: None });
                }
                return;
            };
            let (album, meta) = match source.album(&album_id).await {
                Ok(a) => a,
                Err(e) => return this.send(LibraryMsg::LoadFailed { key, error: Some(e.context("Couldn't load the album")) }),
            };
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
                        this.send(LibraryMsg::AlbumUpdated(Box::new(row)));
                    }
                    this.send(Msg::UserMeta { meta, tracks: tracks.clone() });
                    this.send(LibraryMsg::Tracks { album_id, tracks, fresh: true });
                }
                Err(e) => this.send(LibraryMsg::LoadFailed { key, error: Some(Failure::Local(format!("Couldn't save the album: {e}"))) }),
            }
        });
    }

    /// Recently added albums and the user's playlists; both small, fetched on connect.
    pub fn fetch_home(&self, source: Src) {
        let this = self.scoped();
        self.rt.spawn(async move {
            // Local scans populate Newest once the background scan completes.
            if source.local().is_none() {
                match source.newest(120).await {
                    Ok(albums) => {
                        let ids = albums.iter().map(|a| a.id.clone()).collect();
                        // Newest albums may predate the last full sync; announced
                        // only once they're stored, or they'd show as missing.
                        let saved = Vec::clone(&ids);
                        match this.db(move |l| l.upsert_albums(albums).and_then(|()| l.save(SAVED_NEWEST, &saved))).await {
                            Ok(()) => this.send(LibraryMsg::Newest(ids)),
                            Err(e) => this.send(Msg::Error(Failure::Local(format!("Couldn't save Recently added: {e}")))),
                        }
                    }
                    Err(e) => this.send(Msg::Error(e.context("Couldn't load Recently added"))),
                }
            }
            this.refresh_playlists(&source).await;
        });
    }

    /// Ond's cross-source store for hearts, ratings and playlists.
    pub fn ond_store(&self) -> &Arc<crate::store::OndStore> {
        &self.ond_store
    }

    pub fn open_playlist(&self, source: Option<Src>, id: String) {
        if let Some(account) = crate::source::identity::account(&id)
            && (self.account_tag.is_some_and(|a| a != account) || source.as_ref().is_some_and(|s| s.account() != account))
        {
            return self.send(Msg::Error(Failure::Local("This source is no longer available".into())));
        }
        let id = crate::source::identity::bare(&id).to_owned();
        let this = self.scoped();
        let Some(source) = source else {
            // Offline: the copy saved when it last loaded, if there is one.
            self.rt.spawn(async move {
                let name = saved_playlist_name(&id);
                match this.db(move |l| l.saved::<Vec<Track>>(&name)).await {
                    Ok(Some(tracks)) => this.send(LibraryMsg::PlaylistTracks { id, tracks, fresh: false }),
                    _ => this.send(LibraryMsg::LoadFailed { key: ListKey::Playlist(id), error: None }),
                }
            });
            return;
        };
        if !source.capabilities().playlists {
            self.rt.spawn_blocking(move || match this.read_ond_playlist_tracks(source.account(), &id) {
                Ok(Some(tracks)) => this.send(LibraryMsg::PlaylistTracks { id, tracks, fresh: true }),
                Ok(None) => this.send(LibraryMsg::LoadFailed { key: ListKey::Playlist(id), error: Some(Failure::Local("Playlist not found".into())) }),
                Err(e) => this.send(LibraryMsg::LoadFailed { key: ListKey::Playlist(id), error: Some(e.context("Couldn't load the playlist")) }),
            });
            return;
        }
        self.rt.spawn(async move {
            match source.playlist(&id).await {
                Ok((tracks, meta)) => {
                    let rows = tracks.clone();
                    if let Err(e) = this.db(move |l| l.store_tracks(rows)).await {
                        return this.send(Msg::Error(Failure::Local(e).context("Couldn't save the playlist")));
                    }
                    this.send(Msg::UserMeta { meta, tracks: tracks.clone() });
                    this.send(LibraryMsg::PlaylistTracks { id: id.clone(), tracks: tracks.clone(), fresh: true });
                    this.keep(saved_playlist_name(&id), tracks).await;
                }
                Err(e) => this.send(LibraryMsg::LoadFailed { key: ListKey::Playlist(id), error: Some(e.context("Couldn't load the playlist")) }),
            }
        });
    }

    /// Everything the user starred, and the ratings that come with it.
    pub fn fetch_starred(&self, source: Option<Src>) {
        let Some(source) = source else { return };
        let caps = source.capabilities();
        let this = self.scoped();
        if !caps.stars && !caps.ratings {
            self.rt.spawn_blocking(move || match this.read_ond_starred(source.account()) {
                Ok((starred, ratings)) => this.send(Msg::Starred { starred, ratings }),
                Err(e) => this.send(Msg::Error(e.context("Couldn't load favorites"))),
            });
            return;
        }
        self.rt.spawn(async move {
            let mut merged = match source.starred().await {
                Ok(pair) => pair,
                Err(e) => return this.send(Msg::Error(e.context("Couldn't load favorites"))),
            };
            if !caps.stars || !caps.ratings {
                let account = source.account();
                let me = this.clone();
                if let Ok((ond_starred, ond_ratings)) =
                    tokio::task::spawn_blocking(move || me.read_ond_starred(account)).await.unwrap_or_else(|e| Err(Failure::Local(e.to_string())))
                {
                    if !caps.stars {
                        merged.0 = ond_starred;
                    }
                    if !caps.ratings {
                        merged.1 = ond_ratings;
                    }
                }
            }
            let saved = SavedStarred { starred: merged.0.clone(), ratings: merged.1.clone() };
            this.send(Msg::Starred { starred: merged.0, ratings: merged.1 });
            this.keep(SAVED_STARRED.to_owned(), saved).await;
        });
    }

    fn read_ond_starred(&self, account: accounts::AccountId) -> Result<(Starred, Vec<(String, u8)>), Failure> {
        let raw = self.ond_store.starred(account).map_err(|e| Failure::Local(e.to_string()))?;
        let ratings = self.ond_store.ratings(account).map_err(|e| Failure::Local(e.to_string()))?;
        let lib = self.library();
        let songs = if raw.songs.is_empty() { Vec::new() } else { lib.tracks(&raw.songs).map_err(|e| Failure::Local(e.to_string()))? };
        let artist_names: HashMap<String, String> = if raw.artists.is_empty() {
            HashMap::new()
        } else {
            lib.artists().map_err(|e| Failure::Local(e.to_string()))?.into_iter().map(|a| (a.id, a.name)).collect()
        };
        let artists = raw
            .artists
            .into_iter()
            .map(|(id, stored_name)| {
                let name = artist_names.get(&id).cloned().or(stored_name).unwrap_or_else(|| id.clone());
                (id, name)
            })
            .collect();
        Ok((Starred { songs, albums: raw.albums, artists }, ratings))
    }

    fn read_ond_playlists(&self, account: accounts::AccountId) -> Result<Vec<Playlist>, Failure> {
        if self.account_tag.is_some() {
            return Ok(Vec::new());
        }
        let lists = self.ond_store.playlists().map_err(|e| Failure::Local(e.to_string()))?;
        let mut all_ids = Vec::new();
        for pl in &lists {
            for e in &pl.entries {
                if e.account == account {
                    all_ids.push(e.item.clone());
                }
            }
        }
        let by_id: HashMap<String, Track> = if all_ids.is_empty() {
            HashMap::new()
        } else {
            self.library().tracks(&all_ids).map_err(|e| Failure::Local(e.to_string()))?.into_iter().map(|t| (t.id.clone(), t)).collect()
        };
        Ok(lists
            .into_iter()
            .map(|pl| {
                let mut count = 0u32;
                let mut duration = 0u32;
                let mut cover_art = None;
                for e in pl.entries.iter().filter(|e| e.account == account) {
                    if let Some(t) = by_id.get(&e.item) {
                        count += 1;
                        duration += t.duration_secs.unwrap_or(0);
                        if cover_art.is_none() {
                            cover_art = t.cover_art.clone();
                        }
                    }
                }
                Playlist {
                    id: if self.account_tag.is_some() { format!("ond:{}", pl.id) } else { pl.id },
                    name: pl.name,
                    song_count: count,
                    duration,
                    cover_art,
                    owner: None,
                }
            })
            .collect())
    }

    fn read_ond_playlist_tracks(&self, account: accounts::AccountId, id: &str) -> Result<Option<Vec<Track>>, Failure> {
        let Some(pl) = self.ond_store.playlist(id).map_err(|e| Failure::Local(e.to_string()))? else {
            return Ok(None);
        };
        let ids: Vec<String> = pl.entries.into_iter().filter(|e| e.account == account).map(|e| e.item).collect();
        let tracks = self.library().tracks(&ids).map_err(|e| Failure::Local(e.to_string()))?;
        Ok(Some(tracks))
    }

    /// Lyrics for one track: structured (synced where possible) first, then
    /// plain `getLyrics` by artist and title. Failures read as "no lyrics":
    /// they're a nicety, never worth an error banner.
    pub fn fetch_lyrics(&self, source: Option<Src>, track_id: String, artist: Option<String>, title: String) {
        if let Some(account) = crate::source::identity::account(&track_id)
            && (self.account_tag.is_some_and(|a| a != account) || source.as_ref().is_some_and(|s| s.account() != account))
        {
            return self.send(Msg::Error(Failure::Local("This source is no longer available".into())));
        }
        let track_id = crate::source::identity::bare(&track_id).to_owned();
        let Some(source) = source else { return };
        let this = self.scoped();
        self.rt.spawn(async move {
            let lyrics = if source.capabilities().lyrics { source.lyrics(&track_id, artist.as_deref(), &title).await.ok().flatten() } else { None };
            this.send(Msg::Lyrics { track_id, lyrics });
        });
    }

    /// The playing song's play count and when it was last played, for the
    /// Info tab. Quietly nothing on failure.
    pub fn fetch_song_stats(&self, source: Option<Src>, track_id: String) {
        if let Some(account) = crate::source::identity::account(&track_id)
            && (self.account_tag.is_some_and(|a| a != account) || source.as_ref().is_some_and(|s| s.account() != account))
        {
            return self.send(Msg::Error(Failure::Local("This source is no longer available".into())));
        }
        let track_id = crate::source::identity::bare(&track_id).to_owned();
        let Some(source) = source else { return };
        let this = self.scoped();
        self.rt.spawn(async move {
            let stats = match source.song_stats(&track_id).await {
                Ok(s) => Some(s),
                Err(e) => {
                    tracing::debug!("getSong {track_id}: {e}");
                    None
                }
            };
            this.send(Msg::SongStats { track_id, stats });
        });
    }

    /// Loves or unloves; `seq` comes back in the reply so a stale one is recognised.
    pub fn set_starred(&self, source: Option<Src>, kind: StarKind, id: String, name: Option<String>, on: bool, seq: u64) {
        if let Some(account) = crate::source::identity::account(&id)
            && (self.account_tag.is_some_and(|a| a != account) || source.as_ref().is_some_and(|s| s.account() != account))
        {
            return self.send(Msg::Error(Failure::Local("This source is no longer available".into())));
        }
        let id = crate::source::identity::bare(&id).to_owned();
        let this = self.scoped();
        self.after_previous(format!("star:{kind:?}:{id}"), async move {
            let error = match &source {
                Some(c) if c.capabilities().stars => c.set_starred(kind, &id, on).await.err(),
                Some(c) => {
                    let key = accounts::ItemKey::new(c.account(), &id);
                    let store = Arc::clone(&this.ond_store);
                    let res = tokio::task::spawn_blocking(move || store.set_starred(kind, &key, on, name.as_deref()))
                        .await
                        .map_err(|e| Failure::Local(e.to_string()))
                        .and_then(|r| r.map_err(|e| Failure::Local(e.to_string())));
                    res.err()
                }
                None => Some(Failure::Local(NOT_CONNECTED.into())),
            };
            this.send(Msg::StarDone { kind, id, on, seq, error });
        });
    }

    pub fn set_rating(&self, source: Option<Src>, id: String, rating: u8, previous: u8, seq: u64) {
        if let Some(account) = crate::source::identity::account(&id)
            && (self.account_tag.is_some_and(|a| a != account) || source.as_ref().is_some_and(|s| s.account() != account))
        {
            return self.send(Msg::Error(Failure::Local("This source is no longer available".into())));
        }
        let id = crate::source::identity::bare(&id).to_owned();
        let this = self.scoped();
        self.after_previous(format!("rating:{id}"), async move {
            let error = match &source {
                Some(c) if c.capabilities().ratings => c.set_rating(&id, rating).await.err(),
                Some(c) => {
                    let key = accounts::ItemKey::new(c.account(), &id);
                    let store = Arc::clone(&this.ond_store);
                    let res = tokio::task::spawn_blocking(move || store.set_rating(&key, rating))
                        .await
                        .map_err(|e| Failure::Local(e.to_string()))
                        .and_then(|r| r.map_err(|e| Failure::Local(e.to_string())));
                    res.err()
                }
                None => Some(Failure::Local(NOT_CONNECTED.into())),
            };
            this.send(Msg::RatingDone { id, previous, seq, error });
        });
    }

    /// Reads the server's complete playlist before offering an Ond copy.
    /// No edit is sent to the server, even when the new songs are foreign.
    pub fn prepare_mixed_playlist(&self, source: Src, id: String, added: Vec<Track>) {
        let this = self.scoped();
        self.rt.spawn(async move {
            let raw = crate::source::identity::bare(&id).to_owned();
            let name = match source.playlists().await {
                Ok(ps) => ps.into_iter().find(|p| p.id == raw).map(|p| p.name).unwrap_or_else(|| "Playlist".into()),
                Err(e) => return this.send(Msg::Error(e.context("Couldn't copy the playlist"))),
            };
            match source.playlist(&raw).await {
                Ok((existing, _)) => {
                    let rows = existing.clone();
                    if let Err(e) = this.db(move |l| l.store_tracks(rows)).await {
                        return this.send(Msg::Error(Failure::Local(e).context("Couldn't copy the playlist")));
                    }
                    this.send(Msg::MixedPlaylistReady { id: raw, name, existing, added });
                }
                Err(e) => this.send(Msg::Error(e.context("Couldn't copy the playlist"))),
            }
        });
    }

    /// Makes a playlist of `song_ids`; the app opens it when it's there.
    pub fn create_playlist(&self, source: Option<Src>, name: String, song_ids: Vec<String>) {
        let Some(source) = source else { return self.send(Msg::Error(Failure::Local(NOT_CONNECTED.into()))) };
        if source.capabilities().playlists && song_ids.iter().any(|id| crate::source::identity::account(id).is_some_and(|a| a != source.account())) {
            return self.send(Msg::Error(Failure::Local("Songs from another source need an Ond playlist".into())));
        }
        let song_ids: Vec<_> =
            if source.capabilities().playlists { song_ids.iter().map(|id| crate::source::identity::bare(id).to_owned()).collect() } else { song_ids };
        let this = self.scoped();
        if !source.capabilities().playlists {
            self.rt.spawn_blocking(move || {
                let entries: Vec<accounts::ItemKey> = song_ids.iter().map(|id| crate::source::identity::key(source.account(), id)).collect();
                match this.ond_store.create_playlist(&name, &entries) {
                    Ok(created) => {
                        match this.read_ond_playlists(source.account()) {
                            Ok(p) => this.send(LibraryMsg::Playlists(p)),
                            Err(e) => this.send(Msg::Error(e.context("Couldn't load playlists"))),
                        }
                        this.send(Msg::PlaylistCreated {
                            id: if this.account_tag.is_some() { format!("ond:{}", created.id) } else { created.id },
                            name: created.name,
                        });
                    }
                    Err(e) => this.send(Msg::Error(Failure::Local(e.to_string()).context("Couldn't create the playlist"))),
                }
            });
            return;
        }
        self.rt.spawn(async move {
            match source.create_playlist(&name, &song_ids).await {
                Ok((id, name)) => {
                    this.refresh_playlists(&source).await;
                    this.send(Msg::PlaylistCreated { id, name });
                }
                Err(e) => this.send(Msg::Error(e.context("Couldn't create the playlist"))),
            }
        });
    }

    /// Sends one edit, then reloads the playlist list and the playlist (which
    /// also undoes the app's optimistic copy if the edit failed).
    pub fn edit_playlist(&self, source: Option<Src>, id: String, edit: PlaylistEdit) {
        let Some(source) = source else { return self.send(Msg::Error(Failure::Local(NOT_CONNECTED.into()))) };
        let id = crate::source::identity::bare(&id).to_owned();
        let edit = match edit {
            PlaylistEdit::Add(ids) | PlaylistEdit::Replace(ids)
                if source.capabilities().playlists && ids.iter().any(|id| crate::source::identity::account(id).is_some_and(|a| a != source.account())) =>
            {
                return self.send(Msg::Error(Failure::Local("Songs from another source need an Ond playlist".into())));
            }
            PlaylistEdit::Add(ids) if source.capabilities().playlists => {
                PlaylistEdit::Add(ids.iter().map(|id| crate::source::identity::bare(id).to_owned()).collect())
            }
            PlaylistEdit::Replace(ids) if source.capabilities().playlists => {
                PlaylistEdit::Replace(ids.iter().map(|id| crate::source::identity::bare(id).to_owned()).collect())
            }
            edit => edit,
        };
        let this = self.scoped();
        if !source.capabilities().playlists {
            self.rt.spawn_blocking(move || {
                let account = source.account();
                let res = match &edit {
                    PlaylistEdit::Rename(name) => this.ond_store.rename_playlist(&id, name).map(|_| ()),
                    PlaylistEdit::Add(ids) => {
                        let entries: Vec<_> = ids.iter().map(|tid| crate::source::identity::key(account, tid)).collect();
                        this.ond_store.add_to_playlist(&id, &entries).map(|_| ())
                    }
                    PlaylistEdit::Remove(indices) => this.ond_store.remove_from_playlist(&id, indices).map(|_| ()),
                    PlaylistEdit::Replace(ids) => {
                        let entries: Vec<_> = ids.iter().map(|tid| crate::source::identity::key(account, tid)).collect();
                        this.ond_store.replace_playlist_entries(&id, entries).map(|_| ())
                    }
                    PlaylistEdit::Delete => this.ond_store.delete_playlist(&id).map(|_| ()),
                };
                if let Err(e) = res {
                    this.send(Msg::Error(Failure::Local(e.to_string()).context("Couldn't change the playlist")));
                }
                match this.read_ond_playlists(account) {
                    Ok(p) => this.send(LibraryMsg::Playlists(p)),
                    Err(e) => this.send(Msg::Error(e.context("Couldn't load playlists"))),
                }
                if !matches!(edit, PlaylistEdit::Delete) {
                    match this.read_ond_playlist_tracks(account, &id) {
                        Ok(Some(tracks)) => this.send(LibraryMsg::PlaylistTracks { id, tracks, fresh: true }),
                        Ok(None) => this.send(LibraryMsg::LoadFailed { key: ListKey::Playlist(id), error: None }),
                        Err(e) => this.send(LibraryMsg::LoadFailed { key: ListKey::Playlist(id), error: Some(e.context("Couldn't load the playlist")) }),
                    }
                }
            });
            return;
        }
        self.rt.spawn(async move {
            let result = source.edit_playlist(&id, &edit).await;
            if let Err(e) = result {
                this.send(Msg::Error(e.context("Couldn't change the playlist")));
            }
            this.refresh_playlists(&source).await;
            if !matches!(edit, PlaylistEdit::Delete) {
                let id = this.account_tag.map_or_else(|| id.clone(), |a| accounts::ItemKey::new(a, &id).to_string());
                this.open_playlist(Some(source), id);
            }
        });
    }

    async fn refresh_playlists(&self, source: &Src) {
        if !source.capabilities().playlists {
            let me = self.clone();
            let account = source.account();
            match tokio::task::spawn_blocking(move || me.read_ond_playlists(account)).await {
                Ok(Ok(p)) => self.send(LibraryMsg::Playlists(p)),
                Ok(Err(e)) => self.send(Msg::Error(e.context("Couldn't load playlists"))),
                Err(e) => self.send(Msg::Error(Failure::Local(e.to_string()).context("Couldn't load playlists"))),
            }
            return;
        }
        match source.playlists().await {
            Ok(p) => {
                let saved: Vec<SavedPlaylist> = p.iter().map(SavedPlaylist::from).collect();
                self.send(LibraryMsg::Playlists(p));
                self.keep(SAVED_PLAYLISTS.to_owned(), saved).await;
            }
            Err(e) => self.send(Msg::Error(e.context("Couldn't load playlists"))),
        }
    }

    /// Exports an Ond playlist to an M3U file at `dest` (file paths for local tracks).
    pub fn export_playlist_m3u(&self, source: Option<Src>, id: String, dest: std::path::PathBuf) {
        let this = self.scoped();
        self.rt.spawn_blocking(move || {
            let lib = this.library();
            let result = (|| -> Result<(), Failure> {
                let mut track_ids = Vec::new();
                let pl = this.ond_store.playlist(&id).map_err(|e| Failure::Local(e.to_string()))?;
                if let Some(pl) = &pl {
                    for e in &pl.entries {
                        if source.as_ref().is_some_and(|s| s.account() == e.account) {
                            track_ids.push(e.item.clone());
                        }
                    }
                }
                let meta: HashMap<String, Track> = if track_ids.is_empty() {
                    HashMap::new()
                } else {
                    lib.tracks(&track_ids).map_err(|e| Failure::Local(e.to_string()))?.into_iter().map(|t| (t.id.clone(), t)).collect()
                };
                let body = this
                    .ond_store
                    .export_m3u(&id, |key| {
                        let path = source.as_ref().filter(|s| s.account() == key.account).and_then(|s| s.local())?.track_path(&key.item)?;
                        let tr = meta.get(&key.item);
                        Some(crate::store::M3uTrack {
                            location: path.to_string_lossy().into_owned(),
                            title: tr.map(|t| t.title.clone()),
                            artist: tr.and_then(|t| t.artist.clone()).filter(|s| !s.is_empty()),
                            duration_secs: tr.and_then(|t| t.duration_secs),
                        })
                    })
                    .map_err(|e| Failure::Local(e.to_string()))?
                    .ok_or_else(|| Failure::Local("Playlist not found".into()))?;
                std::fs::write(&dest, body).map_err(|e| Failure::Local(e.to_string()))
            })();
            this.send(Msg::M3uExported { path: dest, error: result.err() });
        });
    }

    /// Looks up which stored albums credit `name` (composer, performer, ...).
    pub fn find_credited(&self, name: String) {
        let this = self.scoped();
        self.rt.spawn_blocking(move || {
            if let Ok(album_ids) = this.library().albums_crediting(&name) {
                this.send(LibraryMsg::Credited { name, album_ids });
            }
        });
    }

    /// Resolves one qualified album for a cross-source queue request. Its
    /// source session protects both the mirror write and the eventual batch.
    pub async fn resolve_album(&self, source: Option<Src>, id: String) -> Vec<Track> {
        let this = self.scoped();
        let raw = crate::source::identity::bare(&id).to_owned();
        let stored_id = raw.clone();
        let (mut tracks, count) =
            this.db(move |l| Ok((l.album_tracks(&stored_id)?, l.album(&stored_id)?.map_or(0, |a| a.song_count)))).await.unwrap_or_default();
        if (tracks.is_empty() || tracks.len() < count as usize)
            && let Some(source) = source
        {
            match source.album(&raw).await {
                Ok((album, meta)) if this.current() => {
                    tracks = album.tracks.clone();
                    if this.db(move |l| l.upsert_album_tracks(album)).await.is_ok() {
                        this.send(Msg::UserMeta { meta, tracks: tracks.clone() });
                    }
                }
                Err(e) => this.send(Msg::Error(e.context("Couldn't load the album"))),
                _ => {}
            }
        }
        if !this.current() {
            return Vec::new();
        }
        if let Some(account) = this.account_tag {
            for t in &mut tracks {
                crate::source::identity::track(account, t);
            }
        }
        tracks
    }

    /// Gathers the tracks of `album_ids` in order: stored ones straight from the
    /// db, the rest from the server ([`sync::CONCURRENCY`] at a time, stored on
    /// the way). Offline, an album that isn't stored just isn't there to play.
    pub fn fetch_tracks<F: sync::Fetch + Clone>(&self, source: Option<F>, key: ListKey, album_ids: Vec<String>) {
        let this = self.scoped();
        self.rt.spawn(async move {
            let ids = album_ids.clone();
            let stored = this.db(move |l| ids.iter().map(|id| l.album_tracks(id)).collect::<library::Result<Vec<_>>>()).await;
            // Each album's tracks in `album_ids` order; `None` until known.
            let mut lists: Vec<Option<Vec<Track>>> = match stored {
                Ok(s) => s.into_iter().map(|t| (!t.is_empty()).then_some(t)).collect(),
                Err(_) => vec![None; album_ids.len()],
            };
            let mut missed = 0;
            let mut failed: Option<Failure> = None;
            if let Some(source) = source {
                let mut todo = album_ids.into_iter().enumerate().filter(|(i, _)| lists[*i].is_none()).collect::<Vec<_>>().into_iter();
                let mut running = tokio::task::JoinSet::new();
                loop {
                    while running.len() < sync::CONCURRENCY {
                        let Some((i, id)) = todo.next() else { break };
                        let (me, source) = (this.clone(), source.clone());
                        running.spawn(async move { (i, me.fetch_album(&source, id).await) });
                    }
                    let Some(joined) = running.join_next().await else { break };
                    match joined.map_err(|e| Failure::Local(e.to_string())).and_then(|(i, r)| r.map(|t| (i, t))) {
                        Ok((i, tracks)) => lists[i] = Some(tracks),
                        Err(e) => {
                            tracing::warn!("fetching an album's tracks: {e}");
                            missed += 1;
                            // One request that got no answer makes it the network's.
                            if failed.as_ref().is_none_or(|f| !f.is_network()) {
                                failed = Some(e);
                            }
                        }
                    }
                }
            }
            if let Some(failed) = failed {
                let albums = if missed == 1 { "1 album".to_owned() } else { format!("{missed} albums") };
                this.send(Msg::Error(failed.retold(format!("Couldn't load the songs of {albums}"))));
            }
            let tracks = lists.into_iter().flatten().flatten().collect();
            this.send(LibraryMsg::TrackBatch { key, tracks });
        });
    }

    /// One album's tracks from the server, stored, as the library has them.
    async fn fetch_album<F: sync::Fetch>(&self, source: &F, id: String) -> Result<Vec<Track>, Failure> {
        let album = source.album(id.clone()).await?;
        self.db(move |l| l.upsert_album_tracks(album).and_then(|_| l.album_tracks(&id))).await.map_err(Failure::Local)
    }
}

enum PingError {
    /// No answer: network down, server off, timeout, or a proxy's error page.
    Unreachable(String),
    /// The server answered and said no (bad credentials, unsupported).
    Refused(String),
}

/// Bounds probes even when a source's server takes the connection but never answers.
async fn ping(source: &Src) -> Result<String, PingError> {
    match tokio::time::timeout(Duration::from_secs(10), source.ping()).await {
        Err(_) => Err(PingError::Unreachable("The server didn't answer".into())),
        Ok(Err(e)) if e.is_network() => {
            tracing::debug!("ping: {e}");
            Err(PingError::Unreachable(e.into_message()))
        }
        Ok(Err(e)) => Err(PingError::Refused(e.into_message())),
        Ok(Ok(info)) => Ok(info),
    }
}

/// Pings with a timeout, so a black-holed server can't hang the connect.
async fn sign_in_ping(client: &Client) -> Result<String, PingError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::tests::{album, song};
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc::Receiver;
    use tokio::sync::Semaphore;

    #[test]
    fn signing_back_in_reuses_the_folder_and_another_account_replaces_it() {
        let dir = tempfile::tempdir().unwrap();
        let old = Settings::default().sign_in("https://music.example.com", "me");
        let account = old.account().unwrap();
        let stores = crate::storage::open(dir.path(), Some(account), &mut vec![]).unwrap();
        stores.library.upsert_albums(vec![album("kept", 1)]).unwrap();
        stores.analyses.put("song", b"old analysis").unwrap();
        let art_root = dir.path().join("art");
        let old_art = crate::art::account_dir(&art_root, account);
        std::fs::create_dir_all(&old_art).unwrap();
        std::fs::write(crate::art::cover_file(&old_art, "al-1", 64), b"old art").unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let ond_store = Arc::new(crate::store::OndStore::in_memory().unwrap());
        let bridge = Bridge::new(rt.handle().clone(), tx, Arc::new(|| {}), stores.library, ond_store);
        bridge.configure_storage(dir.path().to_owned(), art_root.clone(), Some(account), &stores.analyses);
        bridge.end_session();
        let signed_out = Settings { signed_out: true, ..old.clone() };
        let again = signed_out.sign_in("HTTPS://Music.Example.com/", "me");
        assert_eq!(again.account(), Some(account));
        let source = |account| {
            let mut fake = crate::source::fake::Fake::new(crate::source::Capabilities::none());
            fake.account = account;
            Arc::new(fake) as Src
        };
        rt.block_on(bridge.connected(bridge.session(), source(account), again, "test".into(), true));
        assert!(matches!(rx.try_recv().unwrap(), Msg::Connected { new_account: false, .. }));
        assert_eq!(rt.block_on(bridge.db(|l| l.albums())).unwrap()[0].id, "kept");
        assert!(crate::art::cover_file(&old_art, "al-1", 64).is_file());

        let next = signed_out.sign_in("https://music.example.com", "someone-else");
        let next_account = next.account().unwrap();
        assert_ne!(account, next_account);
        // This write passed its session check already. Its database must stay
        // the old one even though it finishes after the new mirror opens.
        let work = bridge.scoped();
        let (release, held) = std::sync::mpsc::channel();
        let (entered, waiting) = std::sync::mpsc::channel();
        let job = rt.spawn(async move {
            work.db(move |l| {
                entered.send(()).unwrap();
                held.recv().unwrap();
                l.upsert_albums(vec![album("late", 1)])
            })
            .await
            .unwrap();
        });
        waiting.recv_timeout(testkit::WAIT).unwrap();
        bridge.end_session();
        rt.block_on(bridge.connected(bridge.session(), source(next_account), next, "test".into(), true));
        let Msg::Connected { new_account: true, stores: Some(prepared), .. } = rx.try_recv().unwrap() else { panic!("expected a prepared new mirror") };
        bridge.end_session();
        let new_analysis = bridge.install_storage(next_account, prepared, &[]);
        assert!(new_analysis.records().unwrap().is_empty());
        release.send(()).unwrap();
        rt.block_on(job).unwrap();
        assert!(rt.block_on(bridge.db(|l| l.albums())).unwrap().is_empty());
        drop(stores.analyses);
        let old_folder = crate::storage::account_dir(dir.path(), account);
        testkit::wait_until("the previous mirror and art folders to be removed", || !old_folder.exists() && !old_art.exists());
        assert!(crate::storage::account_dir(dir.path(), next_account).join("library.redb").is_file());
    }

    #[test]
    fn the_reply_check_turns_away_subsonic_errors_and_sign_in_pages_but_not_audio() {
        let check = reply_check();
        let envelope = br#"{"subsonic-response":{"status":"failed","error":{"code":40,"message":"Wrong username or password"}}}"#;
        assert!(check.is_error(Some("application/json"), envelope));
        assert!(check.is_error(None, envelope), "sniffed from the body when the type says nothing");
        assert_eq!(check.message(envelope), "Wrong username or password");
        assert!(check.is_error(Some("text/html"), b"<!DOCTYPE html><html><head><title>Sign in</title></head></html>"));
        assert!(!check.is_error(Some("audio/flac"), b"fLaC\0\0\0\x22"));
    }

    /// A server whose requests can be held until the test lets them go.
    #[derive(Clone, Default)]
    struct Server {
        albums: Vec<Album>,
        /// Requests wait for a permit here (none: they answer at once).
        gate: Option<Arc<Semaphore>>,
        /// The album list fails (after the gate).
        list_fails: bool,
        /// This album answers last (after the gate).
        slow: Option<String>,
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

    #[async_trait::async_trait]
    impl sync::Fetch for Server {
        async fn album_page(&self, _: u32, offset: u32) -> Result<Vec<Album>, Failure> {
            self.wait().await;
            self.answered.fetch_add(1, Ordering::SeqCst);
            if self.list_fails {
                return Err(Failure::Network("server went away".into()));
            }
            Ok(if offset == 0 { self.albums.clone() } else { Vec::new() })
        }
        async fn artists(&self) -> Result<Vec<library::Artist>, Failure> {
            Ok(Vec::new())
        }
        async fn album(&self, id: String) -> Result<library::AlbumTracks, Failure> {
            self.wait().await;
            if self.slow.as_deref() == Some(id.as_str()) {
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
            let a = self.albums.iter().find(|a| a.id == id).cloned().ok_or(Failure::Server("not found".into()))?;
            let songs = (1..=a.song_count).map(|n| song(format!("{id}-{n}"), &id, n)).collect();
            self.answered.fetch_add(1, Ordering::SeqCst);
            Ok(library::AlbumTracks { album: a, tracks: songs })
        }
        async fn all_songs(&self, _: &'static str, _: u32, _: u32) -> Result<sync::SongPage, Failure> {
            Ok(Vec::new().into())
        }
    }

    /// One thread runs every task, and only while the test waits in `until`:
    /// when `until` sees a condition, every task has run as far as it can.
    fn rig(albums: &[Album]) -> (tokio::runtime::Runtime, Bridge, Receiver<Msg>) {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let lib = Library::in_memory().unwrap();
        lib.upsert_albums(albums.iter().cloned()).unwrap();
        let ond_store = Arc::new(crate::store::OndStore::in_memory().unwrap());
        let bridge = Bridge::new(rt.handle().clone(), tx, Arc::new(|| {}), Arc::new(lib), ond_store);
        (rt, bridge, rx)
    }

    fn unscoped(msg: Msg) -> Msg {
        match msg {
            Msg::Scoped { message, .. } => *message,
            msg => msg,
        }
    }

    /// Runs the bridge's tasks until `done`, collecting what they send.
    fn until(rt: &tokio::runtime::Runtime, rx: &Receiver<Msg>, got: &mut Vec<Msg>, mut done: impl FnMut(&[Msg]) -> bool) {
        rt.block_on(async {
            let deadline = testkit::Deadline::new();
            loop {
                got.extend(rx.try_iter().map(unscoped));
                if done(got) {
                    return;
                }
                assert!(!deadline.passed(), "timed out");
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
    }

    fn names(msgs: &[Msg]) -> Vec<&'static str> {
        msgs.iter()
            .map(|m| match m {
                Msg::Library(LibraryMsg::TrackSync(_)) => "TrackSync",
                Msg::Library(LibraryMsg::TrackSyncDone(_)) => "TrackSyncDone",
                Msg::Library(LibraryMsg::AllTracks(_)) => "AllTracks",
                Msg::Library(LibraryMsg::Albums(_)) => "Albums",
                Msg::Library(LibraryMsg::Artists(_)) => "Artists",
                Msg::Library(LibraryMsg::SyncProgress(_)) => "SyncProgress",
                Msg::Library(LibraryMsg::AlbumSyncDone(Ok(_))) => "AlbumSyncDone",
                Msg::Library(LibraryMsg::AlbumSyncDone(Err(_))) => "AlbumSyncFailed",
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
        let fresh = Server { albums, ..Default::default() };
        bridge.sync_tracks(fresh.clone());
        until(&rt, &rx, &mut got, |m| matches!(m.last(), Some(Msg::Library(LibraryMsg::TrackSyncDone(_)))));
        assert!(matches!(got.last(), Some(Msg::Library(LibraryMsg::TrackSyncDone(sync::Outcome::Done { fetched: 3, .. })))));
        got.clear();
        // The old one's answers come in now: it stores them, but says nothing,
        // and leaves the reload to the next sync.
        gate.add_permits(3);
        until(&rt, &rx, &mut got, |_| slow.answered.load(Ordering::SeqCst) == 3);
        // The next sync has nothing to fetch, yet shows what the old one
        // stored. One started before the old one has finished storing has
        // nothing to show, so sync until one does.
        let deadline = testkit::Deadline::new();
        loop {
            assert!(got.is_empty(), "the replaced sync sent {:?}", names(&got));
            bridge.sync_tracks(fresh.clone());
            until(&rt, &rx, &mut got, |m| matches!(m.last(), Some(Msg::Library(LibraryMsg::TrackSyncDone(_)))));
            if names(&got) == ["Albums", "AllTracks", "TrackSyncDone"] {
                break;
            }
            assert_eq!(names(&got), ["TrackSyncDone"], "nothing to fetch, nothing owed yet");
            assert!(!deadline.passed(), "no sync showed what the replaced one stored");
            got.clear();
        }
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
        until(&rt, &rx, &mut got, |m| matches!(m.last(), Some(Msg::Library(LibraryMsg::TrackSyncDone(_)))));
        assert!(names(&got).contains(&"AlbumSyncDone"));
        got.clear();
        // The old request finally fails: no "Sync failed" after reconnecting.
        gate.add_permits(1);
        // Nothing in that task awaits after the answer, so it ran to its end
        // in the same poll.
        until(&rt, &rx, &mut got, |_| old.answered.load(Ordering::SeqCst) == 1);
        assert!(got.is_empty(), "the replaced pass sent {:?}", names(&got));
    }

    #[test]
    fn work_for_a_session_that_ended_sends_nothing() {
        let (rt, bridge, rx) = rig(&[]);
        let scoped = bridge.scoped();
        scoped.send(Msg::Error(Failure::Local("before".into())));
        bridge.end_session();
        scoped.send(Msg::Error(Failure::Local("after".into())));
        // Copies of a copy keep the session they were made for.
        scoped.scoped().send(Msg::Error(Failure::Local("after, again".into())));
        bridge.scoped().send(Msg::Error(Failure::Local("the new session".into())));
        let texts: Vec<String> = rx.try_iter().map(unscoped).filter_map(|m| if let Msg::Error(e) = m { Some(e.into_message()) } else { None }).collect();
        assert_eq!(texts, ["before", "the new session"]);
        let stored = rt.block_on(scoped.db(|l| l.albums()));
        assert_eq!(stored.unwrap_err(), "Signed out", "nor reads or writes the library");
    }

    #[test]
    fn a_library_read_held_up_until_another_account_signs_in_sends_nothing() {
        // One blocking thread, so a job holding it holds up the read too.
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().max_blocking_threads(1).build().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let lib = Library::in_memory().unwrap();
        lib.upsert_albums(vec![album("old", 1)]).unwrap();
        let ond_store = Arc::new(crate::store::OndStore::in_memory().unwrap());
        let bridge = Bridge::new(rt.handle().clone(), tx, Arc::new(|| {}), Arc::new(lib), ond_store);
        let mut got = Vec::new();
        // Unhindered, the read sends the stored library.
        bridge.load_local(None);
        until(&rt, &rx, &mut got, |m| m.iter().any(|m| matches!(m, Msg::Library(LibraryMsg::AllTracks(_)))));
        assert!(names(&got).contains(&"Albums"));
        got.clear();

        let (release, held) = std::sync::mpsc::channel::<()>();
        bridge.offline_job(move |_| {
            let _ = held.recv();
        });
        bridge.load_local(None);
        // The saved login turned out to be another account: its session ends
        // (see `OndApp::connected`) while the read still waits.
        bridge.end_session();
        release.send(()).unwrap();
        // Queued behind the read, so once it's here the read has run.
        bridge.offline_job(|b| b.post(Msg::Latency(None)));
        until(&rt, &rx, &mut got, |m| m.iter().any(|m| matches!(m, Msg::Latency(_))));
        got.retain(|m| !matches!(m, Msg::Latency(_)));
        assert!(got.is_empty(), "the old account's library arrived: {:?}", names(&got));
    }

    #[test]
    fn an_artists_albums_are_fetched_together_and_kept_in_order() {
        let albums: Vec<_> = (0..4).map(|i| album(&format!("a{i}"), 2)).collect();
        let (rt, bridge, rx) = rig(&albums);
        // One is stored already; the other three come from the server.
        let stored = Server { albums: albums.clone(), ..Default::default() };
        let one = rt.block_on(sync::Fetch::album(&stored, "a2".into())).unwrap();
        bridge.library().upsert_album_tracks(one).unwrap();
        let gate = Arc::new(Semaphore::new(0));
        // The first album answers last.
        let server = Server { albums, gate: Some(gate.clone()), slow: Some("a0".into()), ..Default::default() };
        let key = ListKey::Artist("Artist".into());
        bridge.fetch_tracks(Some(server.clone()), key.clone(), vec!["a0".into(), "a1".into(), "a2".into(), "a3".into()]);
        let mut got = Vec::new();
        // All three requests are out before any answers.
        until(&rt, &rx, &mut got, |_| server.asked.load(Ordering::SeqCst) == 3);
        assert_eq!(server.answered.load(Ordering::SeqCst), 0);
        gate.add_permits(3);
        until(&rt, &rx, &mut got, |m| m.iter().any(|m| matches!(m, Msg::Library(LibraryMsg::TrackBatch { .. }))));
        let Some(Msg::Library(LibraryMsg::TrackBatch { key: k, tracks })) = got.pop() else { panic!("expected the batch last") };
        assert_eq!(k, key);
        let ids: Vec<&str> = tracks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["a0-1", "a0-2", "a1-1", "a1-2", "a2-1", "a2-2", "a3-1", "a3-2"], "in the order asked for");
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
            b.post(Msg::Error(Failure::Local("first".into())));
        });
        bridge.offline_job(|b| b.post(Msg::Error(Failure::Local("second".into()))));
        let mut got = Vec::new();
        rt.block_on(async { tokio::time::sleep(Duration::from_millis(20)).await });
        got.extend(rx.try_iter().map(unscoped));
        assert!(got.is_empty(), "the second waits for the first");
        gate_tx.send(()).unwrap();
        until(&rt, &rx, &mut got, |m| m.len() == 2);
        let texts: Vec<&str> = got.iter().map(|m| if let Msg::Error(e) = m { e.message() } else { "" }).collect();
        assert_eq!(texts, ["first", "second"]);
    }

    #[test]
    fn a_source_without_lyrics_is_not_asked_for_them() {
        use crate::source::{Capabilities, fake::Fake};
        let (rt, bridge, rx) = rig(&[]);
        let source = Arc::new(Fake::new(Capabilities::none()));
        bridge.fetch_lyrics(Some(source.clone()), "song".into(), Some("artist".into()), "title".into());
        let mut got = Vec::new();
        until(&rt, &rx, &mut got, |m| m.iter().any(|m| matches!(m, Msg::Lyrics { .. })));
        assert!(matches!(got.last(), Some(Msg::Lyrics { lyrics: None, .. })));
        assert_eq!(source.lyric_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn saved_playlists_keep_their_existing_bytes_and_round_trip_live_rows() {
        let playlist =
            Playlist { id: "id".into(), name: "name".into(), song_count: 128, duration: 300, cover_art: Some("cover".into()), owner: Some("user".into()) };
        let bytes = postcard::to_allocvec(&SavedPlaylist::from(&playlist)).unwrap();
        assert_eq!(bytes, b"\x02id\x04name\x80\x01\xac\x02\x01\x05cover\x01\x04user");
        let saved: SavedPlaylist = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(Playlist::from(saved), playlist);
    }

    #[test]
    fn connect_errors_read_plainly() {
        let wrong = subsonic::Error::Api { code: 40, message: "Wrong username or password".into() };
        assert_eq!(connect_error(&wrong), "Wrong username or password.");
        let no_token = subsonic::Error::Api { code: 41, message: "Token authentication not supported for LDAP users.".into() };
        assert_eq!(connect_error(&no_token), "This server doesn't allow the way Ond signs in.");
        let other = subsonic::Error::Api { code: 50, message: "Not allowed".into() };
        assert_eq!(connect_error(&other), "The server said: Not allowed");
        let url = subsonic::Client::new("not a url", subsonic::Credentials::new("a", "b")).err().unwrap();
        assert!(connect_error(&url).contains("server address"));
    }

    /// A local server answering with `status` and an HTML page titled
    /// `title`, as a reverse proxy does. Returns the base URL.
    fn proxy_page(status: &'static str, title: &'static str) -> String {
        let body = format!("<html><head><title>{title}</title></head><body>proxy</body></html>");
        let fake = testkit::JsonFake::start(move |_| testkit::Answer::new(status, body.clone()).with_header("Content-Type", "text/html"));
        fake.base().to_owned()
    }

    /// Signing in to a server behind a reverse proxy that answers with an
    /// error page shows what the page says, in the friendly sentence, and
    /// without repeating the status when that's all the page says.
    #[test]
    fn a_failed_sign_in_shows_what_the_server_said() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let sign_in = |base: String| {
            let client = Client::new(&base, Credentials::new("joe", "sesame")).unwrap();
            match rt.block_on(sign_in_ping(&client)) {
                Err(PingError::Unreachable(e)) => e,
                Err(PingError::Refused(e)) => panic!("refused: {e}"),
                Ok(s) => panic!("connected to {s}"),
            }
        };
        assert_eq!(
            sign_in(proxy_page("502 Bad Gateway", "Navidrome is restarting, try again in a minute")),
            "The server answered with an error (502 Bad Gateway): Navidrome is restarting, try again in a minute"
        );
        assert_eq!(sign_in(proxy_page("502 Bad Gateway", "502 Bad Gateway")), "The server answered with an error (502 Bad Gateway).");
        assert_eq!(sign_in(proxy_page("503 Service Unavailable", "Service Unavailable")), "The server answered with an error (503 Service Unavailable).");
    }

    /// Keep collecting through another debounce window: a rescan caused by
    /// the scan's own reads, or by a duplicated native event, is observable.
    fn watch_settles(rt: &tokio::runtime::Runtime, rx: &Receiver<Msg>, got: &mut Vec<Msg>) {
        let quiet = testkit::Deadline::within(Duration::from_secs(4));
        until(rt, rx, got, |_| quiet.passed());
    }

    fn scan_count(messages: &[Msg]) -> usize {
        messages.iter().filter(|m| matches!(m, Msg::Library(LibraryMsg::SyncProgress(0)))).count()
    }

    fn watched_folder(music: &std::path::Path) -> Src {
        Arc::new(crate::source::local::LocalSource::new(accounts::AccountId::new(), vec![scan::ScanRoot::new("music", music.to_owned())], None))
    }

    fn seed_folder(music: &std::path::Path) {
        std::fs::create_dir_all(music).unwrap();
        std::fs::write(music.join("seed.flac"), crate::test_tracks::flac(&[], None)).unwrap();
    }

    #[test]
    fn fifty_files_in_a_watched_folder_cause_one_incremental_rescan() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        seed_folder(&music);
        let (rt, bridge, rx) = rig(&[]);
        bridge.sync_source(&watched_folder(&music));
        let mut got = Vec::new();
        until(&rt, &rx, &mut got, |m| m.iter().any(|m| matches!(m, Msg::Library(LibraryMsg::AllTracks(t)) if t.all.len() == 1)));
        watch_settles(&rt, &rx, &mut got);
        assert_eq!(scan_count(&got), 1, "the startup scan must not trigger itself");
        got.clear();
        for n in 0..50 {
            std::fs::copy(music.join("seed.flac"), music.join(format!("song-{n}.flac"))).unwrap();
        }
        until(&rt, &rx, &mut got, |m| m.iter().any(|m| matches!(m, Msg::Library(LibraryMsg::AllTracks(t)) if t.all.len() == 51)));
        watch_settles(&rt, &rx, &mut got);
        assert_eq!(scan_count(&got), 1, "the whole burst is one scan");
    }

    #[test]
    fn replacing_a_root_during_the_debounce_keeps_its_subfolders_watched() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        seed_folder(&music);
        let (rt, bridge, rx) = rig(&[]);
        bridge.sync_source(&watched_folder(&music));
        let mut got = Vec::new();
        until(&rt, &rx, &mut got, |m| m.iter().any(|m| matches!(m, Msg::Library(LibraryMsg::AllTracks(t)) if t.all.len() == 1)));
        got.clear();
        std::fs::rename(&music, dir.path().join("old-music")).unwrap();
        seed_folder(&music);
        until(&rt, &rx, &mut got, |m| m.iter().any(|m| matches!(m, Msg::Library(LibraryMsg::AllTracks(t)) if t.all.len() == 1)));
        std::fs::create_dir(music.join("album")).unwrap();
        std::fs::copy(music.join("seed.flac"), music.join("album").join("next.flac")).unwrap();
        until(&rt, &rx, &mut got, |m| m.iter().any(|m| matches!(m, Msg::Library(LibraryMsg::AllTracks(t)) if t.all.len() == 2)));
    }

    #[test]
    fn rescan_requests_during_a_scan_coalesce_without_cancelling_it() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        seed_folder(&music);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let first = Arc::new(AtomicBool::new(true));
        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let notify: Notify = {
            let gate = gate.clone();
            Arc::new(move || {
                if first.swap(false, Ordering::Relaxed) {
                    let deadline = testkit::Deadline::new();
                    let mut open = gate.0.lock().unwrap();
                    while !*open {
                        assert!(!deadline.passed(), "scan was never released");
                        open = gate.1.wait_timeout(open, deadline.left()).unwrap().0;
                    }
                }
            })
        };
        let bridge =
            Bridge::new(rt.handle().clone(), tx, notify, Arc::new(Library::in_memory().unwrap()), Arc::new(crate::store::OndStore::in_memory().unwrap()));
        let source = watched_folder(&music);
        bridge.sync_source(&source);
        let mut got = Vec::new();
        until(&rt, &rx, &mut got, |m| scan_count(m) == 1);
        for _ in 0..50 {
            bridge.sync_source(&source);
        }
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        until(&rt, &rx, &mut got, |m| m.iter().filter(|m| matches!(m, Msg::Library(LibraryMsg::TrackSyncDone(_)))).count() == 2);
        watch_settles(&rt, &rx, &mut got);
        assert_eq!(scan_count(&got), 2, "one current scan and one follow-up");
        assert!(!got.iter().any(|m| matches!(m, Msg::Library(LibraryMsg::TrackSyncDone(sync::Outcome::Cancelled)))));
    }

    #[test]
    fn ending_a_local_session_stops_watching_its_folders() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        seed_folder(&music);
        let (rt, bridge, rx) = rig(&[]);
        bridge.sync_source(&watched_folder(&music));
        let mut got = Vec::new();
        until(&rt, &rx, &mut got, |m| m.iter().any(|m| matches!(m, Msg::Library(LibraryMsg::AllTracks(t)) if t.all.len() == 1)));
        bridge.end_session();
        got.clear();
        std::fs::copy(music.join("seed.flac"), music.join("new.flac")).unwrap();
        watch_settles(&rt, &rx, &mut got);
        assert_eq!(scan_count(&got), 0);
        assert_eq!(rt.block_on(bridge.db(|l| l.track_count())).unwrap(), 1);
    }

    #[test]
    fn an_initially_missing_folder_is_scanned_when_its_ancestors_return() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("drive").join("music");
        let (rt, bridge, rx) = rig(&[]);
        bridge.sync_source(&watched_folder(&music));
        let mut got = Vec::new();
        until(&rt, &rx, &mut got, |m| m.iter().any(|m| matches!(m, Msg::Library(LibraryMsg::FolderMissing(paths)) if !paths.is_empty())));
        seed_folder(&music);
        until(&rt, &rx, &mut got, |m| m.iter().any(|m| matches!(m, Msg::Library(LibraryMsg::AllTracks(t)) if t.all.len() == 1)));
        assert!(got.iter().any(|m| matches!(m, Msg::Library(LibraryMsg::FolderMissing(paths)) if paths.is_empty())));
        // Once the root returns, its recursive watch must cover new subfolders.
        std::fs::create_dir(music.join("album")).unwrap();
        std::fs::copy(music.join("seed.flac"), music.join("album").join("next.flac")).unwrap();
        until(&rt, &rx, &mut got, |m| m.iter().any(|m| matches!(m, Msg::Library(LibraryMsg::AllTracks(t)) if t.all.len() == 2)));
    }

    #[test]
    fn bridge_sync_local_populates_library_and_skips_network_or_stars() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        // Minimal valid 48-byte 44.1kHz stereo 16-bit PCM WAV file.
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&40u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&44_100u32.to_le_bytes());
        wav.extend_from_slice(&(44_100u32 * 4).to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&4u32.to_le_bytes());
        wav.extend_from_slice(&[0u8; 4]);
        std::fs::write(music.join("track.wav"), &wav).unwrap();

        let (rt, bridge, rx) = rig(&[]);
        let source: Src = Arc::new(crate::source::local::LocalSource::new(accounts::AccountId::new(), vec![scan::ScanRoot::new("music", music.clone())], None));
        bridge.sync_source(&source);
        bridge.measure_latency(source.clone());
        bridge.fetch_starred(Some(source.clone()));

        let mut got = Vec::new();
        until(&rt, &rx, &mut got, |m| {
            m.iter().any(|m| matches!(m, Msg::Library(LibraryMsg::TrackSyncDone(_))))
                && m.iter().any(|m| matches!(m, Msg::Latency(_)))
                && m.iter().any(|m| matches!(m, Msg::Starred { .. }))
        });
        assert!(got.iter().any(|m| matches!(m, Msg::Library(LibraryMsg::AllTracks(t)) if t.all.len() == 1)));
        assert!(got.iter().any(|m| matches!(m, Msg::Latency(None))));
        assert!(got.iter().any(|m| matches!(m, Msg::Starred { starred, .. } if starred.songs.is_empty())));
        assert!(!got.iter().any(|m| matches!(m, Msg::Error(_))));

        // Renaming the folder away keeps the stored album in library.redb and
        // reports FolderMissing.
        let away = dir.path().join("music-away");
        std::fs::rename(&music, &away).unwrap();
        got.clear();
        bridge.sync_source(&source);
        until(&rt, &rx, &mut got, |m| m.iter().any(|m| matches!(m, Msg::Library(LibraryMsg::FolderMissing(paths)) if paths == std::slice::from_ref(&music))));
        assert!(!got.iter().any(|m| matches!(m, Msg::Error(_) | Msg::Library(LibraryMsg::AlbumSyncDone(Err(_))))));
        assert_eq!(rt.block_on(bridge.db(|l| l.albums())).unwrap().len(), 1);
    }
}
