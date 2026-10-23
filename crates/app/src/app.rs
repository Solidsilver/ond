//! The main window: sidebar · toolbar with the now-playing display · the
//! current view · the queue column. Views are in submodules; they don't change
//! state directly but push [`Action`]s, which are applied once the frame is
//! drawn. That keeps borrows simple and makes every view reusable by the
//! compact layout.

mod album_page;
mod browse;
mod commands;
mod compact;
mod diagnostics;
mod offline_page;
mod dialogs;
mod favorites_page;
mod info_panel;
mod layout;
mod now_playing_view;
mod palette;
mod settings_page;
mod queue_panel;
mod seek;
mod sidebar;
mod songs;
mod toasts;
mod toolbar;
mod widgets;

#[cfg(test)]
mod snapshots;

use crate::analyze;
use crate::art::Art;
use crate::bridge::{Bridge, ListKey, Msg};
use crate::favorites::{Favorites, StarKind};
use crate::keys::Cmd;
use crate::nav::{Nav, Route};
use crate::now_playing::{self, Remote};
use crate::output::Outputs;
use crate::playback;
use crate::player::{self, Note, Player};
use crate::prefs::{Loaded, Prefs};
use crate::queue::{EndAction, Source};
use crate::search::SearchIndex;
use crate::settings::{Paths, Settings};
use crate::theme::{self, ThemePref, Tokens};
use crate::tracks::TrackSet;
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
pub(crate) use widgets::{MODES, choice, elide as elide_text, link as link_text, mode_label};


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
    /// Every stored track once, grouped by album and indexed by id, plus the
    /// albums opened since (fresh from the server).
    pub tracks: TrackSet,
    pub playlist_tracks: HashMap<String, Arc<Vec<Track>>>,
    /// Album ids crediting a person, from stored tracks.
    pub credited: HashMap<String, Vec<String>>,
    /// The server's artists, as separate entities ("A, B & C" split).
    pub artists: Vec<library::Artist>,
    /// Bumped whenever the stored tracks (`tracks.all`) change.
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

    /// Takes the library's tracks, grouped and indexed (off the UI thread).
    /// Stored tracks are the server's as of the last sync; anything fetched
    /// since (an album opened) is in the db too, so this replaces it all.
    fn set_tracks(&mut self, set: TrackSet) {
        self.tracks = set;
        self.tracks_version += 1;
    }

    #[cfg(test)]
    fn set_all_tracks(&mut self, tracks: Vec<Track>) {
        self.set_tracks(TrackSet::new(tracks));
    }

    /// A track by id, from the stored library or albums opened since.
    pub fn track(&self, id: &str) -> Option<&Track> {
        self.tracks.track(id)
    }

    /// A track's artwork: its own, else its album's.
    pub fn cover_for<'a>(&'a self, track: &'a Track) -> Option<&'a str> {
        track.cover_art.as_deref().or_else(|| self.album(&track.album_id).and_then(|a| a.cover_art.as_deref()))
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

pub use crate::player::QueueAction;

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
    /// The full window ↔ the player last used (Player or Mini).
    TogglePlayer,
    /// Show the lyrics: the sidebar's Lyrics tab in the full window; in the
    /// Player, open or close its drawer; from Mini, the Player with it open.
    Lyrics,
    /// Show (or hide again) the playing song's details: the Info tab of the
    /// sidebar, or of the Player's queue; from Mini, the Player on it.
    Info,
    /// Open or close the Player's lyrics drawer.
    SetLyricsDrawer(bool),
    SetScrobble(bool),
    /// Show or hide the diagnostics bar.
    SetDiagnostics(bool),
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
    /// The playing track's lyrics (the full window's sidebar only).
    Lyrics,
    /// The playing track's credits, album, file and analysis details.
    Info,
}

impl QueueTab {
    pub fn label(self) -> &'static str {
        match self {
            QueueTab::UpNext => "Up Next",
            QueueTab::History => "History",
            QueueTab::Lyrics => "Lyrics",
            QueueTab::Info => "Info",
        }
    }
}

/// A song's plays as the server counts them, for the Info tab.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SongStats {
    pub plays: Option<u64>,
    /// RFC 3339, as the server sends it.
    pub last_played: Option<String>,
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
    key: ListKey,
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

impl AlbumSort {
    pub const ALL: [AlbumSort; 4] = [AlbumSort::Artist, AlbumSort::Title, AlbumSort::Year, AlbumSort::Added];

    pub fn label(self) -> &'static str {
        match self {
            AlbumSort::Artist => "Artist",
            AlbumSort::Title => "Title",
            AlbumSort::Year => "Year",
            AlbumSort::Added => "Recently added",
        }
    }
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
    /// The window's egui context (for theme changes and window commands).
    ctx: egui::Context,
    bridge: Bridge,
    inbox: Receiver<Msg>,
    engine: EngineHandle,

    login: Option<LoginForm>,
    /// The server connection and who's signed in.
    session: Session,
    /// Where the library sync is.
    sync: SyncState,

    data: Data,
    nav: Nav,
    sort: AlbumSort,
    /// The album list for the current view, cached by (route, sort, data version).
    grid: crate::memo::Memo<(Route, AlbumSort, u64), Vec<usize>>,
    actions: Vec<Action>,
    pending: Vec<Pending>,
    art: Art,

    /// The queue and what the engine is told about it.
    player: Player,
    /// Sidebar and queue column widths (dragged by their inner edges).
    columns: layout::Columns,
    /// The album page's "Album details" is unfolded (for this session).
    album_details_open: bool,
    /// Narrow windows: the queue opens as a sheet over the content instead.
    queue_sheet: bool,
    /// The window is too narrow to dock the queue column (set each frame).
    queue_narrow: bool,
    queue_ui: QueueUi,
    focus: Focus,
    /// Keyboard selection in the album/playlist track list.
    track_sel: Option<usize>,

    /// Output devices and what reaches the one in use.
    output: Outputs,
    bit_perfect: bool,
    /// Where the seek bar is being dragged to, in seconds.
    seek_preview: Option<f64>,

    /// The palette's search index.
    search: SearchState,
    songs: songs::SongsView,
    /// Errors and notices, shown as toasts.
    toasts: toasts::Toasts,
    palette: Option<Palette>,

    /// Full / Player / Mini and where each was.
    windows: Windows,
    /// Mini is held square: what was last asked of the platform, and whether it
    /// could (if not, the size is snapped back each frame instead).
    square_wanted: Option<bool>,
    square_native: bool,
    /// The window as last seen, saved on quit.
    seen: Option<Geometry>,
    /// After a mode switch: the size asked for, and frames left to wait for it.
    /// The window reports its old size for a frame or two meanwhile.
    settling: Option<([f32; 2], u32)>,
    /// When the current mode switch was asked for (for the debug log).
    switch_started: Option<std::time::Instant>,
    /// When the last scroll happened (frame clock), for showing scroll bars.
    last_scroll: f64,
    bars_shown: bool,
    /// OS Now Playing and server scrobbles.
    now_playing: now_playing::Session,

    /// Offline downloads.
    downloads: Downloads,
    /// The user's preferences: the one copy, saved in the background.
    prefs: Prefs,
    /// Loudness, dynamic range and fake hi-res checks.
    analysis: Analysis,
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
    /// Lyrics by track id, fetched when a lyrics view first needs them.
    lyrics: HashMap<String, crate::lyrics::Fetch>,
    /// Play counts by track id for the Info tab: absent until asked for,
    /// `None` while loading or when the server couldn't say.
    song_stats: HashMap<String, Option<SongStats>>,
    /// Whether the lyrics column follows the sung line.
    lyrics_follow: crate::lyrics::Follow,
    /// The Player's details strip is open.
    np_details: bool,
    /// The diagnostics bar: whether it's on, and what it measures.
    diagnostics: diagnostics::Diagnostics,
}

/// The server connection and who's signed in.
#[derive(Default)]
pub struct Session {
    pub client: Option<Client>,
    /// The server answered last time we asked. While it's unreachable only
    /// downloaded tracks play.
    pub online: bool,
    /// The server stopped answering and reconnecting runs in the background
    /// (as opposed to the first connect still under way).
    pub unreachable: bool,
    /// The login in use, for reconnecting.
    pub settings: Option<Settings>,
    /// "user · navidrome 0.64.2", or "user · offline".
    pub server_label: String,
    /// "navidrome 0.64.2", as the server reported it.
    pub server_info: String,
}

impl Session {
    /// The client, while the server answers.
    pub fn live(&self) -> Option<&Client> {
        self.client.as_ref().filter(|_| self.online)
    }
}

/// Where the library sync is.
#[derive(Default)]
pub struct SyncState {
    pub label: String,
    /// The album list is being fetched (before the track sync).
    pub albums: bool,
    /// A track sync is running (shows a stop button).
    pub tracks: bool,
    /// Where the track sync is, while it runs.
    pub progress: Option<crate::sync::Progress>,
    /// When the last full sync finished (this session).
    pub last: Option<chrono::DateTime<chrono::Local>>,
}

/// The palette's search index.
#[derive(Default)]
pub struct SearchState {
    /// Rebuilt (off the UI thread) whenever albums, artists or tracks change;
    /// the old one serves until the new one lands.
    pub index: Option<SearchIndex>,
    /// Albums, artists or tracks changed since the index was built.
    pub stale: bool,
    /// The latest index build asked for; older builds that land late are dropped.
    pub generation: u64,
}

