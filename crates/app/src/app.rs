//! The main window: sidebar · toolbar with the now-playing display · the
//! current view · the queue column. Views are in submodules; they don't change
//! state directly but push [`Action`]s, which are applied once the frame is
//! drawn. That keeps borrows simple and makes every view reusable by the
//! compact layout.

mod album_page;
mod browse;
mod commands;
mod compact;
mod offline_page;
mod dialogs;
mod favorites_page;
mod palette;
mod settings_page;
mod queue_panel;
mod sidebar;
mod songs;
mod toolbar;
mod widgets;

#[cfg(test)]
mod snapshots;

use crate::analyze;
use crate::art::Art;
use crate::bridge::{Bridge, Msg};
use crate::favorites::{Favorites, StarKind};
use crate::keys::Cmd;
use crate::nav::{Nav, Route};
use crate::now_playing::{self, Remote};
use crate::output::Outputs;
use crate::playback;
use crate::queue::{EndAction, Queue, Source};
use crate::search::SearchIndex;
use crate::settings::{Paths, Settings};
use crate::theme::{self, ThemePref, Tokens};
use crate::window::{Geometry, WindowMode, Windows};
use audio::{Command, EngineHandle, PlayState, PlaybackMode, TrackSource};
use library::{Album, Library, Track};
use offline::{OfflineStore, Pin, TrackRef};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;
use subsonic::Client;

pub use commands::PlaylistAction;
pub use dialogs::Overlay;
pub use palette::Palette;
pub(crate) use widgets::{elide as elide_text, link as link_text, segmented_wide};

/// Narrower windows show the queue as a sheet instead of a column.
const QUEUE_DOCK_MIN_W: f32 = 1100.0;
/// Narrower windows show the sidebar as a rail of icons.
const SIDEBAR_FULL_MIN_W: f32 = 820.0;

#[derive(Default)]
struct LoginForm {
    server: String,
    username: String,
    password: String,
    busy: bool,
    error: Option<String>,
}

/// Everything known about the library, as last loaded.
#[derive(Default)]
pub struct Data {
    pub albums: Vec<Album>,
    pub by_id: HashMap<String, usize>,
    /// Bumped whenever `albums` changes, so derived lists know to rebuild.
    pub version: u64,
    /// Recently added, newest first (album ids from the server).
    pub newest: Vec<String>,
    pub playlists: Vec<subsonic::Playlist>,
    /// Tracks of albums opened (or queued) so far.
    pub tracks: HashMap<String, Vec<Track>>,
    pub playlist_tracks: HashMap<String, Vec<Track>>,
    /// Album ids crediting a person, from stored tracks.
    pub credited: HashMap<String, Vec<String>>,
    /// The server's artists, as separate entities ("A, B & C" split).
    pub artists: Vec<library::Artist>,
    /// Every stored track, grouped by album; the Songs view sorts these.
    pub all_tracks: Arc<Vec<Track>>,
    /// Bumped whenever `all_tracks` changes.
    pub tracks_version: u64,
}

impl Data {
    pub fn album(&self, id: &str) -> Option<&Album> {
        self.by_id.get(id).map(|&i| &self.albums[i])
    }

    fn set_albums(&mut self, albums: Vec<Album>) {
        self.by_id = albums.iter().enumerate().map(|(i, a)| (a.id.clone(), i)).collect();
        self.albums = albums;
        self.version += 1;
    }

    fn set_all_tracks(&mut self, tracks: Vec<Track>) {
        let mut by_album: HashMap<String, Vec<Track>> = HashMap::new();
        for t in &tracks {
            by_album.entry(t.album_id.clone()).or_default().push(t.clone());
        }
        // Stored tracks are the server's as of the last sync; anything fetched
        // since (an album opened) is in the db too, so this replaces it all.
        self.tracks = by_album;
        self.all_tracks = Arc::new(tracks);
        self.tracks_version += 1;
    }

    fn update_album(&mut self, album: Album) {
        match self.by_id.get(&album.id) {
            Some(&i) => self.albums[i] = album,
            None => {
                self.by_id.insert(album.id.clone(), self.albums.len());
                self.albums.push(album);
            }
        }
        self.version += 1;
    }
}

/// What a queue action does with a set of tracks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QueueAction {
    /// Play now, starting at this track.
    Play { start: usize },
    Shuffle,
    PlayNext,
    Add,
}

/// A state change requested by a view; applied after the frame is drawn.
pub enum Action {
    Go(Route),
    Back,
    Forward,
    BackBy(usize),
    Album(String, QueueAction),
    Artist(String, QueueAction),
    Playlist(String, QueueAction),
    Tracks { tracks: Vec<Track>, source: Source, action: QueueAction },
    JumpTo(u64),
    Replay(u64),
    Remove(u64),
    Move(u64, usize),
    /// One step earlier (-1) or later (+1).
    MoveBy(u64, isize),
    ClearQueue,
    SetEnd(EndAction),
    PlayPause,
    Next,
    Previous,
    Seek(f64),
    OpenPalette,
    SetTheme(ThemePref),
    ToggleQueue,
    SetMode(WindowMode),
    /// Into Mini, or back to where Mini was entered from.
    ToggleMini,
    SetScrobble(bool),
    /// Open the Settings window at a section.
    OpenSettings(settings_page::Section),
    /// Re-read the library from the server (albums, then tracks).
    SyncNow,
    /// ReplayGain and streaming preferences.
    SetPlaybackPrefs(playback::PlaybackPrefs),
    /// Remember a device's mode (by UID); the device in use switches now.
    SetDeviceMode(String, PlaybackMode),
    /// Analyse these tracks (those without a current result) in the background.
    Analyze(Vec<Track>),
    AnalyzeLibrary,
    PauseAnalysis(bool),
    /// Stop and forget everything queued for analysis.
    StopAnalysis,
    SetAnalyzePlaying(bool),
    /// Back to the login screen; the saved password is forgotten.
    SignOut,
    /// Keep (true) or stop keeping something on this computer.
    KeepOffline(Keep, bool),
    /// Stop keeping a pin (from the Offline place, where tracks may not be loaded).
    Unpin(Pin),
    PauseDownloads(bool),
    RetryDownloads,
    /// Cap on offline storage in bytes; `None` is unlimited.
    SetOfflineLimit(Option<u64>),
    RevealOffline,
    /// Pick a new folder for downloads (they move at the next launch).
    MoveOffline,
    /// Love or unlove a song, album or artist. `track` / `name` keep the
    /// Favorites view current.
    ToggleStar { kind: StarKind, id: String, track: Option<Box<Track>>, name: Option<String> },
    /// Rate a song 1–5; 0 clears it.
    Rate(String, u8),
    /// A key or menu-bar command.
    Command(Cmd),
    /// Open a sheet (new playlist, rename, delete, shortcuts).
    Overlay(Overlay),
    PlaylistEdit(PlaylistAction),
}

/// What a download action applies to.
#[derive(Debug, Clone)]
pub enum Keep {
    Album(String),
    Playlist(String),
    Tracks(Vec<Track>),
}

/// Tracks being dragged (onto a sidebar playlist, or within a playlist to
/// reorder: then `from` says where they were).
#[derive(Debug, Clone)]
pub struct DragTracks {
    pub tracks: Vec<Track>,
    pub from: Option<(String, usize)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueueTab {
    #[default]
    UpNext,
    History,
}

/// Which side keyboard navigation applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    Main,
    Queue,
}

/// A queue action waiting for tracks to load.
struct Pending {
    key: String,
    source: Source,
    action: QueueAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlbumSort {
    #[default]
    Artist,
    Title,
    Year,
    Added,
}

#[derive(Default)]
pub struct QueueUi {
    pub tab: QueueTab,
    /// Per-group open/closed choices, by [`Source::key`].
    pub open: HashMap<String, bool>,
    pub selected: Option<u64>,
}

pub struct ListenerApp {
    // Kept alive for the app's lifetime; the bridge holds a handle to it.
    _rt: tokio::runtime::Runtime,
    paths: Arc<Paths>,
    bridge: Bridge,
    inbox: Receiver<Msg>,
    engine: EngineHandle,

    login: Option<LoginForm>,
    client: Option<Client>,
    server_label: String,
    sync_label: String,
    error: Option<String>,
    /// Something worth knowing that isn't an error (e.g. the library was rebuilt).
    notice: Option<String>,

    data: Data,
    nav: Nav,
    sort: AlbumSort,
    /// The album list for the current view, cached by (route, sort, data version).
    grid: Option<(Route, AlbumSort, u64, Vec<usize>)>,
    actions: Vec<Action>,
    pending: Vec<Pending>,
    art: Art,

    queue: Queue,
    /// The queue item last sent as `SetNext`, so the engine isn't told twice.
    sent_next: Option<u64>,
    show_queue: bool,
    /// Narrow windows: the queue opens as a sheet over the content instead.
    queue_sheet: bool,
    /// The window is too narrow to dock the queue column (set each frame).
    queue_narrow: bool,
    queue_ui: QueueUi,
    focus: Focus,
    /// Keyboard selection in the album/playlist track list.
    track_sel: Option<usize>,
    credits_open: bool,

    /// Output devices and what reaches the one in use.
    output: Outputs,
    bit_perfect: bool,
    volume: f32,
    /// Where the seek bar is being dragged to, in seconds.
    seek_preview: Option<f64>,
    /// A seek that's been sent but not confirmed yet; shown instead of the engine position.
    pending_seek: Option<f64>,

    /// Rebuilt whenever albums, artists or tracks change.
    search: Option<SearchIndex>,
    /// Albums, artists or tracks changed since the index was built.
    search_stale: bool,
    songs: songs::SongsView,
    /// A track sync is running (shows a stop button in the status bar).
    track_syncing: bool,
    palette: Option<Palette>,
    theme: ThemePref,

