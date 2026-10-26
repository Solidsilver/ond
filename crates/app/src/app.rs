//! The main window: sidebar · toolbar with the now-playing display · the
//! current view · the queue column. Views are in submodules; they don't change
//! state directly but push [`Action`]s, which are applied once the frame is
//! drawn. That keeps borrows simple and makes every view reusable by the
//! compact layout.

mod album_page;
mod analyzing;
mod browse;
mod commands;
mod compact;
mod data;
mod diagnostics;
mod offline_page;
mod dialogs;
mod favorites_page;
mod info_panel;
mod layout;
mod now_playing_view;
mod palette;
mod prefs_view;
mod pins;
mod playing;
mod settings_page;
mod queue_panel;
mod seek;
mod session;
mod sidebar;
mod songs;
mod toasts;
mod toolbar;
mod track_row;
mod view_ctx;
mod visualizer_view;
mod widgets;
mod window_state;

#[cfg(test)]
mod snapshots;

use crate::analyze;
use crate::art::Art;
use crate::bridge::{Bridge, Msg};
use crate::favorites::{Favorites, StarKind};
use crate::keys::{Cmd, EditCmd};
use crate::nav::{Nav, Route};
use crate::now_playing;
use crate::output::Outputs;
use crate::player::{Player, QueueEdit};
use crate::prefs::{Loaded, Prefs};
use crate::queue::Source;
use crate::settings::Paths;
#[cfg(test)]
use {crate::bridge::ListKey, offline::TrackRef, pins::track_ref, crate::search::SearchIndex, crate::settings::Settings, crate::theme::ThemePref, crate::tracks::TrackSet, library::Album};
use crate::theme::{self, Tokens};
use crate::window::WindowMode;
use audio::{Command, EngineHandle, PlayState};
use library::{Library, Track};
use offline::Pin;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

pub use commands::PlaylistAction;
pub use dialogs::Overlay;
pub use palette::Palette;
pub use analyzing::{Analysis, AnalysisAction};
pub use data::{Data, SearchState, SyncState};
use data::Pending;
pub use pins::{Downloads, Keep, OfflineAction};
use pins::playable;
pub use playing::OutputAction;
pub use prefs_view::PrefsAction;
use prefs_view::PrefsView;
pub use session::Session;
pub use window_state::WindowAction;
use window_state::WindowState;
use session::LoginForm;
pub(crate) use widgets::{MODES, choice, elide as elide_text, link as link_text, mode_label};



pub use crate::player::QueueAction;

/// A state change asked for by a view, a key or the menu bar, applied after
/// the frame is drawn (see `main_ui`). Changes that belong to one part of the
/// app are grouped under it ([`WindowAction`], [`PrefsAction`],
/// [`OutputAction`], [`OfflineAction`], [`AnalysisAction`], the queue's
/// [`QueueEdit`]), and that part's module applies them.
pub enum Action {
    Go(Route),
    Back,
    Forward,
    BackBy(usize),
    /// An album's page with one of its tracks selected (⌘L: the playing one).
    Reveal { track: String, album: String },
    OpenPalette,
    /// Open the Settings window at a section.
    OpenSettings(settings_page::Section),
    /// Open a sheet (new playlist, rename, delete, shortcuts).
    Overlay(Overlay),
    /// Show the keyboard shortcuts in the full window, or close them again.
    ToggleShortcuts,
    /// Ask for a new playlist's name, in the full window.
    NewPlaylist,
    /// Open the visualizer, or close it (experimental; only while that's on).
    ToggleVisualizer,
    /// An Edit menu command, handed to egui as input so text fields get it.
    Edit(EditCmd),
    /// Show the lyrics: the sidebar's Lyrics tab in the full window; in the
    /// Player, open or close its drawer; from Mini, the Player with it open.
    Lyrics,
    /// Show (or hide again) the playing song's details: the Info tab of the
    /// sidebar, or of the Player's queue; from Mini, the Player on it.
    Info,
    /// The Info tab wants a song's play count (asked once per song).
    NeedSongStats(String),
    /// A lyrics view wants a song's lyrics (asked once per song).
    NeedLyrics(Box<Track>),
    /// Re-read the library from the server (albums, then tracks).
    SyncNow,
    /// Stop the track sync that's running.
    StopSync,
    /// Measure the server's round trip again, if the last reading is old.
    MeasureLatency,
    /// Back to the login screen; the saved password is forgotten.
    SignOut,
    Album(String, QueueAction),
    Artist(String, QueueAction),
    Playlist(String, QueueAction),
    Tracks { tracks: Vec<Track>, source: Source, action: QueueAction },
    /// A change to the queue itself: jump, replay, remove, move, clear, or
    /// what happens at its end.
    Queue(QueueEdit),
    PlayPause,
    Next,
    Previous,
    Seek(f64),
    /// Window modes, the Player's lyrics drawer, the title bar, the queue column.
    Window(WindowAction),
    /// Appearance, scrobbling, the diagnostics bar and experiments.
    Prefs(PrefsAction),
    /// ReplayGain and streaming, device modes, volume, bit-perfect.
    Output(OutputAction),
    /// Downloads.
    Offline(OfflineAction),
    /// Library analysis.
    Analysis(AnalysisAction),
    /// Love or unlove a song, album or artist. `track` / `name` keep the
    /// Favorites view current.
    ToggleStar { kind: StarKind, id: String, track: Option<Box<Track>>, name: Option<String> },
    /// Rate a song 1–5; 0 clears it.
    Rate(String, u8),
    /// A key or menu-bar command; applying it applies what it stands for.
    Command(Cmd),
    PlaylistEdit(PlaylistAction),
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
            QueueTab::UpNext => "Up next",
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

/// The app's tokio runtime, kept for the app's lifetime (the bridge and the
/// background services hold handles to it).
///
/// Dropping a `Runtime` waits for every `spawn_blocking` call still running,
/// however long. Quitting gives them [`SHUTDOWN_WAIT`] and then goes: the work
/// there (renames, copies, keychain calls) is written to survive being cut off.
struct AppRuntime(Option<tokio::runtime::Runtime>);

const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

impl Drop for AppRuntime {
    fn drop(&mut self) {
        if let Some(rt) = self.0.take() {
            rt.shutdown_timeout(SHUTDOWN_WAIT);
        }
    }
}

pub struct OndApp {
    _rt: AppRuntime,
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
    actions: Vec<Action>,
    pending: Vec<Pending>,
    art: Art,
    /// Played audio, kept for replay and seeking (also the engine's).
    cache: streamcache::StreamCache,

    /// The queue and what the engine is told about it.
    player: Player,

    /// Output devices and what reaches the one in use.
    output: Outputs,
    bit_perfect: bool,