/// Offline downloads, and their state as of this frame.
pub struct Downloads {
    pub store: OfflineStore,
    pub snap: Arc<offline::Snapshot>,
    /// Pins waiting for their track lists to load, by the same keys as `pending`.
    pub pending_pins: Vec<(ListKey, Pin)>,
}

impl Downloads {
    /// Takes in what changed since the last frame.
    pub fn refresh(&mut self) {
        self.snap = self.store.snapshot();
    }

    /// Applies the download actions that need nothing but the store.
    fn apply(&mut self, action: Action) {
        match action {
            Action::Unpin(pin) => {
                self.store.unpin(&pin);
                self.refresh();
            }
            Action::PauseDownloads(paused) => self.store.set_paused(paused),
            Action::RetryDownloads => self.store.retry_failed(),
            _ => unreachable!("not a download action"),
        }
    }
}

/// Library analysis: stored results and the background runner.
pub struct Analysis {
    /// Stored analysis results by track id, with the file stamp they're for.
    pub records: HashMap<String, analysis::Record>,
    /// Bumped whenever `records` changes, so counts over it know to recount.
    pub version: u64,
    /// How much of the library has no current analysis, per (records, tracks) version.
    pub unanalysed: crate::memo::Memo<(u64, u64), (usize, f64)>,
    /// The stored results have been read (until then everything looks unanalysed).
    pub loaded: bool,
    pub runner: analyze::Runner,
}

impl Analysis {
    /// A track's analysis, if there is one for the file as it is now.
    pub fn of(&self, t: &Track) -> Option<&analysis::TrackAnalysis> {
        self.records.get(&t.id).filter(|r| r.stamp == analyze::stamp_of(t)).map(|r| &r.analysis)
    }
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
        // Four workers: downloads, sync and the server calls share them, so one
        // busy task can't hold up the stream that feeds playback.
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build()?;
        let paths = Arc::new(paths);
        // The db is only a mirror of the server, so an unreadable one is set aside
        // and rebuilt by the sync that runs on connect.
        let (library, moved_aside) = Library::open_or_recover(&paths.library_db)?;
        let notice = moved_aside.map(|aside| {
            tracing::warn!("library database was unreadable; moved to {} and starting fresh", aside.display());
            format!("Library database was unreadable and is being rebuilt (old copy: {})", aside.display())
        });
        // Analysis results live apart from the (disposable) library mirror.
        let analyses = Arc::new(library::AnalysisStore::open(paths.library_db.with_file_name("analysis.redb"))?);
        match analyses.adopt(&library) {
            Ok(0) => {}
            Ok(n) => tracing::info!("moved {n} analysis results out of the library database"),
            Err(e) => tracing::warn!("moving analysis results: {e}"),
        }
        let library = Arc::new(library);
        let (tx, inbox) = channel();
        let repaint = ctx.clone();
        let bridge = Bridge::new(rt.handle().clone(), tx, Arc::new(move || repaint.request_repaint()), library.clone());

        let (prefs, loaded) = Prefs::open(&paths.config_file);
        let notice = match loaded {
            Loaded::Unreadable { aside } => Some(format!("Settings couldn't be read and were reset; please sign in again (old copy: {})", aside.display())),
            _ => notice,
        };
        let saved = prefs.snapshot();
        theme::install(ctx, saved.theme);
        let mut cache_cfg = streamcache::CacheConfig::new(&paths.stream_cache);
        if let Some(mb) = saved.stream_cache_mb {
            cache_cfg.max_bytes = mb * 1024 * 1024;
        }
        let cache = streamcache::StreamCache::new(cache_cfg, rt.handle().clone())?;
        let offline = {
            let dir = saved.offline_dir.clone().unwrap_or_else(|| paths.offline.clone());
            // A new location chosen last time: bring the downloads along first.
            if let Some(from) = saved.offline_move_from.clone() {
                match offline::move_store(&from, &dir) {
                    Ok(()) => prefs.edit(|s| s.offline_move_from = None),
                    Err(e) => tracing::warn!("moving downloads from {}: {e}", from.display()),
                }
            }
            let limit = saved.offline_limit_gb.map(|gb| gb * 1_000_000_000);
            let repaint = ctx.clone();
            let store = OfflineStore::open(offline::OfflineConfig { dir, limit }, rt.handle().clone(), Arc::new(move || repaint.request_repaint()))?;
            // A track that finished streaming is linked in rather than downloaded again.
            let cached = cache.clone();
            store.set_cache_lookup(Arc::new(move |r: &TrackRef| cached.complete_path(&streamcache::CacheKey::original(r.id.clone(), r.suffix.clone()))));
            store
        };
        let playback_cache = cache.clone();
        // Analysis fetches uncached files here, so a library-wide run doesn't
        // evict what was just played.
        let mut scratch_cfg = streamcache::CacheConfig::new(paths.stream_cache.with_file_name("analysis"));
        scratch_cfg.max_bytes = 64 * 1024 * 1024;
        let scratch_cache = streamcache::StreamCache::new(scratch_cfg, rt.handle().clone())?;
        let engine = audio::spawn(audio::EngineConfig::default(), backend, Some(cache));
        let output = Outputs::restore(&engine, prefs.clone());
        engine.send(Command::SetVolume(saved.volume));
        let scrobble = saved.scrobble;
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
        bridge.load_analyses(analyses.clone());
        let analyzer = {
            let (results, ticks) = (bridge.clone(), bridge.notifier());
            analyze::Runner::start(analyze::Config {
                store: analyses,
                playback: playback_cache,
                scratch: scratch_cache,
                status: engine.status.clone(),
                on_result: Arc::new(move |track_id, record| results.post(Msg::Analyzed { track_id, record })),
                on_progress: ticks,
            })
        };
        let art = Art::new(rt.handle().clone(), ctx.clone(), paths.art_cache.clone());

        let login = if !prefs.has_login() {
            LoginForm { server: "http://localhost:4533".into(), ..Default::default() }
        } else if saved.signed_out {
            LoginForm { server: saved.server.clone(), username: saved.username.clone(), ..Default::default() }
        } else {
            bridge.connect(saved.clone(), None);
            LoginForm { server: saved.server.clone(), username: saved.username.clone(), busy: true, ..Default::default() }
        };

