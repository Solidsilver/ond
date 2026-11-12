//! What's playing, as the visuals see it: an FFT of the audio tap's latest
//! samples, once per frame that needs it, bucketed into log-spaced bands and
//! smoothed (quick to rise, slower to fall), plus the music's feel for the
//! visualizer (see [`pulse`](super::pulse)). The FFT runs on the UI thread: a
//! 4096-point real transform is tens of microseconds, less than drawing the bars.

use super::pulse::{Feel, Pulse};
use audio::tap::Tap;
use realfft::RealFftPlanner;
use realfft::num_complex::Complex;
use std::sync::Arc;

/// Samples per transform.
const FFT: usize = 4096;
/// Bands, log-spaced from [`LOW_HZ`] to [`HIGH_HZ`]; consumers group them.
pub const BANDS: usize = 64;
pub const LOW_HZ: f32 = 40.0;
pub const HIGH_HZ: f32 = 16_000.0;
/// The level range shown, in dBFS of a band's loudest bin.
const FLOOR_DB: f32 = -75.0;
const TOP_DB: f32 = -12.0;
/// How fast a band falls back, in full heights per second.
const FALL: f32 = 1.6;

/// One frame's view of the music.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// Smoothed band levels, 0..=1, low to high.
    pub bands: [f32; BANDS],
    /// The same, unsmoothed (for a spectrogram).
    pub raw: [f32; BANDS],
    /// Low, middle and high, beats and the waveform, each against the track's own loudness.
    pub feel: Feel,
    /// New audio arrived for this frame (false while paused or stalled).
    pub fresh: bool,
}

impl Default for Frame {
    fn default() -> Self {
        Self { bands: [0.0; BANDS], raw: [0.0; BANDS], feel: Feel::default(), fresh: false }
    }
}

pub struct Listener {
    fft: Arc<dyn realfft::RealToComplex<f32>>,
    window: Vec<f32>,
    input: Vec<f32>,
    /// The input as read, before the window shapes it.
    samples: Vec<f32>,
    output: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    frame: Frame,
    /// The pass the frame was made for, and the tap's count then.
    pass: Option<u64>,
    written: u64,
    /// When the last frame was made (frame clock), for the smoothing.
    time: f64,
    pulse: Pulse,
}

impl Default for Listener {
    fn default() -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FFT);
        let window = (0..FFT).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FFT as f32).cos()).collect();
        Self {
            input: fft.make_input_vec(),
            samples: vec![0.0; FFT],
            output: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            fft,
            window,
            frame: Frame::default(),
            pass: None,
            written: 0,
            time: 0.0,
            pulse: Pulse::default(),
        }
    }
}

impl Listener {
    /// This pass's frame, made from `tap` the first time it's asked for in
    /// pass `pass` (several visuals can share it) at frame time `now`.
    pub fn frame(&mut self, tap: &Tap, pass: u64, now: f64) -> &Frame {
        if self.pass != Some(pass) {
            self.pass = Some(pass);
            let dt = (now - self.time).clamp(0.0, 0.25) as f32;
            self.time = now;
            let (written, rate) = tap.read(&mut self.input);
            let fresh = written != self.written && rate > 0;
            self.written = written;
            if fresh {
                self.analyse(rate, dt);
            } else {
                self.settle(dt);
            }
            self.frame.fresh = fresh;
        }
        &self.frame
    }

    /// Whether anything asked for a frame in pass `pass` (the tap stays on only then).
    pub fn used_in(&self, pass: u64) -> bool {
        self.pass == Some(pass)
    }

    fn analyse(&mut self, rate: u32, dt: f32) {
        self.samples.copy_from_slice(&self.input);
        for (x, w) in self.input.iter_mut().zip(&self.window) {
            *x *= w;
        }
        if self.fft.process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch).is_err() {
            return;
        }
        // A full-scale sine's bin reads 1.0 (0 dBFS): the Hann window's gain is half.
        let norm = 4.0 / FFT as f32;
        let mags: Vec<f32> = self.output.iter().map(|c| c.norm() * norm).collect();
        let raw = bands(&mags, rate);
        self.pulse.hear(&self.samples, &mags, rate, dt);
        let f = &mut self.frame;
        f.raw = raw;
        for (b, r) in f.bands.iter_mut().zip(raw) {
            *b = if r > *b { r } else { (*b - FALL * dt).max(r) };
        }
        f.feel.copy_from(self.pulse.feel());
    }

    /// No new audio: everything falls toward rest.
    fn settle(&mut self, dt: f32) {
        let f = &mut self.frame;
        for b in &mut f.bands {
            *b = (*b - FALL * dt).max(0.0);
        }
        f.raw = [0.0; BANDS];
        self.pulse.rest(dt);
        f.feel.copy_from(self.pulse.feel());
    }
}

/// The edges of the bands, in Hz: [`BANDS`] + 1 of them, log-spaced.
pub fn edges() -> [f32; BANDS + 1] {
    std::array::from_fn(|i| LOW_HZ * (HIGH_HZ / LOW_HZ).powf(i as f32 / BANDS as f32))
}

/// Magnitudes of FFT bins (0..=Nyquist, for [`FFT`] samples at `rate`) as
/// band levels 0..=1: each band's loudest bin, on a dB scale. A band too
/// narrow to hold a bin (the low end) takes the bin nearest its middle.
pub fn bands(mags: &[f32], rate: u32) -> [f32; BANDS] {
    let hz_per_bin = rate as f32 / FFT as f32;
    let e = edges();
    std::array::from_fn(|i| {
        let (lo, hi) = (e[i] / hz_per_bin, e[i + 1] / hz_per_bin);
        let (a, b) = (lo.ceil() as usize, (hi.ceil() as usize).min(mags.len()));
        let peak = if a < b { mags[a..b].iter().copied().fold(0.0, f32::max) } else { mags.get(((lo + hi) / 2.0).round() as usize).copied().unwrap_or(0.0) };
        let db = 20.0 * peak.max(1e-9).log10();
        ((db - FLOOR_DB) / (TOP_DB - FLOOR_DB)).clamp(0.0, 1.0)
    })
}

/// `bands` grouped into `n` (the loudest of each group), for a smaller display.
pub fn grouped(bands: &[f32; BANDS], n: usize) -> Vec<f32> {
    (0..n).map(|i| bands[i * BANDS / n..((i + 1) * BANDS / n).max(i * BANDS / n + 1)].iter().copied().fold(0.0, f32::max)).collect()
}
