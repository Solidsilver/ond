//! Output backends. The engine hands each one a [`RingInput`]; the backend's
//! realtime callback drains it through [`RingInput::fill`].
//!
//! [`CpalBackend`] works everywhere (shared mode). On macOS the native
//! [`crate::coreaudio::CoreAudioBackend`] adds device choice, rate switching, hog
//! mode and integer formats; see [`crate::default_backend`].

use crate::device::{DeviceEvent, DeviceInfo};
use crate::dither::Tpdf;
use crate::signal::{DeviceCaps, RateRange};
use crate::{PlaybackMode, Status};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use crossbeam_channel::{Sender, bounded};
use rtrb::Consumer;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};

/// Software volume changes glide over this long instead of jumping (no zipper noise).
const GAIN_RAMP_SECS: f32 = 0.015;
/// Fade applied to the audio cut off by a seek, in Normal mode only.
const SEEK_FADE_SECS: f32 = 0.004;
/// After a seek, a track start or an underrun, output stays silent until this
/// much audio is queued (or the input ends first).
pub const PREBUFFER_SECS: f32 = 0.4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamFormat {
    pub sample_rate: u32,
    pub channels: u16,
    /// Source bit depth when the codec reports one (FLAC, ALAC, PCM).
    pub bits_per_sample: Option<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("no output device")]
    NoDevice,
    #[error("device does not support {channels}ch @ {rate} Hz")]
    Unsupported { rate: u32, channels: u16 },
    #[error("audio device: {0}")]
    Device(String),
}

/// Control shared between the decoder, the engine and the realtime callback of
/// one ring. Everything is atomics so the callback never blocks.
///
/// Seeking uses two handshakes:
/// - *hold*: the engine bumps `hold_gen` the moment a seek is requested. While
///   `hold_gen > released_gen` the callback plays silence and discards whatever
///   is queued, so stale audio stops immediately even if the decoder's seek is
///   stuck on the network.
/// - *flush*: once the decoder has repositioned, it asks the callback to drop
///   the ring and reset its position counters, and releases the hold in the same
///   step (`pending_release` becomes `released_gen` when the flush is serviced).
#[derive(Debug, Default)]
pub struct RingCtl {
    hold_gen: AtomicU64,
    released_gen: AtomicU64,
    pending_release: AtomicU64,
    flush_requested: AtomicU64,
    flush_acked: AtomicU64,
}

impl RingCtl {
    /// Starts holding; returns the generation the decoder must release.
    pub(crate) fn hold(&self) -> u64 {
        self.hold_gen.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn holding(&self) -> bool {
        self.hold_gen.load(Ordering::Acquire) > self.released_gen.load(Ordering::Acquire)
    }

    /// Asks the callback to empty the ring; releases holds up to `release`.
    pub(crate) fn request_flush(&self, release: u64) -> u64 {
        self.pending_release.fetch_max(release, Ordering::AcqRel);
        self.flush_requested.fetch_add(1, Ordering::AcqRel) + 1
    }

    pub(crate) fn flushed(&self, generation: u64) -> bool {
        self.flush_acked.load(Ordering::Acquire) >= generation
    }
}

/// What a backend reads from: interleaved f32 samples, track-boundary markers
/// and the state the realtime callback keeps between calls.
pub struct RingInput {
    pub samples: Consumer<f32>,
    /// Sample positions (since the ring started or was last flushed) where the
    /// next track begins. Pushed by the decoder when it continues gaplessly.
    pub markers: Consumer<u64>,
    /// Set by the decoder after its last push; starvation after this is the end, not an underrun.
    pub input_done: Arc<AtomicBool>,
    pub ctl: Arc<RingCtl>,
    /// Samples consumed since the ring started or was last flushed.
    pos: u64,
    /// Current volume gain (ramps toward the volume).
    gain: f32,
    /// Current ReplayGain factor: jumps to the next track's value on its first
    /// sample, glides when the setting changes mid-track. Multiplied with `gain`.
    rg: f32,
    /// Silent until enough audio is queued.
    buffering: bool,
    /// Hold generation whose fade-out has been played.
    faded: u64,
    /// Bit depth of the source, when it has one (lossy codecs don't).
    source_bits: Option<u32>,
    /// The decoder resamples this ring's audio.
    resampled: bool,
}

impl std::fmt::Debug for RingInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RingInput").field("queued", &self.samples.slots()).field("pos", &self.pos).finish_non_exhaustive()
    }
}

impl RingInput {
    pub fn new(samples: Consumer<f32>, markers: Consumer<u64>, input_done: Arc<AtomicBool>, ctl: Arc<RingCtl>) -> Self {
        Self {
            samples,
            markers,
            input_done,
            ctl,
            pos: 0,
            gain: 1.0,
            rg: 1.0,
            buffering: true,
            faded: 0,
            source_bits: None,
            resampled: false,
        }
    }

