//! The library as last loaded (albums, tracks, playlists, artists), where
//! the sync is, the palette's search index, and queue actions waiting for
//! tracks to load. Library messages from the bridge land here.
//!
//! Shared in-memory rows use the canonical ItemKey string in every id and
//! link. Views can keep passing strings through routes, selections and memo
//! keys while the account travels with the item. Per-source redb rows stay
//! bare; the live bridge qualifies results before enqueueing them.
//! Source snapshots retain their own versions and shared track indexes.

use super::dialogs::Overlay;
use super::{Action, AlbumSort, OndApp};
use crate::bridge::{LibraryMsg, ListKey};
use crate::nav::Route;
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
    pub account: Option<accounts::AccountId>,
    pub artists_version: u64,
    pub lists_version: u64,
    merged_album_versions: Vec<(Option<accounts::AccountId>, u64)>,
    merged_track_versions: Vec<(Option<accounts::AccountId>, u64)>,
    merged_artist_versions: Vec<(Option<accounts::AccountId>, u64)>,
    merged_list_versions: Vec<(Option<accounts::AccountId>, u64)>,
    pub merged_artists: Vec<crate::source::artists::MergedArtist>,
    pub artist_mbids: HashMap<String, String>,
    /// Shared with a search index being built from it, so handing it over
    /// copies nothing.
    pub albums: Arc<Vec<Album>>,
    pub by_id: HashMap<String, usize>,
    /// Bumped whenever `albums` changes, so derived lists know to rebuild.
    pub version: u64,
    /// Recently added, newest first (album ids from the server).
    pub newest: Vec<String>,
    pub playlists: Vec<crate::source::Playlist>,
    /// Every stored track once (`all`), grouped by album and indexed by id.
    /// The grouping and the index also have the albums opened since (fresh
    /// from the server); `all` doesn't.
    pub tracks: Arc<TrackSet>,
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
    /// Materializes the scope from independently versioned source snapshots.
    /// Source snapshots keep their shared albums and track slices between
    /// batches, so receiving one source never rebuilds another's indexes.
    pub(super) fn merge_sources(&mut self, sources: &[&Data]) {
        let albums: Vec<_> = sources.iter().map(|d| (d.account, d.version)).collect();
        let tracks: Vec<_> = sources.iter().map(|d| (d.account, d.tracks_version)).collect();
        let artists: Vec<_> = sources.iter().map(|d| (d.account, d.artists_version)).collect();
        let lists: Vec<_> = sources.iter().map(|d| (d.account, d.lists_version)).collect();
        if self.merged_album_versions != albums {
            let mut merged: Vec<_> = sources.iter().flat_map(|d| d.albums.iter().cloned()).collect();
            // Each source's albums come in its database's order (artist,
            // year, title); merged, they're put back in that order together.
            if sources.len() > 1 {
                merged.sort_by_cached_key(|a| (a.sort_parts(), a.id.clone()));
            }
            self.set_albums(merged);
            self.newest = sources.iter().flat_map(|d| d.newest.iter().cloned()).collect();
        }
        if self.merged_track_versions != tracks {
            self.set_tracks(TrackSet::merge(sources.iter().map(|d| d.tracks.clone())));
        }
        if self.merged_list_versions != lists {
            self.playlists = sources.iter().flat_map(|d| d.playlists.iter().cloned()).collect();
            self.playlist_tracks = sources.iter().flat_map(|d| d.playlist_tracks.iter().map(|(id, ts)| (id.clone(), ts.clone()))).collect();
            self.credited.clear();
            for d in sources {
                for (name, albums) in &d.credited {
                    self.credited.entry(name.clone()).or_default().extend(albums.iter().cloned());
                }
            }
        }
        if self.merged_artist_versions != artists || self.merged_album_versions != albums {
            self.artist_mbids = sources.iter().flat_map(|d| d.artist_mbids.iter().map(|(k, v)| (k.clone(), v.clone()))).collect();
            self.artists = Arc::new(sources.iter().flat_map(|d| d.artists.iter().cloned()).collect());
            self.rebuild_merged_artists();
        }
        self.merged_album_versions = albums;
        self.merged_track_versions = tracks;
        self.merged_artist_versions = artists;
        self.merged_list_versions = lists;
    }

    pub(super) fn artist_catalog(sources: &[&Data]) -> Vec<crate::source::artists::MergedArtist> {
        if sources.len() < 2 {
            return Vec::new();
        }
        let mut catalog = Data::default();
        catalog.set_albums(sources.iter().flat_map(|d| d.albums.iter().cloned()).collect());
        catalog.artists = Arc::new(sources.iter().flat_map(|d| d.artists.iter().cloned()).collect());
        catalog.artist_mbids = sources.iter().flat_map(|d| d.artist_mbids.iter().map(|(id, mb)| (id.clone(), mb.clone()))).collect();
        catalog.rebuild_merged_artists();
        catalog.merged_artists
    }

    pub(super) fn remember_artist_members(&mut self, catalog: &[crate::source::artists::MergedArtist]) {
        self.merged_artists = catalog.to_vec();
        for group in &mut self.merged_artists {
            for albums in group.albums.values_mut() {
                albums.retain(|id| self.by_id.contains_key(id));
            }
            group.albums.retain(|_, albums| !albums.is_empty());
        }
    }

    pub fn merged_artist(&self, id: Option<&str>, name: &str) -> Option<&crate::source::artists::MergedArtist> {
        use crate::source::artists::normalize_name;
        self.merged_artists
            .iter()
            .find(|g| id.is_some_and(|id| g.members.iter().any(|m| m.key.to_string() == id)))
            .or_else(|| self.merged_artists.iter().find(|g| g.members.iter().any(|m| normalize_name(&m.name) == normalize_name(name))))
    }

    pub fn artist_has_album(&self, album: &Album, id: Option<&str>, name: &str) -> bool {
        if let Some(group) = self.merged_artist(id, name) {
            return group.albums.values().flatten().any(|a| a == &album.id);
        }
        album.has_artist(id, name)
    }

    fn rebuild_merged_artists(&mut self) {
        use crate::source::{
            artists::{ArtistIdentity, merge},
            identity,
        };
        let accounts: std::collections::BTreeSet<_> = self.albums.iter().filter_map(|a| identity::account(&a.id)).collect();
        self.merged_artists.clear();
        if accounts.len() < 2 {
            return;
        }
        let mut artists = self.artists.as_ref().clone();
        let mut synthetic = std::collections::HashSet::new();
        let mut known: std::collections::HashSet<_> = artists.iter().map(|a| a.id.clone()).collect();
        for album in self.albums.iter() {
            let Some(account) = identity::account(&album.id) else { continue };
            let credits: Vec<_> = if album.artists.is_empty() {
                album.artist.as_ref().map(|name| vec![(name, album.artist_ref.as_ref())]).unwrap_or_default()
            } else {
                album.artists.iter().map(|a| (&a.name, a.id.as_ref())).collect()
            };
            for (name, id) in credits {
                let id = id.cloned().unwrap_or_else(|| {
                    let id = accounts::ItemKey::new(account, format!("\0{}", crate::source::artists::normalize_name(name))).to_string();
                    synthetic.insert(id.clone());
                    id
                });
                if known.insert(id.clone()) {
                    artists.push(library::Artist { id, name: name.clone(), sort: None, album_count: 0, cover_art: None });
                }
            }
        }
        let identities = artists.iter().filter_map(|a| {
            Some(ArtistIdentity { key: accounts::ItemKey::parse(&a.id).ok()?, name: a.name.clone(), musicbrainz: self.artist_mbids.get(&a.id).cloned() })
        });
        let mut groups = merge(identities);
        let mut by_id = HashMap::<String, usize>::new();
        let mut by_name = HashMap::<(accounts::AccountId, String), usize>::new();
        for (i, group) in groups.iter().enumerate() {
            for member in &group.members {
                by_id.insert(member.key.to_string(), i);
                by_name.insert((member.key.account, crate::source::artists::normalize_name(&member.name)), i);
            }
        }
        for album in self.albums.iter() {
            let Some(account) = identity::account(&album.id) else { continue };
            let mut matched = std::collections::BTreeSet::new();
            for id in album.artist_ref.iter().chain(album.artists.iter().filter_map(|a| a.id.as_ref())) {
                if let Some(&i) = by_id.get(id) {
                    matched.insert(i);
                }
            }
            if matched.is_empty() {
                for name in album.artist.iter().chain(album.artists.iter().map(|a| &a.name)) {
                    if let Some(&i) = by_name.get(&(account, crate::source::artists::normalize_name(name))) {
                        matched.insert(i);
                    }
                }
            }
            for i in matched {
                groups[i].albums.entry(account).or_default().push(album.id.clone());
            }
        }
        let original: HashMap<_, _> = artists.into_iter().map(|a| (a.id.clone(), a)).collect();
        let mut rows = Vec::new();
        for group in &groups {
            let first = group.members[0].key.to_string();
            if let Some(mut row) = original.get(&first).cloned() {
                if synthetic.contains(&row.id) {
                    row.id.clear();
                }
                row.album_count = group.albums.values().map(Vec::len).sum::<usize>() as u32;
                rows.push(row);
            }
        }
        self.artists = Arc::new(rows);
        self.merged_artists = groups;
    }

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
        self.tracks = Arc::new(set);
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

    /// What's drawn for `track` when it has no art (see [`crate::art::track_cover`]).
    pub fn stand_in<'a>(&'a self, track: &'a Track) -> shell::kit::Generated<'a> {
        crate::art::track_cover(self.album(&track.album_id), track)
    }

    /// `false` when the row was already exactly this (an album opened again),
    /// so nothing built from the list needs redoing.
    pub(super) fn update_album(&mut self, album: Album) -> bool {
        if self.by_id.get(&album.id).is_some_and(|&i| self.albums[i] == album) {
            return false;
        }
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
        true
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
    pub(super) key: ListKey,
    source: Source,
    action: QueueAction,
    /// What's stored is known to be incomplete (an album a sync stored only
    /// part of): wait for the server's own listing, not the stored copy.
    fresh: bool,
}

