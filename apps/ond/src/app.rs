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
//! - `onboarding`: the first-run sheet the login form is drawn in
//! - `sources`: a folder of music added from the sheet, a source's folders,
//!   the folder picker, removing a source ([`SourceAction`])
//! - `source_look`: each source's name, colour and state as the switcher,
//!   chips, dots and Settings show them, and when dots are drawn
//! - `data`: the library as loaded, the sync's state, the search index, queue
//!   actions waiting for tracks
//! - `pins`: offline downloads ([`OfflineAction`])
//! - `analyzing`: library analysis ([`AnalysisAction`])
//! - `playing`: the player, where tracks play from, engine events, output ([`OutputAction`])
//! - `window_state`: window modes and following the window ([`WindowAction`])
//! - `prefs_view`: the preferences a frame reads, and appearance changes ([`PrefsAction`])
//! - `commands`: keys and the menu bar as actions; hearts, ratings, playlist edits
//! - `updating`: newer versions of Ond, the checker's answers ([`UpdateAction`])
//! - `ui_state`, `frame`: what the window shows, and the work each frame does
//!   around the drawing
//! - `view_ctx` and the views: what a view gets, and the views themselves
//!
//! New views take a [`ViewCtx`](view_ctx::ViewCtx) and push `Action`s. Views
//! written as `OndApp` methods (the toolbar, the sidebar, Settings) are the
//! older style, not a pattern to follow. Two places still change things while
//! drawing, and they're known exceptions: the output panel (`output/panel.rs`)
//! edits preferences and sends engine commands, and the visualizer saves the
//! visual it switches to (`visualizer_view.rs`).
//!
//! # Adding things
//!
//! A server call:
//! 1. A `Bridge` method that does the work in the background and posts the
//!    result. Use `self.scoped()` so a result for an account that signed out
//!    is dropped.
//! 2. A `Msg` variant for the result, inside the group it belongs to
//!    (`LibraryMsg`, `OfflineMsg`, `AnalysisMsg`) if any. Failures go as a
//!    `Failure`, so only network ones make the app check on the server.
//! 3. A handler for it in the matching `app/*.rs` (`data.rs` for the library).
//!
//! A preference:
//! 1. A field in `Settings`, with `#[serde(default)]`.
//! 2. A field in `PrefsView` if a frame reads it, and a `PrefsAction`
//!    variant with its arm in `apply_prefs`.
//! 3. An arm in `locked()` (`prefs_view.rs`) and its path in
//!    `managed::ALLOWED` if managed.json may set it.
//! 4. A row in `settings_page.rs`.
//!
//! A view:
//! 1. `app/<view>.rs`, drawing from a `ViewCtx` and pushing `Action`s.
//! 2. A `Route` for it in `nav.rs`, and its arm in `main_view` (`browse.rs`).
//! 3. An entry in `every_view_draws` in `app/tests.rs` and a route in
//!    `every_view_names_its_controls` in `app/ui_tests.rs` (every painted
//!    control needs a `widget_info`), and a scene in `snapshots/scenes.rs`.
//!    Reuse `virtual_list`, `track_list`, `browse::heading_bar` and
//!    `widgets::empty_state` rather than drawing those again.

// The app's parts.
mod all_sources;
mod analyzing;
mod commands;
mod data;
mod frame;
mod mixed_playlists;
mod onboarding;
pub(crate) mod pins;
mod playing;
mod prefs_view;
mod session;
mod signal_card;
mod sources;
mod ui_state;
mod updating;
mod view_ctx;
mod window_state;

pub use updating::{UpdateAction, Updates};

// Views and their pieces.
mod album_list;
mod album_page;
mod alpha;
mod artists;
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
mod playlist_page;
mod queue_panel;
mod seek;
mod settings_page;
mod sidebar;
mod songs;
mod source_look;
mod table;
mod toolbar;
mod track_list;
mod track_row;
mod virtual_list;
mod visualizer_view;

#[cfg(test)]
mod fixture;
#[cfg(test)]
mod snapshots;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod ui_tests;

use crate::analyze;
use crate::art::Art;
use crate::bridge::{AnalysisMsg, Bridge, ListKey, Msg};
use crate::failure::Failure;
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
use library::Track;
use offline::Pin;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;
// What the tests (here, in `fixture` and its users) build their fixtures from.
#[cfg(test)]
use {
    crate::bridge::{LibraryMsg, OfflineMsg},
    crate::nav::Nav,
    crate::search::SearchIndex,
    crate::settings::Settings,
    crate::theme::ThemePref,
    crate::tracks::TrackSet,
    library::Album,
    offline::TrackRef,
    ui_state::QueueUi,
};

