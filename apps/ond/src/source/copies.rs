//! Pure copy matching. Inputs are already scoped and ordered by source.
//! Known release ids are grouped before missing ids, so an untagged copy
//! cannot bridge incompatible editions. Every group has at most one member
//! per source; output members return to input order to choose real leads.

use super::artists::{clean_mbid, normalize_name};
use accounts::ItemKey;
use std::collections::HashMap;

pub struct AlbumIdentity {
    pub key: ItemKey,
    /// An opaque, stable name for the merged album artist group(s).
    pub artist: String,
    pub title: String,
    pub release: Option<String>,
}

pub struct SongIdentity {
    pub key: ItemKey,
    pub title: String,
    pub disc: Option<u32>,
    pub track: Option<u32>,
    pub recording: Option<String>,
}

fn mbid(id: Option<&str>) -> Option<String> {
    clean_mbid(id).map(|s| s.to_lowercase())
}

/// Indices into `albums`, lead first. Year and completeness aren't identity.
#[cfg(test)]
pub fn albums(albums: &[AlbumIdentity]) -> Vec<Vec<usize>> {
    albums_apart(albums, &crate::store::ApartPairs::new())
}

pub fn albums_apart(albums: &[AlbumIdentity], apart: &crate::store::ApartPairs) -> Vec<Vec<usize>> {
    let ids: Vec<_> = albums.iter().map(|a| mbid(a.release.as_deref())).collect();
    let names: Vec<_> = albums.iter().map(|a| (a.artist.clone(), normalize_name(&a.title))).collect();
    let mut order: Vec<_> = (0..albums.len()).collect();
    order.sort_by_key(|&i| ids[i].is_none());
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut by_id = HashMap::<&str, Vec<usize>>::new();
    let mut by_name = HashMap::<&(String, String), Vec<usize>>::new();
    for i in order {
        let candidates = ids[i].as_deref().and_then(|id| by_id.get(id)).into_iter().flatten().chain(by_name.get(&names[i]).into_iter().flatten());
        let found = candidates.copied().find(|&g| {
            groups[g].iter().all(|&j| {
                albums[i].key.account != albums[j].key.account
                    && !apart.contains(&crate::store::album_pair(albums[i].key.clone(), albums[j].key.clone()))
                    && match (&ids[i], &ids[j]) {
                        (Some(a), Some(b)) => a == b,
                        _ => names[i] == names[j],
                    }
            })
        });
        let g = found.unwrap_or_else(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[g].push(i);
        if let Some(id) = ids[i].as_deref() {
            by_id.entry(id).or_default().push(g);
        }
        by_name.entry(&names[i]).or_default().push(g);
    }
    ordered(groups)
}

/// Matches only inside one combined album. Repeated unnumbered titles stay
/// separate, and numbered songs need both their position and whole title.
pub fn songs(songs: &[SongIdentity]) -> Vec<Vec<usize>> {
    let ids: Vec<_> = songs.iter().map(|s| mbid(s.recording.as_deref())).collect();
    let titles: Vec<_> = songs.iter().map(|s| normalize_name(&s.title)).collect();
    let positions: Vec<_> = songs.iter().map(|s| s.track.filter(|&n| n > 0).map(|n| (s.disc.filter(|&d| d > 0).unwrap_or(1), n))).collect();
    let mut counts = HashMap::new();
    for (s, title) in songs.iter().zip(&titles) {
        *counts.entry((s.key.account, title)).or_insert(0usize) += 1;
    }
    let mut order: Vec<_> = (0..songs.len()).collect();
    order.sort_by_key(|&i| ids[i].is_none());
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut by_id = HashMap::<&str, Vec<usize>>::new();
    let mut by_title = HashMap::<&str, Vec<usize>>::new();
    for i in order {
        let candidates = ids[i].as_deref().and_then(|id| by_id.get(id)).into_iter().flatten().chain(by_title.get(titles[i].as_str()).into_iter().flatten());
        let found = candidates.copied().find(|&g| {
            groups[g].iter().all(|&j| {
                songs[i].key.account != songs[j].key.account
                    && (ids[i].is_some() && ids[i] == ids[j]
                        || titles[i] == titles[j]
                            && match (positions[i], positions[j]) {
                                (Some(a), Some(b)) => a == b,
                                _ => counts[&(songs[i].key.account, &titles[i])] == 1 && counts[&(songs[j].key.account, &titles[j])] == 1,
                            })
            })
        });
        let g = found.unwrap_or_else(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[g].push(i);
        if let Some(id) = ids[i].as_deref() {
            by_id.entry(id).or_default().push(g);
        }
        by_title.entry(&titles[i]).or_default().push(g);
    }
    ordered(groups)
}

fn ordered(mut groups: Vec<Vec<usize>>) -> Vec<Vec<usize>> {
    for group in &mut groups {
        group.sort_unstable();
    }
    groups.sort_by_key(|g| g[0]);
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(source: u128, id: &str) -> ItemKey {
        ItemKey::new(format!("00000000-0000-0000-0000-{source:012x}").parse().unwrap(), id)
    }

    fn album(source: u128, title: &str, release: Option<&str>) -> AlbumIdentity {
        AlbumIdentity { key: key(source, title), artist: "artist group".into(), title: title.into(), release: release.map(str::to_owned) }
    }

    fn song(source: u128, title: &str, track: Option<u32>, recording: Option<&str>) -> SongIdentity {
        SongIdentity { key: key(source, title), title: title.into(), disc: Some(1), track, recording: recording.map(str::to_owned) }
    }

    #[test]
    fn equal_release_ids_win_over_album_titles_and_artist_spellings() {
        let one = album(1, "First spelling", Some(" RELEASE "));
        let mut two = album(2, "Other title", Some("release"));
        two.artist = "another group".into();
        assert_eq!(albums(&[one, two]), [vec![0, 1]]);
    }

    #[test]
    fn missing_ids_never_bridge_different_releases_even_when_they_arrive_first() {
        assert_eq!(albums(&[album(1, "Record", None), album(2, "Record", Some("one")), album(3, "Record", Some("two"))]), [vec![0, 1], vec![2]]);
    }

    #[test]
    fn albums_of_one_source_stay_separate_even_with_equal_release_ids() {
        assert_eq!(albums(&[album(1, "Record", Some("one")), album(1, "Record", Some("one")), album(2, "Record", Some("one"))]), [vec![0, 2], vec![1]]);
    }

    #[test]
    fn missing_album_ids_use_the_merged_artist_and_whole_folded_title() {
        let mut other_artist = album(3, "Bjork and Friends", None);
        other_artist.artist = "homonym group".into();
        assert_eq!(
            albums(&[
                album(1, "The BJÖRK & Friends", Some("")),
                album(2, "bjork and friends", Some("   ")),
                album(3, "Bjork and Friends (Deluxe)", None),
                other_artist,
            ]),
            [vec![0, 1], vec![2], vec![3]]
        );
    }

    #[test]
    fn a_known_release_on_a_later_source_does_not_take_the_lead() {
        assert_eq!(albums(&[album(1, "Record", None), album(2, "Record", Some("known"))]), [vec![0, 1]]);
    }

    #[test]
    fn equal_recordings_win_over_song_names_and_numbering() {
        assert_eq!(songs(&[song(1, "First", Some(1), Some(" RECORDING ")), song(2, "Other title", Some(9), Some("recording"))]), [vec![0, 1]]);
    }

    #[test]
    fn numbered_songs_need_both_disc_and_track_and_the_whole_title() {
        let mut second_disc = song(3, "One", Some(1), None);
        second_disc.disc = Some(2);
        assert_eq!(
            songs(&[
                song(1, "The Öne", Some(1), None),
                song(2, "one", Some(1), None),
                second_disc,
                song(4, "One", Some(2), None),
                song(5, "One (Live)", Some(1), None),
            ]),
            [vec![0, 1], vec![2], vec![3], vec![4]]
        );
    }

    #[test]
    fn absent_or_zero_disc_means_the_first_disc() {
        let mut one = song(1, "One", Some(1), None);
        one.disc = None;
        let mut two = song(2, "One", Some(1), None);
        two.disc = Some(0);
        assert_eq!(songs(&[one, two, song(3, "One", Some(1), None)]), [vec![0, 1, 2]]);
    }

    #[test]
    fn unnumbered_titles_match_only_when_unique_on_each_album_copy() {
        assert_eq!(songs(&[song(1, "One", None, None), song(2, "Öne", Some(0), Some(""))]), [vec![0, 1]]);
        assert_eq!(songs(&[song(1, "One", None, None), song(1, "One", None, None), song(2, "One", None, None)]), [vec![0], vec![1], vec![2]]);
    }

    #[test]
    fn one_unnumbered_copy_can_join_a_unique_numbered_song() {
        assert_eq!(songs(&[song(1, "One", None, None), song(2, "One", Some(1), None)]), [vec![0, 1]]);
        assert_eq!(songs(&[song(1, "One", None, None), song(2, "One", Some(1), None), song(2, "One", Some(2), None)]), [vec![0], vec![1], vec![2]]);
    }

    #[test]
    fn recordings_without_an_equal_id_use_the_position_and_title_fallback() {
        assert_eq!(songs(&[song(1, "One", Some(1), Some("one")), song(2, "One", Some(1), Some("two"))]), [vec![0, 1]]);
    }

    #[test]
    fn repeated_recordings_on_one_source_keep_their_own_rows() {
        assert_eq!(
            songs(&[song(1, "One", Some(1), Some("r")), song(1, "Reprise", Some(2), Some("r")), song(2, "One", Some(1), Some("r"))]),
            [vec![0, 2], vec![1]]
        );
    }
}
