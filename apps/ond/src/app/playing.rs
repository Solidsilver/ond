//! Playback as the app drives it: the player with the engine and where each
//! track plays from (a download, else the server), what the player reports
//! back, the engine's events, remote commands and the volume.

use super::pins::Downloads;
use super::{Action, Data, OndApp};
use crate::now_playing::{self, Remote};
use crate::output::Outputs;
use crate::playback;
use crate::player::{self, Note, Player, QueueEdit};
use crate::queue::Item;
use crate::source::{TrackKey, stream_name};
use audio::{Command, PlaybackMode, TrackSource};
use library::Track;
use streamcache::StreamCache;

/// The play/pause button's glyph and name: a spinner while the track is
/// still on its way (buffering, loading, a seek or a hold pending), pause
/// while it plays, play otherwise.
pub(super) fn play_pause(state: audio::PlayState, player: &Player) -> (crate::icons::Icon, &'static str) {
    use crate::icons::Icon;
    use audio::PlayState;
    let waiting = matches!(state, PlayState::Buffering | PlayState::Loading) || player.pending_seek.is_some() || player.held;
    match state {
        _ if waiting => (Icon::Spinner, "Buffering (click to pause)"),
        PlayState::Playing => (Icon::Pause, "Pause"),
        _ => (Icon::Play, "Play"),
    }
}

/// Where queue items play from: a downloaded file first (instant, no
/// network), else the server while it's reachable (original or transcoded).
struct AppSources<'a> {
    /// Knows its account even while its store opens.
    offline: &'a Downloads,
    other_downloads: &'a std::collections::BTreeMap<accounts::AccountId, Downloads>,
    session: &'a super::Session,
    /// Songs streamed before and still whole in the cache play without the
    /// server too.
    cache: &'a StreamCache,
    /// Only while the server answers.
    /// The server is connecting: what needs it waits.
    output: &'a Outputs,
    data: &'a Data,
    source_data: &'a std::collections::BTreeMap<accounts::AccountId, Data>,
    now_playing: &'a now_playing::Session,
    prefer_copy: crate::settings::PreferCopy,
}

impl AppSources<'_> {
    fn live_source(&self, account: accounts::AccountId) -> Option<&crate::source::Src> {
        self.session
            .get(account)
            .filter(|s| !s.unreachable && !s.connecting && s.sign_in.is_none() && !(self.session.work_offline && s.source_caps().network))
            .and_then(|s| s.live())
            .filter(|s| s.account() == account)
    }

    fn downloaded(&self, key: &TrackKey) -> Option<std::path::PathBuf> {
        let offline = if self.offline.owner == Some(key.account) { Some(self.offline) } else { self.other_downloads.get(&key.account) };
        offline.and_then(|d| d.local_path(key))
    }

    fn reachable(&self, key: &TrackKey, track: &Track) -> bool {
        self.session.get(key.account).is_some()
            && (self.downloaded(key).is_some() || self.live_source(key.account).is_some() || cached(self.cache, self.output, key, track).is_some())
    }
}

