//! A whole track's peaks, for drawing its waveform: the loudest sample (of
//! any channel) in each fiftieth of a second. Small enough to keep (a ten
//! minute track is 30,000 bytes) and fine enough for any seek bar.

/// Peak values per second of audio.
pub const PEAKS_PER_SEC: u32 = 50;

#[derive(Debug, Clone, PartialEq)]
pub struct Peaks {
    /// One per 1/[`PEAKS_PER_SEC`] s, 0..=255 for 0..=full scale.
    pub values: Vec<u8>,
}

impl Peaks {
    /// The track as `n` bars, each the loudest of its share of the track,
    /// scaled so the loudest bar is 1 (a quiet recording still shows its shape).
    pub fn bars(&self, n: usize) -> Vec<f32> {
        if n == 0 || self.values.is_empty() {
            return vec![0.0; n];
        }
        let len = self.values.len();
        let raw: Vec<u8> = (0..n)
            .map(|i| {
                let (a, b) = (i * len / n, ((i + 1) * len / n).max(i * len / n + 1).min(len));
                self.values[a.min(len - 1)..b].iter().copied().max().unwrap_or(0)
            })
            .collect();
        let top = f32::from(raw.iter().copied().max().unwrap_or(0).max(1));
        raw.into_iter().map(|v| f32::from(v) / top).collect()
    }
}

/// Builds [`Peaks`] from decoded audio a block at a time.
pub struct PeakMeter {
    pub(crate) rate: u32,
    pub(crate) channels: usize,
    /// Frames per peak value.
    window: usize,
    /// Frames into the current window, and its loudest sample so far.
    filled: usize,
    peak: f32,
    values: Vec<u8>,
}

impl PeakMeter {
    pub fn new(rate: u32, channels: usize) -> Self {
        Self { rate, channels: channels.max(1), window: (rate / PEAKS_PER_SEC).max(1) as usize, filled: 0, peak: 0.0, values: Vec::new() }
    }

    /// Interleaved samples in [-1, 1]; a trailing partial frame is ignored.
    pub fn push(&mut self, samples: &[f32]) {
        for frame in samples.chunks_exact(self.channels) {
            for s in frame {
                self.peak = self.peak.max(s.abs());
            }
            self.filled += 1;
            if self.filled == self.window {
                self.values.push((self.peak.min(1.0) * 255.0).round() as u8);
                self.filled = 0;
                self.peak = 0.0;
            }
        }
    }

    pub fn finish(mut self) -> Peaks {
        if self.filled > 0 {
            self.values.push((self.peak.min(1.0) * 255.0).round() as u8);
        }
        Peaks { values: self.values }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaks_follow_the_loudest_sample_of_any_channel() {
        let rate = 1000;
        let mut m = PeakMeter::new(rate, 2);
        // Half a second quiet (left 0.1, right -0.2), half a second loud on the right.
        let quiet: Vec<f32> = (0..500).flat_map(|_| [0.1, -0.2]).collect();
        let loud: Vec<f32> = (0..500).flat_map(|i| [0.0, if i % 2 == 0 { 0.9 } else { -1.0 }]).collect();
        // In odd-sized blocks, so windows straddle pushes.
        for chunk in quiet.iter().chain(&loud).copied().collect::<Vec<_>>().chunks(334) {
            m.push(chunk);
        }
        let p = m.finish();
        assert_eq!(p.values.len(), PEAKS_PER_SEC as usize);
        assert!(p.values[..25].iter().all(|&v| v == 51), "{:?}", &p.values[..25]);
        assert!(p.values[25..].iter().all(|&v| v == 255));
    }

    #[test]
    fn a_trailing_partial_window_counts() {
        let mut m = PeakMeter::new(1000, 1);
        m.push(&[0.5; 15]);
        assert_eq!(m.finish().values, [128]);
        assert!(PeakMeter::new(44_100, 2).finish().values.is_empty());
    }

    #[test]
    fn bars_take_the_loudest_of_their_span_and_scale_to_the_loudest() {
        let p = Peaks { values: vec![10, 20, 40, 30, 0, 0, 80, 5] };
        assert_eq!(p.bars(4), [0.25, 0.5, 0.0, 1.0]);
        // More bars than values: each bar repeats its value.
        assert_eq!(p.bars(16).len(), 16);
        assert_eq!(p.bars(16)[12], 1.0);
        assert!(Peaks { values: Vec::new() }.bars(3).iter().all(|&v| v == 0.0));
        assert!(p.bars(0).is_empty());
    }
}