    /// Full / Compact / Mini and where each was.
    windows: Windows,
    /// The window as last seen, saved on quit.
    seen: Option<Geometry>,
    /// After a mode switch: the size asked for, and frames left to wait for it.
    /// The window reports its old size for a frame or two meanwhile.
    settling: Option<([f32; 2], u32)>,
    /// When the last scroll happened (frame clock), for showing scroll bars.
    last_scroll: f64,
    bars_shown: bool,
    /// OS Now Playing and server scrobbles.
    now_playing: now_playing::Session,
    scrobble: bool,

    /// Offline downloads, and their state as of this frame.
    offline: OfflineStore,
    offline_snap: Arc<offline::Snapshot>,
    /// Pins waiting for their track lists to load, by the same keys as `pending`.
    pending_pins: Vec<(String, Pin)>,
    /// The server answered last time we asked. While it's unreachable only
    /// downloaded tracks play.
    online: bool,
    /// The login in use, for reconnecting.
    settings: Option<Settings>,
    /// Stored analysis results by track id, with the file stamp they're for.
    analyses: HashMap<String, analysis::Record>,
    /// The stored results have been read (until then everything looks unanalysed).
    analyses_loaded: bool,
    analyzer: analyze::Runner,
    /// A library-wide analysis should be running (resumed after a restart).
    analysis_library: bool,
    /// Analyse each track as it plays (it's in the playback cache by then).
    analyze_playing: bool,
    /// The album page's analysis table is open.
    analysis_open: bool,
    /// Hearts and ratings, mirrored from the server.
    favorites: Favorites,
    /// A sheet over the window, if one is open.
    overlay: Option<Overlay>,
    /// The native menu bar (macOS); kept alive here.
    menu: Option<crate::menu::MenuBar>,
    /// ⌘L: select (and scroll to) this track once its album is shown.
    reveal: Option<String>,
    /// Playlists added to lately, first in "Add to Playlist".
    recent_playlists: Vec<String>,
    fav_tab: favorites_page::FavTab,
    /// The Settings window's section.
    settings_section: settings_page::Section,
    /// When the last full sync finished (this session).
    last_synced: Option<chrono::DateTime<chrono::Local>>,
    /// "navidrome 0.64.2", as the server reported it.
    server_info: String,
}

impl ListenerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        let mut app = Self::build(&cc.egui_ctx, Paths::resolve()?, audio::default_backend())?;
        // The app is running by now, so NSApp takes the menu bar.
        let bridge = app.bridge.clone();
        app.menu = crate::menu::MenuBar::install(move |cmd| bridge.post(Msg::Menu(cmd)));
        Ok(app)
    }

    fn build(ctx: &egui::Context, paths: Paths, backend: Box<dyn audio::OutputBackend>) -> anyhow::Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
        let paths = Arc::new(paths);
        // The db is only a mirror of the server, so an unreadable one is set aside
        // and rebuilt by the sync that runs on connect.
        let (library, moved_aside) = Library::open_or_recover(&paths.library_db)?;
        let notice = moved_aside.map(|aside| {
            tracing::warn!("library database was unreadable; moved to {} and starting fresh", aside.display());
            format!("Library database was unreadable and is being rebuilt (old copy: {})", aside.display())
        });
        let library = Arc::new(library);
        let (tx, inbox) = channel();
        let bridge = Bridge::new(rt.handle().clone(), tx, ctx.clone(), library.clone());

        let saved = Settings::load(&paths);
        let theme_pref = saved.as_ref().map(|s| s.theme).unwrap_or_default();
        theme::install(ctx, theme_pref);
        let mut cache_cfg = audio::CacheConfig::new(&paths.stream_cache);
        if let Some(mb) = saved.as_ref().and_then(|s| s.stream_cache_mb) {
            cache_cfg.max_bytes = mb * 1024 * 1024;
        }
        let cache = audio::StreamCache::new(cache_cfg, rt.handle().clone())?;
        let offline = {
            let dir = saved.as_ref().and_then(|s| s.offline_dir.clone()).unwrap_or_else(|| paths.offline.clone());
            // A new location chosen last time: bring the downloads along first.
            if let Some(from) = saved.as_ref().and_then(|s| s.offline_move_from.clone()) {
                match offline::move_store(&from, &dir) {
                    Ok(()) => Settings::update(&paths, |s| s.offline_move_from = None),
                    Err(e) => tracing::warn!("moving downloads from {}: {e}", from.display()),
                }
            }
            let limit = saved.as_ref().and_then(|s| s.offline_limit_gb).map(|gb| gb * 1_000_000_000);
            let repaint = ctx.clone();
            let store = OfflineStore::open(offline::OfflineConfig { dir, limit }, rt.handle().clone(), Arc::new(move || repaint.request_repaint()))?;
            // A track that finished streaming is linked in rather than downloaded again.
            let cached = cache.clone();
            store.set_cache_lookup(Arc::new(move |r: &TrackRef| cached.complete_path(&audio::CacheKey::new(r.id.clone(), r.suffix.clone()))));
            store
        };
        let playback_cache = cache.clone();
        // Analysis fetches uncached files here, so a library-wide run doesn't
        // evict what was just played.
        let mut scratch_cfg = audio::CacheConfig::new(paths.stream_cache.with_file_name("analysis"));
        scratch_cfg.max_bytes = 64 * 1024 * 1024;
        let scratch_cache = audio::StreamCache::new(scratch_cfg, rt.handle().clone())?;
        let engine = audio::spawn(audio::EngineConfig::default(), backend, Some(cache));
        let output = Outputs::restore(&engine, saved.as_ref());
        let scrobble = saved.as_ref().is_none_or(|s| s.scrobble);
        let remote_bridge = bridge.clone();
        let now_playing = now_playing::Session::start(now_playing::Config {
            status: engine.status.clone(),
            rt: rt.handle().clone(),
            art_dir: paths.art_cache.clone(),
            retry_file: paths.library_db.with_file_name("scrobbles.json"),
            scrobble,
            remote: Arc::new(move |r| remote_bridge.post(Msg::Remote(r))),
        });
        let tap = now_playing.clone();
        bridge.forward_engine_events(engine.events.clone(), move |ev| tap.engine_event(ev));
        bridge.load_cached_albums();
        bridge.load_analyses();
        let analyzer = {
            let (results, ticks) = (bridge.clone(), bridge.ctx().clone());
            analyze::Runner::start(analyze::Config {
                library,
                playback: playback_cache,
                scratch: scratch_cache,
                status: engine.status.clone(),
                on_result: Arc::new(move |track_id, record| results.post(Msg::Analyzed { track_id, record })),
                on_progress: Arc::new(move || ticks.request_repaint()),
            })
        };
        let art = Art::new(rt.handle().clone(), ctx.clone(), paths.art_cache.clone());

        let login = match &saved {
            Some(s) if s.signed_out => LoginForm { server: s.server.clone(), username: s.username.clone(), ..Default::default() },
            Some(s) => {
                bridge.connect(s.clone(), None, paths.clone());
                LoginForm { server: s.server.clone(), username: s.username.clone(), busy: true, ..Default::default() }
            }
            None => LoginForm { server: "http://localhost:4533".into(), ..Default::default() },
        };

        Ok(Self {
            _rt: rt,
            show_queue: saved.as_ref().is_none_or(|s| s.show_queue),
            paths,
            bridge,
            inbox,
            engine,
            login: Some(login),
            client: None,
            server_label: String::new(),
            sync_label: String::new(),
            error: None,
            notice,
            data: Data::default(),
            nav: Nav::default(),
            sort: AlbumSort::default(),
            grid: None,
            actions: Vec::new(),
            pending: Vec::new(),
            art,
            queue: Queue::default(),
            sent_next: None,
            queue_ui: QueueUi::default(),
            queue_sheet: false,
            queue_narrow: false,
            focus: Focus::default(),
            track_sel: None,
            credits_open: false,
            output,
            bit_perfect: false,
            volume: 1.0,
            seek_preview: None,
            pending_seek: None,
            search: None,
            search_stale: false,
            songs: songs::SongsView::default(),
            track_syncing: false,
            palette: None,
            theme: theme_pref,
            windows: saved.as_ref().map(|s| s.window.clone()).unwrap_or_default(),
            seen: None,
            settling: None,
            last_scroll: f64::NEG_INFINITY,
            bars_shown: false,
            now_playing,
            scrobble,
            offline_snap: offline.snapshot(),
            offline,
            pending_pins: Vec::new(),
            online: false,
            analysis_library: saved.as_ref().is_some_and(|s| s.analysis_resume),
            analyze_playing: saved.as_ref().is_some_and(|s| s.analyze_playing),
            settings: saved,
            analyses: HashMap::new(),
            analyses_loaded: false,
            analyzer,
            analysis_open: false,
            favorites: Favorites::default(),
            overlay: None,
            menu: None,
            reveal: None,
            recent_playlists: Vec::new(),
            fav_tab: favorites_page::FavTab::default(),
            settings_section: settings_page::Section::default(),
            last_synced: None,
            server_info: String::new(),
        })
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Connected { client, settings, server } => {
                self.server_label = format!("{} · {}", settings.username, server);
                self.server_info = server.clone();
                self.login = None;
                self.sync_label = "syncing…".into();
                self.bridge.sync_albums(client.clone());
                self.bridge.fetch_home(client.clone());
                self.art.set_client(client.clone());
                self.now_playing.set_client(client.clone());
                self.output.connected(&settings.server);
                let url = client.clone();
                self.offline.set_source(Some(Arc::new(move |id: &str| url.stream_url(id).to_string())));
                // Pinned playlists follow the server: refetch them so additions download.
                for p in &self.offline_snap.pins {
                    if let Pin::Playlist(id) = &p.pin {
                        self.bridge.open_playlist(Some(client.clone()), id.clone());
                    }
                }
                self.bridge.fetch_starred(Some(client.clone()));
                self.client = Some(client);
                self.settings = Some(*settings);
                if !self.online {
                    self.online = true;
                    // Lined up while offline; the server can supply the rest again.
                    self.sent_next = None;
                    self.sync_next();
                }
                self.resume_library_analysis();
            }
            Msg::ConnectFailed(e) => {
                self.set_offline();
                let form = self.login.get_or_insert_with(|| {
                    let s = self.settings.clone().unwrap_or_default();
                    LoginForm { server: s.server, username: s.username, ..Default::default() }
                });
                form.busy = false;
                form.error = Some(e);
            }
            Msg::Unreachable { username, error } => {
                tracing::info!("server unreachable: {error}");
                self.login = None;
                self.set_offline();
                self.server_label = format!("{username} · offline");
                self.sync_label = "reconnecting…".into();
            }
            Msg::Albums(a) => {
                self.data.set_albums(a);
                self.search_stale = true;
            }
            Msg::Artists(a) => {
                self.data.artists = a;
                self.search_stale = true;
            }
            Msg::AllTracks(t) => {
                self.data.set_all_tracks(t);
                self.search_stale = true;
                // A sync may have changed pinned albums' track lists.
                let albums: Vec<String> = self.offline_snap.pins.iter().filter_map(|p| if let Pin::Album(id) = &p.pin { Some(id.clone()) } else { None }).collect();
                for id in albums {
                    if let Some(tracks) = self.data.tracks.get(&id).cloned() {
                        self.refresh_pin(Pin::Album(id), &tracks);
                    }
                }
                self.resume_library_analysis();
            }
            Msg::Analyzed { track_id, record } => {
                self.analyses.insert(track_id, record);
            }
            Msg::AnalysesLoaded(rows) => {
                self.analyses.extend(rows);
                self.analyses_loaded = true;
                self.resume_library_analysis();
            }
            Msg::TrackSync(p) => {
                self.track_syncing = true;
                self.sync_label = p.label();
            }
            Msg::TrackSyncDone(outcome) => {
                self.track_syncing = false;
                let albums = self.data.albums.len();
                let tracks = self.data.all_tracks.len();
                self.sync_label = match outcome {
                    crate::sync::Outcome::Done { .. } => {
                        self.last_synced = Some(chrono::Local::now());
                        format!("{albums} albums · {tracks} tracks")
                    }
                    crate::sync::Outcome::Cancelled => format!("{albums} albums · {tracks} tracks · sync stopped"),
                    crate::sync::Outcome::Failed(e) => {
                        self.error = Some(e);
                        format!("{albums} albums · {tracks} tracks")
                    }
                };
            }
            Msg::AlbumUpdated(a) => self.data.update_album(a),
            Msg::SyncProgress(n) => self.sync_label = format!("syncing… {n}"),
            Msg::SyncDone(n) => {
                self.sync_label = format!("{n} albums");
                self.track_syncing = true;
            }
            Msg::Tracks { album_id, tracks } => {
                self.refresh_pin(Pin::Album(album_id.clone()), &tracks);
                self.data.tracks.insert(album_id.clone(), tracks);
                self.run_pending(&format!("album:{album_id}"));
            }
            Msg::Newest(ids) => {
                self.data.newest = ids;
                self.data.version += 1;
            }
            Msg::Playlists(p) => self.data.playlists = p,
            Msg::PlaylistTracks { id, tracks } => {
                self.refresh_pin(Pin::Playlist(id.clone()), &tracks);
                self.data.playlist_tracks.insert(id.clone(), tracks);
                self.run_pending(&format!("playlist:{id}"));
            }
            Msg::Credited { name, album_ids } => {
                self.data.credited.insert(name, album_ids);
            }
            Msg::TrackBatch { key, tracks } => {
                if let Some(at) = self.pending.iter().position(|p| p.key == key) {
                    let p = self.pending.remove(at);
                    self.queue_tracks(tracks, p.source, p.action);
                }
            }
            Msg::Engine(ev) => self.handle_engine(ev),
            Msg::Remote(r) => self.remote(r),
            Msg::Starred { starred, ratings } => self.favorites.load(starred, ratings),
            Msg::UserMeta { meta, tracks } => self.favorites.note(meta, &tracks),
            Msg::StarDone { kind, id, on, error } => self.star_done(kind, id, on, error),
            Msg::RatingDone { id, previous, error } => self.rating_done(id, previous, error),
            Msg::PlaylistCreated { id, name } => {
                self.recent_playlists.insert(0, id.clone());
                self.recent_playlists.truncate(3);
                self.go(Route::Playlist { id, name });
            }
            Msg::Menu(cmd) => self.command(cmd),
            Msg::Error(e) => {
                self.probe();
                // Offline, network errors are expected and the status bar already says so.
                if self.online {
                    self.error = Some(e);
                }
            }
        }
    }

    /// Something failed; if it was the network, find out whether the server is gone.
    fn probe(&self) {
        if self.online
            && let (Some(client), Some(settings)) = (&self.client, &self.settings)
        {
            self.bridge.probe(client.clone(), settings.clone());
        }
    }

    /// The server's gone: browse what's stored, play what's downloaded.
    fn set_offline(&mut self) {
        self.online = false;
        self.client = None;
        self.offline.set_source(None);
        self.track_syncing = false;
    }

    /// Pins `pin` if it was waiting for these tracks, or refreshes its track
    /// list if it's already pinned.
    fn refresh_pin(&mut self, pin: Pin, tracks: &[Track]) {
        let label = self.pin_label(&pin);
        let refs = || tracks.iter().map(track_ref).collect::<Vec<_>>();
        if let Some(at) = self.pending_pins.iter().position(|(_, p)| p == &pin) {
            self.pending_pins.remove(at);
            self.offline.pin(pin, label, refs());
        } else if self.offline_snap.pin(&pin).is_some() {
            self.offline.update(&pin, label, refs());
        }
    }

    fn pin_label(&self, pin: &Pin) -> String {
        match pin {
            Pin::Album(id) => self.data.album(id).map(|a| a.name.clone()).unwrap_or_default(),
            Pin::Playlist(id) => self.data.playlists.iter().find(|p| &p.id == id).map(|p| p.name.clone()).unwrap_or_default(),
            Pin::Track(id) => self.data.all_tracks.iter().find(|t| &t.id == id).map(|t| t.title.clone()).unwrap_or_default(),
        }
    }

    /// Whether `track` can play right now: downloaded, or the server is there.
    pub fn playable(&self, track: &Track) -> bool {
        self.online || self.offline_snap.is_done(&track.id)
    }

    fn keep_offline(&mut self, what: Keep, on: bool) {
        match what {
            Keep::Album(id) => {
                let pin = Pin::Album(id.clone());
                if !on {
                    return self.offline.unpin(&pin);
                }
                match self.data.tracks.get(&id).cloned() {
                    Some(tracks) => self.offline.pin(pin.clone(), self.pin_label(&pin), tracks.iter().map(track_ref).collect()),
                    None => {
                        self.pending_pins.push((format!("album:{id}"), pin));
                        self.bridge.open_album(self.client.clone(), id);
                    }
                }
            }
            Keep::Playlist(id) => {
                let pin = Pin::Playlist(id.clone());
                if !on {
                    return self.offline.unpin(&pin);
                }
                match self.data.playlist_tracks.get(&id).cloned() {
                    Some(tracks) => self.offline.pin(pin.clone(), self.pin_label(&pin), tracks.iter().map(track_ref).collect()),
                    None => {
                        self.pending_pins.push((format!("playlist:{id}"), pin));
                        self.bridge.open_playlist(self.client.clone(), id);
                    }
                }
            }
            Keep::Tracks(tracks) => {
                for t in tracks {
                    let pin = Pin::Track(t.id.clone());
                    if on {
                        self.offline.pin(pin, t.title.clone(), vec![track_ref(&t)]);
                    } else {
                        self.offline.unpin(&pin);
                    }
                }
            }
        }
        self.offline_snap = self.offline.snapshot();
    }

    /// A media key, Control Center or headphone command.
    fn remote(&mut self, r: Remote) {
        let state = self.engine.status.state();
        match r {
            Remote::Play if state == PlayState::Paused => self.engine.send(Command::Resume),
            Remote::Play if state == PlayState::Stopped => self.play_pause(),
            Remote::Pause if !matches!(state, PlayState::Paused | PlayState::Stopped) => self.engine.send(Command::Pause),
            Remote::Play | Remote::Pause => {}
            Remote::Toggle => self.play_pause(),
            Remote::Next => self.next(),
            Remote::Previous => self.previous(),
            Remote::Seek(to) => self.seek(to.max(0.0)),
            Remote::SkipBy(by) => {
                let end = self.duration_secs().unwrap_or(f64::MAX);
                self.seek((self.position_secs() + by).clamp(0.0, end));
            }
        }
    }

    /// Describes `track` for Now Playing before the engine reaches it.
    fn announce(&self, track: &Track) {
        let album = self.data.album(&track.album_id);
        self.now_playing.known(now_playing::Info {
            track_id: track.id.clone(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: album.map(|a| a.name.clone()),
            duration_secs: track.duration_secs.map(f64::from),
            cover: track.cover_art.clone().or_else(|| album.and_then(|a| a.cover_art.clone())),
        });
    }

    fn rebuild_search(&mut self) {
        let mut index = SearchIndex::new(&self.data.albums, &self.data.artists, &self.data.all_tracks);
        if let Some(p) = self.palette.as_mut() {
            p.refresh(&mut index);
        }
        self.search = Some(index);
    }

    fn handle_engine(&mut self, ev: audio::Event) {
        if let Some(mode) = self.output.handle(&ev) {
            // The engine picked the mode for a new device (remembered, or its default).
            self.bit_perfect = mode == PlaybackMode::BitPerfect;
        }
        match ev {
            audio::Event::TrackStarted { track_id, token, gapless, restart, .. } => {
                if self.analyze_playing
                    && !restart
                    && let Some(t) = self.playing_track(&track_id)
                {
                    self.enqueue_analysis(std::slice::from_ref(&t));
                }
                // Matched by queue item (the token), so the same song twice in a row
                // still moves the queue along. With "repeat queue" over a single
                // item, the next item *is* the current one: a gapless start of it
                // is the next pass.
                let is_next = self.queue.peek_next().is_some_and(|n| n.uid == token);
                let is_current = self.queue.current.as_ref().is_some_and(|c| c.uid == token);
                if is_next && (gapless || !is_current) {
                    // The engine moved on gaplessly; line up the track after this one.
                    self.queue.advance();
                    self.pending_seek = None;
                    self.sent_next = None;
                    self.sync_next();
                }
            }
            audio::Event::Seeked { .. } => self.pending_seek = None,
            // The engine ran out of queued tracks. If the queue has more (the next one
            // was named too late to play gaplessly), carry on from there.
            audio::Event::Ended { .. } => {
                if self.queue.advance().is_some() {
                    self.start_current();
                } else {
                    self.sent_next = None;
                }
            }
            audio::Event::Error { message, .. } => {
                self.pending_seek = None;
                self.error = Some(message);
                self.probe();
            }
            audio::Event::SignalPath(_)
            | audio::Event::Devices { .. }
            | audio::Event::ModeChanged { .. }
            | audio::Event::DeviceLost { .. }
            | audio::Event::DeviceReturned { .. } => {}
        }
    }

    /// What the engine needs to play `track`: the downloaded file first
    /// (instant, no network), else the server while it's reachable (original or
    /// transcoded), with its ReplayGain and whether it plays as part of its album
    /// (from its queue neighbours `prev` and `next`). `token` is the queue item's
    /// id, echoed back by the engine so two plays of one track stay apart.
    fn source(&self, token: u64, prev: Option<&Track>, track: &Track, next: Option<&Track>) -> Option<TrackSource> {
        let replay_gain = playback::replay_gain(track);
        let album_context = playback::in_album(prev, track, next);
        if let Some(path) = self.offline.local_path(&track.id) {
            return Some(TrackSource {
                id: track.id.clone(),
                url: format!("file://{}", path.display()),
                suffix: track.suffix.clone(),
                replay_gain,
                album_context,
                transcode: None,
                token,
            });
        }
        let client = self.client.as_ref().filter(|_| self.online)?;
        let transcode = self.output.transcode();
        let (url, suffix) = match &transcode {
            None => (client.stream_url(&track.id), track.suffix.clone()),
            Some(t) => (client.transcoded_stream_url(&track.id, &t.format, t.kbps), Some(t.format.clone())),
        };
        Some(TrackSource { id: track.id.clone(), url: url.to_string(), suffix, replay_gain, album_context, transcode, token })
    }

    /// Starts the queue's current item from the top (or stops when there is
    /// none). Tracks that can't play (offline, not downloaded) are skipped.
    fn start_current(&mut self) {
        self.pending_seek = None;
        self.sent_next = None;
        let mut skipped = 0;
        // With "repeat queue" the queue never runs out, so give up after one
        // full pass in which nothing could play.
        let limit = self.queue.pass_len().max(1);
        loop {
            let Some(item) = &self.queue.current else {
                self.engine.send(Command::Stop);
                break;
            };
            if skipped >= limit {
                self.engine.send(Command::Stop);
                self.notice = Some("Nothing in the queue is downloaded".to_owned());
                return;
            }
            let prev = self.queue.history.last().map(|i| &i.track);
            let next = self.queue.peek_next().map(|i| &i.track);
            if let Some(src) = self.source(item.uid, prev, &item.track, next) {
                self.announce(&item.track);
                self.engine.send(Command::Play(src));
                self.sync_next();
                break;
            }
            skipped += 1;
            if self.queue.advance().is_none() {
                self.engine.send(Command::Stop);
                break;
            }
        }
        if skipped > 0 {
            self.notice = Some(if skipped == 1 { "Skipped 1 song that isn't downloaded".to_owned() } else { format!("Skipped {skipped} songs that aren't downloaded") });
        }
    }

    /// Tells the engine what follows the current track (prefetch + gapless), if
    /// that changed. A next track that can't play isn't named; when the current
    /// one ends, `start_current` skips past it.
    fn sync_next(&mut self) {
        if self.queue.current.is_none() {
            return;
        }
        let next = self.queue.peek_next().map(|i| (i.uid, i.track.clone()));
        let current = self.queue.current.as_ref().map(|i| &i.track);
        let after = self.queue.upcoming.get(1).map(|i| &i.track);
        let source = next.as_ref().and_then(|(uid, t)| self.source(*uid, current, t, after));
        // By queue item, not track: the same song queued twice is two items.
        let sent = source.as_ref().map(|s| s.token);
        if sent != self.sent_next {
            if let Some((_, t)) = next.as_ref().filter(|_| source.is_some()) {
                self.announce(t);
            }
            self.engine.send(Command::SetNext(source));
            self.sent_next = sent;
        }
    }

    fn position_secs(&self) -> f64 {
        self.pending_seek.unwrap_or_else(|| self.engine.status.position_secs())
    }

    fn duration_secs(&self) -> Option<f64> {
        self.engine
            .status
            .duration_secs()
            .or_else(|| self.queue.current.as_ref().and_then(|c| c.track.duration_secs).map(f64::from))
    }

    fn play_pause(&mut self) {
        match self.engine.status.state() {
            PlayState::Playing | PlayState::Buffering | PlayState::Loading => self.engine.send(Command::Pause),
            PlayState::Paused => self.engine.send(Command::Resume),
            PlayState::Stopped => {
                if self.queue.current.is_some() || self.queue.advance().is_some() {
                    self.start_current();
                }
            }
        }
    }

    fn next(&mut self) {
        if self.queue.advance().is_some() {
            self.start_current();
        } else {
            self.engine.send(Command::Stop);
            self.sent_next = None;
        }
    }

    /// Restarts the track when it's past the first few seconds, else goes back one.
    fn previous(&mut self) {
        if self.position_secs() > 3.0 || self.queue.history.is_empty() {
            self.seek(0.0);
        } else if self.queue.back().is_some() {
            self.start_current();
        }
    }

    fn seek(&mut self, to: f64) {
        if self.queue.current.is_none() {
            return;
        }
        self.pending_seek = Some(to);
        self.engine.send(Command::Seek(Duration::from_secs_f64(to)));
    }

    /// Applies a queue action once the tracks are known.
    fn queue_tracks(&mut self, tracks: Vec<Track>, source: Source, action: QueueAction) {
        if tracks.is_empty() {
            return;
        }
        match action {
            QueueAction::Play { start } => {
                self.queue.play_now(tracks, start, source);
                self.start_current();
            }
            QueueAction::Shuffle => {
                let mut tracks = tracks;
                shuffle(&mut tracks);
                self.queue.play_now(tracks, 0, source);
                self.start_current();
            }
            QueueAction::PlayNext => {
                self.queue.play_next(tracks);
                self.sync_next();
            }
            QueueAction::Add => {
                self.queue.add(tracks);
                self.sync_next();
            }
        }
    }

    fn run_pending(&mut self, key: &str) {
        let Some(at) = self.pending.iter().position(|p| p.key == key) else { return };
        let p = self.pending.remove(at);
        let tracks = match &p.source {
            Source::Album { id, .. } => self.data.tracks.get(id).cloned(),
            Source::Playlist { id, .. } => self.data.playlist_tracks.get(id).cloned(),
            _ => None,
        };
        if let Some(t) = tracks {
            self.queue_tracks(t, p.source, p.action);
        }
    }

    fn apply(&mut self, action: Action) {
        // Browsing and the palette live in the full window.
        if matches!(action, Action::Go(_) | Action::OpenPalette) {
            self.set_mode(Some(WindowMode::Full));
        }
        match action {
            Action::Go(route) => self.go(route),
            Action::Back => {
                self.nav.back();
                self.on_arrive();
            }
            Action::Forward => {
                self.nav.forward();
                self.on_arrive();
            }
            Action::BackBy(n) => {
                self.nav.back_by(n);
                self.on_arrive();
            }
            Action::Album(id, action) => {
                let name = self.data.album(&id).map(|a| a.name.clone()).unwrap_or_default();
                let source = Source::Album { id: id.clone(), name };
                match self.data.tracks.get(&id) {
                    Some(t) => self.queue_tracks(t.clone(), source, action),
                    None => {
                        self.pending.push(Pending { key: format!("album:{id}"), source, action });
                        self.bridge.open_album(self.client.clone(), id);
                    }
                }
            }
            Action::Artist(name, action) => {
                let ids: Vec<String> = self.data.albums.iter().filter(|a| a.has_artist(None, &name)).map(|a| a.id.clone()).collect();
                let key = format!("artist:{name}");
                let source = match action {
                    QueueAction::PlayNext | QueueAction::Add => Source::Added,
                    _ => Source::ArtistShuffle { name: format!("{name}, shuffled") },
                };
                // Playing an artist means all of their albums, shuffled.
                let action = if matches!(action, QueueAction::Play { .. }) { QueueAction::Shuffle } else { action };
                self.pending.push(Pending { key: key.clone(), source, action });
                self.bridge.fetch_tracks(self.client.clone(), key, ids);
            }
            Action::Playlist(id, action) => {
                let name = self.data.playlists.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_default();
                let source = Source::Playlist { id: id.clone(), name };
                match self.data.playlist_tracks.get(&id) {
                    Some(t) => self.queue_tracks(t.clone(), source, action),
                    None => {
                        self.pending.push(Pending { key: format!("playlist:{id}"), source, action });
                        self.bridge.open_playlist(self.client.clone(), id);
                    }
                }
            }
            Action::Tracks { tracks, source, action } => self.queue_tracks(tracks, source, action),
            Action::JumpTo(uid) => {
                if self.queue.jump_to(uid) {
                    self.start_current();
                }
            }
            Action::Replay(uid) => {
                if self.queue.replay(uid) {
                    self.start_current();
                }
            }
            Action::Remove(uid) => {
                if self.queue_ui.selected == Some(uid) {
                    // Keep a selection for repeated ⌫.
                    let at = self.queue.position(uid).unwrap_or(0);
                    self.queue.remove(uid);
                    self.queue_ui.selected = self.queue.upcoming.get(at).or(self.queue.upcoming.last()).map(|i| i.uid);
                } else {
                    self.queue.remove(uid);
                }
                self.sync_next();
            }
            Action::Move(uid, to) => {
                self.queue.move_to(uid, to);
                self.sync_next();
            }
            Action::MoveBy(uid, delta) => {
                self.queue.move_by(uid, delta);
                self.sync_next();
            }
            Action::ClearQueue => {
                self.queue.clear();
                self.sync_next();
            }
            Action::SetEnd(end) => {
                self.queue.end = end;
                self.sync_next();
            }
            Action::PlayPause => self.play_pause(),
            Action::Next => self.next(),
            Action::Previous => self.previous(),
            Action::Seek(to) => self.seek(to),
            Action::OpenPalette => {
                if self.palette.is_none() && self.search.is_some() {
                    self.palette = Some(Palette::default());
                }
            }
            Action::SetTheme(pref) => {
                self.theme = pref;
                theme::set_pref(self.bridge.ctx(), pref);
                Settings::update(&self.paths, |s| s.theme = pref);
            }
            Action::ToggleQueue if self.queue_narrow => self.queue_sheet = !self.queue_sheet,
            Action::ToggleQueue => {
                self.show_queue = !self.show_queue;
                let show = self.show_queue;
                Settings::update(&self.paths, |s| s.show_queue = show);
            }
            Action::SignOut => self.sign_out(),
            Action::SetMode(mode) => self.set_mode(Some(mode)),
            Action::ToggleMini => self.set_mode(None),
            Action::OpenSettings(section) => {
                self.settings_section = section;
                self.set_mode(Some(WindowMode::Full));
                self.overlay = Some(Overlay::Settings);
            }
            Action::SyncNow => {
                if let Some(client) = self.client.clone()
                    && self.online
                    && !self.track_syncing
                {
                    self.sync_label = "syncing…".into();
                    self.bridge.sync_albums(client);
                }
            }
            Action::SetPlaybackPrefs(prefs) => self.output.set_prefs(prefs, &self.engine, &self.paths),
            Action::SetDeviceMode(uid, mode) => {
                if self.output.set_device_mode(&uid, mode, &self.engine, &self.paths) {
                    self.bit_perfect = mode == PlaybackMode::BitPerfect;
                }
            }
            Action::SetScrobble(on) => {
                self.scrobble = on;
                self.now_playing.set_scrobbling(on);
                Settings::update(&self.paths, |s| s.scrobble = on);
            }
            Action::KeepOffline(what, on) => self.keep_offline(what, on),
            Action::Unpin(pin) => {
                self.offline.unpin(&pin);
                self.offline_snap = self.offline.snapshot();
            }
            Action::PauseDownloads(paused) => self.offline.set_paused(paused),
            Action::RetryDownloads => self.offline.retry_failed(),
            Action::SetOfflineLimit(bytes) => {
                self.offline.set_limit(bytes);
                Settings::update(&self.paths, |s| s.offline_limit_gb = bytes.map(|b| b / 1_000_000_000));
            }
            Action::RevealOffline => {
                let dir = self.offline.dir().to_owned();
                if let Err(e) = std::process::Command::new("open").arg(&dir).spawn() {
                    self.error = Some(format!("open {}: {e}", dir.display()));
                }
            }
            Action::MoveOffline => {
                let current = self.offline.dir().to_owned();
                let start = current.parent().unwrap_or(&current).to_owned();
                if let Some(chosen) = crate::folder::pick("Choose where to keep downloaded music", &start) {
                    // Always a folder of our own inside the chosen one: the store
                    // tidies files it doesn't know, which must never be the user's.
                    let dir = if chosen.file_name().is_some_and(|n| n == "Listener Offline") { chosen } else { chosen.join("Listener Offline") };
                    if dir != current {
                        Settings::update(&self.paths, |s| {
                            s.offline_dir = Some(dir.clone());
                            s.offline_move_from = Some(current.clone());
                        });
                        self.notice = Some(format!("Downloads will move to {} when Listener restarts", dir.display()));
                    }
                }
            }
            Action::Analyze(tracks) => {
                self.analyzer.set_paused(false);
                self.enqueue_analysis(&tracks);
            }
            Action::AnalyzeLibrary => {
                self.analysis_library = true;
                Settings::update(&self.paths, |s| s.analysis_resume = true);
                self.analyzer.set_paused(false);
                self.resume_library_analysis();
            }
            Action::PauseAnalysis(paused) => self.analyzer.set_paused(paused),
            Action::StopAnalysis => {
                self.analyzer.clear();
                self.analyzer.set_paused(false);
                self.finish_library_analysis();
            }
            Action::SetAnalyzePlaying(on) => {
                self.analyze_playing = on;
                Settings::update(&self.paths, |s| s.analyze_playing = on);
            }
            Action::ToggleStar { kind, id, track, name } => self.toggle_star(kind, id, track.map(|t| *t), name),
            Action::Rate(id, rating) => self.rate(id, rating),
            Action::Command(cmd) => self.command(cmd),
            Action::Overlay(o) => self.overlay = Some(o),
            Action::PlaylistEdit(edit) => self.playlist_action(edit),
        }
    }

    /// The queue entry the engine just started: the current one, or the next (gapless).
    fn playing_track(&self, id: &str) -> Option<Track> {
        [self.queue.current.as_ref(), self.queue.peek_next()].into_iter().flatten().find(|i| i.track.id == id).map(|i| i.track.clone())
    }

    /// A track's analysis, if there is one for the file as it is now.
    pub fn analysis_of(&self, t: &Track) -> Option<&analysis::TrackAnalysis> {
        self.analyses.get(&t.id).filter(|r| r.stamp == analyze::stamp_of(t)).map(|r| &r.analysis)
    }

    /// Queues the tracks that have no current analysis; returns how many.
    /// Downloaded copies are read straight from disk (and need no server).
    fn enqueue_analysis(&self, tracks: &[Track]) -> usize {
        let jobs: Vec<analyze::Job> = tracks
            .iter()
            .filter(|t| self.analysis_of(t).is_none())
            .filter_map(|t| {
                let local = self.offline.local_path(&t.id);
                let url = match &self.client {
                    Some(c) => c.stream_url(&t.id).to_string(),
                    None if local.is_some() => String::new(),
                    None => return None,
                };
                Some(analyze::job_for(t, url, local))
            })
            .collect();
        self.analyzer.enqueue(jobs)
    }

    /// Queues the whole library, once the tracks, the stored results and the
    /// server are all there (so finished tracks aren't redone).
    fn resume_library_analysis(&mut self) {
        if self.analysis_library && self.analyses_loaded && self.client.is_some() && !self.data.all_tracks.is_empty() {
            let all = self.data.all_tracks.clone();
            self.enqueue_analysis(&all);
            if !self.analyzer.progress().active() {
                self.finish_library_analysis();
            }
        }
    }

    fn finish_library_analysis(&mut self) {
        if std::mem::take(&mut self.analysis_library) {
            Settings::update(&self.paths, |s| s.analysis_resume = false);
        }
    }

    /// Tracks in the library without a current analysis, and their total length.
    pub fn unanalysed(&self) -> (usize, f64) {
        self.data
            .all_tracks
            .iter()
            .filter(|t| self.analysis_of(t).is_none())
            .fold((0, 0.0), |(n, secs), t| (n + 1, secs + f64::from(t.duration_secs.unwrap_or(0))))
    }

    /// Switches window mode (`None` toggles Mini) and saves the choice.
    fn set_mode(&mut self, to: Option<WindowMode>) {
        let ctx = self.bridge.ctx().clone();
        let plan = match to {
            Some(m) => self.windows.switch(m, self.seen),
            None => self.windows.toggle_mini(self.seen),
        };
        if let Some(plan) = plan {
            plan.apply(&ctx);
            // Until the window reports its new size, don't mistake the old one for it.
            self.seen = None;
            self.settling = Some((plan.size, 30));
            let w = self.windows.clone();
            Settings::update(&self.paths, |s| s.window = w);
        }
    }

    fn go(&mut self, route: Route) {
        self.nav.go(route);
        self.on_arrive();
    }

    /// Loads what the place we just arrived at needs.
    fn on_arrive(&mut self) {
        self.track_sel = None;
        self.focus = Focus::Main;
        match self.nav.current().clone() {
            Route::Album(id) => {
                self.credits_open = false;
                self.bridge.open_album(self.client.clone(), id);
            }
            Route::Artist { name, .. } => self.bridge.find_credited(name),
            Route::Playlist { id, .. } => {
                // Offline, a kept playlist is rebuilt from its pin and the stored tracks.
                if !self.online
                    && !self.data.playlist_tracks.contains_key(&id)
                    && let Some((_, ids)) = self.offline.pin_tracks(&Pin::Playlist(id.clone()))
                {
                    let by_id: HashMap<&str, &Track> = self.data.all_tracks.iter().map(|t| (t.id.as_str(), t)).collect();
                    let tracks: Vec<Track> = ids.iter().filter_map(|i| by_id.get(i.as_str()).map(|t| (*t).clone())).collect();
                    self.data.playlist_tracks.insert(id.clone(), tracks);
                }
                self.bridge.open_playlist(self.client.clone(), id)
            }
            Route::Favorites => self.bridge.fetch_starred(self.client.clone()),
            _ => {}
        }
    }

    /// Stops playback, forgets the saved password and shows the login screen
    /// with the server and username filled in.
    fn sign_out(&mut self) {
        self.engine.send(Command::Stop);
        self.queue.clear();
        self.queue.current = None;
        self.sent_next = None;
        self.palette = None;
        self.overlay = None;
        self.favorites = Favorites::default();
        self.bridge.cancel_sync();
        self.client = None;
        self.server_label.clear();
        let saved = Settings::load(&self.paths);
        if let Some(s) = saved.clone() {
            std::thread::spawn(move || s.forget_password());
        }
        Settings::update(&self.paths, |s| s.signed_out = true);
        self.login = Some(match saved {
            Some(s) => LoginForm { server: s.server, username: s.username, ..Default::default() },
            None => LoginForm::default(),
        });
    }

    fn login_ui(&mut self, ui: &mut egui::Ui) {
        let Some(form) = self.login.as_mut() else { return };
        let mut submit = false;
        let t = Tokens::of(ui);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg)).show(ui, |ui| {
            const W: f32 = 320.0;
            const FORM_H: f32 = 330.0;
            ui.add_space(((ui.available_height() - FORM_H) * 0.42).max(24.0));
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("Listener").font(egui::FontId::new(28.0, theme::semibold())).color(t.text));
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Connect to your Navidrome server").size(13.0).color(t.muted));
                ui.add_space(24.0);
            });
            ui.horizontal(|ui| {
                ui.add_space(((ui.available_width() - W) / 2.0).max(0.0));
                ui.vertical(|ui| {
                    ui.set_width(W);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    let mut enter = false;
                    ui.add_enabled_ui(!form.busy, |ui| {
                        let field = |ui: &mut egui::Ui, label: &str, text: &mut String, hint: &str, password: bool| {
                            ui.label(egui::RichText::new(label).font(egui::FontId::new(12.0, theme::semibold())).color(t.muted));
                            let r = ui.add(
                                egui::TextEdit::singleline(text)
                                    .hint_text(hint)
                                    .password(password)
                                    .desired_width(W)
                                    .margin(egui::vec2(10.0, 7.0))
                                    .background_color(t.field),
                            );
                            r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        };
                        enter |= field(ui, "Server", &mut form.server, "https://music.example.com", false);
                        ui.label(egui::RichText::new("Your Navidrome URL, e.g. https://music.example.com").size(11.0).color(t.muted));
                        ui.add_space(10.0);
                        enter |= field(ui, "Username", &mut form.username, "", false);
                        ui.add_space(10.0);
                        enter |= field(ui, "Password", &mut form.password, "", true);
                    });
                    ui.add_space(18.0);
                    let label = if form.busy { "Connecting…" } else { "Connect" };
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(W, 34.0), if form.busy { egui::Sense::hover() } else { egui::Sense::click() });
                    let fill = if form.busy { t.accent.gamma_multiply(0.6) } else if resp.hovered() { t.accent.gamma_multiply(0.88) } else { t.accent };
                    ui.painter().rect_filled(rect, 7.0, fill);
                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::new(13.0, theme::semibold()), t.on_accent);
                    if form.busy {
                        let r = egui::Rect::from_center_size(egui::pos2(rect.left() + 18.0, rect.center().y), egui::vec2(14.0, 14.0));
                        crate::icons::paint(ui.painter(), r, crate::icons::Icon::Spinner, t.on_accent);
                        ui.ctx().request_repaint_after(Duration::from_millis(33));
                    }
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
                    submit = resp.clicked() || enter;
                    if let Some(e) = &form.error {
                        ui.add_space(12.0);
                        ui.add(egui::Label::new(egui::RichText::new(e).size(12.0).color(ui.visuals().error_fg_color)).wrap());
                    }
                });
            });
        });
        if submit && !form.busy {
            form.busy = true;
            form.error = None;
            let settings = Settings {
                server: form.server.trim().to_owned(),
                username: form.username.trim().to_owned(),
                signed_out: false,
                ..Settings::load(&self.paths).unwrap_or_default()
            };
            self.bridge.connect(settings, Some(std::mem::take(&mut form.password)), self.paths.clone());
        }
    }

    /// App-wide keys, from the key map in [`crate::keys`]. Views handle their
    /// own (track list, queue, palette); a sheet takes the keyboard while open
    /// except to close the shortcut list with ⌘/.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.palette.is_some() {
            return;
        }
        use egui::{Key, Modifiers};
        let typing = ctx.memory(|m| m.focused().is_some());
        let cmds = ctx.input_mut(|i| {
            let mut cmds = crate::keys::take(i, typing);
            if !typing && i.consume_key(Modifiers::NONE, Key::Slash) {
                // Keep the `/` itself out of the search field.
                i.events.retain(|e| !matches!(e, egui::Event::Text(t) if t == "/"));
                cmds.push(Cmd::Search);
            }
            cmds
        });
        for cmd in cmds {
            if self.overlay.is_none() || cmd == Cmd::Shortcuts {
                self.actions.push(Action::Command(cmd));
            }
        }
    }

    fn main_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.art.poll();
        self.shortcuts(&ctx);
        self.scroll_bars(&ctx);
        match self.windows.mode {
            WindowMode::Full => {
                self.full_ui(ui);
                self.palette_ui(&ctx);
                self.overlay_ui(&ctx);
                self.drag_ghost(&ctx);
            }
            WindowMode::Compact => self.compact_ui(ui),
            WindowMode::Mini => self.mini_ui(ui),
        }
        // Commands become actions; apply those in the same frame too (bounded,
        // in case something keeps asking).
        for _ in 0..4 {
            if self.actions.is_empty() {
                break;
            }
            for a in std::mem::take(&mut self.actions) {
                self.apply(a);
            }
        }
    }

    /// What's being dragged, following the pointer.
    fn drag_ghost(&self, ctx: &egui::Context) {
        let Some(drag) = egui::DragAndDrop::payload::<DragTracks>(ctx) else { return };
        let Some(at) = ctx.pointer_interact_pos() else { return };
        let t = Tokens::of_ctx(ctx);
        let text = match drag.tracks.len() {
            1 => drag.tracks[0].title.clone(),
            n => format!("{n} songs"),
        };
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("drag-ghost")));
        let font = egui::FontId::proportional(12.0);
        let galley = painter.layout_no_wrap(text, font, t.text);
        let rect = egui::Rect::from_min_size(at + egui::vec2(14.0, 10.0), galley.size() + egui::vec2(20.0, 10.0));
        painter.rect(rect, 6.0, t.panel, egui::Stroke::new(1.0, t.hairline), egui::StrokeKind::Inside);
        painter.galley(rect.min + egui::vec2(10.0, 5.0), galley, t.text);
    }

    /// Scroll bars show while scrolling and fade a second after.
    fn scroll_bars(&mut self, ctx: &egui::Context) {
        let (now, scrolling) = ctx.input(|i| (i.time, i.smooth_scroll_delta != egui::Vec2::ZERO || i.events.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. }))));
        if scrolling {
            self.last_scroll = now;
        }
        let since = now - self.last_scroll;
        let show = since < 1.0;
        if show != self.bars_shown {
            self.bars_shown = show;
            theme::scroll_bars(ctx, show);
        }
        if show {
            ctx.request_repaint_after(Duration::from_secs_f64(1.0 - since + 0.02));
        }
    }

    fn full_ui(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::of(ui);
        let width = ui.available_width();
        // Narrow windows give the content the room: the queue becomes a sheet
        // over it, and below that the sidebar becomes a rail of icons.
        self.queue_narrow = width < QUEUE_DOCK_MIN_W;
        if !self.queue_narrow {
            self.queue_sheet = false;
        }
        let rail = width < SIDEBAR_FULL_MIN_W;

        egui::Panel::top("toolbar")
            .exact_size(52.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(t.bg).inner_margin(egui::Margin::symmetric(12, 0)))
            .show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status")
            .exact_size(24.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(t.bg).inner_margin(egui::Margin::symmetric(12, 0)))
            .show(ui, |ui| self.status_bar(ui));
        if rail {
            egui::Panel::left("sidebar-rail")
                .exact_size(sidebar::RAIL_W)
                .resizable(false)
                .frame(egui::Frame::new().fill(t.sidebar).inner_margin(egui::Margin { left: 4, right: 4, top: 14, bottom: 10 }))
                .show(ui, |ui| self.sidebar_rail(ui));
        } else {
            egui::Panel::left("sidebar")
                .exact_size(sidebar::SIDEBAR_W)
                .resizable(false)
                .frame(egui::Frame::new().fill(t.sidebar).inner_margin(egui::Margin { left: 10, right: 10, top: 14, bottom: 10 }))
                .show(ui, |ui| self.sidebar(ui));
        }
        if self.show_queue && !self.queue_narrow {
            egui::Panel::right("queue")
                .exact_size(320.0)
                .resizable(false)
                .frame(egui::Frame::new().fill(t.panel).inner_margin(egui::Margin { left: 8, right: 8, top: 12, bottom: 8 }))
                .show(ui, |ui| self.queue_panel(ui));
        }
        let content = egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.bg).inner_margin(egui::Margin { left: 28, right: 28, top: 12, bottom: 0 }))
            .show(ui, |ui| self.main_view(ui));
        if self.queue_sheet {
            self.queue_sheet_ui(ui.ctx(), content.response.rect);
        }
    }

    /// The queue as a sheet sliding over the right of the content (narrow
    /// windows). Esc or a click outside closes it.
    fn queue_sheet_ui(&mut self, ctx: &egui::Context, content: egui::Rect) {
        let t = Tokens::of_ctx(ctx);
        let w = 320.0_f32.min(content.width() - 24.0);
        let id = egui::Id::unique("queue-sheet");
        let area = egui::Modal::default_area(id).anchor(egui::Align2::RIGHT_TOP, egui::vec2(0.0, content.top()));
        let shadow = egui::Shadow { offset: [-6, 0], blur: 24, spread: 0, color: egui::Color32::from_black_alpha(if ctx.global_style().visuals.dark_mode { 120 } else { 40 }) };
        let modal = egui::Modal::new(id)
            .area(area)
            .backdrop_color(egui::Color32::from_black_alpha(18))
            .frame(egui::Frame::new().fill(t.panel).shadow(shadow).stroke(egui::Stroke::new(1.0, t.hairline)).inner_margin(egui::Margin { left: 8, right: 8, top: 12, bottom: 8 }))
            .show(ctx, |ui| {
                ui.set_width(w - 16.0);
                ui.set_height(content.height() - 20.0);
                self.queue_panel(ui);
            });
        if modal.should_close() {
            self.queue_sheet = false;
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let status = self.engine.status.clone();
        let t = Tokens::of(ui);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if let Some(e) = self.error.clone() {
                ui.colored_label(t.warn, egui::RichText::new(e).small());
                if crate::icons::sized_button(ui, crate::icons::Icon::Close, "Dismiss", egui::vec2(18.0, 18.0), 8.0, Some(t.muted)).clicked() {
                    self.error = None;
                }
            } else if let Some(n) = self.notice.clone() {
                ui.label(egui::RichText::new(n).small().color(t.muted));
                if crate::icons::sized_button(ui, crate::icons::Icon::Close, "Dismiss", egui::vec2(18.0, 18.0), 8.0, Some(t.muted)).clicked() {
                    self.notice = None;
                }
            } else {
                if !self.online && self.settings.is_some() {
                    crate::icons::inline(ui, crate::icons::Icon::Offline, t.muted);
                }
                ui.label(egui::RichText::new(format!("{} · {}", self.server_label, self.sync_label)).small().color(t.muted));
                if self.track_syncing && widgets::link(ui, "Stop", egui::FontId::proportional(11.0), t.muted).clicked() {
                    self.bridge.cancel_sync();
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.style_mut().override_text_style = Some(egui::TextStyle::Small);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let codec = self.queue.current.as_ref().and_then(|c| c.track.suffix.as_deref()).map(str::to_uppercase);
                    self.output.signal_ui(ui, &status, codec.as_deref());
                });
            });
        });
    }
}

