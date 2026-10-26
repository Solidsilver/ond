//! Offline downloads: the store (opened in the background when the account
//! changes or the folder moves), what's kept and waiting to be kept, and the
//! messages and actions about them.

use super::{Action, OndApp, Session};
use crate::bridge::{Bridge, ListKey, Msg};
use crate::prefs::Prefs;
use library::Track;
use offline::{OfflineStore, Pin, TrackRef};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use subsonic::Client;

/// What a download action applies to, as the user asked for it. The
/// offline store keeps it as `offline::Pin`s: an album or playlist as one,
/// songs one each. `browse::offline_item` is the menu item that asks.
#[derive(Debug, Clone)]
pub enum Keep {
    Album(String),
    Playlist(String),
    Tracks(Vec<Track>),
}

/// A download action.
pub enum OfflineAction {
    /// Keep (true) or stop keeping something on this computer.
    Keep(Keep, bool),
    /// Stop keeping a pin (from the Offline place, where tracks may not be loaded).
    Unpin(Pin),
    Pause(bool),
    Retry,
    /// Cap on offline storage in bytes; `None` is unlimited.
    SetLimit(Option<u64>),
    /// Show the downloads folder.
    Reveal,
    /// Show the open store's folder, where files kept after its list of
    /// downloads couldn't be read are (see `offline::SetAside`).
    RevealSetAside,
    /// Delete those files and the unreadable list, so the folder is tidied again.
    RemoveSetAside,
    /// Pick a new folder for downloads (they move at the next launch).
    Move,
}

impl OfflineAction {
    /// Needs the store: waits while it opens. (Not `RemoveSetAside`: the
    /// store that opens may be another account's.)
    fn needs_store(&self) -> bool {
        matches!(self, OfflineAction::Keep(..) | OfflineAction::Unpin(_) | OfflineAction::Pause(_) | OfflineAction::Retry | OfflineAction::SetLimit(_))
    }
}

/// Whether `track` can play right now: downloaded, or the server is there.
pub(super) fn playable(session: &Session, downloads: &Downloads, track: &Track) -> bool {
    session.online || downloads.snap.is_done(&track.id)
}

/// Offline downloads, and their state as of this frame.
pub struct Downloads {
    /// The open store. `None` while one is being opened off the UI thread
    /// (another account signed in, or the downloads folder is moving), or if
    /// that failed: meanwhile nothing plays from disk and download actions
    /// wait in `deferred`.
    pub store: Option<OfflineStore>,
    pub snap: Arc<offline::Snapshot>,
    /// Pins waiting for their track lists to load, by the same keys as `pending`.
    pub pending_pins: Vec<(ListKey, Pin)>,
    /// The downloads folder; each account's store is a folder inside it.
    pub base: PathBuf,
    /// Whose store is open, or opening (`None` before anyone has signed in).
    account: Option<u32>,
    /// Numbers the opens started. A store that arrives for an older one (the
    /// user switched again meanwhile) is closed unused.
    generation: u64,
    /// An open is under way.
    pub(super) opening: bool,
    /// Download actions taken while the store was opening, run once it's open.
    pub(super) deferred: Vec<OfflineAction>,
    /// The notice showing the downloads folder's move, while it moves.
    moving: Option<String>,
    pub(super) open: OpenStore,
}

/// Opens the offline store kept in a folder, with an optional size limit.
type OpenStore = Arc<dyn Fn(PathBuf, Option<u64>) -> anyhow::Result<OfflineStore> + Send + Sync>;

/// A move's progress notice is updated at most this often.
const MOVE_PROGRESS_EVERY: Duration = Duration::from_millis(500);

