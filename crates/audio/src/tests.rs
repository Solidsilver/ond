//! Engine-level tests with a fake device that runs the real callback on a thread.

use super::*;
use crate::decode::tests::{file_source, pcm, slack, wait_late};
use crate::dither::Tpdf;
use std::sync::Mutex;
use std::time::Instant;

/// Plays rings at roughly 8× real time and records what it played.
struct FakeBackend {
    caps: DeviceCaps,
    played: Arc<Mutex<Vec<f32>>>,
    /// Positions after each callback, including boundaries inside a buffer.
    positions: Arc<Mutex<Vec<(u64, f64, f64)>>>,
    landmarks: Arc<Mutex<Vec<(f64, f32)>>>,
    stop: Option<(Arc<AtomicBool>, thread::JoinHandle<()>)>,
    drained: Arc<AtomicBool>,
    opens: Arc<AtomicU64>,
    /// Calls to `close`, and to `close_for_reopen`.
    closes: Arc<AtomicU64>,
    reopen_closes: Arc<AtomicU64>,
    /// Like CoreAudio, bit-perfect mode follows the song's rate whatever `own_rate` says.
    bit_perfect: bool,
    /// Devices "plugged in"; the first is the system default.
    devices: Arc<Mutex<Vec<DeviceInfo>>>,
    device_events: Arc<Mutex<Vec<DeviceEvent>>>,
    selected: Option<String>,
    /// The device the open stream plays on: like CoreAudio, a stream stays on
    /// its device when the default moves.
    open_dev: Option<String>,
    /// How long the device takes to play one buffer, and its size in frames.
    period: (Duration, usize),
    /// Like CoreAudio not matching the song's rate: only this rate is on offer.
    own_rate: Option<u32>,
    /// Set when the backend is dropped (the engine let go of the device).
    dropped: Arc<AtomicBool>,
    /// The device's own volume, reported while a stream is open (as a held device's is).
    volume: Option<DeviceVolume>,
    /// While set, the device stops asking for audio (a stuck or starved
    /// device): nothing is played and no flush is serviced.
    stalled: Arc<AtomicBool>,
    /// The open ring's controls, as the device sees them.
    ring_ctl: Arc<Mutex<Option<Arc<RingCtl>>>>,
}

impl Drop for FakeBackend {
    fn drop(&mut self) {
        self.close();
        self.dropped.store(true, Ordering::Release);
    }
}

impl FakeBackend {
    fn new(caps: DeviceCaps) -> Self {
        Self {
            caps,
            played: Arc::default(),
            positions: Arc::default(),
            landmarks: Arc::default(),
            stop: None,
            drained: Arc::default(),
            opens: Arc::default(),
            closes: Arc::default(),
            reopen_closes: Arc::default(),
            bit_perfect: false,
            devices: Arc::default(),
            device_events: Arc::default(),
            selected: None,
            open_dev: None,
            period: (Duration::from_millis(1), 512),
            own_rate: None,
            dropped: Arc::default(),
            volume: None,
            stalled: Arc::default(),
            ring_ctl: Arc::default(),
        }
    }

    fn stop_stream(&mut self) {
        self.open_dev = None;
        if let Some((stop, handle)) = self.stop.take() {
            stop.store(true, Ordering::Relaxed);
            let _ = handle.join();
        }
    }

    /// The device a new stream would open on: the selected one if plugged in, else the default.
    fn target(&self) -> Option<DeviceInfo> {
        let devices = self.devices.lock().unwrap();
        let chosen = self.selected.as_ref().and_then(|uid| devices.iter().find(|d| &d.uid == uid));
        chosen.or(devices.first()).cloned()
    }
}

impl OutputBackend for FakeBackend {
    fn name(&self) -> &str {
        "fake"
    }

    fn device_caps(&mut self) -> Option<DeviceCaps> {
        let mut caps = self.caps.clone();
        if let Some(rate) = self.own_rate.filter(|_| !self.bit_perfect) {
            caps.configs = caps.configs.iter().map(|c| RateRange { min: rate, max: rate, ..*c }).collect();
        }
        Some(caps)
    }

    fn set_mode(&mut self, mode: PlaybackMode) {
        self.bit_perfect = mode == PlaybackMode::BitPerfect;
    }

    fn set_match_rate(&mut self, on: bool) {
        self.own_rate = (!on).then_some(48_000);
    }

    fn devices(&mut self) -> Vec<DeviceInfo> {
        self.devices.lock().unwrap().clone()
    }

    fn current_device(&mut self) -> Option<DeviceInfo> {
        let open = self.open_dev.as_ref().and_then(|uid| self.devices.lock().unwrap().iter().find(|d| &d.uid == uid).cloned());
        open.or_else(|| self.target())
    }

    fn target_device(&mut self) -> Option<DeviceInfo> {
        self.target()
    }

    fn select_device(&mut self, uid: Option<&str>) -> Result<(), OutputError> {
        self.selected = uid.map(str::to_owned);
        Ok(())
    }

    fn poll_device_events(&mut self) -> Vec<DeviceEvent> {
        std::mem::take(&mut *self.device_events.lock().unwrap())
    }

    fn device_volume(&mut self) -> Option<DeviceVolume> {
        self.volume.filter(|_| self.stop.is_some())
    }

    fn set_device_volume(&mut self, db: f32) {
        if let Some(v) = self.volume.as_mut().filter(|_| self.stop.is_some()) {
            v.db = db.clamp(v.min_db, v.max_db);
        }
    }

    fn set_device_mute(&mut self, muted: bool) {
        if let Some(v) = self.volume.as_mut().filter(|_| self.stop.is_some()) {
            v.muted = Some(muted);
        }
    }

    fn open(&mut self, format: StreamFormat, mut input: RingInput, status: Arc<Status>) -> Result<(), OutputError> {
        self.stop_stream();
        if !self.caps.supports(format.sample_rate, format.channels) {
            return Err(OutputError::Unsupported { rate: format.sample_rate, channels: format.channels });
        }
        self.opens.fetch_add(1, Ordering::Relaxed);
        self.open_dev = self.target().map(|d| d.uid);
        *self.ring_ctl.lock().unwrap() = Some(input.ctl.clone());
        let stalled = self.stalled.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let drained = Arc::new(AtomicBool::new(false));
        self.drained = drained.clone();
        let (played, stop2) = (self.played.clone(), stop.clone());
        let positions = self.positions.clone();
        let landmarks = self.landmarks.clone();
        let channels = format.channels as usize;
        let (period, frames) = self.period;
        // An integer device gets dither where needed, as through CoreAudio.
        let device_bits = self.caps.active_bits;
        let handle = thread::spawn(move || {
            let mut buf = vec![0.0f32; frames * channels];
            let mut dither = Tpdf::default();
            let mut rendered = false;
            loop {
                // Like a real device, it plays a buffer during the period after
                // the callback filled it: stopped meanwhile, that buffer is lost.
                thread::sleep(period);
                if stop2.load(Ordering::Relaxed) {
                    break;
                }
                if rendered {
                    played.lock().unwrap().extend(buf.iter().filter(|s| **s != 0.0));
                }
                rendered = false;
                if stalled.load(Ordering::Relaxed) {
                    continue;
                }
                if input.fill_for_device(&mut buf, channels, format.sample_rate, &status, device_bits, &mut dither) {
                    drained.store(true, Ordering::Release);
                }
                positions.lock().unwrap().push((status.track_seq.load(Ordering::Acquire), status.position_secs(), status.source_position_secs()));
                landmarks.lock().unwrap().push((status.source_position_secs(), buf[buf.len() - channels]));
                rendered = true;
            }
        });
        self.stop = Some((stop, handle));
        Ok(())
    }

    fn drained(&self) -> bool {
        self.drained.load(Ordering::Acquire)
    }

    fn close(&mut self) {
        self.closes.fetch_add(1, Ordering::Relaxed);
        self.stop_stream();
    }

    fn close_for_reopen(&mut self) {
        self.reopen_closes.fetch_add(1, Ordering::Relaxed);
        self.stop_stream();
    }
}

fn caps(max_bits: Option<u32>, max_rate: u32) -> DeviceCaps {
    DeviceCaps {
        name: "fake".into(),
        configs: vec![RateRange { channels: 2, min: 8_000, max: max_rate }],
        max_bits,
        exclusive: false,
        shared_reason: Some("shared mode (fake)".into()),
        integer: true,
        ..Default::default()
    }
}

/// Waits until the engine's decoder is on track `seq` of its session or past it.
fn wait_decoding(engine: &EngineHandle, seq: u64) {
    let deadline = Instant::now() + slack(Duration::from_secs(10));
    loop {
        let (tx, rx) = crossbeam_channel::bounded(1);
        engine.send(Command::Decoding(tx));
        let at = rx.recv_timeout(slack(Duration::from_secs(5))).expect("the engine answers");
        if at.is_some_and(|at| at >= seq) {
            return;
        }
        assert!(Instant::now() < deadline, "the decoder never reached track {seq} (on {at:?})");
        thread::sleep(Duration::from_millis(2));
    }
}

fn events_until(engine: &EngineHandle, mut stop: impl FnMut(&Event) -> bool) -> Vec<Event> {
    let deadline = Instant::now() + slack(Duration::from_secs(10));
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let ev = engine.events.recv_timeout(left).unwrap_or_else(|_| panic!("timed out; saw {seen:?}"));
        let done = stop(&ev);
        seen.push(ev);
        if done {
            return seen;
        }
    }
}

#[test]
fn pause_while_loading_holds_once_the_track_opens() {
    let dir = tempfile::tempdir().unwrap();
    let a = pcm(2 * 44_100 * 2, 0);
    let config = EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.1 };
    let engine = spawn(config, Box::new(FakeBackend::new(caps(Some(24), 96_000))), None);
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    // Sent right behind Play: the engine sees it while still Loading.
    engine.send(Command::Pause);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(engine.status.state(), PlayState::Paused);
    assert_eq!(engine.status.frames_played.load(Ordering::Relaxed), 0, "nothing played while paused");
    engine.send(Command::Resume);
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    assert!(!seen.iter().any(|e| matches!(e, Event::Error { .. })));
    engine.send(Command::Shutdown);
}

#[test]
fn resume_while_still_loading_stays_loading_until_the_track_opens() {
    let dir = tempfile::tempdir().unwrap();
    let config = EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.1 };
    let engine = spawn(config, Box::new(FakeBackend::new(caps(Some(24), 96_000))), None);
    // The decoder stalls before its first audio, so nothing is open yet.
    engine.send(Command::Play(file_source(&dir, "late-resume", &pcm(2 * 44_100 * 2, 0))));
    engine.send(Command::Pause);
    wait_late("late-resume", false);
    engine.send(Command::Resume);
    // The engine handles commands in order: once it answers the next one, Resume is done.
    engine.send(Command::RefreshDevices);
    events_until(&engine, |e| matches!(e, Event::Devices { .. }));
    assert_eq!(engine.status.state(), PlayState::Loading, "nothing is playing yet");
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    assert!(matches!(engine.status.state(), PlayState::Playing | PlayState::Buffering), "then it plays");
    events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    engine.send(Command::Shutdown);
}

#[test]
fn a_decoder_panic_is_an_error_not_a_hang() {
    let config = EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.1 };
    let engine = spawn(config, Box::new(FakeBackend::new(caps(Some(24), 96_000))), None);
    let src = TrackSource { id: "boom".into(), url: "test://panic".into(), ..Default::default() };
    engine.send(Command::Play(src));
    let seen = events_until(&engine, |e| matches!(e, Event::Error { .. }));
    let Some(Event::Error { message, .. }) = seen.last() else { unreachable!() };
    assert!(message.contains("a decoder blew up"), "{message}");
    // Stopped, not stuck loading or buffering.
    let deadline = Instant::now() + slack(Duration::from_secs(2));
    while engine.status.state() != PlayState::Stopped {
        assert!(Instant::now() < deadline, "still {:?}", engine.status.state());
        std::thread::sleep(Duration::from_millis(10));
    }
    engine.send(Command::Shutdown);
}

#[test]
fn gapless_queue_reports_each_track_as_it_plays() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (pcm(2 * 20_001, 1), pcm(2 * 9_999, 2));
    let backend = FakeBackend::new(caps(None, 192_000));
    let (played, opens) = (backend.played.clone(), backend.opens.clone());
    let config = EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs: 0.1 };
    let engine = spawn(config, Box::new(backend), None);
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    engine.send(Command::SetNext(Some(file_source(&dir, "b", &b))));

    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    let started: Vec<_> = seen
        .iter()
        .filter_map(|e| match e {
            Event::TrackStarted { track_id, gapless, .. } => Some((track_id.as_str(), *gapless)),
            _ => None,
        })
        .collect();
    assert_eq!(started, [("a", false), ("b", true)]);
    assert!(matches!(seen.last(), Some(Event::Ended { track_id, .. }) if track_id == "b"));
    assert_eq!(opens.load(Ordering::Relaxed), 1, "the device stayed open across the boundary");

    let out: Vec<i16> = played.lock().unwrap().iter().map(|s| (s * 32_768.0) as i16).collect();
    let expected: Vec<i16> = a.iter().chain(&b).copied().collect();
    assert!(out == expected, "played {} samples, expected {}", out.len(), expected.len());
    engine.send(Command::Shutdown);
}

