//! The main window: sidebar · toolbar with the now-playing display · the
//! current view · the queue column. Views are in submodules; they don't change
//! state directly but push [`Action`]s, which are applied once the frame is
//! drawn. That keeps borrows simple and makes every view reusable by the
//! compact layout.

mod album_page;
mod browse;
mod palette;
mod queue_panel;
mod sidebar;
mod toolbar;
mod widgets;

use crate::art::Art;
use crate::bridge::{Bridge, Msg};
use crate::nav::{Nav, Route};
use crate::output::Outputs;
use crate::queue::{EndAction, Queue, Source};
use crate::search::SearchIndex;
use crate::settings::{Paths, Settings};
use crate::theme::{self, ThemePref, Tokens};
use audio::{Command, EngineHandle, PlayState, PlaybackMode, TrackSource};
use library::{Album, Library, Track};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;
use subsonic::Client;

pub use palette::Palette;

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

    /// Rebuilt whenever the album list changes.
    search: Option<SearchIndex>,
    palette: Option<Palette>,
    theme: ThemePref,
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
        bridge.forward_engine_events(engine.events.clone());
        bridge.load_cached_albums();
        let art = Art::new(rt.handle().clone(), ctx.clone(), paths.art_cache.clone());

        let login = match &saved {
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
            focus: Focus::default(),
            track_sel: None,
            credits_open: false,
            output,
            bit_perfect: false,
            volume: 1.0,
            seek_preview: None,
            pending_seek: None,
            search: None,
            palette: None,
            theme: theme_pref,
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
                self.search = Some(SearchIndex::new(&self.data.albums));
                if let Some(p) = self.palette.as_mut()
                    && let Some(idx) = self.search.as_mut()
                {
                    p.refresh(idx);
                }
            }
            Msg::AlbumUpdated(a) => self.data.update_album(a),
            Msg::SyncProgress(n) => self.sync_label = format!("syncing… {n}"),
            Msg::SyncDone(n) => self.sync_label = format!("{n} albums"),
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
            Msg::Error(e) => self.error = Some(e),
        }
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

    fn source(client: &Client, track: &Track) -> TrackSource {
        TrackSource { id: track.id.clone(), url: client.stream_url(&track.id).to_string(), suffix: track.suffix.clone() }
    }

    /// Starts the queue's current item from the top (or stops when there is none).
    fn start_current(&mut self) {
        self.pending_seek = None;
        self.sent_next = None;
        match (&self.client, &self.queue.current) {
            (Some(client), Some(item)) => {
                self.engine.send(Command::Play(Self::source(client, &item.track)));
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
            self.engine.send(Command::SetNext(next.map(|t| Self::source(client, &t))));
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
                let ids: Vec<String> = self.data.albums.iter().filter(|a| a.artist.as_deref() == Some(name.as_str())).map(|a| a.id.clone()).collect();
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
            Action::ToggleQueue => {
                self.show_queue = !self.show_queue;
                let show = self.show_queue;
                Settings::update(&self.paths, |s| s.show_queue = show);
            }
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

    fn login_ui(&mut self, ui: &mut egui::Ui) {
        let Some(form) = self.login.as_mut() else { return };
        let mut submit = false;
        egui::CentralPanel::default_margins().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(120.0);
                ui.heading("Connect to Navidrome");
                ui.add_space(16.0);
                ui.add_enabled_ui(!form.busy, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut form.server).hint_text("https://music.example.com").desired_width(320.0));
                    ui.add(egui::TextEdit::singleline(&mut form.username).hint_text("username").desired_width(320.0));
                    let pw = ui.add(egui::TextEdit::singleline(&mut form.password).password(true).hint_text("password").desired_width(320.0));
                    let enter = pw.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    submit = ui.button("Connect").clicked() || enter;
                });
                if form.busy {
                    ui.spinner();
                }
                if let Some(e) = &form.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
            });
        });
        if submit && !form.busy {
            form.busy = true;
            form.error = None;
            let settings = Settings {
                server: form.server.trim().to_owned(),
                username: form.username.trim().to_owned(),
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
        });
        self.actions.extend(out);
    }

    fn main_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.art.poll();
        self.shortcuts(&ctx);
        let t = Tokens::of(ui);

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
        egui::Panel::left("sidebar")
            .exact_size(216.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(t.sidebar).inner_margin(egui::Margin { left: 10, right: 10, top: 14, bottom: 10 }))
            .show(ui, |ui| self.sidebar(ui));
        if self.show_queue {
            egui::Panel::right("queue")
                .exact_size(320.0)
                .resizable(false)
                .frame(egui::Frame::new().fill(t.panel).inner_margin(egui::Margin { left: 8, right: 8, top: 12, bottom: 8 }))
                .show(ui, |ui| self.queue_panel(ui));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.bg).inner_margin(egui::Margin { left: 28, right: 28, top: 12, bottom: 0 }))
            .show(ui, |ui| self.main_view(ui));

        self.palette_ui(&ctx);
        for a in std::mem::take(&mut self.actions) {
            self.apply(a);
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
        // Only the progress readout needs a clock; idle costs nothing. Buffering
        // animates the seek bar and spinner, so it ticks faster.
        match self.engine.status.state() {
            PlayState::Playing => ctx.request_repaint_after(Duration::from_millis(250)),
            PlayState::Buffering | PlayState::Loading => ctx.request_repaint_after(Duration::from_millis(33)),
            _ if self.pending_seek.is_some() => ctx.request_repaint_after(Duration::from_millis(33)),
            _ => {}
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.login.is_some() {
            self.login_ui(ui);
        } else {
            self.main_ui(ui);
        }
    }
}

impl Drop for ListenerApp {
    fn drop(&mut self) {
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
            })
            .collect();
        app.data.set_albums(albums);
        app.search = Some(SearchIndex::new(&app.data.albums));
        let tracks: Vec<Track> = (1..=20).map(|n| track(&format!("t-{n}"), "al-0", n)).collect();
        app.data.tracks.insert("al-0".into(), tracks.clone());
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
        assert_eq!(app.nav.trail(), [&Route::Offline, &Route::Album("al-0".into())]);
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
