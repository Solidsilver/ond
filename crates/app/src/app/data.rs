//! The library as last loaded (albums, tracks, playlists, artists), where
//! the sync is, the palette's search index, and queue actions waiting for
//! tracks to load. Library messages from the bridge land here.

use super::OndApp;
use super::dialogs::Overlay;
use crate::bridge::{ListKey, Msg};
use crate::player::QueueAction;
use crate::queue::{OnPlay, Source};
use crate::search::SearchIndex;
use crate::tracks::TrackSet;
use library::{Album, Track};
use offline::Pin;
use std::collections::HashMap;
use std::sync::Arc;

/// Everything known about the library, as last loaded.
#[derive(Default)]
pub struct Data {
    /// Shared with a search index being built from it, so handing it over
    /// copies nothing.
    pub albums: Arc<Vec<Album>>,
    pub by_id: HashMap<String, usize>,
    /// Bumped whenever `albums` changes, so derived lists know to rebuild.
    pub version: u64,
    /// Recently added, newest first (album ids from the server).
    pub newest: Vec<String>,
    pub playlists: Vec<subsonic::Playlist>,
    /// Every stored track once (`all`), grouped by album and indexed by id.
    /// The grouping and the index also have the albums opened since (fresh
    /// from the server); `all` doesn't.
    pub tracks: TrackSet,
    pub playlist_tracks: HashMap<String, Arc<Vec<Track>>>,
    /// Album ids crediting a person, from stored tracks.
    pub credited: HashMap<String, Vec<String>>,
    /// The server's artists, as separate entities ("A, B & C" split).
    pub artists: Arc<Vec<library::Artist>>,
    /// Bumped whenever the stored tracks (`tracks.all`) change.
    pub tracks_version: u64,
    /// What the server has sent this session, so the saved copies read at
    /// startup don't replace it when they arrive late.
    pub served: Served,
}

/// Lists the server has sent since startup.
#[derive(Default)]
pub struct Served {
    pub newest: bool,
    pub playlists: bool,
    pub starred: bool,
}

impl Data {
    pub fn album(&self, id: &str) -> Option<&Album> {
        self.by_id.get(id).map(|&i| &self.albums[i])
    }

    pub(super) fn set_albums(&mut self, albums: Vec<Album>) {
        self.by_id = albums.iter().enumerate().map(|(i, a)| (a.id.clone(), i)).collect();
        self.albums = Arc::new(albums);
        self.version += 1;
    }

    /// Takes the library's tracks, grouped and indexed (off the UI thread).
    /// Stored tracks are the server's as of the last sync; anything fetched
    /// since (an album opened) is in the db too, so this replaces it all.
    pub(super) fn set_tracks(&mut self, set: TrackSet) {
        self.tracks = set;
        self.tracks_version += 1;
    }

    /// Empties it for another account's library. Versions only move forward,
    /// so derived lists never mistake the empty library for one they've seen.
    pub(super) fn clear(&mut self) {
        let (version, tracks_version) = (self.version + 1, self.tracks_version + 1);
        *self = Data { version, tracks_version, ..Data::default() };
    }

    #[cfg(test)]
    pub(super) fn set_all_tracks(&mut self, tracks: Vec<Track>) {
        self.set_tracks(TrackSet::new(tracks));
    }

    /// A track by id, from the stored library or albums opened since.
    pub fn track(&self, id: &str) -> Option<&Track> {
        self.tracks.track(id)
    }

    /// A track's artwork: its own, else its album's.
    pub fn cover_for<'a>(&'a self, track: &'a Track) -> Option<&'a str> {
        track.cover_art.as_deref().or_else(|| self.album(&track.album_id).and_then(|a| a.cover_art.as_deref()))
    }

    pub(super) fn update_album(&mut self, album: Album) {
        // Copies the list only if a search index is still being built from it.
        let albums = Arc::make_mut(&mut self.albums);
        match self.by_id.get(&album.id) {
            Some(&i) => albums[i] = album,
            None => {
                self.by_id.insert(album.id.clone(), albums.len());
                albums.push(album);
            }
        }
        self.version += 1;
    }
}

/// Where the library sync is.
#[derive(Default)]
pub struct SyncState {
    pub label: String,
    /// The album list is being fetched (before the track sync).
    pub albums: bool,
    /// A track sync is running (shows a stop button).
    pub tracks: bool,
    /// Where the track sync is, while it runs.
    pub progress: Option<crate::sync::Progress>,
    /// When the last full sync finished (this session).
    pub last: Option<chrono::DateTime<chrono::Local>>,
}

