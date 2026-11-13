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
}

#[derive(Debug, Clone)]
pub struct MergedArtist {
    pub name: String,
    pub members: Vec<ArtistIdentity>,
    pub albums: BTreeMap<AccountId, Vec<String>>,
}

pub fn merge(artists: impl IntoIterator<Item = ArtistIdentity>) -> Vec<MergedArtist> {
    let mut artists: Vec<_> = artists.into_iter().collect();
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
        let entry = |id: &str, name: &str, mb: Option<&str>| ArtistIdentity { key: ItemKey::new(a, id), name: name.into(), musicbrainz: mb.map(str::to_owned) };
        let groups =
            merge([entry("1", "Björk", Some("one")), entry("2", "Other spelling", Some("one")), entry("3", "BJORK", None), entry("4", "Björk", Some("two"))]);
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().any(|g| g.members.len() == 3));
    }
}