#[test]
fn signal_path_is_honest_and_follows_mode_switches_mid_track() {
    let dir = tempfile::tempdir().unwrap();
    let a = pcm(2 * 44_100 * 2, 0);
    let config = EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.1 };
    // A 16/48 device: this 16/44.1 file can be bit-perfect, but only "requested" in shared mode.
    let engine = spawn(config, Box::new(FakeBackend::new(caps(Some(16), 48_000))), None);
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    let seen = events_until(&engine, |e| matches!(e, Event::SignalPath(_)));
    assert!(matches!(seen.last(), Some(Event::SignalPath(p)) if p.bit_perfect == BitPerfectStatus::Off));

    engine.send(Command::SetMode(PlaybackMode::BitPerfect));
    let seen = events_until(&engine, |e| matches!(e, Event::SignalPath(_)));
    let Some(Event::SignalPath(path)) = seen.last() else { unreachable!() };
    assert_eq!(path.bit_perfect, BitPerfectStatus::Requested { reason: "shared mode (fake)".into() });
    // Same track, still playing: nothing restarted.
    assert!(!seen.iter().any(|e| matches!(e, Event::TrackStarted { .. })));
    assert!(engine.status.bit_perfect.load(Ordering::Relaxed));
    engine.send(Command::Shutdown);
}

#[test]
fn leaving_the_device_at_its_rate_restarts_the_track_resampled_and_back() {
    let dir = tempfile::tempdir().unwrap();
    let a = pcm(4 * 44_100 * 2, 0);
    let config = EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.1 };
    let engine = spawn(config, Box::new(FakeBackend::new(caps(Some(24), 96_000))), None);
    engine.send(Command::MatchRate(true));
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    let rate = |seen: &[Event]| {
        seen.iter().rev().find_map(|e| match e {
            Event::SignalPath(p) => Some((p.device_rate, p.resampled)),
            _ => None,
        })
    };
    let seen = events_until(&engine, |e| matches!(e, Event::SignalPath(_)));
    assert_eq!(rate(&seen), Some((44_100, false)), "matching: the device runs at the song's rate");

    engine.send(Command::MatchRate(false));
    let seen = events_until(&engine, |e| matches!(e, Event::SignalPath(p) if p.device_rate == 48_000));
    assert_eq!(rate(&seen), Some((48_000, true)), "left alone: resampled to the device's rate");
    assert!(seen.iter().any(|e| matches!(e, Event::TrackStarted { restart: true, .. })), "the same track carries on");

    engine.send(Command::MatchRate(true));
    let seen = events_until(&engine, |e| matches!(e, Event::SignalPath(p) if p.device_rate == 44_100));
    assert_eq!(rate(&seen), Some((44_100, false)));
    engine.send(Command::Shutdown);
}

#[test]
fn a_bit_perfect_toggle_restarts_the_track_without_letting_go_of_the_device() {
    let dir = tempfile::tempdir().unwrap();
    let a = pcm(4 * 44_100 * 2, 0);
    let backend = FakeBackend::new(caps(Some(24), 96_000));
    let (closes, reopen_closes) = (backend.closes.clone(), backend.reopen_closes.clone());
    let config = EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.1 };
    let engine = spawn(config, Box::new(backend), None);
    // Normal mode leaves the device at 48 kHz; this file is 44.1.
    engine.send(Command::MatchRate(false));
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    events_until(&engine, |e| matches!(e, Event::SignalPath(p) if p.device_rate == 48_000));
    let before = (closes.load(Ordering::Relaxed), reopen_closes.load(Ordering::Relaxed));

    engine.send(Command::SetMode(PlaybackMode::BitPerfect));
    let seen = events_until(&engine, |e| matches!(e, Event::SignalPath(p) if p.device_rate == 44_100));
    assert!(seen.iter().any(|e| matches!(e, Event::TrackStarted { restart: true, .. })), "the track starts over at its own rate");
    // A full close lets go of an exclusive device (CoreAudio's hog, ALSA's
    // claim) only for the reopen to take it straight back.
    assert_eq!(closes.load(Ordering::Relaxed), before.0, "the restart never lets go of the device");
    assert!(reopen_closes.load(Ordering::Relaxed) > before.1, "it closes for the reopen instead");

    engine.send(Command::Stop);
    let deadline = Instant::now() + slack(Duration::from_secs(5));
    while closes.load(Ordering::Relaxed) == before.0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(closes.load(Ordering::Relaxed) > before.0, "stopping does let go");
    engine.send(Command::Shutdown);
}

#[test]
fn shutdown_waits_for_the_output_to_let_go() {
    let backend = FakeBackend::new(caps(Some(16), 48_000));
    let dropped = backend.dropped.clone();
    let engine = spawn(EngineConfig::default(), Box::new(backend), None);
    engine.shutdown(slack(Duration::from_secs(5)));
    assert!(dropped.load(Ordering::Acquire), "the backend (and its device) is released before shutdown returns");
    // Again, after it's gone: returns at once.
    let t = Instant::now();
    engine.shutdown(Duration::from_secs(5));
    assert!(t.elapsed() < Duration::from_secs(1));
}

#[test]
fn hi_res_on_a_16_48_device_is_resampled_and_reported_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hr.wav");
    std::fs::write(&path, crate::decode::tests::wav(96_000, 2, &pcm(2 * 9_600, 0))).unwrap();
    let src = TrackSource { id: "hr".into(), url: format!("file://{}", path.display()), suffix: Some("wav".into()), ..Default::default() };
    let config = EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs: 0.1 };
    let device = DeviceCaps { active_bits: Some(16), ..caps(Some(16), 48_000) };
    let engine = spawn(config, Box::new(FakeBackend::new(device)), None);
    engine.send(Command::Play(src));
    // It plays (never refused) and says why it isn't bit-perfect.
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    let path = seen
        .iter()
        .find_map(|e| match e {
            Event::SignalPath(p) => Some(p.clone()),
            _ => None,
        })
        .expect("signal path");
    assert_eq!(path.bit_perfect, BitPerfectStatus::Unavailable { reason: "device max 16/48, file is 16/96".into(), device_limit: true });
    assert!(path.resampled && path.dithered);
    assert_eq!(path.device_rate, 48_000);
    assert_eq!(path.summary(), "16/96 → 16/48 · resampled, dithered · device limit");
    assert!(!seen.iter().any(|e| matches!(e, Event::Error { .. })));
    engine.send(Command::Shutdown);
}

#[test]
fn replay_gain_switches_at_the_gapless_boundary_and_is_reported() {
    use replaygain::db_to_linear;
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (pcm(2 * 20_001, 1), pcm(2 * 9_999, 2));
    let backend = FakeBackend::new(caps(None, 192_000));
    let played = backend.played.clone();
    let config = EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.1 };
    let engine = spawn(config, Box::new(backend), None);
    engine.send(Command::SetVolume(1.0));
    engine.send(Command::SetGain(GainConfig { mode: GainMode::Track, ..Default::default() }));
    let mut first = file_source(&dir, "a", &a);
    first.replay_gain = ReplayGain { track_gain: Some(-6.0), ..Default::default() };
    let mut second = file_source(&dir, "b", &b);
    second.replay_gain = ReplayGain { track_gain: Some(-12.0), ..Default::default() };
    engine.send(Command::Play(first));
    engine.send(Command::SetNext(Some(second)));

    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    let paths: Vec<_> = seen.iter().filter_map(|e| if let Event::SignalPath(p) = e { Some(p.clone()) } else { None }).collect();
    assert!(paths.iter().any(|p| p.summary() == "16/44.1 · RG −6.0 dB track"), "{paths:?}");
    assert!(paths.iter().any(|p| p.summary() == "16/44.1 · RG −12.0 dB track"), "{paths:?}");

    let out = played.lock().unwrap().clone();
    assert_eq!(out.len(), a.len() + b.len(), "every sample played once (none gained to silence)");
    let (ga, gb) = (db_to_linear(-6.0), db_to_linear(-12.0));
    let at = |s: i16, g: f32| s as f32 / 32_768.0 * g;
    // After the start-up fade, A sits at its gain up to its last sample...
    let tail = a.len() - 100..a.len();
    // (A dropout just before the boundary would fade A's tail too.)
    assert!(engine.status.underruns() > 0 || tail.clone().all(|i| (out[i] - at(a[i], ga)).abs() < 1e-7));
    // ...and B is at its own from its very first sample. The fake device reads
    // ~11× faster than real time, so a slow machine can run the ring dry, and
    // the fade out and back in around a dropout is intended. It can land
    // anywhere in B, so then: no sample of B is louder than B's gain (A's,
    // 6 dB louder, carried over would be), and nearly all of B is exactly at it.
    let b_out = |i: usize| out[a.len() + i];
    if engine.status.underruns() == 0 {
        assert!((0..b.len()).all(|i| (b_out(i) - at(b[i], gb)).abs() < 1e-7), "B's gain must start on its first sample");
    } else {
        assert!((0..b.len()).all(|i| b_out(i).abs() <= at(b[i], gb).abs() + 1e-7), "B never above its own gain (underruns: {})", engine.status.underruns());
        let exact = (0..b.len()).filter(|i| (b_out(*i) - at(b[*i], gb)).abs() < 1e-7).count();
        assert!(exact * 10 >= b.len() * 8, "B mostly at its gain: {exact} of {} (underruns: {})", b.len(), engine.status.underruns());
    }
    engine.send(Command::Shutdown);
}

#[test]
fn bit_perfect_bypasses_replay_gain_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let a = pcm(2 * 8_000, 4);
    let backend = FakeBackend::new(caps(None, 192_000));
    let played = backend.played.clone();
    let config = EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs: 0.1 };
    let engine = spawn(config, Box::new(backend), None);
    engine.send(Command::SetGain(GainConfig { mode: GainMode::Track, preamp_db: 3.0, ..Default::default() }));
    let mut src = file_source(&dir, "a", &a);
    src.replay_gain = ReplayGain { track_gain: Some(-9.0), ..Default::default() };
    engine.send(Command::Play(src));
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    let path = seen.iter().find_map(|e| if let Event::SignalPath(p) = e { Some(p.clone()) } else { None }).unwrap();
    assert_eq!(path.gain, GainState::Bypassed);
    let out: Vec<i16> = played.lock().unwrap().iter().map(|s| (s * 32_768.0) as i16).collect();
    assert!(out == a, "samples untouched");
    engine.send(Command::Shutdown);
}

#[test]
fn transcoded_and_original_copies_are_cached_apart() {
    let original = TrackSource { id: "t1".into(), suffix: Some("flac".into()), ..Default::default() };
    let aac256 = TrackSource { transcode: Some(Transcode { format: "aac".into(), kbps: 256 }), ..original.clone() };
    let aac128 = TrackSource { transcode: Some(Transcode { format: "aac".into(), kbps: 128 }), ..original.clone() };
    let keys = [original.cache_key(), aac256.cache_key(), aac128.cache_key()];
    assert_ne!(keys[0], keys[1]);
    assert_ne!(keys[1], keys[2]);
    assert_eq!(keys[1].suffix.as_deref(), Some("aac"));
    assert_eq!(aac256.transcode.unwrap().label(), "AAC 256");
}

fn device(uid: &str, name: &str, kind: DeviceKind) -> DeviceInfo {
    DeviceInfo { uid: uid.into(), name: name.into(), kind, caps: caps(Some(24), 192_000), is_default: false }
}

#[test]
fn a_lost_device_falls_back_in_normal_mode_and_its_return_is_offered() {
    let dir = tempfile::tempdir().unwrap();
    let a = pcm(2 * 44_100 * 20, 0);
    let backend = FakeBackend::new(caps(Some(24), 192_000));
    let (devices, events) = (backend.devices.clone(), backend.device_events.clone());
    *devices.lock().unwrap() = vec![device("spk", "Speakers", DeviceKind::BuiltInSpeakers), device("dac", "DAC", DeviceKind::Usb)];
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.1 }, Box::new(backend), None);

    // Picking the DAC applies the mode the user picked for it.
    engine.send(Command::DeviceModes(HashMap::from([("dac".to_owned(), PlaybackMode::BitPerfect)])));
    engine.send(Command::SelectDevice(Some("dac".into())));
    let seen = events_until(&engine, |e| matches!(e, Event::Devices { .. }));
    assert!(seen.iter().any(|e| matches!(e, Event::ModeChanged { mode: PlaybackMode::BitPerfect, device: Some(d) } if d == "dac")));

    engine.send(Command::Play(file_source(&dir, "a", &a)));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    // Well into the track before unplugging (wait on progress, not a fixed
    // time: CI runners are slower than a desk Mac).
    let deadline = Instant::now() + slack(Duration::from_secs(20));
    while engine.status.position_secs() < 1.0 {
        assert!(Instant::now() < deadline, "never got going");
        thread::sleep(Duration::from_millis(10));
    }

    // Unplugged mid-track: Normal mode on the default, same track, from about where it was.
    devices.lock().unwrap().retain(|d| d.uid != "dac");
    events.lock().unwrap().push(DeviceEvent::ListChanged);
    let seen = events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    assert!(seen.iter().any(|e| matches!(e, Event::DeviceLost { name } if name == "DAC")));
    assert!(seen.iter().any(|e| matches!(e, Event::ModeChanged { mode: PlaybackMode::Normal, device: Some(d) } if d == "spk")));
    assert!(matches!(seen.last(), Some(Event::TrackStarted { track_id, .. }) if track_id == "a"));
    assert!(engine.status.position_secs() > 0.5, "resumed mid-track, not from the start");
    assert_eq!(engine.status.mode(), PlaybackMode::Normal);

    // Plugged back in: offered, not switched to silently.
    devices.lock().unwrap().push(device("dac", "DAC", DeviceKind::Usb));
    events.lock().unwrap().push(DeviceEvent::ListChanged);
    let seen = events_until(&engine, |e| matches!(e, Event::DeviceReturned { .. }));
    assert!(matches!(seen.last(), Some(Event::DeviceReturned { uid, .. }) if uid == "dac"));
    assert_eq!(engine.status.mode(), PlaybackMode::Normal);
    engine.send(Command::Shutdown);
}