use crate::lyrics::SongStats;
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
pub use sources::{FolderTarget, SourceAction};
pub use ui_state::{AlbumSort, ArtistSort, DragTracks, Focus, QueueTab, UiState};
pub use window_state::WindowAction;

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
    /// The sidebar switcher and All sources filter chips.
    Scope(all_sources::ScopeAction),
    /// A source's own controls, independent of the browsing scope.
    ForSource(accounts::AccountId, Box<Action>),
    Go(Route),
    Back,
    Forward,
    BackBy(usize),
    /// An album's page with one of its tracks selected (⌘L: the playing one).
    Reveal {
        track: String,
        album: String,
    },
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
    /// Load an album's or a playlist's tracks again (a failed load's Retry).
    Reload(ListKey),
    /// Stop the track sync that's running.
    StopSync,
    /// Delete the cover art kept on disk (and forget what's loaded).
    ClearArtCache,
    /// Delete the streamed audio kept on disk.
    ClearStreamCache,
    /// Measure the server's round trip again, if the last reading is old.
    MeasureLatency,
    /// Ask whether to sign out (Sign out… in the server popover and Settings).
    AskSignOut,
    /// Back to the login screen; the saved password is forgotten. The
    /// answer to [`AskSignOut`](Action::AskSignOut).
    SignOut,
    /// The server turned the saved login down: the sign-in form, over the library.
    SignInAgain,
    /// Connect with what's typed in the login form.
    SignIn,
    /// Stop the login form's connect: whatever it answers later is dropped.
    CancelSignIn,
    /// Close the login form and keep browsing what's stored.
    CloseLogin,
    /// Something done on the first-run sheet.
    Onboarding(shell::kit::OnboardingEvent),
    /// Folders added or changed, a source removed.
    Sources(SourceAction),
    Update(UpdateAction),
    Album(String, QueueAction),
    Artist(String, Option<String>, QueueAction),
    Playlist(String, QueueAction),
    Tracks {
        tracks: Vec<Track>,
        source: Source,
        action: QueueAction,
    },
    /// The same, once the user said whether to keep the tracks they added
    /// (Settings › Queue › Playing something new: Ask).
    TracksAnswered {
        tracks: Vec<Track>,
        source: Source,
        action: QueueAction,
        keep: bool,
    },
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
    /// Split a combined album, named by its lead's account-qualified key,
    /// into its copies.
    KeepApart(String),
    /// Forget the pairs a Keep apart stored: its toast's Undo.
    UndoKeepApart(crate::store::ApartPairs),
    /// Clear the stored exclusions involving this account-qualified album.
    CombineAgain(String),
    /// Love or unlove a song, album or artist. `track` / `name` keep the
    /// Favorites view current.
    ToggleStar {
        kind: StarKind,
        id: String,
        track: Option<Box<Track>>,
        name: Option<String>,
    },
    /// Rate a song 1–5; 0 clears it.
    Rate(String, u8),
    /// A key or menu-bar command; applying it applies what it stands for.
    Command(Cmd),
    PlaylistEdit(PlaylistAction),
}

// A part's own action stands for the whole one, so a settings row can take
// `PrefsAction::Scrobble` where it needs something that makes an `Action`.
macro_rules! action_from {
    ($($part:ident => $variant:ident),* $(,)?) => {
        $(impl From<$part> for Action {
            fn from(a: $part) -> Self {
                Action::$variant(a)
            }
        })*
    };
}
action_from!(WindowAction => Window, PrefsAction => Prefs, OutputAction => Output, OfflineAction => Offline, AnalysisAction => Analysis);

/// The app's tokio runtime, kept for the app's lifetime (the bridge and the
/// background services hold handles to it).
///
/// Dropping a `Runtime` waits for every `spawn_blocking` call still running,
/// however long. Quitting gives them [`SHUTDOWN_WAIT`] and then goes: the work
/// there (renames, copies, keychain calls) is written to survive being cut off.
struct AppRuntime {
    rt: Option<tokio::runtime::Runtime>,
    /// How long those calls get. [`SHUTDOWN_WAIT`] unless a test says otherwise.
    wait: Duration,
}

const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

