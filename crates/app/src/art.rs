//! Cover art: fetched from the server at a few fixed sizes, cached on disk,
//! decoded off the UI thread, and kept as GPU textures under a byte budget.
//!
//! Callers ask for art only while drawing something visible, so scrolled-away
//! rows never cost a request. A miss paints a neutral tile of the same size
//! (no layout shift) and the finished load triggers a single repaint.
//!
//! Loads are asked for again every frame they're drawn, and the [`Queue`] runs
//! the most recently asked first. One that goes unasked for a couple of frames
//! (scrolled past) is dropped before it starts, or its download aborted, so a
//! fast scroll doesn't leave a backlog ahead of what's on screen now. Disk
//! reads have their own slots and never wait behind downloads. Looks (for the
//! playing and the open album, a cover or two) skip the queue and start at once.

use egui::{Color32, ColorImage, CornerRadius, Rect, TextureHandle, TextureOptions, Ui};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::SystemTime;
use subsonic::Client;

/// Sizes requested from the server, in pixels. A request picks the smallest
/// that covers the drawn size at the screen's pixel density.
pub const SIZES: [u32; 4] = [64, 128, 256, 512];
/// GPU budget for decoded art.
const TEXTURE_BUDGET: usize = 192 * 1024 * 1024;
/// Disk budget for downloaded art.
const DISK_BUDGET: u64 = 512 * 1024 * 1024;
/// Parallel downloads.
const FETCHES: usize = 6;
/// Parallel disk-cache reads (and their decodes).
const READS: usize = 4;
/// Frames a load may go unasked for before it's dropped.
const KEEP_FRAMES: u64 = 2;

pub fn bucket(px: f32) -> u32 {
    SIZES.iter().copied().find(|&s| s as f32 >= px).unwrap_or(SIZES[SIZES.len() - 1])
}

/// A cover at one size. Covers go by their number in [`Art::covers`], so
/// finding a texture while drawing copies no string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    cover: u32,
    size: u32,
}

/// Textures uploaded per frame at most: a screenful of covers arriving at
/// once spreads over a few frames instead of stalling one.
const UPLOADS_PER_FRAME: usize = 8;

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
            let Some(oldest) = self.entries.iter().filter(|(key, _)| **key != k).min_by_key(|(_, (t, _))| *t).map(|(key, _)| key.clone())
            else {
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

/// Where a load runs: the disk cache first, then the server on a miss.
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

    /// Starts a frame, dropping loads unasked for `KEEP_FRAMES` frames, and
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

    #[cfg(test)]
    pub fn contains(&self, k: &K) -> bool {
        self.wants.contains_key(k)
    }
}

/// What a load reports: the art (`None`: there's none to be had), or that the
/// disk cache missed and it needs a download.
enum Outcome {
    Image(Option<ColorImage>),
    Miss,
}

/// A finished job: its key, its number in the queue and what it found.
type Loaded = (Key, u64, Outcome);

/// Covers whose [`Look`] is kept: enough that going back to a few album
/// pages finds their colors at once.
const LOOKS: usize = 64;
/// The size a cover is read at for its look: plenty for a palette.
const LOOK_SIZE: u32 = 128;

/// What the colors and visuals take from a cover, worked out once per cover
/// off the UI thread.
#[derive(Default)]
pub struct Look {
    /// `None` for a cover with too little color (or none at all).
    pub palette: Option<crate::visuals::colors::ArtPalette>,
    /// A small blurred copy of the cover, for the backdrop and for seeing
    /// what lies under controls drawn over the art.
    pub backdrop: Option<crate::visuals::backdrop::Blurred>,
}

/// A look as worked out off the UI thread, before its texture is made.
#[derive(Default)]
struct LookParts {
    palette: Option<crate::visuals::colors::ArtPalette>,
    blurred: Option<(ColorImage, [Color32; 2])>,
}

impl LookParts {
    fn of(image: Option<&ColorImage>) -> Self {
        Self { palette: image.and_then(crate::visuals::colors::extract), blurred: image.and_then(crate::visuals::backdrop::blurred) }
    }
}

pub struct Art {
    rt: tokio::runtime::Handle,
    ctx: egui::Context,
    dir: Arc<PathBuf>,
    client: Option<Client>,
    /// Every cover id asked for, numbered (see [`Key`]), and the ids by
    /// number; the two share each id.
    covers: HashMap<Arc<str>, u32>,
    names: Vec<Arc<str>>,
    textures: HashMap<Key, TextureHandle>,
    lru: Lru<Key>,
    queue: Queue<Key>,
    /// Running downloads, by job, so a scrolled-past one can be aborted.
    downloads: HashMap<Key, (u64, tokio::task::AbortHandle)>,
    /// Art the server doesn't have; not asked for again this session.
    missing: HashSet<Key>,
    tx: Sender<Loaded>,
    rx: Receiver<Loaded>,
    /// Looks by cover id, for the few covers lately asked about.
    looks: HashMap<String, Arc<Look>>,
    looks_lru: Lru<String>,
    looks_pending: HashSet<String>,
    look_tx: Sender<(String, LookParts)>,
    look_rx: Receiver<(String, LookParts)>,
    /// Images given by [`Art::preload`], so looks work without a server.
    #[cfg(test)]
    preloaded: HashMap<String, ColorImage>,
}

