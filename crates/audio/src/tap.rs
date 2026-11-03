//! A copy of what's playing, for visuals (a spectrum, a visualizer). The
//! output callback writes each block it plays, mixed down to mono, into a
//! fixed ring of atomics; the UI reads the latest stretch whenever it draws.
//!
//! The callback side is realtime-safe: the ring is allocated once, up front,
//! and a write is relaxed atomic stores and one release store, no locks and
//! no allocation. It reads the samples the callback already has and never
//! touches what goes to the device. It only runs while something is reading
//! ([`Tap::set_active`]); otherwise it costs one atomic load per callback.
//!
//! The samples are as decoded, before volume and ReplayGain, so a visual
//! looks the same at any volume. A reader racing the writer may see a few
//! samples of the next block at the oldest end of what it copies: harmless
//! for something to look at, and the price of never blocking the callback.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Mono frames kept: 0.37 s at 44.1 kHz, 85 ms at 192 kHz.
pub const TAP_FRAMES: usize = 16_384;
const MASK: u64 = TAP_FRAMES as u64 - 1;

pub struct Tap {
    /// f32 bits, indexed by frame number modulo [`TAP_FRAMES`].
    samples: Box<[AtomicU32]>,
    /// Frames written since the start (the next one goes at this index).
    written: AtomicU64,
    /// The rate of what was last written.
    rate: AtomicU32,
    active: AtomicBool,
}

impl Default for Tap {
    fn default() -> Self {
        Self {
            samples: (0..TAP_FRAMES).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicU64::new(0),
            rate: AtomicU32::new(0),
            active: AtomicBool::new(false),
        }
    }
}

impl std::fmt::Debug for Tap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tap").field("written", &self.written.load(Ordering::Relaxed)).field("active", &self.active()).finish_non_exhaustive()
    }
}

impl Tap {
    /// Starts or stops copying. Off, the callback skips the tap entirely.
    pub fn set_active(&self, on: bool) {
        self.active.store(on, Ordering::Relaxed);
    }

    pub fn active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    /// Realtime: appends one block of interleaved audio (handed over in two
    /// parts, as the ring buffer splits it), mixed down to mono. `a` and `b`
    /// together hold whole frames. The output callback calls this; it's
    /// public so previews and tests can stand in for it.
    pub fn write(&self, a: &[f32], b: &[f32], channels: usize, rate: u32) {
        let channels = channels.max(1);
        let scale = 1.0 / channels as f32;
        let mut at = self.written.load(Ordering::Relaxed);
        let (mut sum, mut n) = (0.0f32, 0);
        for &s in a.iter().chain(b) {
            sum += s;
            n += 1;
            if n == channels {
                self.samples[(at & MASK) as usize].store((sum * scale).to_bits(), Ordering::Relaxed);
                at += 1;
                sum = 0.0;
                n = 0;
            }
        }
        self.rate.store(rate, Ordering::Relaxed);
        self.written.store(at, Ordering::Release);
    }

    /// Copies the latest `out.len()` frames (at most [`TAP_FRAMES`]), oldest
    /// first; frames never written read as silence. Returns how many frames
    /// have been written in all (unchanged since the last read: nothing new
    /// played) and their sample rate.
    pub fn read(&self, out: &mut [f32]) -> (u64, u32) {
        let written = self.written.load(Ordering::Acquire);
        let rate = self.rate.load(Ordering::Relaxed);
        let n = out.len().min(TAP_FRAMES);
        let first = written as i64 - n as i64;
        for (i, o) in out[..n].iter_mut().enumerate() {
            let frame = first + i as i64;
            *o = if frame < 0 { 0.0 } else { f32::from_bits(self.samples[(frame as u64 & MASK) as usize].load(Ordering::Relaxed)) };
        }
        out[n..].fill(0.0);
        (written, rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_mono_and_reads_the_latest_oldest_first() {
        let tap = Tap::default();
        let mut out = [9.0f32; 4];
        assert_eq!(tap.read(&mut out), (0, 0));
        assert_eq!(out, [0.0; 4], "nothing written reads as silence");
        // Stereo frames (0.25, 0.75), (0.5, 1.0), (1.0, -1.0), split mid-frame across the two parts.
        tap.write(&[0.25, 0.75, 0.5], &[1.0, 1.0, -1.0], 2, 48_000);
        assert_eq!(tap.read(&mut out), (3, 48_000));
        assert_eq!(out, [0.0, 0.5, 0.75, 0.0]);
        let mut two = [0.0f32; 2];
        tap.read(&mut two);
        assert_eq!(two, [0.75, 0.0]);
    }

    #[test]
    fn the_ring_wraps_and_keeps_the_newest() {
        let tap = Tap::default();
        let block: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        // Several times round the ring, in blocks that don't divide it.
        let mut next = 0.0;
        for _ in 0..(3 * TAP_FRAMES / 1000 + 7) {
            let b: Vec<f32> = block.iter().map(|x| x + next).collect();
            tap.write(&b, &[], 1, 44_100);
            next += 1000.0;
        }
        let mut out = vec![0.0f32; TAP_FRAMES + 10];
        let (written, _) = tap.read(&mut out);
        assert_eq!(written as f32, next);
        // The whole ring, in order, ending on the last frame written; asking for more than it holds gives the ring.
        assert!(out[..TAP_FRAMES].windows(2).all(|w| w[1] == w[0] + 1.0));
        assert_eq!(out[TAP_FRAMES - 1], next - 1.0);
        assert!(out[TAP_FRAMES..].iter().all(|&s| s == 0.0));
    }

    #[test]
    fn it_starts_off() {
        let tap = Tap::default();
        assert!(!tap.active());
        tap.set_active(true);
        assert!(tap.active());
    }
}
