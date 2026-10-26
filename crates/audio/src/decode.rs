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
use crate::output::{MARKER_CAP, Marker, RingCtl, RingInput, StreamFormat};
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
    /// A seek in track `seq` landed (past the end: at the end, and the track is over).
    Seeked { seq: u64, position_secs: f64 },
    /// A seek for track `seq` arrived after the decoder had moved past it.
    SeekMiss { seq: u64, to: Duration },
    /// Track `seq` (`track_id`) failed; the session ends after what's already queued.
    Error { seq: u64, track_id: String, message: String },
}

/// At the end of the last queued track the decode thread waits for a next one
/// until the output has only this much of it left to play.
const LINGER_FLOOR_SECS: f32 = 0.1;

/// The track to decode after track `.0`.
type NextSlot = Arc<Mutex<Option<(u64, TrackSource)>>>;

pub(crate) struct Session {
    cancel: Arc<AtomicBool>,
    /// Taken by [`Session::cancel`] and around every write the decode thread
    /// makes to the shared [`Status`]: see [`Ctx::publish`].
    publish: Arc<Mutex<()>>,
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
        let publish: Arc<Mutex<()>> = Arc::default();
        let done = Arc::new(AtomicBool::new(false));
        let seek_pending = Arc::new(AtomicBool::new(false));
        let next: NextSlot = Arc::default();
        let (msg_tx, msgs) = unbounded();
        let (seeks, seek_rx) = unbounded();
        let at_seq = Arc::new(AtomicU64::new(0));
        let session = Self {
            cancel: cancel.clone(),
            publish: publish.clone(),
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
            publish,
            seek_pending,
            at_seq,
            track_id: String::new(),
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
                    let track_id = ctx.track_id.clone();
                    let _ = ctx.msgs.send(DecoderMsg::Error { seq, track_id, message });
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
        // The request first, then the flag: a decoder that sees the flag always
        // finds the request. The other way round, a decoder running between the
        // two would clear the flag, find nothing, and strand the request (and
        // the output's hold) until the next seek.
        let _ = self.seeks.send(SeekReq { seq, to, hold });
        self.seek_pending.store(true, Ordering::Release);
    }

    /// Sets what plays after track `after`, replacing any earlier choice, if the
    /// decode thread is still on `after` (decoding it or waiting at its end).
    /// Returns false when it has already moved past it: the choice came too late.
    pub fn set_next(&self, after: u64, next: Option<TrackSource>) -> bool {
        let mut slot = self.next.lock().unwrap_or_else(|e| e.into_inner());
        // Checked under the lock the decode thread takes the slot with, so the
        // answer can't go stale before the slot is written.
        if self.at_seq.load(Ordering::Acquire) != after {
            return false;
        }
        *slot = next.map(|n| (after, n));
        true
    }

    /// Signals the thread to stop. When this returns the thread no longer
    /// writes to the shared [`Status`], so the next session can reset and use it.
    ///
    /// The thread isn't joined: it may still be finishing a read (a blocked
    /// network read notices within ~50 ms) or decoding a packet, and waiting
    /// for that would stall the engine on every Play and Stop. What it does
    /// meanwhile only touches its own ring and channels, which nothing reads
    /// any more.
    pub fn cancel(self) {
        let _publishing = self.publish.lock().unwrap_or_else(|e| e.into_inner());
        self.cancel.store(true, Ordering::Relaxed);
    }
}