impl SyncState {
    /// Marks an album sync as started, if none is running; returns whether to
    /// start one.
    pub(super) fn begin(&mut self) -> bool {
        if self.albums || self.tracks {
            return false;
        }
        self.label = "syncing…".into();
        self.albums = true;
        true
    }
}

/// The palette's search index.
#[derive(Default)]
pub struct SearchState {
    /// Rebuilt (off the UI thread) whenever albums, artists or tracks change;
    /// the old one serves until the new one lands.
    pub index: Option<SearchIndex>,
    /// Albums, artists or tracks changed since the index was built.
    pub stale: bool,
    /// The latest index build asked for; older builds that land late are dropped.
    pub generation: u64,
}

/// A queue action waiting for tracks to load.
pub(super) struct Pending {
    key: ListKey,
    source: Source,
    action: QueueAction,
    /// What's stored is known to be incomplete (an album a sync stored only
    /// part of): wait for the server's own listing, not the stored copy.
    fresh: bool,
}

impl OndApp {
    /// A message about the library, its sync or a track list that loaded.
    pub(super) fn library_msg(&mut self, msg: Msg) {
        match msg {
            Msg::SearchIndex { generation, index } => {
                if generation == self.search.generation {
                    self.install_search(*index);
                }
            }
            Msg::Albums(a) => {
                self.data.set_albums(a);
                self.search.stale = true;
            }
            Msg::Artists(a) => {
                self.data.artists = Arc::new(a);
                self.search.stale = true;
            }
            Msg::AllTracks(set) => {
                self.data.set_tracks(*set);
                self.search.stale = true;
                // A sync may have added tracks to pinned albums. The library can be
                // partial (a sync stopped or failed, a rebuilt database), so this may
                // only add: removals wait for the server's own listing of the album.
                let albums: Vec<String> = self.downloads.snap.pins.iter().filter_map(|p| if let Pin::Album(id) = &p.pin { Some(id.clone()) } else { None }).collect();
                for id in albums {
                    if let Some(tracks) = self.data.tracks.album(&id).cloned() {
                        self.refresh_pin(Pin::Album(id), &tracks, false);
                    }
                }
                self.resume_library_analysis();
            }
            Msg::TrackSync(p) => {
                self.sync.tracks = true;
                self.sync.progress = Some(p);
                self.sync.label = p.label();
            }
            Msg::TrackSyncDone(outcome) => {
                self.sync.tracks = false;
                self.sync.progress = None;
                let albums = self.data.albums.len();
                let tracks = self.data.tracks.all.len();
                self.sync.label = match outcome {
                    crate::sync::Outcome::Done { .. } => {
                        self.sync.last = Some(chrono::Local::now());
                        format!("{albums} albums · {tracks} tracks")
                    }
                    crate::sync::Outcome::Cancelled => format!("{albums} albums · {tracks} tracks · sync stopped"),
                    crate::sync::Outcome::Failed(e) => {
                        self.show_error(e);
                        format!("{albums} albums · {tracks} tracks")
                    }
                };
            }
            Msg::AlbumUpdated(a) => self.data.update_album(a),
            Msg::SyncProgress(n) => {
                self.sync.albums = true;
                self.sync.label = format!("Syncing… {n}");
            }
            Msg::AlbumSyncDone(Ok(n)) => {
                self.sync.albums = false;
                self.sync.label = format!("{n} albums");
                self.sync.tracks = true;
            }
            Msg::AlbumSyncDone(Err(e)) => {
                // Nothing more follows (no artist or track sync): stop saying "syncing".
                self.sync.albums = false;
                self.sync.label = "Sync failed".into();
                self.probe();
                if self.session.online {
                    self.show_error(e);
                }
            }
            Msg::LoadFailed { key, error } => {
                // One that only waited for a complete listing plays what's stored.
                if let Some(at) = self.pending.iter().position(|p| p.key == key && p.fresh) {
                    self.pending[at].fresh = false;
                    self.run_pending(&key, false);
                }
                // Whatever else waited for these tracks gives up, so it can't
                // fire much later when they turn up for some other reason.
                self.pending.retain(|p| p.key != key);
                self.downloads.pending_pins.retain(|(k, _)| *k != key);
                if let Some(e) = error {
                    self.probe();
                    if self.session.online {
                        self.show_error(e);
                    }
                }
            }
            Msg::Tracks { album_id, mut tracks, fresh } => {
                if self.session.work_offline {
                    tracks.retain(|t| super::playable(&self.session, &self.downloads, &self.cache, &self.output, t));
                }
                self.refresh_pin(Pin::Album(album_id.clone()), &tracks, fresh);
                self.data.tracks.set_album(&album_id, tracks);
                self.run_pending(&ListKey::Album(album_id), fresh);
            }
            Msg::Newest(ids) => {
                self.data.served.newest = true;
                self.data.newest = ids;
                self.data.version += 1;
            }
            Msg::Playlists(p) => {
                self.data.served.playlists = true;
                self.data.playlists = p;
            }
            Msg::Saved(saved) => {
                if let Some(ids) = saved.newest.filter(|_| !self.data.served.newest) {
                    self.data.newest = ids;
                    self.data.version += 1;
                }
                if let Some(p) = saved.playlists.filter(|_| !self.data.served.playlists) {
                    self.data.playlists = p;
                }
                if let Some(s) = saved.starred.filter(|_| !self.data.served.starred) {
                    self.favorites.load(s.starred, s.ratings);
                }
            }
            Msg::PlaylistTracks { id, mut tracks, fresh } => {
                if self.session.work_offline {
                    tracks.retain(|t| super::playable(&self.session, &self.downloads, &self.cache, &self.output, t));
                }
                // Only the server's own listing may drop songs from a download.
                self.refresh_pin(Pin::Playlist(id.clone()), &tracks, fresh);
                self.data.playlist_tracks.insert(id.clone(), Arc::new(tracks));
                self.run_pending(&ListKey::Playlist(id), fresh);
            }
            Msg::Credited { name, album_ids } => {
                self.data.credited.insert(name, album_ids);
            }
            Msg::TrackBatch { key, tracks } => {
                if let Some(at) = self.pending.iter().position(|p| p.key == key) {
                    let p = self.pending.remove(at);
                    self.queue_tracks(tracks, p.source, p.action);
                }
            }
            _ => unreachable!("not a library message"),
        }
    }

