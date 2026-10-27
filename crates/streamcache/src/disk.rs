//! File work for async download tasks, run on tokio's blocking pool. The app's
//! few async workers are shared with the library sync, artwork and server
//! calls, so a slow or sleeping disk must not hold one up.

use bytes::Bytes;
use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;

/// Runs `f` on tokio's blocking pool. Works on either runtime flavour (unlike
/// `block_in_place`).
pub async fn blocking<T: Send + 'static>(f: impl FnOnce() -> io::Result<T> + Send + 'static) -> io::Result<T> {
    tokio::task::spawn_blocking(f).await.unwrap_or_else(|e| Err(io::Error::other(e)))
}

/// A file an async task writes, each call made on the blocking pool. A write
/// has reached the file (the OS's cache of it) when its call returns, so a
/// reader with its own handle can read those bytes from then on.
///
/// Copies share the file. One cut short (its task dropped mid-call) still
/// finishes the call in the background.
#[derive(Clone)]
pub struct PartFile(Arc<File>);

impl PartFile {
    pub async fn open(options: OpenOptions, path: PathBuf) -> io::Result<Self> {
        blocking(move || options.open(path)).await.map(|f| Self(Arc::new(f)))
    }

    pub async fn set_len(&self, len: u64) -> io::Result<()> {
        let f = self.0.clone();
        blocking(move || f.set_len(len)).await
    }

    /// Writes `bytes` at offset `pos`.
    pub async fn write_at(&self, pos: u64, bytes: Bytes) -> io::Result<()> {
        let f = self.0.clone();
        blocking(move || {
            let mut f = &*f;
            f.seek(SeekFrom::Start(pos))?;
            f.write_all(&bytes)
        })
        .await
    }

    /// Writes `bytes` where the last write ended (or at the end, opened to append).
    pub async fn write(&self, bytes: Bytes) -> io::Result<()> {
        let f = self.0.clone();
        blocking(move || (&*f).write_all(&bytes)).await
    }

    pub async fn sync_all(&self) -> io::Result<()> {
        let f = self.0.clone();
        blocking(move || f.sync_all()).await
    }
}
