//! Renaming a closed store without changing its state format. `.rename-ids.json`
//! is an atomic JSON journal containing the operation's name, replacement `Pins`
//! and pairs of old and new file names. It reaches disk before the state is
//! replaced and stays until all renames finish. `.renamed-ids` then records the
//! completed operation's name as UTF-8, before the journal is removed. Repeating
//! an operation never maps ids twice, even after its journal was removed.
//! Opening a journaled store is refused so cleanup cannot mistake a download
//! waiting for its rename for a stray.

use crate::{Pin, Pins, STATE_FILE};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::io::{self, Write};
use std::path::Path;

pub(crate) const JOURNAL: &str = ".rename-ids.json";
const COMPLETED: &str = ".renamed-ids";

#[derive(Serialize, Deserialize)]
struct Plan {
    operation: String,
    pins: Pins,
    files: Vec<(String, String)>,
}

/// Renames a closed store's track ids and pins once per named `operation`.
/// Mappers receive the previous operation's name (`None` for an original store),
/// so they need not guess whether an id already carries an older encoding.
/// Track mappings must be injective and nonempty. Missing downloads are fine;
/// existing destination files are never overwritten. Call again after a failure
/// to resume the saved mapping before starting the requested operation.
pub fn rename_ids(dir: &Path, operation: &str, track: impl Fn(Option<&str>, &str) -> String, pin: impl Fn(Option<&str>, &Pin) -> Pin) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let journal = dir.join(JOURNAL);
    match std::fs::read(&journal) {
        Ok(bytes) => finish(dir, &serde_json::from_slice::<Plan>(&bytes).map_err(io::Error::other)?)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let previous = match std::fs::read_to_string(dir.join(COMPLETED)) {
        Ok(previous) if previous == operation => return Ok(()),
        Ok(previous) => Some(previous),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let bytes = match std::fs::read(dir.join(STATE_FILE)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return atomic(&dir.join(COMPLETED), operation.as_bytes()),
        Err(e) => return Err(e),
    };
    let mut pins: Pins = match serde_json::from_slice(&bytes) {
        Ok(pins) => pins,
        Err(_) => {
            // Opening the store will set this aside and disable cleanup;
            // there is no trustworthy mapping for its downloads.
            return atomic(&dir.join(COMPLETED), operation.as_bytes());
        }
    };
    let ids: BTreeMap<_, _> = pins.tracks.keys().map(|id| (id.clone(), track(previous.as_deref(), id))).collect();
    let unique: HashSet<_> = ids.values().collect();
    if unique.len() != ids.len() || unique.contains(&String::new()) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "download ids must be distinct and nonempty"));
    }
    let mut stems = BTreeMap::new();
    pins.tracks = pins
        .tracks
        .into_iter()
        .map(|(id, mut stored)| {
            let new = ids[&id].clone();
            stems.insert(streamcache::escape_id(&id), streamcache::escape_id(&new));
            stored.r.id = new.clone();
            (new, stored)
        })
        .collect();
    for record in &mut pins.pins {
        record.pin = pin(previous.as_deref(), &record.pin);
        for id in &mut record.tracks {
            *id = ids.get(id).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "pin names an unknown download"))?.clone();
        }
    }
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let stem = name.split('.').next().unwrap_or_default();
        if let Some(new) = stems.get(stem).filter(|new| new.as_str() != stem) {
            files.push((name.clone(), format!("{new}{}", &name[stem.len()..])));
        }
    }
    let plan = Plan { operation: operation.to_owned(), pins, files };
    atomic(&journal, &serde_json::to_vec(&plan).map_err(io::Error::other)?)?;
    finish(dir, &plan)
}

fn finish(dir: &Path, plan: &Plan) -> io::Result<()> {
    apply(dir, plan, &mut || Ok(()))?;
    atomic(&dir.join(COMPLETED), plan.operation.as_bytes())?;
    std::fs::remove_file(dir.join(JOURNAL))?;
    sync_dir(dir);
    Ok(())
}

fn apply(dir: &Path, plan: &Plan, renamed: &mut dyn FnMut() -> io::Result<()>) -> io::Result<()> {
    atomic(&dir.join(STATE_FILE), &serde_json::to_vec_pretty(&plan.pins).map_err(io::Error::other)?)?;
    for (old, new) in &plan.files {
        let (old, new) = (dir.join(old), dir.join(new));
        if !old.exists() {
            continue;
        }
        if new.exists() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("download already exists: {}", new.display())));
        }
        std::fs::rename(old, new)?;
        sync_dir(dir);
        renamed()?;
    }
    Ok(())
}

fn atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("rename-tmp");
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(tmp, path)?;
    if let Some(dir) = path.parent() {
        sync_dir(dir);
    }
    Ok(())
}

fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
    #[cfg(not(unix))]
    let _ = dir;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TrackRef;

    fn fixture(dir: &Path) {
        let mut pins = Pins::default();
        pins.set(
            Pin::Album("album".into()),
            "Album".into(),
            vec![TrackRef::new("one", Some("flac".into()), Some(3)), TrackRef::new("two", Some("mp3".into()), Some(4))],
        );
        pins.tracks.get_mut("one").unwrap().done = Some(3);
        std::fs::write(dir.join(STATE_FILE), serde_json::to_vec(&pins).unwrap()).unwrap();
        std::fs::write(dir.join("one.flac"), b"one").unwrap();
        std::fs::write(dir.join("two.mp3.part"), b"two").unwrap();
        std::fs::write(dir.join(".keep"), b"hidden").unwrap();
    }

    #[test]
    fn renaming_preserves_downloads_and_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        rename_ids(dir.path(), "qualified", |_, id| format!("account/{id}"), |_, p| p.clone()).unwrap();
        assert_eq!(std::fs::read(dir.path().join("account%2Fone.flac")).unwrap(), b"one");
        assert_eq!(std::fs::read(dir.path().join("account%2Ftwo.mp3.part")).unwrap(), b"two");
        let pins: Pins = serde_json::from_slice(&std::fs::read(dir.path().join(STATE_FILE)).unwrap()).unwrap();
        assert_eq!(pins.tracks["account/one"].done, Some(3));
        assert_eq!(pins.pins[0].pin, Pin::Album("album".into()));
        assert_eq!(pins.pins[0].tracks, ["account/one", "account/two"]);
        assert!(dir.path().join(".keep").exists());
    }

    #[test]
    fn an_interrupted_rename_replays_the_saved_mapping() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        // A conflict leaves the journal and new state in place, before any rename.
        let conflict = dir.path().join("account%2Fone.flac");
        std::fs::write(&conflict, b"other").unwrap();
        assert!(rename_ids(dir.path(), "qualified", |_, id| format!("account/{id}"), |_, p| p.clone()).is_err());
        let plan: Plan = serde_json::from_slice(&std::fs::read(dir.path().join(JOURNAL)).unwrap()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert!(
            crate::OfflineStore::open(
                crate::OfflineConfig { dir: dir.path().to_owned(), limit: None, reply_check: Default::default() },
                rt.handle().clone(),
                std::sync::Arc::new(|| {})
            )
            .is_err()
        );
        assert!(dir.path().join("one.flac").exists());
        assert!(dir.path().join("two.mp3.part").exists() || dir.path().join("account%2Ftwo.mp3.part").exists());
        std::fs::remove_file(conflict).unwrap();
        assert!(apply(dir.path(), &plan, &mut || Err(io::Error::other("interrupted"))).is_err());
        rename_ids(dir.path(), "qualified", |_, _| panic!("the journal supplies the mapping"), |_, _| panic!("the journal supplies the pins")).unwrap();
        assert!(dir.path().join("account%2Fone.flac").exists());
        assert!(dir.path().join("account%2Ftwo.mp3.part").exists());
        assert!(!dir.path().join(JOURNAL).exists());
    }

    #[test]
    fn a_colliding_mapping_leaves_the_store_untouched() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        let before = std::fs::read(dir.path().join(STATE_FILE)).unwrap();
        assert!(rename_ids(dir.path(), "colliding", |_, _| "same".into(), |_, p| p.clone()).is_err());
        assert_eq!(std::fs::read(dir.path().join(STATE_FILE)).unwrap(), before);
        assert!(dir.path().join("one.flac").exists());
    }
    #[test]
    fn repeating_a_completed_operation_never_maps_ids_again() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        rename_ids(dir.path(), "qualified", |_, id| format!("account/{id}"), |_, p| p.clone()).unwrap();
        let state = std::fs::read(dir.path().join(STATE_FILE)).unwrap();
        rename_ids(dir.path(), "qualified", |_, _| panic!("already done"), |_, _| panic!("already done")).unwrap();
        assert_eq!(std::fs::read(dir.path().join(STATE_FILE)).unwrap(), state);
    }

    #[test]
    fn an_empty_store_records_the_encoding_for_downloads_added_later() {
        let dir = tempfile::tempdir().unwrap();
        rename_ids(dir.path(), "qualified", |_, _| panic!("no tracks"), |_, _| panic!("no pins")).unwrap();
        fixture(dir.path());
        rename_ids(dir.path(), "qualified", |_, _| panic!("already done"), |_, _| panic!("already done")).unwrap();
        assert!(dir.path().join("one.flac").exists());
    }
}
