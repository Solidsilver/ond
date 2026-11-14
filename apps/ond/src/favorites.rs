//! What the user loved (starred) and rated. The server is the record; this
//! mirrors it so hearts and stars show without asking, and applies changes
//! right away, rolling them back if the server refuses.

pub use crate::source::StarKind;
use library::Track;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Loved things for the Favorites view, newest first.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct Starred {
    pub songs: Vec<Track>,
    pub albums: Vec<String>,
    /// (id, name)
    pub artists: Vec<(String, String)>,
}

/// The favorites and the ratings seen with them, as saved for starting
/// offline.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SavedStarred {
    pub starred: Starred,
    pub ratings: Vec<(String, u8)>,
}

/// Per-song facts seen while loading an album or playlist.
#[derive(Debug, Default, Clone)]
pub struct UserMeta {
    pub starred: Vec<(StarKind, String, bool)>,
    pub ratings: Vec<(String, u8)>,
}

#[derive(Debug, Default)]
pub struct Favorites {
    /// What's loved, by kind: [`is_on`](Self::is_on) asks for every visible
    /// row, and a set of `(kind, id)` would need a `String` made per ask.
    on: HashMap<StarKind, HashSet<String>>,
    ratings: HashMap<String, u8>,
    /// Changes sent but not yet confirmed, by the sequence number of the latest
    /// one per item: a snapshot from the server taken before them mustn't undo
    /// them, and only the latest change's reply settles or rolls back.
    pending: HashMap<(StarKind, String), (u64, bool)>,
    pending_ratings: HashMap<String, u64>,
    seq: u64,
    copies: HashMap<StarKind, HashMap<String, Vec<String>>>,
    preferred: HashMap<String, String>,
    visible_songs: HashMap<String, Track>,
    album_leads: HashMap<String, String>,
    /// Shared so the Favorites view draws it without a copy per frame.
    pub view: Arc<Starred>,
    all: Arc<Starred>,
    scope: accounts::Scope,
    /// `getStarred2` has answered at least once.
    pub loaded: bool,
}

impl Favorites {
    pub fn is_on(&self, kind: StarKind, id: &str) -> bool {
        self.copies.get(&kind).and_then(|groups| groups.get(id)).map_or_else(|| self.raw_on(kind, id), |copies| copies.iter().any(|id| self.raw_on(kind, id)))
    }

    pub fn raw_on(&self, kind: StarKind, id: &str) -> bool {
        self.on.get(&kind).is_some_and(|ids| ids.contains(id))
    }

    pub fn rating(&self, id: &str) -> u8 {
        self.raw_rating(self.preferred.get(id).map_or(id, String::as_str))
    }

    fn raw_rating(&self, id: &str) -> u8 {
        self.ratings.get(id).copied().unwrap_or(0)
    }

    /// Installs aliases without changing any source's own metadata.
    pub fn combine(&mut self, data: &crate::app::Data, prefer: crate::settings::PreferCopy) {
        self.copies.clear();
        self.preferred.clear();
        self.visible_songs.clear();
        self.album_leads.clear();
        if !data.album_copies.is_empty() {
            self.visible_songs.extend(data.tracks.all.iter().map(|song| (song.id.clone(), song.clone())));
        }
        for (kind, groups) in [(StarKind::Song, &data.track_copies), (StarKind::Album, &data.album_copies)] {
            for (lead, members) in groups {
                let ids: Vec<_> = members.iter().map(ToString::to_string).collect();
                for id in &ids {
                    self.copies.entry(kind).or_default().insert(id.clone(), ids.clone());
                    if kind == StarKind::Album {
                        self.album_leads.insert(id.clone(), lead.clone());
                    } else if let Some(song) = data.track(lead) {
                        self.visible_songs.insert(id.clone(), song.clone());
                    }
                }
                let preferred = (kind == StarKind::Song).then(|| data.preferred_track(lead, prefer)).flatten().map(|k| k.to_string());
                if let Some(preferred) = preferred {
                    for id in ids {
                        self.preferred.insert(id, preferred.clone());
                    }
                }
            }
        }
        self.refresh_view();
    }