impl OndApp {
    /// A message about the library, its sync or a track list that loaded.
    pub(super) fn library_msg(&mut self, msg: LibraryMsg) {
        match msg {
            LibraryMsg::ScopeTracks { key, parts } => {
                let tracks = parts
                    .into_iter()
                    .filter(|(account, session, _)| self.multi.bridges.get(account).is_some_and(|b| b.session() == *session))
                    .flat_map(|(_, _, tracks)| tracks)
                    .collect();
                self.library_msg(LibraryMsg::TrackBatch { key, tracks });
            }
            LibraryMsg::ArtistMbids(rows) => {
                self.data.artist_mbids = rows.into_iter().collect();
                self.data.artists_version += 1;
                self.search.stale = true;
            }
            LibraryMsg::SearchIndex { generation, index } => {
                if generation == self.search.generation {
                    self.install_search(*index);
                }
            }
            LibraryMsg::Albums(a) => {
                self.data.set_albums(a);
                self.search.stale = true;
            }
            LibraryMsg::Artists(a) => {
                self.data.artists = Arc::new(a);
                self.data.artists_version += 1;
                self.search.stale = true;
            }
            LibraryMsg::AllTracks(set) => {
                self.data.set_tracks(*set);
                self.search.stale = true;
                // A sync may have added tracks to pinned albums. The library can be
                // partial (a sync stopped or failed, a rebuilt database), so this may
                // only add: removals wait for the server's own listing of the album.
                let albums: Vec<String> =
                    self.downloads.snap.pins.iter().filter_map(|p| if let Pin::Album(id) = &p.pin { Some(id.clone()) } else { None }).collect();
                for id in albums {
                    if let Some(tracks) = self.data.tracks.album(&id).cloned() {
                        self.refresh_pin(Pin::Album(id), &tracks, false);
                    }
                }
                self.resume_library_analysis();
            }
            LibraryMsg::TrackSync(p) => {
                self.session.sync.tracks = true;
                self.session.sync.progress = Some(p);
                self.session.sync.label = if self.session.kind().scans {
                    format!("Scanning · {} of {} tracks", crate::fmt::thousands(p.done), crate::fmt::thousands(p.total))
                } else {
                    p.label()
                };
            }
            LibraryMsg::TrackSyncDone(outcome) => {
                self.session.sync.tracks = false;
                self.session.sync.progress = None;
                let albums = self.data.albums.len();
                let tracks = self.data.tracks.all.len();
                self.session.sync.label = match outcome {
                    crate::sync::Outcome::Done { .. } => {
                        self.session.sync.last = Some(chrono::Local::now());
                        self.first_scan_done(None);
                        if !self.session.source_caps().playlists {
                            for id in self.data.playlist_tracks.keys().cloned().collect::<Vec<_>>() {
                                self.bridge.open_playlist(self.session.source.clone(), id);
                            }
                        }
                        format!("{albums} albums · {tracks} tracks")
                    }
                    crate::sync::Outcome::Cancelled => format!("{albums} albums · {tracks} tracks · sync stopped"),
                    crate::sync::Outcome::Failed(e) => {
                        self.show_failure_or_retry(e, Some(Action::SyncNow));
                        format!("{albums} albums · {tracks} tracks")
                    }
                };
            }
            LibraryMsg::AlbumUpdated(a) => {
                // A rebuilt index is tens of ms off-thread; only when the row changed.
                if self.data.update_album(*a) {
                    self.search.stale = true;
                }
            }
            LibraryMsg::SyncProgress(n) => {
                self.session.sync.albums = true;
                self.session.sync.label = format!("Syncing… {n}");
            }
            LibraryMsg::AlbumSyncDone(Ok(n)) => {
                self.session.sync.albums = false;
                self.session.sync.label = format!("{n} albums");
                self.session.sync.tracks = true;
            }
            LibraryMsg::AlbumSyncDone(Err(e)) => {
                // Nothing more follows (no artist or track sync, or the scan
                // stopped): stop saying "syncing".
                self.session.sync.albums = false;
                self.session.sync.tracks = false;
                self.session.sync.progress = None;
                self.session.sync.label = "Sync failed".into();
                if !self.first_scan_done(Some(e.message())) {
                    self.show_failure_or_retry(e, Some(Action::SyncNow));
                }
            }
            LibraryMsg::FolderMissing(paths) => {
                self.session.folder_missing = paths;
                if !self.session.folder_missing.is_empty() {
                    // An unavailable root is already shown in Sources and the
                    // sidebar. A fully missing scan has no more results coming.
                    self.session.sync.albums = false;
                    self.session.sync.tracks = false;
                    self.session.sync.progress = None;
                    self.session.sync.label = "Folder missing".into();
                    self.first_scan_done(None);
                }
            }
            LibraryMsg::Skipped(n) => self.session.skipped = n,
            LibraryMsg::LoadFailed { key, error } => {
                // One that only waited for a complete listing plays what's stored.
                if let Some(at) = self.pending.iter().position(|p| p.key == key && p.fresh) {
                    self.pending[at].fresh = false;
                    self.run_pending(&key, false);
                }
                // Whatever else waited for these tracks gives up, so it can't
                // fire much later when they turn up for some other reason.
                let gave_up = self.pending.iter().position(|p| p.key == key).map(|at| self.pending.remove(at));
                self.pending.retain(|p| p.key != key);
                self.downloads.pending_pins.retain(|(k, _)| *k != key);
                if let Some(e) = error {
                    // Retry does what gave up (it never ran), or loads the list again.
                    let retry = match (key, gave_up) {
                        (ListKey::Album(id), Some(p)) => Some(Action::Album(id, p.action)),
                        (ListKey::Playlist(id), Some(p)) => Some(Action::Playlist(id, p.action)),
                        (key @ (ListKey::Album(_) | ListKey::Playlist(_)), None) => Some(Action::Reload(key)),
                        (ListKey::Artist(_), _) => None,
                    };
                    self.show_failure_or_retry(e, retry);
                }
            }
            LibraryMsg::Tracks { album_id, mut tracks, fresh } => {
                if self.session.work_offline {
                    let transcode = self.output.transcode();
                    tracks.retain(|t| super::playable(&self.session, &self.downloads, &self.cache, transcode.as_ref(), t));
                }
                self.refresh_pin(Pin::Album(album_id.clone()), &tracks, fresh);
                // The server's listing may have songs the last sync didn't:
                // searchable now, not after the next sync (and only then, since
                // every album opened comes through here).
                let unknown = fresh && tracks.iter().any(|t| self.data.tracks.track(&t.id).is_none());
                Arc::make_mut(&mut self.data.tracks).set_album(&album_id, tracks);
                self.data.tracks_version += 1;
                if unknown {
                    self.search.stale = true;
                }
                self.run_pending(&ListKey::Album(album_id), fresh);
            }
            LibraryMsg::Newest(ids) => {
                self.data.served.newest = true;
                self.data.newest = ids;
                self.data.version += 1;
            }
            LibraryMsg::Playlists(p) => {
                self.data.served.playlists = true;
                self.data.playlists = p;
                self.data.lists_version += 1;
            }
            LibraryMsg::Saved(saved) => {
                if let Some(ids) = saved.newest.filter(|_| !self.data.served.newest) {
                    self.data.newest = ids;
                    self.data.version += 1;
                }
                if let Some(p) = saved.playlists.filter(|_| !self.data.served.playlists) {
                    self.data.playlists = p;
                    self.data.lists_version += 1;
                }
                if let Some(s) = saved.starred.filter(|_| !self.data.served.starred) {
                    self.load_favorites(s.starred, s.ratings);
                }
            }
            LibraryMsg::PlaylistTracks { id, mut tracks, fresh } => {
                if self.session.work_offline {
                    let transcode = self.output.transcode();
                    tracks.retain(|t| super::playable(&self.session, &self.downloads, &self.cache, transcode.as_ref(), t));
                }
                // Only the server's own listing may drop songs from a download.
                self.refresh_pin(Pin::Playlist(id.clone()), &tracks, fresh);
                self.data.playlist_tracks.insert(id.clone(), Arc::new(tracks));
                self.data.lists_version += 1;
                self.run_pending(&ListKey::Playlist(id), fresh);
            }
            LibraryMsg::Credited { name, album_ids } => {
                self.data.credited.insert(name, album_ids);
                self.data.lists_version += 1;
            }
            LibraryMsg::TrackBatch { key, tracks } => {
                if let Some(at) = self.pending.iter().position(|p| p.key == key) {
                    let p = self.pending.remove(at);
                    self.queue_tracks(tracks, p.source, p.action);
                }
            }
        }
    }

