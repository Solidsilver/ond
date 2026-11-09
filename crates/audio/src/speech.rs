//! Optional speech processing on the decoder thread. Silence lookahead is
//! bounded at 410 ms; origins travel with buffered frames, one record per run
//! of frames, so a quiet run can cross file boundaries without assigning its
//! tail to the following file. Both processors allocate their channel/window
//! storage at construction, and a seek resets those buffers in place.

/// Quiet windows held back at a cut, as the lead-in to returning speech.
const LEAD_WINDOWS: usize = 3;

/// Below -45 dBFS RMS, across every channel, is quiet.
const QUIET_POWER: f64 = 0.000_031_622_776_601_683_79;

#[derive(Clone, Copy, Default)]
pub(crate) struct Origin {
    pub seq: u64,
    pub frame: u64,
    pub bits: Option<u32>,
}

/// A contiguous span of frames that share one origin. Buffered origins are
/// stored this way: a 410 ms window holds tens of thousands of frames but only
/// a few files, so the lookahead costs bytes instead of half a megabyte.
#[derive(Clone, Copy)]
struct Run {
    origin: Origin,
    frames: usize,
}

pub(crate) struct Silence {
    channels: usize,
    rate: u32,
    window: usize,
    samples: Vec<f32>,
    runs: Vec<Run>,
    /// The quiet windows just before returning speech, kept out of the cut.
    lead: Vec<f32>,
    lead_runs: Vec<Run>,
    partial: usize,
    quiet: usize,
    cutting: bool,
    saved: f64,
}

impl Silence {
    pub fn new(rate: u32, channels: u16, saved: f64) -> Self {
        let window = (rate as usize / 100).max(1);
        let channels = channels as usize;
        Self {
            channels,
            rate,
            window,
            samples: Vec::with_capacity(window * 41 * channels),
            runs: Vec::with_capacity(48),
            // One more window than it holds: a window is appended before the
            // oldest ages out, and that must not grow the buffer mid-playback.
            lead: Vec::with_capacity(window * (LEAD_WINDOWS + 1) * channels),
            lead_runs: Vec::with_capacity(LEAD_WINDOWS + 1),
            partial: 0,
            quiet: 0,
            cutting: false,
            saved,
        }
    }

    /// Clears detection state for a seek or an internal restart, keeping the
    /// buffers when the format hasn't changed: the lookahead is reserved once.
    pub fn reset(&mut self, rate: u32, channels: u16, saved: f64) {
        if rate != self.rate || channels as usize != self.channels {
            *self = Self::new(rate, channels, saved);
            return;
        }
        self.samples.clear();
        self.runs.clear();
        self.lead.clear();
        self.lead_runs.clear();
        self.partial = 0;
        self.quiet = 0;
        self.cutting = false;
        self.saved = saved;
    }

