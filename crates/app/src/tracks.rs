//! Every stored track held once, shared: albums are ranges into one list, and
//! clones of an album's tracks are reference bumps, not copies.

use library::Track;
use std::collections::HashMap;
use std::ops::{Deref, Range};
use std::sync::Arc;

/// Some tracks of a shared list: an album's run inside the library-wide list,
/// or a list fetched on its own. Cheap to clone; reads as `[Track]`.
#[derive(Clone, Default)]
pub struct TrackSlice {
    all: Arc<Vec<Track>>,
    range: Range<usize>,
}

impl TrackSlice {
    /// A list of its own (an album or playlist just fetched).
    pub fn owned(tracks: Vec<Track>) -> Self {
        let range = 0..tracks.len();
        Self { all: Arc::new(tracks), range }
    }

    fn within(all: &Arc<Vec<Track>>, range: Range<usize>) -> Self {
        Self { all: Arc::clone(all), range }
    }
}

impl Deref for TrackSlice {
    type Target = [Track];

    fn deref(&self) -> &[Track] {
        &self.all[self.range.clone()]
    }
}

impl std::fmt::Debug for TrackSlice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter().map(|t| &t.id)).finish()
    }
}

/// Where a track is: its album's slot (see [`TrackSet::slot`]) and its place
/// in that album's list. A number rather than the album's id, so the index
/// holds one string per track (its own id), not two.
type Location = (u32, usize);

/// The library's tracks, grouped by album and indexed by id. Building one walks
/// every track, so the bridge does it off the UI thread.
#[derive(Default)]
pub struct TrackSet {
    /// Every stored track, grouped by album (the Songs view sorts these).
    /// Albums fetched since (`set_album`) aren't in it.
    pub all: Arc<Vec<Track>>,
    /// Each album's tracks: its run in `all`, or the list fetched since.
    pub by_album: HashMap<String, TrackSlice>,
    index: HashMap<String, Location>,
    /// Album ids by slot, and each album's slot: `index` refers to albums by
    /// slot. Slots are never reused, so a stale entry can't point elsewhere.
    album_ids: Vec<String>,
    slots: HashMap<String, u32>,
}

impl TrackSet {
    /// Groups `tracks`, which the library returns album by album. An album
    /// split across runs still works: its runs are joined into one list.
    pub fn new(tracks: Vec<Track>) -> Self {
        let all = Arc::new(tracks);
        let mut by_album: HashMap<String, TrackSlice> = HashMap::new();
        let mut start = 0;
        for i in 1..=all.len() {
            if i < all.len() && all[i].album_id == all[start].album_id {
                continue;
            }
            let id = &all[start].album_id;
            let slice = match by_album.remove(id) {
                Some(earlier) => {
                    let mut joined = earlier.to_vec();
                    joined.extend_from_slice(&all[start..i]);
                    TrackSlice::owned(joined)
                }
                None => TrackSlice::within(&all, start..i),
            };
            by_album.insert(id.clone(), slice);
            start = i;
        }
        let mut set = Self { all, by_album, ..Self::default() };
        set.index.reserve(set.all.len());
        let albums: Vec<String> = set.by_album.keys().cloned().collect();
        for album in albums {
            set.index_album(&album);
        }
        set
    }

    /// Replaces one album's tracks (fresh from the server).
    pub fn set_album(&mut self, album_id: &str, tracks: Vec<Track>) {
        if let Some(old) = self.by_album.get(album_id) {
            let slot = self.slots.get(album_id).copied();
            for t in old.iter() {
                if self.index.get(&t.id).is_some_and(|(a, _)| Some(*a) == slot) {
                    self.index.remove(&t.id);
                }
            }
        }
        self.by_album.insert(album_id.to_owned(), TrackSlice::owned(tracks));
        self.index_album(album_id);
    }

