//! Telling the system and the server what's playing.
//!
//! A small worker thread watches the engine (its status, plus a copy of its
//! events) and keeps two things up to date: the OS Now Playing info, so media
//! keys, Control Center and headphone buttons work, and the server's play
//! counts (`scrobble`, which Navidrome passes on to Last.fm / ListenBrainz).
//! It runs apart from the UI so neither depends on the window being drawn.
//!
//! Remote commands (media keys, Control Center) come back to the app as
//! [`Remote`]s, because acting on them needs the queue.
//!
//! One module per system, each with the same `install` / `publish` /
//! `set_artwork`: MediaPlayer on macOS, MPRIS on Linux and the BSDs, the System
//! Media Transport Controls on Windows.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as system;
#[cfg(all(unix, not(target_os = "macos")))]
mod mpris;
#[cfg(all(unix, not(target_os = "macos")))]
use mpris as system;
#[cfg(windows)]
mod smtc;
#[cfg(windows)]
use smtc as system;
pub mod scrobble;

use audio::{PlayState, Status};
use scrobble::{Listen, Outcome, PendingPlay, RetryQueue};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant, SystemTime};
use subsonic::Client;

/// A command from outside the app: media keys, Control Center, headphones.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Remote {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    /// Jump to this position, in seconds.
    Seek(f64),
    /// Skip this many seconds forward (negative: back).
    SkipBy(f64),
    /// Bring the window forward (clicking the player in the desktop's panel).
    Raise,
}

/// What the system shows for a track.
#[derive(Debug, Clone, PartialEq)]
pub struct Info {
    pub track_id: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_secs: Option<f64>,
    /// Cover art id, for the artwork.
    pub cover: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Playing,
    /// Playing, but waiting for audio: shown as playing with the clock stopped.
    Buffering,
    Paused,
    Stopped,
}

impl State {
    fn of(s: PlayState) -> Self {
        match s {
            PlayState::Playing => State::Playing,
            PlayState::Buffering | PlayState::Loading => State::Buffering,
            PlayState::Paused => State::Paused,
            PlayState::Stopped => State::Stopped,
        }
    }
}

pub struct Config {
    pub status: Arc<Status>,
    pub rt: tokio::runtime::Handle,
    /// The cover art cache, read for artwork (and filled when it's missing).
    pub art_dir: PathBuf,
    /// Where plays the server didn't take wait for a retry.
    pub retry_file: PathBuf,
    pub scrobble: bool,
    /// Called for each remote command (on the main thread on macOS).
    pub remote: Arc<dyn Fn(Remote) + Send + Sync>,
    /// The main window's HWND, which the media controls attach to.
    #[cfg(windows)]
    pub hwnd: Option<isize>,
}

enum In {
    Client(Client),
    Known(Info),
    Engine(audio::Event),
    Scrobbling(bool),
    /// A submitted play came back.
    Submitted { play: PendingPlay, outcome: Outcome, from_retry: bool },
}

/// Handle to the worker; cheap to clone. The worker stops when every handle is gone.
#[derive(Clone)]
pub struct Session {
    tx: Sender<In>,
}

impl Session {
    pub fn start(cfg: Config) -> Self {
        let (tx, rx) = channel();
        #[cfg(not(windows))]
        system::install(cfg.remote.clone());
        #[cfg(windows)]
        system::install(cfg.remote.clone(), cfg.hwnd);
        let worker = Worker::new(cfg, tx.clone());
        std::thread::Builder::new().name("now-playing".into()).spawn(move || worker.run(rx)).expect("spawn now-playing worker");
        Self { tx }
    }

    pub fn set_client(&self, client: Client) {
        let _ = self.tx.send(In::Client(client));
    }

    /// Describes a track the engine may start (the current one and the next).
    pub fn known(&self, info: Info) {
        let _ = self.tx.send(In::Known(info));
    }

    /// A copy of an engine event; called from the event forwarder.
    pub fn engine_event(&self, ev: &audio::Event) {
        if matches!(ev, audio::Event::TrackStarted { .. } | audio::Event::Seeked { .. } | audio::Event::Ended { .. }) {
            let _ = self.tx.send(In::Engine(ev.clone()));
        }
    }

    pub fn set_scrobbling(&self, on: bool) {
        let _ = self.tx.send(In::Scrobbling(on));
    }
}

/// When to tell the system where playback is. macOS extrapolates the clock
/// from the rate, so this only fires on changes and on drift (seeks, stalls).
#[derive(Debug, Default)]
pub struct Publisher {
    last: Option<(String, State, f64, f64)>,
}

/// How far the real position may drift from the extrapolated one before republishing.
const DRIFT_SECS: f64 = 1.5;

impl Publisher {
    /// True when (track, state, position) should be published; records it if so.
    pub fn should(&mut self, track: &str, state: State, pos: f64, now: f64) -> bool {
        let publish = match &self.last {
            None => true,
            Some((t, s, p, at)) => {
                let expected = if *s == State::Playing { p + (now - at) } else { *p };
                t != track || *s != state || (pos - expected).abs() > DRIFT_SECS
            }
        };
        if publish {
            self.last = Some((track.to_owned(), state, pos, now));
        }
        publish
    }

