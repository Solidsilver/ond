//! The music's feel, for the visualizer, worked out alongside the spectrum
//! (see `spectrum::Listener`): how strong the low, middle and high end are
//! right now, when a beat lands, and the latest stretch of waveform.
//!
//! Everything is measured against the track itself, not against full scale:
//! each part of the spectrum against its own recent peak, the waveform
//! against its recent loudest, and a beat as a jump above the quiet between
//! hits. A quiet recording moves the picture as much as a loud one.

use std::f32::consts::TAU;

/// Points in [`Feel::wave`].
pub const WAVE: usize = 512;
/// The stretch the waveform shows, in seconds (about a G-Force screenful).
const WAVE_SECS: f32 = 0.023;
/// Where the low, middle and high end split, in Hz.
const SPLITS: [f32; 4] = [20.0, 250.0, 4000.0, 16_000.0];
/// The dB range under a part's recent peak that reads 0..=1.
const RANGE_DB: f32 = 24.0;
/// How fast a recent peak is forgotten, in dB a second.
const PEAK_FALL_DB: f32 = 3.0;
/// Peaks never sit lower than this, so near-silence reads as nothing.
const PEAK_FLOOR_DB: f32 = -50.0;

/// One frame's feel. All 0..=1 unless said otherwise.
#[derive(Debug, Clone, PartialEq)]
pub struct Feel {
    /// The low, middle and high end, each against its own recent peak, smoothed.
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    /// The whole, the same way.
    pub level: f32,
    /// A beat landed this frame.
    pub onset: bool,
    /// 1 as a beat lands (less for a softer one), falling back over about a third of a second.
    pub beat: f32,
    /// Beats so far (to tell a new one from the same one seen twice).
    pub beats: u64,
    /// The latest stretch of waveform, oldest first, scaled so its recent
    /// loudest reaches ±1 (it can overshoot a little).
    pub wave: Vec<f32>,
}

impl Default for Feel {
    fn default() -> Self {
        Self { bass: 0.0, mid: 0.0, treble: 0.0, level: 0.0, onset: false, beat: 0.0, beats: 0, wave: vec![0.0; WAVE] }
    }
}

/// Works out a [`Feel`] each frame from the latest samples and their spectrum.
#[derive(Debug)]
pub struct Pulse {
    feel: Feel,
    /// Recent peaks, in dB: low, middle, high, whole.
    peaks: [f32; 4],
    beats: Beats,
    /// The waveform's recent loudest.
    wave_peak: f32,
}

impl Default for Pulse {
    fn default() -> Self {
        Self { feel: Feel::default(), peaks: [PEAK_FLOOR_DB; 4], beats: Beats::default(), wave_peak: 0.0 }
    }
}

impl Pulse {
    pub fn feel(&self) -> &Feel {
        &self.feel
    }

    /// Takes `dt` seconds' news: `samples`, the latest audio (oldest first,
    /// at least 4096 of them), and `mags`, its spectrum's magnitudes from 0 Hz
    /// to Nyquist at `rate`.
    pub fn hear(&mut self, samples: &[f32], mags: &[f32], rate: u32, dt: f32) {
        let hz_per_bin = rate as f32 / (2 * (mags.len().max(2) - 1)) as f32;
        let power = |lo: f32, hi: f32| -> f32 {
            let (a, b) = ((lo / hz_per_bin).ceil() as usize, ((hi / hz_per_bin).ceil() as usize).min(mags.len()));
            mags.get(a..b).map_or(0.0, |m| m.iter().map(|v| v * v).sum())
        };
        let parts = [power(SPLITS[0], SPLITS[1]), power(SPLITS[1], SPLITS[2]), power(SPLITS[2], SPLITS[3]), power(SPLITS[0], SPLITS[3])];
        let mut now = [0.0; 4];
        for ((p, peak), out) in parts.iter().zip(&mut self.peaks).zip(&mut now) {
            let db = 10.0 * (p + 1e-12).log10();
            *peak = (*peak - PEAK_FALL_DB * dt).max(db).max(PEAK_FLOOR_DB);
            *out = ((db - (*peak - RANGE_DB)) / RANGE_DB).clamp(0.0, 1.0);
        }
        let f = &mut self.feel;
        for (v, target) in [&mut f.bass, &mut f.mid, &mut f.treble, &mut f.level].into_iter().zip(now) {
            *v = ease(*v, target, if target > *v { 25.0 } else { 6.0 }, dt);
        }
        let hit = self.beats.hear(samples, rate, dt);
        self.feel.onset = hit.is_some();
        self.feel.beat = hit.unwrap_or(0.0).max(self.feel.beat - 3.0 * dt).max(0.0);
        self.feel.beats += u64::from(hit.is_some());
        self.wave(samples, rate, dt);
    }