    /// Work owed once per batch of messages: a sync delivers albums, artists
    /// and tracks back to back, so derived data is rebuilt once, not per message.
    pub(super) fn after_messages(&mut self) {
        if std::mem::take(&mut self.search.stale) {
            self.search.generation += 1;
            self.bridge.build_search(self.search.generation, Arc::clone(&self.data.albums), Arc::clone(&self.data.artists), self.data.tracks.searchable());
        }
    }

    /// Builds the index here and now (fixtures and tests).
    #[cfg(test)]
    pub(super) fn rebuild_search(&mut self) {
        self.install_search(SearchIndex::new(&self.data.albums, &self.data.artists, self.data.tracks.searchable().iter()));
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
        let Some(account) = self.session.account() else { return };
        self.with_player(|p, cx| p.queue_tracks(account, tracks, source, action, cx));
    }

    /// Plays what was asked about: keeping the added tracks (after the new
    /// music) or clearing them.
    pub(super) fn queue_tracks_answered(&mut self, tracks: Vec<Track>, source: Source, action: QueueAction, keep: bool) {
        let on_play = if keep { OnPlay::KeepAfter } else { OnPlay::Clear };
        let Some(account) = self.session.account() else { return };
        self.with_player(|p, cx| p.queue_tracks_with(account, tracks, source, action, on_play, cx));
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
        let partial = stored.is_some_and(|t| t.len() < count) && self.session.source.is_some();
        match stored {
            Some(t) if !partial => self.queue_tracks(t.to_vec(), source, action),
            _ => {
                self.wait_for(Pending { key: ListKey::Album(id.clone()), source, action, fresh: partial });
                self.bridge.open_album(self.session.source.clone(), id);
            }
        }
    }

