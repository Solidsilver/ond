//! One file being fetched: shared state plus the async task that fills it.

use crate::fetch::{self, FetchError, redact};
use crate::ranges::RangeSet;
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::Notify;

/// A reader asking for bytes this close ahead of the download cursor waits for
/// them instead of starting a new request.
const LOOKAHEAD: u64 = 512 * 1024;
/// Consecutive failed requests before the download gives up.
const MAX_FAILURES: u32 = 4;

#[derive(Debug, Default)]
pub(crate) struct DlState {
    /// Total size, once the server has told us (or the stream ended).
    pub len: Option<u64>,
    /// Set once the first response arrived, so `len` is as known as it will get.
    pub headers: bool,
    pub ranges: RangeSet,
    /// Offset a blocked reader is waiting on.
    pub wanted: Option<u64>,
    /// Where the active request is writing.
    pub cursor: u64,
    pub error: Option<String>,
    pub complete: bool,
}

pub(crate) struct Download {
    pub name: String,
    pub part_path: PathBuf,
    pub final_path: PathBuf,
    state: Mutex<DlState>,
    /// Readers wait on this for new bytes, errors and completion.
    pub cond: Condvar,
    /// Wakes the task for a relocation request or cancellation.
    pub wake: Notify,
    cancelled: AtomicBool,
    /// Readers plus the prefetch slot. Only changed under the cache index lock.
    pub holders: AtomicUsize,
}