struct Ctx {
    config: EngineConfig,
    status: Arc<Status>,
    cache: Option<StreamCache>,
    msgs: Sender<DecoderMsg>,
    cancel: Arc<AtomicBool>,
    publish: Arc<Mutex<()>>,
    seek_pending: Arc<AtomicBool>,
    at_seq: Arc<AtomicU64>,
    /// Id of the track at `at_seq`, for error reports.
    track_id: String,
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
    /// The rate the device plays this track at: from the codec parameters
    /// until the first packet, then from its format.
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
    /// What the device plays: the output rate and channels. No bit depth:
    /// the ring takes tracks of any depth.
    format: StreamFormat,
    /// The tracks' rate before resampling.
    src_rate: u32,
    samples: Producer<f32>,
    markers: Producer<Marker>,
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

/// Per-track reading state of [`Ctx::play`].
#[derive(Default)]
struct Packets {
    /// The last packet's audio, interleaved, as decoded.
    scratch: Vec<f32>,
    /// Drop decoded audio before this timestamp (where a seek landed).
    skip_until: Option<Timestamp>,
    /// The track's rate and channel count as its first packet had them.
    first_spec: Option<(u32, usize)>,
    /// A packet in another format was skipped (and logged).
    skipped_spec: bool,
}

/// The format of the audio a packet decoded to.
#[derive(Debug, Clone, Copy)]
struct Spec {
    rate: u32,
    channels: u16,
    /// Speaker positions, when the stream names them.
    positions: Option<u64>,
}

/// What [`Ctx::next_audio`] found.
enum Next {
    /// A packet's audio, in `Packets::scratch`.
    Audio(Spec),
    /// The end of the track (or a seek past it).
    End,
    Cancelled,
}

/// What feeds a ring from a track: its rate and channel count before
/// conversion, and whether those channels are mixed down.
#[derive(Debug, Clone, Copy)]
struct Feed {
    rate: u32,
    channels: u16,
    downmixed: bool,
}

/// What [`Ctx::attach`] did with a starting track.
enum Attached {
    /// It carries on in the current ring, gaplessly.
    Continued,
    /// It starts a new ring (the device opens or reopens for it).
    NewRing(RingInput),
    /// Cancelled while waiting for the old ring to play out or for room.
    Cancelled,
}

enum Push {
    Done,
    Cancelled,
    /// A seek arrived while waiting for room; the rest of this packet is stale.
    Interrupted,
}

enum TrackEnd {
    Eof,
    /// The end, without a single frame of audio.
    Empty,
    Cancelled,
}

impl Ctx {
    fn run(&mut self, first: TrackSource) -> Result<(), (u64, String)> {
        let mut seq = 0;
        let mut src = first;
        let mut out: Option<Out> = None;
        let result = loop {
            self.track_id.clone_from(&src.id);
            let mut track = match self.open(&src) {
                Ok(t) => t,
                Err(_) if self.cancel.load(Ordering::Relaxed) => return Ok(()),
                Err(e) => break Err((seq, e)),
            };
            // A track that produced no audio (empty, or sought past its end
            // before it started) was never reported or marked in the ring: the
            // next one takes over its sequence number, so the engine's list of
            // tracks stays indexed by it.
            let advance = match self.play(seq, &src, &mut track, &mut out) {
                Ok(TrackEnd::Cancelled) => return Ok(()),
                Ok(TrackEnd::Eof) => 1,
                Ok(TrackEnd::Empty) => 0,
                Err(e) => break Err((seq, e)),
            };
            let next = {
                let mut slot = self.next.lock().unwrap_or_else(|e| e.into_inner());
                let next = slot.take_if(|(after, _)| *after == seq);
                if next.is_some() {
                    // Under the lock: from here on `Session::set_next` for `seq` is refused.
                    self.at_seq.store(seq + advance, Ordering::Release);
                }
                next
            };
            match next {
                Some((_, n)) => {
                    seq += advance;
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

    /// Writes to the shared [`Status`], unless the session has been cancelled.
    ///
    /// `Status` outlives sessions: after a Stop or a new Play the engine resets
    /// it and the next session's thread starts writing to it (same sequence
    /// numbers, same counters) while this one may still be running. Checking
    /// `cancel` alone would leave a window between the check and the write;
    /// under the lock [`Session::cancel`] takes, there is none.
    fn publish(&self, write: impl FnOnce(&Status)) {
        #[cfg(test)]
        tests::stall(&self.track_id);
        {
            let _publishing = self.publish.lock().unwrap_or_else(|e| e.into_inner());
            if !self.cancel.load(Ordering::Relaxed) {
                write(&self.status);
            }
        }
        #[cfg(test)]
        tests::published(&self.track_id);
    }

    /// Pushes what's still inside the resampler (end of stream or format change).
    fn flush_resampler(&mut self, o: &mut Out) {
        let Some(r) = o.resampler.as_mut() else { return };
        self.resampled.clear();
        r.flush(&mut self.resampled);
        o.rs_base = o.pushed + self.resampled.len() as u64;
        let frame = o.format.channels as usize;
        let _ = push_all(&mut o.samples, &self.resampled, frame, &mut o.pushed, &self.cancel, &self.seek_pending);
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
            // From the codec parameters, so that a seek before the first packet
            // (a restart at a position) lands at the right output frame; the
            // first packet's own format confirms or corrects it.
            out_rate: self.device_rate(
                params.sample_rate.unwrap_or(0),
                params.channels.as_ref().map_or(2, |c| c.count() as u16),
            ),
            tag_gain,
        })
    }

    /// Decodes track `seq` into the ring (starting one if needed), applying
    /// seeks as they come, until it ends or the session is cancelled.
    fn play(&mut self, seq: u64, src: &TrackSource, t: &mut Track, out: &mut Option<Out>) -> Result<TrackEnd, String> {
        let mut packets = Packets::default();
        // Set up with the track's first audio.
        let mut map: Option<ChannelMap> = None;
        loop {
            let started = map.is_some();
            let spec = match self.next_audio(seq, t, out.as_mut().filter(|_| started), &mut packets)? {
                Next::Audio(spec) => spec,
                Next::End => break,
                Next::Cancelled => return Ok(TrackEnd::Cancelled),
            };
            let mut fresh = None;
            if !started {
                let (m, attached) = self.start_track(seq, src, t, spec, out)?;
                map = Some(m);
                match attached {
                    Attached::Continued => {}
                    Attached::NewRing(input) => fresh = Some(input),
                    Attached::Cancelled => return Ok(TrackEnd::Cancelled),
                }
            }
            let (Some(map), Some(o)) = (map.as_ref(), out.as_mut()) else { unreachable!("attach always leaves a ring") };
            if let Push::Cancelled = self.queue(&packets.scratch, map, o) {
                return Ok(TrackEnd::Cancelled);
            }
            // A new ring goes to the engine once it holds some audio, so the device
            // doesn't open onto an empty buffer.
            if let Some(input) = fresh {
                self.msgs.send(DecoderMsg::Ring { format: o.format, input, first_seq: seq }).map_err(|_| "engine gone")?;
            }
        }
        Ok(if map.is_some() { TrackEnd::Eof } else { TrackEnd::Empty })
    }

    /// Reads and decodes until a packet yields audio (left in
    /// `packets.scratch`), handling seeks, the end of the track (waiting there
    /// for a next one, see [`linger`](Self::linger)) and cancels on the way.
    /// `out` is the ring once the track has started.
    fn next_audio(&mut self, seq: u64, t: &mut Track, mut out: Option<&mut Out>, packets: &mut Packets) -> Result<Next, String> {
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return Ok(Next::Cancelled);
            }
            if self.seek_pending.load(Ordering::Acquire) {
                match self.seek(seq, t, out.as_deref_mut()) {
                    Seeked::To(ts) => packets.skip_until = Some(ts),
                    Seeked::Nothing => {}
                    Seeked::End => return Ok(Next::End),
                }
                continue;
            }
            let packet = match t.reader.next_packet() {
                Ok(Some(p)) => p,
                // A seek that came in as the track ran out is still applied.
                Ok(None) if self.seek_pending.load(Ordering::Acquire) => continue,
                Ok(None) if self.linger(seq, out.as_deref()) => continue,
                Ok(None) => return Ok(Next::End),
                // The read was interrupted for a seek or cancel; the loop head handles both.
                Err(_) if self.cancel.load(Ordering::Relaxed) || self.seek_pending.load(Ordering::Acquire) => continue,
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    if self.linger(seq, out.as_deref()) {
                        continue;
                    }
                    return Ok(Next::End);
                }
                Err(e) => return Err(format!("read: {e}")),
            };
            if packet.track_id != t.track_id {
                continue;
            }
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
            // A stream that changes rate or channels midway (chained Ogg, some
            // MP3s) has those packets skipped: they'd be pushed into a ring set
            // up for the first format.
            match packets.first_spec {
                None => packets.first_spec = Some((rate, channels)),
                Some(first) if first != (rate, channels) => {
                    if !packets.skipped_spec {
                        packets.skipped_spec = true;
                        tracing::warn!("format changed mid-stream ({}/{} → {rate}/{channels}); skipping those packets", first.0, first.1);
                    }
                    continue;
                }
                Some(_) => {}
            }
            let scratch = &mut packets.scratch;
            scratch.resize(buf.samples_interleaved(), 0.0);
            buf.copy_to_slice_interleaved(&mut *scratch);
            if t.clock.rate == 0 {
                t.clock.rate = rate;
            }
            trim_padding(scratch, channels, &packet, t.clock);

            // An accurate seek lands on the packet containing the target: drop the frames before it.
            if let Some(target) = packets.skip_until {
                if packet.pts < target {
                    let skip = t.clock.frames(target.get() - packet.pts.get()) as usize * channels;
                    scratch.drain(..skip.min(scratch.len()));
                    if scratch.is_empty() {
                        continue;
                    }
                }
                packets.skip_until = None;
            }
            if !scratch.is_empty() {
                return Ok(Next::Audio(Spec { rate, channels: channels as u16, positions }));
            }
        }
    }

