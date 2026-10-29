//! Engine-level tests with a fake device that runs the real callback on a thread.

use super::*;
use crate::decode::tests::{file_source, pcm, slack, wait_late};
use std::sync::Mutex;
use std::time::Instant;

/// Plays rings at roughly 8× real time and records what it played.
struct FakeBackend {
    caps: DeviceCaps,
    played: Arc<Mutex<Vec<f32>>>,
    stop: Option<(Arc<AtomicBool>, thread::JoinHandle<()>)>,
    drained: Arc<AtomicBool>,
    opens: Arc<AtomicU64>,
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
            stop: None,
            drained: Arc::default(),
            opens: Arc::default(),
            devices: Arc::default(),
            device_events: Arc::default(),
            selected: None,
            open_dev: None,
            period: (Duration::from_millis(1), 512),
            own_rate: None,
            dropped: Arc::default(),
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
        if let Some(rate) = self.own_rate {
            caps.configs = caps.configs.iter().map(|c| RateRange { min: rate, max: rate, ..*c }).collect();
        }
        Some(caps)
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

    fn open(&mut self, format: StreamFormat, mut input: RingInput, status: Arc<Status>) -> Result<(), OutputError> {
        self.close();
        if !self.caps.supports(format.sample_rate, format.channels) {
            return Err(OutputError::Unsupported { rate: format.sample_rate, channels: format.channels });
        }
        self.opens.fetch_add(1, Ordering::Relaxed);
        self.open_dev = self.target().map(|d| d.uid);
        let stop = Arc::new(AtomicBool::new(false));
        let drained = Arc::new(AtomicBool::new(false));
        self.drained = drained.clone();
        let (played, stop2) = (self.played.clone(), stop.clone());
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
                if input.fill_for_device(&mut buf, channels, format.sample_rate, &status, device_bits, &mut dither) {
                    drained.store(true, Ordering::Release);
                }
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
        self.open_dev = None;
        if let Some((stop, handle)) = self.stop.take() {
            stop.store(true, Ordering::Relaxed);
            let _ = handle.join();
        }
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
    assert_eq!(path.bit_perfect, BitPerfectStatus::Unavailable { reason: "device max 16/48, file is 16/96".into() });
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
    // Give the decoder time to finish "a" and move on to "b".
    thread::sleep(Duration::from_millis(200));
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
    // The decoder is on the second copy by now: this seek restarts the first.
    thread::sleep(Duration::from_millis(200));
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
    // All of A is decoded within a few ms; by now the decoder is opening B.
    thread::sleep(Duration::from_millis(50));
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
    // Both fit the buffer: by now the decoder is done with A and on B, so the
    // seek restarts A past its end, which has no audio left: B takes over.
    thread::sleep(Duration::from_millis(100));
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
    let (tracks, expected): (Vec<_>, Vec<_>) =
        depths.iter().enumerate().map(|(i, bits)| track_of_depth(&dir, &format!("t{i}"), *bits, 60 + i as i64)).unzip();
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
    let (tracks, sources): (Vec<_>, Vec<_>) =
        depths.iter().enumerate().map(|(i, bits)| track_of_depth(&dir, &format!("t{i}"), *bits, 70 + i as i64)).unzip();
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
    assert_eq!(verdict, BitPerfectStatus::Unavailable { reason: "device max 16/192, file is 24/44.1".into() });
}

/// Needs real hardware: the whole engine on this platform's own backend, on the
/// device named in `OND_TEST_DEVICE` (by UID; the default without it), in
/// bit-perfect mode as the app drives it, playing near-silence for
/// `OND_TEST_SECS` (5). With `OND_TEST_SWITCH` set it starts in Normal and
/// switches mid-track, as the picker does. Prints the events and position.
#[test]
#[ignore = "needs audio hardware"]
fn engine_on_a_real_device() {
    let uid = std::env::var("OND_TEST_DEVICE").ok();
    let secs: u64 = std::env::var("OND_TEST_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(5);
    let switch = std::env::var_os("OND_TEST_SWITCH").is_some();
    let dir = tempfile::tempdir().unwrap();
    let quiet = vec![1i16; 44_100 * 2 * (secs as usize + 2)];
    let engine = spawn(EngineConfig::default(), default_backend(), None);
    let first = if switch { PlaybackMode::Normal } else { PlaybackMode::BitPerfect };
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
            println!("{:>5} ms  -- SetMode(BitPerfect)", start.elapsed().as_millis());
            engine.send(Command::SetMode(PlaybackMode::BitPerfect));
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