    /// Plays (or queues) all of an artist's albums, once their tracks load:
    /// in the order their page lists them (by year), or shuffled.
    pub(super) fn play_artist(&mut self, name: String, id: Option<String>, action: QueueAction) {
        let route = Route::Artist { name: name.clone(), id: id.clone() };
        let ids: Vec<String> =
            super::browse::grid_ids(&route, AlbumSort::Year, false, &self.data).into_iter().map(|i| self.data.albums[i].id.clone()).collect();
        let key = ListKey::Artist(id.unwrap_or_else(|| name.clone()));
        let source = match action {
            QueueAction::PlayNext | QueueAction::Add => Source::Added,
            QueueAction::Shuffle => Source::Artist { name: format!("{name}, shuffled"), shuffled: true },
            QueueAction::Play { .. } => Source::Artist { name, shuffled: false },
        };
        // Their albums from the first, whatever the start asked for.
        let action = if matches!(action, QueueAction::Play { .. }) { QueueAction::Play { start: 0 } } else { action };
        self.wait_for(Pending { key: key.clone(), source, action, fresh: false });
        if self.multi.bridges.is_empty() {
            self.bridge.fetch_tracks(self.session.source.as_ref().and_then(|s| s.paged()), key, ids);
        } else {
            self.fetch_scope_tracks(key, ids);
        }
    }

