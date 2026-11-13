//! Per-account mirrors and the startup migration described in docs/sources.md.
//! `sources-migration.json` in the data root is atomic JSON, version 1, with
//! `step` (`account`, `files`, `offline`, `done`) naming the last completed step
//! and `account` holding the UUID chosen before any files move. An absent file
//! means nothing has run. Repeating a step is safe, including after its work
//! finished but before its progress write. No store opens until `done` is saved.
//! A pending offline folder move finishes first, using the existing resumable
//! mover, so the state and files reunite before their names are rewritten.

use crate::prefs::{Prefs, replace_file};
use crate::settings::Paths;
use accounts::{AccountId, ItemKey};
use library::{AnalysisStore, Library};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Step {
    Account,
    Files,
    Offline,
    Done,
}

#[derive(Serialize, Deserialize)]
struct Progress {
    version: u32,
    step: Step,
    account: Option<AccountId>,
}

pub fn account_dir(data: &Path, account: AccountId) -> PathBuf {
    data.join("Sources").join(account.to_string())
}

/// The operation stamp distinguishes an old bare id from a qualified key,
/// including a bare id that happens to look exactly like an ItemKey.
pub fn qualify_downloads(dir: &Path, account: AccountId) -> std::io::Result<()> {
    const PREFIX: &str = "ond-item-keys-v1/";
    let qualify = |previous: Option<&str>, id: &str| {
        let item =
            if previous.is_some_and(|p| p.starts_with(PREFIX)) { ItemKey::parse(id).map(|k| k.item).unwrap_or_else(|_| id.to_owned()) } else { id.to_owned() };
        ItemKey::new(account, item).to_string()
    };
    offline::rename_ids(dir, &format!("{PREFIX}{account}"), qualify, |previous, pin| match pin {
        offline::Pin::Track(id) => offline::Pin::Track(qualify(previous, id)),
        p => p.clone(),
    })
}

/// Runs before opening either the mirror or downloads. The hook is also used
/// by recovery tests to interrupt the real startup path at its durable steps.
pub fn migrate(paths: &Paths, prefs: &Prefs) -> anyhow::Result<()> {
    migrate_with(paths, prefs, |_| Ok(()))
}

