//! What the output path does to the signal, stated honestly.

use crate::PlaybackMode;
use crate::channels::ChannelChange;
use crate::output::StreamFormat;
use crate::replaygain::{AppliedGain, GainSource};

/// Whether playback is bit-perfect, and why not when it isn't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BitPerfectStatus {
    /// Normal mode: volume and other DSP are allowed; no claim is made.
    Off,
    /// Bit-perfect was asked for and nothing in this app alters the samples,
    /// but something outside it may (e.g. the OS mixer in shared mode).
    Requested { reason: String },
    /// Playback isn't bit-perfect and can't be made so here: the device can't
    /// take the file (`device_limit`: its rate or bit depth is out of range, so
    /// playback goes through the resampler and dither), or the samples were
    /// changed before reaching it (a server transcode, a downmix).
    Unavailable { reason: String, device_limit: bool },
    /// Every condition holds: exclusive access, the device runs at the file's
    /// rate with an integer format at least as wide as the file, no resampling
    /// and no gain or DSP.
    Verified,
}

/// One range of sample rates a device supports at a channel count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateRange {
    pub channels: u16,
    pub min: u32,
    pub max: u32,
}

#[derive(Debug, Clone, Default)]
pub struct DeviceCaps {
    pub name: String,
    pub configs: Vec<RateRange>,
    /// The device's real word length (widest integer format, or 24 for a
    /// float-only device), when the backend can see it.
    pub max_bits: Option<u32>,
    /// The backend has exclusive (hog mode) access.
    pub exclusive: bool,
    /// Why the path can't be verified even when formats match, e.g. "shared mode (cpal)".
    pub shared_reason: Option<String>,
    /// The device's current nominal rate, when known.
    pub nominal_rate: Option<u32>,
    /// Integer bits of the physical format in use right now (None: float or unknown).
    pub active_bits: Option<u32>,
    /// The OS processes this output after us (e.g. built-in speaker EQ), so it can never be verified.
    pub post_processed: Option<String>,
    /// Some format takes integer samples: without one, bit-perfect can't be
    /// verified (a Mac's headphone jack is float-only).
    pub integer: bool,
}

impl DeviceCaps {
    pub fn supports(&self, rate: u32, channels: u16) -> bool {
        self.configs.iter().any(|c| c.channels == channels && c.min <= rate && rate <= c.max)
    }

    pub fn max_rate(&self) -> Option<u32> {
        self.configs.iter().map(|c| c.max).max()
    }

    /// Rates worth offering: the common ones the device supports, plus range ends.
    fn rates(&self, channels: u16) -> Vec<u32> {
        let mut rates: Vec<u32> = COMMON_RATES.iter().copied().filter(|&r| self.supports(r, channels)).collect();
        for c in self.configs.iter().filter(|c| c.channels == channels) {
            rates.extend([c.min, c.max]);
        }
        rates.sort_unstable();
        rates.dedup();
        rates
    }
}

const COMMON_RATES: [u32; 15] =
    [8_000, 11_025, 16_000, 22_050, 32_000, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 352_800, 384_000, 705_600, 768_000];

/// The rate to run the device at for a file at `src`:
/// 1. the file's own rate, if the device supports it;
/// 2. else the highest supported rate an integer ratio away below it (176.4 → 88.2
///    or 44.1, 192 → 96 or 48), or the lowest one above it;
/// 3. else the closest supported rate.
///
/// Without capabilities (or for a channel count the device doesn't list) the
/// file's rate is used and the backend decides.
pub fn choose_rate(src: u32, channels: u16, caps: Option<&DeviceCaps>) -> u32 {
    let Some(caps) = caps else { return src };
    if src == 0 || caps.supports(src, channels) {
        return src;
    }
    let rates = caps.rates(channels);
    let related = |r: u32| r > 0 && (src.is_multiple_of(r) || r.is_multiple_of(src));
    if let Some(r) = rates.iter().copied().filter(|&r| related(r) && r < src).max() {
        return r;
    }
    if let Some(r) = rates.iter().copied().filter(|&r| related(r) && r > src).min() {
        return r;
    }
    rates.iter().copied().min_by_key(|&r| (r.abs_diff(src), u32::MAX - r)).unwrap_or(src)
}