    /// Describes what's in the ring, for the dither decision. `processed` means
    /// the decoder recomputed samples (resampled or downmixed).
    pub fn with_source(mut self, bits: Option<u32>, processed: bool) -> Self {
        self.source_bits = bits;
        self.resampled = processed;
        self
    }

    /// Whether samples may be off a `device_bits`-bit integer grid and need
    /// requantizing: resampled or downmixed, under a gain other than unity
    /// (volume or ReplayGain), or from a deeper (or lossy) source. A
    /// bit-perfect signal never needs it.
    pub fn needs_dither(&self, device_bits: u32) -> bool {
        self.resampled || self.gain * self.rg != 1.0 || self.source_bits.is_none_or(|b| b > device_bits)
    }

    /// The realtime path for an integer device seen through a float stream:
    /// [`fill`](Self::fill), then requantize with TPDF dither to `device_bits`
    /// when [`needs_dither`](Self::needs_dither). With `device_bits` `None`
    /// (float device) samples pass through untouched.
    pub fn fill_for_device(
        &mut self,
        out: &mut [f32],
        channels: usize,
        rate: u32,
        status: &Status,
        device_bits: Option<u32>,
        dither: &mut Tpdf,
    ) -> bool {
        let drained = self.fill(out, channels, rate, status);
        if let Some(bits) = device_bits
            && bits < 32
            && self.needs_dither(bits)
        {
            dither.quantize(out, bits);
        }
        drained
    }

    /// Drops everything queued if a flush was requested. Realtime-safe.
    pub fn service_flush(&mut self) {
        let requested = self.ctl.flush_requested.load(Ordering::Acquire);
        if requested != self.ctl.flush_acked.load(Ordering::Relaxed) {
            self.discard();
            while self.markers.pop().is_ok() {}
            self.pos = 0;
            self.buffering = true;
            self.ctl.released_gen.fetch_max(self.ctl.pending_release.load(Ordering::Acquire), Ordering::AcqRel);
            self.ctl.flush_acked.store(requested, Ordering::Release);
        }
    }

    fn discard(&mut self) -> usize {
        let n = self.samples.slots();
        if let Ok(chunk) = self.samples.read_chunk(n) {
            chunk.commit_all();
        }
        n
    }

    /// The realtime path: copy what's available, split at track boundaries,
    /// apply volume and ReplayGain (Normal mode only), pad with silence, update atomics.
    /// Returns whether everything has been played out (input done, ring empty).
    pub fn fill<T>(&mut self, out: &mut [T], channels: usize, rate: u32, status: &Status) -> bool
    where
        T: SizedSample + FromSample<f32>,
    {
        let silence = T::EQUILIBRIUM;
        let bit_perfect = status.bit_perfect.load(Ordering::Relaxed);
        // Before the pause check, so a seek while paused doesn't wait for resume.
        self.service_flush();

        if self.ctl.holding() {
            let generation = self.ctl.hold_gen.load(Ordering::Acquire);
            let mut start = 0;
            if self.faded < generation {
                self.faded = generation;
                // Normal mode: fade out the first few ms of what was about to play.
                if !bit_perfect && !status.paused.load(Ordering::Relaxed) {
                    let frames = ((SEEK_FADE_SECS * rate as f32) as usize).max(1);
                    start = self.fade_out(out, channels, frames);
                }
            }
            self.discard();
            out[start..].fill(silence);
            self.buffering = true;
            status.buffering.store(true, Ordering::Relaxed);
            return false;
        }
        if status.paused.load(Ordering::Relaxed) {
            out.fill(silence);
            return false;
        }

        let done = self.input_done.load(Ordering::Acquire);
        status.buffered_frames.store((self.samples.slots() / channels.max(1)) as u64, Ordering::Relaxed);
        if self.buffering {
            let channels_u = channels.max(1);
            let target = ((PREBUFFER_SECS * rate as f32) as usize * channels_u).min(self.samples.buffer().capacity() / 2);
            if self.samples.slots() >= target || done {
                self.buffering = false;
                // Fade in from silence (Normal mode); bit-perfect starts on the exact sample.
                if !bit_perfect {
                    self.gain = 0.0;
                    self.rg = status.gains.get(status.track_seq.load(Ordering::Acquire));
                }
            } else {
                out.fill(silence);
                status.buffering.store(true, Ordering::Relaxed);
                return false;
            }
        }
        status.buffering.store(false, Ordering::Relaxed);

        let target = if bit_perfect { 1.0 } else { status.volume() };
        let step = 1.0 / (GAIN_RAMP_SECS * rate.max(1) as f32);
        let tap = status.tap.active();
        let avail = self.samples.slots().min(out.len());
        let avail = avail - avail % channels;
        let mut written = 0;
        while written < avail {
            let until = match self.markers.peek() {
                Ok(&m) if m <= self.pos => {
                    // The next track's first sample is about to play.
                    let _ = self.markers.pop();
                    status.frames_played.store(0, Ordering::Relaxed);
                    let seq = status.track_seq.fetch_add(1, Ordering::AcqRel) + 1;
                    // Its ReplayGain applies from exactly this sample.
                    if !bit_perfect {
                        self.rg = status.gains.get(seq);
                    }
                    continue;
                }
                Ok(&m) => (m - self.pos) as usize,
                Err(_) => usize::MAX,
            };
            let rg_target =
                if bit_perfect { 1.0 } else { status.gains.get(status.track_seq.load(Ordering::Acquire)) };
            let n = (avail - written).min(until);
            let Ok(chunk) = self.samples.read_chunk(n) else { break };
            let (a, b) = chunk.as_slices();
            if tap {
                // A read-only copy for visuals; what reaches the device is untouched.
                status.tap.write(a, b, channels, rate);
            }
            let gains = Gains { vol: &mut self.gain, vol_target: target, rg: &mut self.rg, rg_target, step };
            apply_gain(&mut out[written..written + n], a, b, channels, gains);
            chunk.commit_all();
            written += n;
            self.pos += n as u64;
            status.frames_played.fetch_add((n / channels) as u64, Ordering::Relaxed);
        }
        out[written..].fill(silence);

        if written < out.len() {
            if done {
                return self.samples.is_empty();
            }
            // Ran dry mid-track: count it once and rebuild the prebuffer before resuming.
            status.underruns.fetch_add(1, Ordering::Relaxed);
            self.buffering = true;
        }
        false
    }

