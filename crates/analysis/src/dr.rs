//! The DR meter (TT Dynamic Range / foobar2000's DR Meter): per channel, cut
//! the track into 3 s blocks and take each block's RMS (×√2, so a full-scale
//! sine reads 0 dB) and peak. DR is the ratio of the second-highest block
//! peak to the RMS of the loudest 20% of blocks, averaged over channels.

const BLOCK_SECS: u32 = 3;

pub struct DrMeter {
    block_frames: u64,
    channels: usize,
    /// Running sums for the block in progress, per channel.
    sum_sq: Vec<f64>,
    peak: Vec<f32>,
    filled: u64,
    /// Finished blocks: (rms, peak) per channel.
    blocks: Vec<Vec<(f64, f64)>>,
}

impl DrMeter {
    pub fn new(rate: u32, channels: usize) -> Self {
        Self {
            block_frames: u64::from(rate.max(1) * BLOCK_SECS),
            channels,
            sum_sq: vec![0.0; channels],
            peak: vec![0.0; channels],
            filled: 0,
            blocks: vec![Vec::new(); channels],
        }
    }

    pub fn push(&mut self, samples: &[f32]) {
        for frame in samples.chunks_exact(self.channels) {
            for (c, &s) in frame.iter().enumerate() {
                self.sum_sq[c] += f64::from(s) * f64::from(s);
                self.peak[c] = self.peak[c].max(s.abs());
            }
            self.filled += 1;
            if self.filled == self.block_frames {
                self.close_block();
            }
        }
    }

    fn close_block(&mut self) {
        let n = self.filled as f64;
        for c in 0..self.channels {
            let rms = (2.0 * self.sum_sq[c] / n).sqrt();
            self.blocks[c].push((rms, f64::from(self.peak[c])));
            self.sum_sq[c] = 0.0;
            self.peak[c] = 0.0;
        }
        self.filled = 0;
    }

    /// The unrounded DR value, or `None` for silence or nothing at all.
    pub fn finish(mut self) -> Option<f64> {
        // A trailing partial block only counts when it's all there is.
        if self.blocks[0].is_empty() && self.filled > 0 {
            self.close_block();
        }
        let mut per_channel = Vec::new();
        for blocks in &self.blocks {
            if blocks.is_empty() {
                continue;
            }
            let mut rms: Vec<f64> = blocks.iter().map(|b| b.0).collect();
            let mut peaks: Vec<f64> = blocks.iter().map(|b| b.1).collect();
            rms.sort_by(|a, b| b.total_cmp(a));
            peaks.sort_by(|a, b| b.total_cmp(a));
            let top = (rms.len() / 5).max(1);
            let rms_top = (rms[..top].iter().map(|r| r * r).sum::<f64>() / top as f64).sqrt();
            let peak = if peaks.len() >= 2 { peaks[1] } else { peaks[0] };
            if rms_top > 0.0 && peak > 0.0 {
                per_channel.push(20.0 * (peak / rms_top).log10());
            }
        }
        (!per_channel.is_empty()).then(|| per_channel.iter().sum::<f64>() / per_channel.len() as f64)
    }
}
