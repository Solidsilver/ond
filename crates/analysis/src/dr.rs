//! The DR meter (TT Dynamic Range / foobar2000's DR Meter): per channel, cut
//! the track into 3 s blocks and take each block's RMS (×√2, so a full-scale
//! sine reads 0 dB) and peak. DR is the ratio of the second-highest block
//! peak to the RMS of the loudest 20% of blocks, averaged over channels.
//!
//! The details follow `compute_dr14` in dr14_t.meter (simon-r/dr14_t.meter),
//! the open reference implementation:
//! - at 44.1 kHz a block is 3 × 44 160 frames, not 3 × 44 100;
//! - the trailing partial block is a block like the others (its RMS over its
//!   own length);
//! - the block count is `floor(frames / block) + 1`, so the trailing slot
//!   counts even when the length is an exact multiple and it's empty, and
//!   the loudest 20% is `floor(count / 5)` blocks, at least one;
//! - with a single block, its own peak stands in for the second-highest.
//!
//! Where this differs, on purpose: the reference leaves the very last sample
//! out of the last block (an off-by-one with no audible meaning); it treats a
//! lone exactly-3 s block's missing second peak as zero (giving DR 0); and it
//! scores a silent channel, or one above 24 dB, as DR 0 in the channel mean,
//! where this leaves such a channel out (and reports no DR for silence).

const BLOCK_SECS: u64 = 3;

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
        // foobar2000's meter, which dr14_t.meter matches, stretches blocks by
        // 60 frames a second at 44.1 kHz.
        let per_sec = u64::from(rate.max(1)) + if rate == 44_100 { 60 } else { 0 };
        Self {
            block_frames: BLOCK_SECS * per_sec,
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
        // Full blocks plus the trailing slot, filled or not.
        let count = self.blocks.first().map_or(0, Vec::len) + 1;
        if self.filled > 0 {
            self.close_block();
        }
        let top = (count / 5).max(1);
        let mut per_channel = Vec::new();
        for blocks in &self.blocks {
            if blocks.is_empty() {
                continue;
            }
            let mut rms: Vec<f64> = blocks.iter().map(|b| b.0).collect();
            let mut peaks: Vec<f64> = blocks.iter().map(|b| b.1).collect();
            rms.sort_by(|a, b| b.total_cmp(a));
            peaks.sort_by(|a, b| b.total_cmp(a));
            let top = top.min(rms.len());
            let rms_top = (rms[..top].iter().map(|r| r * r).sum::<f64>() / top as f64).sqrt();
            let peak = if peaks.len() >= 2 { peaks[1] } else { peaks[0] };
            if rms_top > 0.0 && peak > 0.0 {
                per_channel.push(20.0 * (peak / rms_top).log10());
            }
        }
        (!per_channel.is_empty()).then(|| per_channel.iter().sum::<f64>() / per_channel.len() as f64)
    }
}
