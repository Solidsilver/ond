//! Streaming sample-rate conversion on the decode thread (never in the callback).
//!
//! Used when the device can't run at the file's rate: in Normal mode, and in
//! bit-perfect mode as the fallback (reported as `Unavailable`). One resampler
//! carries across gapless tracks of the same source rate, so the boundary is as
//! seamless as within a track. Its startup delay is trimmed, so output frame `n`
//! lines up with input time `n / out_rate`; that's what places track markers.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Async, FixedAsync, Indexing, Resampler, SincInterpolationParameters, SincInterpolationType, WindowFunction};

/// Input frames per resampler call.
const CHUNK: usize = 1024;

/// High-quality settings: a 256-tap Blackman-Harris² windowed sinc with an
/// automatic cutoff, 256× oversampled table, cubic interpolation between
/// table points. Passband ripple and alias rejection are measured in the tests.
fn params() -> SincInterpolationParameters {
    SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: None,
        oversampling_factor: 256,
        interpolation: SincInterpolationType::Cubic,
        window: WindowFunction::BlackmanHarris2,
    }
}

pub(crate) struct StreamResampler {
    inner: Async<f32>,
    channels: usize,
    from: u32,
    to: u32,
    /// Interleaved input not yet consumed.
    pending: Vec<f32>,
    /// Interleaved output of one call.
    scratch: Vec<f32>,
    /// Output frames still to drop to cancel the filter delay.
    delay_left: usize,
    /// Since the last reset.
    in_frames: u64,
    out_frames: u64,
}

impl StreamResampler {
    pub fn new(from: u32, to: u32, channels: usize) -> Result<Self, String> {
        let ratio = to as f64 / from as f64;
        let inner = Async::<f32>::new_sinc(ratio, 1.0, &params(), CHUNK, channels, FixedAsync::Input)
            .map_err(|e| format!("resampler {from} → {to}: {e}"))?;
        let scratch = vec![0.0; inner.output_frames_max() * channels];
        let delay_left = inner.output_delay();
        Ok(Self {
            inner,
            channels,
            from,
            to,
            pending: Vec::with_capacity(CHUNK * 2 * channels),
            scratch,
            delay_left,
            in_frames: 0,
            out_frames: 0,
        })
    }

    /// Input frames fed since the last reset.
    pub fn in_frames(&self) -> u64 {
        self.in_frames
    }

    /// The output position (frames since the last reset) of input frame `in_frames`.
    pub fn out_pos(&self, in_frames: u64) -> u64 {
        ((in_frames as u128 * self.to as u128 + self.from as u128 / 2) / self.from as u128) as u64
    }

