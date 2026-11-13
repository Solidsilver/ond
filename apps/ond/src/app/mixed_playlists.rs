//! Ond playlists have global `ond:<uuid>` ids in memory. Their unchanged
//! ond.redb rows contain ordered ItemKeys, including duplicates. Resolving
//! and exporting entries uses each account's mirror and live source, never
//! the account currently selected in the sidebar.

use super::all_sources::PendingMixedPlaylist;
use super::{OndApp, PlaylistAction};
use crate::bridge::{Msg, PlaylistEdit};
use crate::failure::Failure;
use crate::source::{Playlist, identity};
use accounts::{AccountId, ItemKey};
use library::Track;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

impl OndApp {
    pub(super) fn export_ond_m3u(&self, id: String, path: std::path::PathBuf) {
        let store = self.bridge.ond_store().clone();
        let mirrors: BTreeMap<_, _> = self.multi.bridges.iter().map(|(a, b)| (*a, b.library_handle())).collect();
        let sources: BTreeMap<_, _> = self.session.accounts.iter().filter_map(|(a, s)| Some((*a, s.live()?.clone()))).collect();
        let post = self.bridge.poster();
        self.bridge.runtime().spawn_blocking(move || {
            let result = (|| -> Result<(), Failure> {
                let body = store
                    .export_m3u(id.trim_start_matches("ond:"), |key| {
                        let source = sources.get(&key.account)?;
                        let track = mirrors.get(&key.account)?.tracks(std::slice::from_ref(&key.item)).ok()?.into_iter().next()?;
                        let (location, _) = source.stream(&track, None);
                        Some(crate::store::M3uTrack { location, title: Some(track.title), artist: track.artist, duration_secs: track.duration_secs })
                    })
                    .map_err(|e| Failure::Local(e.to_string()))?
                    .ok_or_else(|| Failure::Local("Playlist not found".into()))?;
                std::fs::write(&path, body).map_err(|e| Failure::Local(e.to_string()))
            })();
            post(Msg::M3uExported { path, error: result.err() });
        });
    }

    pub(super) fn offer_mixed_playlist(&mut self, id: String, name: String, existing: Vec<Track>, added: Vec<Track>) {
        let Ok(source) = ItemKey::parse(&id) else { return };
        let entries = existing.iter().chain(&added).map(|t| identity::key(source.account, &t.id)).collect();
        self.multi.pending_playlist = Some(PendingMixedPlaylist { source, name, entries });
        self.ui.overlay = Some(super::Overlay::MixedPlaylist);
    }

    pub(super) fn ond_playlist_action(&mut self, action: &PlaylistAction) -> bool {
        if let PlaylistAction::AcceptMixed(accept) = action {
            let pending = self.multi.pending_playlist.take();
            self.ui.overlay = None;
            if *accept && let Some(pending) = pending {
                self.create_ond_playlist(pending.name, pending.entries);
            }
            return true;
        }
        if self.multi.bridges.is_empty() {
            return false;
        }
        match action {
            PlaylistAction::Create { name, tracks } => {
                let accounts: std::collections::BTreeSet<_> = tracks.iter().filter_map(|t| identity::account(&t.id)).collect();
                if accounts.len() > 1 || !self.session.source_caps().playlists {
                    let fallback = self.session.account().expect("configured source");
                    let entries = tracks.iter().map(|t| identity::key(fallback, &t.id)).collect();
                    self.create_ond_playlist(name.clone(), entries);
                    return true;
                }
            }
            PlaylistAction::Add { id, tracks } if !id.starts_with("ond:") => {
                if let Some(account) = identity::account(id)
                    && tracks.iter().any(|t| identity::account(&t.id).is_some_and(|a| a != account))
                {
                    if let Some(source) = self.session.get(account).and_then(|s| s.live()).cloned() {
                        self.bridge.prepare_mixed_playlist(source, id.clone(), tracks.clone());
                    } else {
                        self.show_error("Connect to this source before copying its playlist");
                    }
                    return true;
                }
            }
            _ => {}
        }
        let (id, edit) = match action {
            PlaylistAction::Add { id, tracks } => (id, PlaylistEdit::Add(tracks.iter().map(|t| t.id.clone()).collect())),
            PlaylistAction::Rename { id, name } => (id, PlaylistEdit::Rename(name.clone())),
            PlaylistAction::Delete { id } => (id, PlaylistEdit::Delete),
            PlaylistAction::Remove { id, index } => (id, PlaylistEdit::Remove(vec![*index])),
            PlaylistAction::Move { id, from, to } => {
                if !id.starts_with("ond:") {
                    return false;
                }
                let Some(ts) = self.data.playlist_tracks.get(id) else { return true };
                let mut ts = ts.as_ref().clone();
                if !super::commands::move_item(&mut ts, *from, *to) {
                    return true;
                }
                (id, PlaylistEdit::Replace(ts.into_iter().map(|t| t.id).collect()))
            }
            PlaylistAction::Restore { id, index, track, after } => {
                if !id.starts_with("ond:") {
                    return false;
                }
                let Some(ts) = self.data.playlist_tracks.get(id) else { return true };
                if ts.iter().map(|t| &t.id).ne(after.iter()) || *index > ts.len() {
                    return true;
                }
                let mut ts = ts.as_ref().clone();
                ts.insert(*index, *track.clone());
                (id, PlaylistEdit::Replace(ts.into_iter().map(|t| t.id).collect()))
            }
            PlaylistAction::ExportM3u { .. } => return false,
            _ => return false,
        };
        if !id.starts_with("ond:") {
            return false;
        }
        let store = self.bridge.ond_store().clone();
        let id = id.trim_start_matches("ond:").to_owned();
        let fallback = self.session.account().expect("configured source");
        let post = self.bridge.poster();
        self.bridge.runtime().spawn_blocking(move || {
            let result = match edit {
                PlaylistEdit::Add(ids) => store.add_to_playlist(&id, &ids.iter().map(|id| identity::key(fallback, id)).collect::<Vec<_>>()).map(|_| ()),
                PlaylistEdit::Replace(ids) => store.replace_playlist_entries(&id, ids.iter().map(|id| identity::key(fallback, id)).collect()).map(|_| ()),
                PlaylistEdit::Remove(indices) => store.remove_from_playlist(&id, &indices).map(|_| ()),
                PlaylistEdit::Rename(name) => store.rename_playlist(&id, &name).map(|_| ()),
                PlaylistEdit::Delete => store.delete_playlist(&id).map(|_| ()),
            };
            match result {
                Ok(()) => post(Msg::OndPlaylistChanged),
                Err(e) => post(Msg::Error(Failure::Local(e.to_string()).context("Couldn't change the playlist"))),
            }
        });
        true
    }

