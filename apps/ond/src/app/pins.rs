//! Offline downloads: the store (opened in the background when the account
//! changes or the folder moves), what's kept and waiting to be kept, and the
//! messages and actions about them.

use super::{Action, OndApp, Session};
use crate::bridge::{Bridge, ListKey, OfflineMsg};
use crate::prefs::Prefs;
use crate::source::{Src, TrackKey, stream_name};
use accounts::AccountId;
use library::Track;
use offline::{OfflineStore, Pin, TrackRef};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

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
    /// Work offline (true) or go back online.
    WorkOffline(bool),
    /// What playback does offline with a song that isn't downloaded.
    Unplayable(crate::settings::Unplayable),
}

impl OfflineAction {
    /// Needs the store: waits while it opens. (Not `RemoveSetAside`: the
    /// store that opens may be another account's.)
    fn needs_store(&self) -> bool {
        matches!(self, OfflineAction::Keep(..) | OfflineAction::Unpin(_) | OfflineAction::Pause(_) | OfflineAction::Retry | OfflineAction::SetLimit(_))
    }
}

/// Whether `track` can play right now: the server is there (or connecting),
/// or it's downloaded, or it was streamed whole and is still cached.
/// `transcode` is what streams are converted to (see `Outputs::transcode`),
/// worked out once by the caller rather than for each track.
pub(super) fn playable(
    session: &Session,
    downloads: &Downloads,
    cache: &streamcache::StreamCache,
    transcode: Option<&audio::Transcode>,
    track: &Track,
) -> bool {
    session.for_item(&track.id).expected()
        || session.for_item(&track.id).account().is_some_and(|account| {
            let key = crate::source::identity::key(account, &track.id);
            downloads.tracks().is_done(&key) || crate::local::is_cached(cache, transcode, &key, track)
        })
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
    pub(crate) owner: Option<AccountId>,
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
    /// Where each kept album is in `snap.pins`, for the snapshot it was
    /// made from (see [`album_pin`](Self::album_pin)).
    album_pins: (Arc<offline::Snapshot>, HashMap<String, usize>),
}

/// Opens the offline store kept in a folder, with an optional size limit.
type OpenStore = Arc<dyn Fn(PathBuf, Option<u64>, Option<AccountId>) -> anyhow::Result<OfflineStore> + Send + Sync>;

/// A move's progress notice is updated at most this often.
const MOVE_PROGRESS_EVERY: Duration = Duration::from_millis(500);

impl Downloads {
    /// Indexes the kept albums of the snapshot in use, if it changed since
    /// (the app calls this before lending the downloads to a view).
    pub(super) fn index_album_pins(&mut self) {
        if Arc::ptr_eq(&self.album_pins.0, &self.snap) {
            return;
        }
        let at = self.snap.pins.iter().enumerate().filter_map(|(i, p)| if let Pin::Album(id) = &p.pin { Some((id.clone(), i)) } else { None }).collect();
        self.album_pins = (Arc::clone(&self.snap), at);
    }

    /// The album's download, when it's kept offline. A lookup, not a walk
    /// through the pins: the grid asks for every tile, every frame. Before
    /// [`index_album_pins`](Self::index_album_pins) catches up with a new
    /// snapshot, it walks them.
    pub fn album_pin(&self, id: &str) -> Option<&offline::PinSummary> {
        if Arc::ptr_eq(&self.album_pins.0, &self.snap) {
            return self.album_pins.1.get(id).and_then(|&i| self.snap.pins.get(i));
        }
        self.snap.pins.iter().find(|p| matches!(&p.pin, Pin::Album(a) if a == id))
    }