    /// Work owed once per batch of messages: a sync delivers albums, artists
    /// and tracks back to back, so derived data is rebuilt once, not per message.
    pub(super) fn after_messages(&mut self) {
        if std::mem::take(&mut self.search.stale) {
            self.search.generation += 1;
            self.bridge.build_search(self.search.generation, Arc::clone(&self.data.albums), Arc::clone(&self.data.artists), Arc::clone(&self.data.tracks.all));
        }
    }

    /// Builds the index here and now (fixtures and tests).
    #[cfg(test)]
    pub(super) fn rebuild_search(&mut self) {
        self.install_search(SearchIndex::new(&self.data.albums, &self.data.artists, &self.data.tracks.all));
    }

    /// Swaps in a freshly built index; an open palette re-runs its query on it.
    pub(super) fn install_search(&mut self, mut index: SearchIndex) {
        if let Some(p) = self.ui.palette.as_mut() {
            p.refresh(&mut index);
        }
        self.search.index = Some(index);
    }

    /// Applies a queue action once the tracks are known. Playing something
    /// new asks first when the queue is set to and there are tracks the user
    /// added to keep or clear.
    pub(super) fn queue_tracks(&mut self, tracks: Vec<Track>, source: Source, action: QueueAction) {
        let q = &self.player.queue;
        let added = q.added_count();
        if matches!(action, QueueAction::Play { .. } | QueueAction::Shuffle) && q.prefs.on_play == OnPlay::Ask && added > 0 && !tracks.is_empty() {
            self.ui.overlay = Some(Overlay::KeepAdded { tracks, source, action, added });
            return;
        }
        self.with_player(|p, cx| p.queue_tracks(tracks, source, action, cx));
    }

    /// Plays what was asked about: keeping the added tracks (after the new
    /// music) or clearing them.
    pub(super) fn queue_tracks_answered(&mut self, tracks: Vec<Track>, source: Source, action: QueueAction, keep: bool) {
        let prefs = self.player.queue.prefs;
        self.player.queue.prefs.on_play = if keep { OnPlay::KeepAfter } else { OnPlay::Clear };
        self.with_player(|p, cx| p.queue_tracks(tracks, source, action, cx));
        self.player.queue.prefs = prefs;
    }

    /// Holds `p` until its tracks load. A newer action for the same list
    /// replaces an older one still waiting.
    pub(super) fn wait_for(&mut self, p: Pending) {
        self.pending.retain(|q| q.key != p.key);
        self.pending.push(p);
    }