    pub fn process(&mut self, samples: &[f32], mut origin: Origin, mut emit: impl FnMut(&[f32], Origin, f64)) {
        for frame in samples.chunks_exact(self.channels) {
            self.samples.extend_from_slice(frame);
            push_run(&mut self.runs, Run { origin, frames: 1 });
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
        // Quiet windows that never led to speech are removed time.
        let held = self.lead.len() / self.channels;
        if held != 0 {
            self.emit_lead(0, held, true, &mut emit);
            self.lead.clear();
            self.lead_runs.clear();
        }
        let total = self.samples.len() / self.channels;
        self.emit(0, total, false, &mut emit);
        self.samples.clear();
        self.runs.clear();
    }

    fn measure(&mut self, emit: &mut impl FnMut(&[f32], Origin, f64)) {
        let count = self.partial;
        self.partial = 0;
        let start = self.samples.len() - count * self.channels;
        let power = self.samples[start..].iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / (count * self.channels) as f64;
        if power < QUIET_POWER {
            self.quiet += count;
            if self.cutting {
                self.hold_lead(start, emit);
                return;
            }
            if self.quiet <= self.window * 40 {
                return;
            }
            let keep = self.window * 20;
            let fade = (self.rate as usize / 200).max(1).min(keep);
            for (i, frame) in self.samples[(keep - fade) * self.channels..keep * self.channels].chunks_exact_mut(self.channels).enumerate() {
                let gain = (fade - 1 - i) as f32 / fade as f32;
                for sample in frame {
                    *sample *= gain;
                }
            }
            let total = self.samples.len() / self.channels;
            self.emit(0, keep, false, emit);
            self.emit(keep, total, true, emit);
            self.samples.clear();
            self.runs.clear();
            self.cutting = true;
        } else {
            // The lead-in carries the fade; the words themselves play as they are.
            self.release_lead(emit);
            let total = self.samples.len() / self.channels;
            self.emit(0, total, false, emit);
            self.samples.clear();
            self.runs.clear();
            self.quiet = 0;
            self.cutting = false;
        }
    }

    /// Holds this window of quiet as the lead-in to returning speech, letting
    /// the oldest held window go (as removed time) once the ring is full.
    fn hold_lead(&mut self, start: usize, emit: &mut impl FnMut(&[f32], Origin, f64)) {
        let total = self.samples.len() / self.channels;
        self.lead.extend_from_slice(&self.samples[start * self.channels..]);
        append_runs(&mut self.lead_runs, &self.runs, start, total);
        self.samples.clear();
        self.runs.clear();
        let held = self.lead.len() / self.channels;
        if held > self.window * LEAD_WINDOWS {
            let drop = held - self.window * LEAD_WINDOWS;
            self.emit_lead(0, drop, true, emit);
            self.lead.drain(..drop * self.channels);
            drop_runs(&mut self.lead_runs, drop);
        }
    }

    /// Plays the held quiet windows into the returning speech, ramped up from
    /// silence over their own length so the cut doesn't land on a consonant.
    fn release_lead(&mut self, emit: &mut impl FnMut(&[f32], Origin, f64)) {
        let frames = self.lead.len() / self.channels;
        if frames == 0 {
            return;
        }
        for (i, frame) in self.lead.chunks_exact_mut(self.channels).enumerate() {
            let gain = i as f32 / frames as f32;
            for sample in frame {
                *sample *= gain;
            }
        }
        self.emit_lead(0, frames, false, emit);
        self.lead.clear();
        self.lead_runs.clear();
    }

    fn emit(&mut self, start: usize, end: usize, removed: bool, emit: &mut impl FnMut(&[f32], Origin, f64)) {
        emit_span(&self.samples, &self.runs, self.channels, self.rate, start, end, removed, &mut self.saved, emit);
    }

    fn emit_lead(&mut self, start: usize, end: usize, removed: bool, emit: &mut impl FnMut(&[f32], Origin, f64)) {
        emit_span(&self.lead, &self.lead_runs, self.channels, self.rate, start, end, removed, &mut self.saved, emit);
    }
}

/// Publishes frames `[start, end)` of `samples`/`runs` as kept audio, or as a
/// source-time jump when they were removed. A piece never spans two files: its
/// origin is its own first frame.
#[allow(clippy::too_many_arguments)]
fn emit_span(
    samples: &[f32],
    runs: &[Run],
    channels: usize,
    rate: u32,
    start: usize,
    end: usize,
    removed: bool,
    saved: &mut f64,
    emit: &mut impl FnMut(&[f32], Origin, f64),
) {
    let mut at = 0;
    for run in runs {
        let run_end = at + run.frames;
        let (lo, hi) = (start.max(at), end.min(run_end));
        if lo < hi {
            let mut origin = run.origin;
            if removed {
                *saved += (hi - lo) as f64 / f64::from(rate.max(1));
                // One past the last removed frame: the next kept frame lands here.
                origin.frame += (hi - at) as u64;
                emit(&[], origin, *saved);
            } else {
                origin.frame += (lo - at) as u64;
                emit(&samples[lo * channels..hi * channels], origin, *saved);
            }
        }
        at = run_end;
    }
}

/// Appends the runs covering frames `[from, to)` of `source` to `dest`.
fn append_runs(dest: &mut Vec<Run>, source: &[Run], from: usize, to: usize) {
    let mut at = 0;
    for run in source {
        let end = at + run.frames;
        let (lo, hi) = (from.max(at), to.min(end));
        if lo < hi {
            let mut origin = run.origin;
            origin.frame += (lo - at) as u64;
            push_run(dest, Run { origin, frames: hi - lo });
        }
        at = end;
    }
}

/// Appends one run, folding it into the last when they're contiguous.
fn push_run(runs: &mut Vec<Run>, run: Run) {
    if let Some(last) = runs.last_mut()
        && last.origin.seq == run.origin.seq
        && last.origin.bits == run.origin.bits
        && last.origin.frame + last.frames as u64 == run.origin.frame
    {
        last.frames += run.frames;
        return;
    }
    runs.push(run);
}

/// Drops `frames` frames from the front of `runs`, splitting the run they fall inside.
fn drop_runs(runs: &mut Vec<Run>, mut frames: usize) {
    while frames != 0 {
        let Some(first) = runs.first_mut() else { return };
        if first.frames <= frames {
            frames -= first.frames;
            runs.remove(0);
        } else {
            first.origin.frame += frames as u64;
            first.frames -= frames;
            frames = 0;
        }
    }
}

/// A linked peak compressor keeps the stereo image steady. Its 20 ms attack
/// leaves consonants intact; the final instantaneous limiter catches those
/// transients without relying on the compressor having settled first.
pub(crate) struct Voice {
    rate: u32,
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

/// Denormal filter and envelope state is flushed to zero: it's inaudible, and
/// on some CPUs subnormal arithmetic is hundreds of times slower.
const DENORMAL: f32 = 1e-25;

/// A finite filter value, with denormals and any non-finite result cleared.
fn clean(value: f32) -> f32 {
    if value.is_finite() && value.abs() > DENORMAL { value } else { 0.0 }
}

impl Voice {
    pub fn new(rate: u32, channels: u16) -> Self {
        let frequency = 3000.0f32.min(rate as f32 * 0.2);
        let w = std::f32::consts::TAU * frequency / rate as f32;
        let alpha = w.sin() / (2.0 * 0.8);
        let a = 10f32.powf(3.0 / 40.0);
        let norm = 1.0 + alpha / a;
        Self {
            rate,
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

    /// Clears the filter and envelope state before a seek plays on, keeping the
    /// filters when the format hasn't changed; a different one needs fresh
    /// coefficients.
    pub fn reset(&mut self, rate: u32, channels: u16) {
        if self.rate != rate || self.filters.len() != channels as usize {
            *self = Self::new(rate, channels);
            return;
        }
        self.filters.iter_mut().for_each(|state| *state = [0.0; 2]);
        self.envelope = 0.0;
        self.limiter = 1.0;
        self.wet = 0.0;
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        for frame in samples.chunks_exact_mut(self.filters.len()) {
            let peak = frame.iter().fold(0.0f32, |peak, sample| if sample.is_finite() { peak.max(sample.abs()) } else { peak });
            let coefficient = if peak > self.envelope { self.attack } else { self.release };
            self.envelope = clean(coefficient * self.envelope + (1.0 - coefficient) * peak);
            // -18 dBFS threshold, 2:1 ratio, +3 dB makeup.
            let gain = if self.envelope > 0.125_892_54 { (0.125_892_54 / self.envelope).sqrt() } else { 1.0 } * 1.412_537_6;
            self.wet = (self.wet + self.step).min(1.0);
            let mut output_peak = 0.0f32;
            for (sample, state) in frame.iter_mut().zip(&mut self.filters) {
                // A bad decode must not leave a NaN in the filter state: one
                // such sample would otherwise silence the rest of the session.
                let dry = if sample.is_finite() { *sample } else { 0.0 };
                let filtered = clean(self.b[0] * dry + state[0]);
                state[0] = clean(self.b[1] * dry - self.a[0] * filtered + state[1]);
                state[1] = clean(self.b[2] * dry - self.a[1] * filtered);
                *sample = clean(dry + (filtered * gain - dry) * self.wet);
                output_peak = output_peak.max(sample.abs());
            }
            let ceiling = if output_peak > 1.0 { output_peak.recip() } else { 1.0 };
            self.limiter = clean(ceiling.min(self.limiter_release * self.limiter + (1.0 - self.limiter_release)));
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
    fn a_long_pause_keeps_two_hundred_ms_a_lead_in_and_exact_origins() {
        let mut silence = Silence::new(1000, 1, 0.0);
        let mut output = Vec::new();
        let mut origins = Vec::new();
        let signal: Vec<_> = [vec![0.2; 100], vec![0.001; 1000], vec![0.2; 100]].concat();
        silence.process(&signal, Origin::default(), |samples, origin, saved| {
            origins.push((output.len(), origin.frame, saved));
            output.extend_from_slice(samples);
        });
        silence.finish(|samples, _, _| output.extend_from_slice(samples));
        // 100 ms of speech, 200 ms of quiet, 30 ms of lead-in, 100 ms of speech.
        assert_eq!(output.len(), 430);
        assert_eq!(output[299], 0.0, "the kept run fades out over its last 5 ms");
        assert_eq!(output[300], 0.0, "the lead-in ramps up from silence");
        assert!((output[329] - 0.001 * 29.0 / 30.0).abs() < 1e-9);
        assert_eq!(output[330], 0.2, "the words themselves are not faded");
        // The last 30 ms of quiet now play, so 770 ms of the pause was removed.
        assert!(origins.iter().any(|(out, source, saved)| *out == 330 && *source == 1100 && (*saved - 0.77).abs() < 1e-12));
        assert!(origins.iter().any(|(out, source, _)| *out == 300 && *source == 1070), "the lead-in keeps its own source frames: {origins:?}");
    }

    #[test]
    fn a_short_pause_is_preserved_bit_for_bit_and_storage_never_grows() {
        let mut silence = Silence::new(48_000, 2, 0.0);
        let input = vec![0.001; 48_000 * 2 / 5];
        let capacities = (silence.samples.capacity(), silence.lead.capacity());
        let mut output = Vec::new();
        for piece in input.chunks(714) {
            silence.process(piece, Origin::default(), |samples, _, _| output.extend_from_slice(samples));
        }
        silence.finish(|samples, _, _| output.extend_from_slice(samples));
        assert_eq!(output, input);
        assert_eq!((silence.samples.capacity(), silence.lead.capacity()), capacities);
    }

    #[test]
    fn returning_speech_keeps_the_quiet_windows_before_it_and_cuts_without_a_click() {
        let rate = 48_000u32;
        let window = (rate / 100) as usize;
        let pause = rate as usize;
        let speech_at = window * 5 + pause;
        // Speech, a full second of room tone below the threshold, then a soft
        // onset: a sine from phase zero, so the first samples rise gently.
        let mut signal = vec![0.3f32; window * 5];
        signal.extend(std::iter::repeat_n(0.002f32, pause));
        signal.extend((0..rate as usize / 5).map(|i| (std::f32::consts::TAU * 220.0 * i as f32 / rate as f32).sin() * 0.06));
        let mut silence = Silence::new(rate, 1, 0.0);
        let mut output = Vec::new();
        let mut origins = Vec::new();
        silence.process(&signal, Origin::default(), |samples, origin, _| {
            origins.push((output.len(), origin.frame));
            output.extend_from_slice(samples);
        });
        silence.finish(|samples, _, _| output.extend_from_slice(samples));

        let at = origins.iter().find(|(_, frame)| *frame == speech_at as u64).map(|(out, _)| *out).expect("the speech played");
        let last_lead = output[at - 1];
        assert!((0.0019..=0.002).contains(&last_lead), "the quiet window before the speech survived: {last_lead}");
        let step = output[at - 3 * window..at + window].windows(2).map(|pair| (pair[1] - pair[0]).abs()).fold(0.0f32, f32::max);
        assert!(step < 0.01, "a click of {step} at the cut");
        assert_eq!(output[at..at + rate as usize / 5], signal[speech_at..], "the returning speech is not faded");
    }

    #[test]
    fn lookahead_origins_are_stored_per_run_and_a_seek_resets_in_place() {
        let mut silence = Silence::new(48_000, 2, 0.0);
        let capacities = (silence.samples.capacity(), silence.lead.capacity(), silence.runs.capacity());
        let window = vec![0.001; 48_000 / 10 * 2];
        for seq in 0..4 {
            silence.process(&window, Origin { seq, ..Default::default() }, |_, _, _| {});
        }
        assert_eq!(silence.runs.len(), 4, "one origin record per file, not one per frame");
        silence.boundary(|_, _, _| {});
        silence.reset(48_000, 2, 1.5);
        assert!(silence.runs.is_empty() && silence.lead_runs.is_empty());
        assert_eq!((silence.samples.capacity(), silence.lead.capacity(), silence.runs.capacity()), capacities);
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
    fn a_nan_or_an_infinity_cannot_poison_the_voice_filters() {
        let mut boost = Voice::new(48_000, 2);
        let mut signal: Vec<f32> = (0..48_000 * 2).map(|i| (std::f32::consts::TAU * 3000.0 * (i / 2) as f32 / 48_000.0).sin() * 0.1).collect();
        signal[100] = f32::NAN;
        signal[101] = f32::INFINITY;
        for piece in signal.chunks_mut(512) {
            boost.process(piece);
        }
        assert!(signal.iter().all(|s| s.is_finite()));
        // A poisoned filter would have gone silent: the presence boost is still there.
        let rms = (signal[24_000..].iter().map(|s| s * s).sum::<f32>() / 24_000.0).sqrt();
        assert!(rms > 0.05, "presence after the NaN: {rms}");
    }

    #[test]
    fn denormal_filter_and_envelope_state_is_flushed_to_zero() {
        let mut boost = Voice::new(48_000, 1);
        let mut signal = vec![1e-30f32; 48_000];
        for piece in signal.chunks_mut(512) {
            boost.process(piece);
        }
        assert_eq!(boost.envelope, 0.0);
        assert_eq!(boost.limiter, 1.0);
        assert!(boost.filters.iter().flatten().all(|state| *state == 0.0), "{:?}", boost.filters);
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