/// Judges what bit-perfect means for a `source` file played at `device_rate` on a
/// device with `caps`. Bit-perfect mode guarantees no gain or DSP in this app;
/// everything else is checked here.
pub fn assess(mode: PlaybackMode, source: &StreamFormat, device_rate: u32, caps: Option<&DeviceCaps>) -> BitPerfectStatus {
    if mode == PlaybackMode::Normal {
        return BitPerfectStatus::Off;
    }
    let Some(caps) = caps else {
        return BitPerfectStatus::Requested { reason: "device capabilities unknown".into() };
    };
    let file = describe(source.bits_per_sample, source.sample_rate);
    let max = caps.max_rate().unwrap_or(source.sample_rate);
    if !caps.supports(source.sample_rate, source.channels) {
        let reason = if source.sample_rate > max {
            format!("device max {}, file is {file}", describe(caps.max_bits, max))
        } else {
            format!("device can't run at {} kHz", khz(source.sample_rate))
        };
        return BitPerfectStatus::Unavailable { reason, device_limit: true };
    }
    if let (Some(max_bits), Some(bits)) = (caps.max_bits, source.bits_per_sample)
        && bits > max_bits
    {
        return BitPerfectStatus::Unavailable { reason: format!("device max {}, file is {file}", describe(caps.max_bits, max)), device_limit: true };
    }
    if device_rate != source.sample_rate {
        return BitPerfectStatus::Unavailable { reason: format!("resampled to {} kHz", khz(device_rate)), device_limit: false };
    }
    if let Some(reason) = &caps.post_processed {
        return BitPerfectStatus::Requested { reason: reason.clone() };
    }
    if !caps.exclusive {
        return BitPerfectStatus::Requested { reason: caps.shared_reason.clone().unwrap_or_else(|| "shared mode".into()) };
    }
    match caps.nominal_rate {
        Some(r) if r == source.sample_rate => {}
        Some(r) => return BitPerfectStatus::Requested { reason: format!("device is at {} kHz", khz(r)) },
        None => return BitPerfectStatus::Requested { reason: "device rate unknown".into() },
    }
    match (caps.active_bits, source.bits_per_sample) {
        (Some(active), Some(bits)) if active >= bits => BitPerfectStatus::Verified,
        (Some(active), Some(_)) => BitPerfectStatus::Requested { reason: format!("device format is {active}-bit") },
        (None, _) => BitPerfectStatus::Requested { reason: "device format isn't integer".into() },
        // Lossy sources have no bit depth: there's nothing to be bit-perfect to.
        (Some(_), None) => BitPerfectStatus::Requested { reason: "lossy source".into() },
    }
}

/// Everything the UI needs to describe the path from file to device.
#[derive(Debug, Clone, PartialEq)]
pub struct SignalPath {
    /// Backend and device, e.g. "CoreAudio · UMC22".
    pub output: String,
    pub source: StreamFormat,
    pub device_rate: u32,
    /// Integer word length reaching the DAC, when known.
    pub device_bits: Option<u32>,
    pub resampled: bool,
    /// The output is requantized with TPDF dither (narrower device or processed signal).
    pub dithered: bool,
    pub bit_perfect: BitPerfectStatus,
    /// Mono duplicated, or surround mixed down.
    pub channels: ChannelChange,
    pub gain: GainState,
    /// The server transcoded the stream, e.g. "AAC 256".
    pub transcoded: Option<String>,
}

/// ReplayGain on the current track.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum GainState {
    /// ReplayGain is off.
    #[default]
    Off,
    Applied(AppliedGain),
    /// On, but bit-perfect mode leaves the samples alone.
    Bypassed,
}

impl GainState {
    fn label(&self) -> Option<String> {
        match self {
            GainState::Off => None,
            GainState::Bypassed => Some("ReplayGain bypassed".into()),
            GainState::Applied(g) => {
                let db = g.db();
                let db = if db.abs() < 0.05 { "0 dB".to_owned() } else { format!("{db:+.1} dB").replace('-', "−") };
                let what = match g.source {
                    GainSource::Track => "track",
                    GainSource::Album => "album",
                    GainSource::Untagged => "untagged",
                };
                Some(format!("RG {db} {what}{}", if g.limited { ", peak-limited" } else { "" }))
            }
        }
    }
}

