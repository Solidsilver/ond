use crate::bridge::{Bridge, Msg};
use crate::icons::{self, Icon};
use crate::search::{Hit, Results, SearchIndex};
use crate::settings::{Paths, Settings};
use crate::output::Outputs;
use audio::{Command, EngineHandle, PlayState, PlaybackMode, TrackSource};
use library::{Album, Library, Track};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;
use subsonic::Client;

#[derive(Default)]
struct LoginForm {
    server: String,
    username: String,
    password: String,
    busy: bool,
    error: Option<String>,
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

    albums: Vec<Album>,
    selected_album: Option<String>,
    tracks: Vec<Track>,

    /// Play order and the index of the current track in it.
    queue: Vec<Track>,
    queue_pos: Option<usize>,
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
    /// Album list narrowed to one album artist (from search).
    artist_filter: Option<String>,
    /// Indices into `albums` that the list shows.
    album_view: Vec<usize>,
    /// Scroll the album list to this album on the next frame.
    scroll_to_album: Option<String>,
    /// Play this album from the top once its tracks arrive (⌘⏎ in search).
    play_when_loaded: Option<String>,
}

#[derive(Default)]
struct Palette {
    query: String,
    results: Results,
    selected: usize,
}

enum PaletteAction {
    Open(Hit),
    Play(Hit),
}

