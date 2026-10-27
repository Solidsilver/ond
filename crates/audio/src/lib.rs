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

mod channels;
#[cfg(target_os = "linux")]
pub mod alsa;
#[cfg(target_os = "macos")]
pub mod coreaudio;
mod decode;
mod device;
mod dither;
pub mod output;
pub mod replaygain;
mod resample;
mod signal;
pub mod tap;

pub use channels::ChannelChange;
pub use crossbeam_channel::Receiver as EventReceiver;
pub use output::{CpalBackend, Marker, OutputBackend, OutputError, PREBUFFER_SECS, RingCtl, RingInput, StreamFormat};
pub use device::{DeviceEvent, DeviceInfo, DeviceKind};
pub use dither::Tpdf;
pub use replaygain::{GainConfig, GainMode, ReplayGain};
pub use signal::{BitPerfectStatus, DeviceCaps, GainState, RateRange, SignalPath, adjust, assess, choose_rate, describe, khz};
use streamcache::{CacheKey, StreamCache};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use decode::{DecoderMsg, Session};
use replaygain::{TrackGains, applied_gain};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

// The decoder can be at most `MARKER_CAP` tracks ahead of the output, and both
// write and read the gain table by track number modulo `GAIN_SLOTS`: the table
// must be larger, or the decoder could overwrite the gain of a track still playing.
const _: () = assert!(output::MARKER_CAP < replaygain::GAIN_SLOTS);

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

#[derive(Debug, Clone, Default)]
pub struct TrackSource {
    pub id: String,
    pub url: String,
    /// File extension from the server (`flac`, `m4a`, ...) to speed up probing.
    /// For a transcoded stream, the transcode's format.
    pub suffix: Option<String>,
    /// From the server or library; the file's own tags fill any gaps.
    pub replay_gain: ReplayGain,
    /// The track plays as part of its album (album gain under [`GainMode::Auto`]).
    pub album_context: bool,
    /// The server converts the file on the fly; `None` streams the original.
    pub transcode: Option<Transcode>,
    /// Opaque to the engine and echoed in [`Event::TrackStarted`] / [`Event::Ended`]:
    /// tells two plays of the same track apart (e.g. a queue item id). 0: none.
    pub token: u64,
}

/// A server-side conversion, e.g. AAC at 256 kbps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcode {
    pub format: String,
    pub kbps: u32,
}

impl Transcode {
    /// "AAC 256".
    pub fn label(&self) -> String {
        format!("{} {}", self.format.to_ascii_uppercase(), self.kbps)
    }
}

