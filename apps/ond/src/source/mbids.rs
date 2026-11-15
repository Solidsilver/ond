//! Copy identity is saved beside frozen rows. Observed missing ids are kept
//! as `None` until saving, so clearing a server tag removes an old saved id.
//! Each blob's name versions its BTreeMap<String, String> wire format.

use super::artists::{clean_mbid, clean_mbids};
use std::collections::BTreeMap;

pub const ALBUMS: &str = "album_mbids_v1";
pub const RECORDINGS: &str = "recording_mbids_v1";
type Rows = Vec<(String, String)>;

#[derive(Clone, Default)]
pub struct CopyMbids {
    pub albums: BTreeMap<String, Option<String>>,
    pub recordings: BTreeMap<String, Option<String>>,
}

impl CopyMbids {
    pub fn save(self, library: &library::Library) -> library::Result<(Rows, Rows)> {
        fn save(library: &library::Library, name: &str, seen: BTreeMap<String, Option<String>>) -> library::Result<Rows> {
            let mut rows: BTreeMap<_, _> = clean_mbids(library.saved::<BTreeMap<String, String>>(name)?.unwrap_or_default()).into_iter().collect();
            for (key, id) in seen {
                if let Some(id) = clean_mbid(id.as_deref()) {
                    rows.insert(key, id);
                } else {
                    rows.remove(&key);
                }
            }
            library.save(name, &rows)?;
            Ok(rows.into_iter().collect())
        }
        Ok((save(library, ALBUMS, self.albums)?, save(library, RECORDINGS, self.recordings)?))
    }
}
