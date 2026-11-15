//! Artist matching is independent of servers and storage. MusicBrainz ids
//! win when both artists have one; otherwise their normalized names match.
//! Members retain their own keys so navigation and writes can still address
//! the original entity, and each page can group albums by account.

use accounts::{AccountId, ItemKey};
use std::collections::BTreeMap;

pub fn normalize_name(name: &str) -> String {
    let folded = library::fold(name).replace('&', " and ");
    let words: Vec<_> = folded.split_whitespace().collect();
    words.strip_prefix(&["the"]).unwrap_or(&words).join(" ")
}

#[derive(Debug, Clone)]
pub struct ArtistIdentity {
    pub key: ItemKey,
    pub name: String,
    pub musicbrainz: Option<String>,
    /// Their photo on this source, so a merged artist's page can try each
    /// source's when one has none.
    pub photo: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MergedArtist {
    pub name: String,
    pub members: Vec<ArtistIdentity>,
    pub albums: BTreeMap<AccountId, Vec<String>>,
}

/// Normalizes an optional MusicBrainz id, treating empty or whitespace-only
/// strings (such as Navidrome's `""`) as absent.
pub fn clean_mbid(mbid: Option<&str>) -> Option<String> {
    let trimmed = mbid?.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Drops entries whose MusicBrainz id is empty or whitespace-only.
pub fn clean_mbids(rows: impl IntoIterator<Item = (String, String)>) -> Vec<(String, String)> {
    rows.into_iter().filter_map(|(id, mbid)| Some((id, clean_mbid(Some(&mbid))?))).collect()
}

pub fn merge(artists: impl IntoIterator<Item = ArtistIdentity>) -> Vec<MergedArtist> {
    let mut artists: Vec<_> = artists
        .into_iter()
        .map(|mut a| {
            a.musicbrainz = clean_mbid(a.musicbrainz.as_deref());
            a
        })
        .collect();
    // Known identities precede name-only records, giving an ambiguous name
    // a deterministic home without joining different known MusicBrainz ids.
    artists.sort_by_key(|a| (a.musicbrainz.is_none(), a.key.clone()));
    let mut groups: Vec<MergedArtist> = Vec::new();
    let mut by_mbid = BTreeMap::<String, usize>::new();
    let mut by_name = BTreeMap::<String, usize>::new();
    for artist in artists {
        let normalized = normalize_name(&artist.name);
        let found = match &artist.musicbrainz {
            Some(mbid) => by_mbid.get(&mbid.to_lowercase()).copied(),
            None => by_name.get(&normalized).copied(),
        };
        let index = found.unwrap_or_else(|| {
            let index = groups.len();
            groups.push(MergedArtist { name: artist.name.clone(), members: Vec::new(), albums: BTreeMap::new() });
            index
        });
        if let Some(mbid) = &artist.musicbrainz {
            by_mbid.insert(mbid.to_lowercase(), index);
        }
        by_name.entry(normalized).or_insert(index);
        groups[index].members.push(artist);
    }
    groups.sort_by_key(|g| normalize_name(&g.name));
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_fold_case_accents_the_and_ampersands() {
        assert_eq!(normalize_name("  The BJÖRK & Friends "), normalize_name("bjork and friends"));
        assert_eq!(normalize_name("The The"), "the");
        assert_eq!(normalize_name("Theatre"), "theatre");
    }
    #[test]
    fn musicbrainz_matches_different_names_and_keeps_distinct_known_artists() {
        let a = AccountId::new();
        let entry = |id: &str, name: &str, mb: Option<&str>| ArtistIdentity {
            key: ItemKey::new(a, id),
            name: name.into(),
            musicbrainz: mb.map(str::to_owned),
            photo: None,
        };
        let groups =
            merge([entry("1", "Björk", Some("one")), entry("2", "Other spelling", Some("one")), entry("3", "BJORK", None), entry("4", "Björk", Some("two"))]);
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().any(|g| g.members.len() == 3));
    }

    #[test]
    fn empty_or_whitespace_musicbrainz_ids_are_ignored_when_merging() {
        let a = AccountId::new();
        let b = AccountId::new();
        let entry = |acc, id: &str, name: &str, mb: Option<&str>| ArtistIdentity {
            key: ItemKey::new(acc, id),
            name: name.into(),
            musicbrainz: mb.map(str::to_owned),
            photo: None,
        };
        let groups = merge([
            entry(a, "1", "Josh Garrels", Some("mbid-jg")),
            entry(a, "2", "Stand", Some("")),
            entry(a, "3", "Paul Simon", Some("   ")),
            entry(b, "4", "Josh Garrels", Some("")),
            entry(b, "5", "Sojourn Music", Some("")),
        ]);
        assert_eq!(groups.len(), 4);
        assert!(groups.iter().any(|g| g.name == "Josh Garrels" && g.members.len() == 2));
        for single in ["Stand", "Paul Simon", "Sojourn Music"] {
            assert!(groups.iter().any(|g| g.name == single && g.members.len() == 1), "{single} should stay separate");
        }
    }
}
