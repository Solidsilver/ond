use crate::Status;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Per-track DSP stage between the decoder and the ring buffer.
///
/// Bit-perfect mode never touches samples. Volume lives in the output callback
/// (so it responds immediately); ReplayGain, a limiter and resampling slot in
/// here later. The mode is read per call, so switching takes effect at once.
pub struct Dsp {
    status: Arc<Status>,
}

impl Dsp {
    pub fn new(status: Arc<Status>) -> Self {
        Self { status }
    }

    /// Processes interleaved samples in place.
    pub fn process(&mut self, samples: &mut [f32]) {
        if self.status.bit_perfect.load(Ordering::Relaxed) {
            return;
        }
        // Normal-mode stages (ReplayGain, limiter) go here.
        let _ = samples;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_perfect_is_untouched() {
        let status = Arc::new(Status::default());
        status.bit_perfect.store(true, Ordering::Relaxed);
        status.set_volume(0.5);
        let mut dsp = Dsp::new(status);
        let mut buf = [0.25f32, -1.0, 0.123_456_79];
        let before = buf;
        dsp.process(&mut buf);
        assert_eq!(buf.map(f32::to_bits), before.map(f32::to_bits));
    }
}
