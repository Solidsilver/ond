//! Ond playlists have global `ond:<uuid>` ids in memory. Their unchanged
//! ond.redb rows contain ordered ItemKeys, including duplicates. Resolving
//! and exporting entries uses each account's mirror and live source, never
//! the account currently selected in the sidebar.

use super::all_sources::PendingMixedPlaylist;
use super::{OndApp, PlaylistAction};
use crate::bridge::{Msg, OndPlaylistEntries};
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
                        // Server entries are skipped: stream URLs contain credentials.
                        // Local entries keep plain paths, as the original exporter did.
                        let location = sources.get(&key.account)?.local()?.track_path(&key.item)?.to_string_lossy().into_owned();
                        let track = mirrors.get(&key.account)?.tracks(std::slice::from_ref(&key.item)).ok()?.into_iter().next()?;
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

    /// What the mixed-playlist sheet says: which source can't hold songs
    /// from which, in the names the user gave them.
    pub(super) fn mixed_playlist_body(&self) -> String {
        let Some(pending) = &self.multi.pending_playlist else { return String::new() };
        let settings = self.prefs.snapshot();
        let name = |a: AccountId| settings.source(a).map_or_else(|| "Another source".to_owned(), super::source_look::name_of);
        let mut others: Vec<AccountId> = pending.entries.iter().map(|k| k.account).filter(|a| *a != pending.source.account).collect();
        others.sort();
        others.dedup();
        let from: Vec<String> = others.iter().map(|a| name(*a)).collect();
        let both = if from.len() > 1 { "all of them" } else { "both" };
        format!("{} can't hold songs from {}. Ond can keep a copy of this playlist that holds {both}.", name(pending.source.account), from.join(" or "))
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
        let id = match action {
            PlaylistAction::Add { id, .. }
            | PlaylistAction::Rename { id, .. }
            | PlaylistAction::Delete { id }
            | PlaylistAction::Remove { id, .. }
            | PlaylistAction::Move { id, .. }
            | PlaylistAction::Restore { id, .. } => id,
            _ => return false,
        };
        if !id.starts_with("ond:") {
            return false;
        }
        let fallback = self.session.account().expect("configured source");
        let edit = match action {
            PlaylistAction::Add { tracks, .. } => OndEdit::Add(tracks.iter().map(|t| identity::key(fallback, &t.id)).collect()),
            PlaylistAction::Rename { name, .. } => OndEdit::Rename(name.clone()),
            PlaylistAction::Delete { .. } => OndEdit::Delete,
            _ => {
                let Some(rows) = self.multi.ond_entries.get(id) else { return true };
                let mut entries = rows.entries.clone();
                match action {
                    PlaylistAction::Remove { index, .. } => {
                        let Some(raw) = rows.visible.get(*index) else { return true };
                        entries.remove(*raw);
                    }
                    PlaylistAction::Move { from, to, .. } => {
                        let (Some(from), Some(to)) = (rows.visible.get(*from), rows.visible.get(*to)) else { return true };
                        if !super::commands::move_item(&mut entries, *from, *to) {
                            return true;
                        }
                    }
                    PlaylistAction::Restore { index, track, after, .. } => {
                        let Some(tracks) = self.multi.ond_tracks.get(id) else { return true };
                        if tracks.iter().map(|t| &t.id).ne(after.iter()) || *index > rows.visible.len() {
                            return true;
                        }
                        let raw = rows.visible.get(*index).copied().unwrap_or(entries.len());
                        entries.insert(raw, identity::key(fallback, &track.id));
                    }
                    _ => unreachable!(),
                }
                OndEdit::Replace { expected: rows.entries.clone(), entries }
            }
        };
        let store = self.bridge.ond_store().clone();
        let id = id.trim_start_matches("ond:").to_owned();
        let post = self.bridge.poster();
        self.bridge.runtime().spawn_blocking(move || {
            let result = match edit {
                OndEdit::Add(entries) => store.add_to_playlist(&id, &entries).map(|_| ()),
                OndEdit::Replace { expected, entries } => store.replace_playlist_entries_if_unchanged(&id, &expected, entries).map(|_| ()),
                OndEdit::Rename(name) => store.rename_playlist(&id, &name).map(|_| ()),
                OndEdit::Delete => store.delete_playlist(&id).map(|_| ()),
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
                let mut entries = HashMap::new();
                for playlist in playlists {
                    let (tracks, visible) = resolve_entries(&playlist.entries, &mirrors)?;
                    let id = format!("ond:{}", playlist.id);
                    rows.push(Playlist {
                        id: id.clone(),
                        name: playlist.name,
                        song_count: tracks.len() as u32,
                        duration: tracks.iter().filter_map(|t| t.duration_secs).sum(),
                        cover_art: tracks.iter().find_map(|t| t.cover_art.clone()),
                        owner: None,
                    });
                    entries.insert(id.clone(), OndPlaylistEntries { entries: playlist.entries, visible });
                    lists.insert(id, Arc::new(tracks));
                }
                Ok((rows, lists, entries))
            };
            match load() {
                Ok((playlists, tracks, entries)) => post(Msg::OndPlaylists { generation, playlists, tracks, entries }),
                Err(e) => post(Msg::Error(e.context("Couldn't load Ond playlists"))),
            }
        });
    }

    pub(super) fn ond_playlists_loaded(
        &mut self,
        generation: u64,
        playlists: Vec<Playlist>,
        tracks: HashMap<String, Arc<Vec<Track>>>,
        entries: HashMap<String, OndPlaylistEntries>,
    ) {
        if generation != self.multi.ond_generation {
            return;
        }
        self.multi.ond_entries = entries;
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

fn resolve_entries(entries: &[ItemKey], mirrors: &BTreeMap<AccountId, Arc<library::Library>>) -> Result<(Vec<Track>, Vec<usize>), Failure> {
    let mut tracks = Vec::new();
    let mut visible = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let Some(mirror) = mirrors.get(&entry.account) else { continue };
        if let Some(mut track) = mirror.tracks(std::slice::from_ref(&entry.item)).map_err(|e| Failure::Local(e.to_string()))?.into_iter().next() {
            identity::track(entry.account, &mut track);
            tracks.push(track);
            visible.push(index);
        }
    }
    Ok((tracks, visible))
}

/// Replacements retain the complete raw list, including unresolved entries.
enum OndEdit {
    Add(Vec<ItemKey>),
    Rename(String),
    Delete,
    Replace { expected: Vec<ItemKey>, entries: Vec<ItemKey> },
}
