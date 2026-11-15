//! Ids at the boundary between a source's bare rows and Ond's shared memory.
//!
//! Views already pass ids through routes, selections and memos as strings.
//! In memory those strings are the canonical `ItemKey` spelling, including
//! links and cover ids. Keeping that spelling lets the existing widgets stay
//! simple without changing any frozen library row. Only source boundaries
//! strip the account; no qualified row is ever written back to a mirror.

use accounts::{AccountId, ItemKey};
use library::{Album, Artist, Track};

pub fn account(id: &str) -> Option<AccountId> {
    id.split_once('/')?.0.parse().ok()
}

pub fn bare(id: &str) -> &str {
    if account(id).is_some() { id.split_once('/').expect("qualified id").1 } else { id }
}

pub fn key(fallback: AccountId, id: &str) -> ItemKey {
    ItemKey::parse(id).unwrap_or_else(|_| ItemKey::new(fallback, id))
}

pub fn qualify(account: AccountId, id: &mut String) {
    *id = ItemKey::new(account, &*id).to_string();
}

fn optional(account: AccountId, id: &mut Option<String>) {
    if let Some(id) = id {
        qualify(account, id);
    }
}

pub fn album(account: AccountId, a: &mut Album) {
    qualify(account, &mut a.id);
    optional(account, &mut a.artist_ref);
    optional(account, &mut a.cover_art);
    for artist in &mut a.artists {
        optional(account, &mut artist.id);
    }
}

pub fn artist(account: AccountId, a: &mut Artist) {
    qualify(account, &mut a.id);
    optional(account, &mut a.cover_art);
}

pub fn track(account: AccountId, t: &mut Track) {
    qualify(account, &mut t.id);
    qualify(account, &mut t.album_id);
    optional(account, &mut t.artist_ref);
    optional(account, &mut t.cover_art);
    for artist in &mut t.artists {
        optional(account, &mut artist.id);
    }
    for credit in &mut t.contributors {
        optional(account, &mut credit.artist_id);
    }
}

pub fn playlist(account: AccountId, p: &mut crate::source::Playlist) {
    qualify(account, &mut p.id);
    optional(account, &mut p.cover_art);
}

pub fn starred(account: AccountId, s: &mut crate::favorites::Starred) {
    for t in &mut s.songs {
        track(account, t);
    }
    for id in &mut s.albums {
        qualify(account, id);
    }
    for (id, _) in &mut s.artists {
        qualify(account, id);
    }
}

pub fn meta(account: AccountId, m: &mut crate::favorites::UserMeta) {
    for (_, id, _) in &mut m.starred {
        qualify(account, id);
    }
    ratings(account, &mut m.ratings);
}

pub fn ratings(account: AccountId, ratings: &mut [(String, u8)]) {
    for (id, _) in ratings {
        qualify(account, id);
    }
}

/// Qualifies worker results as they leave a live source session. The
/// bridge's stored copies and every write retain the bare format; indexing
/// the qualified tracks stays off the UI thread.
pub fn message(account: AccountId, msg: crate::bridge::Msg) -> crate::bridge::Msg {
    use crate::bridge::{LibraryMsg as L, Msg};
    let q = |id: &mut String| qualify(account, id);
    let tracks = |ts: &mut Vec<Track>| {
        for t in ts {
            track(account, t);
        }
    };
    match msg {
        Msg::Scoped { session, message: inner } => Msg::Scoped { session, message: Box::new(message(account, *inner)) },
        Msg::Library(mut m) => {
            match &mut m {
                L::ArtistMbids(rows) | L::AlbumMbids(rows) | L::RecordingMbids(rows) => {
                    for (id, _) in rows {
                        q(id);
                    }
                }
                L::Albums(albums) => {
                    for a in albums {
                        album(account, a);
                    }
                }
                L::AlbumUpdated(a) => album(account, a),
                L::Artists(artists) => {
                    for a in artists {
                        artist(account, a);
                    }
                }
                L::AllTracks(set) => {
                    let mut ts = set.all.as_ref().clone();
                    tracks(&mut ts);
                    **set = crate::tracks::TrackSet::new(ts);
                }
                L::Tracks { album_id, tracks: ts, .. } => {
                    q(album_id);
                    tracks(ts);
                }
                L::PlaylistTracks { id, tracks: ts, .. } => {
                    q(id);
                    tracks(ts);
                }
                L::Newest(ids) => {
                    for id in ids {
                        q(id);
                    }
                }
                L::Playlists(ps) => {
                    for p in ps {
                        playlist(account, p);
                    }
                }
                L::Saved(saved) => {
                    if let Some(ids) = &mut saved.newest {
                        for id in ids {
                            q(id);
                        }
                    }
                    if let Some(ps) = &mut saved.playlists {
                        for p in ps {
                            playlist(account, p);
                        }
                    }
                    if let Some(s) = &mut saved.starred {
                        starred(account, &mut s.starred);
                        ratings(account, &mut s.ratings);
                    }
                }
                L::Credited { album_ids, .. } => {
                    for id in album_ids {
                        q(id);
                    }
                }
                L::TrackBatch { tracks: ts, key } => {
                    list_key(account, key);
                    tracks(ts);
                }
                L::LoadFailed { key, .. } => list_key(account, key),
                _ => {}
            }
            Msg::Library(m)
        }
        Msg::MixedPlaylistReady { mut id, name, mut existing, added } => {
            q(&mut id);
            tracks(&mut existing);
            Msg::MixedPlaylistReady { id, name, existing, added }
        }
        Msg::Starred { starred: mut s, ratings: mut rs } => {
            starred(account, &mut s);
            ratings(account, &mut rs);
            Msg::Starred { starred: s, ratings: rs }
        }
        Msg::UserMeta { meta: mut m, tracks: mut ts } => {
            meta(account, &mut m);
            tracks(&mut ts);
            Msg::UserMeta { meta: m, tracks: ts }
        }
        Msg::Lyrics { mut track_id, lyrics } => {
            q(&mut track_id);
            Msg::Lyrics { track_id, lyrics }
        }
        Msg::SongStats { mut track_id, stats } => {
            q(&mut track_id);
            Msg::SongStats { track_id, stats }
        }
        Msg::StarDone { kind, mut id, on, seq, error } => {
            q(&mut id);
            Msg::StarDone { kind, id, on, seq, error }
        }
        Msg::RatingDone { mut id, previous, seq, error } => {
            q(&mut id);
            Msg::RatingDone { id, previous, seq, error }
        }
        Msg::PlaylistCreated { mut id, name } => {
            q(&mut id);
            Msg::PlaylistCreated { id, name }
        }
        Msg::Analysis(crate::bridge::AnalysisMsg::Loaded(mut rows)) => {
            for (id, _) in &mut rows {
                q(id);
            }
            Msg::Analysis(crate::bridge::AnalysisMsg::Loaded(rows))
        }
        m => m,
    }
}

fn list_key(account: AccountId, key: &mut crate::bridge::ListKey) {
    match key {
        crate::bridge::ListKey::Album(id) | crate::bridge::ListKey::Playlist(id) => qualify(account, id),
        crate::bridge::ListKey::Artist(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_id_that_already_looks_qualified_still_belongs_to_its_source() {
        let (one, two) = (AccountId::new(), AccountId::new());
        let raw = ItemKey::new(two, "1").to_string();
        let mut id = raw.clone();
        qualify(one, &mut id);
        assert_eq!(ItemKey::parse(&id).unwrap(), ItemKey::new(one, &raw));
        assert_eq!(bare(&id), raw);
    }
}
