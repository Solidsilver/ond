//! Playback engine.
//!
//! ```text
//!  UI ──Command──▶ engine thread ──spawns──▶ decode thread ──rtrb──▶ OutputBackend callback
//!  UI ◀──Event───        │                 fetch → decode → DSP      gain ramp (realtime)
//!  UI ◀───────── Arc<Status> (atomics, read every frame without locking) ◀────────┘
//! ```
//!
//! A *session* decodes the queue back to back into one ring buffer: while the
//! format stays the same the device stream stays open and the next track starts
//! on the sample after the last one (gapless). The engine reports a new track
//! when the output has actually reached it, not when the decoder did.
//!
//! Network sources are read through a [`StreamCache`]: a seekable file that fills
//! while it plays. Once the current track is fully downloaded (or close to its
//! end) the engine prefetches the track set with [`Command::SetNext`].
//!
//! The realtime callback only touches the ring buffer consumer and atomics: no
//! allocation, no locks, no syscalls.
//!
//! The ring runs at the rate the device will play: the file's own rate when the
//! device supports it, else the decoder resamples (see [`choose_rate`]). Which
//! device, and which mode each device gets, is the engine's call too: the UI
//! hands it the remembered per-device modes and is told what happened.

#[cfg(target_os = "macos")]
pub mod coreaudio;
mod decode;
mod device;
mod dither;
mod dsp;
pub mod output;
mod resample;
mod signal;

pub use crossbeam_channel::Receiver as EventReceiver;
pub use dsp::Dsp;
pub use output::{CpalBackend, OutputBackend, OutputError, PREBUFFER_SECS, RingCtl, RingInput, StreamFormat};
pub use device::{DeviceEvent, DeviceInfo, DeviceKind};
pub use dither::Tpdf;
pub use signal::{BitPerfectStatus, DeviceCaps, RateRange, SignalPath, assess, choose_rate, describe, khz};
pub use streamcache::{CacheConfig, StreamCache};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use decode::{DecoderMsg, Session};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;
use streamcache::CacheKey;

/// Start fetching the next track this long before the current one ends, even if
/// the current download hasn't finished.
const PREFETCH_LEAD_SECS: f64 = 30.0;

/// How much DSP the signal is allowed to see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackMode {
    /// Software volume, ReplayGain and resampling are allowed.
    #[default]
    Normal,
    /// Samples reach the device untouched; the device follows the source rate.
    BitPerfect,
}

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub mode: PlaybackMode,
    /// Decoded audio buffered ahead of the output, in seconds.
    pub buffer_secs: f32,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self { mode: PlaybackMode::Normal, buffer_secs: 2.0 }
    }
}

#[derive(Debug, Clone)]
pub struct TrackSource {
    pub id: String,
    pub url: String,
    /// File extension from the server (`flac`, `m4a`, ...) to speed up probing.
    pub suffix: Option<String>,
}

impl TrackSource {
    fn cache_key(&self) -> CacheKey {
        CacheKey::new(self.id.clone(), self.suffix.clone())
    }

    fn is_local(&self) -> bool {
        self.url.starts_with("file://")
    }
}

