//! One file being fetched: shared state plus the async task that fills it.

use crate::ranges::RangeSet;
use reqwest::StatusCode;
use reqwest::header::{CONTENT_RANGE, RANGE};
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

        let mut req = client.get(url);
        if ranged != Some(false) {
            req = req.header(RANGE, format!("bytes={start}-"));
        }
        let sent = tokio::select! {
            r = req.send() => r,
            _ = cancelled(dl) => return Err("cancelled".into()),
        };
        let mut resp = match sent.and_then(|r| r.error_for_status()) {
            Ok(r) => r,
            Err(e) if e.status() == Some(StatusCode::RANGE_NOT_SATISFIABLE) => {
                // Asked past the end; whatever is missing must be before `start`.
                failures += 1;
                if failures > MAX_FAILURES {
                    return Err(format!("fetch: {e}"));
                }
                start = 0;
                continue;
            }
            Err(e) => {
                failures += 1;
                if failures > MAX_FAILURES || e.status().is_some_and(|s| s.is_client_error()) {
                    return Err(format!("fetch: {e}"));
                }
                backoff(dl, failures).await;
                continue;
            }
        };

        let mut pos = if resp.status() == StatusCode::PARTIAL_CONTENT {
            ranged = Some(true);
            let (first, total) = resp
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_content_range)
                .ok_or_else(|| "206 without a usable Content-Range".to_owned())?;
            let mut st = dl.lock();
            st.len = st.len.or(total);
            first
        } else {
            // 200: the whole body from byte 0. Range isn't supported; read it straight through.
            if ranged.is_none() {
                tracing::debug!("{url}: server ignored Range, falling back to a sequential download");
            }
            ranged = Some(false);
            let len = resp.content_length();
            let mut st = dl.lock();
            st.len = st.len.or(len);
            0
        };
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

        // Stream this response. `next` is where the following request should start;
        // `None` means the response ended normally.
        let next: Option<u64> = loop {
            let chunk = tokio::select! {
                biased;
                _ = dl.wake.notified() => {
                    if dl.is_cancelled() {
                        return Err("cancelled".into());
                    }
                    match relocation(&mut dl.lock(), pos, ranged == Some(true)) {
                        Some(w) => break Some(w),
                        None => continue,
                    }
                }
                c = resp.chunk() => c,
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
                    if relocate.is_some() {
                        break relocate;
                    }
                    if ran_into_data {
                        break None;
                    }
                }
                Ok(None) => break None,
                Err(e) => {
                    failures += 1;
                    if failures > MAX_FAILURES {
                        return Err(format!("fetch: {e}"));
                    }
                    tracing::debug!("{url}: {e}; retrying");
                    backoff(dl, failures).await;
                    // Ranged: resume where it broke. Sequential: start over (bytes already written stay valid).
                    break Some(if ranged == Some(true) { pos } else { 0 });
                }
            }
        };

        start = match next {
            Some(at) => at,
            None => {
                let mut st = dl.lock();
                let Some(len) = st.len else {
                    // No length header and the body ended: that was the whole file.
                    st.len = Some(pos);
                    return Ok(pos);
                };
                match st.ranges.first_gap(pos, len).or_else(|| st.ranges.first_gap(0, len)) {
                    Some((gap, _)) if ranged == Some(true) => gap,
                    Some(_) => 0,
                    None => return Ok(len),
                }
            }
        };
    }
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
        _ = tokio::time::sleep(Duration::from_millis(250 * u64::from(failures))) => {}
        _ = cancelled(dl) => {}
    }
}

/// `bytes 100-199/1000` → (100, Some(1000)); `bytes 100-199/*` → (100, None).
fn parse_content_range(v: &str) -> Option<(u64, Option<u64>)> {
    let rest = v.trim().strip_prefix("bytes ")?;
    let (span, total) = rest.split_once('/')?;
    let (first, _) = span.split_once('-')?;
    let total = if total == "*" { None } else { Some(total.parse().ok()?) };
    Some((first.parse().ok()?, total))
}

/// Opens a read handle on a download that may be renamed to its final path at any moment.
pub(crate) fn open_read(dl: &Download) -> std::io::Result<File> {
    File::open(&dl.part_path).or_else(|_| File::open(&dl.final_path))
}

#[cfg(test)]
mod tests {
    use super::parse_content_range;

    #[test]
    fn content_range() {
        assert_eq!(parse_content_range("bytes 0-99/1000"), Some((0, Some(1000))));
        assert_eq!(parse_content_range("bytes 500-999/*"), Some((500, None)));
        assert_eq!(parse_content_range("items 0-1/2"), None);
    }
}