        Ok(Self {
            _rt: rt,
            ctx: ctx.clone(),
            album_details_open: false,
            columns: layout::Columns::restore(saved.sidebar_width, saved.queue_width),
            bridge,
            inbox,
            engine,
            login: Some(login),
            session: Session { settings: prefs.has_login().then(|| saved.clone()), ..Default::default() },
            sync: SyncState::default(),
            data: Data::default(),
            nav: Nav::default(),
            sort: AlbumSort::default(),
            grid: Default::default(),
            actions: Vec::new(),
            pending: Vec::new(),
            art,
            player: Player::default(),
            queue_ui: QueueUi::default(),
            queue_sheet: false,
            queue_narrow: false,
            focus: Focus::default(),
            track_sel: None,
            output,
            bit_perfect: false,
            seek_preview: None,
            search: SearchState::default(),
            songs: songs::SongsView::default(),
            // Something worth knowing at start-up (e.g. the library was rebuilt).
            toasts: toasts::Toasts::with_notice(notice),
            palette: None,
            windows: saved.window.clone(),
            seen: None,
            settling: None,
            switch_started: None,
            square_wanted: None,
            square_native: false,
            last_scroll: f64::NEG_INFINITY,
            bars_shown: false,
            now_playing,
            downloads: Downloads { snap: offline.snapshot(), store: offline, pending_pins: Vec::new() },
            diagnostics: diagnostics::Diagnostics::default(),
            prefs,
            analysis: Analysis { records: HashMap::new(), version: 0, unanalysed: Default::default(), loaded: false, runner: analyzer },
            favorites: Favorites::default(),
            overlay: None,
            menu: None,
            reveal: None,
            recent_playlists: Vec::new(),
            fav_tab: favorites_page::FavTab::default(),
            settings_section: settings_page::Section::default(),
            lyrics: HashMap::new(),
            song_stats: HashMap::new(),
            lyrics_follow: crate::lyrics::Follow::default(),
            np_details: false,
        })
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            // From before a sign-out: stale, whatever it says.
            Msg::Connected { session, .. } | Msg::Unreachable { session, .. } if session != self.bridge.session() => {}
            Msg::Connected { client, settings, server, .. } => {
                self.session.server_label = format!("{} · {}", settings.username, server);
                self.session.server_info = server.clone();
                self.login = None;
                self.sync.label = "syncing…".into();
                self.sync.albums = true;
                self.bridge.sync_albums(client.clone());
                self.bridge.fetch_home(client.clone());
                self.art.set_client(client.clone());
                self.now_playing.set_client(client.clone());
                self.output.connected(&settings.server);
                let url = client.clone();
                self.downloads.store.set_source(Some(Arc::new(move |id: &str| url.stream_url(id).to_string())));
                // Pinned playlists follow the server: refetch them so additions download.
                for p in &self.downloads.snap.pins {
                    if let Pin::Playlist(id) = &p.pin {
                        self.bridge.open_playlist(Some(client.clone()), id.clone());
                    }
                }
                self.bridge.fetch_starred(Some(client.clone()));
                self.diagnostics.latency_asked();
                self.bridge.measure_latency(client.clone());
                self.session.unreachable = false;
                self.session.client = Some(client);
                self.prefs.set_login(&settings.server, &settings.username);
                self.session.settings = Some(*settings);
                if !self.session.online {
                    self.session.online = true;
                    // Lined up while offline; the server can supply the rest again.
                    self.player.forget_next();
                    self.sync_next();
                }
                self.resume_library_analysis();
            }
            Msg::ConnectFailed(e) => {
                self.set_offline();
                let form = self.login.get_or_insert_with(|| {
                    let s = self.session.settings.clone().unwrap_or_default();
                    LoginForm { server: s.server, username: s.username, ..Default::default() }
                });
                form.busy = false;
                form.error = Some(e);
            }
            Msg::Unreachable { username, error, .. } => {
                tracing::info!("server unreachable: {error}");
                self.login = None;
                self.set_offline();
                self.session.unreachable = true;
                self.diagnostics.set_latency(None);
                self.session.server_label = format!("{username} · offline");
                self.sync.label = "reconnecting…".into();
            }
            Msg::Latency(rtt) => self.diagnostics.set_latency(rtt),
            Msg::SearchIndex { generation, index } => {
                if generation == self.search.generation {
                    self.install_search(*index);
                }
            }
            Msg::Albums(a) => {
                self.data.set_albums(a);
                self.search.stale = true;
            }
            Msg::Artists(a) => {
                self.data.artists = a;
                self.search.stale = true;
            }
            Msg::AllTracks(set) => {
                self.data.set_tracks(*set);
                self.search.stale = true;
                // A sync may have added tracks to pinned albums. The library can be
                // partial (a sync stopped or failed, a rebuilt database), so this may
                // only add: removals wait for the server's own listing of the album.
                let albums: Vec<String> = self.downloads.snap.pins.iter().filter_map(|p| if let Pin::Album(id) = &p.pin { Some(id.clone()) } else { None }).collect();
                for id in albums {
                    if let Some(tracks) = self.data.tracks.album(&id).cloned() {
                        self.refresh_pin(Pin::Album(id), &tracks, false);
                    }
                }
                self.resume_library_analysis();
            }
            Msg::Analyzed { track_id, record } => {
                self.analysis.records.insert(track_id, record);
                self.analysis.version += 1;
            }
            Msg::AnalysesLoaded(rows) => {
                self.analysis.records.extend(rows);
                self.analysis.version += 1;
                self.analysis.loaded = true;
                self.resume_library_analysis();
            }
            Msg::TrackSync(p) => {
                self.sync.tracks = true;
                self.sync.progress = Some(p);
                self.sync.label = p.label();
            }
            Msg::TrackSyncDone(outcome) => {
                self.sync.tracks = false;
                self.sync.progress = None;
                let albums = self.data.albums.len();
                let tracks = self.data.tracks.all.len();
                self.sync.label = match outcome {
                    crate::sync::Outcome::Done { .. } => {
                        self.sync.last = Some(chrono::Local::now());
                        format!("{albums} albums · {tracks} tracks")
                    }
                    crate::sync::Outcome::Cancelled => format!("{albums} albums · {tracks} tracks · sync stopped"),
                    crate::sync::Outcome::Failed(e) => {
                        self.show_error(e);
                        format!("{albums} albums · {tracks} tracks")
                    }
                };
            }
            Msg::AlbumUpdated(a) => self.data.update_album(a),
            Msg::SyncProgress(n) => {
                self.sync.albums = true;
                self.sync.label = format!("syncing… {n}");
            }
            Msg::AlbumSyncDone(Ok(n)) => {
                self.sync.albums = false;
                self.sync.label = format!("{n} albums");
                self.sync.tracks = true;
            }
            Msg::AlbumSyncDone(Err(e)) => {
                // Nothing more follows (no artist or track sync): stop saying "syncing".
                self.sync.albums = false;
                self.sync.label = "sync failed".into();
                self.probe();
                if self.session.online {
                    self.show_error(e);
                }
            }
            Msg::LoadFailed { key, error } => {
                // Whatever waited for these tracks gives up, so it can't fire
                // much later when they turn up for some other reason.
                self.pending.retain(|p| p.key != key);
                self.downloads.pending_pins.retain(|(k, _)| *k != key);
                if let Some(e) = error {
                    self.probe();
                    if self.session.online {
                        self.show_error(e);
                    }
                }
            }
            Msg::Tracks { album_id, tracks, fresh } => {
                self.refresh_pin(Pin::Album(album_id.clone()), &tracks, fresh);
                self.data.tracks.set_album(&album_id, tracks);
                self.run_pending(&ListKey::Album(album_id));
            }
            Msg::Newest(ids) => {
                self.data.newest = ids;
                self.data.version += 1;
            }
            Msg::Playlists(p) => self.data.playlists = p,
            Msg::PlaylistTracks { id, tracks } => {
                // Always the server's complete listing.
                self.refresh_pin(Pin::Playlist(id.clone()), &tracks, true);
                self.data.playlist_tracks.insert(id.clone(), Arc::new(tracks));
                self.run_pending(&ListKey::Playlist(id));
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
            Msg::Lyrics { track_id, lyrics } => {
                self.lyrics.insert(track_id, crate::lyrics::Fetch::Done(lyrics));
            }
            Msg::SongStats { track_id, stats } => {
                self.song_stats.insert(track_id, stats);
            }
            Msg::UserMeta { meta, tracks } => self.favorites.note(meta, &tracks),
            Msg::StarDone { kind, id, on, seq, error } => self.star_done(kind, id, on, seq, error),
            Msg::RatingDone { id, previous, seq, error } => self.rating_done(id, previous, seq, error),
            Msg::PlaylistCreated { id, name } => {
                self.recent_playlists.insert(0, id.clone());
                self.recent_playlists.truncate(3);
                self.go(Route::Playlist { id, name });
            }
            Msg::Menu(cmd) => self.command(cmd),
            Msg::Error(e) => {
                self.probe();
                // Offline, network errors are expected and the offline pill already says so.
                if self.session.online {
                    self.show_error(e);
                }
            }
        }
    }

    /// Shows an error. Every message gets its own toast, so two failures in
    /// one batch of messages are both seen.
    pub(crate) fn show_error(&mut self, text: impl Into<String>) {
        self.toasts.push(text.into(), true);
    }

    /// Shows something worth knowing that isn't an error.
    pub(crate) fn show_notice(&mut self, text: impl Into<String>) {
        self.toasts.push(text.into(), false);
    }

    /// Something failed; if it was the network, find out whether the server is gone.
    fn probe(&self) {
        if self.session.online
            && let (Some(client), Some(settings)) = (&self.session.client, &self.session.settings)
        {
            self.bridge.probe(client.clone(), settings.clone());
        }
    }

    /// The server's gone: browse what's stored, play what's downloaded.
    fn set_offline(&mut self) {
        self.session.online = false;
        self.session.client = None;
        self.downloads.store.set_source(None);
        self.sync.tracks = false;
        self.sync.albums = false;
        self.sync.progress = None;
    }

    /// Pins `pin` if it was waiting for these tracks, or refreshes its track
    /// list if it's already pinned. Only `complete` lists (the server's own
    /// answer for that album or playlist) may drop tracks and delete their
    /// downloads; anything else (the local library, which may be mid-sync or
    /// rebuilt) can only add.
    fn refresh_pin(&mut self, pin: Pin, tracks: &[Track], complete: bool) {
        let label = self.pin_label(&pin);
        let refs = || tracks.iter().map(track_ref).collect::<Vec<_>>();
        if let Some(at) = self.downloads.pending_pins.iter().position(|(_, p)| p == &pin) {
            self.downloads.pending_pins.remove(at);
            self.downloads.store.pin(pin, label, refs());
        } else if self.downloads.store.is_pinned(&pin) {
            if complete {
                self.downloads.store.update(&pin, label, refs());
            } else {
                self.downloads.store.extend(&pin, refs());
            }
        }
    }

    fn pin_label(&self, pin: &Pin) -> String {
        match pin {
            Pin::Album(id) => self.data.album(id).map(|a| a.name.clone()).unwrap_or_default(),
            Pin::Playlist(id) => self.data.playlists.iter().find(|p| &p.id == id).map(|p| p.name.clone()).unwrap_or_default(),
            Pin::Track(id) => self.data.track(id).map(|t| t.title.clone()).unwrap_or_default(),
        }
    }

    /// Whether `track` can play right now: downloaded, or the server is there.
    pub fn playable(&self, track: &Track) -> bool {
        self.session.online || self.downloads.snap.is_done(&track.id)
    }

    /// In the full window: shows the sidebar on `tab`, or back to Up Next when
    /// it's already showing that tab.
    fn toggle_side_tab(&mut self, tab: QueueTab) {
        if self.queue_ui.tab == tab && (self.show_queue() || self.queue_sheet) {
            self.queue_ui.tab = QueueTab::UpNext;
            return;
        }
        self.queue_ui.tab = tab;
        if self.queue_narrow {
            self.queue_sheet = true;
        } else if !self.show_queue() {
            self.actions.push(Action::ToggleQueue);
        }
    }

    fn keep_offline(&mut self, what: Keep, on: bool) {
        // An album or playlist is pinned whole, once its track list is known.
        let (key, pin, refs) = match what {
            Keep::Album(id) => {
                let refs = self.data.tracks.album(&id).map(|t| t.iter().map(track_ref).collect::<Vec<_>>());
                (ListKey::Album(id.clone()), Pin::Album(id), refs)
            }
            Keep::Playlist(id) => {
                let refs = self.data.playlist_tracks.get(&id).map(|t| t.iter().map(track_ref).collect::<Vec<_>>());
                (ListKey::Playlist(id.clone()), Pin::Playlist(id), refs)
            }
            // One pin per song, saved together: a big selection doesn't stall.
            Keep::Tracks(tracks) => {
                if on {
                    self.downloads.store.pin_many(tracks.iter().map(|t| (Pin::Track(t.id.clone()), t.title.clone(), vec![track_ref(t)])).collect());
                } else {
                    self.downloads.store.unpin_many(&tracks.iter().map(|t| Pin::Track(t.id.clone())).collect::<Vec<_>>());
                }
                self.downloads.refresh();
                return;
            }
        };
        match refs {
            _ if !on => self.downloads.store.unpin(&pin),
            Some(refs) => self.downloads.store.pin(pin.clone(), self.pin_label(&pin), refs),
            None => {
                self.wait_to_pin(key.clone(), pin);
                match key {
                    ListKey::Album(id) => self.bridge.open_album(self.session.client.clone(), id),
                    ListKey::Playlist(id) => self.bridge.open_playlist(self.session.client.clone(), id),
                    ListKey::Artist(_) => {}
                }
            }
        }
        self.downloads.refresh();
    }

    /// A media key, Control Center or headphone command.
    fn remote(&mut self, r: Remote) {
        self.with_player(|p, cx| p.remote(r, cx));
    }

    /// Work owed once per batch of messages: a sync delivers albums, artists
    /// and tracks back to back, so derived data is rebuilt once, not per message.
    fn after_messages(&mut self) {
        if std::mem::take(&mut self.search.stale) {
            self.search.generation += 1;
            self.bridge.build_search(self.search.generation, self.data.albums.clone(), self.data.artists.clone(), Arc::clone(&self.data.tracks.all));
        }
    }

    /// Builds the index here and now (fixtures and tests).
    #[cfg(test)]
    fn rebuild_search(&mut self) {
        self.install_search(SearchIndex::new(&self.data.albums, &self.data.artists, &self.data.tracks.all));
    }

    /// Swaps in a freshly built index; an open palette re-runs its query on it.
    fn install_search(&mut self, mut index: SearchIndex) {
        if let Some(p) = self.palette.as_mut() {
            p.refresh(&mut index);
        }
        self.search.index = Some(index);
    }

    fn handle_engine(&mut self, ev: audio::Event) {
        if let Some(mode) = self.output.handle(&ev) {
            // The engine picked the mode for a new device (remembered, or its default).
            self.bit_perfect = mode == PlaybackMode::BitPerfect;
        }
        self.with_player(|p, cx| p.handle_engine(&ev, cx));
    }

    /// Runs `f` on the player with the engine and the app's sources, then acts
    /// on what it reports.
    fn with_player<R>(&mut self, f: impl FnOnce(&mut Player, &mut player::Ctx<'_>) -> R) -> R {
        let mut notes = Vec::new();
        let sources = AppSources {
            offline: &self.downloads.store,
            client: self.session.live(),
            output: &self.output,
            data: &self.data,
            now_playing: &self.now_playing,
        };
        let result = f(&mut self.player, &mut player::Ctx { engine: &self.engine, sources: &sources, notes: &mut notes });
        for note in notes {
            match note {
                Note::Notice(text) => self.show_notice(text),
                Note::Error(text) => self.show_error(text),
                Note::Probe => self.probe(),
                Note::Started { track, restart } => {
                    if self.analyze_playing() && !restart {
                        self.enqueue_analysis(std::slice::from_ref(&*track));
                    }
                }
            }
        }
        result
    }

    fn start_current(&mut self) {
        self.with_player(|p, cx| p.start_current(cx));
    }

    /// Where `track` would play from now (tests).
    #[cfg(test)]
    fn source(&self, token: u64, prev: Option<&Track>, track: &Track, next: Option<&Track>) -> Option<TrackSource> {
        use player::Sources;
        let sources = AppSources {
            offline: &self.downloads.store,
            client: self.session.live(),
            output: &self.output,
            data: &self.data,
            now_playing: &self.now_playing,
        };
        sources.source(token, prev, track, next)
    }

    fn sync_next(&mut self) {
        self.with_player(|p, cx| p.sync_next(cx));
    }

    fn position_secs(&self) -> f64 {
        self.player.position_secs(&self.engine)
    }

    fn duration_secs(&self) -> Option<f64> {
        self.player.duration_secs(&self.engine)
    }

    fn play_pause(&mut self) {
        self.with_player(|p, cx| p.play_pause(cx));
    }

    fn next(&mut self) {
        self.with_player(|p, cx| p.next(cx));
    }

    fn previous(&mut self) {
        self.with_player(|p, cx| p.previous(cx));
    }

    fn seek(&mut self, to: f64) {
        self.with_player(|p, cx| p.seek(to, cx));
    }

    /// Applies a queue action once the tracks are known.
    fn queue_tracks(&mut self, tracks: Vec<Track>, source: Source, action: QueueAction) {
        self.with_player(|p, cx| p.queue_tracks(tracks, source, action, cx));
    }

    /// Holds `p` until its tracks load. A newer action for the same list
    /// replaces an older one still waiting.
    fn wait_for(&mut self, p: Pending) {
        self.pending.retain(|q| q.key != p.key);
        self.pending.push(p);
    }

    /// Pins `pin` once its tracks load (once, however often it's asked).
    fn wait_to_pin(&mut self, key: ListKey, pin: Pin) {
        if !self.downloads.pending_pins.iter().any(|(k, _)| *k == key) {
            self.downloads.pending_pins.push((key, pin));
        }
    }

    fn run_pending(&mut self, key: &ListKey) {
        let Some(at) = self.pending.iter().position(|p| &p.key == key) else { return };
        let p = self.pending.remove(at);
        let tracks = match &p.source {
            Source::Album { id, .. } => self.data.tracks.album(id).map(|s| s.to_vec()),
            Source::Playlist { id, .. } => self.data.playlist_tracks.get(id).map(|v| v.to_vec()),
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
                match self.data.tracks.album(&id) {
                    Some(t) => self.queue_tracks(t.to_vec(), source, action),
                    None => {
                        self.wait_for(Pending { key: ListKey::Album(id.clone()), source, action });
                        self.bridge.open_album(self.session.client.clone(), id);
                    }
                }
            }
            Action::Artist(name, action) => {
                let ids: Vec<String> = self.data.albums.iter().filter(|a| a.has_artist(None, &name)).map(|a| a.id.clone()).collect();
                let key = ListKey::Artist(name.clone());
                let source = match action {
                    QueueAction::PlayNext | QueueAction::Add => Source::Added,
                    _ => Source::ArtistShuffle { name: format!("{name}, shuffled") },
                };
                // Playing an artist means all of their albums, shuffled.
                let action = if matches!(action, QueueAction::Play { .. }) { QueueAction::Shuffle } else { action };
                self.wait_for(Pending { key: key.clone(), source, action });
                self.bridge.fetch_tracks(self.session.client.clone(), key, ids);
            }
            Action::Playlist(id, action) => {
                let name = self.data.playlists.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_default();
                let source = Source::Playlist { id: id.clone(), name };
                match self.data.playlist_tracks.get(&id) {
                    Some(t) => self.queue_tracks(t.to_vec(), source, action),
                    None => {
                        self.wait_for(Pending { key: ListKey::Playlist(id.clone()), source, action });
                        self.bridge.open_playlist(self.session.client.clone(), id);
                    }
                }
            }
            Action::Tracks { tracks, source, action } => self.queue_tracks(tracks, source, action),
            Action::JumpTo(uid) => {
                if self.player.queue.jump_to(uid) {
                    self.start_current();
                }
            }
            Action::Replay(uid) => {
                if self.player.queue.replay(uid) {
                    self.start_current();
                }
            }
            Action::Remove(uid) => {
                if self.queue_ui.selected == Some(uid) {
                    // Keep a selection for repeated ⌫.
                    let at = self.player.queue.position(uid).unwrap_or(0);
                    self.player.queue.remove(uid);
                    self.queue_ui.selected = self.player.queue.upcoming.get(at).or(self.player.queue.upcoming.last()).map(|i| i.uid);
                } else {
                    self.player.queue.remove(uid);
                }
                self.sync_next();
            }
            Action::Move(uid, to) => {
                self.player.queue.move_to(uid, to);
                self.sync_next();
            }
            Action::MoveBy(uid, delta) => {
                self.player.queue.move_by(uid, delta);
                self.sync_next();
            }
            Action::ClearQueue => {
                self.player.queue.clear();
                self.sync_next();
            }
            Action::SetEnd(end) => {
                self.player.queue.end = end;
                self.sync_next();
            }
            Action::PlayPause => self.play_pause(),
            Action::Next => self.next(),
            Action::Previous => self.previous(),
            Action::Seek(to) => self.seek(to),
            Action::OpenPalette => {
                if self.palette.is_none() && self.search.index.is_some() {
                    self.palette = Some(Palette::default());
                }
            }
            Action::SetTheme(pref) => {
                theme::set_pref(&self.ctx, pref);
                self.prefs.edit(|s| s.theme = pref);
            }
            Action::ToggleQueue if self.queue_narrow => self.queue_sheet = !self.queue_sheet,
            Action::ToggleQueue => self.prefs.edit(|s| s.show_queue = !s.show_queue),
            Action::SignOut => self.sign_out(),
            Action::SetMode(mode) => self.set_mode(Some(mode)),
            Action::ToggleMini => self.set_mode(None),
            Action::TogglePlayer => {
                let plan = self.windows.toggle_player(self.seen);
                self.follow_plan(plan);
            }
            Action::Lyrics => match self.windows.mode {
                WindowMode::Full => self.toggle_side_tab(QueueTab::Lyrics),
                WindowMode::Player => self.actions.push(Action::SetLyricsDrawer(!self.windows.lyrics_open)),
                WindowMode::Mini => {
                    self.windows.lyrics_open = true;
                    self.set_mode(Some(WindowMode::Player));
                }
            },
            Action::Info => match self.windows.mode {
                WindowMode::Full => self.toggle_side_tab(QueueTab::Info),
                WindowMode::Player => {
                    self.queue_ui.tab = if self.queue_ui.tab == QueueTab::Info { QueueTab::UpNext } else { QueueTab::Info };
                }
                WindowMode::Mini => {
                    self.queue_ui.tab = QueueTab::Info;
                    self.set_mode(Some(WindowMode::Player));
                }
            },
            Action::SetLyricsDrawer(open) => {
                let plan = self.windows.set_lyrics(open, self.seen);
                self.follow_plan(plan);
                self.save_windows();
            }
            Action::OpenSettings(section) => {
                self.settings_section = section;
                self.set_mode(Some(WindowMode::Full));
                self.overlay = Some(Overlay::Settings);
            }
            Action::SyncNow => {
                if let Some(client) = self.session.client.clone()
                    && self.session.online
                    && !self.sync.albums
                    && !self.sync.tracks
                {
                    self.sync.label = "syncing…".into();
                    self.sync.albums = true;
                    self.bridge.sync_albums(client);
                }
            }
            Action::SetPlaybackPrefs(prefs) => self.output.set_prefs(prefs, &self.engine),
            Action::SetDeviceMode(uid, mode) => {
                if self.output.set_device_mode(&uid, mode, &self.engine) {
                    self.bit_perfect = mode == PlaybackMode::BitPerfect;
                }
            }
            Action::SetScrobble(on) => {
                self.now_playing.set_scrobbling(on);
                self.prefs.edit(|s| s.scrobble = on);
            }
            Action::SetDiagnostics(on) => self.prefs.edit(|s| s.diagnostics_bar = on),
            Action::KeepOffline(what, on) => self.keep_offline(what, on),
            a @ (Action::Unpin(_) | Action::PauseDownloads(_) | Action::RetryDownloads) => self.downloads.apply(a),
            Action::SetOfflineLimit(bytes) => {
                self.downloads.store.set_limit(bytes);
                self.prefs.edit(|s| s.offline_limit_gb = bytes.map(|b| b / 1_000_000_000));
            }
            Action::RevealOffline => {
                let dir = self.downloads.store.dir().to_owned();
                if let Err(e) = std::process::Command::new("open").arg(&dir).spawn() {
                    self.show_error(format!("open {}: {e}", dir.display()));
                }
            }
            Action::MoveOffline => {
                let current = self.downloads.store.dir().to_owned();
                let start = current.parent().unwrap_or(&current).to_owned();
                if let Some(chosen) = crate::folder::pick("Choose where to keep downloaded music", &start) {
                    // Always a folder of our own inside the chosen one: the store
                    // tidies files it doesn't know, which must never be the user's.
                    let dir = if chosen.file_name().is_some_and(|n| n == "Listener Offline") { chosen } else { chosen.join("Listener Offline") };
                    if dir != current {
                        self.prefs.edit(|s| {
                            s.offline_dir = Some(dir.clone());
                            s.offline_move_from = Some(current.clone());
                        });
                        self.show_notice(format!("Downloads will move to {} when Listener restarts", dir.display()));
                    }
                }
            }
            Action::Analyze(tracks) => {
                self.analysis.runner.set_paused(false);
                self.enqueue_analysis(&tracks);
            }
            Action::AnalyzeLibrary => {
                self.prefs.edit(|s| s.analysis_resume = true);
                self.analysis.runner.set_paused(false);
                self.resume_library_analysis();
            }
            Action::PauseAnalysis(paused) => self.analysis.runner.set_paused(paused),
            Action::StopAnalysis => {
                self.analysis.runner.clear();
                self.analysis.runner.set_paused(false);
                self.finish_library_analysis();
            }
            Action::SetAnalyzePlaying(on) => self.prefs.edit(|s| s.analyze_playing = on),
            Action::ToggleStar { kind, id, track, name } => self.toggle_star(kind, id, track.map(|t| *t), name),
            Action::Rate(id, rating) => self.rate(id, rating),
            Action::Command(cmd) => self.command(cmd),
            Action::Overlay(o) => self.overlay = Some(o),
            Action::PlaylistEdit(edit) => self.playlist_action(edit),
        }
    }

    /// A track's analysis, if there is one for the file as it is now.
    pub fn analysis_of(&self, t: &Track) -> Option<&analysis::TrackAnalysis> {
        self.analysis.of(t)
    }

    /// Queues the tracks that have no current analysis; returns how many.
    /// Downloaded copies are read straight from disk (and need no server).
    fn enqueue_analysis(&self, tracks: &[Track]) -> usize {
        let jobs: Vec<analyze::Job> = tracks
            .iter()
            .filter(|t| self.analysis_of(t).is_none())
            .filter_map(|t| {
                let local = self.downloads.store.local_path(&t.id);
                let url = match &self.session.client {
                    Some(c) => c.stream_url(&t.id).to_string(),
                    None if local.is_some() => String::new(),
                    None => return None,
                };
                Some(analyze::job_for(t, url, local))
            })
            .collect();
        self.analysis.runner.enqueue(jobs)
    }

    /// Queues the whole library, once the tracks, the stored results and the
    /// server are all there (so finished tracks aren't redone).
    fn resume_library_analysis(&mut self) {
        if self.analysis_library() && self.analysis.loaded && self.session.client.is_some() && !self.data.tracks.all.is_empty() {
            let all = self.data.tracks.all.clone();
            self.enqueue_analysis(&all);
            if !self.analysis.runner.progress().active() {
                self.finish_library_analysis();
            }
        }
    }

    fn finish_library_analysis(&mut self) {
        if self.analysis_library() {
            self.prefs.edit(|s| s.analysis_resume = false);
        }
    }

    /// A library-wide analysis should be running (resumed after a restart).
    fn analysis_library(&self) -> bool {
        self.prefs.get(|s| s.analysis_resume)
    }

    /// Analyse each track as it plays (it's in the playback cache by then).
    pub fn analyze_playing(&self) -> bool {
        self.prefs.get(|s| s.analyze_playing)
    }

    /// Whether the queue column is open (in windows wide enough to dock it).
    pub fn show_queue(&self) -> bool {
        self.prefs.get(|s| s.show_queue)
    }

    pub fn theme(&self) -> ThemePref {
        self.prefs.get(|s| s.theme)
    }

    pub fn scrobble(&self) -> bool {
        self.prefs.get(|s| s.scrobble)
    }

    pub fn diagnostics_shown(&self) -> bool {
        self.prefs.get(|s| s.diagnostics_bar)
    }

    /// Software volume, 0..=1.
    pub fn volume(&self) -> f32 {
        self.prefs.get(|s| s.volume)
    }

    /// Sets the volume now; the preference is saved once the change settles.
    pub fn set_volume(&mut self, volume: f32) {
        let volume = volume.clamp(0.0, 1.0);
        self.engine.send(Command::SetVolume(volume));
        self.prefs.edit(|s| s.volume = volume);
    }

    /// Saves the window modes and where each was.
    fn save_windows(&self) {
        let w = self.windows.clone();
        self.prefs.edit(|s| s.window = w);
    }

    /// Tracks in the library without a current analysis, and their total length.
    /// Counted once per change to either, not every frame Settings is open.
    pub fn unanalysed(&mut self) -> (usize, f64) {
        let key = (self.analysis.version, self.data.tracks_version);
        let (analyses, tracks) = (&self.analysis.records, &self.data.tracks.all);
        *self.analysis.unanalysed.get(key, || {
            tracks
                .iter()
                .filter(|t| !analyses.get(&t.id).is_some_and(|r| r.stamp == crate::analyze::stamp_of(t)))
                .fold((0, 0.0), |(n, secs), t| (n + 1, secs + f64::from(t.duration_secs.unwrap_or(0))))
        })
    }

    /// Switches window mode (`None` toggles Mini) and saves the choice.
    fn set_mode(&mut self, to: Option<WindowMode>) {
        let plan = match to {
            Some(m) => self.windows.switch(m, self.seen),
            None => self.windows.toggle_mini(self.seen),
        };
        self.follow_plan(plan);
    }

    /// Resizes the window to `plan` (if any) and saves the window state.
    fn follow_plan(&mut self, plan: Option<crate::window::Plan>) {
        let ctx = self.ctx.clone();
        if let Some(plan) = plan {
            plan.apply(&ctx);
            // Until the window reports its new size, don't mistake the old one for it.
            self.seen = None;
            self.settling = Some((plan.size, 30));
            self.switch_started = Some(std::time::Instant::now());
            tracing::debug!(target: "listener::window", mode = ?plan.mode, size = ?plan.size, "window mode switch");
            self.save_windows();
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
            Route::Album(id) => self.bridge.open_album(self.session.client.clone(), id),
            Route::Artist { name, .. } => self.bridge.find_credited(name),
            Route::Playlist { id, .. } => {
                // Offline, a kept playlist is rebuilt from its pin and the stored tracks.
                if !self.session.online
                    && !self.data.playlist_tracks.contains_key(&id)
                    && let Some((_, ids)) = self.downloads.store.pin_tracks(&Pin::Playlist(id.clone()))
                {
                    let by_id: HashMap<&str, &Track> = self.data.tracks.all.iter().map(|t| (t.id.as_str(), t)).collect();
                    let tracks: Vec<Track> = ids.iter().filter_map(|i| by_id.get(i.as_str()).map(|t| (*t).clone())).collect();
                    self.data.playlist_tracks.insert(id.clone(), Arc::new(tracks));
                }
                self.bridge.open_playlist(self.session.client.clone(), id)
            }
            Route::Favorites => self.bridge.fetch_starred(self.session.client.clone()),
            _ => {}
        }
    }

    /// Stops playback, forgets the saved password and shows the login screen
    /// with the server and username filled in.
    fn sign_out(&mut self) {
        self.with_player(|p, cx| p.reset(cx));
        self.palette = None;
        self.overlay = None;
        self.favorites = Favorites::default();
        self.bridge.cancel_sync();
        // Reconnect loops and connects in flight stop; anything they still
        // deliver is ignored. Downloads lose their server (and its credentials).
        self.bridge.end_session();
        self.set_offline();
        self.session.unreachable = false;
        self.session.server_label.clear();
        let saved = self.prefs.snapshot();
        if self.prefs.has_login() {
            let s = saved.clone();
            std::thread::spawn(move || s.forget_password());
        }
        self.prefs.edit(|s| s.signed_out = true);
        self.login = Some(LoginForm { server: saved.server, username: saved.username, ..Default::default() });
    }

    fn login_ui(&mut self, ui: &mut egui::Ui) {
        let Some(form) = self.login.as_mut() else { return };
        let mut submit = false;
        let t = Tokens::of(ui);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg)).show(ui, |ui| {
            // The top strip is the title bar here too.
            let strip = egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(ui.max_rect().width(), toolbar::TOOLBAR_H));
            let bar = ui.interact(strip, ui.scope_id().with("login-titlebar"), egui::Sense::click_and_drag());
            if bar.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if bar.double_clicked() {
                crate::chrome::title_double_click(ui.ctx());
            }
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
                ..self.prefs.snapshot()
            };
            self.bridge.connect(settings, Some(std::mem::take(&mut form.password)));
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

    /// The height our bar gives the traffic lights to centre in; `None` hides them.
    fn title_bar_h(&self) -> Option<f32> {
        match self.windows.mode {
            WindowMode::Full => Some(toolbar::TOOLBAR_H),
            WindowMode::Player => Some(now_playing_view::PLAYER_BAR_H),
            WindowMode::Mini => None,
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
            WindowMode::Player => self.player_ui(ui),
            WindowMode::Mini => self.mini_ui(ui),
        }
        // Mini is all artwork; messages wait for a bigger window.
        if self.windows.mode != WindowMode::Mini {
            self.toasts.ui(&ctx, ctx.content_rect());
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
        // over it, and below that the sidebar becomes a rail of icons. The
        // columns are as wide as the user dragged them.
        let plan = self.columns.plan(width);
        self.queue_narrow = !plan.queue_docked;
        if !self.queue_narrow {
            self.queue_sheet = false;
        }

        egui::Panel::top("toolbar")
            .exact_size(toolbar::TOOLBAR_H)
            .resizable(false)
            .frame(egui::Frame::new().fill(t.bg).inner_margin(egui::Margin::symmetric(12, 0)))
            .show(ui, |ui| self.toolbar(ui));
        if self.diagnostics_shown() {
            egui::Panel::bottom("diagnostics")
                .exact_size(diagnostics::BAR_H)
                .resizable(false)
                .frame(egui::Frame::new().fill(t.sidebar).inner_margin(egui::Margin::symmetric(12, 0)))
                .show(ui, |ui| self.diagnostics_bar(ui));
        }
        let left_panel = match plan.left {
            None => {
                egui::Panel::left("sidebar-rail")
                    .exact_size(sidebar::RAIL_W)
                    .resizable(false)
                    .frame(egui::Frame::new().fill(t.sidebar).inner_margin(egui::Margin { left: 4, right: 4, top: 14, bottom: 10 }))
                    .show(ui, |ui| self.sidebar_rail(ui));
                None
            }
            Some(w) => Some(
                egui::Panel::left("sidebar")
                    .exact_size(w)
                    .resizable(false)
                    .frame(egui::Frame::new().fill(t.sidebar).inner_margin(egui::Margin { left: 10, right: 10, top: 14, bottom: 10 }))
                    .show(ui, |ui| {
                        ui.set_max_width(ui.available_width());
                        self.sidebar(ui)
                    })
                    .response
                    .rect,
            ),
        };
        let right_panel = (self.show_queue() && !self.queue_narrow).then(|| {
            egui::Panel::right("queue")
                .exact_size(self.columns.right)
                .resizable(false)
                .frame(egui::Frame::new().fill(t.panel).inner_margin(egui::Margin { left: 8, right: 8, top: 12, bottom: 8 }))
                .show(ui, |ui| {
                    ui.set_max_width(ui.available_width());
                    self.queue_panel(ui, true)
                })
                .response
                .rect
        });
        let content = egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.bg).inner_margin(egui::Margin { left: 28, right: 28, top: 12, bottom: 0 }))
            .show(ui, |ui| self.main_view(ui));

        // The column edges, after the content so they win the pointer.
        let mut save = false;
        if let Some(r) = left_panel {
            let d = layout::edge_handle(ui, egui::Id::unique("sidebar-edge"), r.right(), r.top(), r.bottom());
            let limit = self.columns.left_limit(width, plan.queue_docked && self.show_queue());
            if d.reset {
                self.columns.left = layout::LEFT_DEFAULT.min(limit);
                save = true;
            } else if d.dx != 0.0 {
                self.columns.left = (self.columns.left + d.dx).clamp(layout::LEFT_MIN, limit);
            }
            save |= d.released;
        }
        if let Some(r) = right_panel {
            let d = layout::edge_handle(ui, egui::Id::unique("queue-edge"), r.left(), r.top(), r.bottom());
            let limit = self.columns.right_limit(width);
            if d.reset {
                self.columns.right = layout::RIGHT_DEFAULT.min(limit);
                save = true;
            } else if d.dx != 0.0 {
                self.columns.right = (self.columns.right - d.dx).clamp(layout::RIGHT_MIN, limit);
            }
            save |= d.released;
        }
        if save {
            let c = self.columns;
            self.prefs.edit(|s| {
                s.sidebar_width = (c.left != layout::LEFT_DEFAULT).then_some(c.left);
                s.queue_width = (c.right != layout::RIGHT_DEFAULT).then_some(c.right);
            });
        }
        if self.queue_sheet {
            self.queue_sheet_ui(ui.ctx(), content.response.rect);
        }
    }

    /// The queue as a sheet sliding over the right of the content (narrow
    /// windows). Esc or a click outside closes it.
    fn queue_sheet_ui(&mut self, ctx: &egui::Context, content: egui::Rect) {
        let t = Tokens::of_ctx(ctx);
        let w = self.columns.right.min(content.width() - 24.0);
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
                self.queue_panel(ui, true);
            });
        if modal.should_close() {
            self.queue_sheet = false;
        }
    }
}

