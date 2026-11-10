use super::*;
use crate::PlaybackMode;
use crate::resample::StreamResampler;
use testkit::{Deadline, RangeServer, Serve, slack};

const RATE: u32 = 44_100;

/// Minimal 16-bit PCM WAV.
pub(crate) fn wav(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    wav_of(1, 16, rate, channels, &data)
}

/// Minimal 24-bit PCM WAV (samples in the 24-bit range).
pub(crate) fn wav24(rate: u32, channels: u16, samples: &[i32]) -> Vec<u8> {
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()[..3].to_vec()).collect();
    wav_of(1, 24, rate, channels, &data)
}

/// Minimal 32-bit float WAV: a source without a bit depth, as a lossy one is.
pub(crate) fn wav_float(rate: u32, channels: u16, samples: &[f32]) -> Vec<u8> {
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    wav_of(3, 32, rate, channels, &data)
}

fn wav_of(format_tag: u16, bits: u16, rate: u32, channels: u16, data: &[u8]) -> Vec<u8> {
    let block = channels * bits / 8;
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&format_tag.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * block as u32).to_le_bytes());
    v.extend_from_slice(&block.to_le_bytes());
    v.extend_from_slice(&bits.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend_from_slice(data);
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
    let deadline = Deadline::new();
    let mut seen = Vec::new();
    loop {
        match s.try_msg() {
            Some(DecoderMsg::Ring { input, .. }) => return (input, seen),
            Some(DecoderMsg::Error { message, .. }) => panic!("decoder error: {message}"),
            Some(m) => seen.push(m),
            None => {
                assert!(!deadline.passed(), "decoder never produced a ring");
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

fn wait_seeked(s: &Session, dev: &mut Device) -> f64 {
    let deadline = Deadline::new();
    loop {
        assert!(!deadline.passed(), "no Seeked message");
        dev.tick(64);
        match s.try_msg() {
            Some(DecoderMsg::Seeked { position_secs, .. }) => return position_secs,
            Some(DecoderMsg::Error { message, .. }) => panic!("decoder error: {message}"),
            _ => thread::sleep(Duration::from_millis(1)),
        }
    }
}

fn play_to_end(s: &Session, dev: &mut Device) {
    let deadline = Deadline::new();
    loop {
        assert!(!deadline.passed(), "playback stalled");
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
    let deadline = Deadline::new();
    loop {
        assert!(!deadline.passed(), "playback stalled");
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
    let deadline = Deadline::new();
    let second = loop {
        assert!(!deadline.passed(), "no second ring");
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
fn a_cut_quiet_tail_at_a_format_change_carries_its_savings_to_the_next_ring() {
    // Half a second of tone, then a long pause that ends the file: the pause's
    // savings belong to the ring the format change closes, and the next ring
    // has no way to recover time the old one saved. A callback frees the old
    // ring's last slots a moment before it consumes its endpoint segment, which
    // is where that total is published; here the device holds the timeline back
    // so the decoder makes the new ring in exactly that window.
    let mut a = pcm(RATE as usize, 1);
    a.extend(std::iter::repeat_n(32i16, RATE as usize * 10 * 2));
    let dir = tempfile::tempdir().unwrap();
    let status = bit_perfect_status();
    status.skip_silence.store(true, Ordering::Relaxed);
    let s = Session::start(file_source(&dir, "a", &a), None, config(0.05), status.clone(), None, None, Arc::default());
    s.set_next(0, Some(source_at(&dir, "b", 48_000, &pcm(2 * 4_000, 2))));
    let (input, _) = wait_ring(&s);
    let mut dev = Device { input, status: status.clone(), played: Vec::new() };
    let held = dev.input.hold_timeline().expect("a timeline");
    // Play the first ring out, freeing every slot, with nothing published.
    let deadline = Deadline::new();
    while !dev.tick(512).0 {
        assert!(!deadline.passed(), "the first ring never drained");
        thread::sleep(Duration::from_micros(200));
    }
    let deadline = Deadline::new();
    let input = loop {
        if let Some(DecoderMsg::Ring { input, .. }) = s.try_msg() {
            break input;
        }
        assert!(!deadline.passed(), "no second ring");
        thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(status.silence_saved_secs(), 0.0, "the old ring's endpoint is still held");
    // The next callback publishes it, as a real device one buffer later would.
    dev.input.resume_timeline(held);
    dev.tick(64);
    assert!((status.silence_saved_secs() - 9.8).abs() < 1e-9, "saved {}", status.silence_saved_secs());
    // The new ring must carry that total, not whatever `Status` said when it
    // was made: its own segments overwrite the status as it plays.
    play_f32(&s, input, &status, 48_000);
    assert!((status.silence_saved_secs() - 9.8).abs() < 1e-9, "saved {}", status.silence_saved_secs());
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
    Some(DeviceCaps { configs: vec![crate::signal::RateRange { channels: 2, min: 48_000, max: 48_000 }], max_bits: Some(16), ..Default::default() })
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
    let deadline = Deadline::new();
    loop {
        assert!(!deadline.passed(), "playback stalled");
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
    let deadline = Deadline::new();
    loop {
        assert!(!deadline.passed(), "no Seeked");
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

/// A server's pace for a download that is still running when a test looks:
/// 16 KB every 30 ms, about 3× real time for 44.1 kHz stereo WAV.
pub(crate) fn slowly() -> Serve {
    Serve::trickle(Duration::from_millis(30))
}

/// `body` over HTTP at [`slowly`]'s pace.
pub(crate) fn slow_server(body: Vec<u8>) -> String {
    RangeServer::start(body, slowly()).url()
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
    let target = ((crate::output::PREBUFFER_SECS * RATE as f32) as usize * 2).min(capacity / 2);
    let mut dev = Device { input, status: status.clone(), played: Vec::new() };
    // Play a little of the start (it buffers first, like any track start).
    let deadline = Deadline::new();
    while dev.played.len() < 4_000 {
        assert!(!deadline.passed(), "never started");
        dev.tick(256);
        thread::sleep(Duration::from_millis(1));
    }

    let hold = dev.input.ctl.hold();
    s.seek(0, Duration::from_secs(3), hold);
    let before = dev.played.len();
    // From here on, nothing from before the target may play: the stale ring is cut at once.
    let mut resumed_with = None;
    let deadline = Deadline::new();
    while !(s.finished() && dev.input.samples.is_empty() && dev.input.input_done.load(Ordering::Acquire)) {
        assert!(!deadline.passed(), "stalled after seek: {:?}", s.try_msg());
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
    let deadline = Deadline::new();
    while played.len() < frames * 2 {
        assert!(!deadline.passed(), "playback stalled");
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
    let deadline = Deadline::new();
    loop {
        assert!(!deadline.passed(), "playback stalled");
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
    let gains: Vec<ReplayGain> = seen.iter().filter_map(|m| if let DecoderMsg::Track { replay_gain, .. } = m { Some(*replay_gain) } else { None }).collect();
    assert_eq!(gains.len(), 2);
    assert_eq!(gains[1].track_gain, Some(-12.0));
}

#[test]
fn replay_gain_tags_are_read_by_standard_or_raw_key() {
    use symphonia::core::meta::RawTag;
    let tags = [
        Tag { raw: RawTag::new("REPLAYGAIN_TRACK_GAIN", "-7.50 dB"), std: Some(StandardTag::ReplayGainTrackGain(Arc::new("-7.50 dB".into()))) },
        Tag { raw: RawTag::new("TXXX:replaygain_album_gain", "-8.25 dB"), std: None },
        Tag { raw: RawTag::new("----:com.apple.iTunes:replaygain_track_peak", "0.988"), std: None },
        Tag { raw: RawTag::new("ARTIST", "x"), std: None },
    ];
    let rg = replay_gain_from_tags(tags.iter());
    assert_eq!(rg, ReplayGain { track_gain: Some(-7.5), album_gain: Some(-8.25), track_peak: Some(0.988), album_peak: None });
}

/// Tracks whose id starts with `late` stall for this long before each write
/// to the shared `Status`, cancelled or not: a decode thread descheduled at the
/// worst moment.
const LATE_STALL: Duration = Duration::from_millis(300);

/// (track id, the write is over) for every stalled write so far, from all tests.
static LATE: Mutex<Vec<(String, bool)>> = Mutex::new(Vec::new());

pub(crate) fn stall(track_id: &str) {
    if track_id.starts_with("late") {
        LATE.lock().unwrap().push((track_id.to_owned(), false));
        thread::sleep(LATE_STALL);
    }
}

pub(crate) fn published(track_id: &str) {
    if track_id.starts_with("late") {
        LATE.lock().unwrap().push((track_id.to_owned(), true));
    }
}

/// Waits until track `id` (a `late…` one) has reached a write to `Status`
/// (`over` false), or is past it (`over` true).
pub(crate) fn wait_late(id: &str, over: bool) {
    let deadline = Deadline::new();
    while !LATE.lock().unwrap().iter().any(|(t, o)| t == id && *o == over) {
        assert!(!deadline.passed(), "{id} never got to its write");
        thread::sleep(Duration::from_millis(2));
    }
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

#[test]
fn a_decoder_a_full_marker_ring_ahead_waits_for_the_output() {
    const N: usize = MARKER_CAP + 4;
    let dir = tempfile::tempdir().unwrap();
    // 0.2 s each: more than the decoder lingers over (0.1 s) at a track's end
    // when nothing follows, so it waits there for each next track to be named.
    let tracks: Vec<Vec<i16>> = (0..N).map(|i| pcm(2 * 8_820, 100 + i as i64)).collect();
    let srcs: Vec<TrackSource> = tracks.iter().enumerate().map(|(i, t)| file_source(&dir, &format!("t{i}"), t)).collect();
    let status = bit_perfect_status();
    let s = Session::start(srcs[0].clone(), None, config(30.0), status.clone(), None, None, Arc::default());
    // Names the track after `k` once the decoder is on `k`.
    let name_next = |k: usize| {
        let deadline = Deadline::new();
        while !(s.decoding() == k as u64 && s.set_next(k as u64, Some(srcs[k + 1].clone()))) {
            assert!(!deadline.passed(), "the decoder never reached track {k}");
            thread::sleep(Duration::from_millis(1));
        }
    };

    // Nothing plays yet. The first track opens the ring; each of the next
    // MARKER_CAP leaves a boundary marker in it, which fills the marker ring.
    for k in 0..=MARKER_CAP {
        name_next(k);
    }
    let deadline = Deadline::new();
    while s.decoding() != MARKER_CAP as u64 + 1 {
        assert!(!deadline.passed(), "never took track {}", MARKER_CAP + 1);
        thread::sleep(Duration::from_millis(1));
    }
    thread::sleep(Duration::from_millis(100));
    // The next one waits for room: it hasn't been reported, and no marker was dropped.
    let (input, mut seen) = wait_ring(&s);
    seen.extend(std::iter::from_fn(|| s.try_msg()));
    let reported = seen.iter().filter(|m| matches!(m, DecoderMsg::Track { .. })).count();
    assert_eq!(reported, MARKER_CAP + 1, "{seen:?}");

    // Playing frees the markers; the rest of the queue follows.
    let mut dev = Device { input, status: status.clone(), played: Vec::new() };
    let mut k = MARKER_CAP + 1;
    let deadline = Deadline::new();
    loop {
        assert!(!deadline.passed(), "playback stalled");
        if k < N - 1 && s.decoding() == k as u64 && s.set_next(k as u64, Some(srcs[k + 1].clone())) {
            k += 1;
        }
        let (drained, _) = dev.tick(512);
        if drained && s.finished() {
            break;
        }
        thread::sleep(Duration::from_micros(200));
    }
    assert_eq!(status.track_seq.load(Ordering::Relaxed), N as u64 - 1, "every boundary crossed");
    assert!(dev.played == tracks.concat(), "every track, in order, gaplessly");
}

#[test]
fn opus_files_decode() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/sine-1s.opus");
    let src = TrackSource { id: "opus".into(), url: format!("file://{}", path.display()), suffix: Some("opus".into()), ..Default::default() };
    let status = bit_perfect_status();
    let s = Session::start(src, None, config(0.05), status.clone(), None, None, Arc::default());
    let (input, seen) = wait_ring(&s);
    let Some(DecoderMsg::Track { format, .. }) = seen.first() else { panic!("no Track message: {seen:?}") };
    assert_eq!(*format, StreamFormat { sample_rate: 48_000, channels: 2, bits_per_sample: None });
    let mut dev = Device { input, status, played: Vec::new() };
    play_to_end(&s, &mut dev);
    // A second of 440 Hz: most of its 96,000 samples aren't silent.
    assert!(dev.played.len() > 80_000, "{} samples played", dev.played.len());
}

/// A transcode to Opus arrives as Ogg read front to back: it plays the same.
#[test]
fn opus_streams_decode() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/sine-1s.opus");
    let src = TrackSource { id: "opus".into(), url: format!("test-unseekable://{}", path.display()), suffix: Some("opus".into()), ..Default::default() };
    let status = bit_perfect_status();
    let s = Session::start(src, None, config(0.05), status.clone(), None, None, Arc::default());
    let (input, _) = wait_ring(&s);
    let mut dev = Device { input, status, played: Vec::new() };
    play_to_end(&s, &mut dev);
    assert!(dev.played.len() > 80_000, "{} samples played", dev.played.len());
}

/// One half of a 1 kHz sine cut in two, 0.4 s each. Made with
/// `ffmpeg -f lavfi -i sine=frequency=1000:sample_rate=R` (`volume=4`,
/// stereo), cut with `atrim` and encoded by libmp3lame at 64 kbps (`mp3`,
/// R = 44100) or libopus at 48 kbps (`opus`, R = 48000). Each MP3's LAME tag
/// gives 576 frames of encoder delay and 1,368 of padding; each Opus file has
/// a 312-frame pre-skip and an end trim in its last granule position.
fn lossy_half(name: &str, suffix: &str) -> TrackSource {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(format!("{name}.{suffix}"));
    TrackSource { id: name.into(), url: format!("file://{}", path.display()), suffix: Some(suffix.into()), ..Default::default() }
}

#[test]
fn mp3_tracks_drop_encoder_delay_and_padding_and_join_without_a_gap() {
    join_halves("mp3", 17_640);
}

#[test]
fn opus_tracks_drop_pre_skip_and_end_trim_and_join_without_a_gap() {
    join_halves("opus", 19_200);
}

/// Plays `sine-a` then `sine-b` (`frames` each) gaplessly and checks the
/// decoder queued exactly the encoded audio, joined on the next sample.
fn join_halves(suffix: &str, frames: usize) {
    let s = Session::start(lossy_half("sine-a", suffix), None, config(0.05), bit_perfect_status(), None, None, Arc::default());
    s.set_next(0, Some(lossy_half("sine-b", suffix)));
    let (mut input, _) = wait_ring(&s);
    // Read the ring directly: every sample the decoder queued, silence included.
    let (mut out, mut boundary) = (Vec::new(), None);
    let deadline = Deadline::new();
    loop {
        assert!(!deadline.passed(), "decoding stalled");
        if let Ok(m) = input.markers.pop() {
            boundary = Some(m.at as usize);
        }
        let n = input.samples.slots();
        if n == 0 {
            if input.input_done.load(Ordering::Acquire) && input.samples.slots() == 0 {
                break;
            }
            thread::sleep(Duration::from_millis(1));
            continue;
        }
        let chunk = input.samples.read_chunk(n).unwrap();
        let (a, b) = chunk.as_slices();
        out.extend_from_slice(a);
        out.extend_from_slice(b);
        chunk.commit_all();
    }
    assert_eq!(boundary, Some(frames * 2), "the second track starts right after the first one's last real frame");
    assert_eq!(out.len(), frames * 2 * 2, "both tracks, without delay or padding");
    // A 1 kHz tone at half scale is never near zero for more than a sample;
    // delay or padding left in would be over a thousand frames of it.
    let mut longest = 0;
    let mut run = 0;
    for frame in out.chunks(2) {
        run = if frame[0].abs() < 0.02 { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    assert!(longest <= 2, "a silent stretch of {longest} frames");
}

/// A decoder context and a ring that skip silence and nothing else: the pieces
/// of `queue` that need no real session. The ring's consumers are kept here so
/// the producers stay live.
struct Bench {
    ctx: Ctx,
    out: Out,
    segments: rtrb::Consumer<Segment>,
    _samples: rtrb::Consumer<f32>,
    _markers: rtrb::Consumer<Marker>,
    _inbox: Receiver<DecoderMsg>,
    _seeks: Sender<SeekReq>,
}

fn speech_bench(ring: usize) -> Bench {
    let status = Arc::new(Status::default());
    status.skip_silence.store(true, Ordering::Relaxed);
    let (msgs, inbox) = unbounded();
    let (seeks, seek_rx) = unbounded();
    let ctx = Ctx {
        config: EngineConfig::default(),
        speed: 1.0,
        voice_boost: false,
        skip_silence: true,
        stretched: Vec::new(),
        status,
        cache: None,
        msgs,
        cancel: Arc::default(),
        publish: Arc::default(),
        seek_pending: Arc::default(),
        at_seq: Arc::default(),
        track_id: String::new(),
        seek_rx,
        next: Arc::default(),
        caps: None,
        gain: Arc::default(),
        resampled: Vec::new(),
        mapped: Vec::new(),
    };
    let (samples, sample_consumer) = RingBuffer::new(ring);
    let (markers, marker_consumer) = RingBuffer::new(MARKER_CAP);
    let (segments, segment_consumer) = RingBuffer::new(MARKER_CAP + 1);
    let out = Out {
        format: StreamFormat { sample_rate: 1000, channels: 1, bits_per_sample: Some(16) },
        src_rate: 1000,
        samples,
        markers,
        segments,
        done: Arc::default(),
        ctl: Arc::default(),
        pushed: 0,
        src_channels: 1,
        resampler: None,
        stretch: None,
        silence: Some(Silence::new(1000, 1, 0.0)),
        voice: None,
        input_origin: Origin::default(),
        output_seq: 0,
        cut: false,
        saved: 0.0,
        file_start: 0,
        rs_base: 0,
    };
    Bench { ctx, out, segments: segment_consumer, _samples: sample_consumer, _markers: marker_consumer, _inbox: inbox, _seeks: seeks }
}

#[test]
fn an_interrupted_piece_marks_a_cut_so_the_next_one_reanchors_source_time() {
    // A seek arrives as the kept quiet after a cut is pushed. The detector has
    // already let go of it, and the seek may resolve to nothing (a stale flag),
    // so those frames are lost: without a cut the next kept piece would carry
    // no origin and the source clock would run behind the audio.
    let mut bench = speech_bench(1024);
    let kept = [0.2f32; 200];
    bench.ctx.seek_pending.store(true, Ordering::Release);
    assert!(matches!(bench.ctx.speech_piece(&kept, Origin::default(), 0.21, &mut bench.out), Push::Interrupted));
    assert!(bench.out.cut, "the interrupted piece left a cut behind");
    bench.ctx.seek_pending.store(false, Ordering::Release);
    let pushed = bench.out.pushed;
    assert!(matches!(bench.ctx.speech_piece(&kept, Origin { frame: 650, ..Default::default() }, 0.21, &mut bench.out), Push::Done));
    let segment = bench.segments.pop().expect("the next kept piece is anchored in source time");
    assert_eq!((segment.at, segment.source_frame), (pushed, 650.0));
}
