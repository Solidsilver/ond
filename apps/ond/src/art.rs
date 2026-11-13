//! Cover art: fetched from the server at a few fixed sizes, cached on disk,
//! decoded off the UI thread, and kept as GPU textures under a byte budget.
//!
//! Callers ask for art only while drawing something visible, so scrolled-away
//! rows never cost a request. A miss paints a neutral tile of the same size
//! (no layout shift) and the finished load triggers a single repaint. Art
//! the server doesn't have (or an album with no cover at all) is drawn by
//! [`Art::paint_as`] as a generated cover with the album's title and artist
//! ([`shell::kit::generated_cover`]), so albums without art still look apart.
//!
//! Loads are asked for again every frame they're drawn, and the [`Queue`] runs
//! the most recently asked first. One that goes unasked for a couple of frames
//! (scrolled past) is dropped before it starts, or its download aborted, so a
//! fast scroll doesn't leave a backlog ahead of what's on screen now. Disk
//! reads have their own slots and never wait behind downloads. Looks (for the
//! playing and the open album, a cover or two) skip the queue and start at once.
//!
//! Each account has one cache owner, including the primary account held here.
//! Owners share fixed total GPU and disk limits in equal portions. Adding or
//! removing an owner resizes every portion and trims existing contents; a
//! qualified id routes to its owner before the bare id enters that cache.

use crate::source::Src;
use accounts::AccountId;
use egui::{Color32, ColorImage, CornerRadius, Rect, TextureHandle, TextureOptions, Ui};
use shell::art::{Lane, Queue, SIZES, bucket};
use shell::kit::Generated;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

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

/// Where `account` keeps its cached covers under the art cache `root`.
pub fn account_dir(root: &Path, account: AccountId) -> PathBuf {
    root.join(account.to_string())
}

/// A finished job: the cache generation it started under, its key, its number
/// in the queue and what it found.
type Loaded = (u64, Key, u64, Outcome);

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

#[derive(Clone, Copy)]
struct Budget {
    texture: usize,
    disk: u64,
}

pub struct Art {
    sources: HashMap<AccountId, Box<Art>>,
    limits: Budget,
    budget: Budget,
    disk_limit: Arc<AtomicU64>,
    trim_gate: Arc<Mutex<()>>,
    rt: tokio::runtime::Handle,
    ctx: egui::Context,
    root: PathBuf,
    account: Option<AccountId>,
    dir: Option<Arc<PathBuf>>,
    /// Bumped when switching accounts or clearing so late results from an
    /// earlier account's tasks are ignored.
    generation: u64,
    source: Option<Src>,
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
    look_tx: Sender<(u64, String, LookParts)>,
    look_rx: Receiver<(u64, String, LookParts)>,
    /// Images given by [`Art::preload`], so looks work without a server.
    #[cfg(test)]
    preloaded: HashMap<String, ColorImage>,
    /// A size for [`Art::disk_size`] to report as it is (see `pin_disk_size`).
    #[cfg(test)]
    pinned_disk: Option<u64>,
}

impl Art {
    pub fn new(rt: tokio::runtime::Handle, ctx: egui::Context, root: PathBuf, account: Option<AccountId>) -> Self {
        Self::with_budget(rt, ctx, root, account, Budget { texture: TEXTURE_BUDGET, disk: DISK_BUDGET })
    }

    fn with_budget(rt: tokio::runtime::Handle, ctx: egui::Context, root: PathBuf, account: Option<AccountId>, limits: Budget) -> Self {
        let (tx, rx) = channel();
        let (look_tx, look_rx) = channel();
        let dir = account.map(|a| Arc::new(account_dir(&root, a)));
        let art = Self {
            sources: HashMap::new(),
            limits,
            budget: limits,
            disk_limit: Arc::new(AtomicU64::new(limits.disk)),
            trim_gate: Arc::default(),
            rt,
            ctx,
            root,
            account,
            dir,
            generation: 0,
            source: None,
            covers: HashMap::new(),
            names: Vec::new(),
            textures: HashMap::new(),
            lru: Lru::new(limits.texture),
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
        };
        art.trim_disk();
        art
    }

