//! Background work for the UI. Every function here returns immediately; results
//! arrive later as [`Msg`]s, each followed by a repaint request.

use crate::settings::{Paths, Settings};
use crate::sync;
use library::{Album, Library, Track};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use subsonic::{AlbumListType, Client, Credentials};

pub enum Msg {
    Connected { client: Client, settings: Box<Settings>, server: String },
    ConnectFailed(String),
    Albums(Vec<Album>),
    SyncProgress(usize),
    SyncDone(usize),
    Tracks { album_id: String, tracks: Vec<Track> },
    /// An album row refreshed from `getAlbum` (labels, genres).
    AlbumUpdated(Album),
    /// Ids of the most recently added albums, newest first.
    Newest(Vec<String>),
    Playlists(Vec<subsonic::Playlist>),
    PlaylistTracks { id: String, tracks: Vec<Track> },
    /// Albums whose stored tracks credit `name`.
    Credited { name: String, album_ids: Vec<String> },
    /// Tracks of several albums, gathered for one queue action (`key` says which).
    TrackBatch { key: String, tracks: Vec<Track> },
    /// The server's artists (split from joined credits), sorted.
    Artists(Vec<library::Artist>),
    /// Every stored track, grouped by album; sent after load and after a sync.
    AllTracks(Vec<Track>),
    TrackSync(sync::Progress),
    TrackSyncDone(sync::Outcome),
    Engine(audio::Event),
    /// Media keys, Control Center, headphone buttons.
    Remote(crate::now_playing::Remote),
    Error(String),
}

#[derive(Clone)]
pub struct Bridge {
    rt: tokio::runtime::Handle,
    tx: Sender<Msg>,
    ctx: egui::Context,
    library: Arc<Library>,
    /// Stops the running track sync, if any.
    sync_cancel: Arc<Mutex<Option<Arc<AtomicBool>>>>,
}

impl Bridge {
    pub fn new(rt: tokio::runtime::Handle, tx: Sender<Msg>, ctx: egui::Context, library: Arc<Library>) -> Self {
        Self { rt, tx, ctx, library, sync_cancel: Arc::default() }
    }

    pub fn ctx(&self) -> &egui::Context {
        &self.ctx
    }

    fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
        self.ctx.request_repaint();
    }

    /// Delivers `msg` to the UI from any thread.
    pub fn post(&self, msg: Msg) {
        self.send(msg);
    }

    /// Forwards engine events into the UI channel so `Ended` is seen even when
    /// idle; `tap` sees each one first.
    pub fn forward_engine_events(&self, events: audio::EventReceiver<audio::Event>, tap: impl Fn(&audio::Event) + Send + 'static) {
        let this = self.clone();
        std::thread::Builder::new()
            .name("engine-events".into())
            .spawn(move || {
                while let Ok(ev) = events.recv() {
                    tap(&ev);
                    this.send(Msg::Engine(ev));
                }
            })
            .expect("spawn event forwarder");
    }

    /// Loads the cached library from disk without touching the network.
    pub fn load_cached_albums(&self) {
        let this = self.clone();
        self.rt.spawn_blocking(move || {
            match this.library.albums() {
                Ok(a) => this.send(Msg::Albums(a)),
                Err(e) => return this.send(Msg::Error(format!("read library: {e}"))),
            }
            if let Ok(a) = this.library.artists() {
                this.send(Msg::Artists(a));
            }
            this.load_all_tracks();
        });
    }

    /// Blocking: reads every stored track and sends them to the UI.
    fn load_all_tracks(&self) {
        match self.library.all_tracks() {
            Ok(t) => self.send(Msg::AllTracks(t)),
            Err(e) => self.send(Msg::Error(format!("read tracks: {e}"))),
        }
    }

    /// Pings the server; on success persists settings (+ password when `remember`).
    pub fn connect(&self, settings: Settings, password: Option<String>, paths: Arc<Paths>) {
        let this = self.clone();
        self.rt.spawn(async move {
            let password = match password {
                Some(p) => p,
                None => {
                    let s = settings.clone();
                    match tokio::task::spawn_blocking(move || s.load_password()).await.ok().flatten() {
                        Some(p) => p,
                        None => return this.send(Msg::ConnectFailed("no saved password".into())),
                    }
                }
            };
            let client = match Client::new(&settings.server, Credentials::new(&settings.username, &password)) {
                Ok(c) => c,
                Err(e) => return this.send(Msg::ConnectFailed(e.to_string())),
            };
            match client.ping().await {
                Ok(info) => {
                    let server = format!(
                        "{} {}",
                        info.server_type.as_deref().unwrap_or("subsonic"),
                        info.server_version.as_deref().unwrap_or(&info.version)
                    );
                    let s = settings.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        if let Err(e) = s.save(&paths) {
                            tracing::warn!("save settings: {e}");
                        }
                        if let Err(e) = s.store_password(&password) {
                            tracing::warn!("keychain: {e}");
                        }
                    })
                    .await;
                    this.send(Msg::Connected { client, settings: Box::new(settings), server });
                }
                Err(e) => this.send(Msg::ConnectFailed(e.to_string())),
            }
        });
    }

    /// Pages through every album on the server into the local db.
    pub fn sync_albums(&self, client: Client) {
        let this = self.clone();
        self.rt.spawn(async move {
            const PAGE: u32 = 500;
            let mut offset = 0;
            let mut seen = HashSet::new();
            loop {
                let page = match client.album_list(AlbumListType::AlphabeticalByArtist, PAGE, offset).await {
                    Ok(p) => p,
                    Err(e) => return this.send(Msg::Error(format!("sync: {e}"))),
                };
                seen.extend(page.iter().map(|a| a.id.clone()));
                let n = page.len() as u32;
                let lib = this.library.clone();
                let stored = tokio::task::spawn_blocking(move || lib.upsert_albums(page)).await;
                if let Ok(Err(e)) = stored {
                    return this.send(Msg::Error(format!("store albums: {e}")));
                }
                offset += n;
                this.send(Msg::SyncProgress(offset as usize));
                if n < PAGE {
                    break;
                }
            }
            // The full list arrived, so anything else stored is gone from the server.
            let lib = this.library.clone();
            let listed = tokio::task::spawn_blocking(move || {
                let removed = lib.retain_albums(&seen)?;
                if removed > 0 {
                    tracing::info!("sync: removed {removed} albums no longer on the server");
                }
                lib.albums()
            })
            .await;
            if let Ok(Ok(albums)) = listed {
                this.send(Msg::Albums(albums));
            }
            this.send(Msg::SyncDone(offset as usize));
            this.sync_artists(client.clone()).await;
            this.sync_tracks(client);
        });
    }

    async fn sync_artists(&self, client: Client) {
        let artists = match client.artists().await {
            Ok(a) => a,
            Err(e) => return self.send(Msg::Error(format!("artists: {e}"))),
        };
        let lib = self.library.clone();
        if let Ok(Ok(stored)) = tokio::task::spawn_blocking(move || lib.replace_artists(artists).and_then(|_| lib.artists())).await {
            self.send(Msg::Artists(stored));
        }
    }

    /// Fetches the tracks of every album that changed since the last sync, in
    /// the background. Replaces (cancels) a sync already running.
    pub fn sync_tracks(&self, client: Client) {
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(old) = self.sync_cancel.lock().unwrap_or_else(|e| e.into_inner()).replace(cancel.clone()) {
            old.store(true, Ordering::Relaxed);
        }
        let this = self.clone();
        self.rt.spawn(async move {
            // At most ~10 progress repaints a second, however fast albums arrive.
            let last = Mutex::new(Instant::now() - Duration::from_secs(1));
            let outcome = sync::run(this.library.clone(), client, cancel.clone(), |p| {
                let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
                if p.done == p.total || last.elapsed() >= Duration::from_millis(100) {
                    *last = Instant::now();
                    this.send(Msg::TrackSync(p));
                }
            })
            .await;
            // Show what was stored, unless nothing changed. A cancelled sync
            // still stored something worth showing.
            if outcome != (sync::Outcome::Done { fetched: 0, bulk: false }) {
                let me = this.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    if let Ok(albums) = me.library.albums() {
                        me.send(Msg::Albums(albums));
                    }
                    me.load_all_tracks();
                })
                .await;
            }
            // A newer sync took over: it reports for itself.
            if this.is_current(&cancel) {
                this.send(Msg::TrackSyncDone(outcome));
            }
        });
    }

    fn is_current(&self, cancel: &Arc<AtomicBool>) -> bool {
        self.sync_cancel.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|c| Arc::ptr_eq(c, cancel))
    }

    /// Stops the running track sync; what it fetched so far is kept.
    pub fn cancel_sync(&self) {
        if let Some(c) = self.sync_cancel.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            c.store(true, Ordering::Relaxed);
        }
    }

    /// Shows cached tracks immediately, then refreshes them from the server.
    pub fn open_album(&self, client: Option<Client>, album_id: String) {
        let this = self.clone();
        self.rt.spawn(async move {
            let lib = this.library.clone();
            let id = album_id.clone();
            if let Ok(Ok(tracks)) = tokio::task::spawn_blocking(move || lib.album_tracks(&id)).await
                && !tracks.is_empty()
            {
                this.send(Msg::Tracks { album_id: album_id.clone(), tracks });
            }
            let Some(client) = client else { return };
            let album = match client.album(&album_id).await {
                Ok(a) => a,
                Err(e) => return this.send(Msg::Error(format!("load album: {e}"))),
            };
            let lib = this.library.clone();
            let id = album_id.clone();
            let stored = tokio::task::spawn_blocking(move || {
                lib.upsert_album_tracks(album)?;
                Ok::<_, library::Error>((lib.album(&id)?, lib.album_tracks(&id)?))
            })
            .await;
            match stored {
                Ok(Ok((row, tracks))) => {
                    if let Some(row) = row {
                        this.send(Msg::AlbumUpdated(row));
                    }
                    this.send(Msg::Tracks { album_id, tracks });
                }
                Ok(Err(e)) => this.send(Msg::Error(format!("store album: {e}"))),
                Err(_) => {}
            }
        });
    }

    /// Recently added albums and the user's playlists; both small, fetched on connect.
    pub fn fetch_home(&self, client: Client) {
        let this = self.clone();
        self.rt.spawn(async move {
            match client.album_list(AlbumListType::Newest, 120, 0).await {
                Ok(albums) => {
                    let ids = albums.iter().map(|a| a.id.clone()).collect();
                    let lib = this.library.clone();
                    // Newest albums may predate the last full sync.
                    let _ = tokio::task::spawn_blocking(move || lib.upsert_albums(albums)).await;
                    this.send(Msg::Newest(ids));
                }
                Err(e) => this.send(Msg::Error(format!("recently added: {e}"))),
            }
            match client.playlists().await {
                Ok(p) => this.send(Msg::Playlists(p)),
                Err(e) => this.send(Msg::Error(format!("playlists: {e}"))),
            }
        });
    }

    pub fn open_playlist(&self, client: Option<Client>, id: String) {
        let Some(client) = client else { return };
        let this = self.clone();
        self.rt.spawn(async move {
            match client.playlist(&id).await {
                Ok(p) => {
                    let tracks = p
                        .entry
                        .into_iter()
                        .map(|s| {
                            let album = s.album_id.clone().unwrap_or_default();
                            Track::from_song(s, &album)
                        })
                        .collect();
                    this.send(Msg::PlaylistTracks { id, tracks });
                }
                Err(e) => this.send(Msg::Error(format!("load playlist: {e}"))),
            }
        });
    }

    /// Looks up which stored albums credit `name` (composer, performer, ...).
    pub fn find_credited(&self, name: String) {
        let this = self.clone();
        self.rt.spawn_blocking(move || {
            if let Ok(album_ids) = this.library.albums_crediting(&name) {
                this.send(Msg::Credited { name, album_ids });
            }
        });
    }

    /// Gathers the tracks of `album_ids` in order: stored ones straight from the
    /// db, the rest from the server (and stored on the way).
    pub fn fetch_tracks(&self, client: Option<Client>, key: String, album_ids: Vec<String>) {
        let this = self.clone();
        self.rt.spawn(async move {
            let mut all = Vec::new();
            for id in album_ids {
                let lib = this.library.clone();
                let lookup = id.clone();
                let stored = tokio::task::spawn_blocking(move || lib.album_tracks(&lookup)).await.ok().and_then(Result::ok).unwrap_or_default();
                if !stored.is_empty() {
                    all.extend(stored);
                    continue;
                }
                let Some(client) = &client else { continue };
                let Ok(album) = client.album(&id).await else { continue };
                let lib = this.library.clone();
                if let Ok(Ok(tracks)) = tokio::task::spawn_blocking(move || lib.upsert_album_tracks(album).and_then(|_| lib.album_tracks(&id))).await {
                    all.extend(tracks);
                }
            }
            this.send(Msg::TrackBatch { key, tracks: all });
        });
    }
}
