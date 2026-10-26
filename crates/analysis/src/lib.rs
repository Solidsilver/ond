//! Offline signal analysis of whole tracks: EBU R128 loudness (integrated,
//! range, true peak), the DR meter's dynamic range, and fake hi-res detection
//! (padded bit depth, upsampled files, lossy spectral cutoffs).
//!
//! [`Analyzer`] takes decoded interleaved samples a block at a time and keeps
//! bounded state (per-3 s DR blocks, one averaged spectrum, ebur128's
//! histogram), so a track of any length costs the same memory. [`analyze`]
//! decodes a file with symphonia and feeds it through.
//!
//! Verdicts are deliberately cautious: each carries a confidence and the
//! evidence it rests on ("no content above 21.4 kHz"), and nothing is claimed
//! when there isn't enough signal to judge.

mod decode;
mod dr;
pub mod peaks;
mod spectrum;

#[cfg(test)]
mod tests;

pub use decode::{AnalyzeError, analyze, analyze_reader, peaks, peaks_reader};
pub use peaks::{PeakMeter, Peaks};


use ebur128::{EbuR128, Mode};
use serde::{Deserialize, Serialize};

/// Bumped when an algorithm changes, so stored results are redone.
/// v2: DR as dr14_t.meter computes it (trailing partial block, 44.1 kHz block
/// size); "upsampled" needs a sharp edge.
pub const ANALYSIS_VERSION: u16 = 2;

/// The least drop across a cutoff's middle 800 Hz (see `spectrum::Edge`) that
/// reads as a filter's brickwall: about 45 dB per kHz. Natural rolloffs are
/// far gentler; resampler and ADC filters far steeper.
const SHARP_EDGE_DB: f64 = 25.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VerdictKind {
    /// A 24-bit file whose low bits are always zero.
    PaddedBits { bits: u32, effective: u32 },
    /// A high-rate file with nothing above the band of a lower rate.
    Upsampled { from_hz: u32, cutoff_hz: u32 },
    /// A lossless file with the sharp lowpass that MP3/AAC encoders leave.
    LossySource { cutoff_hz: u32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    pub kind: VerdictKind,
    pub confidence: Confidence,
}

impl Verdict {
    /// A few words for a chip: "likely upsampled".
    pub fn label(&self) -> String {
        let hedge = if self.confidence == Confidence::High { "likely" } else { "possibly" };
        match &self.kind {
            VerdictKind::PaddedBits { effective, .. } => format!("effectively {effective}-bit"),
            VerdictKind::Upsampled { .. } => format!("{hedge} upsampled"),
            VerdictKind::LossySource { .. } => format!("{hedge} from a lossy source"),
        }
    }

    /// What the verdict rests on, in a sentence.
    pub fn evidence(&self) -> String {
        match &self.kind {
            VerdictKind::PaddedBits { bits, effective } => {
                format!("The low {} bits of every sample are zero. The file is {bits}-bit in name and {effective}-bit in content.", bits - effective)
            }
            VerdictKind::Upsampled { from_hz, cutoff_hz } => format!(
                "No content above {} kHz, where a native high-rate recording would have some. Likely upsampled from {} kHz.",
                khz(*cutoff_hz),
                khz(*from_hz)
            ),
            VerdictKind::LossySource { cutoff_hz } if *cutoff_hz >= 19_500 => format!(
                "Sharp cutoff at {} kHz, where MP3 and AAC encoders cut the treble. The file may come from a lossy copy, though some early digital recordings stop there too.",
                khz(*cutoff_hz)
            ),
            VerdictKind::LossySource { cutoff_hz } => format!(
                "Sharp cutoff at {} kHz, where MP3 and AAC encoders cut the treble. The file may come from a lossy copy.",
                khz(*cutoff_hz)
            ),
        }
    }

    /// Same kind of finding, ignoring the numbers (for grouping an album's tracks).
    pub fn same_kind(&self, other: &Verdict) -> bool {
        std::mem::discriminant(&self.kind) == std::mem::discriminant(&other.kind)
    }
}

fn khz(hz: u32) -> String {
    let k = f64::from(hz) / 1000.0;
    if (k - k.round()).abs() < 0.05 { format!("{}", k.round()) } else { format!("{k:.1}") }
}

/// Everything learned about one track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackAnalysis {
    pub version: u16,
    pub duration_secs: f64,
    pub sample_rate: u32,
    pub channels: u16,
    /// Bits per sample of a lossless source (None for lossy codecs).
    pub bits: Option<u32>,
    pub integrated_lufs: Option<f64>,
    pub loudness_range_lu: Option<f64>,
    pub true_peak_dbtp: Option<f64>,
    pub sample_peak_dbfs: Option<f64>,
    /// The DR meter value before rounding.
    pub dr: Option<f64>,
    /// Bits actually used, when fewer than `bits`.
    pub effective_bits: Option<u32>,
    /// Where the content stops, when it stops sharply below Nyquist.
    pub cutoff_hz: Option<u32>,
    pub verdicts: Vec<Verdict>,
}

