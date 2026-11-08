//! Book covers: fetched from the account at a few fixed sizes, cached on disk,
//! decoded off the UI thread, and kept as GPU textures under a byte budget.
//!
//! Shaped like Ond's `art.rs`, at the size Rede needs: the same queue (the most
//! recently asked for runs first, a scrolled-past download is dropped), the
//! same disk cache (written whole, then renamed into place) and the same miss
//! (a neutral tile of the same size, no layout shift, one repaint when the
//! cover lands). Rede's books are one cover each, so there are no artist
//! stand-ins and no palettes.
//!
//! Callers ask for art only while drawing something visible, so scrolled-away
//! rows never cost a request. A cover comes through [`sources::Source::cover`],
//! which is what makes the cache work for any source. The machinery under it —
//! the queue, the LRU set, the cache's names and the decode — is
//! [`shell::art`], shared with Ond.

use egui::{Color32, ColorImage, CornerRadius, Rect, TextureHandle, TextureOptions, Ui};
use shell::art::{Lane, Lru, Queue, SIZES, bucket};
use sources::Source;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::SystemTime;

/// GPU budget for decoded covers.
const TEXTURE_BUDGET: usize = 96 * 1024 * 1024;
/// Disk budget for downloaded covers.
const DISK_BUDGET: u64 = 256 * 1024 * 1024;
/// Textures uploaded per frame at most: a screenful of covers arriving at once
/// spreads over a few frames instead of stalling one.
const UPLOADS_PER_FRAME: usize = 8;

/// A cover at one size. Covers go by their number in [`Art::covers`], so
/// finding a texture while drawing copies no string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    cover: u32,
    size: u32,
}

/// What a load reports: the cover, or that the disk cache missed and it needs a
/// download.
enum Outcome {
    Image(Option<ColorImage>),
    Miss,
}

/// A finished job: its key, its number in the queue and what it found.
type Loaded = (Key, u64, Outcome);

pub struct Art {
    rt: tokio::runtime::Handle,
    ctx: egui::Context,
    dir: Arc<PathBuf>,
    /// The signed-in account's source, which serves the covers. `None` before
    /// a sign-in (and after a sign-out).
    source: Option<Arc<dyn Source>>,
    /// Every cover asked for, numbered (see [`Key`]), and their ids by number.
    covers: HashMap<Arc<str>, u32>,
    names: Vec<Arc<str>>,
    textures: HashMap<Key, TextureHandle>,
    lru: Lru<Key>,
    queue: Queue<Key>,
    /// Running downloads, by job, so a scrolled-past one can be aborted.
    downloads: HashMap<Key, (u64, tokio::task::AbortHandle)>,
    /// Covers the account doesn't have; not asked for again this sign-in.
    missing: HashSet<Key>,
    tx: Sender<Loaded>,
    rx: Receiver<Loaded>,
}

impl Art {
    /// Opens the cache in `dir`, trimming it to its budget in the background.
    pub fn new(rt: tokio::runtime::Handle, ctx: egui::Context, dir: PathBuf) -> Self {
        let (tx, rx) = channel();
        let dir = Arc::new(dir);
        let trim = dir.clone();
        rt.spawn_blocking(move || shell::art::trim_disk_cache(&trim, DISK_BUDGET));
        Self {
            rt,
            ctx,
            dir,
            source: None,
            covers: HashMap::new(),
            names: Vec::new(),
            textures: HashMap::new(),
            lru: Lru::new(TEXTURE_BUDGET),
            queue: Queue::new(),
            downloads: HashMap::new(),
            missing: HashSet::new(),
            tx,
            rx,
        }
    }

