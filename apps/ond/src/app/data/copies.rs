//! Materializes matched albums after scope filtering. Copy rows retain their
//! real source keys and formats; only the visible song's album link changes
//! to its album lead so routes and search share the same union.

use super::{Album, CopyFormat, Data, Track, TrackSet};
use crate::source::{copies, identity};
use accounts::{AccountId, ItemKey};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

impl Data {
    /// Every member, lead first in settings order; also works for singletons.
    pub fn copies_of_album(&self, id: &str) -> Vec<ItemKey> {
        let lead = self.album_lead(id);
        self.album_copies.get(lead).cloned().unwrap_or_else(|| ItemKey::parse(id).ok().into_iter().collect())
    }

    /// Every member of a visible song, lead first in settings order.
    pub fn copies_of_track(&self, id: &str) -> Vec<ItemKey> {
        self.track_copies.get(self.track_lead(id)).cloned().unwrap_or_else(|| ItemKey::parse(id).ok().into_iter().collect())
    }

    pub fn album_lead<'a>(&'a self, id: &'a str) -> &'a str {
        self.album_leads.get(id).map_or(id, String::as_str)
    }

    pub fn track_lead<'a>(&'a self, id: &'a str) -> &'a str {
        self.track_leads.get(id).map_or(id, String::as_str)
    }

    /// The source's untouched album, including its own count and duration.
    #[allow(dead_code)]
    pub fn album_copy(&self, id: &str) -> Option<&Album> {
        self.copy_albums.get(id).or_else(|| self.album(id))
    }

    pub fn track_copy(&self, id: &str) -> Option<&Track> {
        self.copy_tracks.as_ref().and_then(|s| s.track(id)).or_else(|| self.track(id))
    }