impl TrackAnalysis {
    pub fn dr_rounded(&self) -> Option<u32> {
        self.dr.map(|d| d.round().max(0.0) as u32)
    }
}

/// A stored result: the analysis plus what the file looked like when analysed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub stamp: String,
    pub analysis: TrackAnalysis,
}

impl Record {
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_stdvec(self).unwrap_or_default()
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        postcard::from_bytes(bytes).ok()
    }
}

/// Identifies a file version: a changed file (or a newer algorithm) gets a new
/// stamp. The server gives no modification time, so a file replaced by one of
/// the same size, format and length (to the second) keeps its stamp.
pub fn stamp(size: Option<u64>, suffix: Option<&str>, bits: Option<u32>, rate: Option<u32>, duration_secs: Option<u32>) -> String {
    format!(
        "v{ANALYSIS_VERSION}:{}:{}:{}:{}:{}",
        size.unwrap_or(0),
        suffix.unwrap_or(""),
        bits.unwrap_or(0),
        rate.unwrap_or(0),
        duration_secs.unwrap_or(0)
    )
}

/// An album at a glance, from whichever of its tracks are analysed.
#[derive(Debug, Clone, PartialEq)]
pub struct AlbumSummary {
    pub analyzed: usize,
    pub total: usize,
    /// Mean of the tracks' DR, rounded (the DR meter's album value).
    pub dr: Option<u32>,
    /// Duration-weighted energy mean of the tracks' integrated loudness.
    pub lufs: Option<f64>,
    pub true_peak_dbtp: Option<f64>,
    /// The most common finding, when at least half the analysed tracks share it,
    /// with how many tracks had it.
    pub verdict: Option<(Verdict, usize)>,
}

impl AlbumSummary {
    pub fn complete(&self) -> bool {
        self.total > 0 && self.analyzed >= self.total
    }
}

pub fn summarize(tracks: &[&TrackAnalysis], total: usize) -> AlbumSummary {
    let drs: Vec<f64> = tracks.iter().filter_map(|t| t.dr).collect();
    let dr = (!drs.is_empty()).then(|| (drs.iter().sum::<f64>() / drs.len() as f64).round().max(0.0) as u32);

    let (mut energy, mut secs) = (0.0, 0.0);
    for t in tracks {
        if let Some(l) = t.integrated_lufs {
            energy += t.duration_secs * 10f64.powf(l / 10.0);
            secs += t.duration_secs;
        }
    }
    let lufs = (secs > 0.0 && energy > 0.0).then(|| 10.0 * (energy / secs).log10());
    let true_peak_dbtp = tracks.iter().filter_map(|t| t.true_peak_dbtp).reduce(f64::max);

    let mut groups: Vec<(Verdict, usize)> = Vec::new();
    for v in tracks.iter().flat_map(|t| &t.verdicts) {
        match groups.iter_mut().find(|(g, _)| g.same_kind(v)) {
            Some((g, n)) => {
                *n += 1;
                // Keep the most confident example for the album's label.
                if v.confidence > g.confidence {
                    *g = v.clone();
                }
            }
            None => groups.push((v.clone(), 1)),
        }
    }
    let verdict = groups.into_iter().filter(|(_, n)| n * 2 >= tracks.len().max(1)).max_by_key(|(_, n)| *n);

    AlbumSummary { analyzed: tracks.len(), total, dr, lufs, true_peak_dbtp, verdict }
}

/// Streaming analysis of one track's decoded audio.
pub struct Analyzer {
    rate: u32,
    channels: usize,
    bits: Option<u32>,
    frames: u64,
    ebu: Option<EbuR128>,
    dr: dr::DrMeter,
    spectrum: spectrum::Spectrum,
    /// OR of every sample's integer magnitude (lossless sources up to 24 bits).
    bits_used: u64,
    peak: f32,
}

