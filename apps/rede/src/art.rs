//! Book covers: fetched from the account at a few fixed sizes, cached on disk,
//! decoded off the UI thread, and kept as GPU textures under a byte budget.
//!
//! Shaped like Ond's `art.rs`, at the size Rede needs: the same queue (the most
//! recently asked for runs first, a scrolled-past download is dropped), the
//! same disk cache (written whole, then renamed into place) and the same miss
//! (a neutral tile of the same size while it loads, no layout shift, one
//! repaint when the cover lands). A book with no cover at all gets a
//! generated one ([`shell::kit::generated_cover`]) with its title and author.
//! Rede's books are one cover each, so there are no artist stand-ins and no
//! palettes.
//!
//! Callers ask for art only while drawing something visible, so scrolled-away
//! rows never cost a request. A cover comes through [`sources::Source::cover`],
//! which is what makes the cache work for any source. The machinery under it —
//! the queue, the LRU set, the cache's names and the decode — is
//! [`shell::art`], shared with Ond.

use egui::{Color32, ColorImage, CornerRadius, Rect, TextureHandle, TextureOptions, Ui};
use shell::art::{Lane, Lru, Queue, SIZES, bucket};
use shell::kit::Generated;
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
    Image(Option<Decoded>),
    Miss,
}

/// A decoded cover, and for one that isn't square its small blurred copy,
/// the ground a square tile draws it on.
struct Decoded {
    image: ColorImage,
    blurred: Option<ColorImage>,
}

impl Decoded {
    fn new(image: ColorImage) -> Self {
        let [w, h] = image.size;
        let square = w == 0 || h == 0 || (w as f32 / h as f32 - 1.0).abs() < SQUARE_WITHIN;
        let blurred = if square { None } else { shell::art::blurred(&image).map(|(b, _)| b) };
        Self { image, blurred }
    }
}