impl AppRuntime {
    fn new(rt: tokio::runtime::Runtime) -> Self {
        Self { rt: Some(rt), wait: SHUTDOWN_WAIT }
    }
}

impl Drop for AppRuntime {
    fn drop(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_timeout(self.wait);
        }
    }
}

pub struct OndApp {
    /// Must stay the first field. Fields drop in order, so the runtime shuts
    /// down (and its tasks are dropped) before anything declared after it.
    /// Nothing declared below may need the runtime in its `Drop`: by then it
    /// is gone, and a `block_on` or `spawn` there would panic or do nothing.
    _rt: AppRuntime,
    /// The window's egui context (for theme changes and window commands).
    ctx: egui::Context,
    bridge: Bridge,
    inbox: Receiver<Msg>,
    engine: EngineHandle,

    login: Option<LoginForm>,
    /// The first-run sheet, while it's up (first run, or after signing out).
    onboarding: Option<onboarding::Onboard>,
    /// The server connection and who's signed in.
    session: Session,

    data: Data,
    multi: all_sources::AllSources,
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
    /// A newer Ond that's out and not skipped.
    update: Option<crate::updates::Version>,
    /// The update checker looks while this is set (the setting).
    update_check: Arc<std::sync::atomic::AtomicBool>,
    /// Wakes the update checker for Check Now.
    update_now: Arc<tokio::sync::Notify>,
    updates: Updates,
    /// The folder picker (the system's; a test's answers at once).
    pick_folder: sources::FolderPicker,
    /// Save dialog for exporting playlists as M3U.
    save_m3u: sources::SaveFilePicker,
}