impl Analyzer {
    /// `bits` is the source's bit depth for lossless codecs, `None` for lossy.
    pub fn new(rate: u32, channels: usize, bits: Option<u32>) -> Self {
        let channels = channels.max(1);
        let mode = Mode::I | Mode::LRA | Mode::TRUE_PEAK | Mode::HISTOGRAM;
        Self {
            rate,
            channels,
            bits,
            frames: 0,
            ebu: EbuR128::new(channels as u32, rate.max(1), mode).ok(),
            dr: dr::DrMeter::new(rate, channels),
            spectrum: spectrum::Spectrum::new(rate),
            bits_used: 0,
            peak: 0.0,
        }
    }

    /// Interleaved samples in [-1, 1]; a trailing partial frame is ignored.
    pub fn push(&mut self, samples: &[f32]) {
        let samples = &samples[..samples.len() - samples.len() % self.channels];
        if samples.is_empty() {
            return;
        }
        self.frames += (samples.len() / self.channels) as u64;
        if let Some(ebu) = &mut self.ebu {
            let _ = ebu.add_frames_f32(samples);
        }
        self.dr.push(samples);
        self.spectrum.push(samples, self.channels);
        for &s in samples {
            self.peak = self.peak.max(s.abs());
        }
        // f32 holds 24-bit integers exactly, so the original values come back.
        if let Some(bits) = self.bits.filter(|b| (9..=24).contains(b)) {
            let scale = (1u64 << (bits - 1)) as f32;
            for &s in samples {
                self.bits_used |= (s * scale).round().abs() as u64;
            }
        }
    }

    pub fn finish(self) -> TrackAnalysis {
        let finite = |v: f64| v.is_finite().then_some(v);
        let (integrated, range, true_peak) = match &self.ebu {
            Some(e) => (
                e.loudness_global().ok().and_then(finite),
                e.loudness_range().ok().and_then(finite),
                (0..self.channels as u32)
                    .filter_map(|c| e.true_peak(c).ok())
                    .reduce(f64::max)
                    .filter(|p| *p > 0.0)
                    .map(|p| 20.0 * p.log10()),
            ),
            None => (None, None, None),
        };
        let sample_peak = (self.peak > 0.0).then(|| 20.0 * f64::from(self.peak).log10());

        let mut verdicts = Vec::new();
        let effective_bits = match self.bits.filter(|b| (9..=24).contains(b)) {
            Some(bits) if self.bits_used != 0 => {
                let effective = bits.saturating_sub(self.bits_used.trailing_zeros());
                if effective < bits && bits >= 20 && effective <= 16 {
                    verdicts.push(Verdict { kind: VerdictKind::PaddedBits { bits, effective }, confidence: Confidence::High });
                }
                (effective < bits).then_some(effective)
            }
            _ => None,
        };

        let edge = self.spectrum.finish();
        let mut cutoff_hz = None;
        if let Some(edge) = edge {
            let lossless = self.bits.is_some();
            let nyquist = self.rate / 2;
            // A resampler's brickwall, not just a deep drop: a native high-rate
            // capture of a source that rolls off (tape, old masters) can fall
            // as far, only more gradually.
            if self.rate >= 88_200 && edge.cutoff_hz <= 24_500 && edge.drop_db >= 20.0 && edge.sharp_db >= SHARP_EDGE_DB {
                let from_hz = if edge.cutoff_hz <= 22_600 { 44_100 } else { 48_000 };
                let confidence = if edge.drop_db >= 40.0 { Confidence::High } else { Confidence::Medium };
                verdicts.push(Verdict { kind: VerdictKind::Upsampled { from_hz, cutoff_hz: edge.cutoff_hz }, confidence });
                cutoff_hz = Some(edge.cutoff_hz);
            }
            if lossless
                && (10_000..=20_300).contains(&edge.cutoff_hz)
                && nyquist.saturating_sub(edge.cutoff_hz) >= 1_500
                && edge.drop_db >= 30.0
            {
                let confidence = if edge.cutoff_hz <= 19_200 && edge.drop_db >= 40.0 { Confidence::High } else { Confidence::Medium };
                verdicts.push(Verdict { kind: VerdictKind::LossySource { cutoff_hz: edge.cutoff_hz }, confidence });
                cutoff_hz = Some(edge.cutoff_hz);
            }
        }

        TrackAnalysis {
            version: ANALYSIS_VERSION,
            duration_secs: self.frames as f64 / f64::from(self.rate.max(1)),
            sample_rate: self.rate,
            channels: self.channels as u16,
            bits: self.bits,
            integrated_lufs: integrated,
            loudness_range_lu: range,
            true_peak_dbtp: true_peak,
            sample_peak_dbfs: sample_peak,
            dr: self.dr.finish(),
            effective_bits,
            cutoff_hz,
            verdicts,
        }
    }
}
