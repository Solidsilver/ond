//! Playback as the app drives it: the player with the engine and where each
//! track plays from (a download, else the server), what the player reports
//! back, the engine's events, remote commands and the volume.

use super::{Action, Data, OndApp};
use crate::now_playing::{self, Remote};
use crate::output::Outputs;
use crate::playback;
use crate::player::{self, Note, Player, QueueEdit};
use audio::{Command, PlaybackMode, TrackSource};
use library::Track;
use offline::OfflineStore;
use subsonic::Client;

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
                self.bit_perfect = !self.bit_perfect;
                let mode = if self.bit_perfect { PlaybackMode::BitPerfect } else { PlaybackMode::Normal };
                self.output.set_mode_in_use(mode, &self.engine);
            }
            OutputAction::SelectDevice(uid) => self.output.choose(Some(uid), &self.engine),
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

    pub(super) fn handle_engine(&mut self, ev: audio::Event) {
        if let Some(mode) = self.output.handle(&ev) {
            // The engine picked the mode for a new device (remembered, or its default).
            self.bit_perfect = mode == PlaybackMode::BitPerfect;
        }
        match &ev {
            audio::Event::DeviceLost { name } => {
                self.ui.toasts.remove(&back_text(name));
                self.show_notice(format!("{name} disconnected. Playing through the system output."));
            }
            // Offered, not switched to.
            audio::Event::DeviceReturned { uid, name } => {
                self.ui.toasts.offer(back_text(name), "Switch back", Action::Output(OutputAction::SelectDevice(uid.clone())));
            }
            // Playing through it again, however that came about: the offer's done.
            audio::Event::Devices { devices, current: Some(uid) } => {
                if let Some(d) = devices.iter().find(|d| &d.uid == uid) {
                    self.ui.toasts.remove(&back_text(&d.name));
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
                    if self.prefs_now().analyze_playing && !restart {
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
        self.with_player(|_, cx| cx.sources.source(token, prev, track, next))
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
