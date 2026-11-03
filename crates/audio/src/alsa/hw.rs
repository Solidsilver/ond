//! Playback straight to a `hw:` device: no sound server, no conversion. The
//! device runs at the ring's rate in the widest integer format it takes, and
//! a thread writes the ring's samples to it as exact integers.

use super::cards::{self, FORMATS, Pcm};
use crate::Status;
use crate::dither::Tpdf;
use crate::output::{RingInput, StreamFormat};
use ::alsa::pcm::{Access, Format, HwParams, PCM};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// How long the device's own buffer is: what a pause or seek waits on.
const BUFFER_US: u32 = 80_000;
const PERIOD_US: u32 = 10_000;
/// How long to wait for a sound server that agreed to let go of the card.
const BUSY_PATIENCE: Duration = Duration::from_millis(1_500);

/// How samples are laid out for the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Packing {
    S16,
    /// 24 bits in the low three bytes of a 32-bit word (`S24_LE`).
    S24In32,
    /// 24 bits in three bytes (`S24_3LE`).
    S24Packed,
    S32,
}

impl Packing {
    fn of(format: Format) -> Option<Self> {
        Some(match format {
            Format::S16LE => Self::S16,
            Format::S24LE => Self::S24In32,
            Format::S243LE => Self::S24Packed,
            Format::S32LE => Self::S32,
            _ => return None,
        })
    }

    pub fn bits(self) -> u32 {
        match self {
            Self::S16 => 16,
            Self::S24In32 | Self::S24Packed => 24,
            Self::S32 => 32,
        }
    }

    fn bytes(self) -> usize {
        match self {
            Self::S16 => 2,
            Self::S24Packed => 3,
            Self::S24In32 | Self::S32 => 4,
        }
    }

    /// Appends `s` (a sample on the `bits` grid, or finer for 32 bits) as this
    /// packing's little-endian bytes. Exact: f32 holds every 16- and 24-bit value.
    pub fn push(self, s: f32, out: &mut Vec<u8>) {
        let v = to_int(s, self.bits());
        match self {
            Self::S16 => out.extend_from_slice(&(v as i16).to_le_bytes()),
            Self::S24In32 | Self::S32 => out.extend_from_slice(&v.to_le_bytes()),
            Self::S24Packed => out.extend_from_slice(&v.to_le_bytes()[..3]),
        }
    }
}

/// `s` scaled to a signed `bits`-bit integer, clamped (+1.0 is one step over the top).
fn to_int(s: f32, bits: u32) -> i32 {
    let scale = (1u64 << (bits - 1)) as f64;
    (s as f64 * scale).round().clamp(-scale, scale - 1.0) as i32
}

/// A device configured for one format, not yet playing.
pub struct Prepared {
    pcm: PCM,
    packing: Packing,
    /// The device's channels: the ring's, or more (the rest get silence).
    channels: u16,
    period: usize,
}

impl Prepared {
    pub fn bits(&self) -> u32 {
        self.packing.bits()
    }
}

/// Opens `pcm` and sets it to `format` exactly: its rate, its channels (or
/// a device with more), and the widest integer format the device takes.
pub fn prepare(pcm: &Pcm, format: StreamFormat) -> Result<Prepared, String> {
    let device = cards::open_hw(pcm, BUSY_PATIENCE).map_err(|e| cards::open_error(pcm, &e))?;
    let (packing, channels) = {
        let hwp = HwParams::any(&device).map_err(|e| e.to_string())?;
        hwp.set_access(Access::RWInterleaved).map_err(|e| e.to_string())?;
        let want = format.channels as u32;
        let channels = if hwp.test_channels(want).is_ok() {
            want
        } else {
            match hwp.get_channels_min() {
                Ok(min) if min > want => min,
                _ => return Err(format!("{} can't play {want} channels", pcm.name)),
            }
        };
        hwp.set_channels(channels).map_err(|e| e.to_string())?;
        if !cards::set_rate_exactly(&hwp, format.sample_rate) {
            return Err(format!("{} can't run at {} Hz", pcm.name, format.sample_rate));
        }
        let (alsa_format, _) = FORMATS.iter().find(|(f, _)| hwp.test_format(*f).is_ok()).ok_or_else(|| format!("{} takes no integer format", pcm.name))?;
        hwp.set_format(*alsa_format).map_err(|e| e.to_string())?;
        hwp.set_period_time_near(PERIOD_US, ::alsa::ValueOr::Nearest).ok();
        hwp.set_buffer_time_near(BUFFER_US, ::alsa::ValueOr::Nearest).ok();
        device.hw_params(&hwp).map_err(|e| format!("{}: {e}", pcm.name))?;
        (Packing::of(*alsa_format).expect("one of FORMATS"), channels as u16)
    };
    let (buffer, period) = device.get_params().map_err(|e| e.to_string())?;
    // Start once the buffer is nearly full, not on the first period.
    if let Ok(swp) = device.sw_params_current() {
        let _ = swp.set_start_threshold(buffer.saturating_sub(period) as ::alsa::pcm::Frames);
        let _ = device.sw_params(&swp);
    }
    Ok(Prepared { pcm: device, packing, channels, period: period.max(64) as usize })
}

/// A device playing a ring.
pub struct Stream {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub bits: u32,
    pub rate: u32,
}