impl OndApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        #[cfg(windows)]
        now_playing::set_window(hwnd(cc));
        let mut app = Self::build(&cc.egui_ctx, Paths::resolve()?, audio::default_backend())?;
        app.gpu = cc.wgpu_render_state.as_ref().map(|s| crate::visuals::gpu::Target::new(s.target_format, &s.adapter));
        // The app is running by now, so NSApp takes the menu bar.
        let post = app.bridge.poster();
        app.menu = crate::menu::MenuBar::install(move |cmd| post(Msg::Menu(cmd)));
        app.update_check.store(app.prefs.get(|s| s.updates) != crate::settings::UpdateCheck::Off, std::sync::atomic::Ordering::Relaxed);
        app.bridge.start_updates(app.update_check.clone(), app.update_now.clone());
        // The window opened from this plan (see `main`); our title bar follows it.
        app.prefs.get(|s| s.window.plan()).apply_frame(&cc.egui_ctx);
        Ok(app)
    }

    fn build(ctx: &egui::Context, paths: Paths, backend: Box<dyn audio::OutputBackend>) -> anyhow::Result<Self> {
        // Four workers: downloads, sync and the server calls share them, so one
        // busy task can't hold up the stream that feeds playback.
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build()?;
        let paths = Arc::new(paths);
        let mut notices: Vec<String> = Vec::new();
        let (prefs, loaded) = Prefs::open(&paths.config_file);
        if let Loaded::Unreadable { aside } = loaded {
            notices.push(format!("Your settings couldn't be read, so Ond reset them. Sign in again. The old file is at {}.", aside.display()));
        }
        // A downloads folder set in managed.json replaced the user's own: the
        // downloads move there, as when one is picked in Settings.
        if let Some(was) = prefs.offline_dir_was() {
            let (from, to) = prefs.get(|s| (was.unwrap_or_else(|| paths.offline.clone()), s.offline_dir.clone().unwrap_or_else(|| paths.offline.clone())));
            if from != to && from.exists() && prefs.get(|s| s.offline_move_from.is_none()) {
                prefs.edit(|s| s.offline_move_from = Some(from));
            }
        }
        let data_dir = paths.library_db.parent().expect("the library has a data folder");
        // A failed move mustn't keep Ond from opening (launched from the Dock,
        // nobody would see why). This session runs on an empty mirror, which
        // the sync fills, and the next start tries the move again.
        let stores = match crate::storage::migrate(&paths, &prefs) {
            Ok(()) => crate::storage::open(data_dir, prefs.get(|s| s.account()), &mut notices)?,
            Err(e) => {
                tracing::error!("moving the library into its account folder: {e:#}");
                notices.push(format!("Ond couldn't move your library into its new folder ({e}). It will try again next time it starts."));
                crate::storage::Stores::in_memory()?
            }
        };
        let ond_store = match crate::store::OndStore::open_or_recover(data_dir.join("ond.redb")) {
            Ok((store, aside)) => {
                if aside.is_some() {
                    notices.push("Ond couldn't read its saved favorites and playlists, so it started a fresh store.".into());
                }
                Arc::new(store)
            }
            Err(e) => {
                tracing::error!("opening ond.redb: {e}");
                notices.push(format!("Ond couldn't open its favorites and playlists store ({e}). Changes won't persist this session."));
                Arc::new(crate::store::OndStore::in_memory()?)
            }
        };
        let apart_pairs = ond_store.apart_pairs().unwrap_or_else(|e| {
            notices.push(format!("Ond couldn't read its album separation choices ({e})."));
            Default::default()
        });
        let mut multi = all_sources::AllSources::default();
        multi.apart_pairs = apart_pairs;
        let analyses = stores.analyses;
        let (tx, inbox) = channel();
        let repaint = ctx.clone();
        let bridge = Bridge::new(rt.handle().clone(), tx, Arc::new(move || repaint.request_repaint()), stores.library, ond_store);
        bridge.configure_storage(data_dir.to_owned(), paths.art_cache.clone(), prefs.get(|s| s.account()), &analyses);
        let saved = prefs.snapshot();
        theme::install(ctx, saved.theme);
        // Before the first frame, so the window never shows at 100% first.
        shell::zoom::install(ctx, saved.zoom);
        let mut cache_cfg = streamcache::CacheConfig::new(&paths.stream_cache);
        cache_cfg.reply_check = crate::bridge::reply_check();
        if let Some(mb) = saved.stream_cache_mb {
            cache_cfg.max_bytes = mb * 1024 * 1024;
        }
        let cache = streamcache::StreamCache::new(cache_cfg, rt.handle().clone())?;
        let downloads = Downloads::start(
            saved.offline_dir.clone().unwrap_or_else(|| paths.offline.clone()),
            &saved,
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
        scratch_cfg.reply_check = crate::bridge::reply_check();
        let scratch_cache = streamcache::StreamCache::new(scratch_cfg, rt.handle().clone())?;
        let engine = audio::spawn(audio::EngineConfig::default(), backend, Some(cache));
        let output = Outputs::restore(&engine, prefs.clone());
        engine.send(Command::SetVolume(saved.volume));
        let scrobble = saved.scrobble;
        let post_remote = bridge.poster();
        let now_playing = now_playing::Session::start(now_playing::Config {
            account: saved.account(),
            status: engine.status.clone(),
            rt: rt.handle().clone(),
            art_dir: paths.art_cache.clone(),
            retry_file: saved.account().map(|a| crate::storage::account_dir(data_dir, a)).unwrap_or_else(|| data_dir.join("Sources")).join("scrobbles.json"),
            retry_root: Some(data_dir.to_owned()),
            scrobble,
            remote: Arc::new(move |r| post_remote(Msg::Remote(r))),
        });
        let tap = now_playing.clone();
        bridge.forward_engine_events(engine.events.clone(), move |ev| tap.engine_event(ev));
        // Working offline, the library loads once downloads are open (only
        // what can play shows).
        let work_offline = saved.work_offline && prefs.has_login();

        let analyzer = {
            let (post_analysis, ticks) = (bridge.poster(), bridge.notifier());
            analyze::Runner::start(analyze::Config {
                store: analyses,
                playback: playback_cache,
                scratch: scratch_cache,
                status: engine.status.clone(),
                on_result: Arc::new(move |key, record, session| {
                    let account = key.account;
                    let result = AnalysisMsg::Analyzed { key, record }.into();
                    post_analysis(if let Some(session) = session { Msg::Source { account, session, message: Box::new(result) } } else { result });
                }),
                on_progress: ticks,
            })
        };
        let art = Art::new(rt.handle().clone(), ctx.clone(), paths.art_cache.clone(), saved.account());

        // With a saved login the library opens at once (what's stored),
        // while the server is tried in the background.
        let login = if !prefs.has_login() {
            Some(LoginForm { server: "http://localhost:4533".into(), ..Default::default() })
        } else if saved
            .sources
            .iter()
            .all(|e| e.kind == crate::settings::SourceKind::Subsonic && (e.signed_out || (saved.signed_out && saved.account() == Some(e.id))))
        {
            Some(LoginForm { server: saved.server().to_owned(), username: saved.username().to_owned(), ..Default::default() })
        } else {
            None
        };
        let connecting = login.is_none() && !work_offline;
        // Without a login to resume, the window opens on the first-run sheet.
        let onboarding = login.is_some().then(onboarding::Onboard::default);

        let shown = PrefsView::read(&prefs);
        let mut player = Player::default();
        player.queue.prefs = prefs.get(|s| s.queue);
        player.stop_on_unplayable = saved.offline_unplayable == crate::settings::Unplayable::Pause;
        let mut app = Self {
            _rt: AppRuntime::new(rt),
            ctx: ctx.clone(),
            bridge,
            inbox,
            engine,
            login,
            onboarding,
            session: Session::new(prefs.has_login().then(|| saved.clone()), connecting, work_offline),
            data: Data::default(),
            multi,
            actions: Vec::new(),
            pending: Vec::new(),
            art,
            cache: app_cache,
            player,
            output,
            ui: UiState::new(notices, shown),
            bit_perfect: false,
            search: SearchState::default(),
            now_playing,
            downloads,
            diagnostics: diagnostics::Diagnostics::default(),
            waveforms: crate::visuals::waveform::Waveforms::new(ctx.clone()),
            gpu: None,
            update: None,
            update_check: Arc::default(),
            update_now: Arc::default(),
            updates: Updates::default(),
            pick_folder: sources::system_picker(),
            save_m3u: sources::system_save_m3u(),
            prefs,
            analysis: Analysis::new(analyzer),
            favorites: Favorites::default(),
            menu: None,
            lyrics: HashMap::new(),
            song_stats: HashMap::new(),
        };
        app.start_sources();
        Ok(app)
    }

    fn handle(&mut self, msg: impl Into<Msg>) {
        match msg.into() {
            Msg::CopySeparation(pairs) => {
                self.multi.apart_pairs = pairs;
                self.project_sources();
            }
            Msg::OndPlaylistChanged => self.refresh_ond_playlists(),
            Msg::MixedPlaylistReady { id, name, existing, added } => self.offer_mixed_playlist(id, name, existing, added),
            Msg::OndPlaylists { generation, playlists, tracks, entries } => self.ond_playlists_loaded(generation, playlists, tracks, entries),
            Msg::Source { account, session, message } => self.source_message(account, session, *message),
            Msg::Scoped { session, message } => {
                if session == self.bridge.session() {
                    self.handle(*message);
                }
            }
            // From before a sign-out: stale, whatever it says.
            Msg::Connected { session, .. } | Msg::ConnectFailed { session, .. } | Msg::Unreachable { session, .. } if session != self.bridge.session() => {}
            Msg::Connected { source, stores, settings, server, new_account, password_saved, .. } => {
                self.connected(source, stores, settings, server, new_account, password_saved)
            }
            Msg::ConnectFailed { error, .. } => self.connect_failed(error),
            Msg::Unreachable { username, error, .. } => self.unreachable(username, error),
            Msg::Latency(rtt) => self.diagnostics.set_latency(rtt),
            Msg::Library(m) => self.library_msg(m),
            Msg::Analysis(m) => self.analysis_msg(m),
            Msg::Engine(ev) => self.handle_engine(ev),
            Msg::Remote(r) => self.remote(r),
            Msg::Offline(m) => self.offline_msg(m),
            Msg::Starred { starred, ratings } => {
                if let Some(account) = self.session.focus.filter(|a| self.multi.bridges.contains_key(a)) {
                    self.multi.data.entry(account).or_default().served.starred = true;
                }
                self.data.served.starred = true;
                self.load_favorites(starred, ratings);
            }
            Msg::Lyrics { track_id, lyrics } => {
                self.lyrics.insert(track_id, crate::lyrics::Fetch::Done(lyrics));
            }
            Msg::SongStats { track_id, stats } => {
                self.multi.song_stats.insert(track_id, stats);
                self.project_song_stats();
            }
            Msg::UserMeta { meta, tracks } => self.favorites.note(meta, &tracks),
            Msg::StarDone { kind, id, on, seq, error } => self.star_done(kind, id, on, seq, error),
            Msg::RatingDone { id, previous, seq, error } => self.rating_done(id, previous, seq, error),
            Msg::PlaylistCreated { id, name } => {
                self.ui.recent_playlists.insert(0, id.clone());
                self.ui.recent_playlists.truncate(3);
                self.go(Route::Playlist { id, name });
            }
            Msg::M3uPathChosen { id, path } => {
                if id.starts_with("ond:") {
                    self.export_ond_m3u(id, path);
                } else {
                    self.bridge.export_playlist_m3u(self.session.source.clone(), id, path);
                }
            }
            Msg::M3uExported { path, error } => match error {
                None => {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("playlist.m3u");
                    self.show_notice(format!("Exported {name}"));
                }
                Some(e) => self.show_failure(e.context("Couldn't export the playlist")),
            },
            Msg::Menu(cmd) => self.command(cmd),
            Msg::FolderChosen { target, path } => self.folder_chosen(target, path),
            Msg::Update(msg) => self.update_message(msg),
            Msg::Error(e) => self.show_failure(e),
        }
    }

    /// Shows what failed. A request that got no answer has the server
    /// checked on (it may be gone); offline those are expected, and the
    /// offline pill already says so. A database that won't read says
    /// nothing about the server, so it's only shown.
    pub(crate) fn show_failure(&mut self, e: Failure) {
        self.show_failure_or_retry(e, None);
    }

    /// [`show_failure`](Self::show_failure), offering Retry when trying
    /// again is `retry`: something safe to repeat, like a load or a sync.
    pub(crate) fn show_failure_or_retry(&mut self, e: Failure, retry: Option<Action>) {
        if e.is_network() {
            self.probe();
            if !self.session.online {
                return;
            }
        }
        match retry {
            Some(action) => self.ui.toasts.error(e.into_message(), "Retry", action),
            None => self.show_error(e.into_message()),
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
        let account = all_sources::action_account(&action).or(match &action {
            Action::SyncNow
            | Action::StopSync
            | Action::MeasureLatency
            | Action::AskSignOut
            | Action::SignOut
            | Action::SignInAgain
            | Action::PlaylistEdit(_)
            | Action::Offline(OfflineAction::Keep(..)) => self.session.focus,
            _ => None,
        });
        if let Some(account) = account {
            if self.multi.bridges.contains_key(&account) {
                return self.with_source(account, |app| app.apply_inner(action));
            }
            if all_sources::action_account(&action).is_some() {
                return;
            }
        }
        self.apply_inner(action);
    }

    fn apply_inner(&mut self, action: Action) {
        if self.locked(&action) || !self.can(&action) {
            return;
        }
        if self.ui.visualizer.is_some() && Self::leaves_visualizer(&action) {
            self.close_visualizer();
        }
        // Browsing and the palette live in the full window.
        if matches!(action, Action::Go(_) | Action::OpenPalette) {
            self.set_mode(Some(WindowMode::Full));
        }
        match action {
            Action::Scope(a) => self.apply_scope(a),
            Action::ForSource(account, action) => {
                if self.multi.bridges.contains_key(&account) {
                    self.with_source(account, |app| app.apply(*action));
                }
            }
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
                if !self.lyrics.contains_key(&track.id) && self.session.source.is_some() {
                    self.lyrics.insert(track.id.clone(), crate::lyrics::Fetch::Loading);
                    self.bridge.fetch_lyrics(self.session.source.clone(), track.id.clone(), track.artist.clone(), track.title.clone());
                }
            }
            Action::NeedSongStats(id) => self.need_song_stats(id),
            Action::SyncNow => {
                if let Some(source) = self.session.live().cloned()
                    && (source.paged().is_some() || source.local().is_some())
                    && self.session.sync.begin()
                {
                    self.bridge.sync_source(&source);
                }
            }
            Action::StopSync => self.bridge.cancel_sync(),
            Action::Reload(key) => match key {
                ListKey::Album(id) => self.bridge.open_album(self.session.source.clone(), id),
                ListKey::Playlist(id) => self.bridge.open_playlist(self.session.source.clone(), id),
                ListKey::Artist(_) => {}
            },
            Action::ClearArtCache => self.art.clear(),
            Action::ClearStreamCache => self.cache.clear(),
            Action::MeasureLatency => self.refresh_latency(),
            Action::AskSignOut => self.ask_sign_out(),
            Action::SignOut => self.sign_out(),
            Action::SignInAgain => self.sign_in_again(),
            Action::SignIn => self.sign_in(),
            Action::CancelSignIn => self.cancel_sign_in(),
            Action::CloseLogin => self.login = None,
            Action::Onboarding(event) => self.onboarding_event(event),
            Action::Sources(a) => self.apply_source(a),
            Action::Update(a) => self.update_action(a),
            Action::Album(id, action) => self.play_album(id, action),
            Action::Artist(name, id, action) => self.play_artist(name, id, action),
            Action::Playlist(id, action) => self.play_playlist(id, action),
            Action::Tracks { tracks, source, action } => self.queue_tracks(tracks, source, action),
            Action::TracksAnswered { tracks, source, action, keep } => self.queue_tracks_answered(tracks, source, action, keep),
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
            Action::KeepApart(id) => self.keep_copies_apart(&id),
            Action::UndoKeepApart(pairs) => self.bridge.separate_copies(crate::store::Separation::Forget(pairs)),
            Action::CombineAgain(id) => {
                if let Ok(key) = accounts::ItemKey::parse(&id) {
                    self.bridge.separate_copies(crate::store::Separation::CombineAgain(key));
                }
            }
            Action::ToggleStar { kind, id, track, name } => self.toggle_star(kind, id, track.map(|t| *t), name),
            Action::Rate(id, rating) => self.rate(id, rating),
            Action::Command(cmd) => self.command(cmd),
            Action::PlaylistEdit(edit) => self.playlist_action(edit),
        }
    }

    /// Whether the source in use can do what `action` asks of it: the
    /// views don't offer what it can't, and this stops keys, menus and the
    /// palette too.
    fn can(&self, action: &Action) -> bool {
        let caps = self.session.caps();
        match action {
            Action::Offline(OfflineAction::Keep(Keep::Tracks(tracks), _)) if !self.multi.bridges.is_empty() => {
                tracks.iter().any(|t| self.session.for_item(&t.id).source_caps().downloads)
            }
            Action::Offline(OfflineAction::Keep(Keep::Playlist(id), _)) if id.starts_with("ond:") => {
                self.session.accounts.values().any(|s| s.source_caps().downloads)
            }
            Action::ToggleStar { .. } => caps.stars,
            Action::Rate(..) => caps.ratings,
            Action::NewPlaylist => caps.playlists,
            Action::PlaylistEdit(PlaylistAction::Create { .. } | PlaylistAction::Add { .. }) => caps.playlists,
            Action::Offline(OfflineAction::Keep(..)) => caps.downloads,
            Action::Offline(OfflineAction::WorkOffline(true)) => caps.network || self.session.accounts.values().any(|s| s.source_caps().network),
            _ => true,
        }
    }

    fn go(&mut self, route: Route) {
        self.ui.nav.go(route);
        self.on_arrive();
    }

    /// Loads what the place we just arrived at needs.
    fn on_arrive(&mut self) {
        let action = Action::Go(self.ui.nav.current().clone());
        if let Some(account) = all_sources::action_account(&action).filter(|a| self.multi.bridges.contains_key(a)) {
            return self.with_source(account, |app| app.on_arrive_inner());
        }
        self.on_arrive_inner();
    }

    fn on_arrive_inner(&mut self) {
        self.ui.track_sel = None;
        self.ui.focus = Focus::Main;
        match self.ui.nav.current().clone() {
            Route::Album(id) => self.bridge.open_album(self.session.source.clone(), id),
            Route::Artist { name, .. } => {
                if self.multi.bridges.is_empty() {
                    self.bridge.find_credited(name);
                } else {
                    for (account, bridge) in &self.multi.bridges {
                        if self.session.scope.includes(*account) {
                            bridge.find_credited(name.clone());
                        }
                    }
                }
            }
            Route::Playlist { id, .. } => {
                if id.starts_with("ond:") {
                    if !self.data.playlist_tracks.contains_key(&id) {
                        self.refresh_ond_playlists();
                    }
                    return;
                }
                // Offline, a kept playlist is rebuilt from its pin and the stored tracks.
                if !self.session.online
                    && !self.data.playlist_tracks.contains_key(&id)
                    && let Some((_, ids)) = self.downloads.store.as_ref().and_then(|s| s.pin_tracks(&Pin::Playlist(id.clone())))
                {
                    let by_id: HashMap<&str, &Track> = self.data.tracks.all.iter().map(|t| (t.id.as_str(), t)).collect();
                    let tracks: Vec<Track> = ids
                        .iter()
                        .filter_map(|id| {
                            let key = self.downloads.tracks().key(id)?;
                            by_id.get(key.item.as_str()).map(|t| (*t).clone())
                        })
                        .collect();
                    self.data.playlist_tracks.insert(id.clone(), Arc::new(tracks));
                }
                self.bridge.open_playlist(self.session.source.clone(), id)
            }
            Route::Favorites => {
                if self.multi.bridges.is_empty() {
                    self.bridge.fetch_starred(self.session.source.clone());
                } else {
                    for (account, bridge) in &self.multi.bridges {
                        if self.session.scope.includes(*account) {
                            bridge.fetch_starred(self.session.get(*account).and_then(|s| s.live()).cloned());
                        }
                    }
                }
            }
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
            // Zoom works over a sheet too, as it would in a browser.
            if self.ui.overlay.is_none() || matches!(cmd, Cmd::Shortcuts | Cmd::ZoomIn | Cmd::ZoomOut | Cmd::ZoomReset) {
                self.actions.push(Action::Command(cmd));
            }
        }
    }

    /// The window while signing in: the first-run sheet or the login form.
    fn sign_in_ui(&mut self, ui: &mut egui::Ui) {
        // Logging in needs the full window. (Read from this frame's copy
        // of the preferences, which `logic` took, not by locking them.)
        if self.ui.prefs.mode != WindowMode::Full {
            self.set_mode(Some(WindowMode::Full));
        }
        self.login_ui(ui);
        crate::csd::overlay(ui.ctx(), Some(toolbar::TOOLBAR_H));
        // The form asks for its changes as actions too, applied once it's drawn.
        for a in std::mem::take(&mut self.actions) {
            self.apply(a);
        }
    }

    fn main_ui(&mut self, ui: &mut egui::Ui) {
        // `logic` took this frame's copy just before; nothing between there
        // and here changes the preferences. Tests draw frames without
        // `logic`, so they take it here.
        #[cfg(test)]
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
        let shadow = egui::Shadow {
            offset: [-6, 0],
            blur: 24,
            spread: 0,
            color: egui::Color32::from_black_alpha(if ctx.global_style().visuals.dark_mode { 120 } else { 40 }),
        };
        let modal = egui::Modal::new(id)
            .area(area)
            .backdrop_color(egui::Color32::from_black_alpha(18))
            .frame(egui::Frame::new().fill(t.panel).shadow(shadow).stroke(egui::Stroke::new(1.0, t.hairline)).inner_margin(egui::Margin {
                left: 8,
                right: 8,
                top: 12,
                bottom: 8,
            }))
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
        self.output.tick(ctx, &self.engine);
        // The rest of this frame reads the preferences as they are now.
        self.read_prefs();
        // Saved in the background: a failure there is told here, once.
        if let Some(e) = self.prefs.take_write_error() {
            self.show_error(format!("Your settings couldn't be saved ({e}). Changes made now may be lost when Ond quits."));
        }
        let queue = if self.ui.queue_narrow { self.ui.queue_sheet } else { self.ui.prefs.show_queue };
        let (theme, diagnostics) = (self.ui.prefs.theme, self.ui.prefs.diagnostics_bar);
        if let Some(menu) = self.menu.as_mut() {
            menu.sync(queue, self.bit_perfect, theme);
            menu.set_diagnostics(diagnostics);
        }
        // Back at the window: if the server's away, try it now rather than
        // at the end of the wait.
        if ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(true)))) {
            if self.multi.bridges.is_empty() && self.session.unreachable {
                self.bridge.retry_now();
            }
            for (account, bridge) in &self.multi.bridges {
                if self.session.get(*account).is_some_and(|s| s.unreachable) {
                    bridge.retry_now();
                }
            }
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
            // A track held for the server can wait as long as a keychain
            // prompt does; its spinner turns slower rather than spin the
            // frame rate up for all that time.
            _ if self.player.held => ctx.request_repaint_after(Duration::from_millis(100)),
            _ => {}
        }
        crate::chrome::sync(ctx, crate::chrome::Bar { height: title_bar_h(self.ui.prefs.mode) });
        self.track_window(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let started = std::time::Instant::now();
        if self.signing_in() {
            self.sign_in_ui(ui);
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
        // Before the process exits: the output puts the device back the way it found it.
        self.engine.shutdown(ENGINE_SHUTDOWN);
    }
}

/// How long quitting waits for the audio engine to let go of the device.
const ENGINE_SHUTDOWN: std::time::Duration = std::time::Duration::from_secs(2);

impl Drop for OndApp {
    fn drop(&mut self) {
        self.bridge.cancel_sync();
        self.engine.shutdown(ENGINE_SHUTDOWN);
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
