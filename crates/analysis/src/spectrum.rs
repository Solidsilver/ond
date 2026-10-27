//! An averaged power spectrum of the loud parts of a track, and where its
//! content stops. Quiet windows are skipped so fades and silence don't drag
//! the average down to the noise floor.

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;

const N: usize = 8192;
/// Windows quieter than this (RMS, dBFS) are ignored.
const LOUD_DBFS: f64 = -50.0;
/// Fewer loud windows than this and the spectrum isn't trusted.
const MIN_WINDOWS: u32 = 16;
/// Content must sit this far above the noise floor to count.
const ABOVE_FLOOR_DB: f64 = 15.0;
/// Smoothing width across frequency.
const SMOOTH_HZ: f64 = 250.0;

/// Where the content ends, and how abruptly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edge {
    pub cutoff_hz: u32,
    /// Level just below the cutoff minus level just above it.
    pub drop_db: f64,
    /// The drop across the edge's middle 800 Hz: how steep it is.
    pub sharp_db: f64,
}

pub struct Spectrum {
    rate: u32,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    pending: Vec<f32>,
    input: Vec<f32>,
    output: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    power: Vec<f64>,
    windows: u32,
}

impl Spectrum {
    pub fn new(rate: u32) -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(N);
        let window = (0..N).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N as f32).cos()).collect();
        Self {
            rate,
            input: fft.make_input_vec(),
            output: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            power: vec![0.0; N / 2 + 1],
            fft,
            window,
            pending: Vec::with_capacity(N),
            windows: 0,
        }
    }

    /// Interleaved samples; channels are averaged to mono.
    pub fn push(&mut self, samples: &[f32], channels: usize) {
        for frame in samples.chunks_exact(channels) {
            self.pending.push(frame.iter().sum::<f32>() / channels as f32);
            if self.pending.len() == N {
                self.window_done();
            }
        }
    }

    fn window_done(&mut self) {
        let ms = self.pending.iter().map(|&s| f64::from(s) * f64::from(s)).sum::<f64>() / N as f64;
        if ms > 0.0 && 10.0 * ms.log10() >= LOUD_DBFS {
            for ((i, &s), &w) in self.input.iter_mut().zip(&self.pending).zip(&self.window) {
                *i = s * w;
            }
            if self.fft.process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch).is_ok() {
                for (p, c) in self.power.iter_mut().zip(&self.output) {
                    *p += f64::from(c.norm_sqr());
                }
                self.windows += 1;
            }
        }
        self.pending.clear();
    }

    /// The content's upper edge, when it ends clearly below Nyquist.
    pub fn finish(self) -> Option<Edge> {
        // Below 8 kHz there's no band worth judging (and the 1 kHz floor below
        // would sit above Nyquist).
        if self.windows < MIN_WINDOWS || self.rate < 8_000 {
            return None;
        }
        let bin_hz = f64::from(self.rate) / N as f64;
        let bin = |hz: f64| ((hz / bin_hz).round() as usize).min(self.power.len() - 1);
        let db: Vec<f64> = self.power.iter().map(|p| 10.0 * (p / f64::from(self.windows) + 1e-30).log10()).collect();
        let sm = smooth(&db, ((SMOOTH_HZ / bin_hz).ceil() as usize) | 1);

        let nyquist = f64::from(self.rate) / 2.0;
        let (lo, hi) = (bin(1000.0), bin(0.985 * nyquist));
        if lo >= hi {
            return None;
        }
        let floor = sm[bin(0.3 * nyquist).max(lo)..=hi].iter().copied().fold(f64::INFINITY, f64::min);
        let cut = (lo..=hi).rev().find(|&k| sm[k] > floor + ABOVE_FLOOR_DB)?;
        // Content all the way up: no edge to speak of.
        if cut + bin(0.015 * nyquist) >= hi {
            return None;
        }
        let cutoff = cut as f64 * bin_hz;
        let mean = |a: f64, b: f64| {
            let (a, b) = (bin(a.max(0.0)), bin(b.min(0.985 * nyquist)));
            (b > a).then(|| sm[a..=b].iter().sum::<f64>() / (b - a + 1) as f64)
        };
        let below = mean(cutoff - 1500.0, cutoff - 500.0)?;
        let above = mean(cutoff + 300.0, cutoff + 1300.0)?;
        // The same across a narrow span: only a filter's brickwall falls far
        // within it, a natural rolloff can't.
        let sharp = mean(cutoff - 600.0, cutoff - 200.0)? - mean(cutoff + 200.0, cutoff + 600.0)?;
        Some(Edge { cutoff_hz: ((cutoff / 10.0).round() * 10.0) as u32, drop_db: below - above, sharp_db: sharp })
    }
}

/// Centered moving average of width `w` (odd).
fn smooth(v: &[f64], w: usize) -> Vec<f64> {
    let half = w / 2;
    let mut prefix = vec![0.0; v.len() + 1];
    for (i, x) in v.iter().enumerate() {
        prefix[i + 1] = prefix[i] + x;
    }
    (0..v.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(half), (i + half + 1).min(v.len()));
            (prefix[b] - prefix[a]) / (b - a) as f64
        })
        .collect()
}
