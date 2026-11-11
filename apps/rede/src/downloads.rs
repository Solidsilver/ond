//! Account-owned downloads and the file identities shared with streaming.
//! The store wakes the bridge only when its snapshot changes; views read the
//! retained snapshot without starting timers or touching the disk.

use offline::{OfflineStore, Pin, Snapshot, TrackRef};
use sources::{Item, ItemKey, SessionTrack, TrackFile};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// The persistent identity of an original audio file in both caches.
pub fn track_id(key: &ItemKey, file: Option<&TrackFile>, index: u32) -> String {
    let id = file.filter(|f| !f.content_id.is_empty()).map_or_else(|| format!("{}/file/{index}", key.item), |f| f.content_id.clone());
    ItemKey::new(key.account, id).to_string()
}

pub fn tracks(key: &ItemKey, item: &Item) -> Vec<TrackRef> {
    item.tracks.iter().map(|f| TrackRef::new(track_id(key, Some(f), f.index), f.suffix.clone(), f.size)).collect()
}

/// Shared with the listening worker, installed by a successful scoped open.
#[derive(Clone, Default)]
pub struct Store(Arc<Mutex<Option<OfflineStore>>>);
impl Store {
    pub fn get(&self) -> Option<OfflineStore> {
        self.0.lock().unwrap().clone()
    }
    pub fn install(&self, store: OfflineStore) {
        *self.0.lock().unwrap() = Some(store);
    }

    /// Called on the bridge's download queue before a folder is reopened.
    pub fn clear(&self) {
        let store = self.0.lock().unwrap().take();
        drop(store);
    }

    /// A book plays from disk only as a whole, so a missing second file never
    /// surprises the listener hours after an offline play press.
    pub fn local_tracks(&self, key: &ItemKey, item: &Item) -> Option<Vec<SessionTrack>> {
        let held = self.0.lock().unwrap();
        let store = held.as_ref()?;
        if item.tracks.is_empty() || !store.snapshot().pin(&Pin::Book(key.to_string()))?.complete() {
            return None;
        }
        item.tracks
            .iter()
            .map(|f| {
                let path = store.local_path(&track_id(key, Some(f), f.index))?;
                let url = url::Url::from_file_path(path).ok()?.to_string();
                Some(SessionTrack { index: f.index, start_offset: f.start_offset, duration: f.duration, size: f.size, mime: f.mime.clone(), url })
            })
            .collect()
    }
}

#[derive(Default)]
pub struct State {
    pub store: Store,
    pub snapshot: Arc<Snapshot>,
    /// A details fetch comes before the store can queue a pin.
    pub preparing: HashMap<ItemKey, u64>,
    pub errors: HashMap<ItemKey, String>,
    pub refreshing: bool,
    pub refreshed: bool,
    pub serial: u64,
    pub remove_after_stop: HashSet<ItemKey>,
}

impl State {
    #[allow(dead_code, reason = "The download views are implemented in the next phase")]
    pub fn book(&self, key: &ItemKey) -> Option<&offline::PinSummary> {
        self.snapshot.pin(&Pin::Book(key.to_string()))
    }
    pub fn refresh(&mut self) {
        let snapshot = self.store.get().map_or_else(|| Arc::new(Snapshot::default()), |s| s.snapshot());
        if snapshot.tracks.iter().any(|(id, state)| matches!(state, offline::TrackState::Done { .. }) && !self.snapshot.is_done(id)) {
            self.refreshed = false;
        }
        self.snapshot = snapshot;
    }
}