    /// Resamples interleaved `input`, appending whatever output is ready to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.pending.extend_from_slice(input);
        self.in_frames += (input.len() / self.channels) as u64;
        loop {
            let need = self.inner.input_frames_next();
            if self.pending.len() < need * self.channels {
                break;
            }
            self.run(need, None, out);
            self.pending.drain(..need * self.channels);
        }
    }

    /// Emits the tail (everything still inside the filter) so the output length
    /// matches the input exactly, then resets. Call at the end of a stream.
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        let target = self.out_pos(self.in_frames);
        let start = out.len();
        let rest = self.pending.len() / self.channels;
        if rest > 0 {
            self.run(rest, Some(rest), out);
        }
        // Feed silence until the delayed tail is out.
        let mut guard = 0;
        while self.out_frames < target && guard < 64 {
            self.run(0, Some(0), out);
            guard += 1;
        }
        let extra = (self.out_frames.saturating_sub(target) as usize * self.channels).min(out.len() - start);
        out.truncate(out.len() - extra);
        self.reset();
    }

    /// Forgets all state (after a seek).
    pub fn reset(&mut self) {
        self.inner.reset();
        self.pending.clear();
        self.delay_left = self.inner.output_delay();
        self.in_frames = 0;
        self.out_frames = 0;
    }

    fn run(&mut self, frames: usize, partial: Option<usize>, out: &mut Vec<f32>) {
        let ch = self.channels;
        let input = InterleavedSlice::new(&self.pending[..frames * ch], ch, frames).expect("input adapter");
        let cap = self.scratch.len() / ch;
        let mut output = InterleavedSlice::new_mut(&mut self.scratch, ch, cap).expect("output adapter");
        let indexing = partial.map(|p| Indexing::new().partial_len(p));
        let produced = match self.inner.process_into_buffer(&input, &mut output, indexing.as_ref()) {
            Ok((_, produced)) => produced,
            Err(e) => {
                // Buffer sizes are fixed by construction; this would be a bug.
                tracing::error!("resampler: {e}");
                return;
            }
        };
        let skip = self.delay_left.min(produced);
        self.delay_left -= skip;
        out.extend_from_slice(&self.scratch[skip * ch..produced * ch]);
        self.out_frames += (produced - skip) as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, freq: f64, frames: usize, amp: f64) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v = (amp * (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin()) as f32;
                [v, v]
            })
            .collect()
    }

    /// Resamples `input` in uneven pieces, like the decoder does.
    fn resample(from: u32, to: u32, input: &[f32]) -> Vec<f32> {
        let mut r = StreamResampler::new(from, to, 2).unwrap();
        let mut out = Vec::new();
        for piece in input.chunks(2 * 1153) {
            r.process(piece, &mut out);
        }
        r.flush(&mut out);
        out
    }

    /// Amplitude of `freq` in the left channel (single-bin DFT over a Hann window).
    fn level(samples: &[f32], rate: u32, freq: f64) -> f64 {
        let left: Vec<f64> = samples.iter().step_by(2).map(|s| *s as f64).collect();
        let n = left.len();
        let (mut re, mut im, mut wsum) = (0.0, 0.0, 0.0);
        for (i, x) in left.iter().enumerate() {
            let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos();
            let ph = 2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64;
            re += x * w * ph.cos();
            im += x * w * ph.sin();
            wsum += w;
        }
        2.0 * (re * re + im * im).sqrt() / wsum
    }

    fn db(x: f64) -> f64 {
        20.0 * x.max(1e-12).log10()
    }

    #[test]
    fn output_length_matches_the_ratio_exactly() {
        for (from, to, frames) in [(96_000, 48_000, 96_001), (44_100, 48_000, 44_100), (176_400, 88_200, 10_007)] {
            let out = resample(from, to, &sine(from, 1000.0, frames, 0.5));
            let expected = (frames as u64 * to as u64 + from as u64 / 2) / from as u64;
            assert_eq!(out.len() as u64 / 2, expected, "{from} → {to}");
        }
    }

    #[test]
    fn downsampling_96k_to_48k_keeps_the_tone_and_rejects_aliases() {
        // Measured on 2 s of signal, skipping the edges.
        let (from, to, n) = (96_000, 48_000, 192_000);
        let cut = |s: &[f32]| s[2 * 4_800..s.len() - 2 * 4_800].to_vec();

        // Passband: tones from 100 Hz to 20 kHz come through within ±0.1 dB.
        let mut worst = 0.0f64;
        for f in [100.0, 1_000.0, 5_000.0, 10_000.0, 15_000.0, 20_000.0] {
            let out = cut(&resample(from, to, &sine(from, f, n, 0.5)));
            let gain = db(level(&out, to, f) / 0.5);
            worst = worst.max(gain.abs());
        }
        eprintln!("passband ripple 100 Hz–20 kHz: ±{worst:.4} dB");
        assert!(worst < 0.1, "passband ripple {worst:.3} dB");

        // Frequency preserved, and nothing else in the output: SNR of a 1 kHz tone.
        let out = cut(&resample(from, to, &sine(from, 1_000.0, n, 0.5)));
        let left: Vec<f64> = out.iter().step_by(2).map(|s| *s as f64).collect();
        let a = level(&out, to, 1_000.0);
        let phase_fit = {
            // Residual after removing the best-fitting 1 kHz sinusoid.
            let (mut c, mut s) = (0.0, 0.0);
            for (i, x) in left.iter().enumerate() {
                let ph = 2.0 * std::f64::consts::PI * 1_000.0 * i as f64 / to as f64;
                c += x * ph.cos();
                s += x * ph.sin();
            }
            let k = 2.0 / left.len() as f64;
            let (c, s) = (c * k, s * k);
            left.iter()
                .enumerate()
                .map(|(i, x)| {
                    let ph = 2.0 * std::f64::consts::PI * 1_000.0 * i as f64 / to as f64;
                    (x - c * ph.cos() - s * ph.sin()).powi(2)
                })
                .sum::<f64>()
                / left.len() as f64
        };
        let snr = db((a * a / 2.0).sqrt() / phase_fit.sqrt());
        assert!((db(a / 0.5)).abs() < 0.05, "1 kHz level");
        eprintln!("1 kHz SNR: {snr:.1} dB");
        assert!(snr > 100.0, "SNR {snr:.1} dB");

        // Stopband: a 30 kHz tone (above the new 24 kHz Nyquist) would fold to 18 kHz.
        let out = cut(&resample(from, to, &sine(from, 30_000.0, n, 0.5)));
        let alias = db(level(&out, to, 18_000.0) / 0.5);
        eprintln!("30 kHz → 18 kHz alias: {alias:.1} dB");
        assert!(alias < -100.0, "alias at 18 kHz: {alias:.1} dB");
    }

    #[test]
    fn continuing_across_a_boundary_equals_one_stream() {
        // Two pieces fed back to back give exactly the output of the joined signal:
        // what makes gapless work through the resampler.
        let a = sine(96_000, 440.0, 30_011, 0.3);
        let b = sine(96_000, 660.0, 20_003, 0.3);
        let joined: Vec<f32> = a.iter().chain(&b).copied().collect();
        let whole = resample(96_000, 48_000, &joined);
        let mut r = StreamResampler::new(96_000, 48_000, 2).unwrap();
        let mut out = Vec::new();
        r.process(&a, &mut out);
        let boundary = r.out_pos(r.in_frames());
        r.process(&b, &mut out);
        r.flush(&mut out);
        assert_eq!(out.iter().map(|s| s.to_bits()).collect::<Vec<_>>(), whole.iter().map(|s| s.to_bits()).collect::<Vec<_>>());
        assert_eq!(boundary, 15_006, "B starts at round(30011 / 2)");
    }
}