impl ListenerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
        let paths = Arc::new(Paths::resolve()?);
        // The db is only a mirror of the server, so an unreadable one is set aside
        // and rebuilt by the sync that runs on connect.
        let (library, moved_aside) = Library::open_or_recover(&paths.library_db)?;
        let notice = moved_aside.map(|aside| {
            tracing::warn!("library database was unreadable; moved to {} and starting fresh", aside.display());
            format!("Library database was unreadable and is being rebuilt (old copy: {})", aside.display())
        });
        let library = Arc::new(library);
        let (tx, inbox) = channel();
        let bridge = Bridge::new(rt.handle().clone(), tx, cc.egui_ctx.clone(), library);

        let saved = Settings::load(&paths);
        let mut cache_cfg = audio::CacheConfig::new(&paths.stream_cache);
        if let Some(mb) = saved.as_ref().and_then(|s| s.stream_cache_mb) {
            cache_cfg.max_bytes = mb * 1024 * 1024;
        }
        let cache = audio::StreamCache::new(cache_cfg, rt.handle().clone())?;
        let engine = audio::spawn(audio::EngineConfig::default(), audio::default_backend(), Some(cache));
        let output = Outputs::restore(&engine, saved.as_ref());
        bridge.forward_engine_events(engine.events.clone());
        bridge.load_cached_albums();

        let login = match &saved {
            Some(s) => {
                bridge.connect(s.clone(), None, paths.clone());
                LoginForm { server: s.server.clone(), username: s.username.clone(), busy: true, ..Default::default() }
            }
            None => LoginForm { server: "http://localhost:4533".into(), ..Default::default() },
        };

        Ok(Self {
            _rt: rt,
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
            albums: Vec::new(),
            selected_album: None,
            tracks: Vec::new(),
            queue: Vec::new(),
            queue_pos: None,
            output,
            bit_perfect: false,
            volume: 1.0,
            seek_preview: None,
            pending_seek: None,
            search: None,
            palette: None,
            artist_filter: None,
            album_view: Vec::new(),
            scroll_to_album: None,
            play_when_loaded: None,
        })
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Connected { client, settings, server } => {
                self.server_label = format!("{} · {}", settings.username, server);
                self.login = None;
                self.sync_label = "syncing…".into();
                self.bridge.sync_albums(client.clone());
                self.client = Some(client);
            }
            Msg::ConnectFailed(e) => {
                if let Some(l) = self.login.as_mut() {
                    l.busy = false;
                    l.error = Some(e);
                }
            }
            Msg::Albums(a) => {
                self.albums = a;
                self.search = Some(SearchIndex::new(&self.albums));
                self.refresh_album_view();
                if let Some(p) = self.palette.as_mut()
                    && let Some(idx) = self.search.as_mut()
                {
                    p.results = idx.search(&p.query);
                    p.selected = p.selected.min(p.results.len().saturating_sub(1));
                }
            }
            Msg::SyncProgress(n) => self.sync_label = format!("syncing… {n}"),
            Msg::SyncDone(n) => self.sync_label = format!("{n} albums"),
            Msg::Tracks { album_id, tracks } => {
                if self.play_when_loaded.as_deref() == Some(album_id.as_str()) && !tracks.is_empty() {
                    self.play_when_loaded = None;
                    self.queue = tracks.clone();
                    self.play_at(0);
                }
                if self.selected_album.as_deref() == Some(album_id.as_str()) {
                    self.tracks = tracks;
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
                let follows = self.queue_pos.map(|p| p + 1).filter(|n| self.queue.get(*n).is_some_and(|t| t.id == track_id));
                if let Some(pos) = follows {
                    // The engine moved on gaplessly; line up the track after this one.
                    self.queue_pos = Some(pos);
                    self.pending_seek = None;
                    self.send_next();
                }
            }
            audio::Event::Seeked { .. } => self.pending_seek = None,
            // The engine ran out of queued tracks. If the queue has more (the next one
            // was named too late to play gaplessly), carry on from there.
            audio::Event::Ended { .. } => self.advance(1),
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

    fn current(&self) -> Option<&Track> {
        self.queue_pos.and_then(|i| self.queue.get(i))
    }

    fn source(client: &Client, track: &Track) -> TrackSource {
        TrackSource { id: track.id.clone(), url: client.stream_url(&track.id).to_string(), suffix: track.suffix.clone() }
    }

    fn play_at(&mut self, pos: usize) {
        let (Some(client), Some(track)) = (&self.client, self.queue.get(pos)) else { return };
        self.queue_pos = Some(pos);
        self.pending_seek = None;
        self.engine.send(Command::Play(Self::source(client, track)));
        self.send_next();
    }

    /// Tells the engine what follows the current track (prefetch + gapless).
    fn send_next(&self) {
        let Some(client) = &self.client else { return };
        let next = self.queue_pos.and_then(|p| self.queue.get(p + 1)).map(|t| Self::source(client, t));
        self.engine.send(Command::SetNext(next));
    }

    fn advance(&mut self, by: isize) {
        match self.queue_pos.map(|p| p as isize + by) {
            Some(next) if next >= 0 && (next as usize) < self.queue.len() => self.play_at(next as usize),
            _ => {
                self.queue_pos = None;
                self.engine.send(Command::Stop);
            }
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

    fn transport_ui(&mut self, ui: &mut egui::Ui) {
        let status = self.engine.status.clone();
        egui::Panel::bottom("transport").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let state = status.state();
                if icons::button(ui, Icon::Previous, "Previous").clicked() {
                    self.advance(-1);
                }
                let waiting = matches!(state, PlayState::Buffering | PlayState::Loading) || self.pending_seek.is_some();
                let (icon, label) = match state {
                    _ if waiting => (Icon::Spinner, "Buffering (click to pause)"),
                    PlayState::Playing => (Icon::Pause, "Pause"),
                    _ => (Icon::Play, "Play"),
                };
                if icons::button(ui, icon, label).clicked() {
                    match state {
                        PlayState::Playing | PlayState::Buffering | PlayState::Loading => self.engine.send(Command::Pause),
                        PlayState::Paused => self.engine.send(Command::Resume),
                        PlayState::Stopped if !self.queue.is_empty() => self.play_at(self.queue_pos.unwrap_or(0)),
                        _ => {}
                    }
                }
                if icons::button(ui, Icon::Next, "Next").clicked() {
                    self.advance(1);
                }
                let dur = status.duration_secs().or_else(|| self.current().and_then(|t| t.duration_secs).map(f64::from));
                let pos = self.pending_seek.unwrap_or_else(|| status.position_secs());
                let shown = self.seek_preview.unwrap_or(pos);
                ui.monospace(mmss(shown));
                let seekable = matches!(state, PlayState::Playing | PlayState::Paused | PlayState::Buffering);
                let buffering = state == PlayState::Buffering || self.pending_seek.is_some();
                let bar = SeekBar { pos, dur: dur.filter(|_| seekable), fetched: status.fetched_fraction(), buffering };
                if let Some(to) = bar.show(ui, &mut self.seek_preview) {
                    self.pending_seek = Some(to);
                    self.engine.send(Command::Seek(Duration::from_secs_f64(to)));
                }
                ui.monospace(dur.map(mmss).unwrap_or_else(|| "--:--".into()));
                if let Some(t) = self.current() {
                    ui.strong(&t.title);
                    if let Some(a) = &t.artist {
                        ui.weak(a);
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let slider = egui::Slider::new(&mut self.volume, 0.0..=1.0).show_value(false);
                    if ui
                        .add_enabled(!self.bit_perfect, slider)
                        .on_disabled_hover_text("In bit-perfect mode volume is controlled by your DAC or system")
                        .changed()
                    {
                        self.engine.send(Command::SetVolume(self.volume));
                    }
                    if ui.checkbox(&mut self.bit_perfect, "Bit-perfect").changed() {
                        let mode = if self.bit_perfect { PlaybackMode::BitPerfect } else { PlaybackMode::Normal };
                        self.engine.send(Command::SetMode(mode));
                        self.output.remember_mode(mode, &self.engine, &self.paths);
                    }
                    self.output.picker_ui(ui, &self.engine, &self.paths);
                });
            });
            ui.horizontal(|ui| self.signal_path_ui(ui, &status));
            ui.add_space(4.0);
        });
    }

    /// `FLAC · 24/96 → 16/48 · resampled, dithered · device limit`.
    fn signal_path_ui(&self, ui: &mut egui::Ui, status: &audio::Status) {
        let codec = self.current().and_then(|t| t.suffix.as_deref()).map(str::to_uppercase);
        self.output.signal_ui(ui, status, codec.as_deref());
    }

    fn library_ui(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong("Listener");
                ui.weak(&self.server_label);
                ui.weak(&self.sync_label);
                if let Some(e) = &self.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                    if icons::button(ui, Icon::Close, "Dismiss").clicked() {
                        self.error = None;
                    }
                } else if let Some(n) = &self.notice {
                    ui.weak(n);
                    if icons::button(ui, Icon::Close, "Dismiss").clicked() {
                        self.notice = None;
                    }
                }
            });
        });
        self.transport_ui(ui);

        let mut open: Option<String> = None;
        let mut clear_filter = false;
        egui::Panel::left("albums").default_size(360.0).resizable(true).show(ui, |ui| {
            if let Some(artist) = &self.artist_filter {
                ui.horizontal(|ui| {
                    ui.strong(artist);
                    if icons::button(ui, Icon::Close, "Show all albums").clicked() {
                        clear_filter = true;
                    }
                });
                ui.separator();
            }
            let row_h = ui.text_style_height(&egui::TextStyle::Body);
            let mut area = egui::ScrollArea::vertical().auto_shrink(false);
            if let Some(id) = self.scroll_to_album.take()
                && let Some(row) = self.album_view.iter().position(|&i| self.albums[i].id == id)
            {
                // Rows are buttons, so a row is the text plus button padding and spacing.
                let spacing = ui.spacing();
                let stride = row_h + 2.0 * spacing.button_padding.y + spacing.item_spacing.y;
                area = area.vertical_scroll_offset((row as f32 * stride - 4.0 * stride).max(0.0));
            }
            area.show_rows(ui, row_h, self.album_view.len(), |ui, rows| {
                for a in self.album_view[rows].iter().map(|&i| &self.albums[i]) {
                    let selected = self.selected_album.as_deref() == Some(a.id.as_str());
                    let text = format!("{} — {}", a.artist.as_deref().unwrap_or("?"), a.name);
                    if ui.add(egui::Button::selectable(selected, text).truncate()).clicked() {
                        open = Some(a.id.clone());
                    }
                }
            });
        });
        if clear_filter {
            self.artist_filter = None;
            self.refresh_album_view();
            self.scroll_to_album = self.selected_album.clone();
        }
        if let Some(id) = open {
            self.open_album(id);
        }

        let mut play: Option<usize> = None;
        egui::CentralPanel::default_margins().show(ui, |ui| {
            let Some(album) = self.selected_album.as_ref().and_then(|id| self.albums.iter().find(|a| &a.id == id)) else {
                ui.weak("Pick an album.");
                return;
            };
            ui.heading(&album.name);
            ui.weak(format!("{} · {}", album.artist.as_deref().unwrap_or("?"), album.year.map(|y| y.to_string()).unwrap_or_default()));
            ui.separator();
            let current_id = self.current().map(|t| t.id.clone());
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                for (i, t) in self.tracks.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.monospace(format!("{:>2}", t.track.unwrap_or(0)));
                        let playing = current_id.as_deref() == Some(t.id.as_str());
                        if ui.add(egui::Button::selectable(playing, &t.title)).double_clicked() {
                            play = Some(i);
                        }
                        ui.weak(t.quality_label());
                        ui.weak(t.duration_secs.map(|d| mmss(d as f64)).unwrap_or_default());
                    });
                }
            });
        });
        if let Some(i) = play {
            self.queue = self.tracks.clone();
            self.play_at(i);
        }
    }
}

