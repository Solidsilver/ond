//! Engine-level tests with a fake device that runs the real callback on a thread.

use super::*;
use crate::decode::tests::{file_source, pcm, slack};
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
        Some(self.caps.clone())
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
        let handle = thread::spawn(move || {
            let mut buf = vec![0.0f32; 512 * channels];
            while !stop2.load(Ordering::Relaxed) {
                if input.fill(&mut buf, channels, format.sample_rate, &status) {
                    drained.store(true, Ordering::Release);
                }
                played.lock().unwrap().extend(buf.iter().filter(|s| **s != 0.0));
                thread::sleep(Duration::from_millis(1));
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

    // Picking the DAC applies its default mode (bit-perfect: it's a wired DAC).
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
    *devices.lock().unwrap() = vec![device("spk", "Speakers", DeviceKind::BuiltInSpeakers), device("dac", "DAC", DeviceKind::Usb)];
    let engine = spawn(EngineConfig { mode: PlaybackMode::Normal, buffer_secs: 0.1 }, Box::new(backend), None);
    // The user plays the speakers bit-perfect, the DAC in Normal.
    engine.send(Command::DeviceModes(HashMap::from([("spk".to_owned(), PlaybackMode::BitPerfect), ("dac".to_owned(), PlaybackMode::Normal)])));
    engine.send(Command::SelectDevice(Some("dac".into())));
    engine.send(Command::Play(file_source(&dir, "a", &a)));
    events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    assert_eq!(engine.status.mode(), PlaybackMode::Normal);
    devices.lock().unwrap().retain(|d| d.uid != "dac");
    events.lock().unwrap().push(DeviceEvent::ListChanged);
    let seen = events_until(&engine, |e| matches!(e, Event::TrackStarted { .. }));
    assert!(seen.iter().any(|e| matches!(e, Event::DeviceLost { name } if name == "DAC")));
    assert!(seen.iter().any(|e| matches!(e, Event::ModeChanged { mode: PlaybackMode::BitPerfect, device: Some(d) } if d == "spk")), "{seen:?}");
    assert_eq!(engine.status.mode(), PlaybackMode::BitPerfect);
    engine.send(Command::Shutdown);
}

#[test]
fn remembered_modes_win_over_device_defaults() {
    let backend = FakeBackend::new(caps(Some(24), 192_000));
    *backend.devices.lock().unwrap() = vec![device("spk", "Speakers", DeviceKind::BuiltInSpeakers), device("dac", "DAC", DeviceKind::Usb)];
    let engine = spawn(EngineConfig::default(), Box::new(backend), None);
    engine.send(Command::DeviceModes(HashMap::from([("dac".to_owned(), PlaybackMode::Normal), ("spk".to_owned(), PlaybackMode::BitPerfect)])));
    engine.send(Command::SelectDevice(Some("spk".into())));
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