impl player::Sources for AppSources<'_> {
    /// With its ReplayGain, and whether it plays as part of its album (from
    /// its queue neighbours `prev` and `next`).
    fn source(&self, item: &Item, prev: Option<&Track>, next: Option<&Track>) -> Option<TrackSource> {
        let (track, key, token) = (&item.track, &item.key, item.uid);
        self.session.get(key.account)?;
        let replay_gain = playback::replay_gain(track);
        let album_context = playback::in_album(prev, track, next);
        if let Some(path) = self.downloaded(key) {
            return Some(TrackSource {
                id: stream_name(key).into_owned(),
                url: format!("file://{}", path.display()),
                suffix: track.suffix.clone(),
                replay_gain,
                album_context,
                transcode: None,
                token,
            });
        }
        let Some(source) = self.live_source(key.account) else {
            let path = cached(self.cache, self.output, key, track)?;
            return Some(TrackSource {
                id: stream_name(key).into_owned(),
                url: format!("file://{}", path.display()),
                suffix: path.extension().map(|e| e.to_string_lossy().into_owned()),
                replay_gain,
                album_context,
                transcode: None,
                token,
            });
        };
        let transcode = source.capabilities().transcoding.then(|| self.output.transcode()).flatten();
        let mut bare = track.clone();
        bare.id = key.item.clone();
        let (url, suffix) = source.stream(&bare, transcode.as_ref());
        Some(TrackSource { id: stream_name(key).into_owned(), url, suffix, replay_gain, album_context, transcode, token })
    }

    fn copies(&self, key: &TrackKey) -> Vec<(TrackKey, Track)> {
        let spelling = key.to_string();
        let mut keys = self.data.copies_of_track(&spelling);
        if keys.len() < 2 {
            return Vec::new();
        }
        if self.prefer_copy == crate::settings::PreferCopy::BestQuality {
            // Stable sorting leaves equal formats in settings source order.
            keys.sort_by_key(|key| {
                let format = self.data.copy_format(&key.to_string());
                std::cmp::Reverse((format.as_ref().is_some_and(|f| f.lossless), format.and_then(|f| f.bitrate_kbps).unwrap_or(0)))
            });
        }
        keys.into_iter()
            .filter_map(|key| {
                let track = self.data.track_copy(&key.to_string())?.clone();
                self.reachable(&key, &track).then_some((key, track))
            })
            .collect()
    }

    /// Describes `track` for Now Playing before the engine reaches it.
    fn announce(&self, item: &Item) {
        let track = &item.track;
        let data = self.source_data.get(&item.key.account).unwrap_or(self.data);
        let album = data.album(&track.album_id);
        self.now_playing.known(now_playing::Info {
            key: item.key.clone(),
            token: item.uid,
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: album.map(|a| a.name.clone()),
            duration_secs: track.duration_secs.map(f64::from),
            cover: data.cover_for(track).map(str::to_owned),
        });
    }

    fn waiting_for(&self, item: &Item) -> bool {
        self.session.get(item.key.account).is_some_and(|s| s.connecting)
    }
}

/// Where the playing song comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Origin {
    Downloaded,
    /// The server's stream: the original file, or converted.
    Streamed(Option<audio::Transcode>),
    /// Streamed earlier and kept in the stream cache (played offline).
    Cached,
    /// A file in a folder of music on this computer.
    File,
}

impl Origin {
    /// On this computer: a download, or the stream cache's copy.
    pub fn is_local(&self) -> bool {
        !matches!(self, Origin::Streamed(_))
    }

    /// A few words, for the format pill's hover.
    pub fn short(&self) -> &'static str {
        match self {
            Origin::Downloaded => "Downloaded",
            Origin::Streamed(None) => "Streaming",
            Origin::Streamed(Some(_)) => "Streaming, converted",
            Origin::Cached => "Stream cache",
            Origin::File => "On this computer",
        }
    }

    /// The signal path's Source row: where from, and the file type (or
    /// what the server converts to).
    pub fn source_text(&self, track: &Track) -> String {
        let kind = track.suffix.as_deref().map(str::to_uppercase);
        let with_kind = |from: &str| kind.as_ref().map_or_else(|| from.to_owned(), |k| format!("{from} · {k}"));
        match self {
            Origin::Downloaded => with_kind("Downloaded"),
            Origin::Streamed(None) => with_kind("Streaming"),
            Origin::Streamed(Some(t)) => format!("Streaming, converted to {} {} kbps", t.format.to_uppercase(), t.kbps),
            Origin::Cached => with_kind("Stream cache"),
            Origin::File => with_kind("On this computer"),
        }
    }
}

/// Where `src` plays from: a file in the stream cache, another file (a
/// download, from a source whose music `downloads`; else the source's own
/// file on this computer), or the server.
pub(super) fn origin(src: &TrackSource, cache: &StreamCache, downloads: bool) -> Origin {
    let Some(path) = src.url.strip_prefix("file://") else { return Origin::Streamed(src.transcode.clone()) };
    if std::path::Path::new(path).starts_with(cache.dir()) {
        Origin::Cached
    } else if downloads {
        Origin::Downloaded
    } else {
        Origin::File
    }
}

/// `track`'s file in the stream cache, as it plays now.
pub(super) fn cached(cache: &StreamCache, output: &Outputs, key: &TrackKey, track: &Track) -> Option<std::path::PathBuf> {
    crate::local::cached(cache, output.transcode().as_ref(), key, track)
}