impl Stream {
    /// Plays `input` (at `format`) on the prepared device until stopped.
    /// `drained` is set once the input is done and the device has played it.
    pub fn start(prepared: Prepared, format: StreamFormat, mut input: RingInput, status: Arc<Status>, drained: Arc<AtomicBool>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let bits = prepared.bits();
        let rate = format.sample_rate;
        let thread = thread::Builder::new().name("audio-alsa".into()).spawn(move || {
            let Prepared { pcm, packing, channels: device_channels, period } = prepared;
            let channels = format.channels as usize;
            let device_channels = device_channels as usize;
            let mut samples = vec![0.0f32; period * channels];
            let mut bytes = Vec::with_capacity(period * device_channels * packing.bytes());
            // 32-bit integers are finer than the f32 samples: nothing to dither to.
            let dither_bits = (packing.bits() < 32).then_some(packing.bits());
            let mut dither = Tpdf::default();
            let frame_bytes = device_channels * packing.bytes();
            let mut finishing = false;
            let io = pcm.io_bytes();
            while !stop2.load(Ordering::Relaxed) {
                if input.fill_for_device(&mut samples, channels, rate, &status, dither_bits, &mut dither) {
                    finishing = true;
                }
                bytes.clear();
                pack_frames(packing, &samples, channels, device_channels, &mut bytes);
                let mut written = 0;
                while written < bytes.len() && !stop2.load(Ordering::Relaxed) {
                    match io.writei(&bytes[written..]) {
                        Ok(frames) => written += frames * frame_bytes,
                        Err(e) => {
                            if let Err(e) = pcm.try_recover(e, true) {
                                tracing::warn!("ALSA output: {e}");
                                // Gone (unplugged): say it's done so the engine moves on.
                                drained.store(true, Ordering::Release);
                                return;
                            }
                        }
                    }
                }
                // Done once what's queued in the device has played out.
                if finishing && pcm.delay().map_or(true, |d| d <= period as ::alsa::pcm::Frames) {
                    drained.store(true, Ordering::Release);
                }
            }
            let _ = pcm.drop();
        });
        let thread = thread.expect("spawn ALSA output thread");
        Self { stop, thread: Some(thread), bits, rate }
    }

    /// Stops playback and closes the device. At most one period's write plus the close.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Packs interleaved `samples` (`channels` per frame) for a device with
/// `device_channels`, the extra channels silent.
pub fn pack_frames(packing: Packing, samples: &[f32], channels: usize, device_channels: usize, out: &mut Vec<u8>) {
    for frame in samples.chunks_exact(channels) {
        for &s in frame {
            packing.push(s, out);
        }
        for _ in channels..device_channels {
            packing.push(0.0, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every value across the range, both signs, odd ones and the extremes.
    fn pattern(bits: u32) -> Vec<i32> {
        let top = 1i64 << (bits - 1);
        let mut v: Vec<i32> = (0..8_192i64).map(|i| ((i * 2_654_435) % (2 * top) - top) as i32).collect();
        v.extend([-(top as i32), (top - 1) as i32, 0, 1, -1]);
        v
    }

    fn unpack(packing: Packing, bytes: &[u8]) -> Vec<i32> {
        bytes
            .chunks_exact(packing.bytes())
            .map(|b| match packing {
                Packing::S16 => i16::from_le_bytes([b[0], b[1]]) as i32,
                Packing::S24Packed => i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8,
                Packing::S24In32 | Packing::S32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            })
            .collect()
    }

    #[test]
    fn packing_is_bit_exact() {
        for (packing, bits) in [(Packing::S16, 16), (Packing::S24In32, 24), (Packing::S24Packed, 24)] {
            let ints = pattern(bits);
            let samples: Vec<f32> = ints.iter().map(|v| *v as f32 / (1i64 << (bits - 1)) as f32).collect();
            let mut bytes = Vec::new();
            pack_frames(packing, &samples, 1, 1, &mut bytes);
            assert_eq!(unpack(packing, &bytes), ints, "{packing:?}");
        }
        // 24-bit audio on a 32-bit device: the same values, eight bits up.
        let ints = pattern(24);
        let samples: Vec<f32> = ints.iter().map(|v| *v as f32 / (1 << 23) as f32).collect();
        let mut bytes = Vec::new();
        pack_frames(Packing::S32, &samples, 1, 1, &mut bytes);
        let back: Vec<i32> = unpack(Packing::S32, &bytes).iter().map(|v| v >> 8).collect();
        assert_eq!(back, ints);
    }

    #[test]
    fn full_scale_clamps_and_extra_channels_are_silent() {
        let mut bytes = Vec::new();
        pack_frames(Packing::S16, &[1.0, -1.0], 2, 4, &mut bytes);
        assert_eq!(unpack(Packing::S16, &bytes), vec![i16::MAX as i32, i16::MIN as i32, 0, 0]);
    }

    /// The whole path bit-perfect mode uses (ring → fill → dither decision →
    /// packing) on a 24-bit pattern, into bytes instead of the hardware.
    #[test]
    fn the_ring_reaches_the_device_untouched() {
        let ints = pattern(24);
        let samples: Vec<f32> = ints.iter().flat_map(|v| [*v as f32 / (1 << 23) as f32; 2]).collect();
        let (mut p, c) = rtrb::RingBuffer::new(samples.len());
        p.push_entire_slice(&samples).unwrap();
        let (_m, mc) = rtrb::RingBuffer::new(4);
        let mut input = RingInput::new(c, mc, Arc::new(AtomicBool::new(true)), Arc::default()).with_source(Some(24), false);
        let status = Arc::new(Status::default());
        status.bit_perfect.store(true, Ordering::Relaxed);
        let mut out = vec![0.0f32; samples.len()];
        let mut dither = Tpdf::default();
        for chunk in out.chunks_mut(512) {
            input.fill_for_device(chunk, 2, 96_000, &status, Some(24), &mut dither);
        }
        let mut bytes = Vec::new();
        pack_frames(Packing::S24Packed, &out, 2, 2, &mut bytes);
        let back: Vec<i32> = unpack(Packing::S24Packed, &bytes).into_iter().step_by(2).collect();
        assert_eq!(back, ints);
    }
}