impl Download {
    pub fn new(name: String, part_path: PathBuf, final_path: PathBuf) -> Self {
        Self {
            name,
            part_path,
            final_path,
            state: Mutex::default(),
            cond: Condvar::new(),
            wake: Notify::new(),
            cancelled: AtomicBool::new(false),
            holders: AtomicUsize::new(0),
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, DlState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.wake.notify_one();
        self.cond.notify_all();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Ends the download for readers: `Ok` marks it complete, `Err` records the failure.
    pub fn finish(&self, result: Result<(), String>) {
        let mut st = self.lock();
        match result {
            Ok(()) => st.complete = true,
            Err(e) => st.error = Some(e),
        }
        drop(st);
        self.cond.notify_all();
    }
}

/// Fetches `url` into `dl.part_path` until every byte is present. Returns the length.
///
/// One request streams at a time. It starts at offset 0; a reader blocked on an
/// offset outside [cursor, cursor + LOOKAHEAD] makes it drop that response and
/// request `bytes=<offset>-` instead. When a response ends, or runs into bytes
/// already on disk, the next request starts at the first gap after the cursor,
/// wrapping to the start. A server that answers `200` instead of `206` is read
/// once, sequentially, with no relocation.
///
/// The length is the server's, until it turns out wrong or missing: a `416`
/// at offset `n` cuts it to `n`, and a response with no length that runs to
/// its end sets it there. Either way the file is only done once every byte
/// below the length is on disk.
pub(crate) async fn run(dl: &Download, client: &reqwest::Client, url: &str) -> Result<u64, String> {
    let io = |e: std::io::Error| format!("cache file: {e}");
    let mut file = OpenOptions::new().create(true).write(true).truncate(true).open(&dl.part_path).map_err(io)?;
    let mut ranged: Option<bool> = None;
    let mut sized = false;
    let mut start = 0u64;
    let mut failures = 0u32;

    loop {
        if dl.is_cancelled() {
            return Err("cancelled".into());
        }
        if let Some(len) = done_len(&dl.lock()) {
            return Ok(len);
        }

        let sent = tokio::select! {
            r = fetch::get_from(client, url, (ranged != Some(false)).then_some(start)) => r,
            _ = cancelled(dl) => return Err("cancelled".into()),
        };
        let mut body = match sent {
            Ok(b) => b,
            Err(FetchError::RangeNotSatisfiable) => {
                // Nothing at `start`: the file ends at or before it. A declared
                // length past that (an estimated transcode size, a truncated
                // file) shrinks to it, so the tail isn't asked for again, and
                // whatever is missing is earlier. Each shrink is strictly
                // smaller, so this can't loop.
                let mut st = dl.lock();
                if start > 0 && st.len.is_none_or(|l| start < l) {
                    st.len = Some(start);
                    let gap = st.ranges.first_gap(0, start).map_or(0, |(gap, _)| gap);
                    drop(st);
                    dl.cond.notify_all();
                    file.set_len(start).map_err(io)?;
                    sized = true;
                    start = gap;
                    continue;
                }
                drop(st);
                failures += 1;
                if failures > MAX_FAILURES {
                    return Err(FetchError::RangeNotSatisfiable.to_string());
                }
                start = 0;
                continue;
            }
            // Subsonic reports errors (bad auth, failed transcode) as a JSON or
            // XML body with status 200. That's not audio and must never be cached.
            Err(e) => {
                failures += 1;
                if failures > MAX_FAILURES || !e.retryable() {
                    return Err(e.to_string());
                }
                backoff(dl, failures).await;
                continue;
            }
        };

        let mut pos = body.start;
        if body.ranged {
            ranged = Some(true);
        } else {
            // 200: the whole body from byte 0. Range isn't supported; read it straight through.
            if ranged.is_none() {
                tracing::debug!("{}: server ignored Range, falling back to a sequential download", redact(url));
            }
            ranged = Some(false);
        }
        {
            let mut st = dl.lock();
            st.len = st.len.or(body.total);
        }
        let len = {
            let mut st = dl.lock();
            st.headers = true;
            st.cursor = pos;
            st.len
        };
        dl.cond.notify_all();
        if !sized && let Some(len) = len {
            // Sparse preallocation so later ranged writes land at their offsets.
            file.set_len(len).map_err(io)?;
            sized = true;
        }
        file.seek(SeekFrom::Start(pos)).map_err(io)?;

        // Stream this response until it ends, runs into bytes already on disk,
        // or the next request should start somewhere else.
        let stop = loop {
            // Dropping `body.chunk()` when the other branch wins loses nothing:
            // it only polls the body for its next frame, which stays queued.
            let chunk = tokio::select! {
                biased;
                _ = dl.wake.notified() => {
                    if dl.is_cancelled() {
                        return Err("cancelled".into());
                    }
                    match relocation(&mut dl.lock(), pos, ranged == Some(true)) {
                        Some(w) => break Stop::Restart(w),
                        None => continue,
                    }
                }
                c = body.chunk() => c,
            };
            match chunk {
                Ok(Some(bytes)) => {
                    file.write_all(&bytes).map_err(io)?;
                    let end = pos + bytes.len() as u64;
                    let mut st = dl.lock();
                    st.ranges.insert(pos, end);
                    pos = end;
                    st.cursor = pos;
                    failures = 0;
                    let relocate = relocation(&mut st, pos, ranged == Some(true));
                    let ran_into_data =
                        ranged == Some(true) && st.ranges.available_from(pos) > 0 && st.len.is_none_or(|l| pos < l);
                    drop(st);
                    dl.cond.notify_all();
                    if let Some(w) = relocate {
                        break Stop::Restart(w);
                    }
                    if ran_into_data {
                        break Stop::RanIntoData;
                    }
                }
                Ok(None) => break Stop::Ended,
                Err(e) => {
                    failures += 1;
                    if failures > MAX_FAILURES {
                        return Err(e.to_string());
                    }
                    tracing::debug!("{}: {e}; retrying", redact(url));
                    backoff(dl, failures).await;
                    // Ranged: resume where it broke. Sequential: start over (bytes already written stay valid).
                    break Stop::Restart(if ranged == Some(true) { pos } else { 0 });
                }
            }
        };

        start = match stop {
            Stop::Restart(at) => at,
            Stop::Ended | Stop::RanIntoData => {
                let mut st = dl.lock();
                let learned = matches!(stop, Stop::Ended) && st.len.is_none();
                if learned {
                    // No length given, and a request for everything from here
                    // on ran out: the file ends here. Bytes skipped by an
                    // earlier relocation may still be missing below.
                    st.len = Some(pos);
                }
                let gap = match st.len {
                    Some(len) => match st.ranges.first_gap(pos, len).or_else(|| st.ranges.first_gap(0, len)) {
                        Some((gap, _)) => gap,
                        None => return Ok(len),
                    },
                    // Ran into bytes from a relocated request with the end
                    // still unknown: carry on after them.
                    None => pos + st.ranges.available_from(pos),
                };
                drop(st);
                if learned {
                    dl.cond.notify_all();
                }
                if ranged == Some(true) { gap } else { 0 }
            }
        };
    }
}

/// Why the download stopped reading a response.
enum Stop {
    /// Start the next request here: a reader is waiting elsewhere, or the
    /// response broke.
    Restart(u64),
    /// The body ended.
    Ended,
    /// The response reached bytes already on disk.
    RanIntoData,
}

fn done_len(st: &DlState) -> Option<u64> {
    st.len.filter(|&l| st.ranges.covers(l))
}

/// Where to restart for a blocked reader, if its offset is outside the current stream's reach.
fn relocation(st: &mut DlState, pos: u64, ranged: bool) -> Option<u64> {
    let w = st.wanted?;
    if st.ranges.available_from(w) > 0 {
        st.wanted = None;
        return None;
    }
    if !ranged || (w >= pos && w - pos <= LOOKAHEAD) {
        return None;
    }
    st.wanted = None;
    Some(w)
}

async fn cancelled(dl: &Download) {
    while !dl.is_cancelled() {
        dl.wake.notified().await;
    }
}

async fn backoff(dl: &Download, failures: u32) {
    tokio::select! {
        _ = tokio::time::sleep(fetch::backoff(failures, Duration::from_millis(250), Duration::from_secs(2))) => {}
        _ = cancelled(dl) => {}
    }
}

/// Opens a read handle on a download that may be renamed to its final path at any moment.
pub(crate) fn open_read(dl: &Download) -> std::io::Result<File> {
    File::open(&dl.part_path).or_else(|_| File::open(&dl.final_path))
}