#[test]
fn the_device_volume_is_reported_while_a_stream_is_open_and_follows_what_is_set() {
    let dir = tempfile::tempdir().unwrap();
    let a = pcm(2 * 44_100 * 20, 0);
    let mut backend = FakeBackend::new(caps(Some(24), 96_000));
    backend.volume = Some(DeviceVolume { db: -20.0, min_db: -60.0, max_db: 0.0, muted: Some(false) });
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.1 }, Box::new(backend), None);
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    let seen = events_until(&engine, |e| matches!(e, Event::DeviceVolume(Some(_))));
    assert!(matches!(seen.last(), Some(Event::DeviceVolume(Some(v))) if v.db == -20.0), "{seen:?}");

    engine.send(Command::SetDeviceVolume(-12.5));
    let seen = events_until(&engine, |e| matches!(e, Event::DeviceVolume(_)));
    assert!(matches!(seen.last(), Some(Event::DeviceVolume(Some(v))) if v.db == -12.5 && v.muted == Some(false)), "{seen:?}");
    engine.send(Command::SetDeviceMute(true));
    let seen = events_until(&engine, |e| matches!(e, Event::DeviceVolume(_)));
    assert!(matches!(seen.last(), Some(Event::DeviceVolume(Some(v))) if v.db == -12.5 && v.muted == Some(true)), "{seen:?}");
    // Past the top: the device's range holds.
    engine.send(Command::SetDeviceVolume(6.0));
    let seen = events_until(&engine, |e| matches!(e, Event::DeviceVolume(_)));
    assert!(matches!(seen.last(), Some(Event::DeviceVolume(Some(v))) if v.db == 0.0), "{seen:?}");

    // Stopped, the device is let go: no volume to show.
    engine.send(Command::Stop);
    events_until(&engine, |e| matches!(e, Event::DeviceVolume(None)));
    engine.send(Command::Shutdown);
}

#[test]
fn a_chosen_device_that_is_not_plugged_in_is_offered_when_it_appears() {
    let dir = tempfile::tempdir().unwrap();
    let a = pcm(2 * 44_100 * 20, 0);
    let backend = FakeBackend::new(caps(Some(24), 192_000));
    let (devices, events) = (backend.devices.clone(), backend.device_events.clone());
    *devices.lock().unwrap() = vec![device("spk", "Speakers", DeviceKind::BuiltInSpeakers)];
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.1 }, Box::new(backend), None);
    // The saved DAC at startup, before it's plugged in: playback goes to the default meanwhile.
    engine.send(Command::SelectDevice(Some("dac".into())));
    let seen = events_until(&engine, |e| matches!(e, Event::Devices { .. }));
    assert!(matches!(seen.last(), Some(Event::Devices { current: Some(d), .. }) if d == "spk"), "{seen:?}");
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));

    devices.lock().unwrap().push(device("dac", "DAC", DeviceKind::Usb));
    events.lock().unwrap().push(DeviceEvent::ListChanged);
    let seen = events_until(&engine, |e| matches!(e, Event::DeviceReturned { .. }));
    assert!(matches!(seen.last(), Some(Event::DeviceReturned { uid, name }) if uid == "dac" && name == "DAC"), "{seen:?}");
    engine.send(Command::Shutdown);
}

#[test]
fn playback_follows_the_default_device_when_it_changes_mid_track() {
    let dir = tempfile::tempdir().unwrap();
    let a = pcm(2 * 44_100 * 20, 23);
    let backend = FakeBackend::new(caps(Some(24), 192_000));
    let (devices, events) = (backend.devices.clone(), backend.device_events.clone());
    *devices.lock().unwrap() = vec![device("spk", "Speakers", DeviceKind::BuiltInSpeakers), device("hp", "Headphones", DeviceKind::BuiltInHeadphones)];
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.1 }, Box::new(backend), None);
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    let deadline = Instant::now() + slack(Duration::from_secs(20));
    while engine.status.position_secs() < 1.0 {
        assert!(Instant::now() < deadline, "never got going");
        thread::sleep(Duration::from_millis(10));
    }
    // Headphones plugged in: they become the system default.
    devices.lock().unwrap().reverse();
    events.lock().unwrap().push(DeviceEvent::DefaultChanged);
    let seen = events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    assert!(seen.iter().any(|e| matches!(e, Event::Devices { current: Some(d), .. } if d == "hp")), "{seen:?}");
    assert!(matches!(seen.last(), Some(Event::TrackStarted { track_id, restart: true, .. }) if track_id == "a"));
    assert!(engine.status.position_secs() > 0.5, "carried on mid-track");
    engine.send(Command::Shutdown);
}

#[test]
fn a_lost_device_falls_back_in_its_remembered_mode() {
    let dir = tempfile::tempdir().unwrap();
    let a = pcm(2 * 44_100 * 20, 24);
    let backend = FakeBackend::new(caps(Some(24), 192_000));
    let (devices, events) = (backend.devices.clone(), backend.device_events.clone());
    *devices.lock().unwrap() = vec![device("tv", "TV", DeviceKind::Hdmi), device("dac", "DAC", DeviceKind::Usb)];
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.1 }, Box::new(backend), None);
    // The user plays the TV bit-perfect, the DAC in Normal.
    engine.send(Command::DeviceModes(HashMap::from([("tv".to_owned(), PlaybackMode::BitPerfect), ("dac".to_owned(), PlaybackMode::Normal)])));
    engine.send(Command::SelectDevice(Some("dac".into())));
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    assert_eq!(engine.status.mode(), PlaybackMode::Normal);
    devices.lock().unwrap().retain(|d| d.uid != "dac");
    events.lock().unwrap().push(DeviceEvent::ListChanged);
    let seen = events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    assert!(seen.iter().any(|e| matches!(e, Event::DeviceLost { name } if name == "DAC")));
    assert!(seen.iter().any(|e| matches!(e, Event::ModeChanged { mode: PlaybackMode::BitPerfect, device: Some(d) } if d == "tv")), "{seen:?}");
    assert_eq!(engine.status.mode(), PlaybackMode::BitPerfect);
    engine.send(Command::Shutdown);
}

#[test]
fn a_device_that_blinks_out_and_back_as_the_default_is_not_offered() {
    let dir = tempfile::tempdir().unwrap();
    let a = pcm(2 * 44_100 * 20, 25);
    let backend = FakeBackend::new(caps(Some(24), 192_000));
    let (devices, events) = (backend.devices.clone(), backend.device_events.clone());
    *devices.lock().unwrap() = vec![device("hp", "Headphones", DeviceKind::Bluetooth), device("spk", "Speakers", DeviceKind::BuiltInSpeakers)];
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.1 }, Box::new(backend), None);
    engine.send(Command::SelectDevice(Some("hp".into())));
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    // Gone for a moment (Bluetooth renegotiating), then back as the system default.
    devices.lock().unwrap().retain(|d| d.uid != "hp");
    events.lock().unwrap().push(DeviceEvent::ListChanged);
    events_until(&engine, |e| matches!(e, Event::Devices { current: Some(d), .. } if d == "spk"));
    devices.lock().unwrap().insert(0, device("hp", "Headphones", DeviceKind::Bluetooth));
    events.lock().unwrap().extend([DeviceEvent::ListChanged, DeviceEvent::DefaultChanged]);
    let mut seen = events_until(&engine, |e| matches!(e, Event::Devices { current: Some(d), .. } if d == "hp"));
    engine.send(Command::RefreshDevices);
    seen.extend(events_until(&engine, |e| matches!(e, Event::Devices { .. })));
    assert!(!seen.iter().any(|e| matches!(e, Event::DeviceReturned { .. })), "already playing there: {seen:?}");
    engine.send(Command::Shutdown);
}

#[test]
fn bit_perfect_is_refused_where_it_cant_mean_anything() {
    let backend = FakeBackend::new(caps(Some(24), 192_000));
    *backend.devices.lock().unwrap() = vec![device("spk", "Speakers", DeviceKind::BuiltInSpeakers), device("dac", "DAC", DeviceKind::Usb)];
    let engine = spawn(EngineConfig::default(), Box::new(backend), None);
    // Remembered from before (or set in managed.json): the speakers still play Normal.
    engine.send(Command::DeviceModes(HashMap::from([("spk".to_owned(), PlaybackMode::BitPerfect)])));
    engine.send(Command::SelectDevice(Some("spk".into())));
    events_until(&engine, |e| matches!(e, Event::Devices { .. }));
    assert_eq!(engine.status.mode(), PlaybackMode::Normal);
    engine.send(Command::SetMode(PlaybackMode::BitPerfect));
    engine.send(Command::RefreshDevices);
    events_until(&engine, |e| matches!(e, Event::Devices { .. }));
    assert_eq!(engine.status.mode(), PlaybackMode::Normal, "asked for directly, too");
    engine.send(Command::Shutdown);
}

#[test]
fn remembered_modes_win_over_device_defaults() {
    let backend = FakeBackend::new(caps(Some(24), 192_000));
    *backend.devices.lock().unwrap() = vec![device("tv", "TV", DeviceKind::Hdmi), device("dac", "DAC", DeviceKind::Usb)];
    let engine = spawn(EngineConfig::default(), Box::new(backend), None);
    engine.send(Command::DeviceModes(HashMap::from([("dac".to_owned(), PlaybackMode::Normal), ("tv".to_owned(), PlaybackMode::BitPerfect)])));
    engine.send(Command::SelectDevice(Some("tv".into())));
    let seen = events_until(&engine, |e| matches!(e, Event::Devices { .. }));
    assert!(seen.iter().any(|e| matches!(e, Event::ModeChanged { mode: PlaybackMode::BitPerfect, .. })));
    engine.send(Command::SelectDevice(Some("dac".into())));
    let seen = events_until(&engine, |e| matches!(e, Event::Devices { .. }));
    assert!(seen.iter().any(|e| matches!(e, Event::ModeChanged { mode: PlaybackMode::Normal, .. })));
    engine.send(Command::Shutdown);
}

#[test]
fn a_seek_after_the_decoder_has_finished_still_lands_and_ends() {
    let dir = tempfile::tempdir().unwrap();
    // 16 s, all of it fits the buffer: the decoder is done long before the output
    // (it waits at the end for a next track, so the seek lands in place).
    let a = pcm(2 * 44_100 * 16, 3);
    let backend = FakeBackend::new(caps(None, 192_000));
    let config = EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 30.0 };
    let engine = spawn(config, Box::new(backend), None);
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    let deadline = Instant::now() + slack(Duration::from_secs(5));
    while engine.status.state() == PlayState::Loading && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    thread::sleep(Duration::from_millis(150));
    engine.send(Command::Seek(Duration::from_secs(12)));
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    assert!(seen.iter().any(|e| matches!(e, Event::Seeked { .. })), "the seek landed: {seen:?}");
    engine.send(Command::Shutdown);
}

#[test]
fn a_seek_after_the_decoder_moved_on_keeps_pause() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (pcm(2 * 44_100 * 8, 4), pcm(2 * 44_100 * 8, 5));
    let backend = FakeBackend::new(caps(None, 192_000));
    let config = EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 30.0 };
    let engine = spawn(config, Box::new(backend), None);
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    engine.send(Command::SetNext(Some(file_source(&dir, "b", &b))));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    engine.send(Command::Pause);
    wait_decoding(&engine, 1);
    engine.send(Command::Seek(Duration::from_secs(2)));
    events_until(&engine, |e| matches!(e, Event::Seeked { .. }));
    thread::sleep(Duration::from_millis(50));
    assert_eq!(engine.status.state(), PlayState::Paused);
    engine.send(Command::Shutdown);
}

#[test]
fn the_same_track_twice_is_told_apart_and_restarts_are_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let a = pcm(2 * 44_100 * 4, 6);
    let backend = FakeBackend::new(caps(None, 192_000));
    let config = EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 30.0 };
    let engine = spawn(config, Box::new(backend), None);
    let first = TrackSource { token: 1, ..file_source(&dir, "a", &a) };
    let again = TrackSource { token: 2, ..first.clone() };
    engine.send(Command::Play(first));
    engine.send(Command::SetNext(Some(again)));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    // Once the decoder is on the second copy, this seek restarts the first.
    wait_decoding(&engine, 1);
    engine.send(Command::Seek(Duration::from_secs(1)));
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    let started: Vec<(u64, bool, bool)> = seen
        .iter()
        .filter_map(|e| match e {
            Event::TrackStarted { token, gapless, restart, .. } => Some((*token, *gapless, *restart)),
            _ => None,
        })
        .collect();
    // The restart is flagged; the second copy still follows gaplessly, as itself.
    assert_eq!(started, [(1, false, true), (2, true, false)]);
    assert!(matches!(seen.last(), Some(Event::Ended { token: 2, .. })), "{seen:?}");
    engine.send(Command::Shutdown);
}