    /// The palette's search index.
    search: SearchState,
    /// What the window shows and how: places, selections, sheets, toasts.
    ui: UiState,
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
    /// The native menu bar (macOS); kept alive here.
    menu: Option<crate::menu::MenuBar>,
    /// Lyrics by track id, fetched when a lyrics view first needs them.
    lyrics: HashMap<String, crate::lyrics::Fetch>,
    /// Play counts by track id for the Info tab: absent until asked for,
    /// `None` while loading or when the server couldn't say.
    song_stats: HashMap<String, Option<SongStats>>,
    /// The diagnostics bar: whether it's on, and what it measures.
    diagnostics: diagnostics::Diagnostics,
    /// Track waveforms for the seek bar (experimental).
    waveforms: crate::visuals::waveform::Waveforms,
    /// What the window renders to, for the visualizer's own GPU drawing;
    /// `None` without a wgpu renderer (it then uses the painter).
    gpu: Option<crate::visuals::gpu::Target>,
}

/// What the window shows and how: the place and its selections, open sheets
/// and panels, toasts, and the window's own bookkeeping. Nothing here is
/// library data or playback state.
pub struct UiState {
    pub nav: Nav,
    pub sort: AlbumSort,
    /// The album list for the current view, cached by (route, sort, data version).
    pub grid: crate::memo::Memo<(Route, AlbumSort, u64), Vec<usize>>,
    /// The album page's "Album details" is unfolded (for this session).
    pub album_details_open: bool,
    /// Narrow windows: the queue opens as a sheet over the content instead.
    pub queue_sheet: bool,
    /// The window is too narrow to dock the queue column (set each frame).
    pub queue_narrow: bool,
    pub queue_ui: QueueUi,
    pub focus: Focus,
    /// Keyboard selection in the album/playlist track list.
    pub track_sel: Option<usize>,
    /// Where the seek bar is being dragged to, in seconds.
    pub seek_preview: Option<f64>,
    pub songs: songs::SongsView,
    /// Errors and notices, shown as toasts.
    pub toasts: toasts::Toasts,
    pub palette: Option<Palette>,
    /// A sheet over the window, if one is open.
    pub overlay: Option<Overlay>,
    /// ⌘L: select (and scroll to) this track once its album is shown.
    pub reveal: Option<String>,
    /// Playlists added to lately, first in "Add to playlist".
    pub recent_playlists: Vec<String>,
    pub fav_tab: favorites_page::FavTab,
    /// The Settings window's section.
    pub settings_section: settings_page::Section,
    /// Whether the lyrics column follows the sung line.
    pub lyrics_follow: crate::lyrics::Follow,
    /// The Player's details strip is open.
    pub np_details: bool,
    /// The window's own bookkeeping: mode switches under way, Mini held square.
    pub window: WindowState,
    /// When the last scroll happened (frame clock), for showing scroll bars.
    pub last_scroll: f64,
    pub bars_shown: bool,
    /// Edit menu commands (paste, undo…) waiting to reach egui as input.
    pub edit_input: Vec<egui::Event>,
    /// Trackpad coasting where the system doesn't do it (Wayland).
    pub momentum: crate::momentum::Momentum,
    /// Frames asked for by visuals that move (experimental; see `visuals`).
    pub live: crate::visuals::Live,
    /// The colors of the now-playing display, the Player, Mini and the
    /// window around the page: the chosen accent, or the playing cover's.
    pub tint: crate::visuals::colors::Easing,
    /// The page's colors: an album page's own cover's, else the same as [`Self::tint`].
    pub page_tint: crate::visuals::colors::Easing,
    /// [`Self::page_tint`] as drawn this frame.
    pub page_tints: Option<crate::visuals::colors::Tints>,
    /// Each cover's tints for the chosen accent.
    pub tint_cache: crate::visuals::colors::TintCache,
    /// The accent egui's own widgets were last given.
    pub accent_applied: Option<theme::Accent>,
    /// The blurred cover behind the Player and Mini, fading between covers (experimental).
    pub backdrop: crate::visuals::backdrop::Crossfade,
    /// The playing audio's spectrum, for the live visuals (experimental).
    pub listener: crate::visuals::spectrum::Listener,
    /// The Info tab's spectrogram (experimental).
    pub spectrogram: crate::visuals::spectrogram::Spectrogram,
    /// The visualizer, while it's open (experimental).
    pub visualizer: Option<visualizer_view::VisualizerView>,
    /// What views work out from the library, kept between frames.
    pub memos: view_ctx::Memos,
    /// The preferences as of this frame (see [`PrefsView`]).
    pub prefs: PrefsView,
}

impl UiState {
    fn new(notices: Vec<String>, prefs: PrefsView) -> Self {
        Self {
            prefs,
            nav: Nav::default(),
            sort: AlbumSort::default(),
            grid: Default::default(),
            album_details_open: false,
            queue_sheet: false,
            queue_narrow: false,
            queue_ui: QueueUi::default(),
            focus: Focus::default(),
            track_sel: None,
            seek_preview: None,
            songs: songs::SongsView::default(),
            memos: Default::default(),
            // Something worth knowing at start-up (e.g. the library was rebuilt).
            toasts: toasts::Toasts::with_notices(notices),
            palette: None,
            overlay: None,
            reveal: None,
            recent_playlists: Vec::new(),
            fav_tab: favorites_page::FavTab::default(),
            settings_section: settings_page::Section::default(),
            lyrics_follow: crate::lyrics::Follow::default(),
            np_details: false,
            window: WindowState::default(),
            last_scroll: f64::NEG_INFINITY,
            bars_shown: false,
            edit_input: Vec::new(),
            momentum: Default::default(),
            live: Default::default(),
            tint: Default::default(),
            page_tint: Default::default(),
            page_tints: None,
            tint_cache: Default::default(),
            accent_applied: None,
            backdrop: Default::default(),
            listener: Default::default(),
            visualizer: None,
            spectrogram: Default::default(),
        }
    }
}

impl OndApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        #[cfg(windows)]
        now_playing::set_window(hwnd(cc));
        let mut app = Self::build(&cc.egui_ctx, Paths::resolve()?, audio::default_backend())?;
        app.gpu = cc.wgpu_render_state.as_ref().map(|s| crate::visuals::gpu::Target::new(s.target_format, &s.adapter));
        // The app is running by now, so NSApp takes the menu bar.
        let bridge = app.bridge.clone();
        app.menu = crate::menu::MenuBar::install(move |cmd| bridge.post(Msg::Menu(cmd)));
        // The window opened from this plan (see `main`); our title bar follows it.
        app.prefs.get(|s| s.window.plan()).apply_frame(&cc.egui_ctx);
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
        let mut notices: Vec<String> = Vec::new();
        if let Some(aside) = moved_aside {
            tracing::warn!("library database was unreadable; moved to {} and starting fresh", aside.display());
            notices.push(format!("Your library database couldn't be read, so Ond is rebuilding it. The old copy is at {}.", aside.display()));
        }
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
        if let Loaded::Unreadable { aside } = loaded {
            notices.push(format!("Your settings couldn't be read, so Ond reset them. Sign in again. The old file is at {}.", aside.display()));
        }
        let saved = prefs.snapshot();
        theme::install(ctx, saved.theme);
        let mut cache_cfg = streamcache::CacheConfig::new(&paths.stream_cache);
        if let Some(mb) = saved.stream_cache_mb {
            cache_cfg.max_bytes = mb * 1024 * 1024;
        }
        let cache = streamcache::StreamCache::new(cache_cfg, rt.handle().clone())?;
        let downloads = Downloads::start(
            saved.offline_dir.clone().unwrap_or_else(|| paths.offline.clone()),
            if prefs.has_login() { saved.account_id() } else { None },
            saved.offline_limit_gb.map(|gb| gb * 1_000_000_000),
            saved.offline_move_from.clone(),
            Downloads::opener(rt.handle().clone(), ctx.clone(), cache.clone()),
            &bridge,
            &mut notices,
        )?;
        let playback_cache = cache.clone();
        let app_cache = cache.clone();
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

