//! Waveforms for the seek bar: a track's peaks, read from its downloaded or
//! fully cached file on a background thread once per track, kept for the few
//! tracks played lately. Until they're ready the seek bar stays plain.

use crate::art::Lru;
use crate::memo::Memo;
use analysis::Peaks;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

/// Tracks whose peaks are kept (a few KB each).
const KEPT: usize = 32;
/// How often to look again for a playing track's file while it downloads.
const LOOK_EVERY: Duration = Duration::from_secs(1);

type Done = (String, Option<Peaks>);

/// How the waveform's bars look (Settings › Experimental): their ends, and
/// how thick and how far apart, in half points (each rounds to whole pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bars {
    pub rounded: bool,
    pub width: u8,
    pub gap: u8,
}

impl Bars {
    pub const WIDTHS: std::ops::RangeInclusive<u8> = 2..=8;
    pub const GAPS: std::ops::RangeInclusive<u8> = 1..=6;

    /// A bar's width and the gap after it, in points, kept in range.
    pub fn width_pt(self) -> f32 {
        f32::from(self.width.clamp(*Self::WIDTHS.start(), *Self::WIDTHS.end())) / 2.0
    }

    pub fn gap_pt(self) -> f32 {
        f32::from(self.gap.clamp(*Self::GAPS.start(), *Self::GAPS.end())) / 2.0
    }
}

impl Default for Bars {
    /// 2 pt bars, 1 pt apart, square.
    fn default() -> Self {
        Self { rounded: false, width: 4, gap: 2 }
    }
}

/// A made-up waveform of `n` bars (for previewing the bars' look): swells
/// and dips with some grain, the same every time.
pub fn sample(n: usize) -> Vec<f32> {
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    (0..n)
        .map(|i| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let t = i as f32 / n.max(1) as f32;
            let swell = 0.55 + 0.3 * (t * 9.0).sin() * (t * 2.3 + 0.4).sin();
            let fade = (t * 12.0).min(1.0) * ((1.0 - t) * 10.0).min(1.0);
            (swell * fade * (0.75 + 0.25 * (seed % 1000) as f32 / 1000.0)).clamp(0.0, 1.0)
        })
        .collect()
}

pub struct Waveforms {
    ctx: egui::Context,
    peaks: HashMap<String, Arc<Peaks>>,
    lru: Lru<String>,
    /// Being read now.
    pending: HashSet<String>,
    /// Couldn't be read; not tried again this session.
    failed: HashSet<String>,
    /// The track last looked for a file for, and when.
    looked: Option<(String, Instant)>,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    /// The last bars handed out, by track and count.
    bars: Memo<(String, usize), Vec<f32>>,
}

impl Waveforms {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, rx) = channel();
        Self { ctx, peaks: HashMap::new(), lru: Lru::new(KEPT), pending: HashSet::new(), failed: HashSet::new(), looked: None, tx, rx, bars: Memo::default() }
    }

    /// Takes in finished reads. Call once per frame before drawing.
    pub fn poll(&mut self) {
        while let Ok((id, peaks)) = self.rx.try_recv() {
            self.pending.remove(&id);
            match peaks {
                Some(p) => self.insert(id, p),
                None => {
                    self.failed.insert(id);
                }
            }
        }
    }

    pub fn insert(&mut self, id: String, peaks: Peaks) {
        for gone in self.lru.insert(id.clone(), 1) {
            self.peaks.remove(&gone);
        }
        self.peaks.insert(id, Arc::new(peaks));
    }

    /// Makes sure track `id`'s peaks are on their way: once `file` finds a
    /// complete copy (asked at most once a second), it's read in the background.
    pub fn want(&mut self, id: &str, suffix: Option<&str>, file: impl FnOnce() -> Option<PathBuf>) {
        if self.peaks.contains_key(id) || self.pending.contains(id) || self.failed.contains(id) {
            return;
        }
        if self.looked.as_ref().is_some_and(|(l, at)| l == id && at.elapsed() < LOOK_EVERY) {
            return;
        }
        self.looked = Some((id.to_owned(), Instant::now()));
        let Some(path) = file() else { return };
        self.pending.insert(id.to_owned());
        let (tx, ctx, job, suffix) = (self.tx.clone(), self.ctx.clone(), id.to_owned(), suffix.map(str::to_owned));
        let spawned = std::thread::Builder::new().name("waveform".into()).spawn(move || {
            crate::analyze::lower_priority();
            let peaks = read(&path, suffix.as_deref());
            if peaks.is_none() {
                tracing::debug!("no waveform for {}", path.display());
            }
            let _ = tx.send((job, peaks));
            ctx.request_repaint();
        });
        if spawned.is_err() {
            self.pending.remove(id);
        }
    }

    /// Track `id` as `n` bars (0..=1), if its peaks are ready.
    pub fn bars(&mut self, id: &str, n: usize) -> Option<Arc<Vec<f32>>> {
        let peaks = self.peaks.get(id)?;
        self.lru.touch(&id.to_owned());
        Some(self.bars.get((id.to_owned(), n), || peaks.bars(n)))
    }
}