    /// `album_id`'s slot, given one the first time it's asked for.
    fn slot(&mut self, album_id: &str) -> u32 {
        if let Some(&slot) = self.slots.get(album_id) {
            return slot;
        }
        let slot = u32::try_from(self.album_ids.len()).expect("fewer than 4 billion albums");
        self.album_ids.push(album_id.to_owned());
        self.slots.insert(album_id.to_owned(), slot);
        slot
    }

    fn index_album(&mut self, album_id: &str) {
        if !self.by_album.contains_key(album_id) {
            return;
        }
        let slot = self.slot(album_id);
        let tracks = &self.by_album[album_id];
        for (i, t) in tracks.iter().enumerate() {
            self.index.insert(t.id.clone(), (slot, i));
        }
    }

    /// An album's tracks, stored or fetched since.
    pub fn album(&self, id: &str) -> Option<&TrackSlice> {
        self.by_album.get(id)
    }

    /// A track by id, stored or fetched since.
    pub fn track(&self, id: &str) -> Option<&Track> {
        let &(slot, i) = self.index.get(id)?;
        self.by_album.get(self.album_ids.get(slot as usize)?)?.get(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str, album: &str) -> Track {
        Track {
            id: id.into(),
            album_id: album.into(),
            title: id.into(),
            artist: None,
            artist_ref: None,
            track: None,
            disc: None,
            duration_secs: None,
            suffix: None,
            content_type: None,
            size: None,
            bit_rate_kbps: None,
            bit_depth: None,
            sample_rate: None,
            channels: None,
            track_gain: None,
            album_gain: None,
            track_peak: None,
            album_peak: None,
            composer: None,
            contributors: Vec::new(),
            cover_art: None,
            artists: Vec::new(),
        }
    }

    #[test]
    fn albums_share_the_one_list_and_ids_resolve() {
        let set = TrackSet::new(vec![t("1", "a"), t("2", "a"), t("3", "b")]);
        let a = &set.by_album["a"];
        assert_eq!(a.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["1", "2"]);
        assert!(Arc::ptr_eq(&a.all, &set.all), "no second copy of the tracks");
        assert_eq!(set.track("3").map(|t| t.album_id.as_str()), Some("b"));
        assert!(set.track("nope").is_none());
    }

    #[test]
    fn an_album_split_across_runs_is_joined() {
        let set = TrackSet::new(vec![t("1", "a"), t("2", "b"), t("3", "a")]);
        assert_eq!(set.by_album["a"].iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["1", "3"]);
        assert_eq!(set.track("3").map(|t| t.id.as_str()), Some("3"));
        assert_eq!(set.track("2").map(|t| t.album_id.as_str()), Some("b"));
    }

    #[test]
    fn a_track_moved_to_another_album_is_found_there() {
        let mut set = TrackSet::new(vec![t("1", "a"), t("2", "a"), t("3", "b")]);
        set.set_album("b", vec![t("3", "b"), t("2", "b")]);
        // Album a's fresh list no longer has 2; it stays found in b.
        set.set_album("a", vec![t("1", "a")]);
        assert_eq!(set.track("2").map(|t| t.album_id.as_str()), Some("b"));
        assert_eq!(set.track("1").map(|t| t.album_id.as_str()), Some("a"));
        // A new album gets a slot of its own.
        set.set_album("c", vec![t("9", "c")]);
        assert_eq!(set.track("9").map(|t| t.album_id.as_str()), Some("c"));
        assert_eq!(set.track("3").map(|t| t.album_id.as_str()), Some("b"));
    }

    #[test]
    fn a_fresh_album_replaces_its_tracks_in_the_index() {
        let mut set = TrackSet::new(vec![t("1", "a"), t("2", "a")]);
        set.set_album("a", vec![t("2", "a"), t("9", "a")]);
        assert!(set.track("1").is_none(), "dropped from the album");
        assert_eq!(set.track("9").map(|t| t.id.as_str()), Some("9"));
        assert_eq!(set.by_album["a"].len(), 2);
    }
}
