//! Fitting a file's channels to the device: mono is duplicated onto both
//! speakers of a stereo device, and more than two channels are folded down to
//! stereo unless the device has that many outputs. Runs on the decode thread.

use crate::signal::DeviceCaps;

/// −3 dB: how much of a centre or surround channel goes into each side.
const SIDE: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// What reaches the device, channel-wise, compared to the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelChange {
    #[default]
    None,
    /// Mono duplicated onto both channels. Samples are unchanged.
    Upmix { from: u16, to: u16 },
    /// Several channels mixed down (the samples are recomputed).
    Downmix { from: u16, to: u16 },
}

impl ChannelChange {
    pub fn label(&self) -> Option<String> {
        match *self {
            ChannelChange::None => None,
            ChannelChange::Upmix { from, to } => Some(format!("{} → {}", layout(from), layout(to))),
            ChannelChange::Downmix { from, to } => Some(format!("{} → {}, downmixed", layout(from), layout(to))),
        }
    }
}

/// "mono", "stereo", "5.1", or "N ch".
pub fn layout(channels: u16) -> String {
    match channels {
        1 => "mono".into(),
        2 => "stereo".into(),
        6 => "5.1".into(),
        8 => "7.1".into(),
        n => format!("{n} ch"),
    }
}

/// How many channels to send for a file with `src` channels.
pub fn output_channels(src: u16, caps: Option<&DeviceCaps>) -> u16 {
    let has = |n: u16| caps.is_some_and(|c| c.configs.iter().any(|r| r.channels == n));
    match src {
        // A real mono-only device keeps mono; anything else gets both speakers.
        1 if has(1) && !has(2) => 1,
        0..=2 => 2,
        n if has(n) => n,
        _ => 2,
    }
}

/// Converts interleaved frames from the file's layout to the device's.
#[derive(Debug, Clone)]
pub enum ChannelMap {
    Identity,
    /// One channel to `to`, copied.
    Duplicate {
        to: usize,
    },
    /// `rows[out][in]` weights, `inputs` channels in.
    Matrix {
        inputs: usize,
        rows: Vec<Vec<f32>>,
    },
}

impl ChannelMap {
    /// The map for `src` channels with speaker positions `positions` (a
    /// symphonia `Position` bit mask; `None` assumes the standard WAVE order:
    /// FL FR FC LFE BL BR FLC FRC BC SL SR ...) going to `out` channels.
    pub fn new(src: u16, positions: Option<u64>, out: u16) -> (Self, ChannelChange) {
        if src == out || src == 0 {
            return (ChannelMap::Identity, ChannelChange::None);
        }
        if src == 1 {
            return (ChannelMap::Duplicate { to: out as usize }, ChannelChange::Upmix { from: 1, to: out });
        }
        if out == 1 {
            let row = vec![1.0 / src as f32; src as usize];
            return (ChannelMap::Matrix { inputs: src as usize, rows: vec![row] }, ChannelChange::Downmix { from: src, to: 1 });
        }
        let mask = positions.filter(|m| m.count_ones() == src as u32).unwrap_or((1u64 << src.min(63)) - 1);
        let mut left = Vec::with_capacity(src as usize);
        let mut right = Vec::with_capacity(src as usize);
        for bit in (0..64).filter(|b| mask & (1 << b) != 0) {
            let (l, r) = weights(bit);
            left.push(l);
            right.push(r);
        }
        // Scale each side so a full-scale signal on every input can't clip.
        for row in [&mut left, &mut right] {
            let sum: f32 = row.iter().sum();
            if sum > 1.0 {
                row.iter_mut().for_each(|w| *w /= sum);
            }
        }
        let mut rows = vec![left, right];
        // A device with more than two outputs gets stereo on the first two.
        rows.resize(out as usize, vec![0.0; src as usize]);
        (ChannelMap::Matrix { inputs: src as usize, rows }, ChannelChange::Downmix { from: src, to: out })
    }

    /// Maps `input` into `out` (cleared first). Identity is a plain copy.
    pub fn apply(&self, input: &[f32], out: &mut Vec<f32>) {
        out.clear();
        match self {
            ChannelMap::Identity => out.extend_from_slice(input),
            ChannelMap::Duplicate { to } => {
                out.reserve(input.len() * to);
                for &s in input {
                    out.extend(std::iter::repeat_n(s, *to));
                }
            }
            ChannelMap::Matrix { inputs, rows } => {
                out.reserve(input.len() / inputs * rows.len());
                for frame in input.chunks_exact(*inputs) {
                    for row in rows {
                        out.push(row.iter().zip(frame).map(|(w, s)| w * s).sum());
                    }
                }
            }
        }
    }

    pub fn is_identity(&self) -> bool {
        matches!(self, ChannelMap::Identity)
    }
}