#[test]
fn an_idle_engine_backs_off_and_a_busy_one_does_not() {
    use crate::{IDLE_TICK, TICK, next_wait};
    let mut wait = TICK;
    for _ in 0..10 {
        wait = next_wait(wait, PlayState::Stopped);
    }
    assert_eq!(wait, IDLE_TICK, "stopped: wakes at most four times a second");
    assert_eq!(next_wait(IDLE_TICK, PlayState::Paused), IDLE_TICK);
    for state in [PlayState::Playing, PlayState::Loading, PlayState::Buffering] {
        assert_eq!(next_wait(IDLE_TICK, state), TICK, "{state:?}: back to the fast tick");
    }
}

#[test]
fn turning_the_volume_down_is_reported_as_dithered() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cd.wav");
    std::fs::write(&path, crate::decode::tests::wav(48_000, 2, &pcm(2 * 48_000 * 2, 0))).unwrap();
    let src = TrackSource { id: "cd".into(), url: format!("file://{}", path.display()), suffix: Some("wav".into()), ..Default::default() };
    let config = EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.1 };
    let device = DeviceCaps { active_bits: Some(16), ..caps(Some(16), 48_000) };
    let engine = spawn(config, Box::new(FakeBackend::new(device)), None);
    engine.send(Command::Play(src));
    let dithered = |engine: &EngineHandle| {
        let seen = events_until(engine, |e| matches!(e, Event::SignalPath(_)));
        let Some(Event::SignalPath(p)) = seen.last() else { unreachable!() };
        p.dithered
    };
    assert!(!dithered(&engine), "16/48 at full volume reaches the 16-bit device untouched");
    engine.send(Command::SetVolume(0.5));
    assert!(dithered(&engine), "software volume requantizes");
    engine.send(Command::SetVolume(1.0));
    assert!(!dithered(&engine));
    engine.send(Command::Shutdown);
}

/// (track id, gapless, restart) of every `TrackStarted` in `seen`.
fn starts(seen: &[Event]) -> Vec<(String, bool, bool)> {
    seen.iter()
        .filter_map(|e| match e {
            Event::TrackStarted { track_id, gapless, restart, .. } => Some((track_id.clone(), *gapless, *restart)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_next_track_that_fails_to_open_lets_the_current_one_play_out() {
    let dir = tempfile::tempdir().unwrap();
    let a = pcm(2 * 44_100, 7);
    let backend = FakeBackend::new(caps(None, 192_000));
    let played = backend.played.clone();
    let engine = spawn(EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs: 30.0 }, Box::new(backend), None);
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    let missing = TrackSource { id: "gone".into(), url: format!("file://{}", dir.path().join("gone.wav").display()), ..Default::default() };
    engine.send(Command::SetNext(Some(missing)));
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    let errors: Vec<_> = seen.iter().filter_map(|e| if let Event::Error { track_id, .. } = e { Some(track_id.clone()) } else { None }).collect();
    assert_eq!(errors, [Some("gone".to_owned())], "the error names the track that failed: {seen:?}");
    assert!(matches!(seen.last(), Some(Event::Ended { track_id, .. }) if track_id == "a"), "{seen:?}");
    let out: Vec<i16> = played.lock().unwrap().iter().map(|s| (s * 32_768.0) as i16).collect();
    assert!(out == a, "the current track played out: {} of {} samples", out.len(), a.len());
    engine.send(Command::Shutdown);
}

#[test]
fn a_next_named_while_the_decoder_opens_the_old_one_is_honoured() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b, c) = (pcm(2 * 44_100 * 3, 8), pcm(2 * 44_100, 9), pcm(2 * 44_100, 10));
    let backend = FakeBackend::new(caps(None, 192_000));
    let engine = spawn(EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs: 30.0 }, Box::new(backend), None);
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    // B takes a while to open (a slow server): the decoder sits between A and B.
    let b = file_source(&dir, "b", &b);
    let slow_b = TrackSource { url: b.url.replacen("file://", "test-slow://", 1), ..b };
    engine.send(Command::SetNext(Some(slow_b)));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    // All of A is decoded within a few ms; then the decoder is opening B.
    wait_decoding(&engine, 1);
    engine.send(Command::SetNext(Some(file_source(&dir, "c", &c))));
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    let ids: Vec<_> = starts(&seen).into_iter().map(|(id, ..)| id).collect();
    assert!(!ids.iter().any(|id| id == "b"), "B was replaced: {seen:?}");
    assert!(matches!(seen.last(), Some(Event::Ended { track_id, .. }) if track_id == "c"), "C followed A: {seen:?}");
    engine.send(Command::Shutdown);
}

#[test]
fn a_track_without_audio_still_ends() {
    let dir = tempfile::tempdir().unwrap();
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.1 }, Box::new(FakeBackend::new(caps(None, 192_000))), None);
    engine.send(Command::Play(TrackSource { token: 3, ..file_source(&dir, "empty", &[]) }));
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. } | Event::Error { .. }));
    assert!(matches!(seen.last(), Some(Event::Ended { track_id, token: 3 }) if track_id == "empty"), "{seen:?}");
    engine.send(Command::Shutdown);
}

#[test]
fn play_does_not_inherit_the_previous_queues_next() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b, c) = (pcm(2 * 44_100 * 20, 11), pcm(2 * 4_410, 12), pcm(2 * 4_410, 13));
    let backend = FakeBackend::new(caps(None, 192_000));
    let engine = spawn(EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs: 0.1 }, Box::new(backend), None);
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    engine.send(Command::SetNext(Some(file_source(&dir, "b", &b))));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    // A new queue whose only track is C: the UI has no next to name.
    engine.send(Command::Stop);
    engine.send(Command::Play(file_source(&dir, "c", &c)));
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    assert_eq!(starts(&seen), [("c".to_owned(), false, false)], "{seen:?}");
    assert!(matches!(seen.last(), Some(Event::Ended { track_id, .. }) if track_id == "c"));
    engine.send(Command::Shutdown);
}

#[test]
fn a_short_track_waits_for_its_next_to_be_named() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b, c) = (pcm(2 * 44_100 * 2, 14), pcm(2 * 44_100 * 3, 15), pcm(2 * 4_410, 16));
    let backend = FakeBackend::new(caps(None, 192_000));
    let (played, opens) = (backend.played.clone(), backend.opens.clone());
    // B fits the buffer whole: the decoder is done with it long before it plays.
    let engine = spawn(EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs: 30.0 }, Box::new(backend), None);
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    engine.send(Command::SetNext(Some(file_source(&dir, "b", &b))));
    // Like the UI: the track after B is named once B starts.
    let seen = events_until(&engine, |e| matches!(e, Event::TrackStarted { track_id, .. } if track_id == "b"));
    engine.send(Command::SetNext(Some(file_source(&dir, "c", &c))));
    let rest = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    let all: Vec<Event> = seen.into_iter().chain(rest).collect();
    let started: Vec<_> = starts(&all).into_iter().map(|(id, gapless, _)| (id, gapless)).collect();
    assert_eq!(started, [("a".to_owned(), false), ("b".to_owned(), true), ("c".to_owned(), true)], "{all:?}");
    assert_eq!(opens.load(Ordering::Relaxed), 1);
    let out: Vec<i16> = played.lock().unwrap().iter().map(|s| (s * 32_768.0) as i16).collect();
    let expected: Vec<i16> = a.iter().chain(&b).chain(&c).copied().collect();
    assert!(out == expected, "played {} samples, expected {}", out.len(), expected.len());
    engine.send(Command::Shutdown);
}

#[test]
fn a_seek_past_the_end_moves_on_to_the_next_track_from_its_start() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (pcm(2 * 44_100 * 20, 17), pcm(2 * 44_100, 18));
    let backend = FakeBackend::new(caps(None, 192_000));
    let played = backend.played.clone();
    // A is longer than the buffer: the decoder is still on it when the seek comes.
    let engine = spawn(EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs: 2.0 }, Box::new(backend), None);
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    engine.send(Command::SetNext(Some(file_source(&dir, "b", &b))));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    engine.send(Command::Seek(Duration::from_secs(100)));
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    assert_eq!(starts(&seen), [("b".to_owned(), true, false)], "B started once, from its start: {seen:?}");
    let seeked: Vec<_> = seen.iter().filter(|e| matches!(e, Event::Seeked { .. })).collect();
    assert!(matches!(seeked[..], [Event::Seeked { track_id, .. }] if track_id == "a"), "one Seeked, for A: {seen:?}");
    assert!(matches!(seen.last(), Some(Event::Ended { track_id, .. }) if track_id == "b"));
    let out: Vec<i16> = played.lock().unwrap().iter().map(|s| (s * 32_768.0) as i16).collect();
    assert!(out.ends_with(&b), "all of B played, from its first sample");
    engine.send(Command::Shutdown);
}

#[test]
fn a_seek_past_the_end_after_the_decoder_moved_on_starts_the_next_track() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (pcm(2 * 44_100 * 3, 21), pcm(2 * 44_100, 22));
    let backend = FakeBackend::new(caps(None, 192_000));
    let played = backend.played.clone();
    let engine = spawn(EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs: 30.0 }, Box::new(backend), None);
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    engine.send(Command::SetNext(Some(file_source(&dir, "b", &b))));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    // Both fit the buffer: once the decoder is done with A and on B, the
    // seek restarts A past its end, which has no audio left: B takes over.
    wait_decoding(&engine, 1);
    engine.send(Command::Seek(Duration::from_secs(100)));
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. } | Event::Error { .. }));
    assert_eq!(starts(&seen), [("b".to_owned(), false, false)], "B starts as itself, not as a restart: {seen:?}");
    let seeked: Vec<_> = seen.iter().filter(|e| matches!(e, Event::Seeked { .. })).collect();
    assert!(matches!(seeked[..], [Event::Seeked { track_id, .. }] if track_id == "a"), "one Seeked, for A: {seen:?}");
    assert!(matches!(seen.last(), Some(Event::Ended { track_id, .. }) if track_id == "b"), "{seen:?}");
    let out: Vec<i16> = played.lock().unwrap().iter().map(|s| (s * 32_768.0) as i16).collect();
    assert!(out.ends_with(&b), "all of B played, from its first sample");
    engine.send(Command::Shutdown);
}

#[test]
fn a_seek_storm_settles_on_the_last_target_and_plays_on() {
    let dir = tempfile::tempdir().unwrap();
    let a = pcm(2 * 44_100 * 30, 19);
    let backend = FakeBackend::new(caps(None, 192_000));
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 2.0 }, Box::new(backend), None);
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    for i in 0..200u64 {
        engine.send(Command::Seek(Duration::from_millis(100 + (i * 37) % 9_000)));
        if i % 20 == 0 {
            thread::sleep(Duration::from_millis(1));
        }
    }
    engine.send(Command::Seek(Duration::from_secs(20)));
    events_until(&engine, |e| matches!(e, Event::Seeked { position_secs, .. } if (*position_secs - 20.0).abs() < 0.1));
    // Not left holding: playback carries on from there.
    let deadline = Instant::now() + slack(Duration::from_secs(5));
    while engine.status.position_secs() < 20.5 {
        assert!(Instant::now() < deadline, "stuck at {:.2} s, {:?}", engine.status.position_secs(), engine.status.state());
        thread::sleep(Duration::from_millis(10));
    }
    engine.send(Command::Shutdown);
}

#[test]
fn a_seek_the_source_cant_serve_is_an_error_not_a_stuck_hold() {
    let dir = tempfile::tempdir().unwrap();
    let a = pcm(2 * 44_100 * 30, 20);
    let src = file_source(&dir, "a", &a);
    let src = TrackSource { url: src.url.replacen("file://", "test-unseekable://", 1), ..src };
    let backend = FakeBackend::new(caps(None, 192_000));
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.5 }, Box::new(backend), None);
    engine.send(Command::Play(src));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    let deadline = Instant::now() + slack(Duration::from_secs(10));
    while engine.status.position_secs() < 2.0 {
        assert!(Instant::now() < deadline, "never got going");
        thread::sleep(Duration::from_millis(10));
    }
    // Backwards on a forward-only source.
    engine.send(Command::Seek(Duration::ZERO));
    let seen = events_until(&engine, |e| matches!(e, Event::Error { .. }));
    assert!(matches!(seen.last(), Some(Event::Error { track_id: Some(id), message }) if id == "a" && message.contains("seek")), "{seen:?}");
    let deadline = Instant::now() + slack(Duration::from_secs(2));
    while engine.status.state() != PlayState::Stopped {
        assert!(Instant::now() < deadline, "still {:?}", engine.status.state());
        thread::sleep(Duration::from_millis(10));
    }
    engine.send(Command::Shutdown);
}

/// Plays `src` paused (so its position stays at 0) and waits for it to open.
fn play_paused(engine: &EngineHandle, src: TrackSource) {
    let id = src.id.clone();
    engine.send(Command::Play(src));
    engine.send(Command::Pause);
    events_until(engine, |e| matches!(e, Event::TrackStarted { track_id, .. } if *track_id == id));
}