    fn source_art(&mut self, account: AccountId) -> &mut Art {
        if self.account == Some(account) {
            return self;
        }
        if self.account.is_none() && !self.sources.contains_key(&account) {
            self.reset_memory();
            self.generation += 1;
            self.account = Some(account);
            self.dir = Some(Arc::new(account_dir(&self.root, account)));
            self.disk = Arc::new(AtomicU64::new(u64::MAX));
            self.counted = None;
        } else if !self.sources.contains_key(&account) {
            let art = Self::with_budget(self.rt.clone(), self.ctx.clone(), self.root.clone(), Some(account), self.limits);
            self.sources.insert(account, Box::new(art));
        } else {
            return self.sources.get_mut(&account).expect("account's cache");
        }
        self.rebalance();
        if self.account == Some(account) { self } else { self.sources.get_mut(&account).expect("new account's cache") }
    }

    fn rebalance(&mut self) {
        let owners = self.sources.len() + usize::from(self.account.is_some());
        let portion = Budget { texture: self.limits.texture / owners.max(1), disk: self.limits.disk / owners.max(1) as u64 };
        self.set_budget(if self.account.is_some() || owners == 0 { portion } else { Budget { texture: 0, disk: 0 } });
        for art in self.sources.values_mut() {
            art.set_budget(portion);
        }
    }

    fn set_budget(&mut self, budget: Budget) {
        if budget.texture > self.budget.texture {
            self.missing.clear();
        }
        self.budget = budget;
        for gone in self.lru.set_budget(budget.texture) {
            self.textures.remove(&gone);
            self.prints.remove(&gone);
        }
        self.disk_limit.store(budget.disk, Ordering::Relaxed);
        self.trim_disk();
    }

    fn trim_disk(&self) {
        if let Some(dir) = self.dir.clone() {
            let (limit, gate) = (self.disk_limit.clone(), self.trim_gate.clone());
            self.rt.spawn_blocking(move || {
                let _guard = gate.lock().unwrap_or_else(|e| e.into_inner());
                shell::art::trim_disk_cache(&dir, limit.load(Ordering::Relaxed));
            });
        }
    }

    /// Reserves a share even for a source starting offline or signed out.
    pub fn add_account(&mut self, account: AccountId) {
        self.source_art(account);
    }

    pub fn add_source(&mut self, source: Src) {
        self.source_art(source.account()).set_source_owned(source);
    }
    pub fn offline_source(&mut self, account: AccountId) {
        if self.account == Some(account) {
            self.clear_source();
        } else if let Some(art) = self.sources.get_mut(&account) {
            art.clear_source();
        }
    }
    pub fn remove_source(&mut self, account: AccountId) {
        if self.account == Some(account) {
            self.clear_source();
            self.reset_memory();
            self.generation += 1;
            self.account = None;
            self.dir = None;
            // Queued trims of the old folder keep its last limit; this empty
            // owner must not turn their limit into zero while being reused.
            self.disk_limit = Arc::new(AtomicU64::new(0));
        } else {
            self.sources.remove(&account);
        }
        self.rebalance();
    }

    pub fn set_source(&mut self, source: Src) {
        if self.sources.contains_key(&source.account()) {
            self.source_art(source.account()).set_source_owned(source);
        } else {
            self.set_source_owned(source);
            self.rebalance();
        }
    }

    fn set_source_owned(&mut self, source: Src) {
        let next = source.account();
        // The one place an account's folder is made (see `fetch_cached`). A
        // single mkdir, made before any download can try to save into it.
        if let Err(e) = std::fs::create_dir_all(account_dir(&self.root, next)) {
            tracing::warn!("making the cover cache for {next}: {e}");
        }
        let switched = self.account.is_some_and(|prev| prev != next);
        self.source = Some(source);
        if switched {
            self.reset_memory();
        } else {
            // Failures may have been "not connected yet".
            self.missing.clear();
            self.looks.retain(|_, l| l.palette.is_some());
            // The dropped looks leave the LRU too, or they'd count against its
            // budget and evict real ones.
            self.looks_lru.retain(|cover| self.looks.contains_key(cover));
            self.looks_pending.clear();
        }
        if self.account != Some(next) {
            self.generation += 1;
            self.account = Some(next);
            let dir = Arc::new(account_dir(&self.root, next));
            self.dir = Some(dir);
            self.disk = Arc::new(AtomicU64::new(u64::MAX));
            self.counted = None;
            self.trim_disk();
            self.ctx.request_repaint();
        }
    }

