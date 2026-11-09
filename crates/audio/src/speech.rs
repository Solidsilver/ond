//! Optional speech processing on the decoder thread. Silence lookahead is
//! bounded at 410 ms; origins travel with buffered frames so a quiet run can
//! cross file boundaries without assigning its tail to the following file.
//! Both processors allocate their channel/window storage only at construction.

#[derive(Clone, Copy, Default)]
pub(crate) struct Origin {
    pub seq: u64,
    pub frame: u64,
    pub bits: Option<u32>,
}

pub(crate) struct Silence {
    channels: usize,
    rate: u32,
    window: usize,
    samples: Vec<f32>,
    origins: Vec<Origin>,
    partial: usize,
    quiet: usize,
    cutting: bool,
    fade_in: usize,
    saved: f64,
}

impl Silence {
    pub fn new(rate: u32, channels: u16, saved: f64) -> Self {
        let window = (rate as usize / 100).max(1);
        Self {
            channels: channels as usize,
            rate,
            window,
            samples: Vec::with_capacity(window * 41 * channels as usize),
            origins: Vec::with_capacity(window * 41),
            partial: 0,
            quiet: 0,
            cutting: false,
            fade_in: 0,
            saved,
        }
    }

    pub fn has_pending(&self) -> bool {
        !self.origins.is_empty()
    }

    pub fn process(&mut self, samples: &[f32], mut origin: Origin, mut emit: impl FnMut(&[f32], Origin, f64)) {
        for frame in samples.chunks_exact(self.channels) {
            self.samples.extend_from_slice(frame);
            self.origins.push(origin);
            origin.frame += 1;
            self.partial += 1;
            if self.partial == self.window {
                self.measure(&mut emit);
            }
        }
    }

    /// A partial window belongs to the old file; pending silence can stay until
    /// the next file establishes whether this is a short pause or a long one.
    pub fn boundary(&mut self, mut emit: impl FnMut(&[f32], Origin, f64)) {
        if self.partial != 0 {
            self.measure(&mut emit);
        }
    }

    pub fn finish(&mut self, mut emit: impl FnMut(&[f32], Origin, f64)) {
        self.boundary(&mut emit);
        self.emit(0, self.origins.len(), false, &mut emit);
        self.samples.clear();
        self.origins.clear();
    }

    fn measure(&mut self, emit: &mut impl FnMut(&[f32], Origin, f64)) {
        let count = self.partial;
        self.partial = 0;
        let start = self.samples.len() - count * self.channels;
        let power = self.samples[start..].iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / (count * self.channels) as f64;
        // -45 dBFS RMS, with every channel contributing to the decision.
        if power < 0.000_031_622_776_601_683_79 {
            self.quiet += count;
            if self.cutting || self.quiet > self.window * 40 {
                if !self.cutting {
                    let keep = self.window * 20;
                    let fade = (self.rate as usize / 200).max(1).min(keep);
                    for (i, frame) in self.samples[(keep - fade) * self.channels..keep * self.channels].chunks_exact_mut(self.channels).enumerate() {
                        let gain = (fade - 1 - i) as f32 / fade as f32;
                        for sample in frame {
                            *sample *= gain;
                        }
                    }
                    self.emit(0, keep, false, emit);
                    self.emit(keep, self.origins.len(), true, emit);
                } else {
                    self.emit(0, self.origins.len(), true, emit);
                }
                self.samples.clear();
                self.origins.clear();
                self.cutting = true;
                self.fade_in = (self.rate as usize / 200).max(1);
            }
        } else {
            let fade = (self.rate as usize / 200).max(1);
            for frame in self.samples.chunks_exact_mut(self.channels) {
                if self.fade_in != 0 {
                    let gain = (fade - self.fade_in) as f32 / fade as f32;
                    for sample in frame {
                        *sample *= gain;
                    }
                    self.fade_in -= 1;
                }
            }
            self.emit(0, self.origins.len(), false, emit);
            self.samples.clear();
            self.origins.clear();
            self.quiet = 0;
            self.cutting = false;
        }
    }

    fn emit(&mut self, mut start: usize, end: usize, removed: bool, emit: &mut impl FnMut(&[f32], Origin, f64)) {
        while start < end {
            let mut stop = start + 1;
            while stop < end && self.origins[stop].seq == self.origins[start].seq {
                stop += 1;
            }
            let mut origin = self.origins[start];
            if removed {
                self.saved += (stop - start) as f64 / self.rate as f64;
                origin.frame = self.origins[stop - 1].frame + 1;
                emit(&[], origin, self.saved);
            } else {
                emit(&self.samples[start * self.channels..stop * self.channels], origin, self.saved);
            }
            start = stop;
        }
    }
}

/// A linked peak compressor keeps the stereo image steady. Its 20 ms attack
/// leaves consonants intact; the final instantaneous limiter catches those
/// transients without relying on the compressor having settled first.
pub(crate) struct Voice {
    filters: Vec<[f32; 2]>,
    b: [f32; 3],
    a: [f32; 2],
    envelope: f32,
    attack: f32,
    release: f32,
    limiter: f32,
    limiter_release: f32,
    wet: f32,
    step: f32,
}

