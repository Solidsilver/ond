//! Fetch + decode thread for a playback session: one or more tracks decoded
//! back to back into the same ring buffer. While consecutive tracks share a
//! format the ring (and the device stream) carries on, and a marker tells the
//! output callback where the next track's first sample is: that's gapless. A
//! format change drains the ring and starts a new one (not gapless).
//!
//! The ring runs at the device's rate, chosen per track with
//! [`choose_rate`]: the file's own rate when the device can do it, else a
//! resampler converts on this thread. Gapless needs the same source rate *and*
//! the same device format, so one resampler carries across the boundary.

use crate::channels::{ChannelChange, ChannelMap, output_channels};
use crate::output::{RingCtl, RingInput, StreamFormat};
use crate::replaygain::{GainConfig, ReplayGain, applied_gain, parse_tag_value};
use crate::resample::StreamResampler;
use crate::signal::{DeviceCaps, choose_rate};
use crate::{EngineConfig, Status, TrackSource};
use symphonia::core::audio::Channels;
use symphonia::core::meta::{StandardTag, Tag};
use crossbeam_channel::{Receiver, Sender, unbounded};
use rtrb::{Producer, RingBuffer};
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use streamcache::{Abort, CachedSource, StreamCache};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::{Error as SymError, SeekErrorKind};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase, Timestamp};

/// Messages from the decode thread to the engine, in the order things happened.
#[derive(Debug)]
pub(crate) enum DecoderMsg {
    /// Track `seq` (0 = the session's first) produced its first audio. `format`
    /// is the file's; `output` what reaches the device (rate after resampling).
    /// `total_frames` counts frames at the output rate. `replay_gain` is the
    /// source's, gaps filled from the file's tags.
    Track {
        seq: u64,
        src: TrackSource,
        format: StreamFormat,
        output: StreamFormat,
        total_frames: Option<u64>,
        replay_gain: ReplayGain,
        channels: ChannelChange,
    },
    /// A new ring whose first track is `first_seq`: open (or reopen) the device for it.
    Ring { format: StreamFormat, input: RingInput, first_seq: u64 },
    /// A seek in track `seq` landed.
    Seeked { seq: u64, position_secs: f64 },
    /// A seek for track `seq` arrived after the decoder had moved past it.
    SeekMiss { seq: u64, to: Duration },
    /// Track `seq` failed; the session ends after what's already queued.
    Error { seq: u64, message: String },
}

/// The track to decode after track `.0`.
type NextSlot = Arc<Mutex<Option<(u64, TrackSource)>>>;

pub(crate) struct Session {
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    msgs: Receiver<DecoderMsg>,
    seeks: Sender<SeekReq>,
    /// Set by [`Session::seek`] so a decode thread blocked on the network or a full ring lets go.
    seek_pending: Arc<AtomicBool>,
    next: NextSlot,
    /// The track the decode thread is opening or decoding.
    at_seq: Arc<AtomicU64>,
}

#[derive(Debug, Clone, Copy)]
struct SeekReq {
    seq: u64,
    to: Duration,
    /// Output hold generation this seek releases once it lands.
    hold: u64,
}