fn migrate_with(paths: &Paths, prefs: &Prefs, mut completed: impl FnMut(Step) -> anyhow::Result<()>) -> anyhow::Result<()> {
    let data = paths.library_db.parent().expect("the library has a data folder");
    std::fs::create_dir_all(data)?;
    let path = data.join("sources-migration.json");
    let mut progress = match std::fs::read(&path) {
        Ok(bytes) => {
            let p: Progress = serde_json::from_slice(&bytes)?;
            anyhow::ensure!(p.version == 1, "unknown sources migration version");
            if p.step == Step::Done {
                return Ok(());
            }
            Some(p)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let record = |p: &Progress| -> anyhow::Result<()> {
        replace_file(&path, &path.with_extension("json.tmp"), &serde_json::to_vec(p)?)?;
        Ok(())
    };
    if progress.is_none() {
        if prefs.get(|s| s.account_id().is_none()) {
            record(&Progress { version: 1, step: Step::Done, account: None })?;
            return Ok(());
        }
        prefs.edit(|s| {
            if s.account().is_none()
                && let Some(entry) = s.subsonic_mut()
            {
                entry.id = AccountId::new();
            }
        });
        prefs.flush_checked()?;
        let p = Progress { version: 1, step: Step::Account, account: prefs.get(|s| s.account()) };
        record(&p)?;
        completed(Step::Account)?;
        progress = Some(p);
    }
    let mut progress = progress.expect("the account step was recorded");
    let account = progress.account.ok_or_else(|| anyhow::anyhow!("migration has no account"))?;
    anyhow::ensure!(prefs.get(|s| s.account()) == Some(account), "migration account differs from settings");
    if progress.step == Step::Account {
        let dir = account_dir(data, account);
        std::fs::create_dir_all(&dir)?;
        for name in ["library.redb", "analysis.redb", "scrobbles.json"] {
            let (from, to) = (data.join(name), dir.join(name));
            if !from.exists() {
                continue;
            }
            if to.exists() {
                tracing::warn!("keeping {}, as {} already exists", from.display(), to.display());
            } else {
                std::fs::rename(from, to)?;
            }
        }
        #[cfg(unix)]
        let _ = std::fs::File::open(&dir).and_then(|d| d.sync_all());
        progress.step = Step::Files;
        record(&progress)?;
        completed(Step::Files)?;
    }
    if progress.step == Step::Files {
        let saved = prefs.snapshot();
        let base = saved.offline_dir.clone().unwrap_or_else(|| paths.offline.clone());
        if let Some(from) = &saved.offline_move_from {
            offline::move_store(from, &base)?;
        }
        let dir = offline::account_store(&base, saved.account_id().expect("configured server"))?;
        qualify_downloads(&dir, account)?;
        if saved.offline_move_from.is_some() {
            prefs.edit(|s| s.offline_move_from = None);
            prefs.flush_checked()?;
        }
        progress.step = Step::Offline;
        record(&progress)?;
        completed(Step::Offline)?;
    }
    progress.step = Step::Done;
    record(&progress)?;
    completed(Step::Done)
}

pub struct Stores {
    pub library: Arc<Library>,
    pub analyses: Arc<AnalysisStore>,
}

impl Stores {
    /// Nothing on disk: before a first sign-in, and for a session whose
    /// migration didn't finish, where creating the account's files would
    /// strand the old ones the next attempt still has to move.
    pub fn in_memory() -> anyhow::Result<Self> {
        Ok(Self { library: Arc::new(Library::in_memory()?), analyses: Arc::new(AnalysisStore::in_memory()?) })
    }
}

/// Opens only the selected account. Before a first sign-in nothing persistent
/// belongs to a source, so neither database is created on disk. The library
/// is only a mirror of the server, so an unreadable one is set aside and
/// rebuilt by the next sync; analysis results can't be rebuilt that way, so a
/// set-aside file is worth telling the user about.
pub fn open(data: &Path, account: Option<AccountId>, notices: &mut Vec<String>) -> anyhow::Result<Stores> {
    let Some(account) = account else {
        return Stores::in_memory();
    };
    let dir = account_dir(data, account);
    std::fs::create_dir_all(&dir)?;
    let (library, aside) = Library::open_or_recover(dir.join("library.redb"))?;
    if let Some(aside) = aside {
        tracing::warn!("library database was unreadable; moved to {} and starting fresh", aside.display());
        notices.push(format!("Your library database couldn't be read, so Ond is rebuilding it. The old copy is at {}.", aside.display()));
    }
    let (analyses, aside) = AnalysisStore::open_or_recover(dir.join("analysis.redb"))?;
    if let Some(aside) = aside {
        notices.push(format!("Your analysis results couldn't be read, so Ond started a new set. The old copy is at {}.", aside.display()));
    }
    match analyses.adopt(&library) {
        Ok(0) => {}
        Ok(n) => tracing::info!("moved {n} analysis results out of the library database"),
        Err(e) => tracing::warn!("moving analysis results: {e}"),
    }
    Ok(Stores { library: Arc::new(library), analyses: Arc::new(analyses) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::pins::{OfflineTracks, offline_name};
    use crate::favorites::{SavedStarred, Starred};
    use crate::now_playing::scrobble::RetryQueue;
    use crate::source::TrackKey;
    use offline::{OfflineConfig, OfflineStore, Pin, Pins, TrackRef};

    fn paths(root: &Path) -> Paths {
        Paths {
            config_file: root.join("settings.json"),
            library_db: root.join("library.redb"),
            offline: root.join("Offline"),
            stream_cache: root.join("streams"),
            art_cache: root.join("art"),
        }
    }

    fn old_install(root: &Path) -> Paths {
        let paths = paths(root);
        std::fs::write(&paths.config_file, br#"{"server":"https://music.example.com/","username":"me","signed_out":true,"work_offline":true}"#).unwrap();
        let library = Library::open(&paths.library_db).unwrap();
        library.upsert_albums(vec![crate::sync::tests::album("a", 2), crate::sync::tests::album("b", 1)]).unwrap();
        let songs: Vec<_> = ["one", "two"].into_iter().map(|id| crate::test_tracks::track(id, "a", id)).collect();
        library.upsert_album_tracks(library::AlbumTracks { album: crate::sync::tests::album("a", 2), tracks: songs.clone() }).unwrap();
        library
            .save(
                "starred",
                &SavedStarred { starred: Starred { songs: songs.clone(), albums: vec!["a".into()], artists: vec![] }, ratings: vec![("one".into(), 5)] },
            )
            .unwrap();
        library.save("playlist/p", &songs).unwrap();
        let analysis = analysis::Record {
            stamp: "stamp".into(),
            analysis: analysis::TrackAnalysis {
                version: analysis::ANALYSIS_VERSION,
                duration_secs: 120.0,
                sample_rate: 44100,
                channels: 2,
                bits: Some(16),
                integrated_lufs: Some(-14.0),
                loudness_range_lu: None,
                true_peak_dbtp: None,
                sample_peak_dbfs: None,
                dr: None,
                effective_bits: None,
                cutoff_hz: None,
                verdicts: vec![],
            },
        };
        AnalysisStore::open(root.join("analysis.redb")).unwrap().put("one", &analysis.encode()).unwrap();
        std::fs::write(root.join("scrobbles.json"), br#"[{"id":"one","time_ms":12345}]"#).unwrap();
        let mut pins = Pins::default();
        pins.set(Pin::Album("a".into()), "Album".into(), ["one", "two"].map(|id| TrackRef::new(id, Some("flac".into()), Some(3))).to_vec());
        pins.set(Pin::Track("one".into()), "One".into(), vec![TrackRef::new("one", Some("flac".into()), Some(3))]);
        for stored in pins.tracks.values_mut() {
            stored.done = Some(3);
        }
        std::fs::create_dir_all(&paths.offline).unwrap();
        std::fs::write(paths.offline.join("offline.json"), serde_json::to_vec(&pins).unwrap()).unwrap();
        for id in ["one", "two"] {
            std::fs::write(paths.offline.join(format!("{id}.flac")), id.as_bytes()).unwrap();
        }
        paths
    }

    fn verify(paths: &Paths, prefs: &Prefs) {
        migrate(paths, prefs).unwrap();
        let account = prefs.get(|s| s.account()).unwrap();
        assert_eq!(crate::settings::Settings::read(&paths.config_file).unwrap().account(), Some(account));
        let root = paths.library_db.parent().unwrap();
        let stores = open(root, Some(account), &mut vec![]).unwrap();
        assert_eq!(stores.library.albums().unwrap().len(), 2);
        assert_eq!(stores.library.album_tracks("a").unwrap().len(), 2);
        let starred = stores.library.saved::<SavedStarred>("starred").unwrap().unwrap();
        assert_eq!(starred.starred.songs.len(), 2);
        assert_eq!(starred.ratings, [("one".into(), 5)]);
        assert_eq!(stores.library.saved::<Vec<library::Track>>("playlist/p").unwrap().unwrap().len(), 2);
        let rows = stores.analyses.records().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(analysis::Record::decode(&rows[0].1).unwrap().stamp, "stamp");
        let queue = RetryQueue::load(&account_dir(root, account).join("scrobbles.json"), Some(account));
        assert_eq!(queue.front().unwrap().key, TrackKey::new(account, "one"));
        assert_eq!(queue.front().unwrap().time_ms, 12345);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let base = prefs.get(|s| s.offline_dir.clone()).unwrap_or_else(|| paths.offline.clone());
        let dir = offline::account_store(&base, prefs.snapshot().account_id().unwrap()).unwrap();
        let store =
            OfflineStore::open(OfflineConfig { dir: dir.clone(), limit: None, reply_check: Default::default() }, rt.handle().clone(), Arc::new(|| {})).unwrap();
        assert!(store.is_pinned(&Pin::Album("a".into())));
        assert!(store.is_pinned(&Pin::Track(TrackKey::new(account, "one").to_string())));
        let snap = store.snapshot();
        let adapter = OfflineTracks { account: Some(account), snap: &snap, store: Some(&store) };
        for id in ["one", "two"] {
            let key = TrackKey::new(account, id);
            assert!(adapter.is_done(&key));
            assert_eq!(std::fs::read(adapter.local_path(&key).unwrap()).unwrap(), id.as_bytes());
            assert_eq!(store.local_path(&offline_name(&key)), adapter.local_path(&key));
            assert!(!dir.join(format!("{id}.flac")).exists());
        }
        for file in ["library.redb", "analysis.redb", "scrobbles.json"] {
            assert!(!root.join(file).exists());
        }
        assert_eq!(serde_json::from_slice::<Progress>(&std::fs::read(root.join("sources-migration.json")).unwrap()).unwrap().step as u8, Step::Done as u8);
    }

    #[test]
    fn an_old_install_keeps_its_library_saved_lists_analyses_downloads_and_scrobbles() {
        let dir = tempfile::tempdir().unwrap();
        let paths = old_install(dir.path());
        let (prefs, _) = Prefs::open(&paths.config_file);
        verify(&paths, &prefs);
    }

    #[test]
    fn every_recorded_step_can_resume_on_the_next_start() {
        for stop in [Step::Account, Step::Files, Step::Offline, Step::Done] {
            let dir = tempfile::tempdir().unwrap();
            let paths = old_install(dir.path());
            let (prefs, _) = Prefs::open(&paths.config_file);
            assert!(
                migrate_with(&paths, &prefs, |step| {
                    anyhow::ensure!(step != stop, "interrupted");
                    Ok(())
                })
                .is_err()
            );
            drop(prefs);
            let (prefs, _) = Prefs::open(&paths.config_file);
            verify(&paths, &prefs);
        }
    }

    #[test]
    fn a_stop_between_offline_file_renames_keeps_every_download() {
        let dir = tempfile::tempdir().unwrap();
        let paths = old_install(dir.path());
        let (prefs, _) = Prefs::open(&paths.config_file);
        assert!(
            migrate_with(&paths, &prefs, |step| {
                anyhow::ensure!(step != Step::Files, "interrupted");
                Ok(())
            })
            .is_err()
        );
        let account = prefs.get(|s| s.account()).unwrap();
        let store_dir = offline::account_store(&paths.offline, prefs.snapshot().account_id().unwrap()).unwrap();
        let new = |id: &str| store_dir.join(streamcache::file_name(&TrackKey::new(account, id).to_string(), Some("flac")));
        // A destination conflict stops the real operation after its state write.
        std::fs::write(new("one"), b"conflict").unwrap();
        assert!(qualify_downloads(&store_dir, account).is_err());
        std::fs::remove_file(new("one")).unwrap();
        std::fs::rename(store_dir.join("one.flac"), new("one")).unwrap();
        drop(prefs);
        let (prefs, _) = Prefs::open(&paths.config_file);
        verify(&paths, &prefs);
    }

    #[test]
    fn an_interrupted_custom_folder_move_finishes_before_qualifying_downloads() {
        let dir = tempfile::tempdir().unwrap();
        let paths = old_install(dir.path());
        let destination = dir.path().join("custom");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::rename(paths.offline.join("offline.json"), destination.join("offline.json")).unwrap();
        std::fs::rename(paths.offline.join("one.flac"), destination.join("one.flac")).unwrap();
        let (prefs, _) = Prefs::open(&paths.config_file);
        prefs.edit(|s| {
            s.offline_dir = Some(destination.clone());
            s.offline_move_from = Some(paths.offline.clone());
        });
        verify(&paths, &prefs);
        assert!(prefs.get(|s| s.offline_move_from.is_none()));
        assert!(!paths.offline.join("two.flac").exists());
    }

    #[test]
    fn existing_targets_are_kept_and_empty_installs_open_only_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(dir.path());
        let (prefs, _) = Prefs::open(&paths.config_file);
        migrate(&paths, &prefs).unwrap();
        let _stores = open(dir.path(), None, &mut vec![]).unwrap();
        assert!(!dir.path().join("Sources").exists());
        assert!(!paths.library_db.exists());
        let other = tempfile::tempdir().unwrap();
        let paths = old_install(other.path());
        let (prefs, _) = Prefs::open(&paths.config_file);
        let account = prefs.get(|s| s.account()).unwrap();
        let folder = account_dir(other.path(), account);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("scrobbles.json"), b"[]").unwrap();
        migrate(&paths, &prefs).unwrap();
        assert_eq!(std::fs::read(folder.join("scrobbles.json")).unwrap(), b"[]");
        assert!(other.path().join("scrobbles.json").exists());
    }
    #[test]
    fn a_bare_id_that_looks_like_a_key_survives_migration_and_an_account_switch() {
        let dir = tempfile::tempdir().unwrap();
        let bare = ItemKey::new(AccountId::new(), "nested/id").to_string();
        let mut pins = Pins::default();
        pins.set(Pin::Track(bare.clone()), "Song".into(), vec![TrackRef::new(&bare, Some("flac".into()), Some(3))]);
        pins.tracks.get_mut(&bare).unwrap().done = Some(3);
        std::fs::write(dir.path().join("offline.json"), serde_json::to_vec(&pins).unwrap()).unwrap();
        let filename = |id: &str| dir.path().join(streamcache::file_name(id, Some("flac")));
        std::fs::write(filename(&bare), b"one").unwrap();
        let account = AccountId::new();
        qualify_downloads(dir.path(), account).unwrap();
        qualify_downloads(dir.path(), account).unwrap();
        assert_eq!(std::fs::read(filename(&ItemKey::new(account, &bare).to_string())).unwrap(), b"one");
        let next = AccountId::new();
        qualify_downloads(dir.path(), next).unwrap();
        assert_eq!(std::fs::read(filename(&ItemKey::new(next, &bare).to_string())).unwrap(), b"one");
        let pins: Pins = serde_json::from_slice(&std::fs::read(dir.path().join("offline.json")).unwrap()).unwrap();
        assert_eq!(pins.pins[0].pin, Pin::Track(ItemKey::new(next, bare).to_string()));
    }
}