impl Voice {
    pub fn new(rate: u32, channels: u16) -> Self {
        let frequency = 3000.0f32.min(rate as f32 * 0.2);
        let w = std::f32::consts::TAU * frequency / rate as f32;
        let alpha = w.sin() / (2.0 * 0.8);
        let a = 10f32.powf(3.0 / 40.0);
        let norm = 1.0 + alpha / a;
        Self {
            filters: vec![[0.0; 2]; channels as usize],
            b: [(1.0 + alpha * a) / norm, -2.0 * w.cos() / norm, (1.0 - alpha * a) / norm],
            a: [-2.0 * w.cos() / norm, (1.0 - alpha / a) / norm],
            envelope: 0.0,
            attack: (-1.0 / (rate as f32 * 0.020)).exp(),
            release: (-1.0 / (rate as f32 * 0.150)).exp(),
            limiter: 1.0,
            limiter_release: (-1.0 / (rate as f32 * 0.050)).exp(),
            wet: 0.0,
            step: 1.0 / (rate as f32 * 0.005),
        }
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        for frame in samples.chunks_exact_mut(self.filters.len()) {
            let peak = frame.iter().fold(0.0f32, |peak, sample| peak.max(sample.abs()));
            let coefficient = if peak > self.envelope { self.attack } else { self.release };
            self.envelope = coefficient * self.envelope + (1.0 - coefficient) * peak;
            // -18 dBFS threshold, 2:1 ratio, +3 dB makeup.
            let gain = if self.envelope > 0.125_892_54 { (0.125_892_54 / self.envelope).sqrt() } else { 1.0 } * 1.412_537_6;
            self.wet = (self.wet + self.step).min(1.0);
            let mut output_peak = 0.0f32;
            for (sample, state) in frame.iter_mut().zip(&mut self.filters) {
                let dry = *sample;
                let filtered = self.b[0] * dry + state[0];
                state[0] = self.b[1] * dry - self.a[0] * filtered + state[1];
                state[1] = self.b[2] * dry - self.a[1] * filtered;
                *sample = dry + (filtered * gain - dry) * self.wet;
                output_peak = output_peak.max(sample.abs());
            }
            let ceiling = if output_peak > 1.0 { output_peak.recip() } else { 1.0 };
            self.limiter = ceiling.min(self.limiter_release * self.limiter + (1.0 - self.limiter_release));
            for sample in frame {
                *sample = (*sample * self.limiter).clamp(-1.0, 1.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_pause_keeps_two_hundred_ms_with_fades_and_exact_origins() {
        let mut silence = Silence::new(1000, 1, 0.0);
        let mut output = Vec::new();
        let mut origins = Vec::new();
        let signal: Vec<_> = [vec![0.2; 100], vec![0.001; 1000], vec![0.2; 100]].concat();
        silence.process(&signal, Origin::default(), |samples, origin, saved| {
            origins.push((output.len(), origin.frame, saved));
            output.extend_from_slice(samples);
        });
        silence.finish(|samples, _, _| output.extend_from_slice(samples));
        assert_eq!(output.len(), 400);
        assert_eq!(output[299], 0.0);
        assert_eq!(output[300], 0.0);
        assert_eq!(output[305], 0.2);
        assert!(origins.iter().any(|(out, source, saved)| *out == 300 && *source == 1100 && (*saved - 0.8).abs() < 1e-12));
    }

    #[test]
    fn a_short_pause_is_preserved_bit_for_bit_and_storage_never_grows() {
        let mut silence = Silence::new(48_000, 2, 0.0);
        let input = vec![0.001; 48_000 * 2 / 5];
        let capacities = (silence.samples.capacity(), silence.origins.capacity());
        let mut output = Vec::new();
        for piece in input.chunks(714) {
            silence.process(piece, Origin::default(), |samples, _, _| output.extend_from_slice(samples));
        }
        silence.finish(|samples, _, _| output.extend_from_slice(samples));
        assert_eq!(output, input);
        assert_eq!((silence.samples.capacity(), silence.origins.capacity()), capacities);
    }

    #[test]
    fn a_quiet_run_spans_files_without_losing_their_origins() {
        let mut silence = Silence::new(1000, 1, 0.0);
        let mut kept = Vec::new();
        let mut saved = 0.0;
        let mut emit = |samples: &[f32], origin: Origin, total| {
            kept.extend(std::iter::repeat_n(origin.seq, samples.len()));
            saved = total;
        };
        silence.process(&[0.001; 250], Origin::default(), &mut emit);
        silence.boundary(&mut emit);
        silence.process(&[0.001; 750], Origin { seq: 1, ..Default::default() }, &mut emit);
        silence.finish(&mut emit);
        assert_eq!(kept, vec![0; 200]);
        assert!((saved - 0.8).abs() < 1e-12);
    }

    fn boosted_rms(frequency: f32, amplitude: f32) -> f32 {
        let mut boost = Voice::new(48_000, 1);
        let mut signal: Vec<_> = (0..48_000).map(|i| (std::f32::consts::TAU * frequency * i as f32 / 48_000.0).sin() * amplitude).collect();
        boost.process(&mut signal);
        (signal[24_000..].iter().map(|s| s * s).sum::<f32>() / 24_000.0).sqrt()
    }

    #[test]
    fn voice_boost_adds_presence_and_gently_reduces_level_differences() {
        let presence_db = 20.0 * (boosted_rms(3000.0, 0.01) / boosted_rms(200.0, 0.01)).log10();
        assert!((presence_db - 3.0).abs() < 0.1, "presence {presence_db} dB");
        let ratio = boosted_rms(200.0, 0.4) / boosted_rms(200.0, 0.2);
        assert!((ratio - 2.0f32.sqrt()).abs() < 0.02, "2:1 compression: {ratio}");
    }

    #[test]
    fn voice_boost_limits_full_scale_transients_on_every_channel() {
        for rate in [8000, 44_100, 48_000, 192_000] {
            let mut boost = Voice::new(rate, 2);
            let mut signal: Vec<_> = (0..rate).flat_map(|i| [(i as f32 * 0.37).sin() * 1.5, if i % 11 == 0 { -2.0 } else { 0.1 }]).collect();
            for piece in signal.chunks_mut(512) {
                boost.process(piece);
            }
            assert!(signal.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        }
    }
}