        let shown = PrefsView::read(&prefs);
        Ok(Self {
            _rt: AppRuntime(Some(rt)),
            ctx: ctx.clone(),
            bridge,
            inbox,
            engine,
            login: Some(login),
            session: Session { settings: prefs.has_login().then(|| saved.clone()), ..Default::default() },
            sync: SyncState::default(),
            data: Data::default(),
            actions: Vec::new(),
            pending: Vec::new(),
            art,
            cache: app_cache,
            player: Player::default(),
            output,
            ui: UiState::new(notices, shown),
            bit_perfect: false,
            search: SearchState::default(),
            now_playing,
            downloads,
            diagnostics: diagnostics::Diagnostics::default(),
            waveforms: crate::visuals::waveform::Waveforms::new(ctx.clone()),
            gpu: None,
            prefs,
            analysis: Analysis::new(analyzer),
            favorites: Favorites::default(),
            menu: None,
            lyrics: HashMap::new(),
            song_stats: HashMap::new(),
        })
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            // From before a sign-out: stale, whatever it says.
            Msg::Connected { session, .. } | Msg::Unreachable { session, .. } if session != self.bridge.session() => {}
            Msg::Connected { client, settings, server, new_account, .. } => self.connected(client, settings, server, new_account),
            Msg::ConnectFailed(e) => self.connect_failed(e),
            Msg::Unreachable { username, error, .. } => self.unreachable(username, error),
            Msg::Latency(rtt) => self.diagnostics.set_latency(rtt),
            m @ (Msg::SearchIndex { .. }
            | Msg::Albums(_)
            | Msg::Artists(_)
            | Msg::AllTracks(_)
            | Msg::TrackSync(_)
            | Msg::TrackSyncDone(_)
            | Msg::AlbumUpdated(_)
            | Msg::SyncProgress(_)
            | Msg::AlbumSyncDone(_)
            | Msg::LoadFailed { .. }
            | Msg::Tracks { .. }
            | Msg::Newest(_)
            | Msg::Playlists(_)
            | Msg::PlaylistTracks { .. }
            | Msg::Credited { .. }
            | Msg::TrackBatch { .. }) => self.library_msg(m),
            m @ (Msg::Analyzed { .. } | Msg::AnalysesLoaded(_)) => self.analysis_msg(m),
            Msg::Engine(ev) => self.handle_engine(ev),
            Msg::Remote(r) => self.remote(r),
            m @ (Msg::OfflineFolder(_) | Msg::OfflineStore { .. } | Msg::OfflineMoving(_) | Msg::OfflineMoved(_)) => self.offline_msg(m),
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
                self.ui.recent_playlists.insert(0, id.clone());
                self.ui.recent_playlists.truncate(3);
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
        self.ui.toasts.push(text.into(), true);
    }

    /// Shows something worth knowing that isn't an error.
    pub(crate) fn show_notice(&mut self, text: impl Into<String>) {
        self.ui.toasts.push(text.into(), false);
    }




    /// In the full window: shows the sidebar on `tab`, or back to Up Next when
    /// it's already showing that tab.
    fn toggle_side_tab(&mut self, tab: QueueTab) {
        let show_queue = self.prefs_now().show_queue;
        if self.ui.queue_ui.tab == tab && (show_queue || self.ui.queue_sheet) {
            self.ui.queue_ui.tab = QueueTab::UpNext;
            return;
        }
        self.ui.queue_ui.tab = tab;
        if self.ui.queue_narrow {
            self.ui.queue_sheet = true;
        } else if !show_queue {
            self.apply_window(WindowAction::ToggleQueue);
        }
    }

    /// Applies one action. Most groups go to the part of the app they belong
    /// to; an action never asks for another one later, it applies it at once.
    fn apply(&mut self, action: Action) {
        if self.ui.visualizer.is_some() && Self::leaves_visualizer(&action) {
            self.close_visualizer();
        }
        // Browsing and the palette live in the full window.
        if matches!(action, Action::Go(_) | Action::OpenPalette) {
            self.set_mode(Some(WindowMode::Full));
        }
        match action {
            Action::Go(route) => self.go(route),
            Action::Back => {
                self.ui.nav.back();
                self.on_arrive();
            }
            Action::Forward => {
                self.ui.nav.forward();
                self.on_arrive();
            }
            Action::BackBy(n) => {
                self.ui.nav.back_by(n);
                self.on_arrive();
            }
            Action::Reveal { track, album } => {
                self.ui.reveal = Some(track);
                self.apply(Action::Go(Route::Album(album)));
            }
            Action::OpenPalette => {
                if self.ui.palette.is_none() && self.search.index.is_some() {
                    self.ui.palette = Some(Palette::default());
                }
            }
            Action::OpenSettings(section) => {
                self.ui.settings_section = section;
                self.set_mode(Some(WindowMode::Full));
                self.ui.overlay = Some(Overlay::Settings);
            }
            Action::Overlay(o) => self.ui.overlay = Some(o),
            Action::ToggleShortcuts => {
                self.set_mode(Some(WindowMode::Full));
                self.ui.overlay = match self.ui.overlay {
                    Some(Overlay::Shortcuts) => None,
                    _ => Some(Overlay::Shortcuts),
                };
            }
            Action::NewPlaylist => {
                self.set_mode(Some(WindowMode::Full));
                self.ui.overlay = Some(Overlay::new_playlist(Vec::new()));
            }
            Action::ToggleVisualizer => self.toggle_visualizer(),
            Action::Edit(e) => self.edit_input(e),
            Action::Lyrics => match self.prefs_now().mode {
                WindowMode::Full => self.toggle_side_tab(QueueTab::Lyrics),
                WindowMode::Player => self.apply_window(WindowAction::SetLyricsDrawer(!self.prefs_now().lyrics_open)),
                WindowMode::Mini => {
                    self.edit_windows(|w| w.lyrics_open = true);
                    self.set_mode(Some(WindowMode::Player));
                }
            },
            Action::Info => match self.prefs_now().mode {
                WindowMode::Full => self.toggle_side_tab(QueueTab::Info),
                WindowMode::Player => {
                    self.ui.queue_ui.tab = if self.ui.queue_ui.tab == QueueTab::Info { QueueTab::UpNext } else { QueueTab::Info };
                }
                WindowMode::Mini => {
                    self.ui.queue_ui.tab = QueueTab::Info;
                    self.set_mode(Some(WindowMode::Player));
                }
            },
            Action::NeedLyrics(track) => {
                if !self.lyrics.contains_key(&track.id) && self.session.client.is_some() {
                    self.lyrics.insert(track.id.clone(), crate::lyrics::Fetch::Loading);
                    self.bridge.fetch_lyrics(self.session.client.clone(), track.id.clone(), track.artist.clone(), track.title.clone());
                }
            }
            Action::NeedSongStats(id) => {
                if !self.song_stats.contains_key(&id) && self.session.client.is_some() {
                    self.song_stats.insert(id.clone(), None);
                    self.bridge.fetch_song_stats(self.session.client.clone(), id);
                }
            }
            Action::SyncNow => {
                if let Some(client) = self.session.live().cloned()
                    && self.sync.begin()
                {
                    self.bridge.sync_albums(client);
                }
            }
            Action::StopSync => self.bridge.cancel_sync(),
            Action::MeasureLatency => self.refresh_latency(),
            Action::SignOut => self.sign_out(),
            Action::Album(id, action) => self.play_album(id, action),
            Action::Artist(name, action) => self.play_artist(name, action),
            Action::Playlist(id, action) => self.play_playlist(id, action),
            Action::Tracks { tracks, source, action } => self.queue_tracks(tracks, source, action),
            Action::Queue(edit) => self.queue_edit(edit),
            Action::PlayPause => self.play_pause(),
            Action::Next => self.next(),
            Action::Previous => self.previous(),
            Action::Seek(to) => self.seek(to),
            Action::Window(a) => self.apply_window(a),
            Action::Prefs(a) => self.apply_prefs(a),
            Action::Output(a) => self.apply_output(a),
            Action::Offline(a) => self.apply_offline(a),
            Action::Analysis(a) => self.apply_analysis(a),
            Action::ToggleStar { kind, id, track, name } => self.toggle_star(kind, id, track.map(|t| *t), name),
            Action::Rate(id, rating) => self.rate(id, rating),
            Action::Command(cmd) => self.command(cmd),
            Action::PlaylistEdit(edit) => self.playlist_action(edit),
        }
    }

    fn go(&mut self, route: Route) {
        self.ui.nav.go(route);
        self.on_arrive();
    }

    /// Loads what the place we just arrived at needs.
    fn on_arrive(&mut self) {
        self.ui.track_sel = None;
        self.ui.focus = Focus::Main;
        match self.ui.nav.current().clone() {
            Route::Album(id) => self.bridge.open_album(self.session.client.clone(), id),
            Route::Artist { name, .. } => self.bridge.find_credited(name),
            Route::Playlist { id, .. } => {
                // Offline, a kept playlist is rebuilt from its pin and the stored tracks.
                if !self.session.online
                    && !self.data.playlist_tracks.contains_key(&id)
                    && let Some((_, ids)) = self.downloads.store.as_ref().and_then(|s| s.pin_tracks(&Pin::Playlist(id.clone())))
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



    /// App-wide keys, from the key map in [`crate::keys`]. Views handle their
    /// own (track list, queue, palette); a sheet takes the keyboard while open
    /// except to close the shortcut list with ⌘/.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.ui.palette.is_some() {
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
            if self.ui.overlay.is_none() || cmd == Cmd::Shortcuts {
                self.actions.push(Action::Command(cmd));
            }
        }
    }

    fn main_ui(&mut self, ui: &mut egui::Ui) {
        self.read_prefs();
        let ctx = ui.ctx().clone();
        self.art.poll();
        self.art_tint(&ctx);
        self.want_waveform();
        if !self.ui.prefs.experimental.spectrogram {
            // Turned off: the picture goes (and its texture with it).
            self.ui.spectrogram.clear();
        }
        self.shortcuts(&ctx);
        self.scroll_bars(&ctx);
        if self.ui.visualizer.is_some() && (!self.ui.prefs.experimental.visualizer || self.visualizer_escaped(&ctx)) {
            self.close_visualizer();
        }
        if self.ui.visualizer.is_some() {
            // The whole window, whatever the mode.
            self.visualizer_ui(ui);
            crate::csd::overlay(&ctx, None);
        } else {
            match self.ui.prefs.mode {
                WindowMode::Full => {
                    self.full_ui(ui);
                    self.view().palette_ui(&ctx);
                    self.overlay_ui(&ctx);
                    self.drag_ghost(&ctx);
                }
                WindowMode::Player => self.player_ui(ui),
                WindowMode::Mini => self.mini_ui(ui),
            }
            // Mini is all artwork; messages wait for a bigger window.
            if self.ui.prefs.mode != WindowMode::Mini {
                self.ui.toasts.ui(&ctx, ctx.content_rect());
            }
            // Window buttons and resize edges where the system draws none (Linux).
            crate::csd::overlay(&ctx, match self.ui.prefs.mode {
                WindowMode::Full => Some(toolbar::TOOLBAR_H),
                WindowMode::Player => Some(now_playing_view::PLAYER_BAR_H),
                WindowMode::Mini => None,
            });
        }
        // What the views, keys and menus asked for this frame. Applying an
        // action never queues another, so one pass takes them all.
        for a in std::mem::take(&mut self.actions) {
            self.apply(a);
        }
        // A visual that moves with the music was drawn: the next frame comes
        // sooner than the usual clock, but only while something plays.
        if let Some(every) = self.ui.live.next(self.engine.status.state() == PlayState::Playing) {
            ctx.request_repaint_after(every);
        }
        // The engine copies what plays only while something on screen shows it.
        let listening = self.ui.listener.used_in(ctx.cumulative_pass_nr());
        if listening != self.engine.status.tap.active() {
            self.engine.status.tap.set_active(listening);
        }
    }

    /// The colors in use: the chosen accent, or the album art's when colors
    /// adapt to it. Two tints ease on their own: the playing cover's, for the
    /// now-playing display, the Player, Mini and the window around the page;
    /// and the page's, which on an album page is that album's and elsewhere
    /// the playing one's. The first goes where `Tokens::of` finds it;
    /// `full_ui` swaps in the second around the page. A cover's colors are
    /// worked out in the background: until they arrive the old ones stay.
    fn art_tint(&mut self, ctx: &egui::Context) {
        let accent = self.ui.prefs.accent;
        if self.ui.accent_applied != Some(accent) {
            theme::set_accent(ctx, accent);
            self.ui.accent_applied = Some(accent);
        }
        let base = self.ui.tint_cache.base(accent);
        let adapt = self.ui.prefs.adapt_colors;
        // The covers' ids are borrowed (this runs every frame): a cover's tints
        // once its look is worked out, `None` meanwhile.
        let full = self.ui.prefs.mode == WindowMode::Full;
        let (art, cache) = (&mut self.art, &mut self.ui.tint_cache);
        let mut cover_tints = |cover: &str| art.look(cover).map(|look| cache.cover(cover, look.palette.as_ref()));
        let playing_cover = self.player.queue.current.as_ref().and_then(|c| self.data.cover_for(&c.track));
        let playing = match playing_cover.filter(|_| adapt) {
            Some(cover) => cover_tints(cover).or(self.ui.tint.target()).unwrap_or(base),
            None => base,
        };
        let page_cover = match self.ui.nav.current() {
            Route::Album(id) if full => Some(self.data.album(id).and_then(|a| a.cover_art.as_deref())),
            _ => None,
        };
        let page = match page_cover {
            Some(Some(cover)) if adapt => cover_tints(cover).or(self.ui.page_tint.target()).unwrap_or(playing),
            Some(None) if adapt => base,
            _ => playing,
        };
        let now = ctx.input(|i| i.time);
        let (playing, moving) = self.ui.tint.step(playing, now);
        let (page, page_moving) = self.ui.page_tint.step(page, now);
        self.ui.page_tints = Some(page);
        theme::set_tints(ctx, playing);
        if moving || page_moving {
            ctx.request_repaint();
        }
    }

    /// The cover the page takes its colors from: `Some` on an album page in
    /// the full window (`Some(None)` for an album without one), `None` on
    /// any other page. (`art_tint` works it out in place.)
    #[cfg(test)]
    fn page_cover(&self) -> Option<Option<String>> {
        match self.ui.nav.current() {
            Route::Album(id) if self.prefs_now().mode == WindowMode::Full => Some(self.data.album(id).and_then(|a| a.cover_art.clone())),
            _ => None,
        }
    }

    /// Waveform seek bar (experimental): has the playing track's peaks read
    /// once a complete copy is on disk (downloaded, or fully streamed).
    fn want_waveform(&mut self) {
        self.waveforms.poll();
        if !self.ui.prefs.experimental.waveform {
            return;
        }
        let Some(track) = self.player.queue.current.as_ref().map(|c| &c.track) else { return };
        let (downloads, cache, transcode) = (&self.downloads, &self.cache, self.output.transcode());
        self.waveforms.want(&track.id, track.suffix.as_deref(), || {
            downloads
                .local_path(&track.id)
                .or_else(|| cache.complete_path(&streamcache::CacheKey::original(track.id.clone(), track.suffix.clone())))
                .or_else(|| transcode.and_then(|t| cache.complete_path(&streamcache::CacheKey::transcode(&track.id, &t.format, t.kbps))))
        });
    }

    /// The blurred cover to draw behind the Player or Mini (experimental),
    /// oldest layer first; empty when that's off or nothing plays.
    pub(crate) fn backdrop_layers(&mut self, ctx: &egui::Context) -> Vec<crate::visuals::backdrop::Layer> {
        let cover = self.player.queue.current.as_ref().and_then(|c| self.data.cover_for(&c.track).map(str::to_owned));
        let Some(cover) = cover.filter(|_| self.ui.prefs.experimental.backdrop) else {
            self.ui.backdrop.clear();
            return Vec::new();
        };
        let look = self.art.look(&cover);
        let next = look.as_ref().and_then(|l| l.backdrop.as_ref());
        let (layers, fading) = self.ui.backdrop.step(next.map(|b| (cover.as_str(), b)), ctx.input(|i| i.time));
        if fading {
            ctx.request_repaint();
        }
        layers
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
            self.ui.last_scroll = now;
        }
        let since = now - self.ui.last_scroll;
        let show = since < 1.0;
        if show != self.ui.bars_shown {
            self.ui.bars_shown = show;
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
        let mut columns = self.ui.prefs.columns;
        let plan = columns.plan(width);
        self.ui.queue_narrow = !plan.queue_docked;
        if !self.ui.queue_narrow {
            self.ui.queue_sheet = false;
        }

        egui::Panel::top("toolbar")
            .exact_size(toolbar::TOOLBAR_H)
            .resizable(false)
            .frame(egui::Frame::new().fill(t.bg).inner_margin(egui::Margin::symmetric(12, 0)))
            .show(ui, |ui| self.toolbar(ui));
        if self.ui.prefs.diagnostics_bar {
            egui::Panel::bottom("diagnostics")
                .exact_size(diagnostics::BAR_H)
                .resizable(false)
                .frame(egui::Frame::new().fill(t.sidebar).inner_margin(egui::Margin::symmetric(12, 0)))
                .show(ui, |ui| self.view().diagnostics_bar(ui));
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
        let right_panel = (self.ui.prefs.show_queue && !self.ui.queue_narrow).then(|| {
            egui::Panel::right("queue")
                .exact_size(columns.right)
                .resizable(false)
                .frame(egui::Frame::new().fill(t.panel).inner_margin(egui::Margin { left: 8, right: 8, top: 12, bottom: 8 }))
                .show(ui, |ui| {
                    ui.set_max_width(ui.available_width());
                    self.view().queue_panel(ui, true)
                })
                .response
                .rect
        });
        // The page in its own colors (an album page, its album's).
        let ctx = ui.ctx().clone();
        let content = theme::with_tints(&ctx, self.ui.page_tints, || {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(t.bg).inner_margin(egui::Margin { left: 28, right: 28, top: 12, bottom: 0 }))
                .show(ui, |ui| self.main_view(ui))
        });

        // The column edges, after the content so they win the pointer. Widths
        // go straight into the preferences (saved once a drag settles).
        let before = columns;
        if let Some(r) = left_panel {
            let d = layout::edge_handle(ui, egui::Id::unique("sidebar-edge"), r.right(), r.top(), r.bottom());
            let limit = columns.left_limit(width, plan.queue_docked && self.ui.prefs.show_queue);
            if d.reset {
                columns.left = layout::LEFT_DEFAULT.min(limit);
            } else if d.dx != 0.0 {
                columns.left = (columns.left + d.dx).clamp(layout::LEFT_MIN, limit);
            }
        }
        if let Some(r) = right_panel {
            let d = layout::edge_handle(ui, egui::Id::unique("queue-edge"), r.left(), r.top(), r.bottom());
            let limit = columns.right_limit(width);
            if d.reset {
                columns.right = layout::RIGHT_DEFAULT.min(limit);
            } else if d.dx != 0.0 {
                columns.right = (columns.right - d.dx).clamp(layout::RIGHT_MIN, limit);
            }
        }
        if columns != before {
            self.set_columns(columns);
        }
        if self.ui.queue_sheet {
            self.queue_sheet_ui(ui.ctx(), content.response.rect, columns.right);
        }
    }

    /// The queue as a sheet sliding over the right of the content (narrow
    /// windows). Esc or a click outside closes it.
    fn queue_sheet_ui(&mut self, ctx: &egui::Context, content: egui::Rect, width: f32) {
        let t = Tokens::of_ctx(ctx);
        let w = width.min(content.width() - 24.0);
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
                self.view().queue_panel(ui, true);
            });
        if modal.should_close() {
            self.ui.queue_sheet = false;
        }
    }
}