    /// Flips a heart locally and marks it in flight; returns the new state and
    /// the change's sequence number (for [`settled`](Self::settled) / [`failed`](Self::failed)).
    /// `track` keeps the Favorites view current when a song is loved.
    #[cfg(test)]
    pub fn toggle(&mut self, kind: StarKind, id: &str, track: Option<&Track>, name: Option<&str>) -> (bool, u64) {
        let on = !self.is_on(kind, id);
        let seq = self.set_pending(kind, id, on, track, name);
        (on, seq)
    }

    pub fn set_pending(&mut self, kind: StarKind, id: &str, on: bool, track: Option<&Track>, name: Option<&str>) -> u64 {
        let previous = self.raw_on(kind, id);
        self.set(kind, id, on, track, name);
        self.seq += 1;
        self.pending.insert((kind, id.to_owned()), (self.seq, previous));
        self.seq
    }

    fn set(&mut self, kind: StarKind, id: &str, on: bool, track: Option<&Track>, name: Option<&str>) {
        let v = Arc::make_mut(&mut self.all);
        if on {
            self.on.entry(kind).or_default().insert(id.to_owned());
            match kind {
                StarKind::Song => {
                    if let Some(t) = track
                        && !v.songs.iter().any(|s| s.id == id)
                    {
                        v.songs.insert(0, t.clone());
                    }
                }
                StarKind::Album => {
                    if !v.albums.iter().any(|a| a == id) {
                        v.albums.insert(0, id.to_owned());
                    }
                }
                StarKind::Artist => {
                    if !v.artists.iter().any(|(a, _)| a == id) {
                        v.artists.insert(0, (id.to_owned(), name.unwrap_or(id).to_owned()));
                    }
                }
            }
        } else {
            if let Some(ids) = self.on.get_mut(&kind) {
                ids.remove(id);
            }
            match kind {
                StarKind::Song => v.songs.retain(|s| s.id != id),
                StarKind::Album => v.albums.retain(|a| a != id),
                StarKind::Artist => v.artists.retain(|(a, _)| a != id),
            }
        }
        self.refresh_view();
    }

    fn refresh_view(&mut self) {
        if self.scope == accounts::Scope::all() && self.copies.is_empty() {
            self.view = self.all.clone();
            return;
        }
        let includes = |id: &str| crate::source::identity::account(id).is_none_or(|a| self.scope.includes(a));
        let mut songs = HashSet::new();
        let mut albums = HashSet::new();
        self.view = Arc::new(Starred {
            songs: self
                .all
                .songs
                .iter()
                .filter(|t| includes(&t.id))
                .map(|t| self.visible_songs.get(&t.id).unwrap_or(t))
                .filter(|t| songs.insert(t.id.clone()))
                .cloned()
                .collect(),
            albums: self
                .all
                .albums
                .iter()
                .filter(|id| includes(id))
                .map(|id| self.album_leads.get(id).unwrap_or(id))
                .filter(|id| albums.insert((*id).clone()))
                .cloned()
                .collect(),
            artists: self.all.artists.iter().filter(|(id, _)| includes(id)).cloned().collect(),
        });
    }

    pub fn set_scope(&mut self, scope: accounts::Scope) {
        self.scope = scope;
        self.refresh_view();
    }

    pub fn load_source(&mut self, account: accounts::AccountId, mut starred: Starred, mut ratings: Vec<(String, u8)>) {
        let other = |id: &str| crate::source::identity::account(id) != Some(account);
        starred.songs.extend(self.all.songs.iter().filter(|t| other(&t.id)).cloned());
        starred.albums.extend(self.all.albums.iter().filter(|id| other(id)).cloned());
        starred.artists.extend(self.all.artists.iter().filter(|(id, _)| other(id)).cloned());
        ratings.extend(self.ratings.iter().filter(|(id, _)| other(id)).map(|(id, r)| (id.clone(), *r)));
        self.ratings.retain(|id, _| other(id) || self.pending_ratings.contains_key(id));
        self.load(starred, ratings);
    }

