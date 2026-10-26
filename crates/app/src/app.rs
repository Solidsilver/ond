//! The main window: sidebar · toolbar with the now-playing display · the
//! current view · the queue column. Views are in submodules; they don't change
//! state directly but push [`Action`]s, which are applied once the frame is
//! drawn. That keeps borrows simple and makes every view reusable by the
//! compact layout.
//!
//! This file holds [`OndApp`]: building it, the dispatch of bridge messages
//! and actions, and the frame (`main_ui`, `full_ui`). The app's parts and the
//! actions and messages each one handles are in their own modules:
//!
//! - `session`: the server connection, the login form, offline and back, signing out
//! - `data`: the library as loaded, the sync's state, the search index, queue
//!   actions waiting for tracks
//! - `pins`: offline downloads ([`OfflineAction`])
//! - `analyzing`: library analysis ([`AnalysisAction`])
//! - `playing`: the player, where tracks play from, engine events, output ([`OutputAction`])
//! - `window_state`: window modes and following the window ([`WindowAction`])
//! - `prefs_view`: the preferences a frame reads, and appearance changes ([`PrefsAction`])
//! - `commands`: keys and the menu bar as actions; hearts, ratings, playlist edits
//! - `ui_state`, `frame`: what the window shows, and the work each frame does
//!   around the drawing
//! - `view_ctx` and the views: what a view gets, and the views themselves

// The app's parts.
mod analyzing;
mod commands;
mod data;
mod frame;
mod pins;
mod playing;
mod prefs_view;
mod session;
mod ui_state;
mod view_ctx;
mod window_state;

// Views and their pieces.
mod album_page;
mod alpha;
mod browse;
mod compact;
mod diagnostics;
mod dialogs;
mod favorites_page;
mod info_panel;
mod layout;
mod now_playing_view;
mod offline_page;
mod palette;
mod queue_panel;
mod seek;
mod settings_page;
mod sidebar;
mod songs;
mod toasts;
mod toolbar;
mod track_row;
mod visualizer_view;
mod widgets;

#[cfg(test)]
mod snapshots;
#[cfg(test)]
mod tests;

use crate::analyze;
use crate::art::Art;
use crate::bridge::{Bridge, Msg};
use crate::favorites::{Favorites, StarKind};
use crate::keys::{Cmd, EditCmd};
use crate::nav::Route;
use crate::now_playing;
use crate::output::Outputs;
use crate::player::{Player, QueueEdit};
use crate::prefs::{Loaded, Prefs};
use crate::queue::Source;
use crate::settings::Paths;
use crate::theme::{self, Tokens};
use crate::window::WindowMode;
use audio::{Command, EngineHandle, PlayState};
use library::{Library, Track};
use offline::Pin;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;
// What the tests (here and in `snapshots`) build their fixtures from.
#[cfg(test)]
use {
    crate::bridge::ListKey, crate::nav::Nav, crate::search::SearchIndex, crate::settings::Settings, crate::theme::ThemePref, crate::tracks::TrackSet, library::Album,
    offline::TrackRef, pins::track_ref, ui_state::QueueUi,
};

pub use crate::player::QueueAction;
pub use analyzing::{Analysis, AnalysisAction};
pub use commands::PlaylistAction;
pub use data::{Data, SearchState, SyncState};
pub use dialogs::Overlay;
pub use palette::Palette;
pub use pins::{Downloads, Keep, OfflineAction};
pub use playing::OutputAction;
pub use prefs_view::PrefsAction;
pub use session::Session;
pub use ui_state::{AlbumSort, DragTracks, Focus, QueueTab, SongStats, UiState};
pub use window_state::WindowAction;
pub(crate) use widgets::{MODES, choice, elide as elide_text, link as link_text, mode_label};

use data::Pending;
use pins::playable;
use prefs_view::PrefsView;
use session::LoginForm;
use window_state::{WindowState, title_bar_h};

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
    /// Delete the cover art kept on disk (and forget what's loaded).
    ClearArtCache,
    /// Delete the streamed audio kept on disk.
    ClearStreamCache,
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
    /// Track waveforms for the seek bar.
    waveforms: crate::visuals::waveform::Waveforms,
    /// What the window renders to, for the visualizer's own GPU drawing;
    /// `None` without a wgpu renderer (it then uses the painter).
    gpu: Option<crate::visuals::gpu::Target>,
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
            Action::ClearArtCache => self.art.clear(),
            Action::ClearStreamCache => self.cache.clear(),
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
                self.ui.toasts.ui(&ctx, ctx.content_rect(), &mut self.actions);
            }
            // Window buttons and resize edges where the system draws none (Linux).
            crate::csd::overlay(&ctx, title_bar_h(self.ui.prefs.mode));
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
        crate::chrome::sync(ctx, crate::chrome::Bar { height: title_bar_h(self.ui.prefs.mode) });
        self.track_window(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let started = std::time::Instant::now();
        if self.login.is_some() {
            // Logging in needs the full window. (Asked only when it isn't:
            // the switch would change nothing, but checking takes a copy of
            // the preferences, which isn't worth doing every frame.)
            if self.prefs.get(|s| s.window.mode) != WindowMode::Full {
                self.set_mode(Some(WindowMode::Full));
            }
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