/// Folds what happens outside the device check into the verdict: a server
/// transcode or a downmix means the file's samples never reach the device;
/// a mono file duplicated onto two channels is exact but not what was asked.
pub fn adjust(status: BitPerfectStatus, channels: ChannelChange, transcoded: Option<&str>) -> BitPerfectStatus {
    if status == BitPerfectStatus::Off {
        return status;
    }
    if let Some(t) = transcoded {
        return BitPerfectStatus::Unavailable { reason: format!("transcoded by the server ({t})"), device_limit: false };
    }
    match channels {
        ChannelChange::Downmix { from, to } => BitPerfectStatus::Unavailable {
            reason: format!("{} downmixed to {}", crate::channels::layout(from), crate::channels::layout(to)),
            device_limit: false,
        },
        ChannelChange::Upmix { from, to } if !matches!(status, BitPerfectStatus::Unavailable { .. }) => {
            BitPerfectStatus::Requested {
                reason: format!("{} upmixed to {}", crate::channels::layout(from), crate::channels::layout(to)),
            }
        }
        _ => status,
    }
}

impl SignalPath {
    /// One terse line: `24/96 → 16/48 · resampled, dithered · device limit`.
    pub fn summary(&self) -> String {
        let src = describe(self.source.bits_per_sample, self.source.sample_rate);
        let changed = self.resampled || matches!((self.device_bits, self.source.bits_per_sample), (Some(d), Some(s)) if d < s);
        let mut parts = Vec::new();
        if let Some(t) = &self.transcoded {
            parts.push(format!("{t} · transcoded"));
        }
        parts.push(if changed { format!("{src} → {}", describe(self.device_bits, self.device_rate)) } else { src });
        if let Some(c) = self.channels.label() {
            parts.push(c);
        }
        let mut processing = Vec::new();
        if self.resampled {
            processing.push("resampled");
        }
        if self.dithered {
            processing.push("dithered");
        }
        if !processing.is_empty() {
            parts.push(processing.join(", "));
        }
        if let Some(g) = self.gain.label() {
            parts.push(g);
        }
        match &self.bit_perfect {
            BitPerfectStatus::Off => {}
            BitPerfectStatus::Verified => parts.push("bit-perfect".into()),
            BitPerfectStatus::Requested { reason } => parts.push(format!("not verified ({reason})")),
            BitPerfectStatus::Unavailable { device_limit: true, .. } => parts.push("device limit".into()),
            BitPerfectStatus::Unavailable { reason, .. } => parts.push(reason.clone()),
        }
        parts.join(" · ")
    }
}

/// `24/96` when the depth is known, else `96 kHz`.
pub fn describe(bits: Option<u32>, rate: u32) -> String {
    match bits {
        Some(b) => format!("{b}/{}", khz(rate)),
        None => format!("{} kHz", khz(rate)),
    }
}

