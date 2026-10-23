//! Background work for the UI. Every function here returns immediately; results
//! arrive later as [`Msg`]s, each followed by a repaint request.

use crate::settings::{Paths, Settings};
use library::{Album, Library, Track};
use std::sync::Arc;
use std::sync::mpsc::Sender;
use subsonic::{AlbumListType, Client, Credentials};

pub enum Msg {
    Connected { client: Client, settings: Box<Settings>, server: String },
    ConnectFailed(String),
    Albums(Vec<Album>),
    SyncProgress(usize),
    SyncDone(usize),
    Tracks { album_id: String, tracks: Vec<Track> },
    Engine(audio::Event),
    Error(String),
}

#[derive(Clone)]
pub struct Bridge {
    rt: tokio::runtime::Handle,
    tx: Sender<Msg>,
    ctx: egui::Context,
    library: Arc<Library>,
}

impl Bridge {
    pub fn new(rt: tokio::runtime::Handle, tx: Sender<Msg>, ctx: egui::Context, library: Arc<Library>) -> Self {
        Self { rt, tx, ctx, library }
    }

    fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
        self.ctx.request_repaint();
    }

    /// Forwards engine events into the UI channel so `Ended` is seen even when idle.
    pub fn forward_engine_events(&self, events: audio::EventReceiver<audio::Event>) {
        let this = self.clone();
        std::thread::Builder::new()
            .name("engine-events".into())
            .spawn(move || {
                while let Ok(ev) = events.recv() {
                    this.send(Msg::Engine(ev));
                }
            })
            .expect("spawn event forwarder");
    }

    /// Loads cached albums from disk without touching the network.
    pub fn load_cached_albums(&self) {
        let this = self.clone();
        self.rt.spawn_blocking(move || match this.library.albums() {
            Ok(a) => this.send(Msg::Albums(a)),
            Err(e) => this.send(Msg::Error(format!("read library: {e}"))),
        });
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
            loop {
                let page = match client.album_list(AlbumListType::AlphabeticalByArtist, PAGE, offset).await {
                    Ok(p) => p,
                    Err(e) => return this.send(Msg::Error(format!("sync: {e}"))),
                };
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
            let lib = this.library.clone();
            if let Ok(Ok(albums)) = tokio::task::spawn_blocking(move || lib.albums()).await {
                this.send(Msg::Albums(albums));
            }
            this.send(Msg::SyncDone(offset as usize));
        });
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
            match tokio::task::spawn_blocking(move || lib.upsert_album_tracks(album).and_then(|_| lib.album_tracks(&id))).await
            {
                Ok(Ok(tracks)) => this.send(Msg::Tracks { album_id, tracks }),
                Ok(Err(e)) => this.send(Msg::Error(format!("store album: {e}"))),
                Err(_) => {}
            }
        });
    }
}
