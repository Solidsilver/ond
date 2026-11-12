//! The parts of an artwork cache every app in the family shares: the sizes
//! asked of a source, the queue that runs what's on screen first (and drops
//! what was scrolled past), the least-recently-used set under a budget, the
//! disk cache's names and its eviction, the decode, and the small blurred
//! copy a cover can be drawn on.
//!
//! What isn't here is the part an app must do itself: asking its own source for
//! the bytes, and uploading the textures it decodes. Ond's `art.rs` and Rede's
//! are those halves, and they look alike because this is under both.

use egui::{Color32, ColorImage, Rect};
use std::cmp::Reverse;
use std::collections::HashMap;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Sizes requested from a source, in pixels. A request picks the smallest that
/// covers the drawn size at the screen's pixel density.
pub const SIZES: [u32; 4] = [64, 128, 256, 512];

/// Parallel downloads.
pub const FETCHES: usize = 6;
/// Parallel disk-cache reads (and their decodes).
pub const READS: usize = 4;
/// Frames a load may go unasked for before it's dropped.
pub const KEEP_FRAMES: u64 = 2;

pub fn bucket(px: f32) -> u32 {
    SIZES.iter().copied().find(|&s| s as f32 >= px).unwrap_or(SIZES[SIZES.len() - 1])
}

/// Least-recently-used set with a weight budget. Pure bookkeeping; the caller
/// owns the values and drops whatever `insert` reports as evicted.
#[derive(Debug)]
pub struct Lru<K> {
    entries: HashMap<K, (u64, usize)>,
    clock: u64,
    total: usize,
    budget: usize,
}

impl<K: Clone + Eq + Hash> Lru<K> {
    pub fn new(budget: usize) -> Self {
        Self { entries: HashMap::new(), clock: 0, total: 0, budget }
    }

    pub fn touch<Q: Hash + Eq + ?Sized>(&mut self, k: &Q)
    where
        K: std::borrow::Borrow<Q>,
    {
        self.clock += 1;
        if let Some(e) = self.entries.get_mut(k) {
            e.0 = self.clock;
        }
    }

    /// Adds `k`, then evicts the least recently used others until within budget.
    /// The new entry itself is never evicted, even if it alone is over budget.
    pub fn insert(&mut self, k: K, weight: usize) -> Vec<K> {
        self.clock += 1;
        if let Some((_, old)) = self.entries.insert(k.clone(), (self.clock, weight)) {
            self.total -= old;
        }
        self.total += weight;
        let mut evicted = Vec::new();
        while self.total > self.budget && self.entries.len() > 1 {
            let Some(oldest) = self.entries.iter().filter(|(key, _)| **key != k).min_by_key(|(_, (t, _))| *t).map(|(key, _)| key.clone()) else {
                break;
            };
            if let Some((_, w)) = self.entries.remove(&oldest) {
                self.total -= w;
            }
            evicted.push(oldest);
        }
        evicted
    }

    /// Keeps only the entries `keep` says to, and stops counting the rest.
    pub fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        let mut dropped = 0;
        self.entries.retain(|k, (_, w)| {
            let kept = keep(k);
            if !kept {
                dropped += *w;
            }
            kept
        });
        self.total -= dropped;
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(test)]
    pub fn total(&self) -> usize {
        self.total
    }
}

/// Which cached files to delete to get under `budget`: least recently used
/// (oldest modification time) first.
pub fn eviction_plan(mut files: Vec<(PathBuf, u64, SystemTime)>, budget: u64) -> Vec<PathBuf> {
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort_by_key(|f| f.2);
    let mut out = Vec::new();
    for (path, size, _) in files {
        if total <= budget {
            break;
        }
        total -= size;
        out.push(path);
    }
    out
}

/// The files in a cache folder: path, size and when last used.
pub fn cached_files(dir: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok().filter(|m| m.is_file())?;
            Some((e.path(), m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
        })
        .collect()
}

/// Deletes the least recently used art until the folder is within `budget`.
pub fn trim_disk_cache(dir: &Path, budget: u64) {
    for path in eviction_plan(cached_files(dir), budget) {
        let _ = std::fs::remove_file(path);
    }
}

