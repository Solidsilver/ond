//! Cover art: fetched from the server at a few fixed sizes, cached on disk,
//! decoded off the UI thread, and kept as GPU textures under a byte budget.
//!
//! Callers ask for art only while drawing something visible, so scrolled-away
//! rows never cost a request. A miss paints a neutral tile of the same size
//! (no layout shift) and the finished load triggers a single repaint.

use egui::{Color32, ColorImage, CornerRadius, Rect, TextureHandle, TextureOptions, Ui};
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

/// A finished load: the key, the cover's id and the image (`None`: no art).
type Loaded = (Key, String, Option<ColorImage>);

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
    /// Every cover id asked for, numbered (see [`Key`]).
    covers: HashMap<String, u32>,
    textures: HashMap<Key, TextureHandle>,
    lru: Lru<Key>,
    pending: HashSet<Key>,
    /// Art the server doesn't have; not asked for again this session.
    missing: HashSet<Key>,
    tx: Sender<Loaded>,
    rx: Receiver<Loaded>,
    permits: Arc<tokio::sync::Semaphore>,
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
            textures: HashMap::new(),
            lru: Lru::new(TEXTURE_BUDGET),
            pending: HashSet::new(),
            missing: HashSet::new(),
            tx,
            rx,
            permits: Arc::new(tokio::sync::Semaphore::new(FETCHES)),
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
    }

    /// `cover` at `size`, numbering the cover the first time it's seen.
    fn key(&mut self, cover: &str, size: u32) -> Key {
        let next = self.covers.len() as u32;
        let cover = match self.covers.get(cover) {
            Some(&n) => n,
            None => *self.covers.entry(cover.to_owned()).or_insert(next),
        };
        Key { cover, size }
    }

    /// Uploads finished loads (a few per frame). Call once per frame before
    /// drawing.
    pub fn poll(&mut self) {
        let mut uploads = 0;
        while uploads < UPLOADS_PER_FRAME
            && let Ok((key, cover, image)) = self.rx.try_recv()
        {
            self.pending.remove(&key);
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
            let tex = self.ctx.load_texture(format!("art:{cover}:{}", key.size), image, TextureOptions::LINEAR);
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
    }

    /// `cover`'s look, once worked out; the first ask starts that (in the
    /// background) and returns `None` meanwhile.
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
            let (client, dir, tx, ctx, permits) = (self.client.clone(), self.dir.clone(), self.look_tx.clone(), self.ctx.clone(), self.permits.clone());
            self.rt.spawn(async move {
                let image = match given {
                    Some(i) => Some(i),
                    None => match permits.acquire().await {
                        Ok(_permit) => load(client.as_ref(), &dir, &key.0, key.1).await,
                        Err(_) => None,
                    },
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
        self.request(key, cover);
        SIZES.iter().rev().find_map(|&size| self.textures.get(&Key { size, ..key }).cloned())
    }

    fn request(&mut self, key: Key, cover: &str) {
        if self.pending.contains(&key) || self.missing.contains(&key) {
            return;
        }
        // No server: art already on disk still loads (offline browsing).
        let client = self.client.clone();
        self.pending.insert(key);
        let cover = cover.to_owned();
        let (tx, ctx, dir, permits) = (self.tx.clone(), self.ctx.clone(), self.dir.clone(), self.permits.clone());
        self.rt.spawn(async move {
            let image = match permits.acquire().await {
                Ok(_permit) => load(client.as_ref(), &dir, &cover, key.size).await,
                Err(_) => None,
            };
            let _ = tx.send((key, cover, image));
            ctx.request_repaint();
        });
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
    let path = cover_file(dir, cover, size);
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => {
            // Mark as recently used for the disk budget.
            let p = path.clone();
            let _ = tokio::task::spawn_blocking(move || std::fs::File::options().write(true).open(p).and_then(|f| f.set_modified(SystemTime::now())))
                .await;
            b
        }
        Err(_) => fetch_cached(client?, dir, cover, size).await?,
    };
    tokio::task::spawn_blocking(move || decode(&bytes, size)).await.ok().flatten()
}

/// Downloads `cover` at `size` px into the cache (written whole, then renamed
/// into place) and returns its bytes.
pub async fn fetch_cached(client: &Client, dir: &Path, cover: &str, size: u32) -> Option<Vec<u8>> {
    let bytes = client.cover_art(cover, Some(size)).await.ok()?;
    let path = cover_file(dir, cover, size);
    let tmp = path.with_extension("part");
    if tokio::fs::create_dir_all(dir).await.is_ok() && tokio::fs::write(&tmp, &bytes).await.is_ok() {
        let _ = tokio::fs::rename(&tmp, &path).await;
    }
    Some(bytes)
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