    pub fn reset(&mut self) {
        self.last = None;
    }
}

struct Worker {
    cfg: Config,
    tx: Sender<In>,
    client: Option<Client>,
    scrobble: bool,
    /// Metadata for recently named tracks, by id.
    known: HashMap<String, Info>,
    current: Option<Info>,
    listen: Option<Listen>,
    publisher: Publisher,
    cleared: bool,
    retry: RetryQueue,
    /// A retry is in flight.
    flushing: bool,
    next_flush: Instant,
    clock: Instant,
}

/// How often to look at the engine while something is current.
const TICK: Duration = Duration::from_millis(500);
/// How often to look when nothing plays (retries only).
const IDLE_TICK: Duration = Duration::from_secs(5);
const RETRY_EVERY: Duration = Duration::from_secs(300);
/// Metadata kept for this many tracks.
const KNOWN_MAX: usize = 16;

impl Worker {
    fn new(cfg: Config, tx: Sender<In>) -> Self {
        let retry = RetryQueue::load(&cfg.retry_file);
        Self {
            scrobble: cfg.scrobble,
            cfg,
            tx,
            client: None,
            known: HashMap::new(),
            current: None,
            listen: None,
            publisher: Publisher::default(),
            cleared: true,
            retry,
            flushing: false,
            next_flush: Instant::now(),
            clock: Instant::now(),
        }
    }

