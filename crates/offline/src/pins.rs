//! What's pinned and which tracks that needs: pure bookkeeping, no IO.
//!
//! A track is kept while at least one pin includes it, so an album and a
//! playlist sharing a song share one file, and unpinning one keeps the song for
//! the other.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

/// Something the user asked to keep offline.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Pin {
    Album(String),
    Playlist(String),
    Track(String),
}

/// A track to download: the server's id, file suffix and size (when known, for
/// verifying the download and the storage limit).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackRef {
    pub id: String,
    #[serde(default)]
    pub suffix: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

impl TrackRef {
    pub fn new(id: impl Into<String>, suffix: Option<String>, size: Option<u64>) -> Self {
        Self { id: id.into(), suffix, size }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinRecord {
    pub pin: Pin,
    /// Album or playlist name, for showing pins while offline.
    pub label: String,
    /// Track ids, in the pin's order.
    pub tracks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stored {
    pub r: TrackRef,
    /// Bytes on disk once the download completed.
    #[serde(default)]
    pub done: Option<u64>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Pins {
    /// In the order they were pinned; downloads follow it.
    pub pins: Vec<PinRecord>,
    pub tracks: BTreeMap<String, Stored>,
    #[serde(default)]
    pub paused: bool,
}

impl Pins {
    /// Adds `pin`, or replaces its track list (a playlist that changed on the
    /// server). Returns the track ids no pin needs any more.
    pub fn set(&mut self, pin: Pin, label: String, tracks: Vec<TrackRef>) -> Vec<String> {
        let mut ids = Vec::with_capacity(tracks.len());
        let mut seen = HashSet::new();
        for t in tracks {
            if !seen.insert(t.id.clone()) {
                continue;
            }
            ids.push(t.id.clone());
            match self.tracks.get_mut(&t.id) {
                // A finished file keeps its name (suffix); only refresh what's not on disk yet.
                Some(s) if s.done.is_none() => s.r = t,
                Some(_) => {}
                None => {
                    self.tracks.insert(t.id.clone(), Stored { r: t, done: None });
                }
            }
        }
        match self.pins.iter_mut().find(|p| p.pin == pin) {
            Some(p) => {
                p.label = label;
                p.tracks = ids;
            }
            None => self.pins.push(PinRecord { pin, label, tracks: ids }),
        }
        self.gc()
    }

    /// Removes `pin`; returns the track ids nothing else keeps.
    pub fn remove(&mut self, pin: &Pin) -> Vec<String> {
        self.pins.retain(|p| &p.pin != pin);
        self.gc()
    }

    pub fn is_pinned(&self, pin: &Pin) -> bool {
        self.pins.iter().any(|p| &p.pin == pin)
    }

    pub fn record(&self, pin: &Pin) -> Option<&PinRecord> {
        self.pins.iter().find(|p| &p.pin == pin)
    }

    /// How many pins include track `id`.
    pub fn refs(&self, id: &str) -> usize {
        self.pins.iter().filter(|p| p.tracks.iter().any(|t| t == id)).count()
    }

    /// Track ids still to download, in pin order, each once.
    pub fn pending(&self) -> Vec<&str> {
        let mut seen = HashSet::new();
        self.pins
            .iter()
            .flat_map(|p| p.tracks.iter())
            .filter(|id| self.tracks.get(*id).is_some_and(|s| s.done.is_none()))
            .filter(|id| seen.insert(id.as_str()))
            .map(String::as_str)
            .collect()
    }

    /// Bytes of finished files.
    pub fn used(&self) -> u64 {
        self.tracks.values().filter_map(|s| s.done).sum()
    }

    fn gc(&mut self) -> Vec<String> {
        let needed: HashSet<&String> = self.pins.iter().flat_map(|p| p.tracks.iter()).collect();
        let gone: Vec<String> = self.tracks.keys().filter(|id| !needed.contains(id)).cloned().collect();
        for id in &gone {
            self.tracks.remove(id);
        }
        gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(ids: &[&str]) -> Vec<TrackRef> {
        ids.iter().map(|id| TrackRef::new(*id, Some("flac".into()), Some(100))).collect()
    }

    #[test]
    fn shared_tracks_are_refcounted() {
        let mut p = Pins::default();
        assert!(p.set(Pin::Album("a".into()), "A".into(), refs(&["1", "2", "3"])).is_empty());
        assert!(p.set(Pin::Playlist("p".into()), "P".into(), refs(&["3", "4"])).is_empty());
        assert_eq!(p.refs("3"), 2);
        assert_eq!(p.pending(), ["1", "2", "3", "4"]);
        // Unpinning the album keeps the track the playlist still has.
        let mut gone = p.remove(&Pin::Album("a".into()));
        gone.sort();
        assert_eq!(gone, ["1", "2"]);
        assert_eq!(p.refs("3"), 1);
        assert!(p.tracks.contains_key("3"));
        assert_eq!(p.remove(&Pin::Playlist("p".into())), ["3", "4"]);
        assert!(p.tracks.is_empty());
    }

    #[test]
    fn playlist_membership_follows_the_server() {
        let mut p = Pins::default();
        p.set(Pin::Playlist("p".into()), "P".into(), refs(&["1", "2", "3"]));
        p.set(Pin::Track("2".into()), "Two".into(), refs(&["2"]));
        // Resync: 1 and 2 left the playlist, 5 joined. 2 is still pinned on its own.
        let gone = p.set(Pin::Playlist("p".into()), "P (renamed)".into(), refs(&["3", "5"]));
        assert_eq!(gone, ["1"]);
        assert_eq!(p.record(&Pin::Playlist("p".into())).unwrap().label, "P (renamed)");
        assert_eq!(p.pending(), ["3", "5", "2"]);
    }

    #[test]
    fn finished_files_keep_their_name_and_duplicates_collapse() {
        let mut p = Pins::default();
        p.set(Pin::Album("a".into()), "A".into(), refs(&["1", "1"]));
        assert_eq!(p.record(&Pin::Album("a".into())).unwrap().tracks, ["1"]);
        p.tracks.get_mut("1").unwrap().done = Some(100);
        p.set(Pin::Album("a".into()), "A".into(), vec![TrackRef::new("1", Some("mp3".into()), Some(5))]);
        assert_eq!(p.tracks["1"].r.suffix.as_deref(), Some("flac"));
        assert!(p.pending().is_empty());
        assert_eq!(p.used(), 100);
    }
}