#[test]
fn a_replaced_decoder_cannot_overwrite_the_new_tracks_gain() {
    use replaygain::db_to_linear;
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::new(caps(None, 192_000));
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.1 }, Box::new(backend), None);
    engine.send(Command::SetGain(GainConfig { mode: GainMode::Track, ..Default::default() }));
    // The old track's decoder stalls just before publishing its gain...
    let mut old = file_source(&dir, "late-gain", &pcm(2 * 44_100, 30));
    old.replay_gain = ReplayGain { track_gain: Some(-12.0), ..Default::default() };
    engine.send(Command::Play(old));
    wait_late("late-gain", false);
    // ...and the new track starts meanwhile, in the same slot.
    let mut new = file_source(&dir, "b", &pcm(2 * 44_100 * 4, 31));
    new.replay_gain = ReplayGain { track_gain: Some(-3.0), ..Default::default() };
    play_paused(&engine, new);
    wait_late("late-gain", true);
    assert_eq!(engine.status.gains.get(0), db_to_linear(-3.0), "the new track keeps its own gain");
    engine.send(Command::Shutdown);
}

#[test]
fn a_decoder_stopped_while_loading_cannot_move_the_next_tracks_position() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::new(caps(None, 192_000));
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.1 }, Box::new(backend), None);
    // A seek while loading: the decoder stalls just before publishing the new position.
    engine.send(Command::Play(file_source(&dir, "late-seek", &pcm(2 * 44_100 * 10, 32))));
    engine.send(Command::Seek(Duration::from_secs(5)));
    wait_late("late-seek", false);
    engine.send(Command::Stop);
    play_paused(&engine, file_source(&dir, "b", &pcm(2 * 44_100 * 4, 33)));
    wait_late("late-seek", true);
    assert_eq!(engine.status.frames_played.load(Ordering::Relaxed), 0, "paused at the start of the new track");
    engine.send(Command::Shutdown);
}

#[test]
fn rapid_plays_settle_on_the_last_track_with_its_own_gain_and_position() {
    use replaygain::db_to_linear;
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::new(caps(None, 192_000));
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.1 }, Box::new(backend), None);
    engine.send(Command::SetGain(GainConfig { mode: GainMode::Track, ..Default::default() }));
    let tracks: Vec<TrackSource> = (0..4)
        .map(|i| {
            let mut t = file_source(&dir, &format!("t{i}"), &pcm(2 * 44_100 * 4, 40 + i));
            t.replay_gain = ReplayGain { track_gain: Some(-2.0 * i as f32), ..Default::default() };
            t
        })
        .collect();
    for i in 0..40 {
        engine.send(Command::Play(tracks[i % 3].clone()));
        if i % 4 == 0 {
            engine.send(Command::Seek(Duration::from_secs(2)));
        }
        if i % 7 == 0 {
            engine.send(Command::Stop);
        }
        if i % 5 == 0 {
            thread::sleep(Duration::from_millis(1));
        }
    }
    play_paused(&engine, tracks[3].clone());
    // Anything left over from the earlier sessions has had time to finish.
    thread::sleep(Duration::from_millis(200));
    assert_eq!(engine.status.gains.get(0), db_to_linear(-6.0));
    assert_eq!(engine.status.frames_played.load(Ordering::Relaxed), 0);
    assert_eq!(engine.status.state(), PlayState::Paused);
    engine.send(Command::Resume);
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    assert!(matches!(seen.last(), Some(Event::Ended { track_id, .. }) if track_id == "t3"), "{seen:?}");
    engine.send(Command::Shutdown);
}

fn source_at(dir: &tempfile::TempDir, name: &str, rate: u32, samples: &[i16]) -> TrackSource {
    let path = dir.path().join(format!("{name}.wav"));
    std::fs::write(&path, crate::decode::tests::wav(rate, 2, samples)).unwrap();
    TrackSource { id: name.into(), url: format!("file://{}", path.display()), suffix: Some("wav".into()), ..Default::default() }
}

fn played_i16(played: &Mutex<Vec<f32>>) -> Vec<i16> {
    played.lock().unwrap().iter().map(|s| (s * 32_768.0) as i16).collect()
}

#[test]
fn a_format_change_and_the_queue_end_play_out_to_the_last_sample() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (pcm(2 * 22_050, 35), pcm(2 * 14_400, 36));
    let mut backend = FakeBackend::new(caps(None, 192_000));
    // A device with long periods (slower than the engine's tick), at about 3x real time.
    backend.period = (Duration::from_millis(30), 4_096);
    let (played, opens) = (backend.played.clone(), backend.opens.clone());
    let engine = spawn(EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs: 0.5 }, Box::new(backend), None);
    engine.send(Command::Play(source_at(&dir, "a", 44_100, &a)));
    engine.send(Command::SetNext(Some(source_at(&dir, "b", 48_000, &b))));
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    assert_eq!(starts(&seen), [("a".to_owned(), false, false), ("b".to_owned(), false, false)], "{seen:?}");
    assert_eq!(opens.load(Ordering::Relaxed), 2, "the device reopened at 48 kHz");
    let out = played_i16(&played);
    assert_eq!(out.len(), a.len() + b.len(), "nothing cut off: {} of A, then B", a.len());
    let expected: Vec<i16> = a.iter().chain(&b).copied().collect();
    assert!(out == expected, "A to its last sample, then all of B");
    engine.send(Command::Shutdown);
}

#[test]
fn an_underrun_rebuffers_and_resumes_where_it_left_off() {
    use crate::decode::tests::{slow_server, wav};
    let a = pcm(2 * 44_100 * 4, 37);
    // The server sends ~3x real time; the fake device plays ~10x.
    let url = slow_server(wav(44_100, 2, &a));
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = StreamCache::new(streamcache::CacheConfig::new(dir.path()), rt.handle().clone()).unwrap();
    let backend = FakeBackend::new(caps(None, 192_000));
    let played = backend.played.clone();
    let engine = spawn(EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs: 0.5 }, Box::new(backend), Some(cache));
    engine.send(Command::Play(TrackSource { id: "net".into(), url, suffix: Some("wav".into()), ..Default::default() }));
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. } | Event::Error { .. }));
    assert!(matches!(seen.last(), Some(Event::Ended { track_id, .. }) if track_id == "net"), "{seen:?}");
    assert!(engine.status.underruns() > 0, "the device outran the download");
    // Bit-perfect: no fades around the dropouts, so every sample arrives exactly once, in order.
    assert!(played_i16(&played) == a, "playback resumed where it ran dry each time");
    engine.send(Command::Shutdown);
}

/// 16-bit test samples at least 64 steps from silence: dithering never turns one into 0.
fn loud(n: usize, salt: i64) -> Vec<i16> {
    pcm(n, salt).into_iter().map(|s| if (s as i32).abs() < 64 { 1_000 } else { s }).collect()
}

fn source_of(dir: &tempfile::TempDir, name: &str, wav: Vec<u8>) -> TrackSource {
    let path = dir.path().join(format!("{name}.wav"));
    std::fs::write(&path, wav).unwrap();
    TrackSource { id: name.into(), url: format!("file://{}", path.display()), suffix: Some("wav".into()), ..Default::default() }
}

/// A track at 44.1 kHz stereo of the given depth (`None`: float, no depth), and the samples the device should get.
fn track_of_depth(dir: &tempfile::TempDir, name: &str, bits: Option<u32>, salt: i64) -> (TrackSource, Vec<f32>) {
    use crate::decode::tests::{wav, wav_float, wav24};
    let base = loud(2 * 44_100, salt);
    match bits {
        Some(16) => (source_of(dir, name, wav(44_100, 2, &base)), base.iter().map(|s| *s as f32 / 32_768.0).collect()),
        // Off the 16-bit grid: the low byte is set.
        Some(24) => {
            let s: Vec<i32> = base.iter().map(|s| *s as i32 * 256 + 91).collect();
            (source_of(dir, name, wav24(44_100, 2, &s)), s.iter().map(|s| *s as f32 / 8_388_608.0).collect())
        }
        None => {
            let s: Vec<f32> = base.iter().map(|s| (*s as f32 + 0.37) / 32_768.0).collect();
            (source_of(dir, name, wav_float(44_100, 2, &s)), s)
        }
        Some(b) => unreachable!("{b}-bit"),
    }
}

/// About twice real time: a queue of 1 s tracks then leaves `play_queue`
/// close to half a second to name each next track, even on a loaded machine.
const UI_PACE: (Duration, usize) = (Duration::from_millis(5), 441);

/// Plays `tracks` as one queue, naming each next track once the one before
/// it starts (as the UI does). Returns every event up to the end.
fn play_queue(mut backend: FakeBackend, tracks: &[TrackSource]) -> Vec<Event> {
    backend.period = UI_PACE;
    let engine = &spawn(EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs: 0.5 }, Box::new(backend), None);
    engine.send(Command::Play(tracks[0].clone()));
    let mut seen = Vec::new();
    for pair in tracks.windows(2) {
        let id = pair[0].id.clone();
        seen.extend(events_until(engine, |e| matches!(e, Event::TrackStarted { track_id, .. } if *track_id == id)));
        engine.send(Command::SetNext(Some(pair[1].clone())));
    }
    seen.extend(events_until(engine, |e| matches!(e, Event::Ended { .. })));
    engine.send(Command::Shutdown);
    seen
}

/// Each `SignalPath` with the track that had started last.
fn paths_by_track(seen: &[Event]) -> Vec<(String, SignalPath)> {
    let mut current = String::new();
    let mut paths = Vec::new();
    for e in seen {
        match e {
            Event::TrackStarted { track_id, .. } => current.clone_from(track_id),
            Event::SignalPath(p) => paths.push((current.clone(), p.clone())),
            _ => {}
        }
    }
    paths
}

#[test]
fn tracks_of_different_depths_play_gaplessly_on_one_stream() {
    let dir = tempfile::tempdir().unwrap();
    let depths = [Some(16), Some(24), None, Some(16)];
    let (tracks, expected): (Vec<_>, Vec<_>) = depths.iter().enumerate().map(|(i, bits)| track_of_depth(&dir, &format!("t{i}"), *bits, 60 + i as i64)).unzip();
    let backend = FakeBackend::new(caps(None, 192_000));
    let (played, opens) = (backend.played.clone(), backend.opens.clone());
    let seen = play_queue(backend, &tracks);

    let started: Vec<_> = starts(&seen).into_iter().map(|(id, gapless, _)| (id, gapless)).collect();
    assert_eq!(started, [("t0".into(), false), ("t1".into(), true), ("t2".into(), true), ("t3".into(), true)], "{seen:?}");
    assert_eq!(opens.load(Ordering::Relaxed), 1, "16 → 24 → lossy → 16 bits without reopening the device");
    // A float device takes every sample as it is.
    let out = played.lock().unwrap().clone();
    let expected: Vec<f32> = expected.concat();
    assert_eq!(out.len(), expected.len());
    assert!(out.iter().zip(&expected).all(|(a, b)| a.to_bits() == b.to_bits()), "every track bit for bit");
    // Each track is described by its own depth.
    let depth: Vec<_> = paths_by_track(&seen).into_iter().map(|(id, p)| (id, p.source.bits_per_sample)).collect();
    assert_eq!(depth, [("t0".into(), Some(16)), ("t1".into(), Some(24)), ("t2".into(), None), ("t3".into(), Some(16))]);
}

#[test]
fn on_a_16_bit_device_only_the_tracks_off_its_grid_are_dithered() {
    let dir = tempfile::tempdir().unwrap();
    let depths = [None, Some(16), Some(24), Some(16)];
    let (tracks, sources): (Vec<_>, Vec<_>) = depths.iter().enumerate().map(|(i, bits)| track_of_depth(&dir, &format!("t{i}"), *bits, 70 + i as i64)).unzip();
    let device = DeviceCaps { active_bits: Some(16), ..caps(Some(16), 192_000) };
    let backend = FakeBackend::new(device);
    let (played, opens) = (backend.played.clone(), backend.opens.clone());
    let seen = play_queue(backend, &tracks);
    assert_eq!(opens.load(Ordering::Relaxed), 1);

    let out = played.lock().unwrap().clone();
    assert_eq!(out.len(), sources.iter().map(Vec::len).sum::<usize>());
    let mut at = 0;
    for (i, (src, bits)) in sources.iter().zip(depths).enumerate() {
        let part = &out[at..at + src.len()];
        at += src.len();
        let exact = part.iter().zip(src).all(|(a, b)| a.to_bits() == b.to_bits());
        if bits == Some(16) {
            assert!(exact, "t{i}: 16-bit reaches the 16-bit device untouched");
        } else {
            let on_grid = part.iter().all(|s| (*s * 32_768.0).fract() == 0.0);
            let close = part.iter().zip(src).all(|(a, b)| (a - b).abs() <= 1.5 / 32_768.0);
            assert!(!exact && on_grid && close, "t{i} ({bits:?}): dithered to 16 bits");
        }
    }
    // And the signal path says so, track by track.
    let dithered: Vec<_> = paths_by_track(&seen).into_iter().map(|(id, p)| (id, p.dithered)).collect();
    assert_eq!(dithered, [("t0".into(), true), ("t1".into(), false), ("t2".into(), true), ("t3".into(), false)]);
    let verdict = paths_by_track(&seen).into_iter().find(|(id, _)| id == "t2").unwrap().1.bit_perfect;
    assert_eq!(verdict, BitPerfectStatus::Unavailable { reason: "device max 16/192, file is 24/44.1".into(), device_limit: true });
}