    fn run(mut self, rx: Receiver<In>) {
        loop {
            let tick = if self.current.is_some() { TICK } else { IDLE_TICK };
            match rx.recv_timeout(tick) {
                Ok(msg) => self.handle(msg),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.poll();
            if !self.flushing && Instant::now() >= self.next_flush {
                self.flush_retries();
            }
        }
        publish(None, State::Stopped, 0.0);
    }

    fn now(&self) -> f64 {
        self.clock.elapsed().as_secs_f64()
    }

    fn handle(&mut self, msg: In) {
        match msg {
            In::Client(c) => {
                self.client = Some(c);
                self.next_flush = Instant::now();
            }
            In::Known(info) => {
                if self.known.len() >= KNOWN_MAX && !self.known.contains_key(&info.track_id) {
                    let keep = self.current.as_ref().map(|c| c.track_id.clone());
                    self.known.retain(|id, _| Some(id) == keep.as_ref());
                }
                // A late description of the current track (e.g. the album name arrived).
                if self.current.as_ref().is_some_and(|c| c.track_id == info.track_id) {
                    self.current = Some(info.clone());
                    self.publisher.reset();
                }
                self.known.insert(info.track_id.clone(), info);
            }
            In::Scrobbling(on) => self.scrobble = on,
            In::Engine(ev) => self.engine(ev),
            In::Submitted { play, outcome, from_retry } => self.submitted(play, outcome, from_retry),
        }
    }

    fn engine(&mut self, ev: audio::Event) {
        let now = self.now();
        match ev {
            audio::Event::TrackStarted { track_id, restart, .. } => {
                // The engine started the same track over (device switch, a seek
                // it couldn't serve in place): the same listen carries on.
                if restart && self.listen.as_ref().is_some_and(|l| l.track_id == track_id) {
                    self.publisher.reset();
                    return;
                }
                let info = self.known.get(&track_id).cloned();
                let started_ms = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
                self.listen = Some(Listen::new(track_id.clone(), started_ms, info.as_ref().and_then(|i| i.duration_secs)));
                self.publisher.reset();
                if self.scrobble
                    && let Some(client) = self.client.clone()
                {
                    // "Now playing" is best effort; it's stale soon anyway.
                    self.cfg.rt.spawn(async move {
                        if let Err(e) = client.scrobble(&track_id, started_ms, false).await {
                            tracing::debug!("now playing: {e}");
                        }
                    });
                }
                if let Some(info) = &info {
                    self.fetch_artwork(info);
                }
                self.current = info;
            }
            audio::Event::Seeked { track_id, position_secs } => {
                if let Some(l) = self.listen.as_mut().filter(|l| l.track_id == track_id) {
                    l.seeked(position_secs, now);
                }
            }
            audio::Event::Ended { .. } => {
                self.listen = None;
                self.current = None;
            }
            _ => {}
        }
    }

    fn poll(&mut self) {
        let status = self.cfg.status.clone();
        let state = State::of(status.state());
        let pos = status.position_secs();
        let now = self.now();
        if state == State::Stopped {
            self.current = None;
            self.listen = None;
        }
        if let Some(l) = self.listen.as_mut() {
            l.set_duration(status.duration_secs());
            l.observe(pos, state == State::Playing, now);
            if l.take_due() {
                let play = PendingPlay { id: l.track_id.clone(), time_ms: l.started_ms };
                self.submit(play);
            }
        }
        match &self.current {
            Some(info) => {
                if self.publisher.should(&info.track_id, state, pos, now) {
                    let mut info = info.clone();
                    if info.duration_secs.is_none() {
                        info.duration_secs = status.duration_secs();
                    }
                    publish(Some(info), state, pos);
                    self.cleared = false;
                }
            }
            None if !self.cleared => {
                publish(None, State::Stopped, 0.0);
                self.publisher.reset();
                self.cleared = true;
            }
            None => {}
        }
    }

    fn submit(&mut self, play: PendingPlay) {
        if !self.scrobble {
            return;
        }
        // Keep plays in order: behind any that are still waiting.
        if !self.retry.is_empty() || self.client.is_none() {
            self.retry.push(play);
            self.flush_retries();
            return;
        }
        self.send(play, false);
    }

    fn send(&mut self, play: PendingPlay, from_retry: bool) {
        let Some(client) = self.client.clone() else { return };
        let tx = self.tx.clone();
        self.cfg.rt.spawn(async move {
            let result = client.scrobble(&play.id, play.time_ms, true).await;
            if let Err(e) = &result {
                tracing::info!("scrobble {}: {e}", play.id);
            }
            let _ = tx.send(In::Submitted { play, outcome: Outcome::of(&result), from_retry });
        });
    }

    fn submitted(&mut self, play: PendingPlay, outcome: Outcome, from_retry: bool) {
        if from_retry {
            self.flushing = false;
        }
        match outcome {
            Outcome::Done | Outcome::Refused => {
                if from_retry {
                    self.retry.pop_front();
                    // The server is answering: carry on with the rest.
                    self.flush_retries();
                }
            }
            Outcome::Unreachable => {
                if !from_retry {
                    self.retry.push(play);
                }
                self.next_flush = Instant::now() + RETRY_EVERY;
            }
        }
    }

    /// Sends the oldest waiting play; each answer sends the next.
    fn flush_retries(&mut self) {
        if self.flushing || self.client.is_none() {
            return;
        }
        let Some(play) = self.retry.front().cloned() else {
            self.next_flush = Instant::now() + RETRY_EVERY;
            return;
        };
        self.flushing = true;
        self.send(play, true);
    }

    /// Hands the cover to the system: from the art cache if it's there, else from
    /// the server (and into the cache for the UI).
    fn fetch_artwork(&self, info: &Info) {
        let Some(cover) = info.cover.clone() else { return };
        let dir = self.cfg.art_dir.clone();
        let client = self.client.clone();
        let track_id = info.track_id.clone();
        self.cfg.rt.spawn(async move {
            for size in [512, 256, 128] {
                if let Ok(bytes) = tokio::fs::read(crate::art::cover_file(&dir, &cover, size)).await {
                    return set_artwork(track_id, bytes);
                }
            }
            let Some(client) = client else { return };
            if let Some(bytes) = crate::art::fetch_cached(&client, &dir, &cover, 512).await {
                set_artwork(track_id, bytes);
            }
        });
    }
}

use system::{publish, set_artwork};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishes_on_changes_and_drift_only() {
        let mut p = Publisher::default();
        assert!(p.should("a", State::Playing, 0.0, 0.0));
        // The clock runs on by itself while playing.
        assert!(!p.should("a", State::Playing, 10.0, 10.0));
        assert!(!p.should("a", State::Playing, 11.2, 10.5));
        // A seek (or a stall) shows up as drift.
        assert!(p.should("a", State::Playing, 60.0, 11.0));
        assert!(p.should("a", State::Paused, 61.0, 12.0));
        // Paused: the clock stands still.
        assert!(!p.should("a", State::Paused, 61.0, 30.0));
        assert!(p.should("a", State::Paused, 20.0, 31.0), "a seek while paused");
        assert!(p.should("a", State::Buffering, 20.0, 31.5));
        assert!(p.should("a", State::Playing, 20.0, 32.0));
        assert!(p.should("b", State::Playing, 0.0, 33.0), "a new track");
        p.reset();
        assert!(p.should("b", State::Playing, 1.0, 34.0));
    }

    #[test]
    fn an_engine_restart_keeps_the_listen() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config {
            status: Arc::default(),
            rt: rt.handle().clone(),
            art_dir: dir.path().join("art"),
            retry_file: dir.path().join("scrobbles.json"),
            scrobble: false,
            remote: Arc::new(|_| {}),
        };
        let (tx, _rx) = channel();
        let mut w = Worker::new(cfg, tx);
        let format = audio::StreamFormat { sample_rate: 44_100, channels: 2, bits_per_sample: Some(16) };
        let started = |restart| audio::Event::TrackStarted { track_id: "a".into(), token: 1, format, gapless: false, restart };
        w.engine(started(false));
        let first = w.listen.as_ref().unwrap().started_ms;
        std::thread::sleep(Duration::from_millis(5));
        w.engine(started(true));
        assert_eq!(w.listen.as_ref().unwrap().started_ms, first, "same listen after a restart");
        w.engine(started(false));
        assert_ne!(w.listen.as_ref().unwrap().started_ms, first, "a real new start is a new listen");
    }
}