impl eframe::App for OndApp {
    /// Edit menu commands become ordinary input, so the focused field
    /// pastes, copies or undoes as if the keys had reached it.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        raw_input.events.append(&mut self.ui.edit_input);
        if self.ui.momentum.process(&mut raw_input.events, std::time::Instant::now()) {
            ctx.request_repaint();
        }
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.inbox.try_recv() {
            self.handle(msg);
        }
        self.after_messages();
        self.poll_downloads();
        self.check_library_analysis();
        // The rest of this frame reads the preferences as they are now.
        self.read_prefs();
        let queue = if self.ui.queue_narrow { self.ui.queue_sheet } else { self.ui.prefs.show_queue };
        let (theme, diagnostics) = (self.ui.prefs.theme, self.ui.prefs.diagnostics_bar);
        if let Some(menu) = self.menu.as_mut() {
            menu.sync(queue, self.bit_perfect, theme);
            menu.set_diagnostics(diagnostics);
        }
        // The diagnostics bar keeps the server round trip fresh, but only while
        // something plays (idle stays idle).
        if self.ui.prefs.diagnostics_bar && self.engine.status.state() == PlayState::Playing {
            self.refresh_latency();
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
        self.track_window(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let started = std::time::Instant::now();
        if self.login.is_some() {
            // Logging in needs the full window.
            self.set_mode(Some(WindowMode::Full));
            self.login_ui(ui);
            crate::csd::overlay(ui.ctx(), Some(toolbar::TOOLBAR_H));
        } else {
            self.main_ui(ui);
        }
        self.diagnostics.note_frame(started.elapsed());
    }

    fn on_exit(&mut self) {
        if let Some(g) = self.ui.window.seen {
            self.edit_windows(|w| w.remember(g));
        }
        self.prefs.flush();
    }
}

impl Drop for OndApp {
    fn drop(&mut self) {
        self.bridge.cancel_sync();
        self.engine.send(Command::Shutdown);
    }
}


/// The main window's HWND, for the media controls.
#[cfg(windows)]
fn hwnd(cc: &eframe::CreationContext<'_>) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match cc.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
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

    fn app(ctx: &egui::Context, dir: &std::path::Path) -> OndApp {
        let paths = Paths {
            config_file: dir.join("settings.json"),
            library_db: dir.join("library.redb"),
            stream_cache: dir.join("streams"),
            art_cache: dir.join("art"),
            offline: dir.join("offline"),
        };
        let mut app = OndApp::build(ctx, paths, Box::new(NullOutput)).unwrap();
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

    fn frame(ctx: &egui::Context, app: &mut OndApp, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1360.0, 860.0))),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| app.main_ui(ui));
        // No renderer here to upload textures to.
        out.textures_delta.clear();
    }

    /// A frame at `time` seconds, so colors can be seen easing.
    fn frame_at(ctx: &egui::Context, app: &mut OndApp, time: f64) {
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1360.0, 860.0))), time: Some(time), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| app.main_ui(ui));
        out.textures_delta.clear();
    }

    #[test]
    fn quitting_waits_only_a_moment_for_blocking_work() {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).build().unwrap();
        let (started_tx, started) = channel();
        let (release, stuck) = channel::<()>();
        // A disk or keychain call that doesn't come back.
        rt.spawn_blocking(move || {
            let _ = started_tx.send(());
            let _ = stuck.recv_timeout(Duration::from_secs(60));
        });
        started.recv().unwrap();
        let t = std::time::Instant::now();
        drop(AppRuntime(Some(rt)));
        assert!(t.elapsed() < Duration::from_secs(30), "quitting waited {:?}", t.elapsed());
        drop(release);
    }

    #[test]
    fn an_album_page_takes_its_own_colors_and_everything_else_the_playing_ones() {
        use crate::visuals::colors::Tints;
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        let solid = |c: egui::Color32| egui::ColorImage::new([32, 32], vec![c; 32 * 32]);
        app.art.preload("al-0", solid(egui::Color32::from_rgb(170, 30, 40)));
        app.art.preload("al-5", solid(egui::Color32::from_rgb(30, 60, 170)));
        let playing: Vec<Track> = (1..=3).map(|n| track(&format!("t-{n}"), "al-0", n)).collect();
        app.player.queue.play_now(playing, 0, Source::Added);
        app.ui.nav.go(Route::Album("al-5".into()));
        let time = std::cell::Cell::new(0.0);
        let settle = |app: &mut OndApp| {
            // The covers' looks are worked out in the background.
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while ["al-0", "al-5"].iter().any(|c| app.art.look(c).is_none()) {
                assert!(std::time::Instant::now() < deadline, "the looks never arrived");
                std::thread::sleep(Duration::from_millis(5));
                app.art.poll();
            }
            for _ in 0..4 {
                time.set(time.get() + 0.5);
                frame_at(&ctx, app, time.get());
            }
        };
        let tints_of = |app: &mut OndApp, cover: &str| {
            let look = app.art.look(cover).unwrap();
            app.ui.tint_cache.cover(cover, look.palette.as_ref())
        };
        let shown = |ctx: &egui::Context| ctx.data(|d| d.get_temp::<Tints>(crate::visuals::colors::id())).unwrap();
        let dark = ctx.global_style().visuals.dark_mode;

        // Not adapting: the chosen accent everywhere, album page or not.
        app.prefs.edit(|s| s.accent = theme::Accent::Teal);
        settle(&mut app);
        let teal = Tints::base(theme::Accent::Teal);
        assert_eq!((shown(&ctx), app.ui.page_tints), (teal, Some(teal)));
        assert_eq!(Tokens::of_ctx(&ctx).accent, teal.of(dark).accent);

        // Adapting: the window in the playing album's colors, the page in its own.
        app.prefs.edit(|s| s.adapt_colors = true);
        settle(&mut app);
        let (red, blue) = (tints_of(&mut app, "al-0"), tints_of(&mut app, "al-5"));
        assert_ne!(red, blue);
        assert_ne!(red.of(dark).accent, teal.of(dark).accent);
        assert_eq!(shown(&ctx), red, "the now-playing display and the rest follow the playing track");
        assert_eq!(app.ui.page_tints, Some(blue), "the album page follows its album");
        // Drawing the page swaps its colors in, and puts the playing ones back after.
        let inside = theme::with_tints(&ctx, app.ui.page_tints, || Tokens::of_ctx(&ctx).accent);
        assert_eq!(inside, blue.of(dark).accent);
        assert_eq!(Tokens::of_ctx(&ctx).accent, red.of(dark).accent);

        // Leaving the page: its colors ease back to the playing album's.
        app.ui.nav.go(Route::Recent);
        for _ in 0..2 {
            time.set(time.get() + 0.3);
            frame_at(&ctx, &mut app, time.get());
        }
        let easing = app.ui.page_tints.unwrap();
        assert!(easing != blue && easing != red, "on the way back");
        settle(&mut app);
        assert_eq!(app.ui.page_tints, Some(red));

        // Back to the album: no wait, its colors are already known.
        app.ui.nav.go(Route::Album("al-5".into()));
        frame_at(&ctx, &mut app, time.get() + 0.01);
        assert_eq!(app.ui.page_tint.target(), Some(blue), "heading for the album's colors at once");
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
        assert_eq!(app.ui.toasts.texts().last().copied(), Some("Nothing in the queue is downloaded"));
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
            app.ui.nav.go(r);
            // Hover somewhere different each time so hover-only details draw too.
            let at = egui::pos2(300.0 + 37.0 * k as f32, 180.0 + 41.0 * k as f32);
            for _ in 0..2 {
                frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
            }
        }
        app.ui.queue_ui.tab = QueueTab::History;
        app.player.queue.advance();
        frame(&ctx, &mut app, Vec::new());
        app.prefs.edit(|s| s.show_queue = false);
        frame(&ctx, &mut app, Vec::new());
        app.ui.nav.go(Route::Album("al-0".into()));
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.ui.nav.trail(), [&Route::Songs, &Route::Album("al-0".into())]);
    }

    #[test]
    fn palette_opens_searches_and_queues() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        let cmd_k = egui::Event::Key { key: egui::Key::K, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND };
        frame(&ctx, &mut app, vec![cmd_k]);
        assert!(app.ui.palette.is_some());
        // The field takes focus on the palette's first frame; type after that.
        frame(&ctx, &mut app, Vec::new());
        frame(&ctx, &mut app, vec![egui::Event::Text("album 12".into())]);
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.ui.palette.as_ref().map(|p| p.query.as_str()), Some("album 12"));
        let before = app.player.queue.upcoming.len();
        let alt_enter = egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::ALT };
        frame(&ctx, &mut app, vec![alt_enter]);
        assert!(app.ui.palette.is_none());
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
    fn until(app: &mut OndApp, mut done: impl FnMut(&OndApp) -> bool) {
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
        app.apply(Action::Offline(OfflineAction::Keep(Keep::Album("al-12".into()), true)));
        app.apply(Action::Offline(OfflineAction::Keep(Keep::Album("al-12".into()), true)));
        assert_eq!(app.pending.len(), 1, "the newer action replaced the older");
        assert_eq!(app.downloads.pending_pins.len(), 1);
        // The load fails: both give up.
        until(&mut app, |a| a.pending.is_empty() && a.downloads.pending_pins.is_empty());
        // The tracks turn up later anyway (browsing, a sync): nothing plays or pins.
        let tracks: Vec<Track> = (1..=3).map(|n| track(&format!("x-{n}"), "al-12", n)).collect();
        app.handle(Msg::Tracks { album_id: "al-12".into(), tracks, fresh: true });
        assert_eq!(app.player.queue.upcoming.len(), before, "a stale action played");
        assert!(!app.downloads.store().is_pinned(&Pin::Album("al-12".into())), "a stale pin was made");
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
        assert!(app.ui.toasts.texts().contains(&"sync: timed out"));
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
        assert_eq!(app.prefs_now().mode, WindowMode::Mini);
        // Hovered (overlay drawn) and not.
        for at in [egui::pos2(100.0, 100.0), egui::pos2(-10.0, -10.0)] {
            for _ in 0..3 {
                frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
            }
        }
        frame(&ctx, &mut app, vec![key(egui::Key::Num2, egui::Modifiers::COMMAND | egui::Modifiers::ALT)]);
        assert_eq!(app.prefs_now().mode, WindowMode::Player);
        for _ in 0..2 {
            frame(&ctx, &mut app, vec![egui::Event::PointerMoved(egui::pos2(120.0, 60.0))]);
        }
        // Browsing goes back to the full window.
        app.actions.push(Action::Go(Route::Album("al-0".into())));
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.prefs_now().mode, WindowMode::Full);
        // ⌘K from a small mode does too.
        app.edit_windows(|w| w.mode = WindowMode::Player);
        frame(&ctx, &mut app, vec![key(egui::Key::K, egui::Modifiers::COMMAND)]);
        assert_eq!(app.prefs_now().mode, WindowMode::Full);
        assert!(app.ui.palette.is_some());
    }

    #[test]
    fn the_visualizer_opens_only_when_its_experiment_is_on_and_esc_leaves() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        let key = |key, modifiers| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        frame(&ctx, &mut app, vec![key(egui::Key::T, egui::Modifiers::COMMAND)]);
        assert!(app.ui.visualizer.is_none(), "off: ⌘T does nothing");
        app.prefs.edit(|s| s.experimental.visualizer = true);
        frame(&ctx, &mut app, vec![key(egui::Key::T, egui::Modifiers::COMMAND)]);
        assert!(app.ui.visualizer.is_some());
        for _ in 0..3 {
            frame(&ctx, &mut app, Vec::new());
        }
        frame(&ctx, &mut app, vec![key(egui::Key::Escape, egui::Modifiers::NONE)]);
        assert!(app.ui.visualizer.is_none(), "Esc leaves");
        // Going somewhere else leaves it too, and so does turning it off.
        app.toggle_visualizer();
        app.apply(Action::Go(Route::Albums));
        assert!(app.ui.visualizer.is_none());
        app.toggle_visualizer();
        app.prefs.edit(|s| s.experimental.visualizer = false);
        frame(&ctx, &mut app, Vec::new());
        assert!(app.ui.visualizer.is_none());
    }

    #[test]
    fn arrows_switch_the_visualizer_and_the_choice_is_kept() {
        use crate::visuals::visualizer::{Kind, Pick};
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        let key = |key, modifiers| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        let showing = |app: &OndApp| app.ui.visualizer.as_ref().map(|v| v.kind());
        app.prefs.edit(|s| {
            s.experimental.visualizer = true;
            s.experimental.visualizer_pick = Pick::Particles;
        });
        app.toggle_visualizer();
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(showing(&app), Some(Kind::Particles), "it opens on the one chosen");
        frame(&ctx, &mut app, vec![key(egui::Key::ArrowRight, egui::Modifiers::NONE)]);
        assert_eq!(showing(&app), Some(Kind::Ribbons));
        assert_eq!(app.prefs_now().experimental.visualizer_pick, Pick::Ribbons, "and that becomes the choice");
        frame(&ctx, &mut app, vec![key(egui::Key::ArrowRight, egui::Modifiers::NONE)]);
        assert_eq!(showing(&app), Some(Kind::Flow), "round from the last to the first");
        frame(&ctx, &mut app, vec![key(egui::Key::ArrowLeft, egui::Modifiers::NONE)]);
        assert_eq!(showing(&app), Some(Kind::Ribbons));
        // ⌘→ is still the next track, not a switch.
        frame(&ctx, &mut app, vec![key(egui::Key::ArrowRight, egui::Modifiers::COMMAND)]);
        assert_eq!(showing(&app), Some(Kind::Ribbons));
        // Kept for next time.
        app.prefs.flush();
        let saved = crate::settings::Settings::read(&dir.path().join("settings.json")).unwrap();
        assert_eq!(saved.experimental.visualizer_pick, Pick::Ribbons);
        app.close_visualizer();
        app.toggle_visualizer();
        assert_eq!(showing(&app), Some(Kind::Ribbons));
        // Shuffled, the arrows still switch, and it stays shuffled.
        app.close_visualizer();
        app.prefs.edit(|s| s.experimental.visualizer_pick = Pick::Shuffle);
        app.toggle_visualizer();
        frame(&ctx, &mut app, Vec::new());
        let first = showing(&app);
        frame(&ctx, &mut app, vec![key(egui::Key::ArrowRight, egui::Modifiers::NONE)]);
        assert_ne!(showing(&app), first);
        assert_eq!(app.prefs_now().experimental.visualizer_pick, Pick::Shuffle);
        // And each new track after the first brings another.
        app.player.queue.play_now(vec![track("t-first", "al-0", 1)], 0, Source::Added);
        frame(&ctx, &mut app, Vec::new());
        for n in 0..4 {
            let before = showing(&app);
            app.player.queue.play_now(vec![track(&format!("t-{n}"), "al-0", 1)], 0, Source::Added);
            frame(&ctx, &mut app, Vec::new());
            assert_ne!(showing(&app), before, "track {n}");
        }
    }

    #[test]
    fn lyrics_go_to_the_sidebar_or_the_players_drawer() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        frame(&ctx, &mut app, Vec::new());
        // Full window: the sidebar switches to its Lyrics tab, and back.
        app.apply(Action::Lyrics);
        assert_eq!(app.ui.queue_ui.tab, QueueTab::Lyrics);
        app.apply(Action::Lyrics);
        assert_eq!(app.ui.queue_ui.tab, QueueTab::UpNext);
        // The artwork toggles to the Player (nothing used yet), where ⌥⌘L opens the drawer.
        app.apply(Action::Window(WindowAction::TogglePlayer));
        assert_eq!(app.prefs_now().mode, WindowMode::Player);
        app.apply(Action::Lyrics);
        frame(&ctx, &mut app, Vec::new());
        assert!(app.prefs_now().lyrics_open);
        // Mini, then back to full: the artwork now toggles to Mini.
        app.apply(Action::Window(WindowAction::ToggleMini));
        app.apply(Action::Window(WindowAction::SetMode(WindowMode::Full)));
        app.apply(Action::Window(WindowAction::TogglePlayer));
        assert_eq!(app.prefs_now().mode, WindowMode::Mini);
        // Lyrics from Mini open the Player with its drawer.
        app.edit_windows(|w| w.lyrics_open = false);
        app.apply(Action::Lyrics);
        assert_eq!(app.prefs_now().mode, WindowMode::Player);
        assert!(app.prefs_now().lyrics_open);
        for _ in 0..2 {
            frame(&ctx, &mut app, Vec::new());
        }
    }

    /// Makes track `id` downloaded, the way a finished stream would: via a file
    /// the stream cache "has", which the store links in.
    fn download(app: &mut OndApp, dir: &std::path::Path, id: &str) {
        let file = dir.join(format!("cached-{id}.flac"));
        std::fs::write(&file, b"fLaC").unwrap();
        let want = id.to_owned();
        app.downloads.store().set_cache_lookup(Arc::new(move |r: &TrackRef| (r.id == want).then(|| file.clone())));
        let t = app.data.tracks.all.iter().find(|t| t.id == id).unwrap().clone();
        app.apply(Action::Offline(OfflineAction::Keep(Keep::Tracks(vec![t]), true)));
        let start = std::time::Instant::now();
        while !app.downloads.store().snapshot().is_done(id) {
            assert!(start.elapsed() < Duration::from_secs(5), "download of {id}");
            std::thread::sleep(Duration::from_millis(5));
        }
        app.downloads.snap = app.downloads.store().snapshot();
    }

    #[test]
    fn every_startup_notice_is_shown() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("library.redb"), b"not a database").unwrap();
        std::fs::write(dir.path().join("settings.json"), b"{ not json").unwrap();
        let ctx = egui::Context::default();
        let app = app(&ctx, dir.path());
        let shown = app.ui.toasts.texts();
        assert!(shown.iter().any(|t| t.starts_with("Your library database couldn't be read")), "{shown:?}");
        assert!(shown.iter().any(|t| t.starts_with("Your settings couldn't be read")), "{shown:?}");
    }

    #[test]
    fn another_account_gets_its_own_library_and_downloads() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        let client = || subsonic::Client::new("http://127.0.0.1:9", subsonic::Credentials::new("u", "p")).unwrap();
        let login = |user: &str| Box::new(Settings { server: "http://music.local".into(), username: user.into(), ..Default::default() });
        let connect = |app: &mut OndApp, user: &str, new_account: bool| {
            let session = app.bridge.session();
            app.handle(Msg::Connected { session, client: client(), settings: login(user), server: "test".into(), new_account });
        };
        connect(&mut app, "luke", false);
        store_opened(&mut app);
        download(&mut app, dir.path(), "t-2");
        assert!(app.downloads.snap.is_done("t-2"));

        // Someone else signs in: the shown library goes, and so do the queue
        // and the old account's downloads (not deleted, just not theirs).
        app.player.queue = crate::queue::Queue::default();
        let tracks: Vec<Track> = app.data.tracks.by_album["al-0"][..2].to_vec();
        app.queue_tracks(tracks, Source::Album { id: "al-0".into(), name: "Album 0".into() }, QueueAction::Play { start: 0 });
        connect(&mut app, "sam", true);
        assert!(app.data.albums.is_empty() && app.data.tracks.all.is_empty());
        assert!(app.player.queue.current.is_none());
        assert!(app.downloads.snap.pins.is_empty(), "nothing shows as downloaded while sam's store opens");
        store_opened(&mut app);
        assert!(app.downloads.snap.pins.is_empty(), "sam's store starts empty");
        assert!(!app.downloads.store().snapshot().is_done("t-2"));

        // Back to the first account: its downloads are where they were.
        connect(&mut app, "luke", true);
        store_opened(&mut app);
        assert!(app.downloads.store().snapshot().is_done("t-2"));
    }

    /// Waits for the offline store opening in the background, handling only
    /// its messages: the rest go back in the inbox, in order, behind them.
    fn store_opened(app: &mut OndApp) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut others = Vec::new();
        while app.downloads.store.is_none() || app.downloads.opening {
            assert!(std::time::Instant::now() < deadline, "the offline store never opened");
            match app.inbox.recv_timeout(Duration::from_millis(20)) {
                Ok(m @ (Msg::OfflineStore { .. } | Msg::OfflineMoving(_) | Msg::OfflineMoved(_))) => app.handle(m),
                Ok(m) => others.push(m),
                Err(_) => {}
            }
        }
        for m in others {
            app.bridge.post(m);
        }
    }

    #[test]
    fn another_accounts_downloads_open_in_the_background_and_wait_for_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        let client = || subsonic::Client::new("http://127.0.0.1:9", subsonic::Credentials::new("u", "p")).unwrap();
        let login = |user: &str| Box::new(Settings { server: "http://music.local".into(), username: user.into(), ..Default::default() });
        let connect = |app: &mut OndApp, user: &str| {
            let session = app.bridge.session();
            app.handle(Msg::Connected { session, client: client(), settings: login(user), server: "test".into(), new_account: false });
        };
        connect(&mut app, "luke");
        // Signed in as someone else before luke's store came: it's dropped unused.
        connect(&mut app, "sam");
        assert!(app.downloads.store.is_none(), "opened on the UI thread");
        // Asked for while it opens: done once it's open.
        let t = app.data.tracks.all.iter().find(|t| t.id == "t-3").unwrap().clone();
        app.apply(Action::Offline(OfflineAction::Keep(Keep::Tracks(vec![t]), true)));
        app.apply(Action::Offline(OfflineAction::Pause(true)));
        store_opened(&mut app);
        let sam = login("sam").account_id().unwrap();
        assert!(app.downloads.store().dir().ends_with(format!("{sam:08x}")), "{}", app.downloads.store().dir().display());
        for a in std::mem::take(&mut app.actions) {
            app.apply(a);
        }
        assert!(app.downloads.store().is_pinned(&Pin::Track("t-3".into())));
        assert!(app.downloads.store().snapshot().paused);
    }

    #[test]
    fn downloads_moved_to_another_folder_move_in_the_background() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("old"), dir.path().join("new"));
        let account = Settings { server: "http://music.local".into(), username: "luke".into(), ..Default::default() };
        let store = old.join(format!("{:08x}", account.account_id().unwrap()));
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(store.join("t-2.flac"), b"fLaC").unwrap();
        let mut saved = Settings { offline_dir: Some(new.clone()), offline_move_from: Some(old.clone()), ..account };
        saved.signed_out = true;
        std::fs::write(dir.path().join("settings.json"), serde_json::to_vec(&saved).unwrap()).unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        assert!(app.downloads.store.is_none(), "the move doesn't hold up the first frame");
        assert!(app.ui.toasts.texts().iter().any(|t| t.starts_with("Moving downloads to")), "{:?}", app.ui.toasts.texts());
        store_opened(&mut app);
        assert_eq!(app.downloads.store().dir(), new.join(store.file_name().unwrap()));
        assert!(new.join(store.file_name().unwrap()).join("t-2.flac").exists());
        assert!(!old.exists());
        assert_eq!(app.prefs.snapshot().offline_move_from, None, "done, so not moved again");
        let shown = app.ui.toasts.texts();
        assert!(shown.iter().any(|t| t.starts_with("Downloads moved to")), "{shown:?}");
        assert!(!shown.iter().any(|t| t.starts_with("Moving downloads")), "the progress notice gives way: {shown:?}");
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
        assert_eq!(app.ui.toasts.texts().last().copied(), Some("Skipped 1 song that isn't downloaded"));
        // Nothing after it can play, so the engine isn't told about a next track.
        assert_eq!(app.player.sent_next(), None);
        // When it ends, the rest is skipped and playback stops cleanly.
        app.handle_engine(audio::Event::Ended { track_id: "t-2".into(), token: 0 });
        assert!(app.player.queue.current.is_none());

        // Back online: everything plays again, downloaded songs still from disk.
        let client = subsonic::Client::new("http://127.0.0.1:9", subsonic::Credentials::new("u", "p")).unwrap();
        app.handle(Msg::Connected { session: app.bridge.session(), client, settings: Box::default(), server: "test".into(), new_account: false });
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
        let shown = app.ui.toasts.texts();
        assert!(shown.contains(&"playlists: timed out") && shown.contains(&"favorites: timed out"), "{shown:?}");
    }

    #[test]
    fn a_library_analysis_resumes_once_the_server_answers() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        let client = || subsonic::Client::new("http://127.0.0.1:9", subsonic::Credentials::new("u", "p")).unwrap();
        // Held, so what's queued stays queued.
        app.analysis.runner.set_paused(true);
        app.prefs.edit(|s| s.analysis_resume = true);
        // Startup: the library and the stored results load well before the
        // server answers (or offline, never). Nothing has run, so nothing finished.
        app.handle(Msg::AnalysesLoaded(Vec::new()));
        app.handle(Msg::AllTracks(Box::new(TrackSet::new(app.data.tracks.all.to_vec()))));
        for _ in 0..3 {
            app.check_library_analysis();
        }
        assert!(app.prefs_now().analysis_resume, "the run is still due");
        assert!(!app.analysis.runner.progress().active());

        app.handle(Msg::Connected { session: app.bridge.session(), client: client(), settings: Box::default(), server: "test".into(), new_account: false });
        assert_eq!(app.analysis.runner.progress().total, 20, "the whole library is queued");
        app.check_library_analysis();
        assert!(app.prefs_now().analysis_resume);

        // Signing out drops the old account's queue, but the run is still due for the next.
        app.sign_out();
        assert!(!app.analysis.runner.progress().active());
        app.check_library_analysis();
        assert!(app.prefs_now().analysis_resume);
        app.handle(Msg::Connected { session: app.bridge.session(), client: client(), settings: Box::default(), server: "test".into(), new_account: false });
        assert_eq!(app.analysis.runner.progress().total, 20);

        // The queue runs dry: done, and not resumed at the next launch.
        app.analysis.runner.clear();
        app.check_library_analysis();
        assert!(!app.prefs_now().analysis_resume);
    }

    #[test]
    fn signing_out_forgets_lyrics_and_song_stats() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        let client = || subsonic::Client::new("http://127.0.0.1:9", subsonic::Credentials::new("u", "p")).unwrap();
        app.handle(Msg::Connected { session: app.bridge.session(), client: client(), settings: Box::default(), server: "test".into(), new_account: false });
        let t = app.data.tracks.all[0].clone();
        // Asked for, still on their way when the user signs out.
        app.apply(Action::NeedLyrics(Box::new(t.clone())));
        app.apply(Action::NeedSongStats(t.id.clone()));
        assert!(matches!(app.lyrics.get(&t.id), Some(crate::lyrics::Fetch::Loading)));
        assert!(app.song_stats.contains_key(&t.id));
        app.sign_out();
        assert!(app.lyrics.is_empty() && app.song_stats.is_empty(), "nothing left loading for good, nor for the next account");
        // Signed in again: asked for afresh.
        app.handle(Msg::Connected { session: app.bridge.session(), client: client(), settings: Box::default(), server: "test".into(), new_account: false });
        app.apply(Action::NeedLyrics(Box::new(t.clone())));
        assert!(matches!(app.lyrics.get(&t.id), Some(crate::lyrics::Fetch::Loading)));
        // Another account: what was fetched for the last one goes too.
        app.handle(Msg::Lyrics { track_id: t.id.clone(), lyrics: None });
        app.handle(Msg::SongStats { track_id: t.id.clone(), stats: None });
        app.handle(Msg::Connected { session: app.bridge.session(), client: client(), settings: Box::default(), server: "test".into(), new_account: true });
        assert!(app.lyrics.is_empty() && app.song_stats.is_empty());
    }

    #[test]
    fn playing_an_album_stored_only_in_part_waits_for_the_server() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        let all: Vec<Track> = (1..=20).map(|n| track(&format!("p-{n}"), "al-3", n)).collect();
        // A sync cut short stored 5 of its 20 songs.
        app.data.tracks.set_album("al-3", all[..5].to_vec());
        app.player.queue = crate::queue::Queue::default();
        // Queued (offline nothing here is downloaded, so it's all skipped into history).
        let queued = |app: &OndApp| app.player.queue.history.len() + app.player.queue.pass_len();

        // Offline: what's stored is all there is.
        app.apply(Action::Album("al-3".into(), QueueAction::Play { start: 0 }));
        assert_eq!(queued(&app), 5);

        // Online: the server's listing, not the stored part.
        app.player.queue = crate::queue::Queue::default();
        app.session.client = Some(subsonic::Client::new("http://127.0.0.1:9", subsonic::Credentials::new("u", "p")).unwrap());
        app.apply(Action::Album("al-3".into(), QueueAction::Play { start: 0 }));
        assert_eq!(queued(&app), 0, "waits");
        // The stored copy arrives first: still waiting.
        app.handle(Msg::Tracks { album_id: "al-3".into(), tracks: all[..5].to_vec(), fresh: false });
        assert_eq!(queued(&app), 0);
        app.handle(Msg::Tracks { album_id: "al-3".into(), tracks: all.clone(), fresh: true });
        assert_eq!(queued(&app), 20);
        assert!(app.pending.is_empty());

        // The server can't be reached: play what's stored rather than nothing.
        app.player.queue = crate::queue::Queue::default();
        app.data.tracks.set_album("al-3", all[..5].to_vec());
        app.apply(Action::Album("al-3".into(), QueueAction::Play { start: 0 }));
        app.handle(Msg::LoadFailed { key: ListKey::Album("al-3".into()), error: Some("timed out".into()) });
        assert_eq!(queued(&app), 5);
        assert!(app.pending.is_empty());

        // A complete album plays at once.
        app.player.queue = crate::queue::Queue::default();
        app.apply(Action::Album("al-0".into(), QueueAction::Play { start: 0 }));
        assert_eq!(queued(&app), 20);
    }

    #[test]
    fn signing_out_ignores_a_reconnect_that_lands_afterwards() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        let client = || subsonic::Client::new("http://127.0.0.1:9", subsonic::Credentials::new("u", "p")).unwrap();
        // Signed in, then the server went away: a reconnect loop is running.
        let before = app.bridge.session();
        app.handle(Msg::Connected { session: before, client: client(), settings: Box::default(), server: "test".into(), new_account: false });
        assert!(app.session.online && app.session.client.is_some());
        app.handle(Msg::Unreachable { session: before, username: "u".into(), error: "gone".into() });
        assert!(app.session.unreachable);

        app.sign_out();
        assert!(app.login.is_some());
        assert!(app.session.client.is_none() && !app.session.online && !app.session.unreachable);
        assert_ne!(app.bridge.session(), before, "sign-out ends the session");

        // The old loop reaches the server after all: stale, ignored.
        app.handle(Msg::Connected { session: before, client: client(), settings: Box::default(), server: "test".into(), new_account: false });
        app.handle(Msg::Unreachable { session: before, username: "u".into(), error: "gone".into() });
        assert!(app.login.is_some(), "still on the login screen");
        assert!(app.session.client.is_none() && !app.session.online && !app.session.unreachable);

        // A fresh sign-in in the new session works.
        app.handle(Msg::Connected { session: app.bridge.session(), client: client(), settings: Box::default(), server: "test".into(), new_account: false });
        assert!(app.login.is_none() && app.session.online);
    }

    #[test]
    fn keeping_an_album_waits_for_its_tracks_and_the_offline_place_draws() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        // Album 5's tracks aren't loaded: the pin waits for them.
        app.apply(Action::Offline(OfflineAction::Keep(Keep::Album("al-5".into()), true)));
        assert!(!app.downloads.store().is_pinned(&Pin::Album("al-5".into())));
        let tracks: Vec<Track> = (1..=3).map(|n| track(&format!("y-{n}"), "al-5", n)).collect();
        app.handle(Msg::Tracks { album_id: "al-5".into(), tracks: tracks.clone(), fresh: false });
        assert!(app.downloads.store().is_pinned(&Pin::Album("al-5".into())));
        assert_eq!(app.downloads.store().pin_tracks(&Pin::Album("al-5".into())).unwrap().1, ["y-1", "y-2", "y-3"]);
        // A partial list from the local library (mid-sync, rebuilt) never shrinks the pin.
        app.downloads.snap = app.downloads.store().snapshot();
        app.handle(Msg::Tracks { album_id: "al-5".into(), tracks: tracks[..1].to_vec(), fresh: false });
        // (The stored album now lists only y-1; a library-wide refresh sees that.)
        let all = app.data.tracks.all.to_vec();
        app.handle(Msg::AllTracks(Box::new(TrackSet::new(all))));
        assert_eq!(app.downloads.store().pin_tracks(&Pin::Album("al-5".into())).unwrap().1, ["y-1", "y-2", "y-3"]);
        // The album changed on the server: the pin follows.
        app.handle(Msg::Tracks { album_id: "al-5".into(), tracks: tracks[..2].to_vec(), fresh: true });
        assert_eq!(app.downloads.store().pin_tracks(&Pin::Album("al-5".into())).unwrap().1, ["y-1", "y-2"]);
        download(&mut app, dir.path(), "t-3");

        for route in [Route::Offline, Route::Album("al-5".into()), Route::Album("al-0".into()), Route::Recent, Route::Songs] {
            app.ui.nav.go(route);
            for _ in 0..2 {
                frame(&ctx, &mut app, vec![egui::Event::PointerMoved(egui::pos2(500.0, 300.0))]);
            }
        }
        // ⇧⌘D on the album page toggles it.
        app.ui.nav.go(Route::Album("al-5".into()));
        let key = egui::Event::Key { key: egui::Key::D, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND | egui::Modifiers::SHIFT };
        frame(&ctx, &mut app, vec![key]);
        assert!(!app.downloads.store().is_pinned(&Pin::Album("al-5".into())));
        assert!(app.downloads.store().is_pinned(&Pin::Track("t-3".into())));
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