impl Downloads {
    /// How a store is opened: in a folder, with a size limit, the stream
    /// cache to link finished streams from, and the window to wake.
    pub(super) fn opener(rt: tokio::runtime::Handle, ctx: egui::Context, cache: streamcache::StreamCache) -> OpenStore {
        Arc::new(move |dir, limit| {
            let repaint = ctx.clone();
            let store = OfflineStore::open(offline::OfflineConfig { dir, limit }, rt.clone(), Arc::new(move || repaint.request_repaint()))?;
            // A track that finished streaming is linked in rather than downloaded again.
            let cached = cache.clone();
            store.set_cache_lookup(Arc::new(move |r: &TrackRef| cached.complete_path(&streamcache::CacheKey::original(r.id.clone(), r.suffix.clone()))));
            Ok(store)
        })
    }

    /// The downloads at start-up: `account`'s store under `base`, opened now,
    /// or after moving the downloads from `move_from` in the background.
    pub(super) fn start(base: PathBuf, account: Option<u32>, limit: Option<u64>, move_from: Option<PathBuf>, open: OpenStore, bridge: &Bridge, notices: &mut Vec<String>) -> anyhow::Result<Self> {
        let mut downloads = Downloads {
            store: None,
            snap: Arc::default(),
            pending_pins: Vec::new(),
            base,
            account,
            generation: 0,
            opening: false,
            deferred: Vec::new(),
            moving: None,
            open,
        };
        match move_from {
            // A new location chosen last time: the downloads move there
            // first, in the background (a copy across drives takes a while).
            Some(from) => {
                let notice = format!("Moving downloads to {}…", downloads.base.display());
                notices.push(notice.clone());
                downloads.moving = Some(notice);
                downloads.open_in_background(limit, Some(from), bridge);
            }
            None => {
                let store = (downloads.open)(Downloads::dir_for(&downloads.base, account)?, limit)?;
                downloads.store = Some(store);
                downloads.refresh();
            }
        }
        Ok(downloads)
    }

    /// The store folder for `account` under `base`. Before any sign-in there is
    /// no account; an empty store stands in until one connects.
    pub(super) fn dir_for(base: &Path, account: Option<u32>) -> std::io::Result<PathBuf> {
        match account {
            Some(a) => offline::account_store(base, a),
            None => Ok(base.join("signed-out")),
        }
    }

    /// Opens `account`'s store in place of the current one (another server or
    /// user signed in), in the background (see [`open_in_background`](Self::open_in_background)).
    /// Its pins and files stay apart from everyone else's.
    pub(super) fn switch_account(&mut self, account: Option<u32>, limit: Option<u64>, bridge: &Bridge) {
        if account == self.account && (self.store.is_some() || self.opening) {
            return;
        }
        self.account = account;
        self.open_in_background(limit, None, bridge);
    }

    /// Shuts the current store down and opens `self.account`'s, on the
    /// bridge's offline queue: reading its pins and checking its files takes a
    /// while with thousands of downloads. With `move_from`, the downloads are
    /// first moved from there into `base`. The store arrives as
    /// `Msg::OfflineStore`, for [`opened`](Self::opened).
    pub(super) fn open_in_background(&mut self, limit: Option<u64>, move_from: Option<PathBuf>, bridge: &Bridge) {
        self.generation += 1;
        self.opening = true;
        self.pending_pins.clear();
        self.snap = Arc::default();
        let old = self.store.take();
        let (generation, base, account, open) = (self.generation, self.base.clone(), self.account, self.open.clone());
        bridge.offline_job(move |b| {
            // First, as it may be the same folder's: this waits for its last save.
            drop(old);
            if let Some(from) = move_from {
                let mut posted: Option<std::time::Instant> = None;
                let moved = offline::move_store_with(&from, &base, &mut |p| {
                    if posted.is_none_or(|at| at.elapsed() >= MOVE_PROGRESS_EVERY) {
                        posted = Some(std::time::Instant::now());
                        b.post(Msg::OfflineMoving(p));
                    }
                });
                if let Err(e) = &moved {
                    tracing::warn!("moving downloads from {}: {e}", from.display());
                }
                b.post(Msg::OfflineMoved(moved.map_err(|e| e.to_string())));
            }
            let store = Self::dir_for(&base, account).map_err(anyhow::Error::from).and_then(|dir| open(dir, limit));
            b.post(Msg::OfflineStore { generation, store: store.map_err(|e| format!("{e:#}")) });
        });
    }