impl Art {
    pub fn new(rt: tokio::runtime::Handle, ctx: egui::Context, dir: PathBuf) -> Self {
        let (tx, rx) = channel();
        let (look_tx, look_rx) = channel();
        let dir = Arc::new(dir);
        let trim = dir.clone();
        rt.spawn_blocking(move || trim_disk_cache(&trim));
        Self {
            rt,
            ctx,
            dir,
            client: None,
            covers: HashMap::new(),
            names: Vec::new(),
            textures: HashMap::new(),
            lru: Lru::new(TEXTURE_BUDGET),
            queue: Queue::new(),
            downloads: HashMap::new(),
            missing: HashSet::new(),
            tx,
            rx,
            looks: HashMap::new(),
            looks_lru: Lru::new(LOOKS),
            looks_pending: HashSet::new(),
            look_tx,
            look_rx,
            #[cfg(test)]
            preloaded: HashMap::new(),
        }
    }

    pub fn set_client(&mut self, client: Client) {
        self.client = Some(client);
        // Failures may have been "not connected yet".
        self.missing.clear();
        self.looks.retain(|_, l| l.palette.is_some());
        // The dropped looks leave the LRU too, or they'd count against its
        // budget and evict real ones.
        self.looks_lru.retain(|cover| self.looks.contains_key(cover));
    }

    /// `cover` at `size`, numbering the cover the first time it's seen.
    fn key(&mut self, cover: &str, size: u32) -> Key {
        let cover = match self.covers.get(cover) {
            Some(&n) => n,
            None => {
                let (n, name) = (self.names.len() as u32, Arc::<str>::from(cover));
                self.covers.insert(name.clone(), n);
                self.names.push(name);
                n
            }
        };
        Key { cover, size }
    }

    /// Uploads finished loads (a few per frame), drops what's no longer drawn
    /// and starts what's next. Call once per frame before drawing.
    pub fn poll(&mut self) {
        let mut uploads = 0;
        while uploads < UPLOADS_PER_FRAME
            && let Ok((key, job, outcome)) = self.rx.try_recv()
        {
            if self.downloads.get(&key).is_some_and(|d| d.0 == job) {
                self.downloads.remove(&key);
            }
            let image = match outcome {
                Outcome::Miss => {
                    self.queue.missed(&key, job);
                    continue;
                }
                Outcome::Image(image) => {
                    self.queue.done(&key, job);
                    image
                }
            };
            let Some(image) = image else {
                self.missing.insert(key);
                continue;
            };
            uploads += 1;
            if uploads == UPLOADS_PER_FRAME {
                // There may be more waiting: the next frame takes them.
                self.ctx.request_repaint();
            }
            let weight = image.width() * image.height() * 4;
            let tex = self.ctx.load_texture(format!("art:{}:{}", self.names[key.cover as usize], key.size), image, TextureOptions::LINEAR);
            for gone in self.lru.insert(key, weight) {
                self.textures.remove(&gone);
            }
            self.textures.insert(key, tex);
        }
        while let Ok((cover, parts)) = self.look_rx.try_recv() {
            self.looks_pending.remove(&cover);
            let backdrop = parts.blurred.map(|(image, extremes)| crate::visuals::backdrop::Blurred {
                texture: self.ctx.load_texture(format!("backdrop:{cover}"), image.clone(), TextureOptions::LINEAR),
                image: Arc::new(image),
                extremes,
            });
            let look = Look { palette: parts.palette, backdrop };
            for gone in self.looks_lru.insert(cover.clone(), 1) {
                self.looks.remove(&gone);
            }
            self.looks.insert(cover, Arc::new(look));
        }
        for key in self.queue.next_frame() {
            if let Some((_, task)) = self.downloads.remove(&key) {
                task.abort();
            }
        }
        self.launch();
    }