/// Needs real hardware: the whole engine on this platform's own backend, on the
/// device named in `OND_TEST_DEVICE` (by UID; the default without it), in
/// bit-perfect mode as the app drives it, playing near-silence for
/// `OND_TEST_SECS` (5). With `OND_TEST_SWITCH` set it starts in Normal and
/// switches to bit-perfect mid-track, as the picker does (`back`: the other
/// way round). Prints the events and position.
#[test]
#[ignore = "needs audio hardware"]
fn engine_on_a_real_device() {
    let uid = std::env::var("OND_TEST_DEVICE").ok();
    let secs: u64 = std::env::var("OND_TEST_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(5);
    // OND_TEST_SWITCH: start in Normal and switch to bit-perfect mid-track, as
    // the picker does; `back` the other way round.
    let back = std::env::var("OND_TEST_SWITCH").is_ok_and(|s| s == "back");
    let switch = std::env::var_os("OND_TEST_SWITCH").is_some();
    let (first, then) = if back { (PlaybackMode::BitPerfect, PlaybackMode::Normal) } else { (PlaybackMode::Normal, PlaybackMode::BitPerfect) };
    let dir = tempfile::tempdir().unwrap();
    let quiet = vec![1i16; 44_100 * 2 * (secs as usize + 2)];
    let engine = spawn(EngineConfig::default(), default_backend(), None);
    let first = if switch { first } else { PlaybackMode::BitPerfect };
    if let Some(uid) = &uid {
        engine.send(Command::DeviceModes(HashMap::from([(uid.clone(), first)])));
    } else {
        engine.send(Command::SetMode(first));
    }
    engine.send(Command::SelectDevice(uid.clone()));
    engine.send(Command::Play(file_source(&dir, "quiet", &quiet)));
    let start = Instant::now();
    let (mut last, mut switched) = (-1.0, !switch);
    while start.elapsed() < Duration::from_secs(secs) {
        if !switched && start.elapsed() > Duration::from_millis(1500) {
            println!("{:>5} ms  -- SetMode({then:?})", start.elapsed().as_millis());
            if let Some(uid) = &uid {
                engine.send(Command::DeviceModes(HashMap::from([(uid.clone(), then)])));
            }
            engine.send(Command::SetMode(then));
            switched = true;
        }
        while let Ok(ev) = engine.events.try_recv() {
            match ev {
                Event::SignalPath(p) => println!("{:>5} ms  {} · {} · {:?}", start.elapsed().as_millis(), p.output, p.summary(), p.bit_perfect),
                Event::Devices { .. } => {}
                ev => println!("{:>5} ms  {ev:?}", start.elapsed().as_millis()),
            }
        }
        let pos = engine.status.position_secs();
        if (pos - last).abs() > 0.9 {
            println!("{:>5} ms  position {pos:.2} {:?}", start.elapsed().as_millis(), engine.status.state());
            last = pos;
        }
        thread::sleep(Duration::from_millis(20));
    }
    println!("final position {:.2}", engine.status.position_secs());
    assert!(engine.status.position_secs() > secs as f64 - 2.5, "it played");
    engine.shutdown(Duration::from_secs(3));
}

#[test]
fn a_device_switch_while_the_track_is_still_loading_plays_that_track_from_where_it_was_asked() {
    let dir = tempfile::tempdir().unwrap();
    let a = file_source(&dir, "a", &pcm(2 * 44_100 * 4, 26));
    // A slow server: the track takes half a second to open, so it's still loading below.
    let a = TrackSource { url: a.url.replacen("file://", "test-slow://", 1), ..a };
    let backend = FakeBackend::new(caps(Some(24), 192_000));
    *backend.devices.lock().unwrap() = vec![device("spk", "Speakers", DeviceKind::BuiltInSpeakers), device("dac", "DAC", DeviceKind::Usb)];
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.1 }, Box::new(backend), None);
    engine.send(Command::Play(a));
    engine.send(Command::Seek(Duration::from_secs(2)));
    engine.send(Command::SelectDevice(Some("dac".into())));
    let seen = events_until(&engine, |e| matches!(e, Event::Devices { .. }));
    assert!(matches!(seen.last(), Some(Event::Devices { current: Some(d), .. }) if d == "dac"), "{seen:?}");
    assert_eq!(engine.status.state(), PlayState::Loading, "the track is opened again on the new device, not dropped");
    let seen = events_until(&engine, |e| matches!(e, Event::TrackStarted { .. } | Event::Ended { .. }));
    // Its first start as far as the UI knows: not flagged as a restart.
    assert_eq!(starts(&seen), [("a".to_owned(), false, false)], "{seen:?}");
    assert!(engine.status.position_secs() >= 1.9, "from the seek made while loading: {:.2} s", engine.status.position_secs());
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. } | Event::Error { .. }));
    assert!(matches!(seen.last(), Some(Event::Ended { track_id, .. }) if track_id == "a"), "{seen:?}");
    engine.send(Command::Shutdown);
}

#[test]
fn a_seek_superseded_while_the_decoder_finishes_the_one_before_is_not_left_holding() {
    let dir = tempfile::tempdir().unwrap();
    let a = pcm(2 * 44_100 * 10, 27);
    let backend = FakeBackend::new(caps(None, 192_000));
    let (stalled, ring_ctl) = (backend.stalled.clone(), backend.ring_ctl.clone());
    // A is longer than the buffer: the decoder is still on it when the seeks come.
    let engine = spawn(EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs: 0.5 }, Box::new(backend), None);
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    // The device stops servicing the ring, so the decoder's first seek waits on
    // its flush while the second goes in.
    stalled.store(true, Ordering::Relaxed);
    engine.send(Command::Seek(Duration::from_secs(100)));
    let ctl = ring_ctl.lock().unwrap().clone().expect("a ring is open");
    let deadline = Instant::now() + slack(Duration::from_secs(5));
    while !ctl.flush_pending() {
        assert!(Instant::now() < deadline, "the decoder never took the first seek");
        thread::sleep(Duration::from_millis(1));
    }
    engine.send(Command::Seek(Duration::from_secs(1)));
    // Handled in order: once this is answered, the second seek is on hold.
    engine.send(Command::RefreshDevices);
    events_until(&engine, |e| matches!(e, Event::Devices { .. }));
    // The first seek, past the end, finishes the track: the decoder exits
    // without reading the second. The output is still holding for it.
    stalled.store(false, Ordering::Relaxed);
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. } | Event::Error { .. }));
    assert!(seen.iter().any(|e| matches!(e, Event::Seeked { position_secs, .. } if *position_secs == 1.0)), "the second seek landed: {seen:?}");
    assert!(matches!(seen.last(), Some(Event::Ended { track_id, .. }) if track_id == "a"), "{seen:?}");
    engine.send(Command::Shutdown);
}

#[test]
fn naming_the_same_next_again_after_the_decoder_moved_on_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (pcm(2 * 44_100 * 8, 28), pcm(2 * 44_100, 29));
    let backend = FakeBackend::new(caps(None, 192_000));
    let opens = backend.opens.clone();
    let engine = spawn(EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs: 30.0 }, Box::new(backend), None);
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    engine.send(Command::SetNext(Some(file_source(&dir, "b", &b))));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    wait_decoding(&engine, 1);
    // The UI says it again (a queue edit that left the next track as it was).
    engine.send(Command::SetNext(Some(file_source(&dir, "b", &b))));
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    assert_eq!(starts(&seen), [("b".to_owned(), true, false)], "no restart, B follows gaplessly: {seen:?}");
    assert_eq!(opens.load(Ordering::Relaxed), 1);
    engine.send(Command::Shutdown);
}

#[test]
fn clearing_the_next_after_the_decoder_moved_on_restarts_the_current_track_where_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (pcm(2 * 44_100 * 8, 30), pcm(2 * 44_100, 31));
    let backend = FakeBackend::new(caps(None, 192_000));
    let engine = spawn(EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs: 30.0 }, Box::new(backend), None);
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    engine.send(Command::SetNext(Some(file_source(&dir, "b", &b))));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    wait_decoding(&engine, 1);
    // B was taken off the queue: A must end the queue instead.
    engine.send(Command::SetNext(None));
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    assert_eq!(starts(&seen), [("a".to_owned(), false, true)], "A starts over as a restart, B never plays: {seen:?}");
    assert!(matches!(seen.last(), Some(Event::Ended { track_id, .. }) if track_id == "a"), "{seen:?}");
    engine.send(Command::Shutdown);
}

/// A stream cache over a fresh directory, and the runtime it downloads on.
fn net_cache() -> (StreamCache, tokio::runtime::Runtime, tempfile::TempDir) {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = StreamCache::new(streamcache::CacheConfig::new(dir.path()), rt.handle().clone()).unwrap();
    (cache, rt, dir)
}

/// `samples` at `rate`, served slowly over HTTP as track `id`.
fn net_source(id: &str, rate: u32, samples: &[i16]) -> TrackSource {
    let url = crate::decode::tests::slow_server(crate::decode::tests::wav(rate, 2, samples));
    TrackSource { id: id.into(), url, suffix: Some("wav".into()), ..Default::default() }
}

/// Waits until the cache has started on `src`, and returns what it has.
fn wait_fetching(cache: &StreamCache, src: &TrackSource, mut meanwhile: impl FnMut()) -> streamcache::Progress {
    let deadline = Instant::now() + slack(Duration::from_secs(10));
    loop {
        if let Some(p) = cache.progress(&src.cache_key()) {
            return p;
        }
        meanwhile();
        assert!(Instant::now() < deadline, "{} was never fetched", src.id);
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_long_tracks_next_is_prefetched_once_its_own_download_completes() {
    let (cache, _rt, _dir) = net_cache();
    // 32 s at 8 kHz (1 MB, about 2 s to arrive): longer than the prefetch lead,
    // and paused at its start, so only the finished download starts the next one.
    let a = net_source("a", 8_000, &pcm(2 * 8_000 * 32, 32));
    let (b, c) = (net_source("b", 8_000, &pcm(2 * 8_000, 33)), net_source("c", 8_000, &pcm(2 * 8_000, 34)));
    let engine = spawn(EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs: 0.5 }, Box::new(FakeBackend::new(caps(None, 192_000))), Some(cache.clone()));
    play_paused(&engine, a);
    engine.send(Command::SetNext(Some(b.clone())));
    let mut seen_downloading = false;
    wait_fetching(&cache, &b, || {
        let status = &engine.status;
        seen_downloading |= status.needs_network() && status.downloading() && status.fetched_fraction().is_some_and(|f| f < 1.0);
    });
    assert!(seen_downloading, "the current track's download was reported while it ran");
    assert_eq!(engine.status.fetched_fraction(), Some(1.0), "the next track waited for the current one");

    // The next track changes: the new one is fetched in its place.
    engine.send(Command::SetNext(Some(c.clone())));
    let deadline = Instant::now() + slack(Duration::from_secs(10));
    while !wait_fetching(&cache, &c, || {}).complete {
        assert!(Instant::now() < deadline, "c never finished");
        thread::sleep(Duration::from_millis(5));
    }
    // A paused engine looks at the network a few times a second, not every tick.
    while engine.status.needs_network() {
        assert!(Instant::now() < deadline, "still using the network once everything is down");
        thread::sleep(Duration::from_millis(5));
    }
    engine.send(Command::Shutdown);
}

#[test]
fn a_short_tracks_next_is_prefetched_while_it_is_still_downloading() {
    let (cache, _rt, _dir) = net_cache();
    // 5 s (880 KB, at least 1.6 s to arrive): within the prefetch lead of its
    // end from the start, so the next one doesn't wait for the download.
    let a = net_source("a", 44_100, &pcm(2 * 44_100 * 5, 35));
    let b = net_source("b", 44_100, &pcm(2 * 4_410, 36));
    let engine = spawn(EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs: 0.5 }, Box::new(FakeBackend::new(caps(None, 192_000))), Some(cache.clone()));
    play_paused(&engine, a);
    engine.send(Command::SetNext(Some(b.clone())));
    wait_fetching(&cache, &b, || {});
    let fetched = engine.status.fetched_fraction();
    assert!(fetched.is_some_and(|f| f < 1.0), "the current track was still downloading: {fetched:?}");
    assert!(engine.status.needs_network());
    engine.send(Command::Shutdown);
}

#[test]
fn playing_from_a_position_never_outputs_the_beginning() {
    let dir = tempfile::tempdir().unwrap();
    let samples = loud(2 * 44_100 * 3, 91);
    let backend = FakeBackend::new(caps(None, 192_000));
    let played = backend.played.clone();
    let engine = spawn(EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs: 0.5 }, Box::new(backend), None);
    engine.send(Command::PlayFrom(file_source(&dir, "book", &samples), Duration::from_secs(2)));
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    assert!(!seen.iter().any(|e| matches!(e, Event::Error { .. })), "{seen:?}");
    assert_eq!(played_i16(&played), samples[2 * 44_100 * 2..]);
    engine.shutdown(Duration::from_secs(2));
}