    /// The account whose covers load from now on. Art already fetched stays;
    /// covers that failed for want of a source are asked for again.
    pub fn set_source(&mut self, source: Option<Arc<dyn Source>>) {
        self.source = source;
        self.missing.clear();
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
            let tex = self.ctx.load_texture(format!("cover:{}:{}", self.names[key.cover as usize], key.size), image, TextureOptions::LINEAR);
            for gone in self.lru.insert(key, weight) {
                self.textures.remove(&gone);
            }
            self.textures.insert(key, tex);
        }
        for key in self.queue.next_frame() {
            if let Some((_, task)) = self.downloads.remove(&key) {
                task.abort();
            }
        }
        self.launch();
    }

    /// The best texture for `cover` drawn at `points` wide. Starts a load when
    /// the right size isn't here yet, and meanwhile returns any other size
    /// already loaded so a resize doesn't flash the placeholder.
    pub fn texture(&mut self, cover: &str, points: f32) -> Option<TextureHandle> {
        let key = self.shown(cover, points)?;
        self.textures.get(&key).cloned()
    }

    /// The key of what's loaded for `cover` at `points` wide (asking for it
    /// when that size isn't), else of the largest size that is.
    fn shown(&mut self, cover: &str, points: f32) -> Option<Key> {
        let key = self.key(cover, bucket(points * self.ctx.pixels_per_point()));
        if self.textures.contains_key(&key) {
            self.lru.touch(&key);
            return Some(key);
        }
        self.request(key, false);
        SIZES.iter().rev().map(|&size| Key { size, ..key }).find(|k| self.textures.contains_key(k))
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
                    // No account: covers already on disk still load.
                    let online = self.source.is_some();
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
                    let source = self.source.clone();
                    let task = self.rt.spawn(async move {
                        let image = fetch(source.as_deref(), &dir, &cover, key.size).await;
                        let _ = tx.send((key, job, Outcome::Image(image)));
                        ctx.request_repaint();
                    });
                    self.downloads.insert(key, (job, task.abort_handle()));
                }
            }
        }
    }

    /// Fits the whole cover on a quiet backdrop, or shows its title while art
    /// is missing or still loading. Both views use the same square bounds.
    pub fn paint(&mut self, ui: &Ui, rect: Rect, cover: Option<&str>, title: &str, radius: impl Into<CornerRadius>) {
        let radius = radius.into();
        let t = shell::theme::Tokens::of(ui);
        let painter = ui.painter();
        painter.rect_filled(rect, radius, t.placeholder);
        match cover.and_then(|c| self.texture(c, rect.width().max(rect.height()))) {
            Some(texture) => {
                let size = texture.size_vec2();
                let scale = (rect.width() / size.x).min(rect.height() / size.y);
                let fitted = Rect::from_center_size(rect.center(), size * scale);
                let uv = Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
                painter.add(egui::epaint::RectShape::filled(fitted, radius, Color32::WHITE).with_texture(texture.id(), uv));
            }
            None => {
                let content = rect.shrink(16.0);
                let painter = painter.with_clip_rect(painter.clip_rect().intersect(content));
                let title = painter.layout(title.to_owned(), egui::FontId::proportional(14.0), t.muted, content.width());
                let top = (rect.center().y - title.size().y / 2.0).max(content.top());
                painter.galley(egui::pos2(content.left(), top), title, t.muted);
            }
        }
    }

    #[cfg(test)]
    pub fn preload(&mut self, cover: &str, image: ColorImage) {
        let key = self.key(cover, 512);
        let weight = image.width() * image.height() * 4;
        let texture = self.ctx.load_texture(cover, image, TextureOptions::LINEAR);
        for gone in self.lru.insert(key, weight) {
            self.textures.remove(&gone);
        }
        self.textures.insert(key, texture);
    }
}

/// The cached file for `cover` at `size`, else the account's (cached on the
/// way); `None` when neither has it, or there's no account to ask.
async fn fetch(source: Option<&dyn Source>, dir: &Path, cover: &str, size: u32) -> Option<ColorImage> {
    let bytes = match read_cached(dir, cover, size).await {
        Some(b) => b,
        None => fetch_cached(source?, dir, cover, size).await?,
    };
    decode_off_thread(bytes, size).await
}

