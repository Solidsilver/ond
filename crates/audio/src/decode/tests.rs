use super::*;
use crate::PlaybackMode;
use crate::resample::StreamResampler;

const RATE: u32 = 44_100;

/// Minimal 16-bit PCM WAV.
pub(crate) fn wav(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    v.extend_from_slice(&(channels * 2).to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    samples.iter().for_each(|s| v.extend_from_slice(&s.to_le_bytes()));
    v
}

/// A test signal that never contains an exact zero, so silence is unambiguous.
/// `salt` makes different tracks distinguishable.
pub(crate) fn pcm(n: usize, salt: i64) -> Vec<i16> {
    (0..n as i64)
        .map(|i| {
            let v = ((i * 7919 + salt * 104_729) % 65_536 - 32_768) as i16;
            if v == 0 { 1 } else { v }
        })
        .collect()
}

pub(crate) fn file_source(dir: &tempfile::TempDir, name: &str, samples: &[i16]) -> TrackSource {
    let path = dir.path().join(format!("{name}.wav"));
    std::fs::write(&path, wav(RATE, 2, samples)).unwrap();
    TrackSource { id: name.into(), url: format!("file://{}", path.display()), suffix: Some("wav".into()), ..Default::default() }
}

/// A file that can only be read forward (`test-unseekable://` sources).
pub(crate) struct Unseekable(pub std::fs::File);

impl Read for Unseekable {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl Seek for Unseekable {
    fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "unseekable"))
    }
}

