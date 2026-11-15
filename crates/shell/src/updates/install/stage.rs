//! A stage owns a lock and deletes itself on every error or cancellation.
//! Only a complete, signature-verified archive gets its final name.

use super::Error;
use minisign_verify::{PublicKey, Signature};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

pub(super) struct Stage {
    // Close the lock before TempDir removes it (Windows requires this order).
    _lock: File,
    dir: tempfile::TempDir,
    pub archive: PathBuf,
}

impl Stage {
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Record both ends so startup can reclaim a copy left beside the app
    /// by a crash, without treating unrelated directories as ours.
    pub fn neighbor(&self, parent: &Path) -> Result<tempfile::TempDir, Error> {
        let dir = tempfile::Builder::new().prefix(".update-").tempdir_in(parent)?;
        let origin = serde_json::to_vec(self.path()).map_err(std::io::Error::other)?;
        let destination = serde_json::to_vec(dir.path()).map_err(std::io::Error::other)?;
        fs::write(dir.path().join("stage-origin.json"), origin)?;
        fs::write(self.path().join("neighbor.json"), destination)?;
        Ok(dir)
    }

    #[cfg(any(unix, test))]
    pub fn keep(self) -> PathBuf {
        self.dir.keep()
    }
}

/// Only directories made here are considered. A live download or prepared
/// update holds its lock, so another running copy cannot sweep it away.
pub fn clean_stale(root: &Path) -> std::io::Result<()> {
    if !root.exists() {
        return Ok(());
    }
    let _guard = root_lock(root)?;
    sweep(root)
}

fn root_lock(root: &Path) -> std::io::Result<File> {
    let file = File::options().read(true).write(true).create(true).truncate(false).open(root.join("lock"))?;
    file.lock()?;
    Ok(file)
}

fn sweep(root: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let kind = match entry.file_type() {
            Ok(kind) => kind,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if !kind.is_dir() || !entry.file_name().to_string_lossy().starts_with("stage-") {
            continue;
        }
        if entry.path().join("recovery").is_file() {
            // A failed rollback leaves the previous app for manual recovery.
            continue;
        }
        // A launched helper owns this stage until it finishes. An abandoned
        // helper marker expires after a day, including a machine restart.
        if entry
            .path()
            .join("installing")
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age < Duration::from_secs(86400))
        {
            continue;
        }
        let lock = match File::options().read(true).write(true).create(true).truncate(false).open(entry.path().join("lock")) {
            Ok(lock) => lock,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if lock.try_lock().is_ok() {
            drop(lock);
            clean_neighbor(&entry.path())?;
            // A dropped Stage or helper can finish cleanup during this sweep.
            if let Err(e) = fs::remove_dir_all(entry.path())
                && e.kind() != std::io::ErrorKind::NotFound
            {
                return Err(e);
            }
        }
    }
    Ok(())
}

fn clean_neighbor(stage: &Path) -> std::io::Result<()> {
    let metadata = stage.join("neighbor.json");
    let bytes = match fs::read(metadata) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let neighbor: PathBuf = serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
    if !neighbor.is_absolute() || !neighbor.file_name().is_some_and(|n| n.to_string_lossy().starts_with(".update-")) {
        return Err(std::io::Error::other("unrecognized update replacement directory"));
    }
    let kind = match fs::symlink_metadata(&neighbor) {
        Ok(metadata) => metadata.file_type(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let origin: PathBuf = serde_json::from_slice(&fs::read(neighbor.join("stage-origin.json"))?).map_err(std::io::Error::other)?;
    if !kind.is_dir() || origin != stage {
        return Err(std::io::Error::other("unrecognized update replacement owner"));
    }
    fs::remove_dir_all(neighbor)
}

/// Streaming verification bounds memory even for a universal macOS archive.
/// Modern minisign produces prehashed signatures; legacy unhashed ones are
/// rejected rather than loading an unbounded file into memory.
pub fn verify(file: &Path, signature: &str, key: &str, expected_name: &str) -> Result<(), Error> {
    let key = PublicKey::from_base64(key).map_err(|_| Error::Signature)?;
    let signature = Signature::decode(signature).map_err(|_| Error::Signature)?;
    let mut verifier = key.verify_stream(&signature).map_err(|_| Error::Signature)?;
    let mut file = File::open(file)?;
    let mut buffer = [0; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        verifier.update(&buffer[..n]);
    }
    verifier.finalize().map_err(|_| Error::Signature)?;
    // Read the filename only after finalize authenticates the trusted comment.
    // Reject ambiguity as well as a signed archive replayed under a new name.
    let mut names = signature.trusted_comment().split('\t').filter_map(|field| field.strip_prefix("file:"));
    if names.next() != Some(expected_name) || names.next().is_some() {
        return Err(Error::Signature);
    }
    Ok(())
}

pub(super) async fn fetch(
    client: &reqwest::Client,
    root: &Path,
    name: &str,
    url: &str,
    signature_url: &str,
    key: &str,
    progress: impl Fn(u64, Option<u64>),
) -> Result<Stage, Error> {
    fs::create_dir_all(root)?;
    // Serialize creating a stage with sweeping it, across app instances.
    // There must be no window in which a new stage has no lock yet.
    let guard = root_lock(root)?;
    sweep(root)?;
    let dir = tempfile::Builder::new().prefix("stage-").tempdir_in(root)?;
    let lock = File::options().read(true).write(true).create_new(true).open(dir.path().join("lock"))?;
    lock.lock()?;
    let stage = Stage { archive: dir.path().join(name), _lock: lock, dir };
    drop(guard);
    let partial = stage.path().join("download.part");
    download(client, url, &partial, 2 * 1024 * 1024 * 1024, progress).await?;
    let signature_path = stage.path().join("signature.part");
    download(client, signature_url, &signature_path, 16 * 1024, |_, _| {}).await?;
    let signature = tokio::fs::read_to_string(&signature_path).await?;
    let key = key.to_owned();
    let expected_name = name.to_owned();
    tokio::task::spawn_blocking(move || {
        verify(&partial, &signature, &key, &expected_name)?;
        fs::rename(partial, &stage.archive)?;
        fs::rename(signature_path, stage.path().join("archive.minisig"))?;
        Ok(stage)
    })
    .await
    .map_err(|e| Error::Worker(e.to_string()))?
}

async fn download(client: &reqwest::Client, url: &str, path: &Path, limit: u64, progress: impl Fn(u64, Option<u64>)) -> Result<(), Error> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    let total = response.content_length();
    if total.is_some_and(|n| n > limit) {
        return Err(Error::Archive);
    }
    let mut file = tokio::fs::File::create(path).await?;
    let mut received = 0;
    let mut last = Instant::now();
    progress(0, total);
    while let Some(chunk) = response.chunk().await? {
        received += chunk.len() as u64;
        if received > limit {
            return Err(Error::Archive);
        }
        file.write_all(&chunk).await?;
        // Events drive repaints, at most twice a second, and stop with I/O.
        if last.elapsed() >= Duration::from_millis(500) {
            progress(received, total);
            last = Instant::now();
        }
    }
    if total.is_some_and(|n| n != received) {
        return Err(Error::Archive);
    }
    file.sync_all().await?;
    progress(received, total);
    Ok(())
}