    pub fn remove_account(&mut self, account: accounts::AccountId) {
        let other = |id: &str| crate::source::identity::account(id) != Some(account);
        self.pending.retain(|(_, id), _| other(id));
        self.pending_ratings.retain(|id, _| other(id));
        self.load_source(account, Starred::default(), Vec::new());
    }

    /// The server confirmed change `seq`. An earlier change's reply doesn't
    /// settle a later one still in flight.
    pub fn settled(&mut self, kind: StarKind, id: &str, seq: u64) {
        let key = (kind, id.to_owned());
        if self.pending.get(&key).map(|p| p.0) == Some(seq) {
            self.pending.remove(&key);
        }
    }

    /// The server refused change `seq`: put back what was there, unless a later
    /// change is in flight (that one decides).
    pub fn failed(&mut self, kind: StarKind, id: &str, _wanted: bool, seq: u64, track: Option<&Track>, name: Option<&str>) {
        let key = (kind, id.to_owned());
        if self.pending.get(&key).map(|p| p.0) != Some(seq) {
            return;
        }
        let (_, previous) = self.pending.remove(&key).expect("pending star");
        self.set(kind, id, previous, track, name);
    }

    /// Sets a rating locally; returns the previous one (for rolling back) and
    /// the change's sequence number.
    pub fn rate(&mut self, id: &str, rating: u8) -> (u8, u64) {
        self.seq += 1;
        self.pending_ratings.insert(id.to_owned(), self.seq);
        let before = self.raw_rating(id);
        self.put_rating(id, rating);
        (before, self.seq)
    }

    pub fn rating_settled(&mut self, id: &str, seq: u64) {
        if self.pending_ratings.get(id) == Some(&seq) {
            self.pending_ratings.remove(id);
        }
    }

    /// Rolls back change `seq`, unless a later rating is in flight.
    pub fn rating_failed(&mut self, id: &str, previous: u8, seq: u64) {
        if self.pending_ratings.get(id) != Some(&seq) {
            return;
        }
        self.pending_ratings.remove(id);
        self.put_rating(id, previous);
    }

    fn put_rating(&mut self, id: &str, rating: u8) {
        if rating == 0 {
            self.ratings.remove(id);
        } else {
            self.ratings.insert(id.to_owned(), rating.min(5));
        }
    }

    /// Replaces everything with the server's `getStarred2`, except changes
    /// still in flight.
    pub fn load(&mut self, starred: Starred, ratings: Vec<(String, u8)>) {
        let keep: Vec<((StarKind, String), bool)> = self.pending.keys().map(|k| (k.clone(), self.raw_on(k.0, &k.1))).collect();
        self.on = HashMap::from([
            (StarKind::Song, starred.songs.iter().map(|s| s.id.clone()).collect()),
            (StarKind::Album, starred.albums.iter().cloned().collect()),
            (StarKind::Artist, starred.artists.iter().map(|(a, _)| a.clone()).collect()),
        ]);
        self.all = Arc::new(starred);
        for ((kind, id), on) in keep {
            let track = self.all.songs.iter().find(|s| s.id == id).cloned();
            self.set(kind, &id, on, track.as_ref(), None);
        }
        for (id, r) in ratings {
            if !self.pending_ratings.contains_key(&id) {
                self.put_rating(&id, r);
            }
        }
        self.loaded = true;
        self.refresh_view();
    }