#[test]
fn source_time_matches_output_time_at_unity_through_resampling_and_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let mut backend = FakeBackend::new(caps(None, 192_000));
    backend.own_rate = Some(48_000);
    let positions = backend.positions.clone();
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.5 }, Box::new(backend), None);
    let source = |id, frames: usize| source_at(&dir, id, 96_000, &loud(frames * 2, 101));
    engine.send(Command::Play(source("a", 96_001)));
    engine.send(Command::SetNext(Some(source("b", 96_007))));
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    assert_eq!(starts(&seen), [("a".into(), false, false), ("b".into(), true, false)]);
    let positions = positions.lock().unwrap();
    assert!(positions.iter().any(|(seq, _, source)| *seq == 1 && *source > 0.1));
    for (_, output, source) in positions.iter() {
        assert!((output - source).abs() < 1.0 / 48_000.0, "output {output}, source {source}");
    }
    engine.shutdown(Duration::from_secs(2));
}

#[test]
fn source_time_starts_at_the_requested_position_and_follows_a_seek() {
    let dir = tempfile::tempdir().unwrap();
    let mut backend = FakeBackend::new(caps(None, 192_000));
    backend.period = UI_PACE;
    let positions = backend.positions.clone();
    let engine = spawn(EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs: 0.5 }, Box::new(backend), None);
    engine.send(Command::PlayFrom(file_source(&dir, "book", &loud(2 * 44_100 * 8, 102)), Duration::from_secs(2)));
    engine.send(Command::Pause);
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    assert_eq!(engine.status.source_position_secs(), 2.0);
    engine.send(Command::Seek(Duration::from_secs_f64(4.25)));
    events_until(&engine, |e| matches!(e, Event::Seeked { .. }));
    assert!((engine.status.source_position_secs() - 4.25).abs() <= 1.0 / 44_100.0);
    engine.send(Command::Resume);
    events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    for (_, output, source) in positions.lock().unwrap().iter() {
        assert!((output - source).abs() <= 1.0 / 44_100.0, "output {output}, source {source}");
    }
    engine.shutdown(Duration::from_secs(2));
}

/// Voiced harmonics with different stereo levels, rather than noise that gives
/// a pitch-synchronous stretcher no useful periods to match.
fn voice(rate: u32, seconds: usize) -> Vec<i16> {
    (0..rate as usize * seconds)
        .flat_map(|i| {
            let phase = std::f64::consts::TAU * 220.0 * i as f64 / rate as f64;
            let sample = ((phase.sin() * 0.4 + (phase * 2.0).sin() * 0.15) * 32_000.0) as i16;
            [if sample == 0 { 1 } else { sample }, if sample / 2 == 0 { 1 } else { sample / 2 }]
        })
        .collect()
}

#[test]
fn source_time_after_starting_and_seeking_is_independent_of_speed() {
    for speed in [0.5, 1.0, 1.5, 2.0, 3.0] {
        let dir = tempfile::tempdir().unwrap();
        let mut backend = FakeBackend::new(caps(None, 192_000));
        backend.period = UI_PACE;
        let positions = backend.positions.clone();
        let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.5 }, Box::new(backend), None);
        engine.send(Command::SetSpeed(speed));
        engine.send(Command::PlayFrom(file_source(&dir, "book", &voice(44_100, 6)), Duration::from_secs_f64(1.25)));
        engine.send(Command::Pause);
        events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
        assert_eq!(engine.status.speed(), speed);
        assert_eq!(engine.status.source_position_secs(), 1.25);
        assert_eq!(engine.status.source_duration_secs(), Some(6.0));
        assert_eq!(engine.status.duration_secs(), Some(6.0 / speed as f64));
        engine.send(Command::Seek(Duration::from_secs_f64(4.25)));
        events_until(&engine, |e| matches!(e, Event::Seeked { .. }));
        assert!((engine.status.source_position_secs() - 4.25).abs() <= 1.0 / 44_100.0);
        engine.send(Command::Resume);
        let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
        assert!(!seen.iter().any(|e| matches!(e, Event::Error { .. })), "{speed}×: {seen:?}");
        let positions = positions.lock().unwrap();
        assert!(positions.iter().any(|(_, _, source)| *source > 5.5), "{speed}× played on");
        // The clock is affine to within one output-frame rounding at the seek.
        // Sonic's local pitch overlap is bounded separately by two pitch periods.
        for (_, output, source) in positions.iter() {
            assert!((source - output * speed as f64).abs() <= speed as f64 / 44_100.0, "{speed}×: source {source}, output {output}");
        }
        engine.shutdown(Duration::from_secs(2));
    }
}

#[test]
fn double_speed_outputs_about_half_the_source_duration() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::new(caps(None, 192_000));
    let played = backend.played.clone();
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.5 }, Box::new(backend), None);
    engine.send(Command::SetSpeed(2.0));
    engine.send(Command::Play(file_source(&dir, "book", &voice(44_100, 10))));
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    assert!(!seen.iter().any(|e| matches!(e, Event::Error { .. })), "{seen:?}");
    let output_seconds = played.lock().unwrap().len() as f64 / (44_100.0 * 2.0);
    // EOF overlap/rounding is bounded by Sonic's two 65 Hz pitch periods.
    assert!((output_seconds - 5.0).abs() < 2.0 / 65.0, "{output_seconds} seconds");
    engine.shutdown(Duration::from_secs(2));
}

fn wait_source_position(engine: &EngineHandle, at: f64) {
    let deadline = Instant::now() + slack(Duration::from_secs(5));
    while engine.status.source_position_secs() < at {
        assert!(Instant::now() < deadline, "never played to {at}: {:?}", engine.status);
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn changing_speed_while_playing_refills_the_same_ring_from_source_time() {
    let dir = tempfile::tempdir().unwrap();
    let mut backend = FakeBackend::new(caps(None, 192_000));
    backend.period = UI_PACE;
    let opens = backend.opens.clone();
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 2.0 }, Box::new(backend), None);
    engine.send(Command::PlayFrom(file_source(&dir, "book", &voice(44_100, 12)), Duration::from_secs_f64(1.25)));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    wait_source_position(&engine, 1.5);
    for speed in [1.5, 2.0, 3.0, 1.0] {
        let before = engine.status.source_position_secs();
        engine.send(Command::SetSpeed(speed));
        let seen = events_until(&engine, |e| matches!(e, Event::Seeked { .. }));
        let Some(Event::Seeked { position_secs: at, .. }) = seen.last() else { unreachable!() };
        assert!((*at - before).abs() < 0.1, "{speed}×: {before} → {at}");
        assert_eq!(engine.status.speed(), speed);
        assert!((engine.status.source_position_secs() - engine.status.position_secs() * speed as f64).abs() < speed as f64 / 44_100.0);
        wait_source_position(&engine, at + 0.25);
        assert_eq!(opens.load(Ordering::Relaxed), 1, "the existing ring's hold/flush handles each change");
    }
    engine.send(Command::Pause);
    engine.send(Command::RefreshDevices);
    events_until(&engine, |e| matches!(e, Event::Devices { .. }));
    let paused_at = engine.status.source_position_secs();
    engine.send(Command::SetSpeed(2.0));
    events_until(&engine, |e| matches!(e, Event::Seeked { .. }));
    assert_eq!(engine.status.state(), PlayState::Paused);
    assert!((engine.status.source_position_secs() - paused_at).abs() <= 1.0 / 44_100.0);
    engine.shutdown(Duration::from_secs(2));
}

#[test]
fn stretching_and_resampling_keep_tracks_gapless_with_their_own_source_clocks() {
    for speed in [1.5, 2.0, 3.0] {
        let dir = tempfile::tempdir().unwrap();
        let mut backend = FakeBackend::new(caps(None, 192_000));
        backend.own_rate = Some(48_000);
        backend.period = UI_PACE;
        let (positions, opens, played) = (backend.positions.clone(), backend.opens.clone(), backend.played.clone());
        let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 2.0 }, Box::new(backend), None);
        engine.send(Command::SetSpeed(speed));
        engine.send(Command::Play(source_at(&dir, "a", 96_000, &voice(96_000, 2))));
        engine.send(Command::SetNext(Some(source_at(&dir, "b", 96_000, &voice(96_000, 3)))));
        let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
        assert_eq!(starts(&seen), [("a".into(), false, false), ("b".into(), true, false)], "{speed}×: {seen:?}");
        assert_eq!(opens.load(Ordering::Relaxed), 1);
        assert_eq!(engine.status.underruns(), 0);
        assert_eq!(engine.status.speed(), speed);
        let positions = positions.lock().unwrap();
        let second: Vec<_> = positions.iter().filter(|(seq, _, _)| *seq == 1).collect();
        assert!(!second.is_empty());
        assert!(second.first().unwrap().2 < 0.05, "new track begins from zero");
        let end = second.last().unwrap().2;
        assert!((end - 3.0).abs() < 2.0 / 65.0, "{speed}×: ended at source {end}");
        for (_, output, source) in positions.iter() {
            assert!((source - output * speed as f64).abs() < 1e-9, "{speed}×: source {source}, output {output}");
        }
        let seconds = played.lock().unwrap().len() as f64 / (2.0 * 48_000.0);
        assert!((seconds - 5.0 / speed as f64).abs() < 4.0 / 65.0, "{seconds} seconds");
        engine.shutdown(Duration::from_secs(2));
    }
}

#[test]
fn returning_to_unity_outputs_the_original_float_samples_bit_for_bit() {
    let dir = tempfile::tempdir().unwrap();
    let samples: Vec<f32> = (0..44_100 * 8).map(|i| 0.1 + i as f32 / 1_000_000.0).collect();
    let source = source_of(&dir, "float", crate::decode::tests::wav_float(44_100, 2, &samples));
    let mut backend = FakeBackend::new(caps(None, 192_000));
    backend.period = UI_PACE;
    let (played, opens) = (backend.played.clone(), backend.opens.clone());
    let engine = spawn(EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs: 0.5 }, Box::new(backend), None);
    engine.send(Command::SetSpeed(2.0));
    engine.send(Command::Play(source));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    wait_source_position(&engine, 0.3);
    engine.send(Command::SetSpeed(1.0));
    let seen = events_until(&engine, |e| matches!(e, Event::Seeked { .. }));
    let Some(Event::Seeked { position_secs: at, .. }) = seen.last() else { unreachable!() };
    let frame = (at * 44_100.0).round() as usize;
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    assert!(!seen.iter().any(|e| matches!(e, Event::Error { .. })), "{seen:?}");
    let played = played.lock().unwrap();
    let expected = &samples[frame * 2..];
    assert!(played.len() >= expected.len());
    let actual = &played[played.len() - expected.len()..];
    assert!(actual.iter().zip(expected).all(|(a, b)| a.to_bits() == b.to_bits()), "unity bypasses all stretching and quantization");
    assert_eq!(opens.load(Ordering::Relaxed), 1);
    engine.shutdown(Duration::from_secs(2));
}

#[test]
fn the_source_clock_follows_an_audible_landmark_without_a_stretch_latency_offset() {
    for speed in [0.5, 1.0, 1.5, 2.0, 3.0] {
        let dir = tempfile::tempdir().unwrap();
        let samples: Vec<i16> = (0..44_100 * 4).flat_map(|i| if i < 44_100 * 2 { [-12_000, -12_000] } else { [12_000, 12_000] }).collect();
        let mut backend = FakeBackend::new(caps(None, 192_000));
        backend.period = UI_PACE;
        let landmarks = backend.landmarks.clone();
        let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.5 }, Box::new(backend), None);
        engine.send(Command::SetSpeed(speed));
        engine.send(Command::PlayFrom(file_source(&dir, "landmark", &samples), Duration::from_secs_f64(0.25)));
        events_until(&engine, |e| matches!(e, Event::Ended { .. }));
        let landmarks = landmarks.lock().unwrap();
        let positive_at = landmarks.iter().find(|(_, sample)| *sample > 0.1).expect("the positive half played").0;
        // Two pitch periods bound Sonic's local overlap; observing one whole
        // callback adds at most 10 ms of output time to the landmark's timestamp.
        let tolerance = 2.0 / 65.0 + 0.01 * speed as f64;
        assert!((positive_at - 2.0).abs() < tolerance, "{speed}×: landmark at {positive_at}, tolerance {tolerance}");
        engine.shutdown(Duration::from_secs(2));
    }
}

#[test]
fn speed_requests_are_bounded_and_invalid_values_leave_playback_alone() {
    let engine = spawn(EngineConfig::default(), Box::new(FakeBackend::new(caps(None, 192_000))), None);
    for (requested, expected) in [(0.0, 0.5), (9.0, 3.0), (1.53, 1.55), (f32::NAN, 1.55), (f32::INFINITY, 1.55), (1.0, 1.0)] {
        engine.send(Command::SetSpeed(requested));
        engine.send(Command::RefreshDevices);
        events_until(&engine, |e| matches!(e, Event::Devices { .. }));
        assert_eq!(engine.status.speed(), expected);
    }
    engine.shutdown(Duration::from_secs(2));
}