impl ListenerApp {
    fn refresh_album_view(&mut self) {
        let filter = self.artist_filter.as_deref();
        self.album_view = (0..self.albums.len()).filter(|&i| filter.is_none_or(|f| self.albums[i].artist.as_deref() == Some(f))).collect();
    }

    fn open_album(&mut self, id: String) {
        if self.selected_album.as_deref() != Some(id.as_str()) {
            self.tracks.clear();
        }
        self.selected_album = Some(id.clone());
        self.bridge.open_album(self.client.clone(), id);
    }

    /// ⌘K anywhere, or `/` when no text field has focus.
    fn palette_shortcuts(&mut self, ctx: &egui::Context) {
        if self.login.is_some() {
            return;
        }
        let typing = ctx.memory(|m| m.focused().is_some());
        let open = ctx.input_mut(|i| {
            let cmd_k = i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::K));
            let slash = !typing && i.consume_key(egui::Modifiers::NONE, egui::Key::Slash);
            if slash {
                // Keep the `/` itself out of the search field.
                i.events.retain(|e| !matches!(e, egui::Event::Text(t) if t == "/"));
            }
            cmd_k || slash
        });
        if open && self.palette.is_none() {
            self.palette = Some(Palette::default());
        }
    }

    fn palette_ui(&mut self, ctx: &egui::Context) {
        let Some(palette) = self.palette.as_mut() else { return };
        let Some(index) = self.search.as_mut() else {
            self.palette = None;
            return;
        };
        let (cmd_enter, enter, up, down) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            )
        });
        let n = palette.results.len();
        if n > 0 {
            if down {
                palette.selected = (palette.selected + 1) % n;
            }
            if up {
                palette.selected = (palette.selected + n - 1) % n;
            }
        }
        let mut action = None;
        let modal = egui::Modal::new(egui::Id::unique("palette")).show(ctx, |ui| {
            ui.set_width(560.0);
            let edit = ui.add(
                egui::TextEdit::singleline(&mut palette.query)
                    .hint_text("Search artists and albums")
                    .desired_width(f32::INFINITY),
            );
            edit.request_focus();
            if edit.changed() {
                palette.results = index.search(&palette.query);
                palette.selected = 0;
            }
            let r = &palette.results;
            if r.len() == 0 {
                if !palette.query.trim().is_empty() {
                    ui.add_space(6.0);
                    ui.weak("No matches");
                }
                return;
            }
            let mut row = 0;
            let mut group = |ui: &mut egui::Ui, title: &str, hits: &mut dyn Iterator<Item = (Hit, String, String)>| {
                let mut first = true;
                for (hit, main, sub) in hits {
                    if first {
                        ui.add_space(6.0);
                        ui.weak(title);
                        first = false;
                    }
                    let selected = row == palette.selected;
                    let resp = ui.horizontal(|ui| {
                        let b = ui.add(egui::Button::selectable(selected, main).truncate());
                        ui.weak(sub);
                        b
                    });
                    if selected {
                        resp.inner.scroll_to_me(None);
                    }
                    if resp.inner.clicked() {
                        action = Some(if ui.input(|i| i.modifiers.command) { PaletteAction::Play(hit) } else { PaletteAction::Open(hit) });
                    }
                    row += 1;
                }
            };
            group(
                ui,
                "Artists",
                &mut r.artists.iter().map(|&i| {
                    let a = &index.artists[i];
                    let n = a.albums;
                    (Hit::Artist(i), a.name.clone(), format!("{n} album{}", if n == 1 { "" } else { "s" }))
                }),
            );
            group(
                ui,
                "Albums",
                &mut r.albums.iter().map(|&i| {
                    let a = &index.albums[i];
                    let sub = match a.year {
                        Some(y) => format!("{} · {y}", a.artist),
                        None => a.artist.clone(),
                    };
                    (Hit::Album(i), a.title.clone(), sub)
                }),
            );
            ui.add_space(8.0);
            ui.weak("⏎ open · ⌘⏎ play · esc close");
        });
        let hit = palette.results.hit(palette.selected);
        if action.is_none() {
            action = match (cmd_enter, enter, hit) {
                (true, _, Some(h)) => Some(PaletteAction::Play(h)),
                (_, true, Some(h)) => Some(PaletteAction::Open(h)),
                _ => None,
            };
        }
        let close = modal.should_close();
        if let Some(a) = action {
            self.run_palette_action(a);
        } else if close {
            self.palette = None;
        }
    }

    fn run_palette_action(&mut self, action: PaletteAction) {
        let Some(index) = self.search.as_ref() else { return };
        let (hit, play) = match action {
            PaletteAction::Open(h) => (h, false),
            PaletteAction::Play(h) => (h, true),
        };
        self.palette = None;
        match hit {
            // Artists only narrow the list for now; playing a whole artist waits for the queue work.
            Hit::Artist(i) => {
                self.artist_filter = Some(index.artists[i].name.clone());
                self.refresh_album_view();
                if let Some(&first) = self.album_view.first() {
                    let id = self.albums[first].id.clone();
                    self.scroll_to_album = Some(id.clone());
                    self.open_album(id);
                }
            }
            Hit::Album(i) => {
                let id = index.albums[i].id.clone();
                if self.artist_filter.is_some() && !self.album_view.iter().any(|&j| self.albums[j].id == id) {
                    self.artist_filter = None;
                    self.refresh_album_view();
                }
                self.scroll_to_album = Some(id.clone());
                if play {
                    self.play_when_loaded = Some(id.clone());
                }
                self.open_album(id);
            }
        }
    }
}

