//! The main window: sidebar · toolbar with the now-playing display · the
//! current view · the queue column. Views are in submodules; they don't change
//! state directly but push [`Action`]s, which are applied once the frame is
//! drawn. That keeps borrows simple and makes every view reusable by the
//! compact layout.

mod album_page;
mod browse;
mod compact;
mod palette;
mod queue_panel;
mod sidebar;
mod songs;
mod toolbar;
mod widgets;

#[cfg(test)]
mod snapshots;

use crate::art::Art;
use crate::bridge::{Bridge, Msg};
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
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;
use subsonic::Client;

pub use palette::Palette;
pub(crate) use widgets::{elide as elide_text, segmented_wide};

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
    /// Back to the login screen; the saved password is forgotten.
    SignOut,
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
    /// The track id last sent as `SetNext`, so the engine isn't told twice.
    sent_next: Option<String>,
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
}

impl ListenerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        Self::build(&cc.egui_ctx, Paths::resolve()?, audio::default_backend())
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
        let bridge = Bridge::new(rt.handle().clone(), tx, ctx.clone(), library);

        let saved = Settings::load(&paths);
        let theme_pref = saved.as_ref().map(|s| s.theme).unwrap_or_default();
        theme::install(ctx, theme_pref);
        let mut cache_cfg = audio::CacheConfig::new(&paths.stream_cache);
        if let Some(mb) = saved.as_ref().and_then(|s| s.stream_cache_mb) {
            cache_cfg.max_bytes = mb * 1024 * 1024;
        }
        let cache = audio::StreamCache::new(cache_cfg, rt.handle().clone())?;
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
        })
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Connected { client, settings, server } => {
                self.server_label = format!("{} · {}", settings.username, server);
                self.login = None;
                self.sync_label = "syncing…".into();
                self.bridge.sync_albums(client.clone());
                self.bridge.fetch_home(client.clone());
                self.art.set_client(client.clone());
                self.now_playing.set_client(client.clone());
                self.output.connected(&settings.server);
                self.client = Some(client);
            }
            Msg::ConnectFailed(e) => {
                if let Some(l) = self.login.as_mut() {
                    l.busy = false;
                    l.error = Some(e);
                }
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
                    crate::sync::Outcome::Done { .. } => format!("{albums} albums · {tracks} tracks"),
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
                self.data.tracks.insert(album_id.clone(), tracks);
                self.run_pending(&format!("album:{album_id}"));
            }
            Msg::Newest(ids) => {
                self.data.newest = ids;
                self.data.version += 1;
            }
            Msg::Playlists(p) => self.data.playlists = p,
            Msg::PlaylistTracks { id, tracks } => {
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
            Msg::Error(e) => self.error = Some(e),
        }
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
            audio::Event::TrackStarted { track_id, .. } => {
                let is_current = self.queue.current.as_ref().is_some_and(|c| c.track.id == track_id);
                let is_next = self.queue.peek_next().is_some_and(|n| n.track.id == track_id);
                if !is_current && is_next {
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
            }
            audio::Event::SignalPath(_)
            | audio::Event::Devices { .. }
            | audio::Event::ModeChanged { .. }
            | audio::Event::DeviceLost { .. }
            | audio::Event::DeviceReturned { .. } => {}
        }
    }

    /// What the engine needs to play `track`: the stream (original or
    /// transcoded), its ReplayGain, and whether it plays as part of its album
    /// (from its queue neighbours `prev` and `next`).
    fn source(
        client: &Client,
        transcode: Option<audio::Transcode>,
        prev: Option<&Track>,
        track: &Track,
        next: Option<&Track>,
    ) -> TrackSource {
        let (url, suffix) = match &transcode {
            None => (client.stream_url(&track.id), track.suffix.clone()),
            Some(t) => (client.transcoded_stream_url(&track.id, &t.format, t.kbps), Some(t.format.clone())),
        };
        TrackSource {
            id: track.id.clone(),
            url: url.to_string(),
            suffix,
            replay_gain: playback::replay_gain(track),
            album_context: playback::in_album(prev, track, next),
            transcode,
        }
    }

    /// Starts the queue's current item from the top (or stops when there is none).
    fn start_current(&mut self) {
        self.pending_seek = None;
        self.sent_next = None;
        match (&self.client, &self.queue.current) {
            (Some(client), Some(item)) => {
                self.announce(&item.track);
                let prev = self.queue.history.last().map(|i| &i.track);
                let next = self.queue.peek_next().map(|i| &i.track);
                let src = Self::source(client, self.output.transcode(), prev, &item.track, next);
                self.engine.send(Command::Play(src));
                self.sync_next();
            }
            (_, None) => self.engine.send(Command::Stop),
            _ => {}
        }
    }

    /// Tells the engine what follows the current track (prefetch + gapless), if
    /// that changed.
    fn sync_next(&mut self) {
        let Some(client) = &self.client else { return };
        if self.queue.current.is_none() {
            return;
        }
        let next = self.queue.peek_next().map(|i| i.track.clone());
        let id = next.as_ref().map(|t| t.id.clone());
        if id != self.sent_next {
            if let Some(t) = &next {
                self.announce(t);
            }
            let current = self.queue.current.as_ref().map(|i| &i.track);
            let after = self.queue.upcoming.get(1).map(|i| &i.track);
            let transcode = self.output.transcode();
            let src = next.map(|t| Self::source(client, transcode, current, &t, after));
            self.engine.send(Command::SetNext(src));
            self.sent_next = id;
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
            Action::SetScrobble(on) => {
                self.scrobble = on;
                self.now_playing.set_scrobbling(on);
                Settings::update(&self.paths, |s| s.scrobble = on);
            }
        }
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
            Route::Playlist { id, .. } => self.bridge.open_playlist(self.client.clone(), id),
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

    /// App-wide keys. Views handle their own (track list, queue, palette).
    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.palette.is_some() {
            return;
        }
        use egui::{Key, KeyboardShortcut, Modifiers};
        let typing = ctx.memory(|m| m.focused().is_some());
        let mut out = Vec::new();
        ctx.input_mut(|i| {
            if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::K)) {
                out.push(Action::OpenPalette);
            }
            if !typing && i.consume_key(Modifiers::NONE, Key::Slash) {
                // Keep the `/` itself out of the search field.
                i.events.retain(|e| !matches!(e, egui::Event::Text(t) if t == "/"));
                out.push(Action::OpenPalette);
            }
            if !typing && i.consume_key(Modifiers::NONE, Key::Space) {
                out.push(Action::PlayPause);
            }
            if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::OpenBracket)) {
                out.push(Action::Back);
            }
            if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::CloseBracket)) {
                out.push(Action::Forward);
            }
            if !typing && i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::ArrowRight)) {
                out.push(Action::Next);
            }
            if !typing && i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::ArrowLeft)) {
                out.push(Action::Previous);
            }
            if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::ALT, Key::U)) {
                out.push(Action::ToggleQueue);
            }
            // Window modes. ⇧⌘3 would be nicer but it's the system screenshot key.
            for (key, mode) in [(Key::Num1, WindowMode::Full), (Key::Num2, WindowMode::Compact), (Key::Num3, WindowMode::Mini)] {
                if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::ALT, key)) {
                    out.push(Action::SetMode(mode));
                }
            }
            if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::M)) {
                out.push(Action::ToggleMini);
            }
        });
        self.actions.extend(out);
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
            }
            WindowMode::Compact => self.compact_ui(ui),
            WindowMode::Mini => self.mini_ui(ui),
        }
        for a in std::mem::take(&mut self.actions) {
            self.apply(a);
        }
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

    #[test]
    fn gapless_start_advances_the_queue() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = app(&ctx, dir.path());
        let next = app.queue.peek_next().unwrap().track.id.clone();
        let format = audio::StreamFormat { sample_rate: 44_100, channels: 2, bits_per_sample: Some(16) };
        app.handle_engine(audio::Event::TrackStarted { track_id: next.clone(), format, gapless: true });
        assert_eq!(app.queue.current.as_ref().unwrap().track.id, next);
        // A repeat of the same event (or the current track) changes nothing.
        let upcoming = app.queue.upcoming.len();
        app.handle_engine(audio::Event::TrackStarted { track_id: next, format, gapless: true });
        assert_eq!(app.queue.upcoming.len(), upcoming);
    }
}