impl eframe::App for ListenerApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.inbox.try_recv() {
            self.handle(msg);
        }
        self.after_messages();
        let snap = self.downloads.store.snapshot();
        if snap.version != self.downloads.snap.version {
            // A download finished: what's next may now play from disk, or play at all.
            let done_changed = snap.tracks.values().filter(|s| matches!(s, offline::TrackState::Done { .. })).count()
                != self.downloads.snap.tracks.values().filter(|s| matches!(s, offline::TrackState::Done { .. })).count();
            self.downloads.snap = snap;
            if done_changed && !self.session.online {
                self.sync_next();
            }
        }
        if self.analysis_library() && self.analysis.loaded && !self.data.tracks.all.is_empty() && !self.analysis.runner.progress().active() {
            self.finish_library_analysis();
        }
        let queue = if self.queue_narrow { self.queue_sheet } else { self.show_queue() };
        let (theme, diagnostics) = self.prefs.get(|s| (s.theme, s.diagnostics_bar));
        if let Some(menu) = self.menu.as_mut() {
            menu.sync(queue, self.bit_perfect, theme);
            menu.set_diagnostics(diagnostics);
        }
        // The diagnostics bar keeps the server round trip fresh, but only while
        // something plays (idle stays idle).
        if self.diagnostics_shown()
            && self.session.online
            && self.engine.status.state() == PlayState::Playing
            && self.diagnostics.latency_stale()
            && let Some(client) = self.session.client.clone()
        {
            self.diagnostics.latency_asked();
            self.bridge.measure_latency(client);
        }
        // Only the progress readout needs a clock; idle costs nothing. Buffering
        // animates the seek bar and spinner, so it ticks faster.
        match self.engine.status.state() {
            PlayState::Playing => ctx.request_repaint_after(Duration::from_millis(250)),
            PlayState::Buffering | PlayState::Loading => ctx.request_repaint_after(Duration::from_millis(33)),
            _ if self.player.pending_seek.is_some() => ctx.request_repaint_after(Duration::from_millis(33)),
            _ => {}
        }
        crate::chrome::sync(ctx, crate::chrome::Bar { height: self.title_bar_h() });
        let square = self.windows.mode == WindowMode::Mini;
        // Until the window exists the platform can't lock it; keep asking.
        if self.square_wanted != Some(square) || (square && !self.square_native) {
            self.square_wanted = Some(square);
            self.square_native = crate::window::lock_square(square) && square;
        }
        if let Some(g) = Geometry::read(ctx) {
            match self.settling {
                Some((want, left)) if left > 0 && ((g.size[0] - want[0]).abs() > 2.0 || (g.size[1] - want[1]).abs() > 2.0) => {
                    self.settling = Some((want, left - 1));
                    ctx.request_repaint();
                }
                _ => {
                    if self.settling.take().is_some() {
                        // Our own resize has landed: put presentation back to normal
                        // (see `chrome::end_programmatic_resize`).
                        crate::chrome::end_programmatic_resize();
                        if let Some(t) = self.switch_started.take() {
                            tracing::debug!(target: "listener::window", elapsed_ms = t.elapsed().as_secs_f64() * 1000.0, size = ?g.size, "window mode switch settled");
                        }
                    }
                    // Mini is square; where the platform doesn't hold it so, snap back.
                    if square
                        && !self.square_native
                        && let Some(side) = crate::window::square(self.seen.map_or(g.size, |s| s.size), g.size)
                    {
                        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(side.into()));
                    }
                    self.seen = Some(g);
                }
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let started = std::time::Instant::now();
        if self.login.is_some() {
            // Logging in needs the full window.
            self.set_mode(Some(WindowMode::Full));
            self.login_ui(ui);
        } else {
            self.main_ui(ui);
        }
        self.diagnostics.note_frame(started.elapsed());
    }

    fn on_exit(&mut self) {
        if let Some(g) = self.seen {
            self.windows.remember(g);
        }
        self.save_windows();
        self.prefs.flush();
    }
}