/// (left, right) weights for a symphonia `Position` bit, ITU-R BS.775 style:
/// front left/right at full level, centre and surrounds at −3 dB, LFE dropped.
fn weights(bit: u32) -> (f32, f32) {
    match bit {
        0 => (1.0, 0.0),                                         // front left
        1 => (0.0, 1.0),                                         // front right
        3 | 18 => (0.0, 0.0),                                    // LFE
        2 | 8 | 11 | 13 | 16 | 21 => (SIDE, SIDE),               // centres
        4 | 6 | 9 | 12 | 15 | 19 | 22 | 24 | 26 => (SIDE, 0.0),  // left surrounds, heights, wides
        5 | 7 | 10 | 14 | 17 | 20 | 23 | 25 | 27 => (0.0, SIDE), // right side
        _ => (SIDE / 2.0, SIDE / 2.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::RateRange;

    fn caps(channels: &[u16]) -> DeviceCaps {
        DeviceCaps { configs: channels.iter().map(|&c| RateRange { channels: c, min: 44_100, max: 96_000 }).collect(), ..Default::default() }
    }

    #[test]
    fn output_channel_choice() {
        let stereo = caps(&[2, 1]);
        assert_eq!(output_channels(1, Some(&stereo)), 2, "mono goes to both speakers");
        assert_eq!(output_channels(1, Some(&caps(&[1]))), 1, "a mono-only device stays mono");
        assert_eq!(output_channels(1, None), 2);
        assert_eq!(output_channels(2, Some(&stereo)), 2);
        assert_eq!(output_channels(6, Some(&stereo)), 2);
        assert_eq!(output_channels(6, None), 2);
        assert_eq!(output_channels(6, Some(&caps(&[6, 2, 1]))), 6, "a 5.1 interface plays 5.1");
    }

    #[test]
    fn mono_is_duplicated_exactly() {
        let (map, change) = ChannelMap::new(1, None, 2);
        assert_eq!(change, ChannelChange::Upmix { from: 1, to: 2 });
        let input = [0.1f32, -0.7, 0.123_456_79];
        let mut out = Vec::new();
        map.apply(&input, &mut out);
        assert_eq!(out, [0.1, 0.1, -0.7, -0.7, 0.123_456_79, 0.123_456_79]);
    }

    #[test]
    fn stereo_is_identity() {
        let (map, change) = ChannelMap::new(2, None, 2);
        assert!(map.is_identity());
        assert_eq!(change, ChannelChange::None);
    }

    #[test]
    fn five_one_downmix_uses_standard_weights_and_never_clips() {
        // FL FR FC LFE BL BR.
        let (map, change) = ChannelMap::new(6, Some(0b11_1111), 2);
        assert_eq!(change, ChannelChange::Downmix { from: 6, to: 2 });
        let mut out = Vec::new();
        // Only front left: lands on the left, scaled by the normalisation.
        map.apply(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0], &mut out);
        let norm = 1.0 + 2.0 * SIDE;
        assert!((out[0] - 1.0 / norm).abs() < 1e-6 && out[1] == 0.0);
        // Centre: equally on both sides at −3 dB relative to the fronts.
        map.apply(&[0.0, 0.0, 1.0, 0.0, 0.0, 0.0], &mut out);
        assert!((out[0] - out[1]).abs() < 1e-7);
        assert!((out[0] - SIDE / norm).abs() < 1e-6);
        // LFE is dropped.
        map.apply(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0], &mut out);
        assert_eq!(out, [0.0, 0.0]);
        // Everything at full scale stays within ±1.
        map.apply(&[1.0; 6], &mut out);
        assert!(out.iter().all(|s| s.abs() <= 1.0 + 1e-6), "{out:?}");
        // Right surround goes right only.
        map.apply(&[0.0, 0.0, 0.0, 0.0, 0.0, 1.0], &mut out);
        assert!(out[0] == 0.0 && out[1] > 0.0);
    }

    #[test]
    fn missing_positions_assume_wave_order() {
        let (a, _) = ChannelMap::new(6, None, 2);
        let (b, _) = ChannelMap::new(6, Some(0b11_1111), 2);
        let mut x = Vec::new();
        let mut y = Vec::new();
        let frame = [0.3, -0.2, 0.5, 0.9, -0.4, 0.1];
        a.apply(&frame, &mut x);
        b.apply(&frame, &mut y);
        assert_eq!(x, y);
    }

    #[test]
    fn labels() {
        assert_eq!(ChannelChange::Upmix { from: 1, to: 2 }.label().as_deref(), Some("mono → stereo"));
        assert_eq!(ChannelChange::Downmix { from: 6, to: 2 }.label().as_deref(), Some("5.1 → stereo, downmixed"));
        assert_eq!(ChannelChange::None.label(), None);
    }
}
