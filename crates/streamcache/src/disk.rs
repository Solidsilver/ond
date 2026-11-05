//! File work for async download tasks, run on tokio's blocking pool. The app's
//! few async workers are shared with the library sync, artwork and server
//! calls, so a slow or sleeping disk must not hold one up.

use bytes::Bytes;
use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// Runs `f` on tokio's blocking pool. Works on either runtime flavour (unlike
/// `block_in_place`).
pub async fn blocking<T: Send + 'static>(f: impl FnOnce() -> io::Result<T> + Send + 'static) -> io::Result<T> {
    tokio::task::spawn_blocking(f).await.unwrap_or_else(|e| Err(io::Error::other(e)))
}

/// Counts blocking calls that haven't returned yet, so an owner shutting down
/// can wait for them.
///
/// Dropping a task doesn't stop a blocking call it started. That call can
/// still write to a file after its owner is gone, and after a new owner of the
/// same folder has read that file's length. Once [`close`](Self::close)d, no
/// new call starts, so after [`wait_idle`](Self::wait_idle) returns true
/// nothing more touches the disk.
#[derive(Debug, Default)]
pub struct InFlight {
    state: Mutex<Calls>,
    idle: Condvar,
}

#[derive(Debug, Default)]
struct Calls {
    running: usize,
    closed: bool,
}

impl InFlight {
    fn lock(&self) -> MutexGuard<'_, Calls> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Refuses new calls from now on. They fail without running.
    pub fn close(&self) {
        self.lock().closed = true;
    }

    /// Waits until no call is running, or `timeout` passes. Returns whether
    /// they all finished.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let (calls, _) = self.idle.wait_timeout_while(self.lock(), timeout, |c| c.running > 0).unwrap_or_else(|e| e.into_inner());
        calls.running == 0
    }

    /// Calls running now. Only the owners' tests look at this.
    pub fn running(&self) -> usize {
        self.lock().running
    }

    fn enter(self: &Arc<Self>) -> io::Result<Ticket> {
        let mut calls = self.lock();
        if calls.closed {
            return Err(io::Error::other("shutting down"));
        }
        calls.running += 1;
        Ok(Ticket(self.clone()))
    }
}

/// One counted call. The closure owns it, so it's released when the call
/// returns, or when the closure is dropped without running.
struct Ticket(Arc<InFlight>);

impl Drop for Ticket {
    fn drop(&mut self) {
        let mut calls = self.0.lock();
        calls.running -= 1;
        if calls.running == 0 {
            self.0.idle.notify_all();
        }
    }
}

/// [`blocking`], counted in `inflight` from now until `f` returns. Fails
/// without running `f` once `inflight` is closed.
pub async fn blocking_tracked<T: Send + 'static>(inflight: &Arc<InFlight>, f: impl FnOnce() -> io::Result<T> + Send + 'static) -> io::Result<T> {
    let ticket = inflight.enter()?;
    blocking(move || {
        let _ticket = ticket;
        f()
    })
    .await
}

/// A file an async task writes, each call made on the blocking pool. A write
/// has reached the file (the OS's cache of it) when its call returns, so a
/// reader with its own handle can read those bytes from then on.
///
/// Copies share the file. One cut short (its task dropped mid-call) still
/// finishes the call in the background. Opened with
/// [`open_tracked`](Self::open_tracked), every call counts in an
/// [`InFlight`], so its owner can wait for the stragglers.
#[derive(Clone)]
pub struct PartFile {
    file: Arc<File>,
    inflight: Option<Arc<InFlight>>,
}

impl PartFile {
    pub async fn open(options: OpenOptions, path: PathBuf) -> io::Result<Self> {
        blocking(move || options.open(path)).await.map(|f| Self { file: Arc::new(f), inflight: None })
    }

    /// [`open`](Self::open), with this call and every later one counted in `inflight`.
    pub async fn open_tracked(options: OpenOptions, path: PathBuf, inflight: Arc<InFlight>) -> io::Result<Self> {
        let file = blocking_tracked(&inflight, move || options.open(path)).await?;
        Ok(Self { file: Arc::new(file), inflight: Some(inflight) })
    }

    async fn call<T: Send + 'static>(&self, f: impl FnOnce(&File) -> io::Result<T> + Send + 'static) -> io::Result<T> {
        let file = self.file.clone();
        match &self.inflight {
            Some(inflight) => blocking_tracked(inflight, move || f(&file)).await,
            None => blocking(move || f(&file)).await,
        }
    }

    pub async fn set_len(&self, len: u64) -> io::Result<()> {
        self.call(move |f| f.set_len(len)).await
    }

    /// Writes `bytes` at offset `pos`.
    pub async fn write_at(&self, pos: u64, bytes: Bytes) -> io::Result<()> {
        self.call(move |mut f| {
            f.seek(SeekFrom::Start(pos))?;
            f.write_all(&bytes)
        })
        .await
    }

    /// Writes `bytes` where the last write ended (or at the end, opened to append).
    pub async fn write(&self, bytes: Bytes) -> io::Result<()> {
        self.call(move |mut f| f.write_all(&bytes)).await
    }

    pub async fn sync_all(&self) -> io::Result<()> {
        self.call(File::sync_all).await
    }
}