impl Drop for ListenerApp {
    fn drop(&mut self) {
        self.bridge.cancel_sync();
        self.engine.send(Command::Shutdown);
    }
}

/// Fisher–Yates with a small xorshift; good enough for a play order.
/// Where queue items play from: a downloaded file first (instant, no
/// network), else the server while it's reachable (original or transcoded).
struct AppSources<'a> {
    offline: &'a OfflineStore,
    /// Only while the server answers.
    client: Option<&'a Client>,
    output: &'a Outputs,
    data: &'a Data,
    now_playing: &'a now_playing::Session,
}

impl player::Sources for AppSources<'_> {
    /// With its ReplayGain, and whether it plays as part of its album (from
    /// its queue neighbours `prev` and `next`).
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
        let client = self.client?;
        let transcode = self.output.transcode();
        let (url, suffix) = match &transcode {
            None => (client.stream_url(&track.id), track.suffix.clone()),
            Some(t) => (client.transcoded_stream_url(&track.id, &t.format, t.kbps), Some(t.format.clone())),
        };
        Some(TrackSource { id: track.id.clone(), url: url.to_string(), suffix, replay_gain, album_context, transcode, token })
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
            cover: self.data.cover_for(track).map(str::to_owned),
        });
    }
}

/// What the offline store needs to download `t`.
pub fn track_ref(t: &Track) -> TrackRef {
    TrackRef::new(t.id.clone(), t.suffix.clone(), t.size)
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
        app.data.playlist_tracks.insert("pl-1".into(), Arc::new(tracks[..3].to_vec()));
        app.data.credited.insert("Scott LaFaro".into(), vec!["al-1".into(), "al-0".into()]);
        app.player.queue.play_now(tracks.clone(), 2, Source::Album { id: "al-0".into(), name: "Album 0".into() });
        app.player.queue.play_next(tracks[..2].to_vec());
        app.player.queue.add(tracks[5..15].to_vec());
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
        app.session.online = false;
        app.player.queue.end = crate::queue::EndAction::RepeatQueue;
        let pass = app.player.queue.pass_len();
        app.start_current();
        assert!(app.player.queue.history.len() <= 2 * pass, "history grew to {}", app.player.queue.history.len());
        assert_eq!(app.toasts.texts().last().copied(), Some("Nothing in the queue is downloaded"));
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
        app.player.queue.advance();
        frame(&ctx, &mut app, Vec::new());
        app.prefs.edit(|s| s.show_queue = false);
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
        let before = app.player.queue.upcoming.len();
        let alt_enter = egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::ALT };
        frame(&ctx, &mut app, vec![alt_enter]);
        assert!(app.palette.is_none());
        // Album 12's tracks aren't loaded, so the action waits for them...
        assert_eq!(app.player.queue.upcoming.len(), before);
        assert_eq!(app.pending.len(), 1);
        // ...and runs when they arrive.
        let tracks: Vec<Track> = (1..=3).map(|n| track(&format!("x-{n}"), "al-12", n)).collect();
        app.handle(Msg::Tracks { album_id: "al-12".into(), tracks, fresh: true });
        assert_eq!(app.player.queue.upcoming.len(), before + 3);
        assert!(app.pending.is_empty());
        // Played next: right after what the user already chose to play next.
        assert_eq!(app.player.queue.upcoming[2].track.id, "x-1");
    }

    /// Handles what the bridge sends until `done` says so (or 3 s pass).
    fn until(app: &mut ListenerApp, mut done: impl FnMut(&ListenerApp) -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !done(app) {
            assert!(std::time::Instant::now() < deadline, "timed out waiting on the bridge");
            match app.inbox.recv_timeout(Duration::from_millis(20)) {
                Ok(msg) => app.handle(msg),
                Err(_) => continue,
            }
        }
    }

    #[test]
    fn the_search_index_is_built_off_the_ui_thread_and_stale_builds_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        let album = |id: &str, name: &str| Album {
            id: id.into(),
            name: name.into(),
            sort: None,
            artist: Some("Bill Evans".into()),
            artist_ref: None,
            year: None,
            genre: None,
            song_count: 0,
            duration_secs: 0,
            cover_art: None,
            created: None,
            labels: Vec::new(),
            genres: Vec::new(),
            artists: Vec::new(),
        };
        app.handle(Msg::Albums(vec![album("a1", "Portrait in Jazz")]));
        app.after_messages();
        assert!(!app.search.stale, "the rebuild was handed off");
        let first = app.search.generation;
        // Before the first build lands, the library changes again.
        app.handle(Msg::Albums(vec![album("a1", "Portrait in Jazz"), album("a2", "Explorations")]));
        app.after_messages();
        assert_eq!(app.search.generation, first + 1);
        until(&mut app, |a| a.search.index.as_ref().is_some_and(|s| s.albums.len() == 2));
        // The older build, if it lands now, doesn't replace the newer index.
        let stale = SearchIndex::new(&[album("x", "Old")], &[], &[]);
        app.handle(Msg::SearchIndex { generation: first, index: Box::new(stale) });
        assert_eq!(app.search.index.as_ref().map(|s| s.albums.len()), Some(2));
    }

    #[test]
    fn a_load_that_fails_drops_what_waited_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        assert!(app.session.client.is_none(), "offline: nothing can be fetched");
        let before = app.player.queue.upcoming.len();
        // Album 12 isn't stored either: the play and the pin both wait, once each.
        app.apply(Action::Album("al-12".into(), QueueAction::Add));
        app.apply(Action::Album("al-12".into(), QueueAction::PlayNext));
        app.apply(Action::KeepOffline(Keep::Album("al-12".into()), true));
        app.apply(Action::KeepOffline(Keep::Album("al-12".into()), true));
        assert_eq!(app.pending.len(), 1, "the newer action replaced the older");
        assert_eq!(app.downloads.pending_pins.len(), 1);
        // The load fails: both give up.
        until(&mut app, |a| a.pending.is_empty() && a.downloads.pending_pins.is_empty());
        // The tracks turn up later anyway (browsing, a sync): nothing plays or pins.
        let tracks: Vec<Track> = (1..=3).map(|n| track(&format!("x-{n}"), "al-12", n)).collect();
        app.handle(Msg::Tracks { album_id: "al-12".into(), tracks, fresh: true });
        assert_eq!(app.player.queue.upcoming.len(), before, "a stale action played");
        assert!(!app.downloads.store.is_pinned(&Pin::Album("al-12".into())), "a stale pin was made");
    }

    #[test]
    fn a_failed_album_sync_stops_saying_syncing() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        app.session.online = true;
        app.sync.albums = true;
        app.handle(Msg::AlbumSyncDone(Err("sync: timed out".into())));
        assert!(!app.sync.albums && !app.sync.tracks);
        assert!(app.toasts.texts().contains(&"sync: timed out"));
        // While an album sync runs, Sync now doesn't start a second one.
        app.sync.albums = true;
        app.session.client = Some(subsonic::Client::new("http://127.0.0.1:9", subsonic::Credentials::new("u", "p")).unwrap());
        app.sync.label = "syncing… 500".into();
        app.apply(Action::SyncNow);
        assert_eq!(app.sync.label, "syncing… 500", "a second album sync started");
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
        assert_eq!(app.windows.mode, WindowMode::Player);
        for _ in 0..2 {
            frame(&ctx, &mut app, vec![egui::Event::PointerMoved(egui::pos2(120.0, 60.0))]);
        }
        // Browsing goes back to the full window.
        app.actions.push(Action::Go(Route::Album("al-0".into())));
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.windows.mode, WindowMode::Full);
        // ⌘K from a small mode does too.
        app.windows.mode = WindowMode::Player;
        frame(&ctx, &mut app, vec![key(egui::Key::K, egui::Modifiers::COMMAND)]);
        assert_eq!(app.windows.mode, WindowMode::Full);
        assert!(app.palette.is_some());
    }

    #[test]
    fn lyrics_go_to_the_sidebar_or_the_players_drawer() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        frame(&ctx, &mut app, Vec::new());
        // Full window: the sidebar switches to its Lyrics tab, and back.
        app.apply(Action::Lyrics);
        assert_eq!(app.queue_ui.tab, QueueTab::Lyrics);
        app.apply(Action::Lyrics);
        assert_eq!(app.queue_ui.tab, QueueTab::UpNext);
        // The artwork toggles to the Player (nothing used yet), where ⌥⌘L opens the drawer.
        app.apply(Action::TogglePlayer);
        assert_eq!(app.windows.mode, WindowMode::Player);
        app.apply(Action::Lyrics);
        frame(&ctx, &mut app, Vec::new());
        assert!(app.windows.lyrics_open);
        // Mini, then back to full: the artwork now toggles to Mini.
        app.apply(Action::ToggleMini);
        app.apply(Action::SetMode(WindowMode::Full));
        app.apply(Action::TogglePlayer);
        assert_eq!(app.windows.mode, WindowMode::Mini);
        // Lyrics from Mini open the Player with its drawer.
        app.windows.lyrics_open = false;
        app.apply(Action::Lyrics);
        assert_eq!(app.windows.mode, WindowMode::Player);
        assert!(app.windows.lyrics_open);
        for _ in 0..2 {
            frame(&ctx, &mut app, Vec::new());
        }
    }

    /// Makes track `id` downloaded, the way a finished stream would: via a file
    /// the stream cache "has", which the store links in.
    fn download(app: &mut ListenerApp, dir: &std::path::Path, id: &str) {
        let file = dir.join(format!("cached-{id}.flac"));
        std::fs::write(&file, b"fLaC").unwrap();
        let want = id.to_owned();
        app.downloads.store.set_cache_lookup(Arc::new(move |r: &TrackRef| (r.id == want).then(|| file.clone())));
        let t = app.data.tracks.all.iter().find(|t| t.id == id).unwrap().clone();
        app.apply(Action::KeepOffline(Keep::Tracks(vec![t]), true));
        let start = std::time::Instant::now();
        while !app.downloads.store.snapshot().is_done(id) {
            assert!(start.elapsed() < Duration::from_secs(5), "download of {id}");
            std::thread::sleep(Duration::from_millis(5));
        }
        app.downloads.snap = app.downloads.store.snapshot();
    }

    #[test]
    fn offline_the_queue_plays_only_what_is_downloaded() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        download(&mut app, dir.path(), "t-2");
        assert!(!app.session.online);
        let tracks: Vec<Track> = app.data.tracks.by_album["al-0"][..4].to_vec();
        assert!(app.source(0, None, &tracks[0], None).is_none(), "offline and not downloaded: nothing to play");
        let local = app.source(0, None, &tracks[1], None).unwrap();
        assert!(local.url.starts_with("file://") && local.url.ends_with("t-2.flac"), "{}", local.url);

        // Playing from the top skips to the downloaded song and says so.
        app.player.queue = crate::queue::Queue::default();
        app.queue_tracks(tracks.clone(), Source::Album { id: "al-0".into(), name: "Album 0".into() }, QueueAction::Play { start: 0 });
        assert_eq!(app.player.queue.current.as_ref().unwrap().track.id, "t-2");
        assert_eq!(app.toasts.texts().last().copied(), Some("Skipped 1 song that isn't downloaded"));
        // Nothing after it can play, so the engine isn't told about a next track.
        assert_eq!(app.player.sent_next(), None);
        // When it ends, the rest is skipped and playback stops cleanly.
        app.handle_engine(audio::Event::Ended { track_id: "t-2".into(), token: 0 });
        assert!(app.player.queue.current.is_none());

        // Back online: everything plays again, downloaded songs still from disk.
        let client = subsonic::Client::new("http://127.0.0.1:9", subsonic::Credentials::new("u", "p")).unwrap();
        app.handle(Msg::Connected { session: app.bridge.session(), client, settings: Box::default(), server: "test".into() });
        assert!(app.session.online);
        assert!(app.source(0, None, &tracks[0], None).unwrap().url.contains("/rest/stream"));
        assert!(app.source(0, None, &tracks[1], None).unwrap().url.starts_with("file://"));
    }

    #[test]
    fn errors_in_one_batch_are_all_shown() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        app.session.online = true;
        // Two failures handled before the next frame: both get a toast.
        app.handle(Msg::Error("playlists: timed out".into()));
        app.handle(Msg::Error("favorites: timed out".into()));
        let shown = app.toasts.texts();
        assert!(shown.contains(&"playlists: timed out") && shown.contains(&"favorites: timed out"), "{shown:?}");
    }

    #[test]
    fn signing_out_ignores_a_reconnect_that_lands_afterwards() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        let client = || subsonic::Client::new("http://127.0.0.1:9", subsonic::Credentials::new("u", "p")).unwrap();
        // Signed in, then the server went away: a reconnect loop is running.
        let before = app.bridge.session();
        app.handle(Msg::Connected { session: before, client: client(), settings: Box::default(), server: "test".into() });
        assert!(app.session.online && app.session.client.is_some());
        app.handle(Msg::Unreachable { session: before, username: "u".into(), error: "gone".into() });
        assert!(app.session.unreachable);

        app.sign_out();
        assert!(app.login.is_some());
        assert!(app.session.client.is_none() && !app.session.online && !app.session.unreachable);
        assert_ne!(app.bridge.session(), before, "sign-out ends the session");

        // The old loop reaches the server after all: stale, ignored.
        app.handle(Msg::Connected { session: before, client: client(), settings: Box::default(), server: "test".into() });
        app.handle(Msg::Unreachable { session: before, username: "u".into(), error: "gone".into() });
        assert!(app.login.is_some(), "still on the login screen");
        assert!(app.session.client.is_none() && !app.session.online && !app.session.unreachable);

        // A fresh sign-in in the new session works.
        app.handle(Msg::Connected { session: app.bridge.session(), client: client(), settings: Box::default(), server: "test".into() });
        assert!(app.login.is_none() && app.session.online);
    }

    #[test]
    fn keeping_an_album_waits_for_its_tracks_and_the_offline_place_draws() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        // Album 5's tracks aren't loaded: the pin waits for them.
        app.apply(Action::KeepOffline(Keep::Album("al-5".into()), true));
        assert!(!app.downloads.store.is_pinned(&Pin::Album("al-5".into())));
        let tracks: Vec<Track> = (1..=3).map(|n| track(&format!("y-{n}"), "al-5", n)).collect();
        app.handle(Msg::Tracks { album_id: "al-5".into(), tracks: tracks.clone(), fresh: false });
        assert!(app.downloads.store.is_pinned(&Pin::Album("al-5".into())));
        assert_eq!(app.downloads.store.pin_tracks(&Pin::Album("al-5".into())).unwrap().1, ["y-1", "y-2", "y-3"]);
        // A partial list from the local library (mid-sync, rebuilt) never shrinks the pin.
        app.downloads.snap = app.downloads.store.snapshot();
        app.handle(Msg::Tracks { album_id: "al-5".into(), tracks: tracks[..1].to_vec(), fresh: false });
        // (The stored album now lists only y-1; a library-wide refresh sees that.)
        let all = app.data.tracks.all.to_vec();
        app.handle(Msg::AllTracks(Box::new(TrackSet::new(all))));
        assert_eq!(app.downloads.store.pin_tracks(&Pin::Album("al-5".into())).unwrap().1, ["y-1", "y-2", "y-3"]);
        // The album changed on the server: the pin follows.
        app.handle(Msg::Tracks { album_id: "al-5".into(), tracks: tracks[..2].to_vec(), fresh: true });
        assert_eq!(app.downloads.store.pin_tracks(&Pin::Album("al-5".into())).unwrap().1, ["y-1", "y-2"]);
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
        assert!(!app.downloads.store.is_pinned(&Pin::Album("al-5".into())));
        assert!(app.downloads.store.is_pinned(&Pin::Track("t-3".into())));
    }

    #[test]
    fn gapless_start_advances_the_queue() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        let next = app.player.queue.peek_next().unwrap().clone();
        let format = audio::StreamFormat { sample_rate: 44_100, channels: 2, bits_per_sample: Some(16) };
        let started = |i: &crate::queue::Item| audio::Event::TrackStarted {
            track_id: i.track.id.clone(),
            token: i.uid,
            format,
            gapless: true,
            restart: false,
        };
        app.handle_engine(started(&next));
        assert_eq!(app.player.queue.current.as_ref().unwrap().uid, next.uid);
        // A repeat of the same event (or the current track) changes nothing.
        let upcoming = app.player.queue.upcoming.len();
        app.handle_engine(started(&next));
        assert_eq!(app.player.queue.upcoming.len(), upcoming);
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
        let playing = app.player.queue.current.clone().unwrap();
        app.player.queue.clear();
        app.player.queue.play_next(vec![playing.track.clone()]);
        let copy = app.player.queue.peek_next().unwrap().clone();
        assert_eq!(copy.track.id, playing.track.id);
        assert_ne!(copy.uid, playing.uid);
        app.handle_engine(started(&copy));
        assert_eq!(app.player.queue.current.as_ref().unwrap().uid, copy.uid, "the queue moved to the copy");

        // Repeat queue over a single item: each gapless start is the next pass.
        app.player.queue = crate::queue::Queue::default();
        app.player.queue.end = crate::queue::EndAction::RepeatQueue;
        app.player.queue.play_now(vec![playing.track.clone()], 0, Source::Added);
        let first = app.player.queue.current.clone().unwrap();
        app.handle_engine(started(&first));
        let second = app.player.queue.current.clone().unwrap();
        assert_ne!(second.uid, first.uid, "a new pass began");
        assert_eq!(app.player.queue.history.len(), 1);
    }
}