impl eframe::App for ListenerApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.inbox.try_recv() {
            self.handle(msg);
        }
        // Once per batch of messages: a sync delivers albums, artists and tracks
        // back to back, and each rebuild is ~30 ms on a 20k-track library.
        if std::mem::take(&mut self.search_stale) {
            self.rebuild_search();
        }
        let snap = self.offline.snapshot();
        if snap.version != self.offline_snap.version {
            // A download finished: what's next may now play from disk, or play at all.
            let done_changed = snap.tracks.values().filter(|s| matches!(s, offline::TrackState::Done { .. })).count()
                != self.offline_snap.tracks.values().filter(|s| matches!(s, offline::TrackState::Done { .. })).count();
            self.offline_snap = snap;
            if done_changed && !self.online {
                self.sync_next();
            }
        }
        if self.analysis_library && self.analyses_loaded && !self.data.all_tracks.is_empty() && !self.analyzer.progress().active() {
            self.finish_library_analysis();
        }
        if let Some(menu) = self.menu.as_mut() {
            let queue = if self.queue_narrow { self.queue_sheet } else { self.show_queue };
            let theme = match self.theme {
                ThemePref::System => 0,
                ThemePref::Light => 1,
                ThemePref::Dark => 2,
            };
            menu.sync(queue, self.bit_perfect, theme);
        }
        // Only the progress readout needs a clock; idle costs nothing. Buffering
        // animates the seek bar and spinner, so it ticks faster.
        match self.engine.status.state() {
            PlayState::Playing => ctx.request_repaint_after(Duration::from_millis(250)),
            PlayState::Buffering | PlayState::Loading => ctx.request_repaint_after(Duration::from_millis(33)),
            _ if self.pending_seek.is_some() => ctx.request_repaint_after(Duration::from_millis(33)),
            _ => {}
        }
        if let Some(g) = Geometry::read(ctx) {
            match self.settling {
                Some((want, left)) if left > 0 && ((g.size[0] - want[0]).abs() > 2.0 || (g.size[1] - want[1]).abs() > 2.0) => {
                    self.settling = Some((want, left - 1));
                    ctx.request_repaint();
                }
                _ => {
                    self.settling = None;
                    self.seen = Some(g);
                }
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.login.is_some() {
            // Logging in needs the full window.
            self.set_mode(Some(WindowMode::Full));
            self.login_ui(ui);
        } else {
            self.main_ui(ui);
        }
    }

    fn on_exit(&mut self) {
        if let Some(g) = self.seen {
            self.windows.remember(g);
        }
        let w = self.windows.clone();
        Settings::update(&self.paths, |s| s.window = w);
    }
}

