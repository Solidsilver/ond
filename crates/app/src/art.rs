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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    cover: String,
    size: u32,
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

    pub fn touch(&mut self, k: &K) {
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

type Loaded = (Key, Option<ColorImage>);

pub struct Art {
    rt: tokio::runtime::Handle,
    ctx: egui::Context,
    dir: Arc<PathBuf>,
    client: Option<Client>,
    textures: HashMap<Key, TextureHandle>,
    lru: Lru<Key>,
    pending: HashSet<Key>,
    /// Art the server doesn't have; not asked for again this session.
    missing: HashSet<Key>,
    tx: Sender<Loaded>,
    rx: Receiver<Loaded>,
    permits: Arc<tokio::sync::Semaphore>,
}

impl Art {
    pub fn new(rt: tokio::runtime::Handle, ctx: egui::Context, dir: PathBuf) -> Self {
        let (tx, rx) = channel();
        let dir = Arc::new(dir);
        let trim = dir.clone();
        rt.spawn_blocking(move || trim_disk_cache(&trim));
        Self {
            rt,
            ctx,
            dir,
            client: None,
            textures: HashMap::new(),
            lru: Lru::new(TEXTURE_BUDGET),
            pending: HashSet::new(),
            missing: HashSet::new(),
            tx,
            rx,
            permits: Arc::new(tokio::sync::Semaphore::new(FETCHES)),
        }
    }

    pub fn set_client(&mut self, client: Client) {
        self.client = Some(client);
        // Failures may have been "not connected yet".
        self.missing.clear();
    }

    /// Uploads finished loads. Call once per frame before drawing.
    pub fn poll(&mut self) {
        while let Ok((key, image)) = self.rx.try_recv() {
            self.pending.remove(&key);
            let Some(image) = image else {
                self.missing.insert(key);
                continue;
            };
            let weight = image.width() * image.height() * 4;
            let tex = self.ctx.load_texture(format!("art:{}:{}", key.cover, key.size), image, TextureOptions::LINEAR);
            for gone in self.lru.insert(key.clone(), weight) {
                self.textures.remove(&gone);
            }
            self.textures.insert(key, tex);
        }
    }

    /// The best texture for `cover` drawn at `points` wide. Starts a load when
    /// the right size isn't here yet, and meanwhile returns any other size
    /// already loaded so a resize doesn't flash the placeholder.
    pub fn texture(&mut self, cover: &str, points: f32) -> Option<TextureHandle> {
        let size = bucket(points * self.ctx.pixels_per_point());
        let key = Key { cover: cover.to_owned(), size };
        if let Some(t) = self.textures.get(&key) {
            self.lru.touch(&key);
            return Some(t.clone());
        }
        self.request(key);
        SIZES.iter().rev().find_map(|&s| self.textures.get(&Key { cover: cover.to_owned(), size: s }).cloned())
    }

    fn request(&mut self, key: Key) {
        if self.pending.contains(&key) || self.missing.contains(&key) {
            return;
        }
        let Some(client) = self.client.clone() else { return };
        self.pending.insert(key.clone());
        let (tx, ctx, dir, permits) = (self.tx.clone(), self.ctx.clone(), self.dir.clone(), self.permits.clone());
        self.rt.spawn(async move {
            let image = match permits.acquire().await {
                Ok(_permit) => load(&client, &dir, &key).await,
                Err(_) => None,
            };
            let _ = tx.send((key, image));
            ctx.request_repaint();
        });
    }

    /// Paints `cover` into `rect` (or the placeholder tile). A rect that isn't
    /// square is filled edge to edge, cropping the art's middle.
    pub fn paint(&mut self, ui: &Ui, rect: Rect, cover: Option<&str>, radius: f32) {
        let painter = ui.painter();
        let tex = cover.and_then(|c| self.texture(c, rect.width().max(rect.height())));
        match tex {
            Some(t) => {
                let uv = crop_uv(rect.width() / rect.height().max(1.0));
                painter.add(egui::epaint::RectShape::filled(rect, CornerRadius::same(radius as u8), Color32::WHITE).with_texture(t.id(), uv));
            }
            None => {
                let t = crate::theme::Tokens::of(ui);
                painter.rect_filled(rect, radius, t.placeholder);
            }
        }
    }
}

/// The part of a square image that fills a rect of this aspect (width / height), centred.
fn crop_uv(aspect: f32) -> Rect {
    if aspect >= 1.0 {
        let h = 1.0 / aspect;
        Rect::from_min_max(egui::pos2(0.0, (1.0 - h) / 2.0), egui::pos2(1.0, (1.0 + h) / 2.0))
    } else {
        let w = aspect;
        Rect::from_min_max(egui::pos2((1.0 - w) / 2.0, 0.0), egui::pos2((1.0 + w) / 2.0, 1.0))
    }
}

fn file_for(dir: &Path, key: &Key) -> PathBuf {
    cover_file(dir, &key.cover, key.size)
}

/// Where the cache keeps `cover` at `size` px.
pub fn cover_file(dir: &Path, cover: &str, size: u32) -> PathBuf {
    let safe: String = cover.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    dir.join(format!("{safe}-{size}"))
}

async fn load(client: &Client, dir: &Path, key: &Key) -> Option<ColorImage> {
    let path = file_for(dir, key);
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => {
            // Mark as recently used for the disk budget.
            let p = path.clone();
            let _ = tokio::task::spawn_blocking(move || std::fs::File::options().write(true).open(p).and_then(|f| f.set_modified(SystemTime::now())))
                .await;
            b
        }
        Err(_) => {
            let bytes = client.cover_art(&key.cover, Some(key.size)).await.ok()?;
            let tmp = path.with_extension("part");
            if tokio::fs::create_dir_all(dir).await.is_ok() && tokio::fs::write(&tmp, &bytes).await.is_ok() {
                let _ = tokio::fs::rename(&tmp, &path).await;
            }
            bytes
        }
    };
    let size = key.size;
    tokio::task::spawn_blocking(move || decode(&bytes, size)).await.ok().flatten()
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
        let p = file_for(Path::new("/c"), &Key { cover: "al-1/../x?y".into(), size: 256 });
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