/// A change to how audio is output.
pub enum OutputAction {
    /// ReplayGain and streaming preferences.
    Playback(playback::PlaybackPrefs),
    /// Remember a device's mode (by UID); the device in use switches now.
    DeviceMode(String, PlaybackMode),
    /// Software volume, 0..=1 (clamped).
    Volume(f32),
    /// Bit-perfect on the device in use, or back to normal.
    ToggleBitPerfect,
    /// Play through this device (by UID), e.g. one that came back.
    SelectDevice(String),
    /// Normal mode switches the device to each song's sample rate.
    MatchRate(bool),
    /// The device's own volume, in dB (bit-perfect, on a device that has one).
    DeviceVolume(f32),
    /// How the output control shows things: the green slider, the dB readout,
    /// and a notice when the device changes.
    Show { green: bool, db: crate::settings::DbReadout, toast: bool },
    /// Leave a device (by UID) out of the output menu, or put it back.
    DeviceHidden(String, bool),
    /// The group a device (by UID) goes in in the output menu; empty for the default.
    DeviceGroup(String, String),
}

impl OndApp {
    pub(super) fn apply_output(&mut self, action: OutputAction) {
        match action {
            OutputAction::Playback(prefs) => self.output.set_prefs(prefs, &self.engine),
            OutputAction::DeviceMode(uid, mode) => {
                if self.output.set_device_mode(&uid, mode, &self.engine) {
                    self.bit_perfect = mode == PlaybackMode::BitPerfect;
                }
            }
            OutputAction::Volume(volume) => self.set_volume(volume),
            OutputAction::ToggleBitPerfect => {
                let on = !self.bit_perfect;
                let mode = if on { PlaybackMode::BitPerfect } else { PlaybackMode::Normal };
                if self.output.set_mode_in_use(mode, &self.engine) {
                    self.bit_perfect = on;
                }
            }
            OutputAction::SelectDevice(uid) => self.output.choose(Some(uid), &self.engine),
            OutputAction::MatchRate(on) => self.output.set_match_rate(on, &self.engine),
            OutputAction::DeviceVolume(db) => self.output.set_device_volume(db, &self.engine),
            OutputAction::Show { green, db, toast } => self.output.set_show(green, db, toast),
            OutputAction::DeviceHidden(uid, hidden) => self.output.set_hidden(&uid, hidden),
            OutputAction::DeviceGroup(uid, group) => self.output.set_group(&uid, &group),
        }
    }

    /// A media key, Control Center or headphone command.
    pub(super) fn remote(&mut self, r: Remote) {
        if r == Remote::Raise {
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            return;
        }
        self.with_player(|p, cx| p.remote(r, cx));
    }

    /// Acts on what the output control asked for this frame.
    pub(super) fn output_control_out(&mut self, out: crate::output::ControlOut) {
        if out.settings {
            self.actions.push(Action::OpenSettings(super::settings_page::Section::Output));
        }
        if let Some(level) = out.volume {
            self.actions.push(Action::Output(OutputAction::Volume(level)));
        }
    }

    pub(super) fn handle_engine(&mut self, ev: audio::Event) {
        let before = self.output.devices().1.map(str::to_owned);
        if let Some(mode) = self.output.handle(&ev) {
            // The engine picked the mode for a new device (remembered, or its default).
            self.bit_perfect = mode == PlaybackMode::BitPerfect;
        }
        match &ev {
            audio::Event::DeviceLost { name } => {
                self.ui.toasts.remove(&back_text(name));
                self.show_notice(format!("{name} disconnected. Playing through the system output."));
            }
            // The device chosen is back: play there again (the panel said it was waiting).
            audio::Event::DeviceReturned { uid, name } => {
                if self.prefs.get(|s| s.output_device.as_ref() == Some(uid)) {
                    self.output.choose(Some(uid.clone()), &self.engine);
                    self.show_notice(format!("{name} is back"));
                } else {
                    self.ui.toasts.offer(back_text(name), "Switch back", Action::Output(OutputAction::SelectDevice(uid.clone())));
                }
            }
            // Playing through it again, however that came about: the offer's done.
            audio::Event::Devices { devices, current: Some(uid) } => {
                if let Some(d) = devices.iter().find(|d| &d.uid == uid) {
                    self.ui.toasts.remove(&back_text(&d.name));
                    // Only a move from one device to another, not the first report.
                    let moved = before.as_ref().is_some_and(|b| b != uid);
                    if moved && self.prefs.get(|s| s.device_change_toast) {
                        self.show_notice(format!("Playing through {}", d.name));
                    }
                }
            }
            _ => {}
        }
        self.with_player(|p, cx| p.handle_engine(&ev, cx));
    }