fn read(path: &std::path::Path, suffix: Option<&str>) -> Option<Peaks> {
    let file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok().map(|m| m.len());
    analysis::peaks_reader(file, len, suffix, &|| false).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One second of a sine that fades in, as a 16-bit mono WAV.
    fn wav() -> Vec<u8> {
        let rate = 8000u32;
        let samples: Vec<i16> = (0..rate).map(|i| ((i as f32 * 0.3).sin() * 30_000.0 * i as f32 / rate as f32) as i16).collect();
        let data = (samples.len() * 2) as u32;
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        for v in [16u32.to_le_bytes().as_slice(), &1u16.to_le_bytes(), &1u16.to_le_bytes(), &rate.to_le_bytes(), &(rate * 2).to_le_bytes(), &2u16.to_le_bytes(), &16u16.to_le_bytes()] {
            w.extend_from_slice(v);
        }
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data.to_le_bytes());
        for s in samples {
            w.extend_from_slice(&s.to_le_bytes());
        }
        w
    }

    #[test]
    fn peaks_are_read_in_the_background_once_the_file_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.wav");
        let mut w = Waveforms::new(egui::Context::default());
        // Still downloading: nothing to read yet, and not asked again at once.
        w.want("t", Some("wav"), || None);
        let mut asked = false;
        w.want("t", Some("wav"), || {
            asked = true;
            None
        });
        assert!(!asked, "looked again within the second");
        assert!(w.bars("t", 10).is_none());

        std::fs::write(&path, wav()).unwrap();
        w.looked = None;
        w.want("t", Some("wav"), || Some(path.clone()));
        let deadline = Instant::now() + Duration::from_secs(5);
        let bars = loop {
            w.poll();
            if let Some(b) = w.bars("t", 10) {
                break b;
            }
            assert!(Instant::now() < deadline, "the waveform never arrived");
            std::thread::sleep(Duration::from_millis(5));
        };
        // A fade-in: quiet at the start, loudest at the end.
        assert_eq!(bars.len(), 10);
        assert!(bars[0] < 0.2 && bars[9] == 1.0, "{bars:?}");
        assert!(bars.windows(2).all(|p| p[1] >= p[0] - 0.05));
        // Asked for again: the same bars, not rebuilt.
        assert!(Arc::ptr_eq(&bars, &w.bars("t", 10).unwrap()));
    }

    #[test]
    fn a_file_that_will_not_read_is_not_tried_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.flac");
        std::fs::write(&path, b"not audio").unwrap();
        let mut w = Waveforms::new(egui::Context::default());
        w.want("bad", Some("flac"), || Some(path.clone()));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !w.failed.contains("bad") {
            w.poll();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        w.looked = None;
        w.want("bad", Some("flac"), || panic!("tried again"));
    }
}
