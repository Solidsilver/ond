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
use shell::art::{Lane, Queue, SIZES, bucket};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant, SystemTime};
use subsonic::Client;

// The crop, the cache's file names and the LRU set are the family's, shared
// with Rede; Ond's callers keep finding them here.
pub use shell::art::{Lru, cover_file, crop_uv};

/// GPU budget for decoded art.
const TEXTURE_BUDGET: usize = 192 * 1024 * 1024;
/// Disk budget for downloaded art.
const DISK_BUDGET: u64 = 512 * 1024 * 1024;
/// How often [`Art::disk_size`] counts the files again.
const RECOUNT: Duration = Duration::from_secs(3);

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

/// What a load reports: the art and its [`Print`] (`None`: there's none to be
/// had), or that the disk cache missed and it needs a download.
enum Outcome {
    Image(Option<(ColorImage, Print)>),
    Miss,
}

/// A hash of an image file as the server sent it: the same picture sent for
/// two covers has the same print.
type Print = u64;

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
    /// Each texture's print, and for artists' photos, the first artist each
    /// print was seen for (see [`Art::portrait`]).
    prints: HashMap<Key, Print>,
    portraits: HashMap<Print, u32>,
    /// Prints seen for more than one artist: the server's stand-in for an
    /// artist without a photo.
    stand_ins: HashSet<Print>,
    tx: Sender<Loaded>,
    rx: Receiver<Loaded>,
    /// Looks by cover id, for the few covers lately asked about.
    looks: HashMap<String, Arc<Look>>,
    looks_lru: Lru<String>,
    looks_pending: HashSet<String>,
    /// Bytes of art on disk as last counted (`u64::MAX`: not yet), and when
    /// that count was started.
    disk: Arc<AtomicU64>,
    counted: Option<Instant>,
    look_tx: Sender<(String, LookParts)>,
    look_rx: Receiver<(String, LookParts)>,
    /// Images given by [`Art::preload`], so looks work without a server.
    #[cfg(test)]
    preloaded: HashMap<String, ColorImage>,
    /// A size for [`Art::disk_size`] to report as it is (see `pin_disk_size`).
    #[cfg(test)]
    pinned_disk: Option<u64>,
}

impl Art {
    pub fn new(rt: tokio::runtime::Handle, ctx: egui::Context, dir: PathBuf) -> Self {
        let (tx, rx) = channel();
        let (look_tx, look_rx) = channel();
        let dir = Arc::new(dir);
        let trim = dir.clone();
        rt.spawn_blocking(move || shell::art::trim_disk_cache(&trim, DISK_BUDGET));
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
            prints: HashMap::new(),
            portraits: HashMap::new(),
            stand_ins: HashSet::new(),
            tx,
            rx,
            looks: HashMap::new(),
            looks_lru: Lru::new(LOOKS),
            looks_pending: HashSet::new(),
            disk: Arc::new(AtomicU64::new(u64::MAX)),
            counted: None,
            look_tx,
            look_rx,
            #[cfg(test)]
            preloaded: HashMap::new(),
            #[cfg(test)]
            pinned_disk: None,
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
            let Some((image, print)) = image else {
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
                self.prints.remove(&gone);
            }
            self.textures.insert(key, tex);
            self.prints.insert(key, print);
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
        let key = self.shown(cover, points)?;
        self.textures.get(&key).cloned()
    }