    /// No news (paused or stalled): everything falls toward rest.
    pub fn rest(&mut self, dt: f32) {
        let f = &mut self.feel;
        for v in [&mut f.bass, &mut f.mid, &mut f.treble, &mut f.level] {
            *v = ease(*v, 0.0, 6.0, dt);
        }
        f.onset = false;
        f.beat = (f.beat - 3.0 * dt).max(0.0);
        for w in &mut f.wave {
            *w *= (-dt * 8.0).exp();
        }
    }

    /// The newest [`WAVE_SECS`], picked out at [`WAVE`] points.
    fn wave(&mut self, samples: &[f32], rate: u32, dt: f32) {
        let span = ((rate as f32 * WAVE_SECS) as usize).clamp(WAVE, samples.len());
        let recent = &samples[samples.len() - span..];
        let loudest = recent.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        // Up at once, down slowly; never so low that hiss fills the screen.
        self.wave_peak = (self.wave_peak * (-dt / 3.0).exp()).max(loudest).max(0.02);
        let scale = 1.0 / self.wave_peak;
        for (i, w) in self.feel.wave.iter_mut().enumerate() {
            *w = recent[i * span / WAVE] * scale;
        }
    }
}

/// Moves `v` toward `target` at `rate` (per second), for a frame of `dt`.
fn ease(v: f32, target: f32, rate: f32, dt: f32) -> f32 {
    v + (target - v) * (1.0 - (-rate * dt).exp())
}

/// Beats, from the low end: the energy below about 150 Hz over the latest
/// stretch, on a log scale (so it's the same at any loudness), against a floor
/// that sinks quickly into the quiet between hits and rises slowly. A beat is
/// a rise well above that floor. A steady sound, however loud or noisy, sits
/// on its floor and never makes one.
#[derive(Debug, Default)]
struct Beats {
    floor: f32,
    last: f32,
    /// Seconds since the last beat.
    since: f32,
    started: bool,
}

/// The stretch measured, in seconds.
const BEAT_WINDOW: f32 = 0.12;
/// Where the low end stops, in Hz.
const BEAT_HZ: f32 = 150.0;
/// The rise over the floor that makes a beat, in nepers of energy (about 4.3 dB a neper).
const BEAT_RISE: f32 = 0.8;
/// Beats closer together than this are one (a fast drummer's 16ths still read as 8ths).
const BEAT_GAP: f32 = 0.2;
/// Mean-square energy below this is silence.
const BEAT_GATE: f32 = 1e-8;
/// The least share of all the energy the low end must hold for its rises to
/// count: in music it holds most of it; in hiss, a sliver that wobbles.
const BEAT_SHARE: f32 = 0.03;

impl Beats {
    /// The beat's strength, if one lands now.
    fn hear(&mut self, samples: &[f32], rate: u32, dt: f32) -> Option<f32> {
        let (energy, total) = energies(samples, rate);
        let x = (energy + 1e-12).ln();
        self.since += dt;
        if !self.started {
            (self.started, self.floor, self.last) = (true, x, x);
            return None;
        }
        let rise = x - self.floor;
        let hit = energy > BEAT_GATE && energy > BEAT_SHARE * total && rise > BEAT_RISE && x > self.last && self.since > BEAT_GAP;
        let tau = if x < self.floor { 0.12 } else { 1.5 };
        self.floor += (x - self.floor) * (1.0 - (-dt / tau).exp());
        self.last = x;
        if hit {
            self.since = 0.0;
            Some((0.55 + (rise - BEAT_RISE) * 0.3).min(1.0))
        } else {
            None
        }
    }
}