    /// Folds in what an album or playlist load said about its songs.
    pub fn note(&mut self, meta: UserMeta, tracks: &[Track]) {
        for (kind, id, on) in meta.starred {
            if self.pending.contains_key(&(kind, id.clone())) || self.raw_on(kind, &id) == on {
                continue;
            }
            let track = tracks.iter().find(|t| t.id == id);
            self.set(kind, &id, on, track, None);
        }
        for (id, r) in meta.ratings {
            if !self.pending_ratings.contains_key(&id) {
                self.put_rating(&id, r);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str) -> Track {
        crate::test_tracks::track(id, "al", id)
    }

    #[test]
    fn toggle_is_optimistic_and_rolls_back() {
        let mut f = Favorites::default();
        let t = track("s1");
        let (on, seq) = f.toggle(StarKind::Song, "s1", Some(&t), None);
        assert!(on);
        assert!(f.is_on(StarKind::Song, "s1"));
        assert_eq!(f.view.songs.len(), 1);
        // The server said no: back to how it was.
        f.failed(StarKind::Song, "s1", true, seq, Some(&t), None);
        assert!(!f.is_on(StarKind::Song, "s1"));
        assert!(f.view.songs.is_empty());
    }

    #[test]
    fn a_stale_snapshot_does_not_undo_a_change_in_flight() {
        let mut f = Favorites::default();
        let (_, seq) = f.toggle(StarKind::Album, "al1", None, None);
        // getStarred2 answered from before the star landed.
        f.load(Starred::default(), Vec::new());
        assert!(f.is_on(StarKind::Album, "al1"));
        f.settled(StarKind::Album, "al1", seq);
        // Once settled, the server's word goes.
        f.load(Starred::default(), Vec::new());
        assert!(!f.is_on(StarKind::Album, "al1"));
    }

    #[test]
    fn load_replaces_and_note_merges() {
        let mut f = Favorites::default();
        f.load(Starred { songs: vec![track("a")], albums: vec!["al".into()], artists: vec![("ar".into(), "Bill".into())] }, vec![("a".into(), 4)]);
        assert!(f.is_on(StarKind::Song, "a") && f.is_on(StarKind::Album, "al") && f.is_on(StarKind::Artist, "ar"));
        assert_eq!(f.rating("a"), 4);
        f.note(
            UserMeta { starred: vec![(StarKind::Song, "b".into(), true), (StarKind::Song, "a".into(), false)], ratings: vec![("b".into(), 2)] },
            &[track("b")],
        );
        assert!(f.is_on(StarKind::Song, "b"));
        assert!(!f.is_on(StarKind::Song, "a"));
        assert_eq!(f.view.songs.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["b"]);
        assert_eq!(f.rating("b"), 2);
    }

    #[test]
    fn ratings_roll_back_and_clear() {
        let mut f = Favorites::default();
        let (before, seq) = f.rate("s", 5);
        assert_eq!(before, 0);
        assert_eq!(f.rating("s"), 5);
        // A server snapshot mid-flight keeps the new rating.
        f.load(Starred::default(), vec![("s".into(), 1)]);
        assert_eq!(f.rating("s"), 5);
        f.rating_failed("s", 0, seq);
        assert_eq!(f.rating("s"), 0);
        let (_, seq) = f.rate("s", 3);
        f.rating_settled("s", seq);
        f.rate("s", 0);
        assert_eq!(f.rating("s"), 0);
    }

    #[test]
    fn only_the_latest_change_settles_or_rolls_back() {
        let mut f = Favorites::default();
        // Rate 3, then 5, quickly. The 3 fails after the 5 went through.
        let (p3, s3) = f.rate("s", 3);
        let (_, s5) = f.rate("s", 5);
        f.rating_settled("s", s5);
        f.rating_failed("s", p3, s3);
        assert_eq!(f.rating("s"), 5, "a stale failure doesn't roll back a newer rating");
        // The 3's success arriving late doesn't clear protection for a newer one.
        let (_, s4) = f.rate("s", 4);
        let (_, s2) = f.rate("s", 2);
        f.rating_settled("s", s4);
        f.load(Starred::default(), vec![("s".into(), 4)]);
        assert_eq!(f.rating("s"), 2, "still in flight");
        f.rating_settled("s", s2);

        // Hearts: love, unlove; the love's reply comes last and fails.
        let (_, love) = f.toggle(StarKind::Song, "x", None, None);
        let (_, unlove) = f.toggle(StarKind::Song, "x", None, None);
        f.settled(StarKind::Song, "x", unlove);
        f.failed(StarKind::Song, "x", true, love, None, None);
        assert!(!f.is_on(StarKind::Song, "x"));
    }
}