    /// Plays up to `frames` queued frames with a linear fade to silence. Returns samples written.
    fn fade_out<T>(&mut self, out: &mut [T], channels: usize, frames: usize) -> usize
    where
        T: SizedSample + FromSample<f32>,
    {
        let n = self.samples.slots().min(out.len()).min(frames * channels);
        let n = n - n % channels;
        let total = (n / channels).max(1) as f32;
        if let Ok(chunk) = self.samples.read_chunk(n) {
            let (a, b) = chunk.as_slices();
            let level = self.gain * self.rg;
            for (i, (o, s)) in out.iter_mut().zip(a.iter().chain(b)).enumerate() {
                let g = (level * (1.0 - (i / channels) as f32 / total)).clamp(0.0, 1.0);
                *o = T::from_sample(*s * g);
            }
            chunk.commit_all();
        }
        n
    }
}

/// The two gain factors of the callback and where each is heading.
struct Gains<'a> {
    vol: &'a mut f32,
    vol_target: f32,
    rg: &'a mut f32,
    rg_target: f32,
    /// Largest change per frame for either factor.
    step: f32,
}

fn toward(x: f32, target: f32, step: f32) -> f32 {
    if x < target { (x + step).min(target) } else { (x - step).max(target) }
}

/// Copies `a` then `b` into `dst` under volume × ReplayGain: one multiply per
/// sample. At unity (always the case in bit-perfect mode once any ramp has
/// finished) samples are copied untouched; no multiply happens at all.
fn apply_gain<T>(dst: &mut [T], a: &[f32], b: &[f32], channels: usize, g: Gains<'_>)
where
    T: SizedSample + FromSample<f32>,
{
    let src = a.iter().chain(b);
    if *g.vol == g.vol_target && *g.rg == g.rg_target {
        let f = *g.vol * *g.rg;
        if f == 1.0 {
            for (o, s) in dst.iter_mut().zip(src) {
                *o = T::from_sample(*s);
            }
        } else {
            for (o, s) in dst.iter_mut().zip(src) {
                *o = T::from_sample(*s * f);
            }
        }
    } else {
        let mut f = *g.vol * *g.rg;
        for (i, (o, s)) in dst.iter_mut().zip(src).enumerate() {
            if i % channels == 0 {
                *g.vol = toward(*g.vol, g.vol_target, g.step);
                *g.rg = toward(*g.rg, g.rg_target, g.step);
                f = *g.vol * *g.rg;
            }
            *o = T::from_sample(*s * f);
        }
    }
}

pub trait OutputBackend: Send {
    fn name(&self) -> &str;
    /// Output devices to choose from. Empty when the backend only uses the default.
    fn devices(&mut self) -> Vec<DeviceInfo> {
        Vec::new()
    }
    /// The device playback goes to (or would go to).
    fn current_device(&mut self) -> Option<DeviceInfo> {
        None
    }
    /// Picks a device by UID; `None` follows the system default. Takes effect at the next `open`.
    fn select_device(&mut self, uid: Option<&str>) -> Result<(), OutputError> {
        match uid {
            None => Ok(()),
            Some(_) => Err(OutputError::Device("this output can't choose devices".into())),
        }
    }
    /// Device list or default-device changes since the last call.
    fn poll_device_events(&mut self) -> Vec<DeviceEvent> {
        Vec::new()
    }
    /// The playback mode changed. Native backends take or release exclusive access here.
    fn set_mode(&mut self, mode: PlaybackMode) {
        let _ = mode;
    }
    /// What the current output device can do, if the backend can tell. Used to
    /// judge whether bit-perfect playback is possible for a file.
    fn device_caps(&mut self) -> Option<DeviceCaps>;
    /// Opens the device for `format` and starts draining `input`. Replaces any open stream.
    fn open(&mut self, format: StreamFormat, input: RingInput, status: Arc<Status>) -> Result<(), OutputError>;
    /// True once the input is done and every sample has been handed to the device.
    fn drained(&self) -> bool;
    /// Stops the stream. When this returns, the callback will not run again.
    fn close(&mut self);
}

/// cpal on the default output device. On macOS cpal sets the device's nominal
/// rate when the stream opens, so matching rates works, but it shares the
/// device (no hog mode) and always runs the float mixer path, so it can never
/// verify bit-perfect output.
pub struct CpalBackend {
    name: String,
    ctl: Option<(Sender<()>, JoinHandle<()>)>,
    drained: Arc<AtomicBool>,
}

impl Default for CpalBackend {
    fn default() -> Self {
        Self { name: "cpal".into(), ctl: None, drained: Arc::new(AtomicBool::new(false)) }
    }
}

impl OutputBackend for CpalBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn device_caps(&mut self) -> Option<DeviceCaps> {
        let device = cpal::default_host().default_output_device()?;
        let name = device.description().map(|d| d.name().to_owned()).unwrap_or_else(|_| "default".into());
        let mut configs = Vec::new();
        let mut any_float = false;
        let mut max_int_bits = None;
        for c in device.supported_output_configs().ok()? {
            configs.push(RateRange { channels: c.channels(), min: c.min_sample_rate(), max: c.max_sample_rate() });
            let bits = match c.sample_format() {
                SampleFormat::F32 | SampleFormat::F64 => {
                    any_float = true;
                    None
                }
                SampleFormat::I32 | SampleFormat::U32 => Some(32),
                SampleFormat::I24 => Some(24),
                SampleFormat::I16 | SampleFormat::U16 => Some(16),
                _ => None,
            };
            max_int_bits = max_int_bits.max(bits);
        }
        Some(DeviceCaps {
            name,
            configs,
            // A float stream hides the device's real word length (CoreAudio's
            // virtual format is always f32), so only integer-only devices tell us.
            max_bits: if any_float { None } else { max_int_bits },
            exclusive: false,
            shared_reason: Some("shared mode (cpal)".into()),
            ..Default::default()
        })
    }

    fn open(&mut self, format: StreamFormat, input: RingInput, status: Arc<Status>) -> Result<(), OutputError> {
        self.close();
        self.drained = Arc::new(AtomicBool::new(false));
        let drained = self.drained.clone();
        let (ctl_tx, ctl_rx) = bounded::<()>(1);
        let (ready_tx, ready_rx) = bounded(1);
        // cpal streams are !Send on some platforms, so each lives on its own thread
        // until told to close.
        let handle = thread::Builder::new()
            .name("audio-output".into())
            .spawn(move || match start_stream(format, input, status, drained) {
                Ok((stream, device)) => {
                    let _ = ready_tx.send(Ok(device));
                    let _ = ctl_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| OutputError::Device(e.to_string()))?;
        let device = match ready_rx.recv() {
            Ok(r) => r,
            Err(_) => Err(OutputError::Device("output thread died".into())),
        };
        let device = match device {
            Ok(d) => d,
            Err(e) => {
                let _ = handle.join();
                return Err(e);
            }
        };
        self.name = format!("cpal · {device}");
        self.ctl = Some((ctl_tx, handle));
        Ok(())
    }

    fn drained(&self) -> bool {
        self.drained.load(Ordering::Acquire)
    }

    fn close(&mut self) {
        if let Some((ctl, handle)) = self.ctl.take() {
            let _ = ctl.send(());
            // Dropping the stream stops the device callback; joining makes that a
            // guarantee for the caller (no stale callback after close returns).
            let _ = handle.join();
        }
    }
}

fn start_stream(
    format: StreamFormat,
    input: RingInput,
    status: Arc<Status>,
    drained: Arc<AtomicBool>,
) -> Result<(cpal::Stream, String), OutputError> {
    let dev_err = |e: cpal::Error| OutputError::Device(e.to_string());
    let device = cpal::default_host().default_output_device().ok_or(OutputError::NoDevice)?;
    let device_name = device.description().map(|d| d.name().to_owned()).unwrap_or_else(|_| "default".into());

    let range = device
        .supported_output_configs()
        .map_err(dev_err)?
        .filter(|c| {
            c.channels() == format.channels
                && c.min_sample_rate() <= format.sample_rate
                && c.max_sample_rate() >= format.sample_rate
        })
        // Prefer float, then the widest integer format.
        .max_by_key(|c| match c.sample_format() {
            SampleFormat::F32 => 4,
            SampleFormat::I32 => 3,
            SampleFormat::I24 => 2,
            SampleFormat::I16 => 1,
            _ => 0,
        })
        .ok_or(OutputError::Unsupported { rate: format.sample_rate, channels: format.channels })?;
    let sample_format = range.sample_format();
    let config: StreamConfig = range.with_sample_rate(format.sample_rate).into();

    // 16- and 24-bit integer formats get TPDF dither when samples are off their
    // grid; 32-bit integers are finer than the f32 samples themselves.
    let stream = match sample_format {
        SampleFormat::F32 => build::<f32>(&device, config, input, status, drained, None),
        SampleFormat::I32 => build::<i32>(&device, config, input, status, drained, None),
        SampleFormat::I24 => build::<cpal::I24>(&device, config, input, status, drained, Some(24)),
        SampleFormat::I16 => build::<i16>(&device, config, input, status, drained, Some(16)),
        other => return Err(OutputError::Device(format!("unsupported sample format {other}"))),
    }
    .map_err(dev_err)?;
    stream.play().map_err(dev_err)?;
    Ok((stream, format!("{device_name} · {sample_format}")))
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut input: RingInput,
    status: Arc<Status>,
    drained: Arc<AtomicBool>,
    device_bits: Option<u32>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let rate = config.sample_rate;
    // Allocated here, never in the callback.
    let mut scratch = vec![0.0f32; SCRATCH_FRAMES * channels];
    let mut dither = Tpdf::default();
    let callback = move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
        if fill_converted(&mut input, out, &mut scratch, channels, rate, &status, device_bits, &mut dither) {
            drained.store(true, Ordering::Release);
        }
    };
    let on_error = |e: cpal::Error| tracing::warn!("output stream: {e}");
    device.build_output_stream(config, callback, on_error, None)
}

/// Frames the cpal callback converts at a time (larger than any buffer it asks for).
const SCRATCH_FRAMES: usize = 16_384;

/// Fills `out` from the ring. For an integer device (`device_bits`) the samples
/// go through `scratch` first so they're dithered to its grid where needed,
/// rather than truncated by the conversion.
#[allow(clippy::too_many_arguments)]
fn fill_converted<T>(
    input: &mut RingInput,
    out: &mut [T],
    scratch: &mut [f32],
    channels: usize,
    rate: u32,
    status: &Status,
    device_bits: Option<u32>,
    dither: &mut Tpdf,
) -> bool
where
    T: SizedSample + FromSample<f32>,
{
    if device_bits.is_none() || scratch.is_empty() {
        return input.fill(out, channels, rate, status);
    }
    let mut drained = false;
    for chunk in out.chunks_mut(scratch.len()) {
        let buf = &mut scratch[..chunk.len()];
        drained = input.fill_for_device(buf, channels, rate, status, device_bits, dither);
        for (o, s) in chunk.iter_mut().zip(buf.iter()) {
            *o = T::from_sample(*s);
        }
    }
    drained
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 1000;

    /// A ring with `samples` queued. Prebuffering is skipped (the ring is small),
    /// unless the test turns it back on.
    fn ring(samples: &[f32], done: bool) -> (rtrb::Producer<f32>, rtrb::Producer<u64>, RingInput) {
        let (mut p, c) = rtrb::RingBuffer::new(64);
        p.push_entire_slice(samples).unwrap();
        let (m, mc) = rtrb::RingBuffer::new(4);
        let mut input = RingInput::new(c, mc, Arc::new(AtomicBool::new(done)), Arc::default());
        input.buffering = false;
        (p, m, input)
    }

    fn bit_perfect_status() -> Status {
        let s = Status::default();
        s.bit_perfect.store(true, Ordering::Relaxed);
        s
    }

    #[test]
    fn fill_copies_whole_frames_and_pads() {
        let status = bit_perfect_status();
        let (_p, _m, mut input) = ring(&[0.1, 0.2, 0.3, 0.4, 0.5], false);
        let mut out = [9.0f32; 8];
        assert!(!input.fill(&mut out, 2, RATE, &status));
        // 5 samples available but only 2 whole stereo frames are taken.
        assert_eq!(out, [0.1, 0.2, 0.3, 0.4, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(status.frames_played.load(Ordering::Relaxed), 2);
        assert_eq!(status.underruns.load(Ordering::Relaxed), 1);
        // An underrun drops back into buffering.
        assert!(input.buffering);
    }

    #[test]
    fn the_tap_copies_what_plays_without_changing_it() {
        let samples: Vec<f32> = (0..32).map(|i| (i as f32 * 0.37).sin() * 0.8).collect();
        for bit_perfect in [true, false] {
            let play = |tap: bool| {
                let status = Status::default();
                status.bit_perfect.store(bit_perfect, Ordering::Relaxed);
                status.set_volume(0.5);
                status.tap.set_active(tap);
                let (_p, _m, mut input) = ring(&samples, false);
                let mut out = [0.0f32; 32];
                input.fill(&mut out, 2, RATE, &status);
                let mut copy = [0.0f32; 16];
                let (written, _) = status.tap.read(&mut copy);
                (out, written, copy)
            };
            let (plain, none, _) = play(false);
            let (tapped, written, copy) = play(true);
            assert_eq!(plain.map(f32::to_bits), tapped.map(f32::to_bits), "the device gets the same samples, bit for bit");
            assert_eq!((none, written), (0, 16), "off, the tap is skipped; on, it sees every frame");
            // Mono, before the volume.
            for (i, c) in copy.iter().enumerate() {
                assert!((c - (samples[2 * i] + samples[2 * i + 1]) / 2.0).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn starvation_after_input_done_is_drain_not_underrun() {
        let status = bit_perfect_status();
        let (_p, _m, mut input) = ring(&[0.5, 0.5], true);
        let mut out = [0.0f32; 4];
        assert!(input.fill(&mut out, 2, RATE, &status));
        assert_eq!(status.underruns.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn flush_drops_queued_audio_once() {
        let status = bit_perfect_status();
        let (_p, _m, mut input) = ring(&[0.1, 0.2, 0.3, 0.4], false);
        let generation = input.ctl.request_flush(0);
        let mut out = [9.0f32; 4];
        input.fill(&mut out, 2, RATE, &status);
        assert_eq!(out, [0.0; 4]);
        assert!(input.ctl.flushed(generation));
        assert!(input.samples.is_empty());
    }

    #[test]
    fn paused_outputs_silence_without_consuming() {
        let status = bit_perfect_status();
        status.paused.store(true, Ordering::Relaxed);
        let (_p, _m, mut input) = ring(&[0.5, 0.5], false);
        let mut out = [1i16; 2];
        input.fill(&mut out, 2, RATE, &status);
        assert_eq!(out, [0, 0]);
        assert_eq!(input.samples.slots(), 2);
    }

    #[test]
    fn marker_splits_tracks_at_the_exact_sample() {
        let status = bit_perfect_status();
        // Track A is 3 stereo frames, B starts at sample 6.
        let (_p, mut m, mut input) = ring(&[1., 1., 2., 2., 3., 3., 4., 4., 5., 5.], false);
        m.push(6).unwrap();
        let mut out = [0.0f32; 4];
        input.fill(&mut out, 2, RATE, &status);
        assert_eq!((status.track_seq.load(Ordering::Relaxed), status.frames_played.load(Ordering::Relaxed)), (0, 2));
        // This callback plays A's last frame and B's first: the boundary is mid-buffer.
        input.fill(&mut out, 2, RATE, &status);
        assert_eq!(out, [3., 3., 4., 4.]);
        assert_eq!(status.track_seq.load(Ordering::Relaxed), 1);
        assert_eq!(status.frames_played.load(Ordering::Relaxed), 1, "position is relative to B");
    }

    #[test]
    fn bit_perfect_never_touches_samples() {
        let status = bit_perfect_status();
        status.set_volume(0.3);
        let src: Vec<f32> = (0..32).map(|i| (i as f32 * 0.123_456_7).sin()).collect();
        let (_p, _m, mut input) = ring(&src, false);
        let mut out = [0.0f32; 32];
        input.fill(&mut out, 2, RATE, &status);
        assert_eq!(out.map(f32::to_bits).to_vec(), src.iter().map(|s| s.to_bits()).collect::<Vec<_>>());
    }

    #[test]
    fn gain_ramps_smoothly_then_holds() {
        let status = Status::default();
        status.set_volume(0.5);
        // 1 kHz "device": the 15 ms ramp is 15 frames.
        let (_p, _m, mut input) = ring(&[1.0; 64], false);
        let mut out = [0.0f32; 40];
        input.fill(&mut out, 1, RATE, &status);
        let steps: Vec<f32> = out.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
        assert!(steps.iter().all(|s| *s <= 1.0 / 15.0 + 1e-6), "no jumps: {out:?}");
        assert!((out[0] - (1.0 - 1.0 / 15.0)).abs() < 1e-6);
        assert!(out[20..].iter().all(|s| *s == 0.5), "settles on the target: {out:?}");
    }

    #[test]
    fn switching_to_bit_perfect_ramps_to_unity_then_bypasses() {
        let status = Status::default();
        status.set_volume(0.5);
        let src: Vec<f32> = (0..64).map(|i| i as f32 / 64.0).collect();
        let (_p, _m, mut input) = ring(&src, false);
        let mut out = [0.0f32; 24];
        input.fill(&mut out, 1, RATE, &status);
        assert_eq!(input.gain, 0.5);

        status.bit_perfect.store(true, Ordering::Relaxed);
        let mut out = [0.0f32; 40];
        input.fill(&mut out, 1, RATE, &status);
        // ~7.5 frames of ramp (0.5 → 1.0), then exact copies.
        assert_eq!(input.gain, 1.0);
        assert_eq!(out[20..].iter().map(|s| s.to_bits()).collect::<Vec<_>>(), src[44..64].iter().map(|s| s.to_bits()).collect::<Vec<_>>());
    }

    #[test]
    fn replay_gain_switches_on_the_first_sample_of_the_next_track() {
        let status = Status::default();
        status.set_volume(1.0);
        status.gains.set(0, 0.5);
        status.gains.set(1, 0.25);
        // Track A: 3 stereo frames, B starts at sample 6. All samples 0.8.
        let (_p, mut m, mut input) = ring(&[0.8; 12], false);
        input.rg = 0.5; // as set when A's prebuffer finished
        m.push(6).unwrap();
        let mut out = [0.0f32; 12];
        input.fill(&mut out, 2, RATE, &status);
        assert_eq!(&out[..6], &[0.4; 6], "A at its gain");
        assert_eq!(&out[6..], &[0.2; 6], "B at its gain from its first sample, no glide across the boundary");
        assert_eq!(status.track_seq.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn replay_gain_and_volume_multiply_once() {
        let status = Status::default();
        status.set_volume(0.5);
        status.gains.set(0, 0.5);
        let (_p, _m, mut input) = ring(&[1.0; 8], false);
        (input.gain, input.rg) = (0.5, 0.5);
        let mut out = [0.0f32; 8];
        input.fill(&mut out, 2, RATE, &status);
        assert_eq!(out, [0.25; 8]);
        assert!(input.needs_dither(24), "a gained signal is off the integer grid");
    }

    #[test]
    fn changing_replay_gain_mid_track_glides() {
        let status = Status::default();
        status.set_volume(1.0);
        let (_p, _m, mut input) = ring(&[1.0; 64], false);
        status.gains.set(0, 0.5);
        let mut out = [0.0f32; 40];
        input.fill(&mut out, 1, RATE, &status);
        let steps: Vec<f32> = out.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
        assert!(steps.iter().all(|s| *s <= 1.0 / 15.0 + 1e-6), "no jumps: {out:?}");
        assert!(out[20..].iter().all(|s| *s == 0.5), "settles on the new gain: {out:?}");
    }

    #[test]
    fn bit_perfect_ignores_replay_gain() {
        let status = bit_perfect_status();
        status.set_volume(0.3);
        status.gains.set(0, 0.4);
        status.gains.set(1, 0.2);
        let src: Vec<f32> = (0..32).map(|i| (i as f32 * 0.123_456_7).sin()).collect();
        let (_p, mut m, mut input) = ring(&src, false);
        m.push(16).unwrap();
        let mut out = [0.0f32; 32];
        input.fill(&mut out, 2, RATE, &status);
        assert_eq!(out.map(f32::to_bits).to_vec(), src.iter().map(|s| s.to_bits()).collect::<Vec<_>>());
        assert_eq!(status.track_seq.load(Ordering::Relaxed), 1);
        assert_eq!(input.gain * input.rg, 1.0, "nothing to dither away");
    }

    #[test]
    fn prebuffer_end_picks_up_the_current_tracks_gain() {
        let status = Status::default();
        status.set_volume(1.0);
        status.gains.set(0, 0.5);
        let (_p, _m, mut input) = ring(&[1.0; 64], false);
        input.buffering = true;
        let mut out = [0.0f32; 40];
        input.fill(&mut out, 1, RATE, &status);
        // Fades in from silence to the track's gain; never overshoots it.
        assert!(out.iter().all(|s| *s <= 0.5 + 1e-6), "{out:?}");
        assert_eq!(out[39], 0.5);
    }

    #[test]
    fn hold_silences_and_discards_until_the_decoder_releases_it() {
        let status = Status::default();
        let (mut p, _m, mut input) = ring(&[0.5; 32], false);
        let generation = input.ctl.hold();
        let mut out = [0.0f32; 16];
        input.fill(&mut out, 2, RATE, &status);
        // Normal mode: a short fade (4 frames at 1 kHz) then silence.
        assert!(out[..8].iter().all(|s| *s <= 0.5) && out[0] > 0.0, "fade: {out:?}");
        assert!(out[8..].iter().all(|s| *s == 0.0));
        assert!(input.samples.is_empty(), "stale audio is dropped");

        // Stale audio the decoder pushes before it notices the seek is dropped too.
        p.push_entire_slice(&[0.7; 8]).unwrap();
        input.fill(&mut out, 2, RATE, &status);
        assert_eq!(out, [0.0; 16]);
        assert!(input.samples.is_empty());

        // The decoder repositions and releases the hold through a flush.
        input.ctl.request_flush(generation);
        p.push_entire_slice(&[0.25; 16]).unwrap();
        input.fill(&mut out, 2, RATE, &status);
        assert!(!input.ctl.holding());
        assert!(status.buffering.load(Ordering::Relaxed), "back to prebuffering after the flush");
    }

    #[test]
    fn buffering_waits_for_the_prebuffer_target() {
        let status = bit_perfect_status();
        let (mut p, _m, mut input) = ring(&[], false);
        input.buffering = true;
        // Ring capacity 64 caps the target at 32 samples.
        p.push_entire_slice(&[0.5; 20]).unwrap();
        let mut out = [9.0f32; 8];
        input.fill(&mut out, 2, RATE, &status);
        assert_eq!(out, [0.0; 8]);
        assert!(status.buffering.load(Ordering::Relaxed));
        assert_eq!(input.samples.slots(), 20, "nothing consumed while buffering");
        p.push_entire_slice(&[0.5; 12]).unwrap();
        input.fill(&mut out, 2, RATE, &status);
        assert_eq!(out, [0.5; 8]);
        assert!(!status.buffering.load(Ordering::Relaxed));
    }

    #[test]
    fn integer_device_path_dithers_only_off_grid_audio() {
        let status = bit_perfect_status();
        let mut dither = Tpdf::default();
        // 16-bit source on a 16-bit device: on the grid already, passed through exactly.
        let src: Vec<f32> = (0..32).map(|i| (i as f32 * 977.0 - 15_000.0) / 32_768.0).collect();
        let (_p, _m, input) = ring(&src, false);
        let mut input = input.with_source(Some(16), false);
        let mut out = [0.0f32; 32];
        input.fill_for_device(&mut out, 2, RATE, &status, Some(16), &mut dither);
        assert_eq!(out.map(f32::to_bits).to_vec(), src.iter().map(|s| s.to_bits()).collect::<Vec<_>>());

        // Resampled audio is requantized onto the 16-bit grid.
        let off: Vec<f32> = (0..32).map(|i| (i as f32 * 0.031_7).sin() * 0.5).collect();
        let (_p, _m, input) = ring(&off, false);
        let mut input = input.with_source(Some(16), true);
        input.fill_for_device(&mut out, 2, RATE, &status, Some(16), &mut dither);
        assert!(out.iter().all(|s| (*s * 32_768.0).fract() == 0.0));
        assert!(out.iter().zip(&off).all(|(a, b)| (a - b).abs() <= 1.5 / 32_768.0));

        // 24-bit source on a 16-bit device needs it too; a float device never does.
        let (_p, _m, input) = ring(&off, false);
        let mut input = input.with_source(Some(24), false);
        assert!(input.needs_dither(16) && !input.needs_dither(24));
        input.fill_for_device(&mut out, 2, RATE, &status, None, &mut dither);
        assert_eq!(out.map(f32::to_bits).to_vec(), off.iter().map(|s| s.to_bits()).collect::<Vec<_>>());
    }

    #[test]
    fn a_16_bit_cpal_device_gets_dither_not_truncation() {
        // A quarter of a 16-bit step, from a 24-bit source: truncation would
        // turn all of it into silence; dither keeps it, on average.
        let quarter = 0.25 / 32_768.0;
        let samples = vec![quarter as f32; 64];
        let (_p, _m, input) = ring(&samples, true);
        let mut input = input.with_source(Some(24), false);
        let status = Status::default();
        status.set_volume(1.0);
        let mut out = [0i16; 64];
        let mut scratch = vec![0.0f32; 128];
        fill_converted(&mut input, &mut out, &mut scratch, 2, RATE, &status, Some(16), &mut Tpdf::default());
        assert!(out.iter().any(|&s| s != 0), "dither keeps sub-step detail");
        assert!(out.iter().all(|&s| (-2..=2).contains(&s)), "{out:?}");

        // Bit-perfect content already on the grid passes through exactly.
        let exact: Vec<f32> = (0..64).map(|i| (i as f32 - 32.0) / 32_768.0).collect();
        let (_p, _m, input) = ring(&exact, true);
        let mut input = input.with_source(Some(16), false);
        let status = bit_perfect_status();
        let mut out = [0i16; 64];
        fill_converted(&mut input, &mut out, &mut scratch, 2, RATE, &status, Some(16), &mut Tpdf::default());
        assert_eq!(out.to_vec(), (0..64).map(|i| i as i16 - 32).collect::<Vec<_>>());
    }
}