/// A cover this close to square fills its tile; past it, it gets a ground.
const SQUARE_WITHIN: f32 = 0.02;
/// The blurred ground, dimmed so the cover on it stays the thing seen.
const GROUND_TINT: Color32 = Color32::from_gray(190);

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
    /// The blurred grounds of the covers that aren't square, by the same key.
    grounds: HashMap<Key, TextureHandle>,
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
            grounds: HashMap::new(),
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
            let Some(decoded) = image else {
                self.missing.insert(key);
                continue;
            };
            uploads += 1;
            if uploads == UPLOADS_PER_FRAME {
                // There may be more waiting: the next frame takes them.
                self.ctx.request_repaint();
            }
            let name = format!("{}:{}", self.names[key.cover as usize], key.size);
            self.upload(key, &name, decoded);
        }
        for key in self.queue.next_frame() {
            if let Some((_, task)) = self.downloads.remove(&key) {
                task.abort();
            }
        }
        self.launch();
    }

    /// Puts a decoded cover (and its ground) on the GPU under `key`, making
    /// room in the budget.
    fn upload(&mut self, key: Key, name: &str, decoded: Decoded) {
        let ground_weight = decoded.blurred.as_ref().map_or(0, |b| b.width() * b.height() * 4);
        let weight = decoded.image.width() * decoded.image.height() * 4 + ground_weight;
        let tex = self.ctx.load_texture(format!("cover:{name}"), decoded.image, TextureOptions::LINEAR);
        for gone in self.lru.insert(key, weight) {
            self.textures.remove(&gone);
            self.grounds.remove(&gone);
        }
        self.textures.insert(key, tex);
        match decoded.blurred {
            Some(b) => self.grounds.insert(key, self.ctx.load_texture(format!("ground:{name}"), b, TextureOptions::LINEAR)),
            None => self.grounds.remove(&key),
        };
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

    /// Fits the whole cover on a quiet backdrop. While it loads the tile is
    /// only the backdrop; a book with no cover (none on the server, or none
    /// to be had offline) gets `stand_in`, a generated cover with its title,
    /// so a shelf of them still tells the books apart. Both use the same
    /// square, so nothing shifts when the art lands.
    pub fn paint(&mut self, ui: &Ui, rect: Rect, cover: Option<&str>, radius: impl Into<CornerRadius>, stand_in: &Generated) {
        let backdrop = shell::theme::Tokens::of(ui).placeholder;
        self.draw(ui, rect, cover, radius.into(), backdrop, stand_in, false);
    }

    /// The same on a backdrop of the caller's choosing: Mini fills its whole
    /// window with the cover, and a light band beside a tall cover would
    /// glare there in either theme. A book without a cover fills it with its
    /// generated ground alone: Mini's controls sit over the cover and its
    /// band already names the book.
    pub fn paint_on(&mut self, ui: &Ui, rect: Rect, cover: Option<&str>, radius: impl Into<CornerRadius>, backdrop: Color32, stand_in: &Generated) {
        self.draw(ui, rect, cover, radius.into(), backdrop, stand_in, true);
    }

    /// Where `cover` sits drawn at its own shape within `bounds`, placed by
    /// `align`, so a page can draw it without the backdrop around it. Until
    /// its art is here (and for a book without any, whose generated cover is
    /// square) that's all of `bounds`.
    pub fn fitted(&mut self, bounds: Rect, cover: Option<&str>, align: egui::Align2) -> Rect {
        let points = bounds.width().max(bounds.height());
        let Some(size) = cover.and_then(|c| self.texture(c, points)).map(|t| t.size_vec2()).filter(|s| s.x > 0.0 && s.y > 0.0) else {
            return bounds;
        };
        let scale = (bounds.width() / size.x).min(bounds.height() / size.y);
        align.align_size_within_rect(size * scale, bounds)
    }

    /// `bare`: a book without a cover gets only its generated ground, no
    /// title.
    #[allow(clippy::too_many_arguments)]
    fn draw(&mut self, ui: &Ui, rect: Rect, cover: Option<&str>, radius: CornerRadius, backdrop: Color32, stand_in: &Generated, bare: bool) {
        let points = rect.width().max(rect.height());
        let key = cover.and_then(|c| self.shown(c, points));
        let texture = key.and_then(|k| self.textures.get(&k).cloned());
        let painter = ui.painter();
        match texture {
            Some(texture) => {
                let size = texture.size_vec2();
                let scale = (rect.width() / size.x).min(rect.height() / size.y);
                let fitted = Rect::from_center_size(rect.center(), size * scale);
                let uv = Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
                // Where the cover doesn't fill the tile, the tile is the cover
                // itself blurred and dimmed (cropped to the tile, not
                // stretched), else the quiet backdrop. Mini picks its own
                // (`bare`).
                let ground = key.filter(|_| !bare && size.x > 0.0 && size.y > 0.0).and_then(|k| self.grounds.get(&k));
                match ground {
                    Some(ground) => {
                        let crop = shell::art::crop_uv(size.y / size.x * rect.width() / rect.height());
                        painter.add(egui::epaint::RectShape::filled(rect, radius, GROUND_TINT).with_texture(ground.id(), crop));
                    }
                    None => {
                        painter.rect_filled(rect, radius, backdrop);
                    }
                }
                painter.add(egui::epaint::RectShape::filled(fitted, radius, Color32::WHITE).with_texture(texture.id(), uv));
            }
            None if cover.is_some_and(|c| !self.is_missing(c, points)) => {
                painter.rect_filled(rect, radius, backdrop);
            }
            None if !bare => shell::kit::generated_cover(ui, rect, radius, stand_in),
            None => shell::kit::generated_ground(ui, rect, radius, stand_in.seed),
        }
    }

    /// Whether `cover` at `points` wide is known not to be had.
    fn is_missing(&mut self, cover: &str, points: f32) -> bool {
        let key = self.key(cover, bucket(points * self.ctx.pixels_per_point()));
        self.missing.contains(&key)
    }

    /// Fixtures know their missing covers already; disk work would race the
    /// idle assertion and make snapshots depend on task scheduling.
    #[cfg(test)]
    pub fn preload_missing(&mut self, cover: &str) {
        for size in SIZES {
            let key = self.key(cover, size);
            self.missing.insert(key);
        }
    }

    #[cfg(test)]
    pub fn preload(&mut self, cover: &str, image: ColorImage) {
        let key = self.key(cover, 512);
        self.upload(key, cover, Decoded::new(image));
    }
}