    pub fn clear_source(&mut self) {
        self.source = None;
        for (_, (_, task)) in self.downloads.drain() {
            task.abort();
        }
    }

    fn reset_memory(&mut self) {
        for (_, (_, task)) in self.downloads.drain() {
            task.abort();
        }
        self.queue.clear();
        self.textures.clear();
        self.prints.clear();
        self.portraits.clear();
        self.stand_ins.clear();
        self.lru = Lru::new(self.budget.texture);
        self.missing.clear();
        self.looks.clear();
        self.looks_lru = Lru::new(LOOKS);
        self.looks_pending.clear();
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
        for art in self.sources.values_mut() {
            art.poll();
        }
        let mut uploads = 0;
        while uploads < UPLOADS_PER_FRAME
            && let Ok((generation, key, job, outcome)) = self.rx.try_recv()
        {
            if generation != self.generation {
                continue;
            }
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
            if weight > self.budget.texture {
                self.missing.insert(key);
                continue;
            }
            let tex = self.ctx.load_texture(format!("art:{}:{}", self.names[key.cover as usize], key.size), image, TextureOptions::LINEAR);
            for gone in self.lru.insert(key, weight) {
                self.textures.remove(&gone);
                self.prints.remove(&gone);
            }
            self.textures.insert(key, tex);
            self.prints.insert(key, print);
        }
        let mut looks_loaded = false;
        while let Ok((generation, cover, parts)) = self.look_rx.try_recv() {
            if generation != self.generation {
                continue;
            }
            looks_loaded = true;
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
        if uploads > 0 || looks_loaded {
            self.trim_disk();
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
        if let Some(account) = crate::source::identity::account(cover) {
            return self.source_art(account).look_bare(crate::source::identity::bare(cover));
        }
        self.look_bare(cover)
    }

    fn look_bare(&mut self, cover: &str) -> Option<Arc<Look>> {
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
            let (source, dir, tx, ctx, generation) = (self.source.clone(), self.dir.clone(), self.look_tx.clone(), self.ctx.clone(), self.generation);
            self.rt.spawn(async move {
                let image = match given {
                    Some(i) => Some(i),
                    None => match dir.as_deref() {
                        Some(dir) => load(source.as_ref(), dir, &key.0, key.1).await,
                        None => None,
                    },
                };
                let look = tokio::task::spawn_blocking(move || LookParts::of(image.as_ref())).await.unwrap_or_default();
                let _ = tx.send((generation, key.0, look));
                ctx.request_repaint();
            });
        }
        None
    }

    /// The best texture for `cover` drawn at `points` wide. Starts a load when
    /// the right size isn't here yet, and meanwhile returns any other size
    /// already loaded so a resize doesn't flash the placeholder.
    pub fn texture(&mut self, cover: &str, points: f32) -> Option<TextureHandle> {
        if let Some(account) = crate::source::identity::account(cover) {
            return self.source_art(account).texture_bare(crate::source::identity::bare(cover), points);
        }
        self.texture_bare(cover, points)
    }

    fn texture_bare(&mut self, cover: &str, points: f32) -> Option<TextureHandle> {
        let key = self.shown(cover, points)?;
        self.textures.get(&key).cloned()
    }

    /// An artist's photo, like [`Art::texture`], but `None` for the picture
    /// the server sends for every artist it has no photo of. That one is
    /// known once it's come back for a second artist; until then it shows.
    pub fn portrait(&mut self, cover: &str, points: f32) -> Option<TextureHandle> {
        if let Some(account) = crate::source::identity::account(cover) {
            return self.source_art(account).portrait_bare(crate::source::identity::bare(cover), points);
        }
        self.portrait_bare(cover, points)
    }

    fn portrait_bare(&mut self, cover: &str, points: f32) -> Option<TextureHandle> {
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
        if let Some(account) = crate::source::identity::account(cover) {
            return self.source_art(account).prefetch_bare(crate::source::identity::bare(cover), points);
        }
        self.prefetch_bare(cover, points)
    }

    fn prefetch_bare(&mut self, cover: &str, points: f32) {
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
            let (tx, ctx, dir, generation) = (self.tx.clone(), self.ctx.clone(), self.dir.clone(), self.generation);
            match lane {
                Lane::Disk => {
                    // No server: art already on disk still loads (offline browsing).
                    let online = self.source.is_some();
                    self.rt.spawn(async move {
                        let outcome = match dir.as_deref() {
                            Some(dir) => match read_cached(dir, &cover, key.size).await {
                                Some(bytes) => Outcome::Image(decode_off_thread(bytes, key.size).await),
                                None if online => Outcome::Miss,
                                None => Outcome::Image(None),
                            },
                            None if online => Outcome::Miss,
                            None => Outcome::Image(None),
                        };
                        let _ = tx.send((generation, key, job, outcome));
                        ctx.request_repaint();
                    });
                }
                Lane::Net => {
                    let source = self.source.clone();
                    let task = self.rt.spawn(async move {
                        let bytes = match (&source, dir.as_deref()) {
                            (Some(c), Some(dir)) => fetch_cached(c, dir, &cover, key.size).await,
                            (Some(c), None) => c.cover(&cover, key.size).await.ok(),
                            (None, _) => None,
                        };
                        let image = match bytes {
                            Some(b) => decode_off_thread(b, key.size).await,
                            None => None,
                        };
                        let _ = tx.send((generation, key, job, Outcome::Image(image)));
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
        let own = self.disk_size_owned();
        let mut others = Some(0u64);
        for art in self.sources.values_mut() {
            others = others.zip(art.disk_size()).map(|(total, bytes)| total + bytes);
        }
        own.zip(others).map(|(own, others)| own + others)
    }

    fn disk_size_owned(&mut self) -> Option<u64> {
        let Some(dir) = self.dir.clone() else {
            return Some(0);
        };
        if self.counted.is_none_or(|at| at.elapsed() > RECOUNT) {
            self.counted = Some(Instant::now());
            let (disk, ctx) = (self.disk.clone(), self.ctx.clone());
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
        for art in self.sources.values_mut() {
            art.clear();
        }
        self.reset_memory();
        self.generation += 1;
        if let Some(dir) = self.dir.clone() {
            let (disk, ctx) = (self.disk.clone(), self.ctx.clone());
            self.rt.spawn_blocking(move || {
                for (path, _, _) in shell::art::cached_files(&dir) {
                    let _ = std::fs::remove_file(path);
                }
                disk.store(shell::art::cached_files(&dir).iter().map(|f| f.1).sum(), Ordering::Relaxed);
                ctx.request_repaint();
            });
        } else {
            self.disk.store(0, Ordering::Relaxed);
        }
        self.ctx.request_repaint();
    }

    /// Forgets every loaded texture (the disk is left alone), as a session
    /// that hasn't drawn any albums yet would have none at hand.
    #[cfg(test)]
    pub fn unload(&mut self) {
        for art in self.sources.values_mut() {
            art.unload();
        }
        self.textures.clear();
        self.prints.clear();
        self.lru = Lru::new(self.budget.texture);
    }

    /// Marks `cover` as art the server doesn't have, at every size.
    #[cfg(test)]
    pub fn preload_missing(&mut self, cover: &str) {
        for size in SIZES {
            let key = self.key(cover, size);
            self.textures.remove(&key);
            self.missing.insert(key);
        }
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

    /// [`Art::paint`], but art that isn't to be had (none on the server, or
    /// no `cover` at all) is `stand_in`, a generated cover, rather than the
    /// quiet tile that stands in while art loads.
    pub fn paint_as(&mut self, ui: &Ui, rect: Rect, cover: Option<&str>, radius: impl Into<CornerRadius>, stand_in: &Generated) {
        let radius = radius.into();
        let points = rect.width().max(rect.height());
        if cover.is_none_or(|c| self.texture(c, points).is_none() && self.is_missing(c, points)) {
            shell::kit::generated_cover(ui, rect, radius, stand_in);
        } else {
            self.paint(ui, rect, cover, radius);
        }
    }

    /// Whether `cover` at `points` wide is known not to be had.
    fn is_missing(&mut self, cover: &str, points: f32) -> bool {
        if let Some(account) = crate::source::identity::account(cover) {
            return self.source_art(account).is_missing_bare(crate::source::identity::bare(cover), points);
        }
        self.is_missing_bare(cover, points)
    }

    fn is_missing_bare(&mut self, cover: &str, points: f32) -> bool {
        let key = self.key(cover, bucket(points * self.ctx.pixels_per_point()));
        self.missing.contains(&key)
    }

    /// The largest size of `cover` already loaded, without asking for it
    /// when there's none: for drawing what's at hand (an artist's albums on
    /// their disc) without a request of its own.
    pub fn loaded(&mut self, cover: &str) -> Option<TextureHandle> {
        if let Some(account) = crate::source::identity::account(cover) {
            return self.source_art(account).loaded_bare(crate::source::identity::bare(cover));
        }
        self.loaded_bare(cover)
    }

    fn loaded_bare(&mut self, cover: &str) -> Option<TextureHandle> {
        let &n = self.covers.get(cover)?;
        let key = SIZES.iter().rev().map(|&size| Key { cover: n, size }).find(|k| self.textures.contains_key(k))?;
        self.lru.touch(&key);
        self.textures.get(&key).cloned()
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

/// The cover an album without art gets: its title and artist.
pub fn album_cover(album: &library::Album) -> Generated<'_> {
    Generated { title: &album.name, byline: album.artist.as_deref(), seed: crate::source::identity::bare(&album.id) }
}

/// The cover a track without art gets: its album's, else its own title and
/// artist (in its album's colour, so an album's tracks agree).
pub fn track_cover<'a>(album: Option<&'a library::Album>, track: &'a library::Track) -> Generated<'a> {
    match album {
        Some(a) => album_cover(a),
        None => Generated { title: &track.title, byline: track.artist.as_deref(), seed: crate::source::identity::bare(&track.album_id) },
    }
}

/// The cached file for `cover` at `size`, else the server's (cached on the
/// way); `None` when neither has it, or there's no server to ask.
async fn load(source: Option<&Src>, dir: &Path, cover: &str, size: u32) -> Option<ColorImage> {
    let bytes = match read_cached(dir, cover, size).await {
        Some(b) => b,
        None => fetch_cached(source?, dir, cover, size).await?,
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
///
/// The cover is saved only if the account's folder exists. [`Art::set_source`]
/// makes it; once a removed account's folder is gone, a download still in
/// flight for it fails to save instead of bringing the folder back for
/// nothing to clean up.
pub async fn fetch_cached(source: &Src, dir: &Path, cover: &str, size: u32) -> Option<Vec<u8>> {
    let bytes = source.cover(cover, size).await.ok()?;
    let path = cover_file(dir, cover, size);
    // Saved by a task of its own: a load aborted from here on (scrolled past)
    // still leaves the cover cached, and never a half-written file.
    tokio::spawn(async move {
        let tmp = path.with_extension("part");
        if tokio::fs::write(&tmp, &bytes).await.is_ok() {
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

    fn source_with_cover(account: AccountId, id: &str, bytes: Vec<u8>) -> Src {
        let mut fake = crate::source::fake::Fake::new(crate::source::Capabilities::none());
        fake.account = account;
        fake.covers.lock().unwrap().insert(id.into(), bytes);
        Arc::new(fake)
    }

    #[test]
    fn the_same_photo_for_two_artists_is_the_servers_stand_in() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut art = Art::new(rt.handle().clone(), egui::Context::default(), dir.path().to_owned(), None);
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
        let account = AccountId::new();
        let account_cache = account_dir(dir.path(), account);
        std::fs::create_dir_all(&account_cache).unwrap();
        // A 64 px cover already on disk from an earlier session.
        let png = crate::test_tracks::png([200, 40, 40, 255]);
        let png_len = png.len() as u64;
        std::fs::write(cover_file(&account_cache, "al-1", 64), png).unwrap();
        let ctx = egui::Context::default();
        let mut art = Art::new(rt.handle().clone(), ctx, dir.path().to_owned(), Some(account));
        // Never connected: no source at all.
        testkit::wait_until("cached art to load offline", || {
            art.poll();
            art.texture("al-1", 40.0).is_some()
        });
        // Art that isn't cached can't load without a server, and isn't asked for again.
        let deadline = testkit::Deadline::new();
        while !art.covers.get("al-2").is_some_and(|&cover| art.missing.contains(&Key { cover, size: 64 })) {
            art.poll();
            assert!(art.texture("al-2", 40.0).is_none());
            assert!(!deadline.passed());
            std::thread::sleep(Duration::from_millis(10));
        }
        // Clearing forgets the loaded cover and deletes the file, and the size says so.
        testkit::wait_until("the cache to be counted", || art.disk_size() == Some(png_len));
        art.clear();
        assert!(art.texture("al-1", 40.0).is_none());
        testkit::wait_until("the cache to be cleared", || art.disk_size() == Some(0));
        assert!(!cover_file(&account_cache, "al-1", 64).exists());
    }

    #[test]
    fn a_download_finishing_after_its_accounts_folder_is_removed_does_not_bring_it_back() {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let account = AccountId::new();
        let gone = account_dir(dir.path(), account);
        let bytes = crate::test_tracks::png([1, 2, 3, 255]);
        let source = source_with_cover(account, "al-1", bytes.clone());
        assert_eq!(rt.block_on(fetch_cached(&source, &gone, "al-1", 64)), Some(bytes), "the cover still shows");
        assert!(!gone.exists());
    }

    #[test]
    fn qualified_primary_covers_have_one_owner_even_when_their_bare_id_looks_qualified() {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (one, two) = (AccountId::new(), AccountId::new());
        let bare = accounts::ItemKey::new(two, "cover").to_string();
        let qualified = accounts::ItemKey::new(one, &bare).to_string();
        let source = source_with_cover(one, &bare, crate::test_tracks::png([220, 30, 30, 255]));
        let mut art = Art::new(rt.handle().clone(), egui::Context::default(), dir.path().to_owned(), Some(one));
        art.add_source(source.clone());
        art.set_source(source);
        testkit::wait_until("the qualified primary cover to load", || {
            art.poll();
            art.texture(&qualified, 40.0).is_some()
        });
        assert!(art.sources.is_empty(), "the primary owner is reused and the bare id is not routed a second time");
        assert!(art.covers.contains_key(bare.as_str()));
        assert!(cover_file(&account_dir(dir.path(), one), &bare, 64).is_file());
        art.offline_source(one);
        assert!(art.source.is_none());
        assert!(art.loaded(&qualified).is_some(), "offline keeps its cached cover");
    }

    #[test]
    fn adding_sources_splits_one_texture_budget_and_removing_them_restores_the_share() {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (one, two, three) = (AccountId::new(), AccountId::new(), AccountId::new());
        let weight = 64 * 64 * 4;
        let mut fake = crate::source::fake::Fake::new(crate::source::Capabilities::none());
        fake.account = one;
        for id in ["old", "new"] {
            fake.covers.lock().unwrap().insert(id.into(), crate::test_tracks::png([220, 30, 30, 255]));
        }
        let mut art = Art::with_budget(
            rt.handle().clone(),
            egui::Context::default(),
            dir.path().to_owned(),
            Some(one),
            Budget { texture: 2 * weight, disk: DISK_BUDGET },
        );
        art.add_source(Arc::new(fake));
        let qualified = |account, id| accounts::ItemKey::new(account, id).to_string();
        for id in ["old", "new"] {
            let id = qualified(one, id);
            testkit::wait_until("the primary cover to load", || {
                art.poll();
                art.texture(&id, 40.0).is_some()
            });
        }
        art.add_source(source_with_cover(two, "new", crate::test_tracks::png([30, 30, 220, 255])));
        assert!(art.loaded(&qualified(one, "old")).is_none(), "the old texture is evicted when the primary's share shrinks");
        assert!(art.loaded(&qualified(one, "new")).is_some());
        testkit::wait_until("the second account's cover to load", || {
            art.poll();
            art.texture(&qualified(two, "new"), 40.0).is_some()
        });
        assert_ne!(art.loaded(&qualified(one, "new")).unwrap().id(), art.loaded(&qualified(two, "new")).unwrap().id());
        art.add_source(source_with_cover(three, "new", crate::test_tracks::png([30, 220, 30, 255])));
        for account in [one, two, three] {
            assert!(art.loaded(&qualified(account, "new")).is_none(), "no share can retain even one oversized texture");
        }
        testkit::wait_until("the oversized image to be rejected", || {
            art.poll();
            art.texture(&qualified(two, "new"), 40.0);
            art.is_missing(&qualified(two, "new"), 40.0)
        });
        art.remove_source(one);
        art.remove_source(three);
        testkit::wait_until("the remaining account to reclaim the budget", || {
            art.poll();
            art.texture(&qualified(two, "new"), 40.0).is_some()
        });
        assert_eq!(art.sources[&two].budget.texture, 2 * weight);
    }

    #[test]
    fn each_account_folder_is_trimmed_once_under_a_shared_disk_budget() {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (one, two) = (AccountId::new(), AccountId::new());
        for account in [one, two] {
            let folder = account_dir(dir.path(), account);
            std::fs::create_dir_all(&folder).unwrap();
            for id in ["old", "new"] {
                std::fs::write(cover_file(&folder, id, 64), vec![0; 50]).unwrap();
            }
        }
        let mut art =
            Art::with_budget(rt.handle().clone(), egui::Context::default(), dir.path().to_owned(), Some(one), Budget { texture: TEXTURE_BUDGET, disk: 100 });
        art.add_account(one);
        art.add_account(two);
        assert!(art.source.is_none() && art.sources[&two].source.is_none(), "offline accounts still share the disk limit");
        let bytes = |account| shell::art::cached_files(&account_dir(dir.path(), account)).iter().map(|f| f.1).sum::<u64>();
        testkit::wait_until("both folders to fit their shares", || bytes(one) == 50 && bytes(two) == 50);
        assert!(!art.sources.contains_key(&one), "exactly one owner manages the primary folder");
        testkit::wait_until("the total disk use to be reported", || art.disk_size() == Some(100));
        art.remove_source(one);
        for id in ["old", "new"] {
            std::fs::write(cover_file(&account_dir(dir.path(), two), id, 64), vec![0; 50]).unwrap();
        }
        art.set_source(source_with_cover(two, "unused", Vec::new()));
        assert!(art.account.is_none(), "the existing second owner is reused");
        assert_eq!(art.sources[&two].budget.disk, 100);
        assert_eq!(bytes(two), 100);
        assert_eq!(bytes(one), 50, "removing a source stops managing its old folder");
    }

    #[test]
    fn two_accounts_with_the_same_cover_id_do_not_collide() {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (one, two) = (AccountId::new(), AccountId::new());
        let (red, blue) = (crate::test_tracks::png([220, 30, 30, 255]), crate::test_tracks::png([30, 30, 220, 255]));
        let mut art = Art::new(rt.handle().clone(), egui::Context::default(), dir.path().to_owned(), Some(one));
        art.set_source(source_with_cover(one, "al-1", red.clone()));
        testkit::wait_until("account one's cover to load", || {
            art.poll();
            art.texture("al-1", 40.0).is_some()
        });
        let one_file = cover_file(&account_dir(dir.path(), one), "al-1", 64);
        testkit::wait_until("account one's cover to land on disk", || one_file.exists());
        assert_eq!(std::fs::read(&one_file).unwrap(), red);
        assert!(!cover_file(dir.path(), "al-1", 64).exists(), "nothing is written flat under art/");

        // Switching to another account drops in-memory textures and reads/writes its own folder.
        art.set_source(source_with_cover(two, "al-1", blue.clone()));
        assert!(art.texture("al-1", 40.0).is_none(), "never reuses the previous account's texture");
        testkit::wait_until("account two's cover to load", || {
            art.poll();
            art.texture("al-1", 40.0).is_some()
        });
        let two_file = cover_file(&account_dir(dir.path(), two), "al-1", 64);
        testkit::wait_until("account two's cover to land on disk", || two_file.exists());
        assert_eq!(std::fs::read(&two_file).unwrap(), blue);
        assert_eq!(std::fs::read(&one_file).unwrap(), red, "account one's cover on disk stays intact");

        // Clearing account two leaves account one's cached art intact.
        art.clear();
        testkit::wait_until("account two's cache to clear", || !two_file.exists());
        assert_eq!(std::fs::read(&one_file).unwrap(), red);
    }
}