impl Session {
    pub fn start(
        first: TrackSource,
        start_at: Option<Duration>,
        config: EngineConfig,
        status: Arc<Status>,
        cache: Option<StreamCache>,
        caps: Option<DeviceCaps>,
        gain: Arc<Mutex<GainConfig>>,
    ) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let seek_pending = Arc::new(AtomicBool::new(false));
        let next: NextSlot = Arc::default();
        let (msg_tx, msgs) = unbounded();
        let (seeks, seek_rx) = unbounded();
        let at_seq = Arc::new(AtomicU64::new(0));
        let session = Self {
            cancel: cancel.clone(),
            done: done.clone(),
            msgs,
            seeks,
            seek_pending: seek_pending.clone(),
            next: next.clone(),
            at_seq: at_seq.clone(),
        };
        if let Some(to) = start_at {
            // Applied right after probing, before any audio is queued.
            session.seek(0, to, 0);
        }
        let mut ctx = Ctx {
            config,
            status,
            cache,
            msgs: msg_tx,
            cancel,
            seek_pending,
            at_seq,
            seek_rx,
            next,
            caps,
            gain,
            resampled: Vec::new(),
            mapped: Vec::new(),
        };
        thread::Builder::new()
            .name("audio-decode".into())
            .spawn(move || {
                // A panic in a decoder (bad file, library bug) becomes an
                // error for the track it hit, so the engine never waits forever.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ctx.run(first)))
                    .unwrap_or_else(|p| Err((ctx.at_seq.load(Ordering::Acquire), format!("decoder crashed: {}", panic_text(&*p)))));
                if let Err((seq, message)) = result
                    && !ctx.cancel.load(Ordering::Relaxed)
                {
                    let _ = ctx.msgs.send(DecoderMsg::Error { seq, message });
                }
                done.store(true, Ordering::Release);
            })
            .expect("spawn decode thread");
        session
    }

    pub fn try_msg(&self) -> Option<DecoderMsg> {
        self.msgs.try_recv().ok()
    }

    /// The decode thread has exited (all tracks decoded, failed or cancelled).
    pub fn finished(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    /// The track the decode thread is on (opening or decoding). Only that one can
    /// be repositioned in place.
    pub fn decoding(&self) -> u64 {
        self.at_seq.load(Ordering::Acquire)
    }

    /// Asks the decode thread to jump to `to` within track `seq`. Only the latest
    /// pending request is honoured.
    pub fn seek(&self, seq: u64, to: Duration, hold: u64) {
        self.seek_pending.store(true, Ordering::Release);
        let _ = self.seeks.send(SeekReq { seq, to, hold });
    }

    /// Sets what plays after track `after`. Replaces any earlier choice.
    pub fn set_next(&self, after: u64, next: Option<TrackSource>) {
        *self.next.lock().unwrap_or_else(|e| e.into_inner()) = next.map(|n| (after, n));
    }

    /// Signals the thread to stop. It isn't joined: a blocked read notices within ~50 ms.
    pub fn cancel(self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

struct Ctx {
    config: EngineConfig,
    status: Arc<Status>,
    cache: Option<StreamCache>,
    msgs: Sender<DecoderMsg>,
    cancel: Arc<AtomicBool>,
    seek_pending: Arc<AtomicBool>,
    at_seq: Arc<AtomicU64>,
    seek_rx: Receiver<SeekReq>,
    next: NextSlot,
    /// The output device, as it was when the session started (picks the ring's rate).
    caps: Option<DeviceCaps>,
    /// ReplayGain settings, shared with (and updated by) the engine.
    gain: Arc<Mutex<GainConfig>>,
    /// Resampler output for one packet.
    resampled: Vec<f32>,
    /// Channel-mapped samples for one packet.
    mapped: Vec<f32>,
}

/// Timing facts needed to turn timestamps into frame counts.
#[derive(Clone, Copy)]
struct Clock {
    time_base: Option<TimeBase>,
    rate: u32,
}

impl Clock {
    fn frames(&self, ts: i64) -> u64 {
        if ts <= 0 || self.rate == 0 {
            return 0;
        }
        match self.time_base {
            Some(tb) => {
                (ts as i128 * tb.numer.get() as i128 * self.rate as i128 / tb.denom.get() as i128) as u64
            }
            None => ts as u64,
        }
    }
}

struct Track {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    clock: Clock,
    total_frames: Option<u64>,
    bits: Option<u32>,
    /// The rate the device plays this track at (set from the first packet; 0 before).
    out_rate: u32,
    /// ReplayGain from the file's tags.
    tag_gain: ReplayGain,
}

impl Track {
    /// Source frames → output frames.
    fn to_output(&self, frames: u64) -> u64 {
        let (from, to) = (self.clock.rate as u128, self.out_rate as u128);
        if from == 0 || to == 0 || from == to {
            return frames;
        }
        ((frames as u128 * to + from / 2) / from) as u64
    }
}

/// The ring being filled, and how many samples have gone into it.
struct Out {
    /// What the device plays: the output rate.
    format: StreamFormat,
    /// The tracks' rate before resampling.
    src_rate: u32,
    samples: Producer<f32>,
    markers: Producer<u64>,
    done: Arc<AtomicBool>,
    ctl: Arc<RingCtl>,
    pushed: u64,
    /// The tracks' channel count before mapping: a different layout starts a new
    /// ring, since a downmix changes whether the samples need dither.
    src_channels: u16,
    resampler: Option<StreamResampler>,
    /// `pushed` when the resampler last started from zero: its output frame `n`
    /// is ring sample `rs_base + n * channels`.
    rs_base: u64,
}


enum Seeked {
    /// Decoding resumes; drop decoded frames before this timestamp.
    To(Timestamp),
    /// Nothing to do (no request, it was for another track, or it failed and was reported).
    Nothing,
    /// Past the end: finish the track.
    End,
}

enum Push {
    Done,
    Cancelled,
    /// A seek arrived while waiting for room; the rest of this packet is stale.
    Interrupted,
}

enum TrackEnd {
    Eof,
    Cancelled,
}

impl Ctx {
    fn run(&mut self, first: TrackSource) -> Result<(), (u64, String)> {
        let mut seq = 0;
        let mut src = first;
        let mut out: Option<Out> = None;
        let result = loop {
            self.at_seq.store(seq, Ordering::Release);
            let mut track = match self.open(&src) {
                Ok(t) => t,
                Err(_) if self.cancel.load(Ordering::Relaxed) => return Ok(()),
                Err(e) => break Err((seq, e)),
            };
            match self.play(seq, &src, &mut track, &mut out) {
                Ok(TrackEnd::Cancelled) => return Ok(()),
                Ok(TrackEnd::Eof) => {}
                Err(e) => break Err((seq, e)),
            }
            let next = self.next.lock().unwrap_or_else(|e| e.into_inner()).take_if(|(after, _)| *after == seq);
            match next {
                Some((_, n)) => {
                    seq += 1;
                    src = n;
                }
                None => break Ok(()),
            }
        };
        if let Some(o) = out.as_mut() {
            if !self.cancel.load(Ordering::Relaxed) {
                self.flush_resampler(o);
            }
            o.done.store(true, Ordering::Release);
        }
        result
    }

    /// Pushes what's still inside the resampler (end of stream or format change).
    fn flush_resampler(&mut self, o: &mut Out) {
        let Some(r) = o.resampler.as_mut() else { return };
        self.resampled.clear();
        r.flush(&mut self.resampled);
        o.rs_base = o.pushed + self.resampled.len() as u64;
        let _ = push_all(&mut o.samples, &self.resampled, &mut o.pushed, &self.cancel, &self.seek_pending);
    }

    fn open(&mut self, src: &TrackSource) -> Result<Track, String> {
        // Reads give up on cancel at any time, and on a pending seek once probing is over
        // (a seek before that is simply applied after the probe).
        let probed = Arc::new(AtomicBool::new(false));
        let abort: Abort = {
            let (cancel, seek, probed) = (self.cancel.clone(), self.seek_pending.clone(), probed.clone());
            Arc::new(move || {
                cancel.load(Ordering::Relaxed) || (probed.load(Ordering::Relaxed) && seek.load(Ordering::Relaxed))
            })
        };
        let mss = MediaSourceStream::new(open_source(src, self.cache.as_ref(), abort)?, Default::default());

        let mut hint = Hint::new();
        if let Some(ext) = &src.suffix {
            hint.with_extension(ext);
        }
        let mut reader = symphonia::default::get_probe()
            .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
            .map_err(|e| format!("probe: {e}"))?;
        probed.store(true, Ordering::Relaxed);
        let tag_gain = tag_replay_gain(reader.as_mut());
        let track = reader.default_track(TrackType::Audio).ok_or("no audio track")?;
        let track_id = track.id;
        let total_frames = track.num_frames;
        let time_base = track.time_base;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or("track has no audio codec parameters")?
            .clone();
        // `gapless` makes decoders that support it (MP3, Vorbis) drop encoder delay and padding.
        let mut opts = AudioDecoderOptions::default();
        opts.gapless = true;
        let decoder =
            symphonia::default::get_codecs().make_audio_decoder(&params, &opts).map_err(|e| format!("decoder: {e}"))?;
        Ok(Track {
            reader,
            decoder,
            track_id,
            clock: Clock { time_base, rate: params.sample_rate.unwrap_or(0) },
            total_frames,
            bits: params.bits_per_sample,
            out_rate: choose_rate(
                params.sample_rate.unwrap_or(0),
                output_channels(params.channels.as_ref().map_or(2, |c| c.count() as u16), self.caps.as_ref()),
                self.caps.as_ref(),
            ),
            tag_gain,
        })
    }

    fn play(&mut self, seq: u64, src: &TrackSource, t: &mut Track, out: &mut Option<Out>) -> Result<TrackEnd, String> {
        let mut scratch: Vec<f32> = Vec::new();
        let mut skip_until: Option<Timestamp> = None;
        let mut started = false;
        let mut map = ChannelMap::Identity;

        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return Ok(TrackEnd::Cancelled);
            }
            if self.seek_pending.load(Ordering::Acquire) {
                match self.seek(seq, t, out.as_mut().filter(|_| started)) {
                    Seeked::To(ts) => skip_until = Some(ts),
                    Seeked::Nothing => {}
                    Seeked::End => break,
                }
                continue;
            }
            let packet = match t.reader.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => break,
                // The read was interrupted for a seek or cancel; the loop head handles both.
                Err(_) if self.cancel.load(Ordering::Relaxed) || self.seek_pending.load(Ordering::Acquire) => continue,
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(format!("read: {e}")),
            };
            if packet.track_id != t.track_id {
                continue;
            }
            let pts = packet.pts;
            let buf = match t.decoder.decode(&packet) {
                Ok(b) => b,
                // A corrupt frame: skip it rather than abort the track.
                Err(SymError::DecodeError(e)) => {
                    tracing::warn!("decode error, skipping packet: {e}");
                    continue;
                }
                Err(e) => return Err(format!("decode: {e}")),
            };
            let spec = buf.spec();
            let channels = spec.channels().count().max(1);
            let positions = match spec.channels() {
                Channels::Positioned(p) => Some(p.bits()),
                _ => None,
            };
            let rate = spec.rate();
            scratch.resize(buf.samples_interleaved(), 0.0);
            buf.copy_to_slice_interleaved(&mut scratch);
            if t.clock.rate == 0 {
                t.clock.rate = rate;
            }
            trim_padding(&mut scratch, channels, &packet, t.clock);

            // An accurate seek lands on the packet containing the target: drop the frames before it.
            if let Some(target) = skip_until {
                if pts < target {
                    let skip = t.clock.frames(target.get() - pts.get()) as usize * channels;
                    scratch.drain(..skip.min(scratch.len()));
                    if scratch.is_empty() {
                        continue;
                    }
                }
                skip_until = None;
            }
            if scratch.is_empty() {
                continue;
            }

            let mut fresh = None;
            if !started {
                started = true;
                let format = StreamFormat { sample_rate: rate, channels: channels as u16, bits_per_sample: t.bits };
                let out_channels = output_channels(format.channels, self.caps.as_ref());
                let (m, change) = ChannelMap::new(format.channels, positions, out_channels);
                map = m;
                let output = StreamFormat {
                    sample_rate: choose_rate(rate, out_channels, self.caps.as_ref()),
                    channels: out_channels,
                    ..format
                };
                t.out_rate = output.sample_rate;
                // The gain must be in place before the boundary marker goes in.
                let replay_gain = src.replay_gain.or(t.tag_gain);
                let cfg = *self.gain.lock().unwrap_or_else(|e| e.into_inner());
                let linear = applied_gain(&cfg, &replay_gain, src.album_context).map_or(1.0, |g| g.linear);
                self.status.gains.set(seq, linear);
                let downmixed = matches!(change, ChannelChange::Downmix { .. });
                match self.attach(output, rate, format.channels, downmixed, out) {
                    Some(f) => fresh = f,
                    None => return Ok(TrackEnd::Cancelled),
                }
                let total_frames = t.total_frames.map(|f| t.to_output(f));
                let _ = self.msgs.send(DecoderMsg::Track {
                    seq,
                    src: src.clone(),
                    format,
                    output,
                    total_frames,
                    replay_gain,
                    channels: change,
                });
            }
            let mapped = if map.is_identity() {
                &mut scratch
            } else {
                map.apply(&scratch, &mut self.mapped);
                &mut self.mapped
            };
            let Some(o) = out.as_mut() else { unreachable!("attach always leaves a ring") };
            let data = match o.resampler.as_mut() {
                Some(r) => {
                    self.resampled.clear();
                    r.process(mapped, &mut self.resampled);
                    &self.resampled
                }
                None => &*mapped,
            };
            match push_all(&mut o.samples, data, &mut o.pushed, &self.cancel, &self.seek_pending) {
                Push::Done | Push::Interrupted => {}
                Push::Cancelled => return Ok(TrackEnd::Cancelled),
            }
            // A new ring goes to the engine once it holds some audio, so the device
            // doesn't open onto an empty buffer.
            if let Some(input) = fresh {
                self.msgs.send(DecoderMsg::Ring { format: o.format, input, first_seq: seq }).map_err(|_| "engine gone")?;
            }
        }
        Ok(TrackEnd::Eof)
    }

    /// Hooks a starting track up to an output ring: continue the current one when
    /// the format matches (gapless: just mark where the track begins), else let
    /// it drain and make a new one. Returns the consumer side of a new ring, or
    /// `None` if cancelled while draining.
    fn attach(
        &mut self,
        format: StreamFormat,
        src_rate: u32,
        src_channels: u16,
        downmixed: bool,
        out: &mut Option<Out>,
    ) -> Option<Option<RingInput>> {
        if let Some(o) = out.as_mut() {
            if o.format == format && o.src_rate == src_rate && o.src_channels == src_channels {
                // With a resampler carrying on, the next track starts where its first
                // input frame lands in the output, which may still be inside the filter.
                let at = match &o.resampler {
                    Some(r) => o.rs_base + r.out_pos(r.in_frames()) * format.channels as u64,
                    None => o.pushed,
                };
                while o.markers.push(at).is_err() {
                    if self.cancel.load(Ordering::Relaxed) {
                        return None;
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                return Some(None);
            }
            // Different format: the device has to be reopened. Play the old ring out first.
            self.flush_resampler(o);
            o.done.store(true, Ordering::Release);
            while o.samples.slots() < o.samples.buffer().capacity() {
                if self.cancel.load(Ordering::Relaxed) || o.samples.is_abandoned() {
                    break;
                }
                thread::sleep(Duration::from_millis(2));
            }
            if self.cancel.load(Ordering::Relaxed) {
                return None;
            }
            self.status.frames_played.store(0, Ordering::Relaxed);
        }
        let cap = (format.sample_rate as f32 * format.channels as f32 * self.config.buffer_secs) as usize;
        let (samples, consumer) = RingBuffer::new(cap.max(8192));
        let (markers, marker_consumer) = RingBuffer::new(16);
        let done = Arc::new(AtomicBool::new(false));
        let ctl = Arc::new(RingCtl::default());
        let resampler = if src_rate == format.sample_rate {
            None
        } else {
            match StreamResampler::new(src_rate, format.sample_rate, format.channels as usize) {
                Ok(r) => Some(r),
                Err(e) => {
                    let _ = self.msgs.send(DecoderMsg::Error { seq: u64::MAX, message: e });
                    return None;
                }
            }
        };
        let input = RingInput::new(consumer, marker_consumer, done.clone(), ctl.clone())
            .with_source(format.bits_per_sample, resampler.is_some() || downmixed);
        *out = Some(Out { format, src_rate, samples, markers, done, ctl, pushed: 0, src_channels, resampler, rs_base: 0 });
        Some(Some(input))
    }

    /// Applies the newest pending seek: reposition the reader, reset the decoder,
    /// flush stale audio out of the ring (releasing the output's hold), then
    /// publish the new position.
    fn seek(&mut self, seq: u64, t: &mut Track, out: Option<&mut Out>) -> Seeked {
        // Clear before draining: a request racing in after this is seen on the next pass.
        self.seek_pending.store(false, Ordering::Release);
        let Some(req) = self.seek_rx.try_iter().last() else { return Seeked::Nothing };
        if req.seq != seq {
            // The output is still on an earlier track than the one being decoded.
            let _ = self.msgs.send(DecoderMsg::SeekMiss { seq: req.seq, to: req.to });
            return Seeked::Nothing;
        }

        let time = Time::from_nanos_u64(req.to.as_nanos().min(u64::MAX as u128) as u64);
        let result = t.reader.seek(SeekMode::Accurate, SeekTo::Time { time, track_id: Some(t.track_id) });
        if self.cancel.load(Ordering::Relaxed) {
            return Seeked::Nothing;
        }
        let outcome = match result {
            Ok(s) => {
                t.decoder.reset();
                Seeked::To(s.required_ts)
            }
            Err(SymError::SeekError(SeekErrorKind::OutOfRange)) => Seeked::End,
            Err(e) => {
                if !self.seek_pending.load(Ordering::Acquire) {
                    let _ = self.msgs.send(DecoderMsg::Error { seq, message: format!("seek: {e}") });
                }
                Seeked::Nothing
            }
        };
        // Whatever happened, the output must stop holding: flush, then let it buffer.
        if let Some(o) = out {
            let generation = o.ctl.request_flush(req.hold);
            let deadline = Instant::now() + Duration::from_millis(500);
            while !o.ctl.flushed(generation) {
                if self.cancel.load(Ordering::Relaxed) {
                    return Seeked::Nothing;
                }
                if Instant::now() > deadline {
                    // No callback is running (device not open yet?). Carry on; a little stale audio may play.
                    tracing::warn!("ring flush not acknowledged");
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
            o.pushed = 0;
            o.rs_base = 0;
            if let Some(r) = o.resampler.as_mut() {
                r.reset();
            }
        }
        if let Seeked::To(ts) = outcome {
            let frames = t.clock.frames(ts.get());
            // The callback counts frames at the output rate.
            let played = t.to_output(frames);
            self.status.frames_played.store(played, Ordering::Relaxed);
            let position_secs = if t.clock.rate == 0 { 0.0 } else { frames as f64 / t.clock.rate as f64 };
            let _ = self.msgs.send(DecoderMsg::Seeked { seq, position_secs });
        }
        outcome
    }
}

/// The message a panic carried, for error reports.
pub(crate) fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

/// Drops encoder delay/padding the decoder left in. Decoders that honour
/// `gapless` (MP3, Vorbis) already produce exactly `dur` frames and are left
/// alone; this only acts when the output still includes the trim.
fn trim_padding(scratch: &mut Vec<f32>, channels: usize, packet: &symphonia::core::packet::Packet, clock: Clock) {
    let start = clock.frames(packet.trim_start.get() as i64) as usize;
    let end = clock.frames(packet.trim_end.get() as i64) as usize;
    if start + end == 0 {
        return;
    }
    let valid = clock.frames(packet.dur.get() as i64) as usize;
    let decoded = scratch.len() / channels;
    if decoded == valid + start + end {
        scratch.truncate((decoded - end) * channels);
        scratch.drain(..start * channels);
    }
}

/// ReplayGain from the file's tags, in every metadata revision the reader has
/// (container tags, ID3 in front of it, per-track tags).
fn tag_replay_gain(reader: &mut dyn FormatReader) -> ReplayGain {
    let mut rg = ReplayGain::default();
    let mut meta = reader.metadata();
    loop {
        if let Some(rev) = meta.current() {
            let tags = rev.media.tags.iter().chain(rev.per_track.iter().flat_map(|t| t.metadata.tags.iter()));
            rg = rg.or(replay_gain_from_tags(tags));
        }
        if meta.pop().is_none() {
            break;
        }
    }
    rg
}

/// Reads ReplayGain from tags, by standard tag or by raw key
/// (`REPLAYGAIN_TRACK_GAIN`, `TXXX:replaygain_track_gain`, iTunes `----:…:replaygain_track_gain`).
fn replay_gain_from_tags<'a>(tags: impl Iterator<Item = &'a Tag>) -> ReplayGain {
    let mut rg = ReplayGain::default();
    for tag in tags {
        let (slot, value) = match &tag.std {
            Some(StandardTag::ReplayGainTrackGain(v)) => (&mut rg.track_gain, v.as_str().to_owned()),
            Some(StandardTag::ReplayGainAlbumGain(v)) => (&mut rg.album_gain, v.as_str().to_owned()),
            Some(StandardTag::ReplayGainTrackPeak(v)) => (&mut rg.track_peak, v.as_str().to_owned()),
            Some(StandardTag::ReplayGainAlbumPeak(v)) => (&mut rg.album_peak, v.as_str().to_owned()),
            _ => {
                let key = tag.raw.key.to_ascii_uppercase();
                let slot = if key.ends_with("REPLAYGAIN_TRACK_GAIN") {
                    &mut rg.track_gain
                } else if key.ends_with("REPLAYGAIN_ALBUM_GAIN") {
                    &mut rg.album_gain
                } else if key.ends_with("REPLAYGAIN_TRACK_PEAK") {
                    &mut rg.track_peak
                } else if key.ends_with("REPLAYGAIN_ALBUM_PEAK") {
                    &mut rg.album_peak
                } else {
                    continue;
                };
                (slot, tag.raw.value.to_string())
            }
        };
        if slot.is_none() {
            *slot = parse_tag_value(&value);
        }
    }
    rg
}

/// `file://` for local files (offline downloads, tests); anything else goes through the stream cache.
fn open_source(src: &TrackSource, cache: Option<&StreamCache>, abort: Abort) -> Result<Box<dyn MediaSource>, String> {
    #[cfg(test)]
    if src.url == "test://panic" {
        panic!("a decoder blew up");
    }
    if let Some(path) = src.url.strip_prefix("file://") {
        let f = std::fs::File::open(path).map_err(|e| format!("open {path}: {e}"))?;
        return Ok(Box::new(f));
    }
    let cache = cache.ok_or("no stream cache configured")?;
    let source = cache.open(&src.cache_key(), &src.url, abort).map_err(|e| format!("fetch: {e}"))?;
    Ok(Box::new(Cached(source)))
}

/// Adapts the cache's reader to symphonia (the orphan rule keeps the impl here).
struct Cached(CachedSource);

impl Read for Cached {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl Seek for Cached {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        self.0.seek(to)
    }
}

impl MediaSource for Cached {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        self.0.byte_len()
    }
}

/// Blocks (politely) until all of `data` is in the ring, or a cancel or seek
/// interrupts. Counts what went in.
fn push_all(p: &mut Producer<f32>, mut data: &[f32], pushed: &mut u64, cancel: &AtomicBool, seek: &AtomicBool) -> Push {
    while !data.is_empty() {
        if cancel.load(Ordering::Relaxed) || p.is_abandoned() {
            return Push::Cancelled;
        }
        if seek.load(Ordering::Acquire) {
            return Push::Interrupted;
        }
        let (_, rest) = p.push_partial_slice(data);
        *pushed += (data.len() - rest.len()) as u64;
        if rest.len() == data.len() {
            thread::sleep(Duration::from_millis(5));
        }
        data = rest;
    }
    Push::Done
}

#[cfg(test)]
pub(crate) mod tests;