impl TrackSource {
    /// Original and transcoded copies of a track are cached separately.
    fn cache_key(&self) -> CacheKey {
        match &self.transcode {
            None => CacheKey::original(self.id.clone(), self.suffix.clone()),
            Some(t) => CacheKey::transcode(&self.id, &t.format, t.kbps),
        }
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
    /// ReplayGain settings. Applies to the current track at once (with a short
    /// glide) and to every track after it.
    SetGain(GainConfig),
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
    /// Normal mode switches the device to each song's rate (true), or leaves
    /// the device at its own rate and resamples to it (false, the default).
    /// The current track carries on from where it was.
    MatchRate(bool),
    /// Ask for an [`Event::Devices`].
    RefreshDevices,
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum Event {
    /// The output reached a track. `gapless` is true when it followed the previous
    /// track on the next sample without reopening the device. `restart` is true
    /// when the engine started the same track over by itself (device switch, a
    /// seek it couldn't serve in place, the queue changing under it): not a new play.
    TrackStarted { track_id: String, token: u64, format: StreamFormat, gapless: bool, restart: bool },
    /// The device stream changed, or the mode did; describes what reaches the device.
    SignalPath(SignalPath),
    /// The output devices, and which one playback uses.
    Devices { devices: Vec<DeviceInfo>, current: Option<String> },
    /// The engine changed mode by itself (device switch or fallback). Not sent for `SetMode`.
    ModeChanged { mode: PlaybackMode, device: Option<String> },
    /// The device in use went away; playback moved to the system default, in
    /// the mode picked for it if there is one, else in Normal mode.
    DeviceLost { name: String },
    /// A device that went away is back; the UI may offer to switch to it.
    DeviceReturned { uid: String, name: String },
    /// Playback reached the end of the last queued track.
    Ended { track_id: String, token: u64 },
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
///
/// Three threads write to it: the engine, the output callback and the
/// session's decode thread. Each field says which. Two rules keep them apart:
/// - The engine resets the callback's fields only while no callback runs
///   (after [`OutputBackend::close`], which is synchronous).
/// - The decoder writes only through `Ctx::publish` (in `decode.rs`), which
///   does nothing once the session has been cancelled: a replaced decoder
///   can't reach into its successor's state.
#[derive(Debug, Default)]
pub struct Status {
    /// Engine.
    state: AtomicU8,
    /// Frames the device has actually consumed for the current track.
    /// Callback (advances it, zeroes it at a gapless boundary); engine (zeroes
    /// it on stop and when a format change reopens the device); decoder (sets
    /// it where a seek landed: after the callback has flushed the ring and
    /// before any new audio is queued, so the callback isn't counting then).
    pub(crate) frames_played: AtomicU64,
    /// 0 when unknown (e.g. unseekable stream without a duration header). Engine.
    pub(crate) total_frames: AtomicU64,
    /// The output rate of the current track. Engine.
    pub(crate) sample_rate: AtomicU32,
    /// Callback.
    pub(crate) underruns: AtomicU64,
    /// Decoded frames queued ahead of the output (the ring's fill), as of the
    /// last callback. Callback.
    pub(crate) buffered_frames: AtomicU64,
    /// Bytes of the current track on disk, and its size (0 when unknown or
    /// local). Engine.
    pub(crate) fetched_bytes: AtomicU64,
    pub(crate) total_bytes: AtomicU64,
    /// Playback is downloading: the current track (whatever its size, known or
    /// not, as with a transcode) or the next one's prefetch isn't complete. Engine.
    pub(crate) downloading: AtomicBool,
    /// Track boundaries the output has crossed in this session. Callback
    /// (bumps it at each boundary); engine (sets it when a ring opens, resets it on stop).
    pub(crate) track_seq: AtomicU64,
    /// Engine.
    volume_bits: AtomicU32,
    /// Engine.
    pub(crate) paused: AtomicBool,
    /// The callback is holding or prebuffering. Callback; engine (sets it the
    /// moment it asks for a seek hold, clears it on stop).
    pub(crate) buffering: AtomicBool,
    /// The current mode: read by the output callback (gain bypass). Engine.
    pub(crate) bit_perfect: AtomicBool,
    /// ReplayGain per session track, read by the callback at each boundary.
    /// Decoder (each track's, before its first sample is queued); engine (on a
    /// settings change, when the decoder reports the track, and reset on stop).
    pub(crate) gains: TrackGains,
    /// A copy of what plays, for visuals; idle until something reads it.
    /// Callback writes, the UI reads.
    pub tap: tap::Tap,
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

    /// Playback is using the network: loading, buffering, or still downloading
    /// the current track or the next one's prefetch. Background work that wants
    /// bandwidth should wait.
    pub fn needs_network(&self) -> bool {
        matches!(self.state(), PlayState::Loading | PlayState::Buffering) || self.downloading.load(Ordering::Relaxed)
    }

    /// Share of the current file downloaded, if the size is known.
    pub fn fetched_fraction(&self) -> Option<f32> {
        let total = self.total_bytes.load(Ordering::Relaxed);
        (total > 0).then(|| self.fetched_bytes.load(Ordering::Relaxed) as f32 / total as f32)
    }

    /// Seconds of decoded audio queued ahead of the output.
    pub fn buffered_secs(&self) -> f64 {
        secs(self.buffered_frames.load(Ordering::Relaxed), self.sample_rate.load(Ordering::Relaxed))
    }

    /// The current track or the next one's prefetch is still downloading.
    pub fn downloading(&self) -> bool {
        self.downloading.load(Ordering::Relaxed)
    }

    /// Times the output ran dry since the engine started.
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    /// Bytes of the current file on disk and its size (0 when unknown or local).
    pub fn fetched_bytes(&self) -> (u64, u64) {
        (self.fetched_bytes.load(Ordering::Relaxed), self.total_bytes.load(Ordering::Relaxed))
    }

    pub fn duration_secs(&self) -> Option<f64> {
        let total = self.total_frames.load(Ordering::Relaxed);
        (total > 0).then(|| secs(total, self.sample_rate.load(Ordering::Relaxed)))
    }

    /// Shows a track at `position` of `duration` seconds in `state`, with
    /// `fetched` of the file downloaded, without an engine stream behind it.
    /// For UI previews only; a running engine overwrites it.
    #[cfg(feature = "preview")]
    pub fn preview(&self, state: PlayState, position: f64, duration: f64, fetched: f32) {
        const RATE: u32 = 44_100;
        let buffering = state == PlayState::Buffering;
        self.set_state(if buffering { PlayState::Playing } else { state });
        self.buffering.store(buffering, Ordering::Relaxed);
        self.sample_rate.store(RATE, Ordering::Relaxed);
        self.frames_played.store((position * RATE as f64) as u64, Ordering::Relaxed);
        self.total_frames.store((duration * RATE as f64) as u64, Ordering::Relaxed);
        self.total_bytes.store(1_000_000, Ordering::Relaxed);
        self.fetched_bytes.store((fetched.clamp(0.0, 1.0) * 1_000_000.0) as u64, Ordering::Relaxed);
        self.buffered_frames.store((1.6 * RATE as f64) as u64, Ordering::Relaxed);
        self.downloading.store(fetched < 1.0, Ordering::Relaxed);
    }
}

/// How often the engine checks on playback while something is happening.
const TICK: Duration = Duration::from_millis(10);
/// The longest a format change waits for the old stream's last buffer to play
/// (normally one device period).
const REOPEN_DRAIN_WAIT: Duration = Duration::from_millis(250);
/// The longest it sleeps while stopped or paused (it still notices devices
/// coming and going). A command always wakes it at once.
const IDLE_TICK: Duration = Duration::from_millis(250);

/// The next wait after a quiet one: back to `TICK` while playing or loading,
/// otherwise doubling up to `IDLE_TICK`, so a stopped engine barely wakes.
fn next_wait(wait: Duration, state: PlayState) -> Duration {
    match state {
        PlayState::Stopped | PlayState::Paused => (wait * 2).min(IDLE_TICK),
        _ => TICK,
    }
}

fn secs(frames: u64, rate: u32) -> f64 {
    if rate == 0 { 0.0 } else { frames as f64 / rate as f64 }
}

/// The best output for this platform: native CoreAudio on macOS (device choice,
/// rate switching, exclusive mode), ALSA on Linux (device choice, exclusive
/// hardware access in bit-perfect mode, the sound server otherwise), cpal
/// elsewhere or if CoreAudio can't start.
pub fn default_backend() -> Box<dyn OutputBackend> {
    #[cfg(target_os = "macos")]
    match coreaudio::CoreAudioBackend::new() {
        Ok(b) => return Box::new(b),
        Err(e) => tracing::warn!("CoreAudio output unavailable ({e}); using cpal"),
    }
    #[cfg(target_os = "linux")]
    let backend: Box<dyn OutputBackend> = Box::new(self::alsa::AlsaBackend::new());
    #[cfg(not(target_os = "linux"))]
    let backend: Box<dyn OutputBackend> = Box::new(CpalBackend::default());
    backend
}

/// Cloneable handle to a running engine.
#[derive(Clone)]
pub struct EngineHandle {
    commands: Sender<Command>,
    pub events: Receiver<Event>,
    pub status: Arc<Status>,
    /// Disconnects once the engine thread has finished, output closed.
    finished: Receiver<()>,
}

impl EngineHandle {
    pub fn send(&self, cmd: Command) {
        // The engine only goes away on Shutdown; a send after that is a no-op.
        let _ = self.commands.send(cmd);
    }

    /// Stops the engine and waits (up to `wait`) for it to let go of the
    /// output: the backend puts the device back the way it found it, which
    /// can't happen once the process has exited. Calling it again returns at once.
    pub fn shutdown(&self, wait: Duration) {
        self.send(Command::Shutdown);
        let _ = self.finished.recv_timeout(wait);
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
    let (finished_tx, finished) = crossbeam_channel::bounded::<()>(0);
    thread::Builder::new()
        .name("audio-engine".into())
        .spawn(move || {
            Engine {
                config,
                backend,
                status: st,
                events: ev_tx,
                cache,
                gain: Arc::default(),
                session: None,
                tracks: Vec::new(),
                out_seq: 0,
                ring: None,
                next: None,
                prefetched: None,
                opening: None,
                held_seek: None,
                device_modes: HashMap::new(),
                current_device: None,
                known: Vec::new(),
                lost: None,
                restarting: false,
            }
            .run(cmd_rx);
            // The engine (and its backend) is gone: the device is released.
            drop(finished_tx);
        })
        .expect("spawn audio engine thread");
    EngineHandle { commands: cmd_tx, events: ev_rx, status, finished }
}

/// A track the current session's decoder has started, by session sequence number.
struct SessionTrack {
    src: TrackSource,
    /// The file's format.
    format: StreamFormat,
    /// What the device plays (after channel mapping and resampling).
    output: StreamFormat,
    /// At the output rate.
    total_frames: Option<u64>,
    /// The source's ReplayGain merged with the file's tags.
    replay_gain: ReplayGain,
    channels: ChannelChange,
}

impl SessionTrack {
    fn gain(&self, cfg: &GainConfig) -> Option<replaygain::AppliedGain> {
        applied_gain(cfg, &self.replay_gain, self.src.album_context)
    }
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
    /// Shared with the decode thread, which sets each new track's gain as it starts.
    gain: Arc<Mutex<GainConfig>>,
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
    /// The track the session started with: what ends if it never produces audio.
    opening: Option<TrackSource>,
    /// A seek the output is holding for, until the decoder reports it done.
    held_seek: Option<Duration>,
    device_modes: HashMap<String, PlaybackMode>,
    /// UID of the device in use, when the backend can tell.
    current_device: Option<String>,
    /// The last device list seen.
    known: Vec<DeviceInfo>,
    /// A device that disappeared while in use: (uid, name).
    lost: Option<(String, String)>,
    /// The session being started picks up a track that was already playing, so
    /// its start is announced as a restart rather than a new play.
    restarting: bool,
}

impl Engine {
    fn run(mut self, commands: Receiver<Command>) {
        let mut wait = TICK;
        loop {
            match commands.recv_timeout(wait) {
                Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(cmd) => {
                    self.handle(cmd);
                    wait = TICK;
                }
                Err(RecvTimeoutError::Timeout) => wait = next_wait(wait, self.status.state()),
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
            Command::Play(src) => {
                // A new queue: whatever the old one had lined up doesn't follow this track.
                self.forget_next();
                self.start(src, None);
            }
            Command::Pause => {
                // Loading too: a pause pressed while a slow server answers must
                // hold once the track opens (`open_ring` respects `paused`).
                if matches!(self.status.state(), PlayState::Loading | PlayState::Playing | PlayState::Buffering) {
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
            Command::Stop => {
                self.forget_next();
                self.stop();
            }
            Command::SetVolume(v) => {
                let was_full = self.status.volume() >= 1.0;
                self.status.set_volume(v);
                // Leaving or reaching full volume changes whether samples are requantized.
                if was_full != (self.status.volume() >= 1.0) {
                    self.report_signal_path();
                }
            }
            Command::SetGain(cfg) => {
                *self.gain.lock().unwrap_or_else(|e| e.into_inner()) = cfg;
                for seq in self.out_seq..self.tracks.len() as u64 {
                    self.store_gain(seq);
                }
                self.report_signal_path();
            }
            Command::SetMode(m) => self.apply_mode(m),
            Command::Seek(to) => self.seek(to),
            Command::SetNext(next) => self.set_next(next),
            Command::SelectDevice(uid) => match self.backend.select_device(uid.as_deref()) {
                Ok(()) => {
                    self.lost = None;
                    self.device_switched(false);
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
            Command::MatchRate(on) => {
                self.backend.set_match_rate(on);
                self.follow_rate();
                self.report_signal_path();
            }
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
        self.follow_rate();
        self.report_signal_path();
    }

    /// The rates the device offers can change with the mode (Normal can leave
    /// the device at its own rate): when the current track would now run at a
    /// different rate, start it over from where it is.
    fn follow_rate(&mut self) {
        let Some((file_rate, channels, rate)) =
            self.ring.as_ref().zip(self.current()).map(|(r, t)| (t.format.sample_rate, r.format.channels, r.format.sample_rate))
        else {
            return;
        };
        let caps = self.backend.device_caps();
        if choose_rate(file_rate, channels, caps.as_ref()) == rate && !self.backend.wants_reopen() {
            return;
        }
        if let Some(at) = self.take_position() {
            self.stop();
            self.resume(at);
        }
    }

    /// Starts a new session at `src`, optionally from `start_at`.
    fn start(&mut self, src: TrackSource, start_at: Option<Duration>) {
        self.stop();
        self.restarting = false;
        self.status.set_state(PlayState::Loading);
        self.status.paused.store(false, Ordering::Relaxed);
        let caps = self.backend.device_caps();
        self.opening = Some(src.clone());
        let session = Session::start(
            src,
            start_at,
            self.config.clone(),
            self.status.clone(),
            self.cache.clone(),
            caps,
            self.gain.clone(),
        );
        session.set_next(0, self.next.clone());
        self.session = Some(session);
    }

    fn stop(&mut self) {
        self.held_seek = None;
        self.opening = None;
        // From here on the old decode thread leaves `Status` alone (it may
        // still be winding down).
        if let Some(s) = self.session.take() {
            s.cancel();
        }
        // Synchronous: no callback runs after this. With the decoder silenced
        // too, the counters below stay reset.
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
        self.status.downloading.store(false, Ordering::Relaxed);
        self.status.buffering.store(false, Ordering::Relaxed);
        self.status.gains.reset();
    }

    /// Publishes track `seq`'s ReplayGain for the callback (1.0 when off).
    fn store_gain(&self, seq: u64) {
        let Some(t) = self.tracks.get(seq as usize) else { return };
        let cfg = *self.gain.lock().unwrap_or_else(|e| e.into_inner());
        self.status.gains.set(seq, t.gain(&cfg).map_or(1.0, |g| g.linear));
    }

    fn current(&self) -> Option<&SessionTrack> {
        self.tracks.get(self.out_seq as usize)
    }

    fn seek(&mut self, to: Duration) {
        let Some(session) = &self.session else { return };
        // The decoder can only reposition the track it's on. Once it has exited
        // (the queue's last track is fully decoded, or the next one failed) or
        // moved on to opening the next track, nothing would release a hold:
        // start the output's track over at the target instead.
        if session.finished() || session.decoding() != self.out_seq {
            if let Some(src) = self.current().map(|t| t.src.clone()) {
                self.restart_at(src, to);
            }
            return;
        }
        // Silence the output now; the decoder releases it once it has repositioned.
        let hold = self.ring.as_ref().map_or(0, |r| r.ctl.hold());
        if hold > 0 {
            self.status.buffering.store(true, Ordering::Relaxed);
            self.held_seek = Some(to);
        }
        session.seek(self.out_seq, to, hold);
    }

    fn set_next(&mut self, next: Option<TrackSource>) {
        if next.as_ref().map(|n| &n.id) != self.prefetched.as_ref() {
            self.prefetched = None;
        }
        let before = std::mem::replace(&mut self.next, next.clone());
        let Some(session) = &self.session else { return };
        let after = self.out_seq;
        let queued = self.tracks.get(after as usize + 1).map(|t| &t.src);
        // The decoder is still on the output's track (decoding it, or waiting at
        // its end): it takes this as what follows. (A decoder that has exited
        // takes nothing; `Ended` follows and the UI starts the next track itself.)
        if queued.is_none() && session.set_next(after, next.clone()) {
            return;
        }
        // It moved on: to the track it has reported, or, until that track's first
        // audio, to the one it was last told about.
        let queued = queued.or(before.as_ref());
        let same = |a: &TrackSource, b: &TrackSource| a.id == b.id && a.token == b.token;
        if queued.zip(next.as_ref()).is_some_and(|(q, n)| same(q, n)) {
            return;
        }
        // It moved on to a track that's no longer next (the queue changed in the
        // last few seconds of the track). Restart the current track where it is.
        if let Some(resume) = self.take_position() {
            self.resume(resume);
        }
    }

    /// The UI's next track no longer applies (a new queue, or stopped).
    fn forget_next(&mut self) {
        self.next = None;
        self.prefetched = None;
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

    /// Starts `src` over at `to`, staying paused if playback was paused.
    fn restart_at(&mut self, src: TrackSource, to: Duration) {
        let paused = self.status.state() == PlayState::Paused;
        let track_id = src.id.clone();
        self.resume((src, to, paused));
        // The new session's own report would arrive before it knows its track.
        let _ = self.events.send(Event::Seeked { track_id, position_secs: to.as_secs_f64() });
    }

    /// Restarts at a position from [`take_position`](Self::take_position), keeping pause.
    fn resume(&mut self, (src, at, paused): (TrackSource, Duration, bool)) {
        self.start(src, Some(at));
        self.restarting = true;
        if paused {
            self.status.paused.store(true, Ordering::Relaxed);
            self.status.set_state(PlayState::Paused);
        }
    }

    /// Playback moved to another device: pick its mode and carry on where we were.
    /// The mode is the one the user picked for that device; failing that, the
    /// device's default, or Normal for a `fallback` (the device in use went away).
    fn device_switched(&mut self, fallback: bool) {
        let resume = self.take_position();
        self.stop();
        self.known = self.backend.devices();
        let info = self.backend.current_device();
        self.current_device = info.as_ref().map(|d| d.uid.clone());
        let unpicked = |d: &DeviceInfo| if fallback { PlaybackMode::Normal } else { d.default_mode() };
        let mode = match &info {
            Some(d) => self.device_modes.get(&d.uid).copied().unwrap_or_else(|| unpicked(d)),
            None if fallback => PlaybackMode::Normal,
            None => self.config.mode,
        };
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
            self.device_switched(true);
        } else if events.contains(&DeviceEvent::DefaultChanged)
            // Where a new stream would go, not where the open one is (it stays put).
            && self.backend.target_device().map(|d| d.uid) != current
        {
            // Following the default and it moved (e.g. headphones plugged in).
            self.device_switched(false);
        } else {
            self.known = devices.clone();
            self.send_devices();
        }
        if let Some((uid, _)) = self.lost.clone()
            && let Some(back) = devices.iter().find(|d| d.uid == uid)
        {
            self.lost = None;
            // Nothing to offer when playback is there already: the default followed
            // it back, or it only dropped out for a moment (Bluetooth can, when the
            // mode changes).
            if self.current_device.as_ref() != Some(&uid) {
                let _ = self.events.send(Event::DeviceReturned { uid, name: back.name.clone() });
            }
        }
    }

    /// Describes the path from the current track to the device.
    fn report_signal_path(&mut self) {
        let (Some(ring), Some(track)) = (self.ring.as_ref(), self.current()) else { return };
        let (source, device_rate) = (track.format, ring.format.sample_rate);
        let channels = track.channels;
        let transcoded = track.src.transcode.as_ref().map(Transcode::label);
        let gain_cfg = *self.gain.lock().unwrap_or_else(|e| e.into_inner());
        let gain = match track.gain(&gain_cfg) {
            None => GainState::Off,
            Some(_) if self.config.mode == PlaybackMode::BitPerfect => GainState::Bypassed,
            Some(g) => GainState::Applied(g),
        };
        let caps = self.backend.device_caps();
        // Judged on what reaches the device channel-wise; the mapping itself is folded in below.
        let sent = StreamFormat { channels: ring.format.channels, ..source };
        let bit_perfect = adjust(
            assess(self.config.mode, &sent, device_rate, caps.as_ref()),
            channels,
            transcoded.as_deref(),
        );
        // The integer format actually in use: what the callback dithers to.
        let device_bits = caps.as_ref().and_then(|c| c.active_bits);
        let resampled = device_rate != source.sample_rate;
        // Software volume changes samples too, outside bit-perfect mode.
        let volume = self.config.mode == PlaybackMode::Normal && self.status.volume() < 1.0;
        // The same rule the callback applies to what it plays.
        let treatment = dither::Treatment {
            processed: resampled || matches!(channels, ChannelChange::Downmix { .. }),
            gained: matches!(gain, GainState::Applied(g) if g.linear != 1.0) || volume,
            source_bits: source.bits_per_sample,
        };
        let dithered = dither::needs_dither(device_bits, treatment);
        let path = SignalPath {
            output: self.backend.name().to_owned(),
            source,
            device_rate,
            device_bits,
            resampled,
            dithered,
            bit_perfect,
            channels,
            gain,
            transcoded,
        };
        let _ = self.events.send(Event::SignalPath(path));
    }

    /// Publishes download progress and starts the next track's prefetch when it's time.
    fn poll_fetch(&mut self) {
        let progress = match (&self.cache, self.current().map(|t| &t.src)) {
            (Some(cache), Some(src)) if !src.is_local() => cache.progress(&src.cache_key()),
            _ => None,
        };
        let downloading = self.network_in_use(progress.as_ref());
        self.status.downloading.store(downloading, Ordering::Relaxed);
        let Some(cache) = &self.cache else { return };
        let Some(src) = self.current().map(|t| &t.src) else { return };
        if src.is_local() {
            return;
        }
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

    /// The current track's stream (whose `progress` the caller already has) or
    /// the next one's prefetch is still downloading.
    fn network_in_use(&self, progress: Option<&streamcache::Progress>) -> bool {
        let Some(cache) = &self.cache else { return false };
        let incomplete = |src: &TrackSource| !src.is_local() && cache.progress(&src.cache_key()).is_none_or(|p| !p.complete);
        let current = self.session.is_some()
            && self.current().is_some_and(|t| !t.src.is_local() && progress.is_none_or(|p| !p.complete));
        let prefetch = self.prefetched.is_some() && self.next.as_ref().is_some_and(incomplete);
        current || prefetch
    }

    fn poll_session(&mut self) {
        // Read before draining messages: anything the thread sent before exiting is then seen below.
        let finished = self.session.as_ref().is_some_and(Session::finished);
        while let Some(msg) = self.session.as_ref().and_then(Session::try_msg) {
            match msg {
                DecoderMsg::Track { seq, src, format, output, total_frames, replay_gain, channels } => {
                    debug_assert_eq!(seq as usize, self.tracks.len());
                    self.tracks.push(SessionTrack { src, format, output, total_frames, replay_gain, channels });
                    // The decoder set it already; this catches a settings change it raced with.
                    self.store_gain(seq);
                }
                DecoderMsg::Ring { format, input, first_seq } => {
                    if !self.open_ring(format, input, first_seq) {
                        return;
                    }
                }
                DecoderMsg::Seeked { seq, position_secs } => {
                    self.held_seek = None;
                    if let Some(t) = self.tracks.get(seq as usize) {
                        let track_id = t.src.id.clone();
                        let _ = self.events.send(Event::Seeked { track_id, position_secs });
                    }
                }
                DecoderMsg::SeekMiss { seq, to } => {
                    if let Some(src) = self.tracks.get(seq as usize).map(|t| t.src.clone()) {
                        self.restart_at(src, to);
                    }
                    return;
                }
                DecoderMsg::Error { seq, track_id, message } => {
                    let _ = self.events.send(Event::Error { track_id: Some(track_id), message });
                    // A later track failing lets the current one play out; anything else stops.
                    if self.ring.is_none() || seq <= self.out_seq {
                        self.stop();
                        return;
                    }
                }
            }
        }

        // The decoder exited just as a seek went in (it had finished the last
        // track), so nothing will release the output: start over at the target.
        if finished
            && let Some(to) = self.held_seek.take()
        {
            if let Some(src) = self.current().map(|t| t.src.clone()) {
                self.restart_at(src, to);
            }
            return;
        }

        // Track boundaries the callback has played past. The decoder reports a
        // track before queuing its audio, but the callback can cross into it
        // between the messages being drained above and this load: such a track is
        // announced on the next poll, once its report is in.
        let crossed = self.status.track_seq.load(Ordering::Acquire);
        let mut advanced = false;
        while self.out_seq < crossed && (self.out_seq as usize + 1) < self.tracks.len() {
            self.out_seq += 1;
            advanced = true;
            // The output can't cross while it holds for a seek: one still on
            // record is stale (it ended its track), and must not restart this one.
            self.held_seek = None;
            // The UI names the new next once it hears about this track.
            self.forget_next();
            self.announce(true);
        }
        if advanced {
            // Same device format, but the gain, transcode or channel layout may differ.
            self.report_signal_path();
        }

        if self.session.is_some() && finished && (self.ring.is_none() || self.backend.drained()) {
            // A session that never produced audio (an empty file) ends too, as its first track.
            let ended = self.current().map(|t| &t.src).or(self.opening.as_ref()).map(|s| (s.id.clone(), s.token));
            self.stop();
            if let Some((track_id, token)) = ended {
                let _ = self.events.send(Event::Ended { track_id, token });
            }
        }
    }

    /// Connects a new ring to the device. Returns false if the session was stopped.
    fn open_ring(&mut self, format: StreamFormat, input: RingInput, first_seq: u64) -> bool {
        let ctl = input.ctl.clone();
        let reopened = self.ring.is_some();
        if reopened {
            // The decoder only sends a new ring once the old one is empty, but
            // the device may not have played the callback's last buffer yet:
            // closing now would cut the end of the old track. (Bounded: a pause
            // pressed just now keeps the callback from reporting it.)
            let deadline = Instant::now() + REOPEN_DRAIN_WAIT;
            while !self.backend.drained() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(1));
            }
        }
        // Close first so the old callback can't touch the counters set below.
        // (The device itself may stay claimed: it's reopened right away.)
        self.backend.close_for_reopen();
        self.out_seq = first_seq;
        self.status.track_seq.store(first_seq, Ordering::Release);
        if reopened {
            // The new ring starts on a new track. (A first ring keeps the
            // position a session started at, set by the decoder's seek.)
            self.status.frames_played.store(0, Ordering::Relaxed);
            // As at a gapless boundary: the track that was next is playing
            // now, and the UI names the new next once it hears about it.
            self.forget_next();
        }
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
        let (track_id, token, format, total, rate) =
            (t.src.id.clone(), t.src.token, t.format, t.total_frames, t.output.sample_rate);
        self.status.total_frames.store(total.unwrap_or(0), Ordering::Relaxed);
        self.status.sample_rate.store(rate, Ordering::Relaxed);
        // Only the first announcement of a restarted session is a restart, and only
        // if it's the track that was restarted (not the next one, when that had no audio left).
        let restarted = self.opening.as_ref().is_some_and(|o| o.id == t.src.id && o.token == t.src.token);
        let restart = !gapless && std::mem::take(&mut self.restarting) && restarted;
        let _ = self.events.send(Event::TrackStarted { track_id, token, format, gapless, restart });
    }
}

#[cfg(test)]
mod tests;