    /// A store opened in the background arrived. Returns whether it's the one
    /// waited for, now in use; one for an older open is shut down (off this
    /// thread, in turn) instead.
    pub(super) fn opened(&mut self, generation: u64, store: Result<OfflineStore, String>, bridge: &Bridge) -> Result<bool, String> {
        if generation != self.generation {
            if let Ok(store) = store {
                bridge.offline_job(move |_| drop(store));
            }
            return Ok(false);
        }
        self.opening = false;
        self.store = Some(store?);
        self.refresh();
        Ok(true)
    }

    /// The finished file for track `id`, if there is one.
    pub fn local_path(&self, id: &str) -> Option<PathBuf> {
        self.store.as_ref()?.local_path(id)
    }

    /// Takes in what changed since the last frame.
    pub fn refresh(&mut self) {
        if let Some(store) = &self.store {
            self.snap = store.snapshot();
        }
    }

    /// The open store (tests).
    #[cfg(test)]
    pub fn store(&self) -> &OfflineStore {
        self.store.as_ref().expect("the offline store is open")
    }

    /// Applies a download action. Returns a notice to show, or an error.
    pub(super) fn apply(&mut self, action: OfflineAction, prefs: &Prefs) -> Result<Option<String>, String> {
        let store = || self.store.clone().ok_or_else(|| "Downloads aren't open".to_owned());
        match action {
            OfflineAction::Unpin(pin) => {
                store()?.unpin(&pin);
                self.refresh();
            }
            OfflineAction::Pause(paused) => store()?.set_paused(paused),
            OfflineAction::Retry => store()?.retry_failed(),
            OfflineAction::SetLimit(bytes) => {
                store()?.set_limit(bytes);
                prefs.edit(|s| s.offline_limit_gb = bytes.map(|b| b / 1_000_000_000));
            }
            OfflineAction::Reveal => {
                crate::folder::reveal(&self.base).map_err(|e| format!("open {}: {e}", self.base.display()))?;
            }
            OfflineAction::RevealSetAside => {
                let store = store()?;
                crate::folder::reveal(store.dir()).map_err(|e| format!("open {}: {e}", store.dir().display()))?;
            }
            OfflineAction::RemoveSetAside => {
                store()?.remove_set_aside();
                self.refresh();
            }
            _ => unreachable!("not a download action"),
        }
        Ok(None)
    }

    /// Moves downloads into `chosen` at the next start. Returns the notice to show.
    pub(super) fn move_to(&self, chosen: PathBuf, prefs: &Prefs) -> Option<String> {
        // Always a folder of our own inside the chosen one: the store tidies
        // files it doesn't know, which must never be the user's.
        let dir = if chosen.file_name().is_some_and(|n| n == "Ond Offline") { chosen } else { chosen.join("Ond Offline") };
        if dir == self.base {
            return None;
        }
        if self.moving.is_some() {
            // Its source would be forgotten if Ond quit before the move ends.
            return Some("Downloads are still moving. Choose another folder once they're done.".into());
        }
        let current = self.base.clone();
        prefs.edit(|s| {
            s.offline_dir = Some(dir.clone());
            s.offline_move_from = Some(current);
        });
        Some(format!("Downloads move to {} the next time Ond starts", dir.display()))
    }
}

/// What the offline store needs to download `t`.
pub fn track_ref(t: &Track) -> TrackRef {
    TrackRef::new(t.id.clone(), t.suffix.clone(), t.size)
}