    /// An artist's photo, like [`Art::texture`], but `None` for the picture
    /// the server sends for every artist it has no photo of. That one is
    /// known once it's come back for a second artist; until then it shows.
    pub fn portrait(&mut self, cover: &str, points: f32) -> Option<TextureHandle> {
        let key = self.shown(cover, points)?;
        if let Some(&print) = self.prints.get(&key) {
            let first = *self.portraits.entry(print).or_insert(key.cover);
            if first != key.cover {
                self.stand_ins.insert(print);
            }
            if self.stand_ins.contains(&print) {
                return None;
            }
        }
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

    /// Bytes of art on disk, recounted in the background every few seconds
    /// while asked for; `None` until first counted.
    pub fn disk_size(&mut self) -> Option<u64> {
        #[cfg(test)]
        if let Some(bytes) = self.pinned_disk {
            return Some(bytes);
        }
        if self.counted.is_none_or(|at| at.elapsed() > RECOUNT) {
            self.counted = Some(Instant::now());
            let (dir, disk, ctx) = (self.dir.clone(), self.disk.clone(), self.ctx.clone());
            self.rt.spawn_blocking(move || {
                disk.store(shell::art::cached_files(&dir).iter().map(|f| f.1).sum(), Ordering::Relaxed);
                ctx.request_repaint();
            });
        }
        Some(self.disk.load(Ordering::Relaxed)).filter(|&n| n != u64::MAX)
    }

    /// Reports `bytes` as the art on disk whatever the walk finds: a snapshot
    /// shows one number, not whichever count finished first.
    #[cfg(test)]
    pub fn pin_disk_size(&mut self, bytes: u64) {
        self.pinned_disk = Some(bytes);
    }

    /// Forgets every cover, here and on disk: what's on screen loads again
    /// from the server.
    pub fn clear(&mut self) {
        for (_, (_, task)) in self.downloads.drain() {
            task.abort();
        }
        self.queue.clear();
        self.textures.clear();
        self.prints.clear();
        self.portraits.clear();
        self.stand_ins.clear();
        self.lru = Lru::new(TEXTURE_BUDGET);
        self.missing.clear();
        self.looks.clear();
        self.looks_lru = Lru::new(LOOKS);
        let (dir, disk, ctx) = (self.dir.clone(), self.disk.clone(), self.ctx.clone());
        self.rt.spawn_blocking(move || {
            for (path, _, _) in shell::art::cached_files(&dir) {
                let _ = std::fs::remove_file(path);
            }
            disk.store(shell::art::cached_files(&dir).iter().map(|f| f.1).sum(), Ordering::Relaxed);
            ctx.request_repaint();
        });
        self.ctx.request_repaint();
    }

    /// Makes `image` the art for `cover` at every size, without a server.
    #[cfg(test)]
    pub fn preload(&mut self, cover: &str, image: ColorImage) {
        self.preloaded.insert(cover.to_owned(), image.clone());
        let image_pixels = image.pixels.clone();
        let tex = self.ctx.load_texture(format!("art:{cover}"), image, TextureOptions::LINEAR);
        let print = print(&image_pixels);
        for size in SIZES {
            let key = self.key(cover, size);
            self.textures.insert(key, tex.clone());
            self.prints.insert(key, print);
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

/// The cached file for `cover` at `size`, else the server's (cached on the
/// way); `None` when neither has it, or there's no server to ask.
async fn load(client: Option<&Client>, dir: &Path, cover: &str, size: u32) -> Option<ColorImage> {
    let bytes = match read_cached(dir, cover, size).await {
        Some(b) => b,
        None => fetch_cached(client?, dir, cover, size).await?,
    };
    decode_off_thread(bytes, size).await.map(|(image, _)| image)
}

/// The cached file for `cover` at `size`, if there is one.
async fn read_cached(dir: &Path, cover: &str, size: u32) -> Option<Vec<u8>> {
    let path = cover_file(dir, cover, size);
    let bytes = tokio::fs::read(&path).await.ok()?;
    // Mark as recently used for the disk budget, without waiting on it.
    tokio::task::spawn_blocking(move || std::fs::File::options().write(true).open(path).and_then(|f| f.set_modified(SystemTime::now())));
    Some(bytes)
}

async fn decode_off_thread(bytes: Vec<u8>, size: u32) -> Option<(ColorImage, Print)> {
    tokio::task::spawn_blocking(move || shell::art::decode(&bytes, size).map(|image| (image, print(&bytes)))).await.ok().flatten()
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

/// Same bytes, same print (`DefaultHasher::new` has fixed keys).
fn print(data: impl Hash) -> Print {
    let mut h = std::hash::DefaultHasher::new();
    data.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_photo_for_two_artists_is_the_servers_stand_in() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut art = Art::new(rt.handle().clone(), egui::Context::default(), dir.path().to_owned());
        let image = |c: u8| ColorImage::new([4, 4], vec![Color32::from_gray(c); 16]);
        art.preload("ar-1", image(10));
        art.preload("ar-2", image(20));
        art.preload("ar-3", image(20));
        assert!(art.portrait("ar-1", 36.0).is_some());
        assert!(art.portrait("ar-2", 36.0).is_some(), "not known as the stand-in until it's seen twice");
        assert!(art.portrait("ar-3", 36.0).is_none());
        assert!(art.portrait("ar-2", 36.0).is_none());
        assert!(art.portrait("ar-1", 36.0).is_some());
        assert!(art.texture("ar-2", 36.0).is_some(), "still there as art");
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
        let png_len = png.len() as u64;
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
        // Clearing forgets the loaded cover and deletes the file, and the size says so.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while art.disk_size() != Some(png_len) {
            assert!(std::time::Instant::now() < deadline, "the cache was never counted");
            std::thread::sleep(Duration::from_millis(10));
        }
        art.clear();
        assert!(art.texture("al-1", 40.0).is_none());
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while art.disk_size() != Some(0) {
            assert!(std::time::Instant::now() < deadline, "the cache was never cleared");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!cover_file(dir.path(), "al-1", 64).exists());
    }
}