    /// Plays (or queues) a playlist: what's loaded, or once it loads.
    pub(super) fn play_playlist(&mut self, id: String, action: QueueAction) {
        let name = self.data.playlists.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_default();
        let source = Source::Playlist { id: id.clone(), name };
        match self.data.playlist_tracks.get(&id) {
            Some(t) => self.queue_tracks(t.to_vec(), source, action),
            None => {
                self.wait_for(Pending { key: ListKey::Playlist(id.clone()), source, action, fresh: false });
                if id.starts_with("ond:") {
                    self.refresh_ond_playlists();
                } else {
                    self.bridge.open_playlist(self.session.source.clone(), id);
                }
            }
        }
    }
}

pub(super) fn belongs_to(key: &ListKey, account: accounts::AccountId) -> bool {
    match key {
        ListKey::Album(id) | ListKey::Playlist(id) => crate::source::identity::account(id) == Some(account),
        ListKey::Artist(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_sources_albums_merge_in_artist_order_not_one_source_after_the_other() {
        let source = |n: u32, albums: &[(&str, &str)]| {
            let account: accounts::AccountId = format!("00000000-0000-4000-8000-{n:012x}").parse().unwrap();
            let mut d = Data { account: Some(account), ..Default::default() };
            d.set_albums(
                albums
                    .iter()
                    .map(|(name, artist)| Album { artist: Some((*artist).into()), ..album(&accounts::ItemKey::new(account, *name).to_string(), name) })
                    .collect(),
            );
            d
        };
        let server = source(1, &[("Moon Safari", "Air"), ("OK Computer", "Radiohead")]);
        let folder = source(2, &[("Fingerlings", "Andrew Bird"), ("Kid A", "Radiohead")]);
        let mut merged = Data::default();
        merged.merge_sources(&[&server, &folder]);
        let names: Vec<&str> = merged.albums.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Moon Safari", "Fingerlings", "Kid A", "OK Computer"]);
    }

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