impl Drop for ListenerApp {
    fn drop(&mut self) {
        self.bridge.cancel_sync();
        self.engine.send(Command::Shutdown);
    }
}

/// Fisher–Yates with a small xorshift; good enough for a play order.
fn shuffle<T>(v: &mut [T]) {
    let mut s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0x9E37_79B9, |d| d.as_nanos() as u64) | 1;
    for i in (1..v.len()).rev() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        v.swap(i, (s % (i as u64 + 1)) as usize);
    }
}

/// What the offline store needs to download `t`.
pub fn track_ref(t: &Track) -> TrackRef {
    TrackRef::new(t.id.clone(), t.suffix.clone(), t.size)
}

pub fn mmss(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

#[cfg(test)]
mod tests {
    //! Headless frames of the main window with a populated library, queue and
    //! palette: catches layout panics and checks the views wire up to actions.
    use super::*;
    use crate::nav::FilterKind;

    struct NullOutput;

    impl audio::OutputBackend for NullOutput {
        fn name(&self) -> &str {
            "null"
        }
        fn device_caps(&mut self) -> Option<audio::DeviceCaps> {
            None
        }
        fn open(&mut self, _: audio::StreamFormat, _: audio::RingInput, _: Arc<audio::Status>) -> Result<(), audio::OutputError> {
            Ok(())
        }
        fn drained(&self) -> bool {
            true
        }
        fn close(&mut self) {}
    }

    fn track(id: &str, album: &str, n: u32) -> Track {
        Track {
            id: id.into(),
            album_id: album.into(),
            title: format!("Track {n} with a fairly long title that has to be cut"),
            artist: Some("Bill Evans Trio".into()),
            artist_ref: None,
            track: Some(n),
            disc: Some(1 + n / 8),
            duration_secs: Some(240 + n),
            suffix: Some("flac".into()),
            content_type: None,
            size: None,
            bit_rate_kbps: None,
            bit_depth: Some(16),
            sample_rate: Some(44_100),
            channels: Some(2),
            track_gain: None,
            album_gain: None,
            track_peak: None,
            album_peak: None,
            composer: Some("Scott LaFaro".into()),
            contributors: vec![
                library::Contributor { role: "performer".into(), sub_role: Some("piano".into()), name: "Bill Evans".into(), artist_id: None },
                library::Contributor { role: "composer".into(), sub_role: None, name: "Scott LaFaro".into(), artist_id: None },
            ],
            cover_art: None,
            artists: Vec::new(),
        }
    }

    fn app(ctx: &egui::Context, dir: &std::path::Path) -> ListenerApp {
        let paths = Paths {
            config_file: dir.join("settings.json"),
            library_db: dir.join("library.redb"),
            stream_cache: dir.join("streams"),
            art_cache: dir.join("art"),
            offline: dir.join("offline"),
        };
        let mut app = ListenerApp::build(ctx, paths, Box::new(NullOutput)).unwrap();
        app.login = None;
        let albums: Vec<Album> = (0..60)
            .map(|i| Album {
                id: format!("al-{i}"),
                name: format!("Album {i}"),
                sort: None,
                artist: Some(if i % 3 == 0 { "Bill Evans Trio".into() } else { format!("Artist {}", i % 7) }),
                artist_ref: None,
                year: Some(1955 + i),
                genre: Some("Jazz".into()),
                song_count: 20,
                duration_secs: 3000,
                cover_art: Some(format!("al-{i}")),
                created: Some(format!("2024-01-{:02}", 1 + i % 28)),
                labels: vec!["Riverside".into()],
                genres: vec!["Jazz".into(), "Cool Jazz".into()],
                artists: Vec::new(),
            })
            .collect();
        app.data.set_albums(albums);
        let tracks: Vec<Track> = (1..=20).map(|n| track(&format!("t-{n}"), "al-0", n)).collect();
        app.data.set_all_tracks(tracks.clone());
        app.rebuild_search();
        app.data.playlists = vec![subsonic::Playlist { id: "pl-1".into(), name: "Late night".into(), song_count: 3, duration: 900, cover_art: None, owner: None }];
        app.data.playlist_tracks.insert("pl-1".into(), tracks[..3].to_vec());
        app.data.credited.insert("Scott LaFaro".into(), vec!["al-1".into(), "al-0".into()]);
        app.queue.play_now(tracks.clone(), 2, Source::Album { id: "al-0".into(), name: "Album 0".into() });
        app.queue.play_next(tracks[..2].to_vec());
        app.queue.add(tracks[5..15].to_vec());
        app
    }

    fn frame(ctx: &egui::Context, app: &mut ListenerApp, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1360.0, 860.0))),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| app.main_ui(ui));
        // No renderer here to upload textures to.
        out.textures_delta.clear();
    }

    #[test]
    fn repeat_queue_with_nothing_playable_stops() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        app.online = false;
        app.queue.end = crate::queue::EndAction::RepeatQueue;
        let pass = app.queue.pass_len();
        app.start_current();
        assert!(app.queue.history.len() <= 2 * pass, "history grew to {}", app.queue.history.len());
        assert_eq!(app.notice.as_deref(), Some("Nothing in the queue is downloaded"));
    }

    #[test]
    fn every_view_draws() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        let routes = [
            Route::Recent,
            Route::Albums,
            Route::Artists,
            Route::Artist { name: "Scott LaFaro".into(), id: None },
            Route::Artist { name: "Bill Evans Trio".into(), id: None },
            Route::Album("al-0".into()),
            Route::Album("al-missing".into()),
            Route::Playlist { id: "pl-1".into(), name: "Late night".into() },
            Route::Filter { kind: FilterKind::Genre, value: "Cool Jazz".into() },
            Route::Filter { kind: FilterKind::Label, value: "Nobody".into() },
            Route::Offline,
            Route::Songs,
        ];
        for (k, r) in routes.into_iter().enumerate() {
            app.nav.go(r);
            // Hover somewhere different each time so hover-only details draw too.
            let at = egui::pos2(300.0 + 37.0 * k as f32, 180.0 + 41.0 * k as f32);
            for _ in 0..2 {
                frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
            }
        }
        app.queue_ui.tab = QueueTab::History;
        app.queue.advance();
        frame(&ctx, &mut app, Vec::new());
        app.show_queue = false;
        frame(&ctx, &mut app, Vec::new());
        app.nav.go(Route::Album("al-0".into()));
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.nav.trail(), [&Route::Songs, &Route::Album("al-0".into())]);
    }

    #[test]
    fn palette_opens_searches_and_queues() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        let cmd_k = egui::Event::Key { key: egui::Key::K, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND };
        frame(&ctx, &mut app, vec![cmd_k]);
        assert!(app.palette.is_some());
        // The field takes focus on the palette's first frame; type after that.
        frame(&ctx, &mut app, Vec::new());
        frame(&ctx, &mut app, vec![egui::Event::Text("album 12".into())]);
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.palette.as_ref().map(|p| p.query.as_str()), Some("album 12"));
        let before = app.queue.upcoming.len();
        let alt_enter = egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::ALT };
        frame(&ctx, &mut app, vec![alt_enter]);
        assert!(app.palette.is_none());
        // Album 12's tracks aren't loaded, so the action waits for them...
        assert_eq!(app.queue.upcoming.len(), before);
        assert_eq!(app.pending.len(), 1);
        // ...and runs when they arrive.
        let tracks: Vec<Track> = (1..=3).map(|n| track(&format!("x-{n}"), "al-12", n)).collect();
        app.handle(Msg::Tracks { album_id: "al-12".into(), tracks });
        assert_eq!(app.queue.upcoming.len(), before + 3);
        assert!(app.pending.is_empty());
        // Played next: right after what the user already chose to play next.
        assert_eq!(app.queue.upcoming[2].track.id, "x-1");
    }

    #[test]
    fn window_modes_draw_and_switch() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        let key = |key, modifiers| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        frame(&ctx, &mut app, vec![key(egui::Key::M, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT)]);
        assert_eq!(app.windows.mode, WindowMode::Mini);
        // Hovered (overlay drawn) and not.
        for at in [egui::pos2(100.0, 100.0), egui::pos2(-10.0, -10.0)] {
            for _ in 0..3 {
                frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
            }
        }
        frame(&ctx, &mut app, vec![key(egui::Key::Num2, egui::Modifiers::COMMAND | egui::Modifiers::ALT)]);
        assert_eq!(app.windows.mode, WindowMode::Compact);
        for _ in 0..2 {
            frame(&ctx, &mut app, vec![egui::Event::PointerMoved(egui::pos2(120.0, 60.0))]);
        }
        // Browsing goes back to the full window.
        app.actions.push(Action::Go(Route::Album("al-0".into())));
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.windows.mode, WindowMode::Full);
        // ⌘K from a small mode does too.
        app.windows.mode = WindowMode::Compact;
        frame(&ctx, &mut app, vec![key(egui::Key::K, egui::Modifiers::COMMAND)]);
        assert_eq!(app.windows.mode, WindowMode::Full);
        assert!(app.palette.is_some());
    }

    /// Makes track `id` downloaded, the way a finished stream would: via a file
    /// the stream cache "has", which the store links in.
    fn download(app: &mut ListenerApp, dir: &std::path::Path, id: &str) {
        let file = dir.join(format!("cached-{id}.flac"));
        std::fs::write(&file, b"fLaC").unwrap();
        let want = id.to_owned();
        app.offline.set_cache_lookup(Arc::new(move |r: &TrackRef| (r.id == want).then(|| file.clone())));
        let t = app.data.all_tracks.iter().find(|t| t.id == id).unwrap().clone();
        app.apply(Action::KeepOffline(Keep::Tracks(vec![t]), true));
        let start = std::time::Instant::now();
        while !app.offline.snapshot().is_done(id) {
            assert!(start.elapsed() < Duration::from_secs(5), "download of {id}");
            std::thread::sleep(Duration::from_millis(5));
        }
        app.offline_snap = app.offline.snapshot();
    }

    #[test]
    fn offline_the_queue_plays_only_what_is_downloaded() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        download(&mut app, dir.path(), "t-2");
        assert!(!app.online);
        let tracks: Vec<Track> = app.data.tracks["al-0"][..4].to_vec();
        assert!(app.source(0, None, &tracks[0], None).is_none(), "offline and not downloaded: nothing to play");
        let local = app.source(0, None, &tracks[1], None).unwrap();
        assert!(local.url.starts_with("file://") && local.url.ends_with("t-2.flac"), "{}", local.url);

        // Playing from the top skips to the downloaded song and says so.
        app.queue = Queue::default();
        app.queue_tracks(tracks.clone(), Source::Album { id: "al-0".into(), name: "Album 0".into() }, QueueAction::Play { start: 0 });
        assert_eq!(app.queue.current.as_ref().unwrap().track.id, "t-2");
        assert_eq!(app.notice.as_deref(), Some("Skipped 1 song that isn't downloaded"));
        // Nothing after it can play, so the engine isn't told about a next track.
        assert_eq!(app.sent_next, None);
        // When it ends, the rest is skipped and playback stops cleanly.
        app.handle_engine(audio::Event::Ended { track_id: "t-2".into(), token: 0 });
        assert!(app.queue.current.is_none());

        // Back online: everything plays again, downloaded songs still from disk.
        let client = subsonic::Client::new("http://127.0.0.1:9", subsonic::Credentials::new("u", "p")).unwrap();
        app.handle(Msg::Connected { client, settings: Box::default(), server: "test".into() });
        assert!(app.online);
        assert!(app.source(0, None, &tracks[0], None).unwrap().url.contains("/rest/stream"));
        assert!(app.source(0, None, &tracks[1], None).unwrap().url.starts_with("file://"));
    }

    #[test]
    fn keeping_an_album_waits_for_its_tracks_and_the_offline_place_draws() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        // Album 5's tracks aren't loaded: the pin waits for them.
        app.apply(Action::KeepOffline(Keep::Album("al-5".into()), true));
        assert!(!app.offline.is_pinned(&Pin::Album("al-5".into())));
        let tracks: Vec<Track> = (1..=3).map(|n| track(&format!("y-{n}"), "al-5", n)).collect();
        app.handle(Msg::Tracks { album_id: "al-5".into(), tracks: tracks.clone() });
        assert!(app.offline.is_pinned(&Pin::Album("al-5".into())));
        assert_eq!(app.offline.pin_tracks(&Pin::Album("al-5".into())).unwrap().1, ["y-1", "y-2", "y-3"]);
        // The album changed on the server: the pin follows.
        app.offline_snap = app.offline.snapshot();
        app.handle(Msg::Tracks { album_id: "al-5".into(), tracks: tracks[..2].to_vec() });
        assert_eq!(app.offline.pin_tracks(&Pin::Album("al-5".into())).unwrap().1, ["y-1", "y-2"]);
        download(&mut app, dir.path(), "t-3");

        for route in [Route::Offline, Route::Album("al-5".into()), Route::Album("al-0".into()), Route::Recent, Route::Songs] {
            app.nav.go(route);
            for _ in 0..2 {
                frame(&ctx, &mut app, vec![egui::Event::PointerMoved(egui::pos2(500.0, 300.0))]);
            }
        }
        // ⇧⌘D on the album page toggles it.
        app.nav.go(Route::Album("al-5".into()));
        let key = egui::Event::Key { key: egui::Key::D, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND | egui::Modifiers::SHIFT };
        frame(&ctx, &mut app, vec![key]);
        assert!(!app.offline.is_pinned(&Pin::Album("al-5".into())));
        assert!(app.offline.is_pinned(&Pin::Track("t-3".into())));
    }

    #[test]
    fn gapless_start_advances_the_queue() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        let next = app.queue.peek_next().unwrap().clone();
        let format = audio::StreamFormat { sample_rate: 44_100, channels: 2, bits_per_sample: Some(16) };
        let started = |i: &crate::queue::Item| audio::Event::TrackStarted {
            track_id: i.track.id.clone(),
            token: i.uid,
            format,
            gapless: true,
            restart: false,
        };
        app.handle_engine(started(&next));
        assert_eq!(app.queue.current.as_ref().unwrap().uid, next.uid);
        // A repeat of the same event (or the current track) changes nothing.
        let upcoming = app.queue.upcoming.len();
        app.handle_engine(started(&next));
        assert_eq!(app.queue.upcoming.len(), upcoming);
    }

    #[test]
    fn the_playing_song_queued_next_still_advances_gaplessly() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        let format = audio::StreamFormat { sample_rate: 44_100, channels: 2, bits_per_sample: Some(16) };
        let started = |i: &crate::queue::Item| audio::Event::TrackStarted {
            track_id: i.track.id.clone(),
            token: i.uid,
            format,
            gapless: true,
            restart: false,
        };
        // "Play next" on the song that's playing: the same track, a new queue item.
        let playing = app.queue.current.clone().unwrap();
        app.queue.clear();
        app.queue.play_next(vec![playing.track.clone()]);
        let copy = app.queue.peek_next().unwrap().clone();
        assert_eq!(copy.track.id, playing.track.id);
        assert_ne!(copy.uid, playing.uid);
        app.handle_engine(started(&copy));
        assert_eq!(app.queue.current.as_ref().unwrap().uid, copy.uid, "the queue moved to the copy");

        // Repeat queue over a single item: each gapless start is the next pass.
        app.queue = Queue::default();
        app.queue.end = crate::queue::EndAction::RepeatQueue;
        app.queue.play_now(vec![playing.track.clone()], 0, Source::Added);
        let first = app.queue.current.clone().unwrap();
        app.handle_engine(started(&first));
        let second = app.queue.current.clone().unwrap();
        assert_ne!(second.uid, first.uid, "a new pass began");
        assert_eq!(app.queue.history.len(), 1);
    }
}