#[test]
fn a_speed_change_after_the_decoder_moves_on_restarts_at_source_time() {
    let dir = tempfile::tempdir().unwrap();
    let mut backend = FakeBackend::new(caps(None, 192_000));
    backend.period = UI_PACE;
    let positions = backend.positions.clone();
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 30.0 }, Box::new(backend), None);
    engine.send(Command::SetSpeed(1.5));
    engine.send(Command::PlayFrom(file_source(&dir, "a", &voice(44_100, 5)), Duration::from_secs_f64(2.25)));
    engine.send(Command::Pause);
    engine.send(Command::SetNext(Some(file_source(&dir, "b", &voice(44_100, 2)))));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    wait_decoding(&engine, 1);
    engine.send(Command::SetSpeed(2.0));
    let seen = events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    assert_eq!(starts(&seen), [("a".into(), false, true)], "{seen:?}");
    assert_eq!(engine.status.state(), PlayState::Paused);
    assert_eq!(engine.status.source_position_secs(), 2.25, "restart used source time, not output time");
    assert_eq!(engine.status.speed(), 2.0);
    positions.lock().unwrap().clear();
    engine.send(Command::Resume);
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    assert_eq!(starts(&seen), [("b".into(), true, false)], "the next track follows at the new speed: {seen:?}");
    assert!(positions.lock().unwrap().iter().filter(|(seq, _, _)| *seq == 1).all(|(_, output, source)| (source - output * 2.0).abs() < 1e-9));
    engine.shutdown(Duration::from_secs(2));
}

#[test]
fn speed_changes_during_a_seek_keep_the_pending_source_target() {
    let dir = tempfile::tempdir().unwrap();
    let mut backend = FakeBackend::new(caps(None, 192_000));
    backend.period = UI_PACE;
    let (stalled, ring_ctl) = (backend.stalled.clone(), backend.ring_ctl.clone());
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.5 }, Box::new(backend), None);
    play_paused(&engine, file_source(&dir, "a", &voice(44_100, 8)));
    stalled.store(true, Ordering::Relaxed);
    engine.send(Command::Seek(Duration::from_secs_f64(4.25)));
    let ctl = ring_ctl.lock().unwrap().clone().expect("open ring");
    let deadline = Instant::now() + slack(Duration::from_secs(5));
    while !ctl.flush_pending() {
        assert!(Instant::now() < deadline, "the decoder never took the seek");
        thread::sleep(Duration::from_millis(1));
    }
    engine.send(Command::SetSpeed(3.0));
    engine.send(Command::RefreshDevices);
    events_until(&engine, |e| matches!(e, Event::Devices { .. }));
    stalled.store(false, Ordering::Relaxed);
    // Both seeks report the same target. Wait for the output-equivalent
    // position to show that the request carrying the new speed also landed.
    let deadline = Instant::now() + slack(Duration::from_secs(5));
    while (engine.status.position_secs() - 4.25 / 3.0).abs() > 1.0 / 44_100.0 {
        assert!(Instant::now() < deadline, "the speed change never landed: {:?}", engine.status);
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(engine.status.source_position_secs(), 4.25);
    assert_eq!(engine.status.state(), PlayState::Paused);
    engine.send(Command::Resume);
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    assert!(!seen.iter().any(|e| matches!(e, Event::Error { .. })), "{seen:?}");
    engine.shutdown(Duration::from_secs(2));
}

#[test]
fn seeks_under_stretch_and_resampling_start_from_source_frames() {
    for (source_rate, output_rate) in [(96_000, 48_000), (44_100, 192_000)] {
        let dir = tempfile::tempdir().unwrap();
        let mut backend = FakeBackend::new(caps(None, 192_000));
        backend.own_rate = Some(output_rate);
        backend.period = UI_PACE;
        let positions = backend.positions.clone();
        let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.5 }, Box::new(backend), None);
        engine.send(Command::SetSpeed(3.0));
        engine.send(Command::PlayFrom(source_at(&dir, "a", source_rate, &voice(source_rate, 6)), Duration::from_secs_f64(1.25)));
        engine.send(Command::Pause);
        events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
        engine.send(Command::Seek(Duration::from_secs_f64(4.125013)));
        events_until(&engine, |e| matches!(e, Event::Seeked { .. }));
        assert!((engine.status.source_position_secs() - 4.125013).abs() <= 1.0 / source_rate as f64);
        engine.send(Command::Resume);
        events_until(&engine, |e| matches!(e, Event::Ended { .. }));
        let positions = positions.lock().unwrap();
        assert!(positions.iter().any(|(_, _, source)| *source > 5.5));
        for (_, output, source) in positions.iter() {
            assert!((source - output * 3.0).abs() <= 3.0 / output_rate as f64, "source {source}, output {output}");
        }
        engine.shutdown(Duration::from_secs(2));
    }
}

#[test]
fn silence_cuts_keep_source_landmarks_and_saved_time_at_every_speech_speed() {
    for speed in [1.0, 1.5, 2.0] {
        let dir = tempfile::tempdir().unwrap();
        let samples: Vec<i16> = (0..44_100 * 3)
            .flat_map(|i| {
                if i < 44_100 {
                    [-12_000; 2]
                } else if i < 44_100 * 2 {
                    [32; 2]
                } else {
                    [12_000; 2]
                }
            })
            .collect();
        let mut backend = FakeBackend::new(caps(None, 192_000));
        backend.period = UI_PACE;
        backend.own_rate = Some(48_000);
        let (landmarks, played) = (backend.landmarks.clone(), backend.played.clone());
        let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.5 }, Box::new(backend), None);
        engine.send(Command::SetSpeed(speed));
        engine.send(Command::SetSkipSilence(true));
        engine.send(Command::Play(file_source(&dir, "pauses", &samples)));
        let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
        assert!(!seen.iter().any(|e| matches!(e, Event::Error { .. })), "{speed}×: {seen:?}");
        let seconds = played.lock().unwrap().len() as f64 / (48_000.0 * 2.0);
        assert!((seconds - 2.2 / speed as f64).abs() < 4.0 / 65.0, "{speed}×: {seconds} seconds");
        let landmarks = landmarks.lock().unwrap();
        let positive_at = landmarks.iter().find(|(_, sample)| *sample > 0.1).expect("the speech after the cut played").0;
        assert!((positive_at - 2.0).abs() < 2.0 / 65.0 + 0.02 * speed as f64, "{speed}×: landmark at {positive_at}");
        assert!((engine.status.silence_saved_secs() - 0.8).abs() < 1e-9);
        engine.shutdown(Duration::from_secs(2));
    }
}

#[test]
fn a_pause_crossing_gapless_files_is_cut_once_and_retains_both_source_clocks() {
    for speed in [1.0, 1.5, 2.0] {
        let dir = tempfile::tempdir().unwrap();
        let first = vec![32; 44_100 / 4 * 2];
        let second: Vec<_> = std::iter::repeat_n(32, 44_100 * 3 / 4 * 2).chain(voice(44_100, 1)).collect();
        let mut backend = FakeBackend::new(caps(None, 192_000));
        backend.own_rate = Some(48_000);
        backend.period = UI_PACE;
        let (played, opens, positions) = (backend.played.clone(), backend.opens.clone(), backend.positions.clone());
        let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.5 }, Box::new(backend), None);
        engine.send(Command::SetSpeed(speed));
        engine.send(Command::SetSkipSilence(true));
        engine.send(Command::SetVoiceBoost(true));
        engine.send(Command::Play(file_source(&dir, "first", &first)));
        engine.send(Command::SetNext(Some(file_source(&dir, "second", &second))));
        let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
        assert_eq!(starts(&seen), [("first".into(), false, false), ("second".into(), true, false)], "{seen:?}");
        assert_eq!(opens.load(Ordering::Relaxed), 1);
        let played = played.lock().unwrap();
        let seconds = played.len() as f64 / (48_000.0 * 2.0);
        assert!((seconds - 1.2 / speed as f64).abs() < 4.0 / 65.0, "{speed}×: {seconds} seconds");
        assert!(played.iter().all(|sample| sample.abs() <= 1.0));
        assert!((engine.status.silence_saved_secs() - 0.8).abs() < 1e-9);
        assert!(positions.lock().unwrap().iter().any(|(seq, _, source)| *seq == 1 && *source > 1.70));
        engine.shutdown(Duration::from_secs(2));
    }
}

#[test]
fn a_long_quiet_tail_drains_without_stranding_timeline_segments() {
    let dir = tempfile::tempdir().unwrap();
    let mut backend = FakeBackend::new(caps(None, 192_000));
    backend.own_rate = Some(48_000);
    let played = backend.played.clone();
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.5 }, Box::new(backend), None);
    engine.send(Command::SetSkipSilence(true));
    engine.send(Command::Play(file_source(&dir, "quiet", &vec![32; 44_100 * 20 * 2])));
    let seen = events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    assert!(!seen.iter().any(|e| matches!(e, Event::Error { .. })), "{seen:?}");
    assert!((played.lock().unwrap().len() as isize - (48_000 / 5 * 2)).abs() <= 2);
    assert!((engine.status.silence_saved_secs() - 19.8).abs() < 1e-8);
    engine.shutdown(Duration::from_secs(2));
}

#[test]
fn speech_switches_share_the_seek_handshake_and_survive_seeks_and_speed_changes() {
    let dir = tempfile::tempdir().unwrap();
    let mut backend = FakeBackend::new(caps(None, 192_000));
    backend.period = UI_PACE;
    let opens = backend.opens.clone();
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 2.0 }, Box::new(backend), None);
    engine.send(Command::PlayFrom(file_source(&dir, "speech", &voice(44_100, 12)), Duration::from_secs(1)));
    engine.send(Command::Pause);
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    for command in [Command::SetVoiceBoost(true), Command::SetSkipSilence(true), Command::SetSpeed(2.0), Command::Seek(Duration::from_secs(4))] {
        engine.send(command);
        events_until(&engine, |e| matches!(e, Event::Seeked { .. }));
        assert_eq!(engine.status.state(), PlayState::Paused);
    }
    assert!(engine.status.voice_boost() && engine.status.skip_silence());
    assert_eq!(engine.status.speed(), 2.0);
    assert_eq!(engine.status.source_position_secs(), 4.0);
    engine.send(Command::Resume);
    wait_source_position(&engine, 4.25);
    for command in [Command::SetVoiceBoost(false), Command::SetSkipSilence(false)] {
        let before = engine.status.source_position_secs();
        engine.send(command);
        let seen = events_until(&engine, |e| matches!(e, Event::Seeked { .. }));
        assert!(matches!(seen.last(), Some(Event::Seeked { position_secs, .. }) if (*position_secs - before).abs() < 0.1));
    }
    assert!(!engine.status.voice_boost() && !engine.status.skip_silence());
    assert_eq!(opens.load(Ordering::Relaxed), 1);
    engine.shutdown(Duration::from_secs(2));
}

#[test]
fn disabled_speech_processing_preserves_float_samples_bit_for_bit() {
    let dir = tempfile::tempdir().unwrap();
    let samples: Vec<f32> = (0..44_100 * 2).map(|i| if i % 2 == 0 { 0.001 } else { -0.002 }).collect();
    let backend = FakeBackend::new(caps(None, 192_000));
    let played = backend.played.clone();
    let engine = spawn(EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs: 0.5 }, Box::new(backend), None);
    engine.send(Command::SetVoiceBoost(false));
    engine.send(Command::SetSkipSilence(false));
    engine.send(Command::Play(source_of(&dir, "bypass", crate::decode::tests::wav_float(44_100, 2, &samples))));
    events_until(&engine, |e| matches!(e, Event::Ended { .. }));
    let played = played.lock().unwrap();
    assert_eq!(played.len(), samples.len());
    assert!(played.iter().zip(&samples).all(|(actual, expected)| actual.to_bits() == expected.to_bits()));
    engine.shutdown(Duration::from_secs(2));
}

#[test]
fn seeking_after_a_cut_keeps_heard_savings_and_restarts_the_source_clock() {
    let dir = tempfile::tempdir().unwrap();
    let samples: Vec<_> = voice(44_100, 1).into_iter().chain(std::iter::repeat_n(32, 44_100 * 2)).chain(voice(44_100, 8)).collect();
    let mut backend = FakeBackend::new(caps(None, 192_000));
    backend.period = UI_PACE;
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.5 }, Box::new(backend), None);
    engine.send(Command::SetSkipSilence(true));
    engine.send(Command::Play(file_source(&dir, "seek-cut", &samples)));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    wait_source_position(&engine, 2.2);
    engine.send(Command::Pause);
    engine.send(Command::RefreshDevices);
    events_until(&engine, |e| matches!(e, Event::Devices { .. }));
    let saved = engine.status.silence_saved_secs();
    assert!((saved - 0.8).abs() < 1e-9);
    engine.send(Command::Seek(Duration::from_secs(4)));
    events_until(&engine, |e| matches!(e, Event::Seeked { .. }));
    engine.send(Command::SetSpeed(1.5));
    events_until(&engine, |e| matches!(e, Event::Seeked { .. }));
    assert_eq!(engine.status.source_position_secs(), 4.0);
    assert_eq!(engine.status.silence_saved_secs(), saved);
    engine.send(Command::Resume);
    wait_source_position(&engine, 4.25);
    assert_eq!(engine.status.silence_saved_secs(), saved);
    engine.shutdown(Duration::from_secs(2));
}