    /// Runs `f` on the player with the engine and the app's sources, then acts
    /// on what it reports.
    pub(super) fn with_player<R>(&mut self, f: impl FnOnce(&mut Player, &mut player::Ctx<'_>) -> R) -> R {
        let mut notes = Vec::new();
        let sources = AppSources {
            offline: &self.downloads,
            other_downloads: &self.multi.downloads,
            session: &self.session,
            cache: &self.cache,
            output: &self.output,
            data: &self.data,
            source_data: &self.multi.data,
            now_playing: &self.now_playing,
            prefer_copy: self.prefs_now().prefer_copy,
        };
        let result = f(&mut self.player, &mut player::Ctx { engine: &self.engine, sources: &sources, notes: &mut notes });
        for note in notes {
            match note {
                Note::Notice(text) => self.show_notice(text),
                Note::Error(text) => self.show_error(text),
                Note::Cleared(n) => {
                    let songs = if n == 1 { "1 song".to_owned() } else { format!("{n} songs") };
                    self.ui.toasts.note(format!("Cleared {songs} from the queue"), "Undo", Action::Queue(QueueEdit::UndoClear));
                }
                Note::ProbeSource(account) => {
                    if self.multi.bridges.contains_key(&account) {
                        self.with_source(account, |app| app.probe());
                    } else if self.session.account() == Some(account) {
                        self.probe();
                    }
                }
                Note::Probe => {
                    if let Some(account) = self.player.queue.current.as_ref().map(|i| i.key.account).filter(|a| self.multi.bridges.contains_key(a)) {
                        self.with_source(account, |app| app.probe());
                    } else {
                        self.probe();
                    }
                }
                Note::Started { key, track, restart } => {
                    if self.session.get(key.account).is_some() && self.prefs_now().analyze_playing && !restart {
                        self.enqueue_analysis(std::slice::from_ref(&*track));
                    }
                }
            }
        }
        result
    }

    #[cfg(test)]
    pub(super) fn start_current(&mut self) {
        self.with_player(|p, cx| p.start_current(cx));
    }

    pub(super) fn queue_edit(&mut self, edit: QueueEdit) {
        let mut selected = self.ui.queue_ui.selected;
        self.with_player(|p, cx| p.edit(edit, &mut selected, cx));
        self.ui.queue_ui.selected = selected;
    }

    /// Where `track` would play from now (tests).
    #[cfg(test)]
    pub(super) fn source(&mut self, token: u64, prev: Option<&Track>, track: &Track, next: Option<&Track>) -> Option<TrackSource> {
        let key = self.session.key(&track.id)?;
        let item = Item { uid: token, key, track: track.clone(), source: crate::queue::Source::Added, next: false };
        self.with_player(|_, cx| cx.sources.source(&item, prev, next))
    }

    /// The server is known to be away (or turned the login down): a track
    /// held for it is skipped, or stops, as offline.
    pub(super) fn server_gone(&mut self) {
        self.with_player(|p, cx| p.resume_held(cx));
    }

    pub(super) fn sync_next(&mut self) {
        self.with_player(|p, cx| p.sync_next(cx));
    }

    pub(super) fn position_secs(&self) -> f64 {
        self.player.position_secs(&self.engine)
    }

    pub(super) fn duration_secs(&self) -> Option<f64> {
        self.player.duration_secs(&self.engine)
    }

    pub(super) fn play_pause(&mut self) {
        self.with_player(|p, cx| p.play_pause(cx));
    }

    pub(super) fn next(&mut self) {
        self.with_player(|p, cx| p.next(cx));
    }

    pub(super) fn previous(&mut self) {
        self.with_player(|p, cx| p.previous(cx));
    }

    pub(super) fn seek(&mut self, to: f64) {
        self.with_player(|p, cx| p.seek(to, cx));
    }

    /// Sets the volume now; the preference is saved once the change settles.
    fn set_volume(&mut self, volume: f32) {
        let volume = volume.clamp(0.0, 1.0);
        self.engine.send(Command::SetVolume(volume));
        self.prefs.edit(|s| s.volume = volume);
    }
}

/// The offer to go back to a device that came back.
fn back_text(name: &str) -> String {
    format!("{name} is back")
}