/// 44100 → "44.1", 48000 → "48".
pub fn khz(rate: u32) -> String {
    if rate.is_multiple_of(1000) { (rate / 1000).to_string() } else { format!("{:.1}", rate as f64 / 1000.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Behringer UMC22-like device: 16-bit, 44.1 or 48 kHz.
    fn umc22() -> DeviceCaps {
        DeviceCaps {
            name: "UMC22".into(),
            configs: vec![RateRange { channels: 2, min: 44_100, max: 44_100 }, RateRange { channels: 2, min: 48_000, max: 48_000 }],
            max_bits: Some(16),
            exclusive: false,
            shared_reason: Some("shared mode (cpal)".into()),
            ..Default::default()
        }
    }

    /// An M-series headphone jack: discrete rates up to 96 kHz, 24-bit.
    fn jack() -> DeviceCaps {
        let configs = [44_100, 48_000, 88_200, 96_000].map(|r| RateRange { channels: 2, min: r, max: r }).to_vec();
        DeviceCaps { name: "External Headphones".into(), configs, max_bits: Some(24), ..Default::default() }
    }

    fn fmt(bits: u32, rate: u32) -> StreamFormat {
        StreamFormat { sample_rate: rate, channels: 2, bits_per_sample: Some(bits) }
    }

    fn exclusive(caps: DeviceCaps, rate: u32, active_bits: u32) -> DeviceCaps {
        DeviceCaps { exclusive: true, nominal_rate: Some(rate), active_bits: Some(active_bits), shared_reason: None, ..caps }
    }

    #[test]
    fn rate_choice_prefers_the_file_rate_then_the_same_family() {
        let (u, j) = (umc22(), jack());
        assert_eq!(choose_rate(44_100, 2, Some(&u)), 44_100);
        assert_eq!(choose_rate(96_000, 2, Some(&u)), 48_000);
        assert_eq!(choose_rate(192_000, 2, Some(&u)), 48_000);
        assert_eq!(choose_rate(88_200, 2, Some(&u)), 44_100);
        assert_eq!(choose_rate(176_400, 2, Some(&u)), 44_100);
        assert_eq!(choose_rate(176_400, 2, Some(&j)), 88_200);
        assert_eq!(choose_rate(192_000, 2, Some(&j)), 96_000);
        // Below the device: the lowest integer multiple, not the highest.
        assert_eq!(choose_rate(22_050, 2, Some(&j)), 44_100);
        // No integer relation: the closest rate.
        assert_eq!(choose_rate(32_000, 2, Some(&u)), 44_100);
        assert_eq!(choose_rate(50_000, 2, Some(&u)), 48_000);
        // Unknown device or channel layout: leave it to the backend.
        assert_eq!(choose_rate(96_000, 2, None), 96_000);
        assert_eq!(choose_rate(96_000, 6, Some(&u)), 96_000);
    }

    #[test]
    fn normal_mode_makes_no_claim() {
        assert_eq!(assess(PlaybackMode::Normal, &fmt(16, 44_100), 44_100, Some(&umc22())), BitPerfectStatus::Off);
    }

    #[test]
    fn hi_res_on_a_16_48_device_is_unavailable() {
        let s = assess(PlaybackMode::BitPerfect, &fmt(24, 96_000), 48_000, Some(&umc22()));
        assert_eq!(s, BitPerfectStatus::Unavailable { reason: "device max 16/48, file is 24/96".into(), device_limit: true });
        let s = assess(PlaybackMode::BitPerfect, &fmt(24, 48_000), 48_000, Some(&umc22()));
        assert_eq!(s, BitPerfectStatus::Unavailable { reason: "device max 16/48, file is 24/48".into(), device_limit: true });
        let s = assess(PlaybackMode::BitPerfect, &fmt(16, 88_200), 44_100, Some(&umc22()));
        assert_eq!(s, BitPerfectStatus::Unavailable { reason: "device max 16/48, file is 16/88.2".into(), device_limit: true });
    }

    #[test]
    fn matching_format_in_shared_mode_is_only_requested() {
        let s = assess(PlaybackMode::BitPerfect, &fmt(16, 44_100), 44_100, Some(&umc22()));
        assert_eq!(s, BitPerfectStatus::Requested { reason: "shared mode (cpal)".into() });
    }

    #[test]
    fn unknown_device_depth_falls_back_to_rate_only() {
        let caps = DeviceCaps { max_bits: None, ..umc22() };
        let s = assess(PlaybackMode::BitPerfect, &fmt(24, 96_000), 48_000, Some(&caps));
        assert_eq!(s, BitPerfectStatus::Unavailable { reason: "device max 48 kHz, file is 24/96".into(), device_limit: true });
        // A 24-bit file at a supported rate can't be judged on depth: only "requested".
        let s = assess(PlaybackMode::BitPerfect, &fmt(24, 48_000), 48_000, Some(&caps));
        assert!(matches!(s, BitPerfectStatus::Requested { .. }));
    }

    #[test]
    fn verified_needs_every_condition() {
        let ok = exclusive(jack(), 96_000, 24);
        assert_eq!(assess(PlaybackMode::BitPerfect, &fmt(24, 96_000), 96_000, Some(&ok)), BitPerfectStatus::Verified);
        // 16-bit files on a 24-bit integer format are exact too.
        let ok44 = exclusive(jack(), 44_100, 24);
        assert_eq!(assess(PlaybackMode::BitPerfect, &fmt(16, 44_100), 44_100, Some(&ok44)), BitPerfectStatus::Verified);

        let wrong_rate = exclusive(jack(), 48_000, 24);
        assert!(matches!(assess(PlaybackMode::BitPerfect, &fmt(24, 96_000), 96_000, Some(&wrong_rate)), BitPerfectStatus::Requested { .. }));
        let narrow = exclusive(jack(), 96_000, 16);
        assert!(matches!(assess(PlaybackMode::BitPerfect, &fmt(24, 96_000), 96_000, Some(&narrow)), BitPerfectStatus::Requested { .. }));
        let float = DeviceCaps { active_bits: None, ..ok.clone() };
        assert!(matches!(assess(PlaybackMode::BitPerfect, &fmt(24, 96_000), 96_000, Some(&float)), BitPerfectStatus::Requested { .. }));
        let shared = DeviceCaps { exclusive: false, ..ok.clone() };
        assert!(matches!(assess(PlaybackMode::BitPerfect, &fmt(24, 96_000), 96_000, Some(&shared)), BitPerfectStatus::Requested { .. }));
        let speakers = DeviceCaps { post_processed: Some("macOS speaker processing".into()), ..ok.clone() };
        assert_eq!(
            assess(PlaybackMode::BitPerfect, &fmt(24, 96_000), 96_000, Some(&speakers)),
            BitPerfectStatus::Requested { reason: "macOS speaker processing".into() }
        );
        // Resampled is never bit-perfect, whatever else holds.
        assert!(matches!(assess(PlaybackMode::BitPerfect, &fmt(24, 96_000), 48_000, Some(&ok)), BitPerfectStatus::Unavailable { .. }));
    }

    #[test]
    fn summary_is_terse() {
        let path = SignalPath {
            output: "CoreAudio · UMC22".into(),
            source: fmt(24, 96_000),
            device_rate: 48_000,
            device_bits: Some(16),
            resampled: true,
            dithered: true,
            bit_perfect: BitPerfectStatus::Unavailable { reason: "device max 16/48, file is 24/96".into(), device_limit: true },
            channels: ChannelChange::None,
            gain: GainState::Off,
            transcoded: None,
        };
        assert_eq!(path.summary(), "24/96 → 16/48 · resampled, dithered · device limit");
        let verified = SignalPath {
            source: fmt(16, 44_100),
            device_rate: 44_100,
            resampled: false,
            dithered: false,
            bit_perfect: BitPerfectStatus::Verified,
            ..path.clone()
        };
        assert_eq!(verified.summary(), "16/44.1 · bit-perfect");
        let normal = SignalPath { bit_perfect: BitPerfectStatus::Off, ..verified.clone() };
        assert_eq!(normal.summary(), "16/44.1");

        let gained = SignalPath {
            gain: GainState::Applied(AppliedGain { linear: 0.398_107_2, source: GainSource::Album, limited: false }),
            ..normal.clone()
        };
        assert_eq!(gained.summary(), "16/44.1 · RG −8.0 dB album");
        let bypassed = SignalPath { gain: GainState::Bypassed, ..verified.clone() };
        assert_eq!(bypassed.summary(), "16/44.1 · ReplayGain bypassed · bit-perfect");
        let mono = SignalPath { channels: ChannelChange::Upmix { from: 1, to: 2 }, ..normal.clone() };
        assert_eq!(mono.summary(), "16/44.1 · mono → stereo");
        let aac = SignalPath {
            source: StreamFormat { sample_rate: 44_100, channels: 2, bits_per_sample: None },
            transcoded: Some("AAC 256".into()),
            ..normal
        };
        assert_eq!(aac.summary(), "AAC 256 · transcoded · 44.1 kHz");
    }

    #[test]
    fn transcodes_and_channel_changes_fold_into_the_verdict() {
        let v = BitPerfectStatus::Verified;
        let downmix = ChannelChange::Downmix { from: 6, to: 2 };
        assert_eq!(adjust(BitPerfectStatus::Off, downmix, Some("AAC 256")), BitPerfectStatus::Off);
        assert_eq!(
            adjust(v.clone(), ChannelChange::None, Some("AAC 256")),
            BitPerfectStatus::Unavailable { reason: "transcoded by the server (AAC 256)".into(), device_limit: false }
        );
        assert_eq!(adjust(v.clone(), downmix, None), BitPerfectStatus::Unavailable { reason: "5.1 downmixed to stereo".into(), device_limit: false });
        assert_eq!(
            adjust(v.clone(), ChannelChange::Upmix { from: 1, to: 2 }, None),
            BitPerfectStatus::Requested { reason: "mono upmixed to stereo".into() }
        );
        let limit = BitPerfectStatus::Unavailable { reason: "device max 16/48, file is 24/96".into(), device_limit: true };
        assert_eq!(adjust(limit.clone(), ChannelChange::Upmix { from: 1, to: 2 }, None), limit);
        assert_eq!(adjust(v.clone(), ChannelChange::None, None), v);
    }
}