    /// How a store is opened: in a folder, with a size limit, the stream
    /// cache to link finished streams from, and the window to wake.
    pub(super) fn opener(rt: tokio::runtime::Handle, ctx: egui::Context, cache: streamcache::StreamCache) -> OpenStore {
        Arc::new(move |dir, limit, account| {
            let repaint = ctx.clone();
            if let Some(account) = account {
                crate::storage::qualify_downloads(&dir, account)?;
            }
            let cfg = offline::OfflineConfig { dir, limit, reply_check: crate::bridge::reply_check() };
            let store = OfflineStore::open(cfg, rt.clone(), Arc::new(move || repaint.request_repaint()))?;
            // A track that finished streaming is linked in rather than downloaded again.
            let cached = cache.clone();
            store.set_cache_lookup(Arc::new(move |r: &TrackRef| {
                let key = TrackKey::parse(&r.id).ok().filter(|k| Some(k.account) == account)?;
                cached.complete_path(&streamcache::CacheKey::original(stream_name(&key).into_owned(), r.suffix.clone()))
            }));
            Ok(store)
        })
    }

    /// The downloads at start-up: `account`'s store under `base`, opened now,
    /// or after moving the downloads from `move_from` in the background.
    pub(super) fn start(
        base: PathBuf,
        settings: &crate::settings::Settings,
        open: OpenStore,
        bridge: &Bridge,
        notices: &mut Vec<String>,
    ) -> anyhow::Result<Self> {
        let (account, owner) = (settings.account_id(), settings.account());
        let limit = settings.offline_limit_gb.map(|gb| gb * 1_000_000_000);
        let move_from = settings.offline_move_from.clone();
        let mut downloads = Downloads {
            store: None,
            snap: Arc::default(),
            pending_pins: Vec::new(),
            base,
            account,
            owner,
            generation: 0,
            opening: false,
            deferred: Vec::new(),
            moving: None,
            open,
            album_pins: Default::default(),
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
                let store = (downloads.open)(Downloads::dir_for(&downloads.base, account)?, limit, owner)?;
                downloads.store = Some(store);
                downloads.refresh();
            }
        }
        Ok(downloads)
    }

    pub(super) fn for_source(&self, settings: &crate::settings::Settings, bridge: &Bridge) -> Self {
        let mut downloads = Self {
            store: None,
            snap: Arc::default(),
            pending_pins: Vec::new(),
            base: self.base.clone(),
            account: settings.account_id(),
            owner: settings.account(),
            generation: 0,
            opening: false,
            deferred: Vec::new(),
            moving: None,
            open: self.open.clone(),
            album_pins: Default::default(),
        };
        downloads.open_in_background(settings.offline_limit_gb.map(|gb| gb * 1_000_000_000), None, bridge);
        downloads
    }

    pub(super) fn remove_files(&mut self, bridge: &Bridge) {
        let store = self.store.take();
        self.snap = Arc::default();
        self.pending_pins.clear();
        self.deferred.clear();
        self.opening = false;
        if let Some(store) = store {
            let path = store.dir().to_path_buf();
            bridge.offline_job(move |_| {
                drop(store);
                let _ = std::fs::remove_dir_all(path);
            });
        }
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
    pub(super) fn switch_account(&mut self, account: Option<u32>, owner: Option<AccountId>, limit: Option<u64>, bridge: &Bridge) {
        if account == self.account && owner == self.owner && (self.store.is_some() || self.opening) {
            return;
        }
        self.account = account;
        self.owner = owner;
        self.open_in_background(limit, None, bridge);
    }

    /// Shuts the current store down and opens `self.account`'s, on the
    /// bridge's offline queue: reading its pins and checking its files takes a
    /// while with thousands of downloads. With `move_from`, the downloads are
    /// first moved from there into `base`. The store arrives as
    /// `OfflineMsg::Store`, for [`opened`](Self::opened).
    pub(super) fn open_in_background(&mut self, limit: Option<u64>, move_from: Option<PathBuf>, bridge: &Bridge) {
        self.generation += 1;
        self.opening = true;
        self.pending_pins.clear();
        self.snap = Arc::default();
        let old = self.store.take();
        let (generation, base, account, owner, open) = (self.generation, self.base.clone(), self.account, self.owner, self.open.clone());
        bridge.offline_job(move |b| {
            // First, as it may be the same folder's: this waits for its last save.
            drop(old);
            if let Some(from) = move_from {
                let mut posted: Option<std::time::Instant> = None;
                let moved = offline::move_store_with(&from, &base, &mut |p| {
                    if posted.is_none_or(|at| at.elapsed() >= MOVE_PROGRESS_EVERY) {
                        posted = Some(std::time::Instant::now());
                        b.post(OfflineMsg::Moving(p));
                    }
                });
                if let Err(e) = &moved {
                    tracing::warn!("moving downloads from {}: {e}", from.display());
                }
                b.post(OfflineMsg::Moved(moved.map(drop).map_err(|e| e.to_string())));
            }
            let store = Self::dir_for(&base, account).map_err(anyhow::Error::from).and_then(|dir| open(dir, limit, owner));
            b.post(OfflineMsg::Store { generation, store: store.map_err(|e| format!("{e:#}")) });
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

    /// The finished file for this account's track, if there is one.
    pub fn local_path(&self, key: &TrackKey) -> Option<PathBuf> {
        self.tracks().local_path(key)
    }

    pub fn tracks(&self) -> OfflineTracks<'_> {
        OfflineTracks { account: self.owner, snap: &self.snap, store: self.store.as_ref() }
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

    /// Applies a download action that needs only the store (or the folder).
    pub(super) fn apply(&mut self, action: OfflineAction, prefs: &Prefs) -> Result<(), String> {
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
            // The app applies these itself (`apply_offline`); listed so a new
            // variant has to be placed in one or the other.
            OfflineAction::Keep(..) | OfflineAction::Move | OfflineAction::WorkOffline(_) | OfflineAction::Unplayable(_) => {
                unreachable!("applied by the app; see apply_offline")
            }
        }
        Ok(())
    }

    /// Moves downloads into `chosen` at the next start. Returns the notice to show.
    pub(super) fn move_to(&self, chosen: PathBuf, prefs: &Prefs) -> Option<String> {
        let dir = crate::settings::offline_folder(chosen);
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

/// The qualified key keeps files apart across sources.
pub fn offline_name(key: &TrackKey) -> std::borrow::Cow<'_, str> {
    std::borrow::Cow::Owned(key.to_string())
}

/// The only track boundary into the string-based store. Its account is known
/// even when the server is absent, and a foreign key never names a local file.
pub struct OfflineTracks<'a> {
    pub account: Option<AccountId>,
    pub snap: &'a offline::Snapshot,
    pub store: Option<&'a OfflineStore>,
}

impl<'a> OfflineTracks<'a> {
    /// Resolves a stored name only inside the account that owns this store.
    pub fn key(&self, name: &str) -> Option<TrackKey> {
        TrackKey::parse(name).ok().filter(|key| Some(key.account) == self.account)
    }

    fn name<'k>(&self, key: &'k TrackKey) -> Option<std::borrow::Cow<'k, str>> {
        (Some(key.account) == self.account).then(|| offline_name(key))
    }

    pub fn track(&self, key: &TrackKey) -> Option<&'a offline::TrackState> {
        self.snap.track(&self.name(key)?)
    }

    pub fn is_done(&self, key: &TrackKey) -> bool {
        self.name(key).is_some_and(|name| self.snap.is_done(&name))
    }

    pub fn local_path(&self, key: &TrackKey) -> Option<PathBuf> {
        self.store?.local_path(&self.name(key)?)
    }

    pub fn pin(&self, key: &TrackKey) -> Option<Pin> {
        Some(Pin::Track(self.name(key)?.into_owned()))
    }

    pub fn track_ref(&self, key: &TrackKey, t: &Track) -> Option<TrackRef> {
        Some(TrackRef::new(self.name(key)?.into_owned(), t.suffix.clone(), t.size))
    }
}