    /// Sets track `seq` up from its first audio: the channel mapping, the
    /// output format, its ReplayGain, its place in a ring, and the report to
    /// the engine.
    fn start_track(
        &mut self,
        seq: u64,
        src: &TrackSource,
        t: &mut Track,
        spec: Spec,
        out: &mut Option<Out>,
    ) -> Result<(ChannelMap, Attached), String> {
        let format = StreamFormat { sample_rate: spec.rate, channels: spec.channels, bits_per_sample: t.bits };
        let out_channels = output_channels(format.channels, self.caps.as_ref());
        let (map, change) = ChannelMap::new(format.channels, spec.positions, out_channels);
        // What the first packet says wins over the codec parameters `open` went by.
        t.out_rate = self.device_rate(spec.rate, spec.channels);
        let output = StreamFormat { sample_rate: t.out_rate, channels: out_channels, ..format };
        // The gain must be in place before the boundary marker goes in.
        let replay_gain = src.replay_gain.or(t.tag_gain);
        let cfg = *self.gain.lock().unwrap_or_else(|e| e.into_inner());
        let linear = applied_gain(&cfg, &replay_gain, src.album_context).map_or(1.0, |g| g.linear);
        self.publish(|s| s.gains.set(seq, linear));
        let feed = Feed { rate: spec.rate, channels: spec.channels, downmixed: matches!(change, ChannelChange::Downmix { .. }) };
        let attached = self.attach(output, feed, out)?;
        if !matches!(attached, Attached::Cancelled) {
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
        Ok((map, attached))
    }

    /// The rate the device runs at for a source at `rate` with `channels`.
    fn device_rate(&self, rate: u32, channels: u16) -> u32 {
        choose_rate(rate, output_channels(channels, self.caps.as_ref()), self.caps.as_ref())
    }

    /// Maps, resamples and pushes one packet's audio into the ring.
    fn queue(&mut self, samples: &[f32], map: &ChannelMap, o: &mut Out) -> Push {
        let mapped = if map.is_identity() {
            samples
        } else {
            map.apply(samples, &mut self.mapped);
            &self.mapped
        };
        let data = match o.resampler.as_mut() {
            Some(r) => {
                self.resampled.clear();
                r.process(mapped, &mut self.resampled);
                &self.resampled
            }
            None => mapped,
        };
        let frame = o.format.channels as usize;
        match push_all(&mut o.samples, data, frame, &mut o.pushed, &self.cancel, &self.seek_pending) {
            // Interrupted: the rest of the packet is dropped. The seek that
            // interrupted normally flushes the ring; when it turns out to be
            // nothing to do (stale flag) the output skips that bit of audio,
            // but stays on whole frames.
            Push::Done | Push::Interrupted => Push::Done,
            Push::Cancelled => Push::Cancelled,
        }
    }

    /// At the end of track `seq` with nothing lined up after it, waits while the
    /// output still has more than [`LINGER_FLOOR_SECS`] of it queued: the UI
    /// names the next track once the output reaches this one, which for a track
    /// shorter than the buffer is after the decoder is done with it. Leaving
    /// at once would lose that track's gapless start. Returns true when a seek
    /// or a cancel came in (the caller's loop handles both), false to finish
    /// the track (a next track is set, the output is nearly through, or it
    /// is prebuffering).
    ///
    /// A prebuffering output (a short track's start, or after an underrun)
    /// waits for more audio or the end of the input before it plays anything.
    /// Waiting here for it to drain would then be a deadlock: it ends the
    /// wait, and the track ends. Holding for a seek doesn't: the seek follows.
    fn linger(&self, seq: u64, out: Option<&Out>) -> bool {
        let Some(o) = out else { return false };
        let floor = (o.format.sample_rate as f32 * o.format.channels as f32 * LINGER_FLOOR_SECS) as usize;
        loop {
            if self.cancel.load(Ordering::Relaxed) || self.seek_pending.load(Ordering::Acquire) {
                return true;
            }
            if self.next.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|(after, _)| *after == seq) {
                return false;
            }
            let queued = o.samples.buffer().capacity() - o.samples.slots();
            if queued <= floor || o.samples.is_abandoned() {
                return false;
            }
            if self.status.buffering.load(Ordering::Relaxed) && !o.ctl.holding() {
                return false;
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// Hooks a starting track up to an output ring: continue the current one
    /// when the track fits it (gapless: just mark where the track begins),
    /// else let it drain and make a new one.
    ///
    /// A ring is fixed by the rate and layout going in and coming out, not by
    /// bit depth: each track's depth travels with its [`Marker`], so a 16-bit
    /// track followed by a 24-bit one (or a lossy one by a lossless one) at
    /// the same rate stays gapless.
    fn attach(&mut self, output: StreamFormat, feed: Feed, out: &mut Option<Out>) -> Result<Attached, String> {
        let format = StreamFormat { bits_per_sample: None, ..output };
        if let Some(o) = out.as_mut() {
            if o.format == format && o.src_rate == feed.rate && o.src_channels == feed.channels {
                // With a resampler carrying on, the next track starts where its first
                // input frame lands in the output, which may still be inside the filter.
                let at = match &o.resampler {
                    Some(r) => o.rs_base + r.out_pos(r.in_frames()) * format.channels as u64,
                    None => o.pushed,
                };
                // Full when the decoder is `MARKER_CAP` tracks ahead: wait for the output.
                let marker = Marker { at, source_bits: output.bits_per_sample };
                while o.markers.push(marker).is_err() {
                    if self.cancel.load(Ordering::Relaxed) {
                        return Ok(Attached::Cancelled);
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                return Ok(Attached::Continued);
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
                return Ok(Attached::Cancelled);
            }
            // The engine zeroes the position when it reopens the device.
        }
        let resampler = if feed.rate == format.sample_rate {
            None
        } else {
            Some(StreamResampler::new(feed.rate, format.sample_rate, format.channels as usize)?)
        };
        let cap = (format.sample_rate as f32 * format.channels as f32 * self.config.buffer_secs) as usize;
        let (samples, consumer) = RingBuffer::new(cap.max(8192));
        let (markers, marker_consumer) = RingBuffer::new(MARKER_CAP);
        let done = Arc::new(AtomicBool::new(false));
        let ctl = Arc::new(RingCtl::default());
        let input = RingInput::new(consumer, marker_consumer, done.clone(), ctl.clone())
            .with_source(output.bits_per_sample, resampler.is_some() || feed.downmixed);
        let (src_rate, src_channels) = (feed.rate, feed.channels);
        *out = Some(Out { format, src_rate, samples, markers, done, ctl, pushed: 0, src_channels, resampler, rs_base: 0 });
        Ok(Attached::NewRing(input))
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
                    let _ = self.msgs.send(DecoderMsg::Error { seq, track_id: self.track_id.clone(), message: format!("seek: {e}") });
                }
                Seeked::Nothing
            }
        };
        // Whatever happened, the output must stop holding: flush, then let it buffer.
        let started = out.is_some();
        if let Some(o) = out {
            let generation = o.ctl.request_flush(req.hold);
            // The callback empties the ring. It runs whenever the device is open
            // (paused too), and the engine opens the device as soon as it hears
            // of the ring, so this normally takes a callback period. Pushing new
            // audio before the flush lands would have it thrown away with the
            // stale audio, so wait for it; a device that never opens ends in a
            // cancel, and the deadline is only a guard against a stuck callback.
            let deadline = Instant::now() + Duration::from_secs(5);
            while !o.ctl.flushed(generation) {
                if self.cancel.load(Ordering::Relaxed) {
                    return Seeked::Nothing;
                }
                if Instant::now() > deadline {
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
        // Every seek that moved the track is reported: the engine forgets the
        // seek it's holding for only then.
        let frames = match outcome {
            Seeked::To(ts) => Some(t.clock.frames(ts.get())),
            Seeked::End => Some(t.total_frames.unwrap_or((req.to.as_secs_f64() * t.clock.rate as f64) as u64)),
            Seeked::Nothing => None,
        };
        if let Some(frames) = frames {
            // The callback counts frames at the output rate. (Past the end of a
            // track that hasn't started, the next one starts its own count.)
            if started || matches!(outcome, Seeked::To(_)) {
                let played = t.to_output(frames);
                self.publish(|s| s.frames_played.store(played, Ordering::Relaxed));
            }
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
    // A source that can only be read forward.
    #[cfg(test)]
    if let Some(path) = src.url.strip_prefix("test-unseekable://") {
        let f = std::fs::File::open(path).map_err(|e| format!("open {path}: {e}"))?;
        return Ok(Box::new(tests::Unseekable(f)));
    }
    // A file behind a slow server: opening takes half a second.
    #[cfg(test)]
    if let Some(path) = src.url.strip_prefix("test-slow://") {
        let until = Instant::now() + Duration::from_millis(500);
        while Instant::now() < until {
            if abort() {
                return Err("aborted".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
        let f = std::fs::File::open(path).map_err(|e| format!("open {path}: {e}"))?;
        return Ok(Box::new(f));
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
/// interrupts. Counts what went in. A seek interrupts on a `frame` boundary
/// (samples per frame), so the ring never ends mid-frame: the channels would
/// swap for everything after it if no flush followed.
fn push_all(
    p: &mut Producer<f32>,
    mut data: &[f32],
    frame: usize,
    pushed: &mut u64,
    cancel: &AtomicBool,
    seek: &AtomicBool,
) -> Push {
    let frame = frame.max(1);
    while !data.is_empty() {
        if cancel.load(Ordering::Relaxed) || p.is_abandoned() {
            return Push::Cancelled;
        }
        let mut chunk = data;
        if seek.load(Ordering::Acquire) {
            let partial = (*pushed % frame as u64) as usize;
            if partial == 0 {
                return Push::Interrupted;
            }
            // Finish the frame that's half in first.
            chunk = &data[..(frame - partial).min(data.len())];
        }
        let (_, rest) = p.push_partial_slice(chunk);
        let n = chunk.len() - rest.len();
        *pushed += n as u64;
        if n == 0 {
            thread::sleep(Duration::from_millis(5));
        }
        data = &data[n..];
    }
    Push::Done
}

#[cfg(test)]
pub(crate) mod tests;