/// Mean-square energy below [`BEAT_HZ`] over the newest [`BEAT_WINDOW`] of
/// `samples` (two one-pole low-passes, 12 dB an octave), and of everything.
fn energies(samples: &[f32], rate: u32) -> (f32, f32) {
    let n = ((rate as f32 * BEAT_WINDOW) as usize).clamp(64, samples.len());
    let a = 1.0 - (-TAU * BEAT_HZ / rate.max(1) as f32).exp();
    let (mut y1, mut y2, mut low, mut all) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for &x in &samples[samples.len() - n..] {
        y1 += a * (x - y1);
        y2 += a * (y1 - y2);
        low += y2 * y2;
        all += x * x;
    }
    (low / n as f32, all / n as f32)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const RATE: u32 = 44_100;

    /// A few seconds of music with a kick drum on every beat at 120 bpm: a
    /// sustained chord with its own steady bass note, some hiss, and the
    /// kick (a 50 Hz thump that dies away). `gain` scales the lot. The kicks
    /// land on whole seconds and halves.
    pub fn kick_track(secs: f32, gain: f32, kick: f32) -> Vec<f32> {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        (0..(secs * RATE as f32) as usize)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let since = (t * 2.0).fract() / 2.0;
                // The pitch drops from about 130 Hz to 50 as it sounds.
                let thump = kick * (-since * 15.0).exp() * (TAU * (50.0 * since + 3.2 * (1.0 - (-since * 25.0).exp()))).sin();
                let chord: f32 = [(55.0, 0.15), (110.0, 0.12), (220.0, 0.15), (330.0, 0.1), (440.0, 0.08), (1320.0, 0.03)].iter().map(|(hz, a)| a * (TAU * hz * t).sin()).sum();
                gain * (chord + thump + 0.02 * noise(&mut seed))
            })
            .collect()
    }

    /// White noise, -1..1.
    pub fn noise(seed: &mut u64) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        (*seed >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    }

    /// When beats were heard in `signal`, played at 60 frames a second.
    fn beats_in(signal: &[f32]) -> Vec<f32> {
        let mut b = Beats::default();
        let hop = RATE as usize / 60;
        let mut out = Vec::new();
        let mut end = 4096;
        while end <= signal.len() {
            if b.hear(&signal[end - 4096..end], RATE, 1.0 / 60.0).is_some() {
                out.push(end as f32 / RATE as f32);
            }
            end += hop;
        }
        out
    }

    #[test]
    fn kicks_are_heard_on_time_at_any_loudness() {
        for (gain, kick) in [(1.0, 0.8), (0.02, 0.8), (0.5, 0.5), (2.0, 0.5)] {
            let heard = beats_in(&kick_track(8.0, gain, kick));
            // Kicks from 0.5 s (the first window is still filling before then) to 7.5 s.
            let kicks: Vec<f32> = (1..16).map(|k| k as f32 * 0.5).collect();
            for k in &kicks {
                assert!(heard.iter().any(|h| (k..=&(k + 0.05)).contains(&h)), "gain {gain}, kick {kick}: missed the kick at {k} s; heard {heard:?}");
            }
            assert!(heard.len() <= kicks.len() + 1, "gain {gain}, kick {kick}: extra beats {heard:?}");
        }
    }

    #[test]
    fn steady_sound_has_no_beats() {
        let mut seed = 7u64;
        let hiss: Vec<f32> = (0..RATE as usize * 8).map(|_| 0.3 * noise(&mut seed)).collect();
        assert_eq!(beats_in(&hiss), Vec::<f32>::new(), "white noise");
        let chord = kick_track(8.0, 1.0, 0.0);
        assert_eq!(beats_in(&chord), Vec::<f32>::new(), "a held chord");
        let silence = vec![0.0; RATE as usize * 2];
        assert_eq!(beats_in(&silence), Vec::<f32>::new());
    }

    #[test]
    fn quiet_and_loud_tracks_feel_the_same() {
        let feel = |gain: f32| {
            let mut p = Pulse::default();
            let track = kick_track(3.0, gain, 0.8);
            let mut planner = realfft::RealFftPlanner::<f32>::new();
            let fft = planner.plan_fft_forward(4096);
            let (mut sum, mut n) = ([0.0f32; 4], 0);
            for end in (4096..track.len()).step_by(735) {
                let mut input = track[end - 4096..end].to_vec();
                let mut out = fft.make_output_vec();
                fft.process(&mut input, &mut out).unwrap();
                let mags: Vec<f32> = out.iter().map(|c| c.norm() * 4.0 / 4096.0).collect();
                p.hear(&track[end - 4096..end], &mags, RATE, 1.0 / 60.0);
                let f = p.feel();
                for (s, v) in sum.iter_mut().zip([f.bass, f.mid, f.treble, f.level]) {
                    *s += v;
                }
                n += 1;
            }
            (sum.map(|s| s / n as f32), p.feel().wave.iter().fold(0.0f32, |m, w| m.max(w.abs())))
        };
        let (loud, loud_wave) = feel(1.0);
        let (quiet, quiet_wave) = feel(0.03);
        // (The highs here are only hiss, which in the quiet copy is near silence.)
        for i in [0, 1, 3] {
            assert!(loud[i] > 0.2 && (loud[i] - quiet[i]).abs() < 0.08, "loud {loud:?}, quiet {quiet:?}");
        }
        assert!(loud_wave > 0.3 && (loud_wave - quiet_wave).abs() < 0.1, "{loud_wave} {quiet_wave}");
    }
}