    fn create_ond_playlist(&mut self, name: String, entries: Vec<ItemKey>) {
        let store = self.bridge.ond_store().clone();
        let post = self.bridge.poster();
        self.bridge.runtime().spawn_blocking(move || {
            let name = if name.trim().is_empty() { "Untitled playlist" } else { name.trim() };
            match store.create_playlist(name, &entries) {
                Ok(p) => {
                    post(Msg::OndPlaylistChanged);
                    post(Msg::PlaylistCreated { id: format!("ond:{}", p.id), name: p.name });
                }
                Err(e) => post(Msg::Error(Failure::Local(e.to_string()).context("Couldn't create the playlist"))),
            }
        });
    }

    pub(super) fn refresh_ond_playlists(&mut self) {
        if self.multi.bridges.is_empty() {
            return;
        }
        self.multi.ond_generation += 1;
        let generation = self.multi.ond_generation;
        let mirrors: BTreeMap<_, _> = self.multi.bridges.iter().map(|(a, b)| (*a, b.library_handle())).collect();
        let store = self.bridge.ond_store().clone();
        let post = self.bridge.poster();
        self.bridge.runtime().spawn_blocking(move || {
            let load = || -> Result<_, Failure> {
                let playlists = store.playlists().map_err(|e| Failure::Local(e.to_string()))?;
                let mut rows = Vec::new();
                let mut lists = HashMap::new();
                for playlist in playlists {
                    let tracks = resolve_entries(&playlist.entries, &mirrors)?;
                    let id = format!("ond:{}", playlist.id);
                    rows.push(Playlist {
                        id: id.clone(),
                        name: playlist.name,
                        song_count: tracks.len() as u32,
                        duration: tracks.iter().filter_map(|t| t.duration_secs).sum(),
                        cover_art: tracks.iter().find_map(|t| t.cover_art.clone()),
                        owner: None,
                    });
                    lists.insert(id, Arc::new(tracks));
                }
                Ok((rows, lists))
            };
            match load() {
                Ok((playlists, tracks)) => post(Msg::OndPlaylists { generation, playlists, tracks }),
                Err(e) => post(Msg::Error(e.context("Couldn't load Ond playlists"))),
            }
        });
    }

    pub(super) fn ond_playlists_loaded(&mut self, generation: u64, playlists: Vec<Playlist>, tracks: HashMap<String, Arc<Vec<Track>>>) {
        if generation != self.multi.ond_generation {
            return;
        }
        self.multi.ond_playlists = playlists;
        self.multi.ond_tracks = tracks.into_iter().collect();
        self.multi.projection = None;
        self.project_sources();
        let accounts: Vec<_> = self.session.accounts.keys().copied().collect();
        for account in accounts {
            let lists = self.multi.ond_tracks.clone();
            self.with_source(account, |app| {
                for (id, tracks) in lists {
                    app.refresh_pin(offline::Pin::Playlist(id), &tracks, true);
                }
            });
        }
        for id in self.multi.ond_tracks.keys().cloned().collect::<Vec<_>>() {
            self.run_pending(&crate::bridge::ListKey::Playlist(id), true);
        }
    }
}

fn resolve_entries(entries: &[ItemKey], mirrors: &BTreeMap<AccountId, Arc<library::Library>>) -> Result<Vec<Track>, Failure> {
    let mut tracks = Vec::new();
    for entry in entries {
        let Some(mirror) = mirrors.get(&entry.account) else { continue };
        if let Some(mut track) = mirror.tracks(std::slice::from_ref(&entry.item)).map_err(|e| Failure::Local(e.to_string()))?.into_iter().next() {
            identity::track(entry.account, &mut track);
            tracks.push(track);
        }
    }
    Ok(tracks)
}
