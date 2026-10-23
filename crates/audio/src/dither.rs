//! Requantization to a device's integer word length with TPDF dither.
//!
//! Used only when the signal is off the device's integer grid: it was
//! resampled, scaled by volume, or comes from a source deeper than the device
//! (24-bit file on a 16-bit DAC). A bit-perfect signal is already on the grid
//! and is never touched. After quantizing, every sample is exactly representable
//! in the device's format, so the OS's own float → integer conversion is exact
//! and adds no truncation distortion.

/// Triangular-PDF dither source. Realtime-safe: no allocation, no locks.
#[derive(Debug, Clone)]
pub struct Tpdf {
    state: u32,
}

impl Default for Tpdf {
    fn default() -> Self {
        Self::new(0x9E37_79B9)
    }
}

impl Tpdf {
    pub fn new(seed: u32) -> Self {
        Self { state: seed.max(1) }
    }

    /// Uniform in [0, 1) (xorshift32; plenty for dither).
    fn uniform(&mut self) -> f64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        (x >> 8) as f64 / (1u32 << 24) as f64
    }

    /// Requantizes `buf` (full scale ±1.0) to `bits`-bit integers, adding TPDF
    /// noise of ±1 LSB peak before rounding. Values are clipped to the integer range.
    pub fn quantize(&mut self, buf: &mut [f32], bits: u32) {
        let scale = (1u64 << (bits.clamp(2, 24) - 1)) as f64;
        let (lo, hi) = (-scale, scale - 1.0);
        for s in buf {
            let noise = self.uniform() - self.uniform();
            let q = (*s as f64 * scale + noise).round().clamp(lo, hi);
            *s = (q / scale) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: usize = 200_000;

    fn on_grid(s: f32, bits: u32) -> bool {
        let v = s as f64 * (1u64 << (bits - 1)) as f64;
        v == v.round()
    }

    #[test]
    fn output_is_on_the_integer_grid_and_clipped() {
        let mut d = Tpdf::default();
        let mut buf: Vec<f32> = (0..N).map(|i| ((i as f32) * 0.000_37).sin() * 1.2).collect();
        d.quantize(&mut buf, 16);
        assert!(buf.iter().all(|s| on_grid(*s, 16)));
        assert!(buf.iter().all(|s| (-1.0..=32_767.0 / 32_768.0).contains(s)));
    }

    #[test]
    fn error_is_unbiased_with_tpdf_variance() {
        // Total error of TPDF-dithered rounding: rounding (1/12 LSB²) + dither (1/6 LSB²) = 1/4 LSB².
        let mut d = Tpdf::new(12345);
        let lsb = 1.0 / 32_768.0;
        let src: Vec<f32> = (0..N).map(|i| ((i as f64 * 0.001_3).sin() * 0.5) as f32).collect();
        let mut buf = src.clone();
        d.quantize(&mut buf, 16);
        let err: Vec<f64> = buf.iter().zip(&src).map(|(q, s)| (*q as f64 - *s as f64) / lsb).collect();
        let mean = err.iter().sum::<f64>() / N as f64;
        let var = err.iter().map(|e| (e - mean).powi(2)).sum::<f64>() / N as f64;
        assert!(mean.abs() < 0.01, "mean error {mean} LSB");
        assert!((var - 0.25).abs() < 0.01, "error variance {var} LSB², expected 0.25");
        assert!(err.iter().all(|e| e.abs() <= 1.5 + 1e-9), "peak error stays within 1.5 LSB");
    }

    #[test]
    fn dither_keeps_signals_below_one_lsb() {
        // A sine at 0.4 LSB peak vanishes when plainly rounded, but survives dithering
        // as a correlated signal buried in noise.
        let lsb = 1.0 / 32_768.0;
        let src: Vec<f32> = (0..N).map(|i| ((i as f64 * 0.05).sin() * 0.4 * lsb) as f32).collect();
        let rounded: Vec<f32> = src.iter().map(|s| (*s as f64 / lsb).round() as f32).collect();
        assert!(rounded.iter().all(|s| *s == 0.0), "undithered rounding erases it");
        let mut buf = src.clone();
        Tpdf::new(7).quantize(&mut buf, 16);
        let dot: f64 = buf.iter().zip(&src).map(|(a, b)| *a as f64 * *b as f64).sum();
        let energy: f64 = src.iter().map(|s| (*s as f64).powi(2)).sum();
        let gain = dot / energy;
        assert!((gain - 1.0).abs() < 0.05, "dithered output carries the signal at unity gain ({gain})");
    }
}