impl eframe::App for ListenerApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.inbox.try_recv() {
            self.handle(msg);
        }
        // Only the progress readout needs a clock; idle costs nothing. Buffering
        // animates the seek bar, so it ticks faster.
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
            let ctx = ui.ctx().clone();
            self.palette_shortcuts(&ctx);
            self.library_ui(ui);
            self.palette_ui(&ctx);
        }
    }
}

impl Drop for ListenerApp {
    fn drop(&mut self) {
        self.engine.send(Command::Shutdown);
    }
}

/// A plain progress bar you can click or drag. The downloaded share of the file is
/// shaded behind the played part; while buffering, a soft pulse sits just ahead of
/// the position.
struct SeekBar {
    pos: f64,
    dur: Option<f64>,
    fetched: Option<f32>,
    buffering: bool,
}

impl SeekBar {
    /// Returns a target (seconds) when the user releases or clicks.
    fn show(self, ui: &mut egui::Ui, preview: &mut Option<f64>) -> Option<f64> {
        let width = (ui.available_width() * 0.4).clamp(120.0, 480.0);
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 16.0), egui::Sense::click_and_drag());
        let visuals = ui.visuals();
        let track = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), 4.0));
        let painter = ui.painter();
        painter.rect_filled(track, 2.0, visuals.widgets.inactive.bg_fill);

        let Some(dur) = self.dur.filter(|d| *d > 0.0) else {
            *preview = None;
            return None;
        };
        let frac_at = |x: f32| f64::from(((x - rect.left()) / rect.width()).clamp(0.0, 1.0));
        let mut target = None;
        if (resp.dragged() || resp.drag_started())
            && let Some(p) = resp.interact_pointer_pos()
        {
            *preview = Some(frac_at(p.x) * dur);
        }
        if resp.drag_stopped() || resp.clicked() {
            target = resp.interact_pointer_pos().map(|p| frac_at(p.x) * dur).or(*preview);
            *preview = None;
        }

        let x_at = |f: f32| rect.left() + rect.width() * f.clamp(0.0, 1.0);
        if let Some(f) = self.fetched {
            let r = egui::Rect::from_min_max(track.left_top(), egui::pos2(x_at(f), track.bottom()));
            painter.rect_filled(r, 2.0, visuals.widgets.hovered.bg_fill);
        }
        let shown = (preview.unwrap_or(self.pos) / dur) as f32;
        let x = x_at(shown);
        painter.rect_filled(egui::Rect::from_min_max(track.left_top(), egui::pos2(x, track.bottom())), 2.0, visuals.selection.bg_fill);
        if self.buffering {
            let t = ui.input(|i| i.time) as f32;
            let alpha = 0.3 + 0.3 * (t * std::f32::consts::TAU / 1.2).sin();
            let pulse = egui::Rect::from_min_max(egui::pos2(x, track.top()), egui::pos2((x + 32.0).min(rect.right()), track.bottom()));
            painter.rect_filled(pulse, 2.0, visuals.selection.bg_fill.gamma_multiply(alpha));
        }
        if resp.hovered() || preview.is_some() {
            painter.circle_filled(egui::pos2(x, rect.center().y), 6.0, visuals.widgets.active.fg_stroke.color);
        }
        target
    }
}

fn mmss(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}
