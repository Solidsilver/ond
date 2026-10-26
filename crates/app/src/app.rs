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
mod track_row;
mod view_ctx;
mod visualizer_view;
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
use crate::player::{self, Note, Player, QueueEdit};
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
use std::path::{Path, PathBuf};
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

    /// Empties it for another account's library. Versions only move forward,
    /// so derived lists never mistake the empty library for one they've seen.
    fn clear(&mut self) {
        let (version, tracks_version) = (self.version + 1, self.tracks_version + 1);
        *self = Data { version, tracks_version, ..Data::default() };
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
    /// The Info tab wants a song's play count (asked once per song).
    NeedSongStats(String),
    /// A lyrics view wants a song's lyrics (asked once per song).
    NeedLyrics(Box<Track>),
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
    /// The accent color used without (or instead of) the album art's.
    SetAccent(theme::Accent),
    /// Whether colors adapt to the album art.
    SetAdaptColors(bool),
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
    /// The system's title bar instead of our own window buttons (Linux).
    SetSystemTitleBar(bool),
    /// Turn experimental visuals on or off.
    SetExperimental(crate::visuals::Experimental),
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

/// A queue action waiting for tracks to load.
struct Pending {
    key: ListKey,
    source: Source,
    action: QueueAction,
    /// What's stored is known to be incomplete (an album a sync stored only
    /// part of): wait for the server's own listing, not the stored copy.
    fresh: bool,
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

/// How long a mode switch waits for the window to leave maximized.
const UNMAXIMIZE_WAIT: Duration = Duration::from_secs(1);

/// Waiting for the window to reach a size we asked for.
#[derive(Debug, Clone, Copy)]
pub struct Settle {
    pub want: [f32; 2],
    /// Give up (and take the window as it is) after this.
    until: std::time::Instant,
    /// When the size was last asked for.
    pub asked: std::time::Instant,
}

impl Settle {
    const WAIT: Duration = Duration::from_millis(1500);
    const REASK: Duration = Duration::from_millis(200);

    fn new(want: [f32; 2]) -> Self {
        let now = std::time::Instant::now();
        Self { want, until: now + Self::WAIT, asked: now }
    }

    /// Still short of the size, and still willing to wait for it.
    fn waiting(&self, size: [f32; 2]) -> bool {
        let off = (size[0] - self.want[0]).abs() > 2.0 || (size[1] - self.want[1]).abs() > 2.0;
        off && std::time::Instant::now() < self.until
    }
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
    /// Mini is held square: what was last asked of the platform, and whether it
    /// could (if not, the size is snapped back each frame instead).
    pub square_wanted: Option<bool>,
    pub square_native: bool,
    /// The window as last seen, saved on quit.
    pub seen: Option<Geometry>,
    /// A mode switch waiting for the window to leave maximized first: a
    /// compositor answers an un-maximize with the window's old size, which
    /// would land after (and over) the new one.
    pub unmaximizing: Option<(crate::window::Plan, std::time::Instant)>,
    /// After a mode switch, until the window reaches the size asked for. It
    /// reports its old size for a frame or two meanwhile.
    pub settling: Option<Settle>,
    /// When the current mode switch was asked for (for the debug log).
    pub switch_started: Option<std::time::Instant>,
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
}

impl UiState {
    fn new(notices: Vec<String>) -> Self {
        Self {
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
            square_wanted: None,
            square_native: false,
            seen: None,
            unmaximizing: None,
            settling: None,
            switch_started: None,
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

/// Whether `track` can play right now: downloaded, or the server is there.
fn playable(session: &Session, downloads: &Downloads, track: &Track) -> bool {
    session.online || downloads.snap.is_done(&track.id)
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

impl SyncState {
    /// Marks an album sync as started, if none is running; returns whether to
    /// start one.
    fn begin(&mut self) -> bool {
        if self.albums || self.tracks {
            return false;
        }
        self.label = "syncing…".into();
        self.albums = true;
        true
    }
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
    /// The open store. `None` while one is being opened off the UI thread
    /// (another account signed in, or the downloads folder is moving), or if
    /// that failed: meanwhile nothing plays from disk and download actions
    /// wait in `deferred`.
    pub store: Option<OfflineStore>,
    pub snap: Arc<offline::Snapshot>,
    /// Pins waiting for their track lists to load, by the same keys as `pending`.
    pub pending_pins: Vec<(ListKey, Pin)>,
    /// The downloads folder; each account's store is a folder inside it.
    pub base: PathBuf,
    /// Whose store is open, or opening (`None` before anyone has signed in).
    account: Option<u32>,
    /// Numbers the opens started. A store that arrives for an older one (the
    /// user switched again meanwhile) is closed unused.
    generation: u64,
    /// An open is under way.
    opening: bool,
    /// Download actions taken while the store was opening, run once it's open.
    deferred: Vec<Action>,
    /// The notice showing the downloads folder's move, while it moves.
    moving: Option<String>,
    open: OpenStore,
}

/// Opens the offline store kept in a folder, with an optional size limit.
type OpenStore = Arc<dyn Fn(PathBuf, Option<u64>) -> anyhow::Result<OfflineStore> + Send + Sync>;

/// A move's progress notice is updated at most this often.
const MOVE_PROGRESS_EVERY: Duration = Duration::from_millis(500);

impl Downloads {
    /// The store folder for `account` under `base`. Before any sign-in there is
    /// no account; an empty store stands in until one connects.
    fn dir_for(base: &Path, account: Option<u32>) -> std::io::Result<PathBuf> {
        match account {
            Some(a) => offline::account_store(base, a),
            None => Ok(base.join("signed-out")),
        }
    }

    /// Opens `account`'s store in place of the current one (another server or
    /// user signed in), in the background (see [`open_in_background`](Self::open_in_background)).
    /// Its pins and files stay apart from everyone else's.
    fn switch_account(&mut self, account: Option<u32>, limit: Option<u64>, bridge: &Bridge) {
        if account == self.account && (self.store.is_some() || self.opening) {
            return;
        }
        self.account = account;
        self.open_in_background(limit, None, bridge);
    }

    /// Shuts the current store down and opens `self.account`'s, on the
    /// bridge's offline queue: reading its pins and checking its files takes a
    /// while with thousands of downloads. With `move_from`, the downloads are
    /// first moved from there into `base`. The store arrives as
    /// `Msg::OfflineStore`, for [`opened`](Self::opened).
    fn open_in_background(&mut self, limit: Option<u64>, move_from: Option<PathBuf>, bridge: &Bridge) {
        self.generation += 1;
        self.opening = true;
        self.pending_pins.clear();
        self.snap = Arc::default();
        let old = self.store.take();
        let (generation, base, account, open) = (self.generation, self.base.clone(), self.account, self.open.clone());
        bridge.offline_job(move |b| {
            // First, as it may be the same folder's: this waits for its last save.
            drop(old);
            if let Some(from) = move_from {
                let mut posted: Option<std::time::Instant> = None;
                let moved = offline::move_store_with(&from, &base, &mut |p| {
                    if posted.is_none_or(|at| at.elapsed() >= MOVE_PROGRESS_EVERY) {
                        posted = Some(std::time::Instant::now());
                        b.post(Msg::OfflineMoving(p));
                    }
                });
                if let Err(e) = &moved {
                    tracing::warn!("moving downloads from {}: {e}", from.display());
                }
                b.post(Msg::OfflineMoved(moved.map_err(|e| e.to_string())));
            }
            let store = Self::dir_for(&base, account).map_err(anyhow::Error::from).and_then(|dir| open(dir, limit));
            b.post(Msg::OfflineStore { generation, store: store.map_err(|e| format!("{e:#}")) });
        });
    }

    /// A store opened in the background arrived. Returns whether it's the one
    /// waited for, now in use; one for an older open is shut down (off this
    /// thread, in turn) instead.
    fn opened(&mut self, generation: u64, store: Result<OfflineStore, String>, bridge: &Bridge) -> Result<bool, String> {
        if generation != self.generation {
            if let Ok(store) = store {
                bridge.offline_job(move |_| drop(store));
            }
            return Ok(false);
        }
        self.opening = false;
        self.store = Some(store?);
        self.refresh();
        Ok(true)
    }

    /// Download actions that need the store: they wait while it opens.
    fn needs_store(action: &Action) -> bool {
        matches!(
            action,
            Action::KeepOffline(..) | Action::Unpin(_) | Action::PauseDownloads(_) | Action::RetryDownloads | Action::SetOfflineLimit(_)
        )
    }

    /// The finished file for track `id`, if there is one.
    pub fn local_path(&self, id: &str) -> Option<PathBuf> {
        self.store.as_ref()?.local_path(id)
    }

    /// Takes in what changed since the last frame.
    pub fn refresh(&mut self) {
        if let Some(store) = &self.store {
            self.snap = store.snapshot();
        }
    }

    /// The open store (tests).
    #[cfg(test)]
    pub fn store(&self) -> &OfflineStore {
        self.store.as_ref().expect("the offline store is open")
    }

    /// Applies a download action. Returns a notice to show, or an error.
    fn apply(&mut self, action: Action, prefs: &Prefs) -> Result<Option<String>, String> {
        let store = || self.store.clone().ok_or_else(|| "Downloads aren't open".to_owned());
        match action {
            Action::Unpin(pin) => {
                store()?.unpin(&pin);
                self.refresh();
            }
            Action::PauseDownloads(paused) => store()?.set_paused(paused),
            Action::RetryDownloads => store()?.retry_failed(),
            Action::SetOfflineLimit(bytes) => {
                store()?.set_limit(bytes);
                prefs.edit(|s| s.offline_limit_gb = bytes.map(|b| b / 1_000_000_000));
            }
            Action::RevealOffline => {
                crate::folder::reveal(&self.base).map_err(|e| format!("open {}: {e}", self.base.display()))?;
            }
            _ => unreachable!("not a download action"),
        }
        Ok(None)
    }

    /// Moves downloads into `chosen` at the next start. Returns the notice to show.
    fn move_to(&self, chosen: PathBuf, prefs: &Prefs) -> Option<String> {
        // Always a folder of our own inside the chosen one: the store tidies
        // files it doesn't know, which must never be the user's.
        let dir = if chosen.file_name().is_some_and(|n| n == "Ond Offline") { chosen } else { chosen.join("Ond Offline") };
        if dir == self.base {
            return None;
        }
        if self.moving.is_some() {
            // Its source would be forgotten if Ond quit before the move ends.
            return Some("Downloads are still moving. Choose another folder once they're done.".into());
        }
        let current = self.base.clone();
        prefs.edit(|s| {
            s.offline_dir = Some(dir.clone());
            s.offline_move_from = Some(current);
        });
        Some(format!("Downloads move to {} the next time Ond starts", dir.display()))
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
    /// This session queued the library-wide run (see `check_library_analysis`).
    pub library_run: bool,
}

impl Analysis {
    /// Applies the analysis actions that need only the runner and preferences.
    fn apply(&mut self, action: Action, prefs: &Prefs) {
        match action {
            Action::PauseAnalysis(paused) => self.runner.set_paused(paused),
            Action::StopAnalysis => {
                self.runner.clear();
                self.runner.set_paused(false);
                // A library-wide run isn't resumed at the next launch either.
                prefs.edit(|s| s.analysis_resume = false);
            }
            Action::SetAnalyzePlaying(on) => prefs.edit(|s| s.analyze_playing = on),
            _ => unreachable!("not an analysis action"),
        }
    }

    /// A track's analysis, if there is one for the file as it is now.
    pub fn of(&self, t: &Track) -> Option<&analysis::TrackAnalysis> {
        self.records.get(&t.id).filter(|r| r.stamp == analyze::stamp_of(t)).map(|r| &r.analysis)
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
        let downloads = {
            let base = saved.offline_dir.clone().unwrap_or_else(|| paths.offline.clone());
            let open: OpenStore = {
                let (rt, ctx, cache) = (rt.handle().clone(), ctx.clone(), cache.clone());
                Arc::new(move |dir, limit| {
                    let repaint = ctx.clone();
                    let store = OfflineStore::open(offline::OfflineConfig { dir, limit }, rt.clone(), Arc::new(move || repaint.request_repaint()))?;
                    // A track that finished streaming is linked in rather than downloaded again.
                    let cached = cache.clone();
                    store.set_cache_lookup(Arc::new(move |r: &TrackRef| cached.complete_path(&streamcache::CacheKey::original(r.id.clone(), r.suffix.clone()))));
                    Ok(store)
                })
            };
            let account = if prefs.has_login() { saved.account_id() } else { None };
            let limit = saved.offline_limit_gb.map(|gb| gb * 1_000_000_000);
            let mut downloads = Downloads {
                store: None,
                snap: Arc::default(),
                pending_pins: Vec::new(),
                base,
                account,
                generation: 0,
                opening: false,
                deferred: Vec::new(),
                moving: None,
                open,
            };
            match saved.offline_move_from.clone() {
                // A new location chosen last time: the downloads move there
                // first, in the background (a copy across drives takes a while).
                Some(from) => {
                    let notice = format!("Moving downloads to {}…", downloads.base.display());
                    notices.push(notice.clone());
                    downloads.moving = Some(notice);
                    downloads.open_in_background(limit, Some(from), &bridge);
                }
                None => {
                    let store = (downloads.open)(Downloads::dir_for(&downloads.base, account)?, limit)?;
                    downloads.store = Some(store);
                    downloads.refresh();
                }
            }
            downloads
        };
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
            ui: UiState::new(notices),
            bit_perfect: false,
            search: SearchState::default(),
            now_playing,
            downloads,
            diagnostics: diagnostics::Diagnostics::default(),
            waveforms: crate::visuals::waveform::Waveforms::new(ctx.clone()),
            gpu: None,
            prefs,
            analysis: Analysis { records: HashMap::new(), version: 0, unanalysed: Default::default(), loaded: false, runner: analyzer, library_run: false },
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
            Msg::Connected { client, settings, server, new_account, .. } => {
                if new_account {
                    self.forget_library();
                }
                let limit = self.prefs.snapshot().offline_limit_gb.map(|gb| gb * 1_000_000_000);
                self.downloads.switch_account(settings.account_id(), limit, &self.bridge);
                self.session.server_label = format!("{} · {}", settings.username, server);
                self.session.server_info = server.clone();
                self.login = None;
                // Always a new pass, even if the flags say one runs: any still
                // running is from before the server went away, and the bridge
                // has this one replace it.
                self.sync.label = "syncing…".into();
                self.sync.albums = true;
                self.bridge.sync_albums(client.clone());
                self.bridge.fetch_home(client.clone());
                self.art.set_client(client.clone());
                self.now_playing.set_client(client.clone());
                self.output.connected(&settings.server);
                // A store still opening gets the server once it's open.
                self.connect_downloads(&client);
                let url = client.clone();
                self.analysis.runner.set_server(Some(Arc::new(move |id: &str| url.stream_url(id).to_string())));
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
                self.sync.label = format!("Syncing… {n}");
            }
            Msg::AlbumSyncDone(Ok(n)) => {
                self.sync.albums = false;
                self.sync.label = format!("{n} albums");
                self.sync.tracks = true;
            }
            Msg::AlbumSyncDone(Err(e)) => {
                // Nothing more follows (no artist or track sync): stop saying "syncing".
                self.sync.albums = false;
                self.sync.label = "Sync failed".into();
                self.probe();
                if self.session.online {
                    self.show_error(e);
                }
            }
            Msg::LoadFailed { key, error } => {
                // One that only waited for a complete listing plays what's stored.
                if let Some(at) = self.pending.iter().position(|p| p.key == key && p.fresh) {
                    self.pending[at].fresh = false;
                    self.run_pending(&key, false);
                }
                // Whatever else waited for these tracks gives up, so it can't
                // fire much later when they turn up for some other reason.
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
                self.run_pending(&ListKey::Album(album_id), fresh);
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
                self.run_pending(&ListKey::Playlist(id), true);
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
            Msg::OfflineFolder(dir) => {
                if let Some(notice) = self.downloads.move_to(dir, &self.prefs) {
                    self.show_notice(notice);
                }
            }
            Msg::OfflineStore { generation, store } => match self.downloads.opened(generation, store, &self.bridge) {
                Ok(true) => {
                    if let Some(client) = self.session.live().cloned() {
                        self.connect_downloads(&client);
                    }
                    // Download actions taken while it opened.
                    self.actions.append(&mut self.downloads.deferred);
                    if !self.session.online {
                        // What's downloaded can play again.
                        self.sync_next();
                    }
                }
                Ok(false) => {}
                Err(e) => {
                    self.downloads.deferred.clear();
                    self.show_error(format!("Downloads couldn't be opened: {e}"));
                }
            },
            Msg::OfflineMoving(p) => {
                let text = format!("Moving downloads to {}: {} of {}", self.downloads.base.display(), crate::fmt::bytes(p.done), crate::fmt::bytes(p.total));
                self.moving_notice(text, false);
            }
            Msg::OfflineMoved(result) => {
                let base = self.downloads.base.display().to_string();
                match result {
                    Ok(()) => {
                        self.prefs.edit(|s| s.offline_move_from = None);
                        self.moving_notice(format!("Downloads moved to {base}"), false);
                    }
                    // The pref stays: the next start carries on where this stopped.
                    Err(e) => self.moving_notice(format!("Some downloads couldn't be moved to {base} ({e}). Ond tries again next time it starts."), true),
                }
                self.downloads.moving = None;
            }
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

    /// Something failed; if it was the network, find out whether the server is gone.
    fn probe(&self) {
        if self.session.online
            && let (Some(client), Some(settings)) = (&self.session.client, &self.session.settings)
        {
            self.bridge.probe(client.clone(), settings.clone());
        }
    }

    /// The server's gone: browse what's stored, play what's downloaded.
    /// Another server or user signed in: what was shown came from the old one.
    /// The library database was cleared already; this drops the in-memory copy,
    /// the queue and the cached audio, which a sync then refills.
    fn forget_library(&mut self) {
        self.with_player(|p, cx| p.reset(cx));
        self.data.clear();
        self.search.stale = true;
        self.pending.clear();
        self.favorites = Favorites::default();
        self.lyrics.clear();
        self.song_stats.clear();
        self.ui.nav = Nav::default();
        self.cache.clear();
    }

    /// Hands the offline store the server, and refetches pinned playlists so
    /// what was added to them downloads. Nothing while the store is opening:
    /// it gets the same once it's open.
    fn connect_downloads(&mut self, client: &Client) {
        let Some(store) = &self.downloads.store else { return };
        let url = client.clone();
        store.set_source(Some(Arc::new(move |id: &str| url.stream_url(id).to_string())));
        for p in &self.downloads.snap.pins {
            if let Pin::Playlist(id) = &p.pin {
                self.bridge.open_playlist(Some(client.clone()), id.clone());
            }
        }
    }

    /// Shows `text` in place of the move's last notice.
    fn moving_notice(&mut self, text: String, error: bool) {
        let old = self.downloads.moving.replace(text.clone());
        self.ui.toasts.replace(old.as_deref(), text, error);
    }

    fn set_offline(&mut self) {
        self.session.online = false;
        self.session.client = None;
        if let Some(store) = &self.downloads.store {
            store.set_source(None);
        }
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
        // While a store opens there's nothing to refresh: its pins aren't
        // known yet, and none waits (opening clears `pending_pins`).
        let Some(store) = self.downloads.store.clone() else { return };
        let label = self.pin_label(&pin);
        let refs = || tracks.iter().map(track_ref).collect::<Vec<_>>();
        if let Some(at) = self.downloads.pending_pins.iter().position(|(_, p)| p == &pin) {
            self.downloads.pending_pins.remove(at);
            store.pin(pin, label, refs());
        } else if store.is_pinned(&pin) {
            if complete {
                store.update(&pin, label, refs());
            } else {
                store.extend(&pin, refs());
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

    /// In the full window: shows the sidebar on `tab`, or back to Up Next when
    /// it's already showing that tab.
    fn toggle_side_tab(&mut self, tab: QueueTab) {
        if self.ui.queue_ui.tab == tab && (self.show_queue() || self.ui.queue_sheet) {
            self.ui.queue_ui.tab = QueueTab::UpNext;
            return;
        }
        self.ui.queue_ui.tab = tab;
        if self.ui.queue_narrow {
            self.ui.queue_sheet = true;
        } else if !self.show_queue() {
            self.actions.push(Action::ToggleQueue);
        }
    }

    /// Runs only with the store open (see `apply`, which holds it back while one opens).
    fn keep_offline(&mut self, what: Keep, on: bool) {
        let Some(store) = self.downloads.store.clone() else { return };
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
                    store.pin_many(tracks.iter().map(|t| (Pin::Track(t.id.clone()), t.title.clone(), vec![track_ref(t)])).collect());
                } else {
                    store.unpin_many(&tracks.iter().map(|t| Pin::Track(t.id.clone())).collect::<Vec<_>>());
                }
                self.downloads.refresh();
                return;
            }
        };
        match refs {
            _ if !on => store.unpin(&pin),
            Some(refs) => store.pin(pin.clone(), self.pin_label(&pin), refs),
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
        if r == Remote::Raise {
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            return;
        }
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
        if let Some(p) = self.ui.palette.as_mut() {
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
            offline: self.downloads.store.as_ref(),
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

    #[cfg(test)]
    fn start_current(&mut self) {
        self.with_player(|p, cx| p.start_current(cx));
    }

    fn queue_edit(&mut self, edit: QueueEdit) {
        let mut selected = self.ui.queue_ui.selected;
        self.with_player(|p, cx| p.edit(edit, &mut selected, cx));
        self.ui.queue_ui.selected = selected;
    }

    /// Where `track` would play from now (tests).
    #[cfg(test)]
    fn source(&self, token: u64, prev: Option<&Track>, track: &Track, next: Option<&Track>) -> Option<TrackSource> {
        use player::Sources;
        let sources = AppSources {
            offline: self.downloads.store.as_ref(),
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

    /// Runs what waited for `key`'s tracks, now loaded (`fresh`: the server's
    /// own listing, not the stored copy).
    fn run_pending(&mut self, key: &ListKey, fresh: bool) {
        let Some(at) = self.pending.iter().position(|p| &p.key == key && (fresh || !p.fresh)) else { return };
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
        if self.ui.visualizer.is_some() && Self::leaves_visualizer(&action) {
            self.close_visualizer();
        }
        // Browsing and the palette live in the full window.
        if matches!(action, Action::Go(_) | Action::OpenPalette) {
            self.set_mode(Some(WindowMode::Full));
        }
        if self.downloads.store.is_none() && Downloads::needs_store(&action) {
            if self.downloads.opening {
                self.downloads.deferred.push(action);
            } else {
                self.show_error("Downloads couldn't be opened. Restart Ond to try again.");
            }
            return;
        }
        match action {
            Action::Go(route) => self.go(route),
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
            Action::Album(id, action) => {
                let name = self.data.album(&id).map(|a| a.name.clone()).unwrap_or_default();
                let source = Source::Album { id: id.clone(), name };
                // An album a sync stored only part of (cut short) would play a
                // subset: fetch it, unless there's no server to ask.
                let stored = self.data.tracks.album(&id);
                let count = self.data.album(&id).map_or(0, |a| a.song_count as usize);
                let partial = stored.is_some_and(|t| t.len() < count) && self.session.client.is_some();
                match stored {
                    Some(t) if !partial => self.queue_tracks(t.to_vec(), source, action),
                    _ => {
                        self.wait_for(Pending { key: ListKey::Album(id.clone()), source, action, fresh: partial });
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
                self.wait_for(Pending { key: key.clone(), source, action, fresh: false });
                self.bridge.fetch_tracks(self.session.client.clone(), key, ids);
            }
            Action::Playlist(id, action) => {
                let name = self.data.playlists.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_default();
                let source = Source::Playlist { id: id.clone(), name };
                match self.data.playlist_tracks.get(&id) {
                    Some(t) => self.queue_tracks(t.to_vec(), source, action),
                    None => {
                        self.wait_for(Pending { key: ListKey::Playlist(id.clone()), source, action, fresh: false });
                        self.bridge.open_playlist(self.session.client.clone(), id);
                    }
                }
            }
            Action::Tracks { tracks, source, action } => self.queue_tracks(tracks, source, action),
            Action::JumpTo(uid) => self.queue_edit(QueueEdit::JumpTo(uid)),
            Action::Replay(uid) => self.queue_edit(QueueEdit::Replay(uid)),
            Action::Remove(uid) => self.queue_edit(QueueEdit::Remove(uid)),
            Action::Move(uid, to) => self.queue_edit(QueueEdit::Move(uid, to)),
            Action::MoveBy(uid, delta) => self.queue_edit(QueueEdit::MoveBy(uid, delta)),
            Action::ClearQueue => self.queue_edit(QueueEdit::Clear),
            Action::SetEnd(end) => self.queue_edit(QueueEdit::SetEnd(end)),
            Action::PlayPause => self.play_pause(),
            Action::Next => self.next(),
            Action::Previous => self.previous(),
            Action::Seek(to) => self.seek(to),
            Action::OpenPalette => {
                if self.ui.palette.is_none() && self.search.index.is_some() {
                    self.ui.palette = Some(Palette::default());
                }
            }
            Action::SetTheme(pref) => {
                theme::set_pref(&self.ctx, pref);
                self.prefs.edit(|s| s.theme = pref);
            }
            Action::SetAccent(accent) => self.prefs.edit(|s| s.accent = accent),
            Action::SetAdaptColors(on) => self.prefs.edit(|s| s.adapt_colors = on),
            Action::ToggleQueue if self.ui.queue_narrow => self.ui.queue_sheet = !self.ui.queue_sheet,
            Action::ToggleQueue => self.prefs.edit(|s| s.show_queue = !s.show_queue),
            Action::SignOut => self.sign_out(),
            Action::SetMode(mode) => self.set_mode(Some(mode)),
            Action::ToggleMini => self.set_mode(None),
            Action::TogglePlayer => {
                let seen = self.ui.seen;
                let plan = self.edit_windows(|w| w.toggle_player(seen));
                self.follow_plan(plan);
            }
            Action::Lyrics => match self.window_mode() {
                WindowMode::Full => self.toggle_side_tab(QueueTab::Lyrics),
                WindowMode::Player => self.actions.push(Action::SetLyricsDrawer(!self.lyrics_drawer())),
                WindowMode::Mini => {
                    self.edit_windows(|w| w.lyrics_open = true);
                    self.set_mode(Some(WindowMode::Player));
                }
            },
            Action::Info => match self.window_mode() {
                WindowMode::Full => self.toggle_side_tab(QueueTab::Info),
                WindowMode::Player => {
                    self.ui.queue_ui.tab = if self.ui.queue_ui.tab == QueueTab::Info { QueueTab::UpNext } else { QueueTab::Info };
                }
                WindowMode::Mini => {
                    self.ui.queue_ui.tab = QueueTab::Info;
                    self.set_mode(Some(WindowMode::Player));
                }
            },
            Action::SetLyricsDrawer(open) => {
                let seen = self.ui.seen;
                let plan = self.edit_windows(|w| w.set_lyrics(open, seen));
                self.follow_plan(plan);
            }
            Action::OpenSettings(section) => {
                self.ui.settings_section = section;
                self.set_mode(Some(WindowMode::Full));
                self.ui.overlay = Some(Overlay::Settings);
            }
            Action::SyncNow => {
                if let Some(client) = self.session.live().cloned()
                    && self.sync.begin()
                {
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
            Action::SetExperimental(x) => self.prefs.edit(|s| s.experimental = x),
            Action::SetSystemTitleBar(on) => {
                let plan = self.edit_windows(|w| {
                    w.system_title_bar = on;
                    w.plan()
                });
                plan.apply_frame(&self.ctx);
            }
            Action::KeepOffline(what, on) => self.keep_offline(what, on),
            Action::MoveOffline => {
                let base = &self.downloads.base;
                let start = base.parent().unwrap_or(base).to_owned();
                let bridge = self.bridge.clone();
                crate::folder::pick("Choose where to keep downloaded music", &start, move |chosen| {
                    if let Some(dir) = chosen {
                        bridge.post(Msg::OfflineFolder(dir));
                    }
                });
            }
            a @ (Action::Unpin(_)
            | Action::PauseDownloads(_)
            | Action::RetryDownloads
            | Action::SetOfflineLimit(_)
            | Action::RevealOffline) => match self.downloads.apply(a, &self.prefs) {
                Ok(Some(notice)) => self.show_notice(notice),
                Ok(None) => {}
                Err(e) => self.show_error(e),
            },
            Action::Analyze(tracks) => {
                self.analysis.runner.set_paused(false);
                self.enqueue_analysis(&tracks);
            }
            Action::AnalyzeLibrary => {
                self.prefs.edit(|s| s.analysis_resume = true);
                self.analysis.runner.set_paused(false);
                self.resume_library_analysis();
            }
            a @ (Action::PauseAnalysis(_) | Action::StopAnalysis | Action::SetAnalyzePlaying(_)) => self.analysis.apply(a, &self.prefs),
            Action::ToggleStar { kind, id, track, name } => self.toggle_star(kind, id, track.map(|t| *t), name),
            Action::Rate(id, rating) => self.rate(id, rating),
            Action::Command(cmd) => self.command(cmd),
            Action::Overlay(o) => self.ui.overlay = Some(o),
            Action::PlaylistEdit(edit) => self.playlist_action(edit),
        }
    }

    /// A track's analysis, if there is one for the file as it is now.
    pub fn analysis_of(&self, t: &Track) -> Option<&analysis::TrackAnalysis> {
        self.analysis.of(t)
    }

    /// Queues the tracks that have no current analysis; returns how many.
    /// Downloaded copies are read straight from disk (and need no server).
    /// The stream URLs are made by the runner as it gets to each track.
    fn enqueue_analysis(&self, tracks: &[Track]) -> usize {
        let online = self.session.client.is_some();
        let jobs: Vec<analyze::Job> = tracks
            .iter()
            .filter(|t| self.analysis_of(t).is_none())
            .filter_map(|t| {
                let local = self.downloads.local_path(&t.id);
                (online || local.is_some()).then(|| analyze::job_for(t, local))
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
            self.analysis.library_run = true;
            if !self.analysis.runner.progress().active() {
                self.finish_library_analysis();
            }
        }
    }

    /// The library run, once queued, has nothing left to do: it's finished
    /// and isn't resumed at the next launch. Until it has been queued (the
    /// server not answered yet, or offline) there's nothing to finish.
    fn check_library_analysis(&mut self) {
        if self.analysis.library_run && self.analysis_library() && !self.analysis.runner.progress().active() {
            self.finish_library_analysis();
        }
    }

    fn finish_library_analysis(&mut self) {
        self.analysis.library_run = false;
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

    /// Sidebar and queue column widths, as saved (dragged by their inner edges).
    pub fn columns(&self) -> layout::Columns {
        self.prefs.get(|s| layout::Columns::restore(s.sidebar_width, s.queue_width))
    }

    /// Saves new column widths; defaults are stored as "not set".
    fn set_columns(&self, c: layout::Columns) {
        self.prefs.edit(|s| {
            s.sidebar_width = (c.left != layout::LEFT_DEFAULT).then_some(c.left);
            s.queue_width = (c.right != layout::RIGHT_DEFAULT).then_some(c.right);
        });
    }

    /// Full, Player or Mini.
    pub fn window_mode(&self) -> WindowMode {
        self.prefs.get(|s| s.window.mode)
    }

    /// The Player's lyrics drawer is open.
    pub fn lyrics_drawer(&self) -> bool {
        self.prefs.get(|s| s.window.lyrics_open)
    }

    /// Changes the window modes and geometry kept in the preferences.
    fn edit_windows<R>(&self, change: impl FnOnce(&mut Windows) -> R) -> R {
        let mut out = None;
        self.prefs.edit(|s| out = Some(change(&mut s.window)));
        out.expect("edit runs its closure")
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

    /// The accent color the user chose.
    pub fn accent(&self) -> theme::Accent {
        self.prefs.get(|s| s.accent)
    }

    /// Whether colors adapt to the album art.
    pub fn adapt_colors(&self) -> bool {
        self.prefs.get(|s| s.adapt_colors)
    }

    /// Which experimental visuals are on.
    pub fn experimental(&self) -> crate::visuals::Experimental {
        self.prefs.get(|s| s.experimental)
    }

    pub fn system_title_bar(&self) -> bool {
        self.prefs.get(|s| s.window.system_title_bar)
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
        let seen = self.ui.seen;
        let plan = self.edit_windows(|w| match to {
            Some(m) => w.switch(m, seen),
            None => w.toggle_mini(seen),
        });
        self.follow_plan(plan);
    }

    /// Resizes the window to `plan` (if any). The window state itself is in the
    /// preferences already (they save in the background).
    fn follow_plan(&mut self, plan: Option<crate::window::Plan>) {
        let Some(plan) = plan else { return };
        let ctx = self.ctx.clone();
        // Until the window reports its new size, don't mistake the old one for it.
        self.ui.seen = None;
        self.ui.switch_started = Some(std::time::Instant::now());
        tracing::debug!(target: "ond::window", mode = ?plan.mode, size = ?plan.size, "window mode switch");
        // A maximized window ignores sizes (Linux compositors maximize a large
        // window on their own), and answers an un-maximize with its old size,
        // which would land over ours. So un-maximize first; `track_window`
        // resizes once the window has let go. (macOS zoom is just a size.)
        if !cfg!(target_os = "macos") && !plan.maximized && ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
            self.ui.unmaximizing = Some((plan, std::time::Instant::now()));
            ctx.request_repaint();
            return;
        }
        self.resize_to(plan);
    }

    /// Follows the window each frame: finishes a mode switch (after an
    /// un-maximize, until the new size lands), keeps Mini square, and notes
    /// where the window is for next time.
    fn track_window(&mut self, ctx: &egui::Context) {
        let square = self.window_mode() == WindowMode::Mini;
        // Until the window exists the platform can't lock it; keep asking.
        if self.ui.square_wanted != Some(square) || (square && !self.ui.square_native) {
            self.ui.square_wanted = Some(square);
            self.ui.square_native = crate::window::lock_square(square) && square;
        }
        if let Some((plan, since)) = self.ui.unmaximizing {
            let still = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            if !still || since.elapsed() > UNMAXIMIZE_WAIT {
                self.ui.unmaximizing = None;
                self.resize_to(plan);
            } else {
                ctx.request_repaint();
            }
        }
        if self.ui.unmaximizing.is_none()
            && let Some(g) = Geometry::read(ctx)
        {
            match &mut self.ui.settling {
                Some(s) if s.waiting(g.size) => {
                    // A compositor can still answer with an older size after
                    // ours; ask again now and then.
                    if !g.maximized && s.asked.elapsed() > Settle::REASK {
                        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(s.want.into()));
                        s.asked = std::time::Instant::now();
                    }
                    ctx.request_repaint();
                }
                _ => {
                    if self.ui.settling.take().is_some() {
                        // Our own resize has landed: put presentation back to normal
                        // (see `chrome::end_programmatic_resize`).
                        crate::chrome::end_programmatic_resize();
                        if let Some(t) = self.ui.switch_started.take() {
                            tracing::debug!(target: "ond::window", elapsed_ms = t.elapsed().as_secs_f64() * 1000.0, size = ?g.size, "window mode switch settled");
                        }
                    }
                    // Mini is square; where the platform doesn't hold it so, snap back.
                    if square
                        && !self.ui.square_native
                        && let Some(side) = crate::window::square(self.ui.seen.map_or(g.size, |s| s.size), g.size)
                    {
                        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(side.into()));
                    }
                    self.ui.seen = Some(g);
                }
            }
        }
    }

    fn resize_to(&mut self, plan: crate::window::Plan) {
        plan.apply(&self.ctx);
        // A maximized window takes the screen's size, not the plan's.
        self.ui.settling = (!plan.maximized).then(|| Settle::new(plan.size));
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

    /// Stops playback, forgets the saved password and shows the login screen
    /// with the server and username filled in.
    fn sign_out(&mut self) {
        self.with_player(|p, cx| p.reset(cx));
        self.ui.palette = None;
        self.ui.overlay = None;
        self.favorites = Favorites::default();
        // Another account may sign in next (ids can collide), and a fetch still
        // in flight now delivers nothing, so its `Loading` would stay for good.
        self.lyrics.clear();
        self.song_stats.clear();
        self.bridge.cancel_sync();
        // Reconnect loops and connects in flight stop; anything they still
        // deliver is ignored. Downloads lose their server (and its credentials).
        self.bridge.end_session();
        // So does analysis: the old account's queue goes (a library-wide run
        // resumes for whoever signs in next).
        self.analysis.runner.set_server(None);
        self.analysis.runner.clear();
        self.analysis.library_run = false;
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
                ui.label(egui::RichText::new("Ond").font(egui::FontId::new(28.0, theme::semibold())).color(t.text));
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
                        ui.label(egui::RichText::new("Your Navidrome address, for example https://music.example.com").size(11.0).color(t.muted));
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

    /// The height our bar gives the traffic lights to centre in; `None` hides them.
    fn title_bar_h(&self) -> Option<f32> {
        match self.window_mode() {
            WindowMode::Full => Some(toolbar::TOOLBAR_H),
            WindowMode::Player => Some(now_playing_view::PLAYER_BAR_H),
            WindowMode::Mini => None,
        }
    }

    fn main_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.art.poll();
        self.art_tint(&ctx);
        self.want_waveform();
        if !self.experimental().spectrogram {
            // Turned off: the picture goes (and its texture with it).
            self.ui.spectrogram.clear();
        }
        self.shortcuts(&ctx);
        self.scroll_bars(&ctx);
        if self.ui.visualizer.is_some() && (!self.experimental().visualizer || self.visualizer_escaped(&ctx)) {
            self.close_visualizer();
        }
        if self.ui.visualizer.is_some() {
            // The whole window, whatever the mode.
            self.visualizer_ui(ui);
            crate::csd::overlay(&ctx, None);
        } else {
            match self.window_mode() {
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
            if self.window_mode() != WindowMode::Mini {
                self.ui.toasts.ui(&ctx, ctx.content_rect());
            }
            // Window buttons and resize edges where the system draws none (Linux).
            crate::csd::overlay(&ctx, match self.window_mode() {
                WindowMode::Full => Some(toolbar::TOOLBAR_H),
                WindowMode::Player => Some(now_playing_view::PLAYER_BAR_H),
                WindowMode::Mini => None,
            });
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
        let accent = self.accent();
        if self.ui.accent_applied != Some(accent) {
            theme::set_accent(ctx, accent);
            self.ui.accent_applied = Some(accent);
        }
        let base = self.ui.tint_cache.base(accent);
        let adapt = self.adapt_colors();
        // The covers' ids are borrowed (this runs every frame): a cover's tints
        // once its look is worked out, `None` meanwhile.
        let full = self.window_mode() == WindowMode::Full;
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
            Route::Album(id) if self.window_mode() == WindowMode::Full => Some(self.data.album(id).and_then(|a| a.cover_art.clone())),
            _ => None,
        }
    }

    /// Waveform seek bar (experimental): has the playing track's peaks read
    /// once a complete copy is on disk (downloaded, or fully streamed).
    fn want_waveform(&mut self) {
        self.waveforms.poll();
        if !self.experimental().waveform {
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
        let Some(cover) = cover.filter(|_| self.experimental().backdrop) else {
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
        let mut columns = self.columns();
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
        if self.diagnostics_shown() {
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
        let right_panel = (self.show_queue() && !self.ui.queue_narrow).then(|| {
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
            let limit = columns.left_limit(width, plan.queue_docked && self.show_queue());
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
            self.queue_sheet_ui(ui.ctx(), content.response.rect);
        }
    }

    /// The queue as a sheet sliding over the right of the content (narrow
    /// windows). Esc or a click outside closes it.
    fn queue_sheet_ui(&mut self, ctx: &egui::Context, content: egui::Rect) {
        let t = Tokens::of_ctx(ctx);
        let w = self.columns().right.min(content.width() - 24.0);
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
        if let Some(snap) = self.downloads.store.as_ref().map(|s| s.snapshot())
            && snap.version != self.downloads.snap.version
        {
            // A download finished: what's next may now play from disk, or play at all.
            let done_changed = snap.tracks.values().filter(|s| matches!(s, offline::TrackState::Done { .. })).count()
                != self.downloads.snap.tracks.values().filter(|s| matches!(s, offline::TrackState::Done { .. })).count();
            self.downloads.snap = snap;
            if done_changed && !self.session.online {
                self.sync_next();
            }
        }
        self.check_library_analysis();
        let queue = if self.ui.queue_narrow { self.ui.queue_sheet } else { self.show_queue() };
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
        if let Some(g) = self.ui.seen {
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

/// Fisher–Yates with a small xorshift; good enough for a play order.
/// Where queue items play from: a downloaded file first (instant, no
/// network), else the server while it's reachable (original or transcoded).
struct AppSources<'a> {
    /// `None` while the store opens.
    offline: Option<&'a OfflineStore>,
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
        if let Some(path) = self.offline.and_then(|o| o.local_path(&track.id)) {
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
        assert_eq!(app.window_mode(), WindowMode::Mini);
        // Hovered (overlay drawn) and not.
        for at in [egui::pos2(100.0, 100.0), egui::pos2(-10.0, -10.0)] {
            for _ in 0..3 {
                frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
            }
        }
        frame(&ctx, &mut app, vec![key(egui::Key::Num2, egui::Modifiers::COMMAND | egui::Modifiers::ALT)]);
        assert_eq!(app.window_mode(), WindowMode::Player);
        for _ in 0..2 {
            frame(&ctx, &mut app, vec![egui::Event::PointerMoved(egui::pos2(120.0, 60.0))]);
        }
        // Browsing goes back to the full window.
        app.actions.push(Action::Go(Route::Album("al-0".into())));
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.window_mode(), WindowMode::Full);
        // ⌘K from a small mode does too.
        app.edit_windows(|w| w.mode = WindowMode::Player);
        frame(&ctx, &mut app, vec![key(egui::Key::K, egui::Modifiers::COMMAND)]);
        assert_eq!(app.window_mode(), WindowMode::Full);
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
        assert_eq!(app.experimental().visualizer_pick, Pick::Ribbons, "and that becomes the choice");
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
        assert_eq!(app.experimental().visualizer_pick, Pick::Shuffle);
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
        app.apply(Action::TogglePlayer);
        assert_eq!(app.window_mode(), WindowMode::Player);
        app.apply(Action::Lyrics);
        frame(&ctx, &mut app, Vec::new());
        assert!(app.lyrics_drawer());
        // Mini, then back to full: the artwork now toggles to Mini.
        app.apply(Action::ToggleMini);
        app.apply(Action::SetMode(WindowMode::Full));
        app.apply(Action::TogglePlayer);
        assert_eq!(app.window_mode(), WindowMode::Mini);
        // Lyrics from Mini open the Player with its drawer.
        app.edit_windows(|w| w.lyrics_open = false);
        app.apply(Action::Lyrics);
        assert_eq!(app.window_mode(), WindowMode::Player);
        assert!(app.lyrics_drawer());
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
        app.apply(Action::KeepOffline(Keep::Tracks(vec![t]), true));
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
        app.apply(Action::KeepOffline(Keep::Tracks(vec![t]), true));
        app.apply(Action::PauseDownloads(true));
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
        assert!(app.analysis_library(), "the run is still due");
        assert!(!app.analysis.runner.progress().active());

        app.handle(Msg::Connected { session: app.bridge.session(), client: client(), settings: Box::default(), server: "test".into(), new_account: false });
        assert_eq!(app.analysis.runner.progress().total, 20, "the whole library is queued");
        app.check_library_analysis();
        assert!(app.analysis_library());

        // Signing out drops the old account's queue, but the run is still due for the next.
        app.sign_out();
        assert!(!app.analysis.runner.progress().active());
        app.check_library_analysis();
        assert!(app.analysis_library());
        app.handle(Msg::Connected { session: app.bridge.session(), client: client(), settings: Box::default(), server: "test".into(), new_account: false });
        assert_eq!(app.analysis.runner.progress().total, 20);

        // The queue runs dry: done, and not resumed at the next launch.
        app.analysis.runner.clear();
        app.check_library_analysis();
        assert!(!app.analysis_library());
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
        app.apply(Action::KeepOffline(Keep::Album("al-5".into()), true));
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
