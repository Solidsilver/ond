//! Download summaries for combined rows. Real stores retain their account
//! ownership and pins; the view receives a versioned, read-only projection.

use super::{Downloads, OndApp};
use accounts::AccountId;
use offline::{Pin, PinSummary, Snapshot, TrackState};
use std::sync::Arc;

type DownloadKey = (u64, u64, Vec<(AccountId, u64, u64)>);

#[derive(Default)]
pub(in crate::app) struct DownloadView {
    key: Option<DownloadKey>,
    pub downloads: Option<Downloads>,
}

impl Downloads {
    fn projected(&self, snap: Snapshot) -> Self {
        let mut result = Self {
            store: None,
            snap: Arc::new(snap),
            pending_pins: Vec::new(),
            base: self.base.clone(),
            account: None,
            owner: None,
            generation: 0,
            opening: false,
            deferred: Vec::new(),
            moving: None,
            open: self.open.clone(),
            album_pins: Default::default(),
        };
        result.index_album_pins();
        result
    }
}

impl OndApp {
    pub(in crate::app) fn project_downloads(&mut self) {
        if self.data.album_copies.is_empty() {
            self.multi.download_view = DownloadView::default();
            return;
        }
        let stores: Vec<_> =
            self.session.accounts.keys().filter(|a| self.session.scope.includes(**a)).filter_map(|a| self.downloads_for(*a).map(|d| (*a, d))).collect();
        let key = (self.data.version, self.data.tracks_version, stores.iter().map(|(a, d)| (*a, d.generation, d.snap.version)).collect());
        if self.multi.download_view.key.as_ref() == Some(&key) {
            return;
        }
        let mut snap = Snapshot {
            limit: self.downloads.snap.limit,
            free_bytes: self.downloads.snap.free_bytes,
            set_aside: self.downloads.snap.set_aside.clone(),
            paused: !stores.is_empty() && stores.iter().all(|(_, d)| d.snap.paused),
            ..Default::default()
        };
        for (_, downloads) in stores {
            let source = &downloads.snap;
            snap.tracks.extend(source.tracks.iter().map(|(k, v)| (k.clone(), v.clone())));
            snap.pins.extend(source.pins.iter().cloned());
            snap.used_bytes += source.used_bytes;
            snap.queued += source.queued;
            snap.active += source.active;
            snap.failed += source.failed;
            snap.online |= source.online;
            snap.limit_reached |= source.limit_reached;
        }
        for (lead, members) in &self.data.album_copies {
            let album_pinned = snap.pins.iter().any(|p| matches!(&p.pin, Pin::Album(id) if members.iter().any(|k| k.to_string() == *id)));
            snap.pins.retain(|p| !matches!(&p.pin, Pin::Album(id) if members.iter().any(|k| k.to_string() == *id)));
            let Some(songs) = self.data.tracks.album(lead) else { continue };
            let mut sum = PinSummary {
                pin: Pin::Album(lead.clone()),
                label: self.data.album(lead).map(|a| a.name.clone()).unwrap_or_default(),
                total: songs.len(),
                done: 0,
                failed: 0,
                bytes_done: 0,
                bytes_total: 0,
                downloading: false,
            };
            for song in songs.iter() {
                let state = self.data.copies_of_track(&song.id).iter().filter_map(|k| snap.track(&k.to_string())).max_by_key(|s| match s {
                    TrackState::Done { .. } => 3,
                    TrackState::Downloading { .. } => 2,
                    TrackState::Queued => 1,
                    TrackState::Failed { .. } => 0,
                });
                match state {
                    Some(TrackState::Done { size }) => {
                        sum.done += 1;
                        sum.bytes_done += size;
                    }
                    Some(TrackState::Downloading { done, .. }) => {
                        sum.downloading = true;
                        sum.bytes_done += done;
                    }
                    Some(TrackState::Failed { .. }) => sum.failed += 1,
                    _ => {}
                }
            }
            if album_pinned || sum.total > 0 && sum.complete() {
                let song_keys: std::collections::HashSet<_> = songs.iter().flat_map(|s| self.data.copies_of_track(&s.id)).map(|k| k.to_string()).collect();
                snap.pins.retain(|p| !matches!(&p.pin, Pin::Track(id) if song_keys.contains(id)));
                snap.pins.push(sum);
            }
        }
        self.multi.download_view.downloads = Some(self.downloads.projected(snap));
        self.multi.download_view.key = Some(key);
    }
}