    /// `cover`'s look, once worked out; the first ask starts that (in the
    /// background, ahead of the queue) and returns `None` meanwhile.
    pub fn look(&mut self, cover: &str) -> Option<Arc<Look>> {
        if let Some(l) = self.looks.get(cover) {
            self.looks_lru.touch(cover);
            return Some(l.clone());
        }
        if self.looks_pending.insert(cover.to_owned()) {
            let key = (cover.to_owned(), LOOK_SIZE);
            #[cfg(test)]
            let given = self.preloaded.get(cover).cloned();
            #[cfg(not(test))]
            let given: Option<ColorImage> = None;
            let (client, dir, tx, ctx) = (self.client.clone(), self.dir.clone(), self.look_tx.clone(), self.ctx.clone());
            self.rt.spawn(async move {
                let image = match given {
                    Some(i) => Some(i),
                    None => load(client.as_ref(), &dir, &key.0, key.1).await,
                };
                let look = tokio::task::spawn_blocking(move || LookParts::of(image.as_ref())).await.unwrap_or_default();
                let _ = tx.send((key.0, look));
                ctx.request_repaint();
            });
        }
        None
    }

    /// The best texture for `cover` drawn at `points` wide. Starts a load when
    /// the right size isn't here yet, and meanwhile returns any other size
    /// already loaded so a resize doesn't flash the placeholder.
    pub fn texture(&mut self, cover: &str, points: f32) -> Option<TextureHandle> {
        let key = self.key(cover, bucket(points * self.ctx.pixels_per_point()));
        if let Some(t) = self.textures.get(&key) {
            self.lru.touch(&key);
            return Some(t.clone());
        }
        self.request(key, false);
        SIZES.iter().rev().find_map(|&size| self.textures.get(&Key { size, ..key }).cloned())
    }

    /// Asks for `cover` at `points` wide before it's drawn (just past the
    /// viewport); it loads after everything on screen.
    pub fn prefetch(&mut self, cover: &str, points: f32) {
        let key = self.key(cover, bucket(points * self.ctx.pixels_per_point()));
        if !self.textures.contains_key(&key) {
            self.request(key, true);
        }
    }

    fn request(&mut self, key: Key, soon: bool) {
        if !self.missing.contains(&key) && self.queue.ask(key, soon) {
            self.launch();
        }
    }

    /// Spawns whatever the queue says runs next.
    fn launch(&mut self) {
        for (key, job, lane) in self.queue.start() {
            let cover = self.names[key.cover as usize].clone();
            let (tx, ctx, dir) = (self.tx.clone(), self.ctx.clone(), self.dir.clone());
            match lane {
                Lane::Disk => {
                    // No server: art already on disk still loads (offline browsing).
                    let online = self.client.is_some();
                    self.rt.spawn(async move {
                        let outcome = match read_cached(&dir, &cover, key.size).await {
                            Some(bytes) => Outcome::Image(decode_off_thread(bytes, key.size).await),
                            None if online => Outcome::Miss,
                            None => Outcome::Image(None),
                        };
                        let _ = tx.send((key, job, outcome));
                        ctx.request_repaint();
                    });
                }
                Lane::Net => {
                    let client = self.client.clone();
                    let task = self.rt.spawn(async move {
                        let bytes = match &client {
                            Some(c) => fetch_cached(c, &dir, &cover, key.size).await,
                            None => None,
                        };
                        let image = match bytes {
                            Some(b) => decode_off_thread(b, key.size).await,
                            None => None,
                        };
                        let _ = tx.send((key, job, Outcome::Image(image)));
                        ctx.request_repaint();
                    });
                    self.downloads.insert(key, (job, task.abort_handle()));
                }
            }
        }
    }

    /// Makes `image` the art for `cover` at every size, without a server.
    #[cfg(test)]
    pub fn preload(&mut self, cover: &str, image: ColorImage) {
        self.preloaded.insert(cover.to_owned(), image.clone());
        let tex = self.ctx.load_texture(format!("art:{cover}"), image, TextureOptions::LINEAR);
        for size in SIZES {
            let key = self.key(cover, size);
            self.textures.insert(key, tex.clone());
        }
    }