impl OndApp {
    /// Applies a download action. One that needs the store waits while it opens.
    pub(super) fn apply_offline(&mut self, action: OfflineAction) {
        if !self.multi.bridges.is_empty() {
            match &action {
                OfflineAction::Keep(Keep::Tracks(tracks), on) => {
                    let mut groups = std::collections::BTreeMap::<AccountId, Vec<Track>>::new();
                    for track in tracks {
                        if let Some(key) = self.session.key(&track.id)
                            && self.session.get(key.account).is_some_and(|s| s.source_caps().downloads)
                        {
                            groups.entry(key.account).or_default().push(track.clone());
                        }
                    }
                    for (account, tracks) in groups {
                        self.with_source(account, |app| app.apply_offline_for_source(OfflineAction::Keep(Keep::Tracks(tracks), *on)));
                    }
                    return;
                }
                OfflineAction::Keep(Keep::Playlist(id), on) if id.starts_with("ond:") => {
                    let accounts: Vec<_> = self.session.accounts.iter().filter(|(_, s)| s.source_caps().downloads).map(|(a, _)| *a).collect();
                    for account in accounts {
                        self.with_source(account, |app| app.apply_offline_for_source(OfflineAction::Keep(Keep::Playlist(id.clone()), *on)));
                    }
                    return;
                }
                _ => {}
            }
        }
        self.apply_offline_for_source(action);
    }