    /// Runs what waited for `key`'s tracks, now loaded (`fresh`: the server's
    /// own listing, not the stored copy).
    pub(super) fn run_pending(&mut self, key: &ListKey, fresh: bool) {
        let Some(at) = self.pending.iter().position(|p| &p.key == key && (fresh || !p.fresh)) else { return };
        let p = self.pending.remove(at);
        let tracks = match &p.source {
            Source::Album { id, .. } => self.data.tracks.album(id).map(|s| s.to_vec()),
            Source::Playlist { id, .. } => self.data.playlist_tracks.get(id).map(|v| v.to_vec()),
            _ => None,
        };
        if let Some(t) = tracks {
            self.queue_tracks(t, p.source, p.action);
        }
    }

    /// Plays (or queues) an album: what's stored, or once its tracks load.
    pub(super) fn play_album(&mut self, id: String, action: QueueAction) {
        let name = self.data.album(&id).map(|a| a.name.clone()).unwrap_or_default();
        let source = Source::Album { id: id.clone(), name };
        // An album a sync stored only part of (cut short) would play a
        // subset: fetch it, unless there's no server to ask.
        let stored = self.data.tracks.album(&id);
        let count = self.data.album(&id).map_or(0, |a| a.song_count as usize);
        let partial = stored.is_some_and(|t| t.len() < count) && self.session.client.is_some();
        match stored {
            Some(t) if !partial => self.queue_tracks(t.to_vec(), source, action),
            _ => {
                self.wait_for(Pending { key: ListKey::Album(id.clone()), source, action, fresh: partial });
                self.bridge.open_album(self.session.client.clone(), id);
            }
        }
    }

    /// Plays (or queues) all of an artist's albums, once their tracks load.
    pub(super) fn play_artist(&mut self, name: String, action: QueueAction) {
        let ids: Vec<String> = self.data.albums.iter().filter(|a| a.has_artist(None, &name)).map(|a| a.id.clone()).collect();
        let key = ListKey::Artist(name.clone());
        let source = match action {
            QueueAction::PlayNext | QueueAction::Add => Source::Added,
            _ => Source::ArtistShuffle { name: format!("{name}, shuffled") },
        };
        // Playing an artist means all of their albums, shuffled.
        let action = if matches!(action, QueueAction::Play { .. }) { QueueAction::Shuffle } else { action };
        self.wait_for(Pending { key: key.clone(), source, action, fresh: false });
        self.bridge.fetch_tracks(self.session.client.clone(), key, ids);
    }

    /// Plays (or queues) a playlist: what's loaded, or once it loads.
    pub(super) fn play_playlist(&mut self, id: String, action: QueueAction) {
        let name = self.data.playlists.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_default();
        let source = Source::Playlist { id: id.clone(), name };
        match self.data.playlist_tracks.get(&id) {
            Some(t) => self.queue_tracks(t.to_vec(), source, action),
            None => {
                self.wait_for(Pending { key: ListKey::Playlist(id.clone()), source, action, fresh: false });
                self.bridge.open_playlist(self.session.client.clone(), id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(id: &str, name: &str) -> Album {
        Album {
            id: id.into(),
            name: name.into(),
            sort: None,
            artist: None,
            artist_ref: None,
            year: None,
            genre: None,
            song_count: 0,
            duration_secs: 0,
            cover_art: None,
            created: None,
            labels: Vec::new(),
            genres: Vec::new(),
            artists: Vec::new(),
        }
    }

    #[test]
    fn an_album_update_leaves_a_list_handed_out_as_it_was() {
        let mut data = Data::default();
        data.set_albums(vec![album("a", "Old"), album("b", "Two")]);
        // A search index being built holds the list meanwhile.
        let building = Arc::clone(&data.albums);
        let version = data.version;
        data.update_album(album("a", "New"));
        data.update_album(album("c", "Three"));
        assert_eq!(building.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Old", "Two"]);
        assert_eq!(data.album("a").map(|a| a.name.as_str()), Some("New"));
        assert_eq!(data.album("c").map(|a| a.name.as_str()), Some("Three"));
        assert_eq!(data.version, version + 2);
        // Nothing holds it now: an update changes it in place.
        drop(building);
        let at = Arc::as_ptr(&data.albums);
        data.update_album(album("b", "Second"));
        assert_eq!(Arc::as_ptr(&data.albums), at);
    }
}