impl MediaSource for Unseekable {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

fn config(buffer_secs: f32) -> EngineConfig {
    EngineConfig { mode: PlaybackMode::BitPerfect, buffer_secs }
}

fn bit_perfect_status() -> Arc<Status> {
    let s = Arc::new(Status::default());
    s.bit_perfect.store(true, Ordering::Relaxed);
    s
}

fn to_i16(s: f32) -> i16 {
    (s * 32_768.0) as i16
}

/// Stands in for the device: calls the realtime callback and keeps what it played.
struct Device {
    input: RingInput,
    status: Arc<Status>,
    /// Non-silent samples, in play order.
    played: Vec<i16>,
}

impl Device {
    /// One callback of `frames` stereo frames. Returns (drained, non-silent samples written).
    fn tick(&mut self, frames: usize) -> (bool, usize) {
        let mut buf = vec![0.0f32; frames * 2];
        let drained = self.input.fill(&mut buf, 2, RATE, &self.status);
        let before = self.played.len();
        self.played.extend(buf.iter().filter(|s| **s != 0.0).map(|s| to_i16(*s)));
        (drained, self.played.len() - before)
    }
}

/// Waits for the session's first ring; returns it with every other message seen.
fn wait_ring(s: &Session) -> (RingInput, Vec<DecoderMsg>) {
    let deadline = Instant::now() + slack(Duration::from_secs(5));
    let mut seen = Vec::new();
    loop {
        match s.try_msg() {
            Some(DecoderMsg::Ring { input, .. }) => return (input, seen),
            Some(DecoderMsg::Error { message, .. }) => panic!("decoder error: {message}"),
            Some(m) => seen.push(m),
            None => {
                assert!(Instant::now() < deadline, "decoder never produced a ring");
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

fn wait_seeked(s: &Session, dev: &mut Device) -> f64 {
    let deadline = Instant::now() + slack(Duration::from_secs(5));
    loop {
        assert!(Instant::now() < deadline, "no Seeked message");
        dev.tick(64);
        match s.try_msg() {
            Some(DecoderMsg::Seeked { position_secs, .. }) => return position_secs,
            Some(DecoderMsg::Error { message, .. }) => panic!("decoder error: {message}"),
            _ => thread::sleep(Duration::from_millis(1)),
        }
    }
}

fn play_to_end(s: &Session, dev: &mut Device) {
    let deadline = Instant::now() + slack(Duration::from_secs(10));
    loop {
        assert!(Instant::now() < deadline, "playback stalled");
        let (drained, _) = dev.tick(512);
        if drained && s.finished() {
            return;
        }
        thread::sleep(Duration::from_micros(200));
    }
}

#[test]
fn bit_perfect_decode_round_trips_pcm() {
    let a = pcm(20_000, 0);
    let dir = tempfile::tempdir().unwrap();
    let status = bit_perfect_status();
    let s = Session::start(file_source(&dir, "a", &a), None, config(0.05), status.clone(), None, None, Arc::default());
    let (input, seen) = wait_ring(&s);
    let Some(DecoderMsg::Track { format, total_frames, .. }) = seen.first() else { panic!("no Track message: {seen:?}") };
    assert_eq!(*format, StreamFormat { sample_rate: RATE, channels: 2, bits_per_sample: Some(16) });
    assert_eq!(*total_frames, Some(10_000));

    let mut dev = Device { input, status, played: Vec::new() };
    play_to_end(&s, &mut dev);
    assert!(s.try_msg().is_none());
    assert_eq!(dev.played, a);
}

#[test]
fn gapless_two_tracks_share_one_ring_and_switch_on_the_exact_sample() {
    // Odd lengths so the boundary falls mid-callback.
    let (a, b) = (pcm(2 * 30_001, 1), pcm(2 * 17_003, 2));
    let dir = tempfile::tempdir().unwrap();
    let status = bit_perfect_status();
    let s = Session::start(file_source(&dir, "a", &a), None, config(0.05), status.clone(), None, None, Arc::default());
    s.set_next(0, Some(file_source(&dir, "b", &b)));
    let (input, _) = wait_ring(&s);
    let mut dev = Device { input, status: status.clone(), played: Vec::new() };

    let mut boundary_at = None;
    let deadline = Instant::now() + slack(Duration::from_secs(10));
    loop {
        assert!(Instant::now() < deadline, "playback stalled");
        let (drained, _) = dev.tick(333);
        if boundary_at.is_none() && status.track_seq.load(Ordering::Relaxed) == 1 {
            // frames_played now counts B's frames in this callback only.
            boundary_at = Some(dev.played.len() as u64 / 2 - status.frames_played.load(Ordering::Relaxed));
        }
        if drained && s.finished() {
            break;
        }
        thread::sleep(Duration::from_micros(200));
    }
    assert_eq!(boundary_at, Some(30_001), "B is reported exactly when its first frame plays");
    let expected: Vec<i16> = a.iter().chain(&b).copied().collect();
    assert_eq!(dev.played.len(), expected.len(), "no gap, no overlap");
    assert!(dev.played == expected, "samples differ from A followed by B");

    let mut tracks = Vec::new();
    while let Some(m) = s.try_msg() {
        match m {
            DecoderMsg::Track { seq, src, .. } => tracks.push((seq, src.id)),
            DecoderMsg::Ring { .. } => panic!("a second ring means the device would reopen"),
            _ => {}
        }
    }
    assert_eq!(tracks, [(1, "b".to_owned())]);
}

#[test]
fn format_change_starts_a_new_ring() {
    // Longer than the ring, so `b` is queued before `a` can finish decoding.
    let a = pcm(2 * 30_000, 1);
    let dir = tempfile::tempdir().unwrap();
    let b_path = dir.path().join("b.wav");
    std::fs::write(&b_path, wav(48_000, 2, &pcm(2 * 4_000, 2))).unwrap();
    let b = TrackSource { id: "b".into(), url: format!("file://{}", b_path.display()), suffix: Some("wav".into()), ..Default::default() };
    let status = bit_perfect_status();
    let s = Session::start(file_source(&dir, "a", &a), None, config(0.05), status.clone(), None, None, Arc::default());
    s.set_next(0, Some(b));
    let (input, _) = wait_ring(&s);
    let mut dev = Device { input, status, played: Vec::new() };
    let deadline = Instant::now() + slack(Duration::from_secs(5));
    let second = loop {
        assert!(Instant::now() < deadline, "no second ring");
        dev.tick(512);
        if let Some(DecoderMsg::Ring { format, first_seq, .. }) = s.try_msg() {
            break (format.sample_rate, first_seq);
        }
        thread::sleep(Duration::from_micros(200));
    };
    assert_eq!(second, (48_000, 1));
    assert_eq!(dev.played, a, "the first ring played out completely before the switch");
}

#[test]
fn seek_cuts_stale_audio_and_resumes_at_the_exact_frame() {
    // 2 s of stereo; the ring fills long before the end, so the decoder is mid-file.
    let a = pcm(RATE as usize * 2 * 2, 0);
    let dir = tempfile::tempdir().unwrap();
    let status = bit_perfect_status();
    let s = Session::start(file_source(&dir, "a", &a), None, config(0.05), status.clone(), None, None, Arc::default());
    let (input, _) = wait_ring(&s);
    let mut dev = Device { input, status: status.clone(), played: Vec::new() };
    while dev.played.len() < 2_000 {
        dev.tick(256);
    }

    let hold = dev.input.ctl.hold();
    let t = Instant::now();
    s.seek(0, Duration::from_millis(1_500), hold);
    let before = dev.played.len();
    let position = wait_seeked(&s, &mut dev);
    assert_eq!(position, 1.5);
    // The decoder set the position to the target; a cached seek may already have played on.
    assert!((66_150..66_150 + 4_096).contains(&status.frames_played.load(Ordering::Relaxed)));
    // Cached data: playback resumes without a noticeable wait.
    while dev.played.len() == before {
        assert!(t.elapsed() < slack(Duration::from_millis(50)), "cached seek took {:?}", t.elapsed());
        dev.tick(64);
    }
    play_to_end(&s, &mut dev);
    assert!(dev.played[before..] == a[66_150 * 2..], "after the seek only post-target audio plays");
}

/// A device that only runs at 48 kHz (stereo, 16-bit), like a UMC22 asked for 96 kHz.
fn caps_48k() -> Option<DeviceCaps> {
    Some(DeviceCaps {
        configs: vec![crate::signal::RateRange { channels: 2, min: 48_000, max: 48_000 }],
        max_bits: Some(16),
        ..Default::default()
    })
}

fn source_at(dir: &tempfile::TempDir, name: &str, rate: u32, samples: &[i16]) -> TrackSource {
    let path = dir.path().join(format!("{name}.wav"));
    std::fs::write(&path, wav(rate, 2, samples)).unwrap();
    TrackSource { id: name.into(), url: format!("file://{}", path.display()), suffix: Some("wav".into()), ..Default::default() }
}

/// Reference: what the resampler makes of `samples` in one go.
fn resampled(from: u32, to: u32, samples: &[i16]) -> Vec<f32> {
    let input: Vec<f32> = samples.iter().map(|s| *s as f32 / 32_768.0).collect();
    let mut r = StreamResampler::new(from, to, 2).unwrap();
    let mut out = Vec::new();
    r.process(&input, &mut out);
    r.flush(&mut out);
    out.retain(|s| *s != 0.0);
    out
}

/// Plays a ring as f32 (resampled audio isn't on the 16-bit grid). Returns the
/// non-silent samples and, if a track boundary was crossed, its output frame.
fn play_f32(s: &Session, mut input: RingInput, status: &Status, rate: u32) -> (Vec<f32>, Option<u64>) {
    let (mut played, mut frames, mut boundary) = (Vec::new(), 0u64, None);
    let mut buf = vec![0.0f32; 2 * 401];
    let deadline = Instant::now() + slack(Duration::from_secs(20));
    loop {
        assert!(Instant::now() < deadline, "playback stalled");
        let seq = status.track_seq.load(Ordering::Relaxed);
        let drained = input.fill(&mut buf, 2, rate, status);
        let fresh: Vec<f32> = buf.iter().copied().filter(|v| *v != 0.0).collect();
        if boundary.is_none() && status.track_seq.load(Ordering::Relaxed) > seq {
            boundary = Some(frames + fresh.len() as u64 / 2 - status.frames_played.load(Ordering::Relaxed));
        }
        frames += fresh.len() as u64 / 2;
        played.extend(fresh);
        if drained && s.finished() {
            return (played, boundary);
        }
        thread::sleep(Duration::from_micros(200));
    }
}

#[test]
fn hi_res_tracks_are_resampled_for_the_device_and_stay_gapless() {
    // Two 96 kHz tracks on a 48 kHz-only device. Odd lengths put the boundary mid-chunk.
    let (a, b) = (pcm(2 * 48_001, 3), pcm(2 * 24_003, 4));
    let dir = tempfile::tempdir().unwrap();
    let status = bit_perfect_status();
    let s = Session::start(source_at(&dir, "a", 96_000, &a), None, config(0.1), status.clone(), None, caps_48k(), Arc::default());
    s.set_next(0, Some(source_at(&dir, "b", 96_000, &b)));
    let (input, seen) = wait_ring(&s);
    let Some(DecoderMsg::Track { format, output, total_frames, .. }) = seen.first() else { panic!("no Track: {seen:?}") };
    assert_eq!((format.sample_rate, output.sample_rate), (96_000, 48_000));
    assert_eq!(*total_frames, Some(24_001), "duration counted at the output rate");

    let (played, boundary) = play_f32(&s, input, &status, 48_000);
    let joined: Vec<i16> = a.iter().chain(&b).copied().collect();
    let expected = resampled(96_000, 48_000, &joined);
    assert_eq!(played.len(), expected.len(), "one continuous stream: no gap, no overlap");
    assert!(played == expected, "same samples as resampling A and B as one signal");
    assert_eq!(boundary, Some(24_001), "B starts at round(48001 / 2)");
    while let Some(m) = s.try_msg() {
        assert!(!matches!(m, DecoderMsg::Ring { .. }), "the device would reopen");
    }
}

#[test]
fn seek_with_resampling_restarts_cleanly_at_the_target() {
    let a = pcm(96_000 * 2 * 2, 5);
    let dir = tempfile::tempdir().unwrap();
    let status = bit_perfect_status();
    let s = Session::start(source_at(&dir, "a", 96_000, &a), None, config(0.1), status.clone(), None, caps_48k(), Arc::default());
    let (mut input, _) = wait_ring(&s);
    let mut buf = vec![0.0f32; 2 * 256];
    for _ in 0..20 {
        input.fill(&mut buf, 2, 48_000, &status);
    }
    let hold = input.ctl.hold();
    s.seek(0, Duration::from_millis(1_500), hold);
    let deadline = Instant::now() + slack(Duration::from_secs(5));
    loop {
        assert!(Instant::now() < deadline, "no Seeked");
        input.fill(&mut buf, 2, 48_000, &status);
        if let Some(DecoderMsg::Seeked { position_secs, .. }) = s.try_msg() {
            assert_eq!(position_secs, 1.5);
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!((72_000..72_000 + 2_048).contains(&status.frames_played.load(Ordering::Relaxed)), "position at 48 kHz");
    // Everything played from here on is the resampled tail, starting from a fresh filter.
    let (rest, _) = play_f32(&s, input, &status, 48_000);
    let expected = resampled(96_000, 48_000, &a[144_000 * 2..]);
    let skipped = expected.len() - rest.len();
    assert!(skipped < 2 * 2_048, "only what played between Seeked and here is missing ({skipped})");
    assert!(rest == expected[skipped..], "post-seek audio is the resampled tail");
}

/// Serves `body` over HTTP with Range support, 16 KB every 30 ms.
pub(crate) fn slow_server(body: Vec<u8>) -> String {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/rest/stream", listener.local_addr().unwrap());
    let body = Arc::new(body);
    thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let body = body.clone();
            thread::spawn(move || {
                let mut reader = BufReader::new(conn.try_clone().unwrap());
                let mut start = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        start = v.trim().trim_end_matches('-').parse().unwrap_or(0);
                    }
                }
                let len = body.len();
                let mut out = conn;
                let head = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{len}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    len - 1,
                    len - start
                );
                if out.write_all(head.as_bytes()).is_err() {
                    return;
                }
                for chunk in body[start..].chunks(16 * 1024) {
                    if out.write_all(chunk).is_err() {
                        return;
                    }
                    thread::sleep(Duration::from_millis(30));
                }
            });
        }
    });
    url
}

#[test]
fn seek_beyond_the_download_holds_silence_then_buffers_before_playing() {
    // 4 s of stereo, ~700 KB: ~1.3 s to arrive sequentially at this server's pace.
    let a = pcm(RATE as usize * 2 * 4, 0);
    let url = slow_server(wav(RATE, 2, &a));
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = StreamCache::new(streamcache::CacheConfig::new(dir.path()), rt.handle().clone()).unwrap();
    let status = bit_perfect_status();
    let src = TrackSource { id: "net".into(), url, suffix: Some("wav".into()), ..Default::default() };
    // A 1 s ring so the 0.4 s prebuffer target isn't capped by its size.
    let s = Session::start(src, None, config(1.0), status.clone(), Some(cache), None, Arc::default());
    let (input, _) = wait_ring(&s);
    let capacity = input.samples.buffer().capacity();
    let target = ((crate::PREBUFFER_SECS * RATE as f32) as usize * 2).min(capacity / 2);
    let mut dev = Device { input, status: status.clone(), played: Vec::new() };
    // Play a little of the start (it buffers first, like any track start).
    let deadline = Instant::now() + slack(Duration::from_secs(20));
    while dev.played.len() < 4_000 {
        assert!(Instant::now() < deadline, "never started");
        dev.tick(256);
        thread::sleep(Duration::from_millis(1));
    }

    let t = Instant::now();
    let hold = dev.input.ctl.hold();
    s.seek(0, Duration::from_secs(3), hold);
    let before = dev.played.len();
    // From here on, nothing from before the target may play: the stale ring is cut at once.
    let mut resumed_with = None;
    let deadline = t + Duration::from_secs(20);
    while !(s.finished() && dev.input.samples.is_empty() && dev.input.input_done.load(Ordering::Acquire)) {
        assert!(Instant::now() < deadline, "stalled after seek: {:?}", s.try_msg());
        let queued = dev.input.samples.slots();
        let done = dev.input.input_done.load(Ordering::Acquire);
        let (_, wrote) = dev.tick(256);
        if wrote > 0 && resumed_with.is_none() {
            resumed_with = Some((queued, done));
        }
        thread::sleep(Duration::from_millis(1));
    }
    let (queued, done) = resumed_with.expect("never resumed");
    assert!(queued >= target || done, "resumed with {queued} samples queued, target {target}");
    // Without a ranged jump this would wait for ~1 s of sequential download.
    assert!(dev.played[before..] == a[132_300 * 2..], "only audio from the seek target on");
}

fn wav_source(dir: &tempfile::TempDir, name: &str, channels: u16, samples: &[i16]) -> TrackSource {
    let path = dir.path().join(format!("{name}.wav"));
    std::fs::write(&path, wav(RATE, channels, samples)).unwrap();
    TrackSource { id: name.into(), url: format!("file://{}", path.display()), suffix: Some("wav".into()), ..Default::default() }
}

#[test]
fn mono_plays_on_both_channels_sample_exact() {
    let a = pcm(6_000, 3);
    let dir = tempfile::tempdir().unwrap();
    let status = bit_perfect_status();
    let s = Session::start(wav_source(&dir, "m", 1, &a), None, config(0.05), status.clone(), None, None, Arc::default());
    let (input, seen) = wait_ring(&s);
    let Some(DecoderMsg::Track { format, output, channels, .. }) = seen.first() else { panic!("no Track: {seen:?}") };
    assert_eq!((format.channels, output.channels), (1, 2));
    assert_eq!(*channels, ChannelChange::Upmix { from: 1, to: 2 });
    let mut dev = Device { input, status, played: Vec::new() };
    play_to_end(&s, &mut dev);
    let doubled: Vec<i16> = a.iter().flat_map(|s| [*s, *s]).collect();
    assert_eq!(dev.played, doubled);
}

#[test]
fn surround_is_downmixed_to_stereo() {
    // 5.1 frames with only the front left and the LFE carrying signal.
    let frames = 4_000;
    let samples: Vec<i16> = (0..frames).flat_map(|_| [16_000i16, 0, 0, 20_000, 0, 0]).collect();
    let dir = tempfile::tempdir().unwrap();
    let status = bit_perfect_status();
    let s = Session::start(wav_source(&dir, "s", 6, &samples), None, config(0.05), status.clone(), None, None, Arc::default());
    let (mut input, seen) = wait_ring(&s);
    let Some(DecoderMsg::Track { output, channels, .. }) = seen.first() else { panic!("no Track: {seen:?}") };
    assert_eq!(output.channels, 2);
    assert_eq!(*channels, ChannelChange::Downmix { from: 6, to: 2 });
    let mut played = Vec::new();
    let deadline = Instant::now() + slack(Duration::from_secs(5));
    while played.len() < frames * 2 {
        assert!(Instant::now() < deadline, "playback stalled");
        let mut buf = vec![0.0f32; 512];
        input.fill(&mut buf, 2, RATE, &status);
        played.extend(buf.as_chunks::<2>().0.iter().filter(|f| f[0] != 0.0).flatten().copied());
        thread::sleep(Duration::from_micros(200));
    }
    let left = 16_000.0 / 32_768.0 / (1.0 + 2.0 * std::f32::consts::FRAC_1_SQRT_2);
    for f in played.as_chunks::<2>().0 {
        assert!((f[0] - left).abs() < 1e-6, "left {f:?}");
        assert_eq!(f[1], 0.0, "the LFE is dropped and nothing else reaches the right: {f:?}");
    }
}

#[test]
fn each_track_gets_its_gain_before_its_first_sample() {
    use crate::replaygain::{GainMode, db_to_linear};
    let dir = tempfile::tempdir().unwrap();
    // Longer than the ring, so the decoder waits on the device and can't
    // finish `a` before `b` is queued.
    let mut a = file_source(&dir, "a", &pcm(2 * 30_000, 1));
    a.replay_gain = ReplayGain { track_gain: Some(-6.0), album_gain: Some(-9.0), ..Default::default() };
    let mut b = file_source(&dir, "b", &pcm(2 * 3_000, 2));
    b.replay_gain = ReplayGain { track_gain: Some(-12.0), album_gain: Some(-9.0), ..Default::default() };
    b.album_context = true;
    let status = bit_perfect_status();
    let gain = Arc::new(Mutex::new(GainConfig { mode: GainMode::Auto, ..Default::default() }));
    let s = Session::start(a, None, config(0.05), status.clone(), None, None, gain);
    s.set_next(0, Some(b));
    let (input, mut seen) = wait_ring(&s);
    let mut dev = Device { input, status: status.clone(), played: Vec::new() };
    let deadline = Instant::now() + slack(Duration::from_secs(5));
    loop {
        assert!(Instant::now() < deadline, "playback stalled");
        seen.extend(std::iter::from_fn(|| s.try_msg()));
        if dev.tick(512).0 && s.finished() {
            break;
        }
        thread::sleep(Duration::from_micros(200));
    }
    seen.extend(std::iter::from_fn(|| s.try_msg()));
    // Auto: a plays alone (track gain), b as part of its album (album gain).
    assert!((status.gains.get(0) - db_to_linear(-6.0)).abs() < 1e-6);
    assert!((status.gains.get(1) - db_to_linear(-9.0)).abs() < 1e-6);
    let gains: Vec<ReplayGain> = seen
        .iter()
        .filter_map(|m| if let DecoderMsg::Track { replay_gain, .. } = m { Some(*replay_gain) } else { None })
        .collect();
    assert_eq!(gains.len(), 2);
    assert_eq!(gains[1].track_gain, Some(-12.0));
}

#[test]
fn replay_gain_tags_are_read_by_standard_or_raw_key() {
    use symphonia::core::meta::RawTag;
    let tags = [
        Tag {
            raw: RawTag::new("REPLAYGAIN_TRACK_GAIN", "-7.50 dB"),
            std: Some(StandardTag::ReplayGainTrackGain(Arc::new("-7.50 dB".into()))),
        },
        Tag { raw: RawTag::new("TXXX:replaygain_album_gain", "-8.25 dB"), std: None },
        Tag { raw: RawTag::new("----:com.apple.iTunes:replaygain_track_peak", "0.988"), std: None },
        Tag { raw: RawTag::new("ARTIST", "x"), std: None },
    ];
    let rg = replay_gain_from_tags(tags.iter());
    assert_eq!(rg, ReplayGain { track_gain: Some(-7.5), album_gain: Some(-8.25), track_peak: Some(0.988), album_peak: None });
}

/// Time budgets in these tests are tight on purpose (they guard real
/// performance), but shared CI runners are slower and noisier: allow 10× there.
pub(crate) fn slack(d: std::time::Duration) -> std::time::Duration {
    if std::env::var_os("CI").is_some() { d * 10 } else { d }
}

#[test]
fn a_seek_interrupts_a_push_on_a_frame_boundary() {
    let (mut p, c) = RingBuffer::new(16);
    let (cancel, seek) = (AtomicBool::new(false), AtomicBool::new(true));
    // Half a stereo frame is in: the other half goes in before the push gives way.
    let mut pushed = 3;
    assert!(matches!(push_all(&mut p, &[0.5; 8], 2, &mut pushed, &cancel, &seek), Push::Interrupted));
    assert_eq!((pushed, c.slots()), (4, 1));
    assert!(matches!(push_all(&mut p, &[0.5; 8], 2, &mut pushed, &cancel, &seek), Push::Interrupted));
    assert_eq!(pushed, 4, "on a boundary it stops at once");
    seek.store(false, Ordering::Relaxed);
    assert!(matches!(push_all(&mut p, &[0.5; 8], 2, &mut pushed, &cancel, &seek), Push::Done));
    assert_eq!(pushed, 12);
}

#[test]
fn a_track_shorter_than_the_prebuffer_plays_out() {
    // 0.25 s: more than the decoder waits at the end for a next track to be
    // named (0.1 s), less than the output prebuffers (0.4 s) before it plays.
    let a = pcm(RATE as usize / 4 * 2, 5);
    let dir = tempfile::tempdir().unwrap();
    let status = bit_perfect_status();
    let s = Session::start(file_source(&dir, "a", &a), None, config(1.0), status.clone(), None, None, Arc::default());
    let (input, _) = wait_ring(&s);
    let mut dev = Device { input, status, played: Vec::new() };
    play_to_end(&s, &mut dev);
    assert_eq!(dev.played, a);
}