impl OndApp {
    /// Applies a download action. One that needs the store waits while it opens.
    pub(super) fn apply_offline(&mut self, action: OfflineAction) {
        if self.downloads.store.is_none() && action.needs_store() {
            if self.downloads.opening {
                self.downloads.deferred.push(action);
            } else {
                self.show_error("Downloads couldn't be opened. Restart Ond to try again.");
            }
            return;
        }
        match action {
            OfflineAction::Keep(what, on) => self.keep_offline(what, on),
            OfflineAction::Move => self.choose_offline_folder(),
            other => match self.downloads.apply(other, &self.prefs) {
                Ok(Some(notice)) => self.show_notice(notice),
                Ok(None) => {}
                Err(e) => self.show_error(e),
            },
        }
    }

    /// A message about the offline store: it opened, or its folder moved.
    pub(super) fn offline_msg(&mut self, msg: Msg) {
        match msg {
            Msg::OfflineFolder(dir) => {
                if let Some(notice) = self.downloads.move_to(dir, &self.prefs) {
                    self.show_notice(notice);
                }
            }
            Msg::OfflineStore { generation, store } => match self.downloads.opened(generation, store, &self.bridge) {
                Ok(true) => {
                    if let Some(client) = self.session.live().cloned() {
                        self.connect_downloads(&client);
                    }
                    // Download actions taken while it opened.
                    self.actions.extend(self.downloads.deferred.drain(..).map(Action::Offline));
                    if !self.session.online {
                        // What's downloaded can play again.
                        self.sync_next();
                    }
                }
                Ok(false) => {}
                Err(e) => {
                    self.downloads.deferred.clear();
                    self.show_error(format!("Downloads couldn't be opened: {e}"));
                }
            },
            Msg::OfflineMoving(p) => {
                let text = format!("Moving downloads to {}: {} of {}", self.downloads.base.display(), crate::fmt::bytes(p.done), crate::fmt::bytes(p.total));
                self.moving_notice(text, false);
            }
            Msg::OfflineMoved(result) => {
                let base = self.downloads.base.display().to_string();
                match result {
                    Ok(()) => {
                        self.prefs.edit(|s| s.offline_move_from = None);
                        self.moving_notice(format!("Downloads moved to {base}"), false);
                    }
                    // The pref stays: the next start carries on where this stopped.
                    Err(e) => self.moving_notice(format!("Some downloads couldn't be moved to {base} ({e}). Ond tries again next time it starts."), true),
                }
                self.downloads.moving = None;
            }
            _ => unreachable!("not an offline message"),
        }
    }

    /// Takes in the store's changes since the last frame. A download that
    /// finished may let what's next play from disk, or play at all.
    pub(super) fn poll_downloads(&mut self) {
        if let Some(snap) = self.downloads.store.as_ref().map(|s| s.snapshot())
            && snap.version != self.downloads.snap.version
        {
            // A download finished: what's next may now play from disk, or play at all.
            let done_changed = snap.tracks.values().filter(|s| matches!(s, offline::TrackState::Done { .. })).count()
                != self.downloads.snap.tracks.values().filter(|s| matches!(s, offline::TrackState::Done { .. })).count();
            self.downloads.snap = snap;
            if done_changed && !self.session.online {
                self.sync_next();
            }
        }
    }

    /// Hands the offline store the server, and refetches pinned playlists so
    /// what was added to them downloads. Nothing while the store is opening:
    /// it gets the same once it's open.
    pub(super) fn connect_downloads(&mut self, client: &Client) {
        let Some(store) = &self.downloads.store else { return };
        let url = client.clone();
        store.set_source(Some(Arc::new(move |id: &str| url.stream_url(id).to_string())));
        for p in &self.downloads.snap.pins {
            if let Pin::Playlist(id) = &p.pin {
                self.bridge.open_playlist(Some(client.clone()), id.clone());
            }
        }
    }

    /// Shows `text` in place of the move's last notice.
    pub(super) fn moving_notice(&mut self, text: String, error: bool) {
        let old = self.downloads.moving.replace(text.clone());
        self.ui.toasts.replace(old.as_deref(), text, error);
    }