/// Where a cache keeps `cover` at `size` px.
pub fn cover_file(dir: &Path, cover: &str, size: u32) -> PathBuf {
    let safe: String = cover.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    dir.join(format!("{safe}-{size}"))
}

/// Decodes an image file to RGBA, scaled down to about `size`.
pub fn decode(bytes: &[u8], size: u32) -> Option<ColorImage> {
    let img = image::load_from_memory(bytes).ok()?;
    // The server scales to `size`, but some covers come back full size.
    let img = if img.width() > size * 3 / 2 || img.height() > size * 3 / 2 { img.thumbnail(size, size) } else { img };
    let rgba = img.to_rgba8();
    Some(ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

/// Where a load runs: the disk cache first, then the source on a miss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Disk,
    Net,
}

impl Lane {
    fn slots(self) -> usize {
        match self {
            Lane::Disk => READS,
            Lane::Net => FETCHES,
        }
    }
}

#[derive(Debug)]
struct Want {
    /// The last frame it was asked for.
    frame: u64,
    /// Order of first asking; also the job's id.
    job: u64,
    /// Only prefetched (just past the viewport) in its last frame.
    soon: bool,
    lane: Lane,
    running: bool,
}

/// Loads asked for and which run next: asked for most recently first, on
/// screen before prefetch, then longest waiting. Pure bookkeeping; the caller
/// starts the tasks and aborts the downloads `next_frame` reports.
#[derive(Debug)]
pub struct Queue<K> {
    wants: HashMap<K, Want>,
    frame: u64,
    jobs: u64,
}

impl<K: Clone + Eq + Hash> Queue<K> {
    pub fn new() -> Self {
        Self { wants: HashMap::new(), frame: 0, jobs: 0 }
    }

    /// Notes `k` as wanted this frame (`soon`: only as a prefetch). True when
    /// it wasn't already queued or running.
    pub fn ask(&mut self, k: K, soon: bool) -> bool {
        if let Some(w) = self.wants.get_mut(&k) {
            w.soon = if w.frame == self.frame { w.soon && soon } else { soon };
            w.frame = self.frame;
            return false;
        }
        self.jobs += 1;
        self.wants.insert(k, Want { frame: self.frame, job: self.jobs, soon, lane: Lane::Disk, running: false });
        true
    }

    /// Starts a frame, dropping loads unasked for [`KEEP_FRAMES`] frames, and
    /// returns the running downloads among them to abort. Running disk reads
    /// are left to finish; they're quick.
    pub fn next_frame(&mut self) -> Vec<K> {
        self.frame += 1;
        let frame = self.frame;
        let mut abort = Vec::new();
        self.wants.retain(|k, w| {
            let keep = w.frame + KEEP_FRAMES >= frame || (w.running && w.lane == Lane::Disk);
            if !keep && w.running {
                abort.push(k.clone());
            }
            keep
        });
        abort
    }

    /// The next loads to run, best first, as many as their lanes have free
    /// slots, now marked running.
    pub fn start(&mut self) -> Vec<(K, u64, Lane)> {
        let busy = |l: Lane| self.wants.values().filter(|w| w.running && w.lane == l).count();
        let mut free = [Lane::Disk, Lane::Net].map(|l| l.slots().saturating_sub(busy(l)));
        let mut ready: Vec<_> = self.wants.iter().filter(|(_, w)| !w.running).collect();
        ready.sort_by_key(|(_, w)| (Reverse(w.frame), w.soon, w.job));
        let mut out = Vec::new();
        for (k, w) in ready {
            let n = &mut free[w.lane as usize];
            if *n > 0 {
                *n -= 1;
                out.push((k.clone(), w.job, w.lane));
            }
        }
        for (k, _, _) in &out {
            if let Some(w) = self.wants.get_mut(k) {
                w.running = true;
            }
        }
        out
    }

    /// Job `job` found nothing on disk: `k` queues for a download.
    pub fn missed(&mut self, k: &K, job: u64) {
        if let Some(w) = self.wants.get_mut(k)
            && w.job == job
        {
            w.lane = Lane::Net;
            w.running = false;
        }
    }

    /// Job `job` finished, with art or without.
    pub fn done(&mut self, k: &K, job: u64) {
        if self.wants.get(k).is_some_and(|w| w.job == job) {
            self.wants.remove(k);
        }
    }

    /// Drops every load. Job numbers carry on, so reports from loads still
    /// running match nothing.
    pub fn clear(&mut self) {
        self.wants.clear();
    }

    #[cfg(test)]
    pub fn contains(&self, k: &K) -> bool {
        self.wants.contains_key(k)
    }
}

impl<K: Clone + Eq + Hash> Default for Queue<K> {
    fn default() -> Self {
        Self::new()
    }
}

/// The part of a square image that fills a rect of this aspect (width / height), centred.
pub fn crop_uv(aspect: f32) -> Rect {
    if aspect >= 1.0 {
        let h = 1.0 / aspect;
        Rect::from_min_max(egui::pos2(0.0, (1.0 - h) / 2.0), egui::pos2(1.0, (1.0 + h) / 2.0))
    } else {
        let w = aspect;
        Rect::from_min_max(egui::pos2((1.0 - w) / 2.0, 0.0), egui::pos2((1.0 + w) / 2.0, 1.0))
    }
}

/// The blurred copy's side, in pixels: the blur hides any lack of detail.
const SIDE: usize = 48;
/// Box blur radius in pixels (of [`SIDE`]); three passes make it near-Gaussian.
const RADIUS: usize = 4;

/// The blurred copy of `image` and its darkest and brightest colors: Ond's
/// backdrop behind the Player and Mini, and Rede's ground behind a cover
/// that doesn't fill its square.
pub fn blurred(image: &ColorImage) -> Option<(ColorImage, [Color32; 2])> {
    let [w, h] = image.size;
    if w == 0 || h == 0 {
        return None;
    }
    // Downscale by averaging into SIDE × SIDE cells.
    let mut px = vec![[0f32; 3]; SIDE * SIDE];
    let mut counts = vec![0f32; SIDE * SIDE];
    for y in 0..h {
        for x in 0..w {
            let cell = (y * SIDE / h) * SIDE + x * SIDE / w;
            let c = image.pixels[y * w + x];
            for (p, v) in px[cell].iter_mut().zip([c.r(), c.g(), c.b()]) {
                *p += f32::from(v);
            }
            counts[cell] += 1.0;
        }
    }
    for (p, n) in px.iter_mut().zip(&counts) {
        for v in p.iter_mut() {
            *v /= n.max(1.0);
        }
    }
    for _ in 0..3 {
        box_pass(&mut px, true);
        box_pass(&mut px, false);
    }
    let pixels: Vec<Color32> = px.iter().map(|p| Color32::from_rgb(p[0].round() as u8, p[1].round() as u8, p[2].round() as u8)).collect();
    let lum = |c: &&Color32| c.intensity();
    let darkest = *pixels.iter().min_by(|a, b| lum(a).total_cmp(&lum(b)))?;
    let brightest = *pixels.iter().max_by(|a, b| lum(a).total_cmp(&lum(b)))?;
    Some((ColorImage::new([SIDE, SIDE], pixels), [darkest, brightest]))
}

/// One horizontal (or vertical) box blur pass, edges clamped.
fn box_pass(px: &mut [[f32; 3]], horizontal: bool) {
    let src = px.to_vec();
    let at = |line: usize, i: usize| if horizontal { line * SIDE + i } else { i * SIDE + line };
    let n = (2 * RADIUS + 1) as f32;
    for line in 0..SIDE {
        for i in 0..SIDE {
            let mut sum = [0f32; 3];
            for k in 0..=2 * RADIUS {
                let j = (i + k).saturating_sub(RADIUS).min(SIDE - 1);
                for (s, v) in sum.iter_mut().zip(src[at(line, j)]) {
                    *s += v;
                }
            }
            px[at(line, i)] = sum.map(|s| s / n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_blur_is_small_smooth_and_keeps_the_colors() {
        // Left half red, right half blue: a hard edge.
        let n = 128;
        let img =
            ColorImage::new([n, n], (0..n * n).map(|i| if i % n < n / 2 { Color32::from_rgb(200, 20, 20) } else { Color32::from_rgb(20, 20, 200) }).collect());
        let (b, [dark, bright]) = blurred(&img).unwrap();
        assert_eq!(b.size, [SIDE, SIDE]);
        let row = &b.pixels[SIDE * (SIDE / 2)..SIDE * (SIDE / 2 + 1)];
        // Far from the edge the colors are as they were; across it they blend.
        assert!(row[0].r() > 190 && row[SIDE - 1].b() > 190);
        let mid = row[SIDE / 2];
        assert!(mid.r() > 60 && mid.b() > 60, "the edge is soft: {mid:?}");
        // No step between neighbours is anywhere near the original jump.
        assert!(row.windows(2).all(|w| (i32::from(w[0].r()) - i32::from(w[1].r())).abs() < 40));
        assert!(dark.intensity() <= bright.intensity());
        assert!(blurred(&ColorImage::new([0, 0], Vec::new())).is_none());
    }

    #[test]
    fn a_size_rounds_up_to_a_bucket_and_caps_at_the_largest() {
        assert_eq!(bucket(10.0), 64);
        assert_eq!(bucket(64.0), 64);
        assert_eq!(bucket(65.0), 128);
        assert_eq!(bucket(336.0), 512);
        assert_eq!(bucket(4000.0), 512);
    }

    #[test]
    fn lru_retain_drops_entries_and_their_weight() {
        let mut l = Lru::new(100);
        l.insert("a", 30);
        l.insert("b", 30);
        l.insert("c", 30);
        l.retain(|k| *k != "b");
        assert_eq!((l.len(), l.total()), (2, 60));
        // Room again for what the dropped entry took: nothing is evicted.
        assert!(l.insert("d", 40).is_empty());
        assert_eq!(l.total(), 100);
    }

    #[test]
    fn lru_evicts_least_recent_within_budget() {
        let mut l = Lru::new(100);
        assert!(l.insert("a", 40).is_empty());
        assert!(l.insert("b", 40).is_empty());
        l.touch(&"a");
        // "b" is now the least recently used.
        assert_eq!(l.insert("c", 40), ["b"]);
        assert_eq!((l.len(), l.total()), (2, 80));
        // A single oversized entry evicts everything else but stays itself.
        let mut gone = l.insert("big", 500);
        gone.sort();
        assert_eq!(gone, ["a", "c"]);
        assert_eq!((l.len(), l.total()), (1, 500));
        // Re-inserting replaces the weight instead of adding to it.
        let mut l = Lru::new(100);
        l.insert("a", 60);
        assert!(l.insert("a", 70).is_empty());
        assert_eq!(l.total(), 70);
    }

    /// Starts what's ready and returns just the keys, in order.
    fn started<K: Clone + Eq + Hash>(q: &mut Queue<K>) -> Vec<K> {
        q.start().into_iter().map(|(k, _, _)| k).collect()
    }

    #[test]
    fn queue_runs_what_was_asked_last() {
        let mut q = Queue::new();
        for k in ["a", "b", "c", "d", "e", "f"] {
            assert!(q.ask(k, false));
        }
        q.next_frame();
        // Scrolled on: "g" is new, "a".."f" were last drawn a frame ago.
        q.ask("g", false);
        assert_eq!(started(&mut q), ["g", "a", "b", "c"]);
        // All disk slots are busy.
        assert!(started(&mut q).is_empty());
        // Unasked for `KEEP_FRAMES` frames, the queued ones go; running reads finish.
        for _ in 1..KEEP_FRAMES {
            assert!(q.next_frame().is_empty());
        }
        assert!(q.contains(&"d"));
        assert!(q.next_frame().is_empty());
        assert!(!q.contains(&"d") && q.contains(&"a"));
        // Drawn again, a dropped one is asked for afresh.
        assert!(q.ask("d", false));
    }

    #[test]
    fn queue_keeps_what_is_still_drawn() {
        let mut q = Queue::new();
        q.ask("a", false);
        for _ in 0..10 {
            q.next_frame();
            assert!(!q.ask("a", false));
        }
        assert!(q.contains(&"a"));
    }

    #[test]
    fn queue_aborts_scrolled_past_downloads() {
        let mut q = Queue::new();
        q.ask("a", false);
        let [(_, job, Lane::Disk)] = q.start()[..] else { panic!() };
        // Not on disk: it goes to the network lane.
        q.missed(&"a", job);
        assert_eq!(q.start(), [("a", job, Lane::Net)]);
        for _ in 0..KEEP_FRAMES {
            assert!(q.next_frame().is_empty());
        }
        assert_eq!(q.next_frame(), ["a"]);
        assert!(q.ask("a", false));
    }

    #[test]
    fn queue_lanes_have_their_own_slots() {
        let mut q = Queue::new();
        for i in 0..=FETCHES {
            q.ask(i, false);
        }
        // Every read misses and waits for a download.
        loop {
            let reads: Vec<_> = q.start().into_iter().filter(|r| r.2 == Lane::Disk).collect();
            if reads.is_empty() {
                break;
            }
            for (k, job, _) in reads {
                q.missed(&k, job);
            }
        }
        // Downloads are full, one waiting...
        assert!(q.start().is_empty());
        // ...yet a disk read starts at once.
        q.ask(100, false);
        assert_eq!(started(&mut q), [100]);
    }

    #[test]
    fn queue_puts_prefetch_after_the_screen() {
        let mut q = Queue::new();
        q.ask("ahead", true);
        q.ask("seen", false);
        q.ask("both", true);
        q.ask("both", false);
        assert_eq!(started(&mut q), ["seen", "both", "ahead"]);
    }

    #[test]
    fn queue_ignores_reports_from_old_jobs() {
        let mut q = Queue::new();
        q.ask("a", false);
        let [(_, old, _)] = q.start()[..] else { panic!() };
        q.missed(&"a", old);
        q.start();
        for _ in 0..=KEEP_FRAMES {
            q.next_frame();
        }
        // Dropped and asked for again; the old job's late report changes nothing.
        q.ask("a", false);
        q.done(&"a", old);
        q.missed(&"a", old);
        assert!(q.contains(&"a"));
        assert_eq!(q.start()[0].2, Lane::Disk);
    }

    #[test]
    fn disk_plan_removes_oldest_first() {
        let t0 = SystemTime::UNIX_EPOCH;
        let f = |name: &str, size: u64, age: u64| (PathBuf::from(name), size, t0 + Duration::from_secs(age));
        let plan = eviction_plan(vec![f("new", 50, 30), f("old", 50, 10), f("mid", 50, 20)], 100);
        assert_eq!(plan, [PathBuf::from("old")]);
        assert!(eviction_plan(vec![f("a", 10, 1)], 100).is_empty());
        assert_eq!(eviction_plan(vec![f("a", 10, 2), f("b", 10, 1)], 0).len(), 2);
    }

    #[test]
    fn cover_file_names_are_safe() {
        let p = cover_file(Path::new("/c"), "al-1/../x?y", 256);
        assert_eq!(p, PathBuf::from("/c/al-1____x_y-256"));
        assert!(!p.to_string_lossy().contains(".."), "the name is a file, not a path");
    }

    #[test]
    fn decodes_and_downscales_oversized_art() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(600, 600, image::Rgba([10, 20, 30, 255])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let img = decode(&png, 128).unwrap();
        assert_eq!(img.size, [128, 128]);
        assert!(decode(b"not an image", 64).is_none());
    }

    #[test]
    fn a_cover_is_cropped_to_fill_a_square() {
        assert_eq!(crop_uv(1.0), Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)));
        // Wider than tall: a band across the middle.
        assert_eq!(crop_uv(2.0), Rect::from_min_max(egui::pos2(0.0, 0.25), egui::pos2(1.0, 0.75)));
        assert_eq!(crop_uv(0.5), Rect::from_min_max(egui::pos2(0.25, 0.0), egui::pos2(0.75, 1.0)));
    }
}