/// The cached file for `cover` at `size`, if there is one.
async fn read_cached(dir: &Path, cover: &str, size: u32) -> Option<Vec<u8>> {
    let path = shell::art::cover_file(dir, cover, size);
    let bytes = tokio::fs::read(&path).await.ok()?;
    // Mark as recently used for the disk budget, without waiting on it.
    tokio::task::spawn_blocking(move || std::fs::File::options().write(true).open(path).and_then(|f| f.set_modified(SystemTime::now())));
    Some(bytes)
}

async fn decode_off_thread(bytes: Vec<u8>, size: u32) -> Option<ColorImage> {
    tokio::task::spawn_blocking(move || shell::art::decode(&bytes, size)).await.ok().flatten()
}

/// Fetches `cover` at `size` px from the account into the cache (written whole,
/// then renamed into place) and returns its bytes.
async fn fetch_cached(source: &dyn Source, dir: &Path, cover: &str, size: u32) -> Option<Vec<u8>> {
    let bytes = source.cover(cover, Some(size)).await.ok()?.to_vec();
    let (dir, path) = (dir.to_owned(), shell::art::cover_file(dir, cover, size));
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn non_square_covers_are_centred_and_fitted_without_cropping() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut art = Art::new(rt.handle().clone(), ctx.clone(), dir.path().to_owned());
        let rect = Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(200.0, 200.0));
        for (size, expected) in [([100, 200], egui::vec2(100.0, 200.0)), ([200, 100], egui::vec2(200.0, 100.0)), ([200, 200], rect.size())] {
            art.preload("book", ColorImage::filled(size, Color32::WHITE));
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| art.paint(ui, rect, Some("book"), "A book", 5));
            out.textures_delta.clear();
            let painted = out
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Rect(r) if r.fill_texture_id() != egui::TextureId::default() => Some(r),
                    _ => None,
                })
                .expect("the cover is painted");
            assert_eq!(painted.rect.size(), expected);
            assert_eq!(painted.rect.center(), rect.center());
            assert_eq!(painted.brush.as_ref().unwrap().uv, Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)));
        }
    }

    #[test]
    fn a_missing_or_loading_cover_shows_a_wrapped_and_clipped_title() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut art = Art::new(rt.handle().clone(), ctx.clone(), dir.path().to_owned());
        let rect = Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(176.0, 176.0));
        let title = "The Long Title of a Book Without Artwork ".repeat(20);
        for cover in [None, Some("not-loaded")] {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| art.paint(ui, rect, cover, &title, 5));
            out.textures_delta.clear();
            let (clip, text) = out
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some((s.clip_rect, t)),
                    _ => None,
                })
                .expect("the placeholder names the book");
            assert_eq!(text.galley.text(), title);
            assert!(text.galley.rows.len() > 1);
            assert!(text.galley.size().x <= rect.width() - 32.0);
            assert_eq!(clip, rect.shrink(16.0));
        }
    }

    #[test]
    fn cached_covers_load_without_an_account() {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        // A 64 px cover already on disk from an earlier sign-in.
        let png = {
            let img = image::RgbaImage::from_pixel(64, 64, image::Rgba([200, 40, 40, 255]));
            let mut out = std::io::Cursor::new(Vec::new());
            img.write_to(&mut out, image::ImageFormat::Png).unwrap();
            out.into_inner()
        };
        std::fs::write(shell::art::cover_file(dir.path(), "book-1", 64), png).unwrap();
        let ctx = egui::Context::default();
        let mut art = Art::new(rt.handle().clone(), ctx, dir.path().to_owned());
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            art.poll();
            if art.texture("book-1", 40.0).is_some() {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "cached art never loaded offline");
            std::thread::sleep(Duration::from_millis(10));
        }
        // A cover that isn't cached can't load without an account, and isn't
        // asked for again this sign-in: the tile stays a tile.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !art.covers.get("book-2").is_some_and(|&cover| art.missing.contains(&Key { cover, size: 64 })) {
            art.poll();
            assert!(art.texture("book-2", 40.0).is_none());
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