    /// Pins `pin` if it was waiting for these tracks, or refreshes its track
    /// list if it's already pinned. Only `complete` lists (the server's own
    /// answer for that album or playlist) may drop tracks and delete their
    /// downloads; anything else (the local library, which may be mid-sync or
    /// rebuilt) can only add.
    pub(super) fn refresh_pin(&mut self, pin: Pin, tracks: &[Track], complete: bool) {
        // While a store opens there's nothing to refresh: its pins aren't
        // known yet, and none waits (opening clears `pending_pins`).
        let Some(store) = self.downloads.store.clone() else { return };
        let label = self.pin_label(&pin);
        let refs = || tracks.iter().map(track_ref).collect::<Vec<_>>();
        if let Some(at) = self.downloads.pending_pins.iter().position(|(_, p)| p == &pin) {
            self.downloads.pending_pins.remove(at);
            store.pin(pin, label, refs());
        } else if store.is_pinned(&pin) {
            if complete {
                store.update(&pin, label, refs());
            } else {
                store.extend(&pin, refs());
            }
        }
    }

    pub(super) fn pin_label(&self, pin: &Pin) -> String {
        match pin {
            Pin::Album(id) => self.data.album(id).map(|a| a.name.clone()).unwrap_or_default(),
            Pin::Playlist(id) => self.data.playlists.iter().find(|p| &p.id == id).map(|p| p.name.clone()).unwrap_or_default(),
            Pin::Track(id) => self.data.track(id).map(|t| t.title.clone()).unwrap_or_default(),
        }
    }

    /// Runs only with the store open (see `apply_offline`, which holds it back while one opens).
    pub(super) fn keep_offline(&mut self, what: Keep, on: bool) {
        let Some(store) = self.downloads.store.clone() else { return };
        // An album or playlist is pinned whole, once its track list is known.
        let (key, pin, refs) = match what {
            Keep::Album(id) => {
                let refs = self.data.tracks.album(&id).map(|t| t.iter().map(track_ref).collect::<Vec<_>>());
                (ListKey::Album(id.clone()), Pin::Album(id), refs)
            }
            Keep::Playlist(id) => {
                let refs = self.data.playlist_tracks.get(&id).map(|t| t.iter().map(track_ref).collect::<Vec<_>>());
                (ListKey::Playlist(id.clone()), Pin::Playlist(id), refs)
            }
            // One pin per song, saved together: a big selection doesn't stall.
            Keep::Tracks(tracks) => {
                if on {
                    store.pin_many(tracks.iter().map(|t| (Pin::Track(t.id.clone()), t.title.clone(), vec![track_ref(t)])).collect());
                } else {
                    store.unpin_many(&tracks.iter().map(|t| Pin::Track(t.id.clone())).collect::<Vec<_>>());
                }
                self.downloads.refresh();
                return;
            }
        };
        match refs {
            _ if !on => store.unpin(&pin),
            Some(refs) => store.pin(pin.clone(), self.pin_label(&pin), refs),
            None => {
                self.wait_to_pin(key.clone(), pin);
                match key {
                    ListKey::Album(id) => self.bridge.open_album(self.session.client.clone(), id),
                    ListKey::Playlist(id) => self.bridge.open_playlist(self.session.client.clone(), id),
                    ListKey::Artist(_) => {}
                }
            }
        }
        self.downloads.refresh();
    }

    /// Pins `pin` once its tracks load (once, however often it's asked).
    pub(super) fn wait_to_pin(&mut self, key: ListKey, pin: Pin) {
        if !self.downloads.pending_pins.iter().any(|(k, _)| *k == key) {
            self.downloads.pending_pins.push((key, pin));
        }
    }

    /// Asks where downloads should go from now on (they move at the next start).
    pub(super) fn choose_offline_folder(&mut self) {
        let base = &self.downloads.base;
        let start = base.parent().unwrap_or(base).to_owned();
        let bridge = self.bridge.clone();
        crate::folder::pick("Choose where to keep downloaded music", &start, move |chosen| {
            if let Some(dir) = chosen {
                bridge.post(Msg::OfflineFolder(dir));
            }
        });
    }
}