    fn apply_offline_for_source(&mut self, action: OfflineAction) {
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
            OfflineAction::WorkOffline(on) => self.set_work_offline(on),
            OfflineAction::Unplayable(u) => {
                self.prefs.edit(|s| s.offline_unplayable = u);
                self.player.stop_on_unplayable = u == crate::settings::Unplayable::Pause;
            }
            a @ (OfflineAction::Unpin(_)
            | OfflineAction::Pause(_)
            | OfflineAction::Retry
            | OfflineAction::SetLimit(_)
            | OfflineAction::Reveal
            | OfflineAction::RevealSetAside
            | OfflineAction::RemoveSetAside) => {
                if let Err(e) = self.downloads.apply(a, &self.prefs) {
                    self.show_error(e);
                }
            }
        }
    }

    /// A message about the offline store: it opened, or its folder moved.
    pub(super) fn offline_msg(&mut self, msg: OfflineMsg) {
        match msg {
            OfflineMsg::Folder(dir) => {
                if let Some(notice) = self.downloads.move_to(dir, &self.prefs) {
                    self.show_notice(notice);
                }
            }
            OfflineMsg::Store { generation, store } => match self.downloads.opened(generation, store, &self.bridge) {
                Ok(true) => {
                    if let Some(source) = self.session.live().cloned() {
                        self.connect_downloads(&source);
                    }
                    // Download actions taken while it opened.
                    self.actions.extend(self.downloads.deferred.drain(..).map(Action::Offline));
                    if !self.session.online {
                        // What's downloaded can play again.
                        self.sync_next();
                    }
                    if self.session.work_offline {
                        self.load_local();
                    }
                }
                Ok(false) => {}
                Err(e) => {
                    self.downloads.deferred.clear();
                    self.show_error(format!("Downloads couldn't be opened: {e}"));
                    if self.session.work_offline {
                        self.load_local();
                    }
                }
            },
            OfflineMsg::Moving(p) => {
                let text = format!("Moving downloads to {}: {} of {}", self.downloads.base.display(), crate::fmt::bytes(p.done), crate::fmt::bytes(p.total));
                self.moving_notice(text, false);
            }
            OfflineMsg::Moved(result) => {
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
        }
    }

    /// Takes in the store's changes since the last frame. A download that
    /// finished may let what's next play from disk, or play at all.
    pub(super) fn poll_downloads(&mut self) {
        let mut changed = false;
        for downloads in self.multi.downloads.values_mut() {
            let version = downloads.snap.version;
            downloads.refresh();
            changed |= version != downloads.snap.version;
        }
        if changed {
            self.sync_next();
        }
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
    pub(super) fn connect_downloads(&mut self, source: &Src) {
        let Some(store) = &self.downloads.store else { return };
        if self.downloads.owner != Some(source.account()) {
            return;
        }
        let url = source.clone();
        store.set_source(source.capabilities().downloads.then(|| {
            Arc::new(move |id: &str| {
                let key = TrackKey::parse(id).expect("downloads have qualified keys");
                assert_eq!(key.account, url.account());
                url.download_url(&key.item).expect("a downloadable source supplies track URLs")
            }) as offline::UrlFn
        }));
        let mut ond = false;
        for p in &self.downloads.snap.pins {
            if let Pin::Playlist(id) = &p.pin {
                if id.starts_with("ond:") {
                    ond = true;
                } else {
                    self.bridge.open_playlist(Some(source.clone()), id.clone());
                }
            }
        }
        if ond {
            self.refresh_ond_playlists();
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
        let Some(account) = self.session.account().filter(|a| Some(*a) == self.downloads.owner) else { return };
        let snapshot = self.downloads.snap.clone();
        let adapter = OfflineTracks { account: self.downloads.owner, snap: &snapshot, store: Some(&store) };
        let refs = || tracks.iter().filter_map(|t| adapter.track_ref(&crate::source::identity::key(account, &t.id), t)).collect::<Vec<_>>();
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
            Pin::Track(id) => {
                self.data.track(id).or_else(|| TrackKey::parse(id).ok().and_then(|k| self.data.track(&k.item))).map(|t| t.title.clone()).unwrap_or_default()
            }
            Pin::Book(_) => String::new(),
        }
    }

    /// Runs only with the store open (see `apply_offline`, which holds it back while one opens).
    pub(super) fn keep_offline(&mut self, what: Keep, on: bool) {
        let Some(store) = self.downloads.store.clone() else { return };
        let Some(account) = self.session.account().filter(|a| Some(*a) == self.downloads.owner) else { return };
        let adapter = self.downloads.tracks();
        let track_ref = |t: &Track| adapter.track_ref(&crate::source::identity::key(account, &t.id), t);
        // An album or playlist is pinned whole, once its track list is known.
        let (key, pin, refs) = match what {
            Keep::Album(id) => {
                let refs = self.data.tracks.album(&id).map(|t| t.iter().filter_map(track_ref).collect::<Vec<_>>());
                (ListKey::Album(id.clone()), Pin::Album(id), refs)
            }
            Keep::Playlist(id) => {
                let refs = self.data.playlist_tracks.get(&id).map(|t| t.iter().filter_map(track_ref).collect::<Vec<_>>());
                (ListKey::Playlist(id.clone()), Pin::Playlist(id), refs)
            }
            // One pin per song, saved together: a big selection doesn't stall.
            Keep::Tracks(tracks) => {
                if on {
                    store.pin_many(
                        tracks
                            .iter()
                            .filter_map(|t| Some((adapter.pin(&crate::source::identity::key(account, &t.id))?, t.title.clone(), vec![track_ref(t)?])))
                            .collect(),
                    );
                } else {
                    store.unpin_many(&tracks.iter().filter_map(|t| adapter.pin(&crate::source::identity::key(account, &t.id))).collect::<Vec<_>>());
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
                    ListKey::Album(id) => self.bridge.open_album(self.session.source.clone(), id),
                    ListKey::Playlist(id) if id.starts_with("ond:") => self.refresh_ond_playlists(),
                    ListKey::Playlist(id) => self.bridge.open_playlist(self.session.source.clone(), id),
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
        (self.pick_folder)(
            "Choose where to keep downloaded music",
            &start,
            Box::new(move |chosen| {
                if let Some(dir) = chosen {
                    bridge.post(OfflineMsg::Folder(dir));
                }
            }),
        );
    }
}