#[derive(Debug)]
pub enum Command {
    Play(TrackSource),
    Pause,
    Resume,
    Stop,
    SetVolume(f32),
    /// Takes effect immediately: the gain stage is bypassed (after a 15 ms glide
    /// to unity) and the signal-path status is re-judged.
    SetMode(PlaybackMode),
    /// Jump within the current track.
    Seek(Duration),
    /// The track that will play after the current one: prefetched, and played
    /// gaplessly when its format matches. Doesn't change what's playing now.
    SetNext(Option<TrackSource>),
    /// Play through this device (by UID), or `None` to follow the system default.
    /// The current track carries on from where it was.
    SelectDevice(Option<String>),
    /// The modes the user picked per device UID. A device not listed gets
    /// [`DeviceInfo::default_mode`] when playback moves to it.
    DeviceModes(HashMap<String, PlaybackMode>),
    /// Ask for an [`Event::Devices`].
    RefreshDevices,
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum Event {
    /// The output reached a track. `gapless` is true when it followed the previous
    /// track on the next sample without reopening the device.
    TrackStarted { track_id: String, format: StreamFormat, gapless: bool },
    /// The device stream changed, or the mode did; describes what reaches the device.
    SignalPath(SignalPath),
    /// The output devices, and which one playback uses.
    Devices { devices: Vec<DeviceInfo>, current: Option<String> },
    /// The engine changed mode by itself (device switch or fallback). Not sent for `SetMode`.
    ModeChanged { mode: PlaybackMode, device: Option<String> },
    /// The device in use went away; playback moved to the system default in Normal mode.
    DeviceLost { name: String },
    /// A device that went away is back; the UI may offer to switch to it.
    DeviceReturned { uid: String, name: String },
    /// Playback reached the end of the last queued track.
    Ended { track_id: String },
    /// A seek landed; playback continues from `position_secs` once buffered.
    Seeked { track_id: String, position_secs: f64 },
    Error { track_id: Option<String>, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PlayState {
    Stopped = 0,
    Loading = 1,
    Playing = 2,
    Paused = 3,
    /// Playing, but silent until enough audio is queued (after a seek, a track
    /// start or an underrun).
    Buffering = 4,
}

/// Lock-free snapshot the UI polls every frame.
#[derive(Debug, Default)]
pub struct Status {
    state: AtomicU8,
    /// Frames the device has actually consumed for the current track.
    pub frames_played: AtomicU64,
    /// 0 when unknown (e.g. unseekable stream without a duration header).
    pub total_frames: AtomicU64,
    pub sample_rate: AtomicU32,
    pub underruns: AtomicU64,
    /// Bytes of the current track on disk, and its size (0 when unknown or local).
    pub fetched_bytes: AtomicU64,
    pub total_bytes: AtomicU64,
    /// Track boundaries the output has crossed in this session (bumped by the callback).
    pub track_seq: AtomicU64,
    volume_bits: AtomicU32,
    pub(crate) paused: AtomicBool,
    /// The callback is holding or prebuffering.
    pub(crate) buffering: AtomicBool,
    /// The current mode: read by the callback (gain bypass) and the DSP stage.
    pub(crate) bit_perfect: AtomicBool,
}

impl Status {
    pub fn state(&self) -> PlayState {
        match self.state.load(Ordering::Relaxed) {
            1 => PlayState::Loading,
            2 if self.buffering.load(Ordering::Relaxed) => PlayState::Buffering,
            2 => PlayState::Playing,
            3 => PlayState::Paused,
            _ => PlayState::Stopped,
        }
    }

    fn set_state(&self, s: PlayState) {
        self.state.store(s as u8, Ordering::Relaxed);
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume_bits.load(Ordering::Relaxed))
    }

    fn set_volume(&self, v: f32) {
        self.volume_bits.store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn mode(&self) -> PlaybackMode {
        if self.bit_perfect.load(Ordering::Relaxed) { PlaybackMode::BitPerfect } else { PlaybackMode::Normal }
    }

    pub fn position_secs(&self) -> f64 {
        secs(self.frames_played.load(Ordering::Relaxed), self.sample_rate.load(Ordering::Relaxed))
    }

    /// Share of the current file downloaded, if the size is known.
    pub fn fetched_fraction(&self) -> Option<f32> {
        let total = self.total_bytes.load(Ordering::Relaxed);
        (total > 0).then(|| self.fetched_bytes.load(Ordering::Relaxed) as f32 / total as f32)
    }

    pub fn duration_secs(&self) -> Option<f64> {
        let total = self.total_frames.load(Ordering::Relaxed);
        (total > 0).then(|| secs(total, self.sample_rate.load(Ordering::Relaxed)))
    }
}

fn secs(frames: u64, rate: u32) -> f64 {
    if rate == 0 { 0.0 } else { frames as f64 / rate as f64 }
}

/// The best output for this platform: native CoreAudio on macOS (device choice,
/// rate switching, exclusive mode), cpal elsewhere or if CoreAudio can't start.
pub fn default_backend() -> Box<dyn OutputBackend> {
    #[cfg(target_os = "macos")]
    match coreaudio::CoreAudioBackend::new() {
        Ok(b) => return Box::new(b),
        Err(e) => tracing::warn!("CoreAudio output unavailable ({e}); using cpal"),
    }
    Box::new(CpalBackend::default())
}

/// Cloneable handle to a running engine.
#[derive(Clone)]
pub struct EngineHandle {
    commands: Sender<Command>,
    pub events: Receiver<Event>,
    pub status: Arc<Status>,
}

impl EngineHandle {
    pub fn send(&self, cmd: Command) {
        // The engine only goes away on Shutdown; a send after that is a no-op.
        let _ = self.commands.send(cmd);
    }
}

/// Starts the engine thread with the given output backend. Without a cache only
/// `file://` sources play.
pub fn spawn(config: EngineConfig, backend: Box<dyn OutputBackend>, cache: Option<StreamCache>) -> EngineHandle {
    let (cmd_tx, cmd_rx) = unbounded();
    let (ev_tx, ev_rx) = unbounded();
    let status = Arc::new(Status::default());
    status.set_volume(1.0);
    status.bit_perfect.store(config.mode == PlaybackMode::BitPerfect, Ordering::Relaxed);
    let st = status.clone();
    thread::Builder::new()
        .name("audio-engine".into())
        .spawn(move || {
            Engine {
                config,
                backend,
                status: st,
                events: ev_tx,
                cache,
                session: None,
                tracks: Vec::new(),
                out_seq: 0,
                ring: None,
                next: None,
                prefetched: None,
                device_modes: HashMap::new(),
                current_device: None,
                known: Vec::new(),
                lost: None,
            }
            .run(cmd_rx)
        })
        .expect("spawn audio engine thread");
    EngineHandle { commands: cmd_tx, events: ev_rx, status }
}

/// A track the current session's decoder has started, by session sequence number.
struct SessionTrack {
    src: TrackSource,
    /// The file's format.
    format: StreamFormat,
    /// What the device plays (after resampling).
    output: StreamFormat,
    /// At the output rate.
    total_frames: Option<u64>,
}

/// The ring currently connected to the device.
struct OpenRing {
    format: StreamFormat,
    ctl: Arc<RingCtl>,
}

struct Engine {
    config: EngineConfig,
    backend: Box<dyn OutputBackend>,
    status: Arc<Status>,
    events: Sender<Event>,
    cache: Option<StreamCache>,
    session: Option<Session>,
    /// Indexed by session sequence number.
    tracks: Vec<SessionTrack>,
    /// Sequence number of the track the output is playing.
    out_seq: u64,
    ring: Option<OpenRing>,
    /// What the UI wants after the output's current track.
    next: Option<TrackSource>,
    /// Id of the track already handed to the cache's prefetch slot.
    prefetched: Option<String>,
    device_modes: HashMap<String, PlaybackMode>,
    /// UID of the device in use, when the backend can tell.
    current_device: Option<String>,
    /// The last device list seen.
    known: Vec<DeviceInfo>,
    /// A device that disappeared while in use: (uid, name).
    lost: Option<(String, String)>,
}

impl Engine {
    fn run(mut self, commands: Receiver<Command>) {
        loop {
            match commands.recv_timeout(Duration::from_millis(10)) {
                Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.poll_session();
            self.poll_fetch();
            self.poll_devices();
        }
        self.stop();
        if let Some(cache) = &self.cache {
            cache.cancel_all();
        }
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Play(src) => self.start(src, None),
            Command::Pause => {
                if matches!(self.status.state(), PlayState::Playing | PlayState::Buffering) {
                    self.status.paused.store(true, Ordering::Relaxed);
                    self.status.set_state(PlayState::Paused);
                }
            }
            Command::Resume => {
                if self.status.state() == PlayState::Paused {
                    self.status.paused.store(false, Ordering::Relaxed);
                    self.status.set_state(PlayState::Playing);
                }
            }
            Command::Stop => self.stop(),
            Command::SetVolume(v) => self.status.set_volume(v),
            Command::SetMode(m) => self.apply_mode(m),
            Command::Seek(to) => self.seek(to),
            Command::SetNext(next) => self.set_next(next),
            Command::SelectDevice(uid) => match self.backend.select_device(uid.as_deref()) {
                Ok(()) => {
                    self.lost = None;
                    self.device_switched(None);
                    // Chosen but not plugged in (e.g. the saved DAC at startup): offer it when it appears.
                    if let Some(uid) = uid.filter(|u| self.current_device.as_ref() != Some(u)) {
                        self.lost = Some((uid.clone(), uid));
                    }
                }
                Err(e) => {
                    let _ = self.events.send(Event::Error { track_id: None, message: e.to_string() });
                }
            },
            Command::DeviceModes(modes) => self.device_modes = modes,
            Command::RefreshDevices => {
                self.known = self.backend.devices();
                self.current_device = self.backend.current_device().map(|d| d.uid);
                self.send_devices();
            }
            Command::Shutdown => unreachable!("handled in run"),
        }
    }

    fn apply_mode(&mut self, m: PlaybackMode) {
        self.config.mode = m;
        self.status.bit_perfect.store(m == PlaybackMode::BitPerfect, Ordering::Relaxed);
        self.backend.set_mode(m);
        self.report_signal_path();
    }

    /// Starts a new session at `src`, optionally from `start_at`.
    fn start(&mut self, src: TrackSource, start_at: Option<Duration>) {
        self.stop();
        self.status.set_state(PlayState::Loading);
        self.status.paused.store(false, Ordering::Relaxed);
        let caps = self.backend.device_caps();
        let session =
            Session::start(src, start_at, self.config.clone(), self.status.clone(), self.cache.clone(), caps);
        session.set_next(0, self.next.clone());
        self.session = Some(session);
    }

    fn stop(&mut self) {
        if let Some(s) = self.session.take() {
            s.cancel();
        }
        // Synchronous: no callback runs after this, so the counters below stay reset.
        self.backend.close();
        self.ring = None;
        self.tracks.clear();
        self.out_seq = 0;
        self.status.set_state(PlayState::Stopped);
        self.status.track_seq.store(0, Ordering::Relaxed);
        self.status.frames_played.store(0, Ordering::Relaxed);
        self.status.total_frames.store(0, Ordering::Relaxed);
        self.status.fetched_bytes.store(0, Ordering::Relaxed);
        self.status.total_bytes.store(0, Ordering::Relaxed);
        self.status.buffering.store(false, Ordering::Relaxed);
    }

    fn current(&self) -> Option<&SessionTrack> {
        self.tracks.get(self.out_seq as usize)
    }

    fn seek(&mut self, to: Duration) {
        let Some(session) = &self.session else { return };
        // Silence the output now; the decoder releases it once it has repositioned.
        let hold = self.ring.as_ref().map_or(0, |r| r.ctl.hold());
        if hold > 0 {
            self.status.buffering.store(true, Ordering::Relaxed);
        }
        session.seek(self.out_seq, to, hold);
    }

    fn set_next(&mut self, next: Option<TrackSource>) {
        if next.as_ref().map(|n| &n.id) != self.prefetched.as_ref() {
            self.prefetched = None;
        }
        self.next = next.clone();
        let Some(session) = &self.session else { return };
        match self.tracks.get(self.out_seq as usize + 1) {
            // The decoder is still on the output's track: tell it what follows.
            None => session.set_next(self.out_seq, next),
            // It already moved on to the right track.
            Some(t) if next.as_ref().is_some_and(|n| n.id == t.src.id) => {}
            // It moved on to a track that's no longer next (the queue changed in the
            // last few seconds of the track). Restart the current track where it is.
            Some(_) => {
                if let Some(resume) = self.take_position() {
                    self.resume(resume);
                }
            }
        }
    }

    /// The current track and position, if something is playing or paused.
    fn take_position(&self) -> Option<(TrackSource, Duration, bool)> {
        let state = self.status.state();
        if self.session.is_none() || state == PlayState::Stopped {
            return None;
        }
        let src = self.current().map(|t| t.src.clone())?;
        Some((src, Duration::from_secs_f64(self.status.position_secs()), state == PlayState::Paused))
    }

    /// Restarts at a position from [`take_position`](Self::take_position), keeping pause.
    fn resume(&mut self, (src, at, paused): (TrackSource, Duration, bool)) {
        self.start(src, Some(at));
        if paused {
            self.status.paused.store(true, Ordering::Relaxed);
            self.status.set_state(PlayState::Paused);
        }
    }

    /// Playback moved to another device: pick its mode and carry on where we were.
    fn device_switched(&mut self, force: Option<PlaybackMode>) {
        let resume = self.take_position();
        self.stop();
        self.known = self.backend.devices();
        let info = self.backend.current_device();
        self.current_device = info.as_ref().map(|d| d.uid.clone());
        let mode = force.unwrap_or_else(|| match &info {
            Some(d) => self.device_modes.get(&d.uid).copied().unwrap_or_else(|| d.default_mode()),
            None => self.config.mode,
        });
        if mode != self.config.mode {
            self.apply_mode(mode);
            let _ = self.events.send(Event::ModeChanged { mode, device: self.current_device.clone() });
        } else {
            self.backend.set_mode(mode);
        }
        self.send_devices();
        if let Some(r) = resume {
            self.resume(r);
        }
    }

    fn send_devices(&self) {
        let _ = self.events.send(Event::Devices { devices: self.known.clone(), current: self.current_device.clone() });
    }

    /// Reacts to devices coming and going.
    fn poll_devices(&mut self) {
        let events = self.backend.poll_device_events();
        if events.is_empty() {
            return;
        }
        let devices = self.backend.devices();
        let current = self.current_device.clone();
        let gone = current.as_ref().filter(|uid| !devices.iter().any(|d| &d.uid == *uid));
        if let Some(uid) = gone {
            let name = self.known.iter().find(|d| &d.uid == uid).map_or_else(|| uid.clone(), |d| d.name.clone());
            tracing::warn!("output device {name} went away; falling back to the system default");
            self.lost = Some((uid.clone(), name.clone()));
            let _ = self.backend.select_device(None);
            let _ = self.events.send(Event::DeviceLost { name });
            self.device_switched(Some(PlaybackMode::Normal));
        } else if events.contains(&DeviceEvent::DefaultChanged)
            && self.backend.current_device().map(|d| d.uid) != current
        {
            // Following the default and it moved (e.g. headphones plugged in).
            self.device_switched(None);
        } else {
            self.known = devices.clone();
            self.send_devices();
        }
        if let Some((uid, _)) = self.lost.clone()
            && let Some(back) = devices.iter().find(|d| d.uid == uid)
        {
            self.lost = None;
            let _ = self.events.send(Event::DeviceReturned { uid, name: back.name.clone() });
        }
    }

    /// Describes the path from the current track to the device.
    fn report_signal_path(&mut self) {
        let (Some(ring), Some(track)) = (self.ring.as_ref(), self.current()) else { return };
        let (source, device_rate) = (track.format, ring.format.sample_rate);
        let caps = self.backend.device_caps();
        let bit_perfect = assess(self.config.mode, &source, device_rate, caps.as_ref());
        // The integer format actually in use: what the callback dithers to.
        let device_bits = caps.as_ref().and_then(|c| c.active_bits);
        let resampled = device_rate != source.sample_rate;
        let dithered = device_bits
            .is_some_and(|b| b < 32 && (resampled || source.bits_per_sample.is_none_or(|s| s > b)));
        let path = SignalPath {
            output: self.backend.name().to_owned(),
            source,
            device_rate,
            device_bits,
            resampled,
            dithered,
            bit_perfect,
        };
        let _ = self.events.send(Event::SignalPath(path));
    }

    /// Publishes download progress and starts the next track's prefetch when it's time.
    fn poll_fetch(&mut self) {
        let Some(cache) = &self.cache else { return };
        let Some(src) = self.tracks.get(self.out_seq as usize).map(|t| &t.src) else { return };
        if src.is_local() {
            return;
        }
        let progress = cache.progress(&src.cache_key());
        if let Some(pr) = progress {
            self.status.fetched_bytes.store(pr.downloaded, Ordering::Relaxed);
            self.status.total_bytes.store(pr.len.unwrap_or(0), Ordering::Relaxed);
        }

        let Some(next) = &self.next else { return };
        if next.is_local() || self.prefetched.as_ref() == Some(&next.id) {
            return;
        }
        let near_end =
            self.status.duration_secs().is_some_and(|d| d - self.status.position_secs() <= PREFETCH_LEAD_SECS);
        if progress.is_some_and(|p| p.complete) || near_end {
            cache.prefetch(&next.cache_key(), &next.url);
            self.prefetched = Some(next.id.clone());
        }
    }

    fn poll_session(&mut self) {
        // Read before draining messages: anything the thread sent before exiting is then seen below.
        let finished = self.session.as_ref().is_some_and(Session::finished);
        while let Some(msg) = self.session.as_ref().and_then(Session::try_msg) {
            match msg {
                DecoderMsg::Track { seq, src, format, output, total_frames } => {
                    debug_assert_eq!(seq as usize, self.tracks.len());
                    self.tracks.push(SessionTrack { src, format, output, total_frames });
                }
                DecoderMsg::Ring { format, input, first_seq } => {
                    if !self.open_ring(format, input, first_seq) {
                        return;
                    }
                }
                DecoderMsg::Seeked { seq, position_secs } => {
                    if let Some(t) = self.tracks.get(seq as usize) {
                        let track_id = t.src.id.clone();
                        let _ = self.events.send(Event::Seeked { track_id, position_secs });
                    }
                }
                DecoderMsg::SeekMiss { seq, to } => {
                    if let Some(src) = self.tracks.get(seq as usize).map(|t| t.src.clone()) {
                        self.start(src, Some(to));
                    }
                    return;
                }
                DecoderMsg::Error { seq, message } => {
                    let seq = seq.min(self.tracks.len().saturating_sub(1) as u64);
                    let track_id = self.tracks.get(seq as usize).map(|t| t.src.id.clone());
                    let _ = self.events.send(Event::Error { track_id, message });
                    // A later track failing lets the current one play out; anything else stops.
                    if self.ring.is_none() || seq <= self.out_seq {
                        self.stop();
                        return;
                    }
                }
            }
        }

        // Track boundaries the callback has played past.
        let crossed = self.status.track_seq.load(Ordering::Acquire);
        while self.out_seq < crossed {
            self.out_seq += 1;
            // The UI names the new next once it hears about this track.
            self.next = None;
            self.prefetched = None;
            self.announce(true);
        }

        if self.session.is_some() && finished && (self.ring.is_none() || self.backend.drained()) {
            let track_id = self.current().map(|t| t.src.id.clone());
            self.stop();
            if let Some(track_id) = track_id {
                let _ = self.events.send(Event::Ended { track_id });
            }
        }
    }

    /// Connects a new ring to the device. Returns false if the session was stopped.
    fn open_ring(&mut self, format: StreamFormat, input: RingInput, first_seq: u64) -> bool {
        let ctl = input.ctl.clone();
        let reopened = self.ring.is_some();
        // Close first so the old callback can't touch the counters set below.
        self.backend.close();
        self.out_seq = first_seq;
        self.status.track_seq.store(first_seq, Ordering::Release);
        self.status.sample_rate.store(format.sample_rate, Ordering::Relaxed);
        if !reopened && !self.status.paused.load(Ordering::Relaxed) {
            self.status.set_state(PlayState::Playing);
        }
        if self.current_device.is_none() {
            self.current_device = self.backend.current_device().map(|d| d.uid);
        }
        match self.backend.open(format, input, self.status.clone()) {
            Ok(()) => {
                self.ring = Some(OpenRing { format, ctl });
                self.announce(false);
                self.report_signal_path();
                true
            }
            Err(e) => {
                let track_id = self.current().map(|t| t.src.id.clone());
                let _ = self.events.send(Event::Error { track_id, message: e.to_string() });
                self.stop();
                false
            }
        }
    }

    /// Publishes the output's current track.
    fn announce(&mut self, gapless: bool) {
        let Some(t) = self.current() else { return };
        let (track_id, format, total, rate) = (t.src.id.clone(), t.format, t.total_frames, t.output.sample_rate);
        self.status.total_frames.store(total.unwrap_or(0), Ordering::Relaxed);
        self.status.sample_rate.store(rate, Ordering::Relaxed);
        let _ = self.events.send(Event::TrackStarted { track_id, format, gapless });
    }
}

#[cfg(test)]
mod tests;