/// The cached file for `cover` at `size`, else the account's (cached on the
/// way); `None` when neither has it, or there's no account to ask.
async fn fetch(source: Option<&dyn Source>, dir: &Path, cover: &str, size: u32) -> Option<Decoded> {
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

async fn decode_off_thread(bytes: Vec<u8>, size: u32) -> Option<Decoded> {
    tokio::task::spawn_blocking(move || shell::art::decode(&bytes, size).map(Decoded::new)).await.ok().flatten()
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

    #[test]
    fn non_square_covers_are_centred_and_fitted_without_cropping_on_their_blurred_selves() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut art = Art::new(rt.handle().clone(), ctx.clone(), dir.path().to_owned());
        let rect = Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(200.0, 200.0));
        for (size, expected) in [([100, 200], egui::vec2(100.0, 200.0)), ([200, 100], egui::vec2(200.0, 100.0)), ([200, 200], rect.size())] {
            art.preload("book", ColorImage::filled(size, Color32::WHITE));
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| art.paint(ui, rect, Some("book"), 5, &STAND_IN));
            out.textures_delta.clear();
            let textured: Vec<_> = out
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Rect(r) if r.fill_texture_id() != egui::TextureId::default() => Some(r),
                    _ => None,
                })
                .collect();
            let painted = textured.last().expect("the cover is painted");
            // A cover that isn't square lies on its own blurred copy, which
            // fills the tile; a square one needs none.
            if expected == rect.size() {
                assert_eq!(textured.len(), 1);
            } else {
                assert_eq!(textured.len(), 2);
                assert_eq!(textured[0].rect, rect, "the ground fills the tile");
            }
            assert_eq!(painted.rect.size(), expected);
            assert_eq!(painted.rect.center(), rect.center());
            assert_eq!(painted.brush.as_ref().unwrap().uv, Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)));
        }
    }

    const STAND_IN: Generated = Generated { title: "The Dispossessed", byline: Some("Ursula K. Le Guin"), seed: "book-9" };

    #[test]
    fn a_loading_cover_is_a_quiet_tile_and_a_missing_one_a_generated_cover() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut art = Art::new(rt.handle().clone(), ctx.clone(), dir.path().to_owned());
        art.preload_missing("gone");
        // The theme's fonts (its semibold) arrive a frame after they're installed.
        shell::theme::install(&ctx, shell::theme::ThemePref::Light);
        ctx.run_ui(egui::RawInput::default(), |_| ()).textures_delta.clear();
        let rect = Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(176.0, 176.0));
        let texts = |out: &egui::FullOutput| out.shapes.iter().filter(|s| matches!(s.shape, egui::Shape::Text(_))).count();
        let tile = |out: &egui::FullOutput| {
            out.shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Rect(r) => Some((r.rect, r.fill)),
                _ => None,
            })
        };
        // Still loading: the backdrop alone, so the cover doesn't flash a
        // made-up one first.
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| art.paint(ui, rect, Some("loading"), 6, &STAND_IN));
        out.textures_delta.clear();
        assert_eq!(texts(&out), 0);
        let (loading, _) = tile(&out).unwrap();
        assert_eq!(loading, rect, "the tile keeps the cover's square, so nothing shifts when the art lands");
        // None to be had, or no cover at all: the title and author on a ground of the book's own.
        for cover in [None, Some("gone")] {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| art.paint(ui, rect, cover, 6, &STAND_IN));
            out.textures_delta.clear();
            assert_eq!(texts(&out), 2, "the title and the author");
            let (r, fill) = tile(&out).unwrap();
            assert_eq!(r, rect);
            assert_eq!(fill, shell::kit::covers::ground("book-9", ctx.global_style().visuals.dark_mode).fill);
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
        testkit::wait_until("cached art to load offline", || {
            art.poll();
            art.texture("book-1", 40.0).is_some()
        });
        // A cover that isn't cached can't load without an account, and isn't
        // asked for again this sign-in: the tile stays a tile.
        testkit::wait_until("the uncached cover to be marked missing", || {
            art.poll();
            assert!(art.texture("book-2", 40.0).is_none());
            art.covers.get("book-2").is_some_and(|&cover| art.missing.contains(&Key { cover, size: 64 }))
        });
    }
}