    /// Paints `cover` into `rect` (or the placeholder tile). A rect that isn't
    /// square is filled edge to edge, cropping the art's middle.
    /// `radius` may round only some corners (a mosaic's outer ones).
    pub fn paint(&mut self, ui: &Ui, rect: Rect, cover: Option<&str>, radius: impl Into<CornerRadius>) {
        let radius = radius.into();
        let painter = ui.painter();
        let tex = cover.and_then(|c| self.texture(c, rect.width().max(rect.height())));
        match tex {
            Some(t) => {
                let uv = crop_uv(rect.width() / rect.height().max(1.0));
                painter.add(egui::epaint::RectShape::filled(rect, radius, Color32::WHITE).with_texture(t.id(), uv));
            }
            None => {
                let t = crate::theme::Tokens::of(ui);
                painter.rect_filled(rect, radius, t.placeholder);
            }
        }
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

/// Where the cache keeps `cover` at `size` px.
pub fn cover_file(dir: &Path, cover: &str, size: u32) -> PathBuf {
    let safe: String = cover.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    dir.join(format!("{safe}-{size}"))
}

/// The cached file for `cover` at `size`, else the server's (cached on the
/// way); `None` when neither has it, or there's no server to ask.
async fn load(client: Option<&Client>, dir: &Path, cover: &str, size: u32) -> Option<ColorImage> {
    let bytes = match read_cached(dir, cover, size).await {
        Some(b) => b,
        None => fetch_cached(client?, dir, cover, size).await?,
    };
    decode_off_thread(bytes, size).await
}

/// The cached file for `cover` at `size`, if there is one.
async fn read_cached(dir: &Path, cover: &str, size: u32) -> Option<Vec<u8>> {
    let path = cover_file(dir, cover, size);
    let bytes = tokio::fs::read(&path).await.ok()?;
    // Mark as recently used for the disk budget, without waiting on it.
    tokio::task::spawn_blocking(move || std::fs::File::options().write(true).open(path).and_then(|f| f.set_modified(SystemTime::now())));
    Some(bytes)
}

async fn decode_off_thread(bytes: Vec<u8>, size: u32) -> Option<ColorImage> {
    tokio::task::spawn_blocking(move || decode(&bytes, size)).await.ok().flatten()
}

/// Downloads `cover` at `size` px into the cache (written whole, then renamed
/// into place) and returns its bytes.
pub async fn fetch_cached(client: &Client, dir: &Path, cover: &str, size: u32) -> Option<Vec<u8>> {
    let bytes = client.cover_art(cover, Some(size)).await.ok()?;
    let (dir, path) = (dir.to_owned(), cover_file(dir, cover, size));
    // Saved by a task of its own: a load aborted from here on (scrolled past)
    // still leaves the cover cached, and never a half-written file.
    tokio::spawn(async move {
        let tmp = path.with_extension("part");
        if tokio::fs::create_dir_all(&dir).await.is_ok() && tokio::fs::write(&tmp, &bytes).await.is_ok() {
            let _ = tokio::fs::rename(&tmp, &path).await;
        }
        bytes
    })
    .await
    .ok()
}

fn decode(bytes: &[u8], size: u32) -> Option<ColorImage> {
    let img = image::load_from_memory(bytes).ok()?;
    // The server scales to `size`, but some covers come back full size.
    let img = if img.width() > size * 3 / 2 || img.height() > size * 3 / 2 { img.thumbnail(size, size) } else { img };
    let rgba = img.to_rgba8();
    Some(ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

fn trim_disk_cache(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let files = entries
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((e.path(), m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
        })
        .collect();
    for path in eviction_plan(files, DISK_BUDGET) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn buckets() {
        assert_eq!(bucket(10.0), 64);
        assert_eq!(bucket(64.0), 64);
        assert_eq!(bucket(65.0), 128);
        assert_eq!(bucket(336.0), 512);
        assert_eq!(bucket(4000.0), 512);
    }

    #[test]
    fn cached_art_loads_without_a_server() {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        // A 64 px cover already on disk from an earlier session.
        let png = {
            let img = image::RgbaImage::from_pixel(64, 64, image::Rgba([200, 40, 40, 255]));
            let mut out = std::io::Cursor::new(Vec::new());
            img.write_to(&mut out, image::ImageFormat::Png).unwrap();
            out.into_inner()
        };
        std::fs::write(cover_file(dir.path(), "al-1", 64), png).unwrap();
        let ctx = egui::Context::default();
        let mut art = Art::new(rt.handle().clone(), ctx, dir.path().to_owned());
        // Never connected: no client at all.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            art.poll();
            if art.texture("al-1", 40.0).is_some() {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "cached art never loaded offline");
            std::thread::sleep(Duration::from_millis(10));
        }
        // Art that isn't cached can't load without a server, and isn't asked for again.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !art.covers.get("al-2").is_some_and(|&cover| art.missing.contains(&Key { cover, size: 64 })) {
            art.poll();
            assert!(art.texture("al-2", 40.0).is_none());
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
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
    fn file_names_are_safe() {
        let p = cover_file(Path::new("/c"), "al-1/../x?y", 256);
        assert_eq!(p, PathBuf::from("/c/al-1____x_y-256"));
    }

    #[test]
    fn decodes_and_downscales_oversized_art() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(600, 600, image::Rgba([10, 20, 30, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let img = decode(&png, 128).unwrap();
        assert_eq!(img.size, [128, 128]);
        assert!(decode(b"not an image", 64).is_none());
    }

    #[test]
    fn crops_to_fill() {
        assert_eq!(crop_uv(1.0), Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)));
        // Wider than tall: a band across the middle.
        assert_eq!(crop_uv(2.0), Rect::from_min_max(egui::pos2(0.0, 0.25), egui::pos2(1.0, 0.75)));
        assert_eq!(crop_uv(0.5), Rect::from_min_max(egui::pos2(0.25, 0.0), egui::pos2(0.75, 1.0)));
    }
}