    pub fn copy_format(&self, id: &str) -> Option<CopyFormat<'_>> {
        let track = self.track_copy(id)?;
        let suffix = track.suffix.as_deref();
        let lossless = match suffix.map(str::to_ascii_lowercase).as_deref() {
            Some("flac" | "alac" | "wav" | "wave" | "aif" | "aiff" | "aifc" | "ape" | "wv") => true,
            Some("mp3" | "aac" | "ogg" | "oga" | "opus" | "wma") => false,
            _ => track.bit_depth.is_some_and(|d| d > 0),
        };
        Some(CopyFormat { suffix, bitrate_kbps: track.bit_rate_kbps, lossless })
    }

    /// Per-source formats, in source order, including songs absent on the lead.
    pub fn album_copy_formats(&self, id: &str) -> Vec<(AccountId, Vec<CopyFormat<'_>>)> {
        self.copies_of_album(id)
            .into_iter()
            .map(|key| {
                let spelling = key.to_string();
                let tracks = self.copy_tracks.as_ref().unwrap_or(&self.tracks);
                let mut formats = Vec::new();
                if let Some(songs) = tracks.album(&spelling) {
                    for song in songs.iter() {
                        if let Some(format) = self.copy_format(&song.id)
                            && !formats.contains(&format)
                        {
                            formats.push(format);
                        }
                    }
                }
                (key.account, formats)
            })
            .collect()
    }

    pub(super) fn combine_copies(&mut self, sources: &[&Data]) {
        let originals = self.tracks.clone();
        // Rows are qualified at the message boundary; one that somehow isn't
        // stays a plain album of its own rather than taking the view down.
        let (albums, loose): (Vec<_>, Vec<_>) = sources.iter().flat_map(|d| d.albums.iter()).partition(|a| ItemKey::parse(&a.id).is_ok());
        let mut artist_groups = HashMap::<&str, Vec<usize>>::new();
        for (i, group) in self.merged_artists.iter().enumerate() {
            for id in group.albums.values().flatten() {
                artist_groups.entry(id).or_default().push(i);
            }
        }
        let identities: Vec<_> = albums
            .iter()
            .filter_map(|album| {
                let key = ItemKey::parse(&album.id).ok()?;
                let artist = artist_groups.get(album.id.as_str()).map_or_else(|| format!("unknown:{}", key.account), |g| format!("{g:?}"));
                Some(copies::AlbumIdentity { key, artist, title: album.name.clone(), release: self.album_mbids.get(&album.id).cloned() })
            })
            .collect();
        self.copy_albums = albums.iter().map(|a| (a.id.clone(), (*a).clone())).collect();
        let mut visible_albums: Vec<Album> = loose.iter().map(|a| (*a).clone()).collect();
        let mut visible_tracks: Vec<Track> = loose.iter().filter_map(|a| originals.album(&a.id)).flat_map(|s| s.iter().cloned()).collect();
        for group in copies::albums_apart(&identities, &self.apart_pairs) {
            let mut album = albums[group[0]].clone();
            if group.len() == 1 {
                if let Some(songs) = originals.album(&album.id) {
                    visible_tracks.extend(songs.iter().cloned());
                }
            } else {
                let keys: Vec<_> = group.iter().map(|&i| identities[i].key.clone()).collect();
                for key in &keys {
                    self.album_leads.insert(key.to_string(), album.id.clone());
                }
                self.album_copies.insert(album.id.clone(), keys);
                let (songs, song_ids): (Vec<_>, Vec<_>) = group
                    .iter()
                    .filter_map(|&i| originals.album(&albums[i].id))
                    .flat_map(|s| s.iter())
                    .filter_map(|s| {
                        let key = ItemKey::parse(&s.id).ok()?;
                        let recording = self.recording_mbids.get(&s.id).cloned();
                        Some((s, copies::SongIdentity { key, title: s.title.clone(), disc: s.disc, track: s.track, recording }))
                    })
                    .unzip();
                let mut union = Vec::new();
                for members in copies::songs(&song_ids) {
                    let mut song = songs[members[0]].clone();
                    if members.len() > 1 {
                        for &i in &members {
                            self.track_leads.insert(song_ids[i].key.to_string(), song.id.clone());
                        }
                        self.track_copies.insert(song.id.clone(), members.iter().map(|&i| song_ids[i].key.clone()).collect());
                    }
                    song.album_id = album.id.clone();
                    union.push(song);
                }
                union.sort_by_cached_key(|s| {
                    (
                        s.disc.filter(|&d| d > 0).unwrap_or(1),
                        s.track.filter(|&n| n > 0).unwrap_or(u32::MAX),
                        crate::source::artists::normalize_name(&s.title),
                        s.id.clone(),
                    )
                });
                album.song_count = union.len() as u32;
                album.duration_secs = union.iter().filter_map(|s| s.duration_secs).sum();
                visible_tracks.extend(union);
            }
            visible_albums.push(album);
        }
        let known: HashSet<_> = albums.iter().chain(&loose).map(|a| a.id.as_str()).collect();
        visible_tracks.extend(originals.searchable().iter().filter(|s| !known.contains(s.album_id.as_str())).cloned());
        visible_albums.sort_by_cached_key(|a| (a.sort_parts(), a.id.clone()));
        self.set_albums(visible_albums);
        self.set_tracks(TrackSet::combined(visible_tracks, originals.clone()));
        self.copy_tracks = Some(originals);
        let mut seen = HashSet::new();
        self.newest = self.newest.iter().map(|id| self.album_lead(id).to_owned()).filter(|id| seen.insert(id.clone())).collect();
        let leads = &self.album_leads;
        for ids in self.credited.values_mut() {
            for id in ids.iter_mut() {
                if let Some(lead) = leads.get(id) {
                    *id = lead.clone();
                }
            }
            ids.sort();
            ids.dedup();
        }
        self.remap_artist_albums();
    }

    pub(super) fn remap_artist_albums(&mut self) {
        for group in &mut self.merged_artists {
            let mut albums = BTreeMap::<AccountId, Vec<String>>::new();
            let mut seen = HashSet::new();
            for id in group.albums.values().flatten() {
                let lead = self.album_leads.get(id).unwrap_or(id);
                if self.by_id.contains_key(lead)
                    && seen.insert(lead.clone())
                    && let Some(account) = identity::account(lead)
                {
                    albums.entry(account).or_default().push(lead.clone());
                }
            }
            group.albums = albums;
        }
        let counts: HashMap<_, _> = self
            .merged_artists
            .iter()
            .flat_map(|g| {
                let count = g.albums.values().map(Vec::len).sum::<usize>() as u32;
                g.members.iter().map(move |m| (m.key.to_string(), count))
            })
            .collect();
        for row in Arc::make_mut(&mut self.artists) {
            if let Some(count) = counts.get(&row.id) {
                row.album_count = *count;
            }
        }
    }
}
