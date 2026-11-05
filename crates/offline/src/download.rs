//! The download engine: a worker claims a track from the store, takes it from
//! the stream cache or fetches it into its `.part` (resuming when the server
//! proves it's the same file), checks its size and renames it into place.
//! Every touch of the disk runs on the blocking pool, counted in `Inner::io`,
//! so dropping the store can wait for the writes an aborted worker left.

use crate::{Inner, Job, JobError, Next, PROGRESS_EVERY, RESUME_OVERLAP};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use streamcache::disk::{PartFile, blocking_tracked};
use streamcache::fetch::{self, FetchError};

/// Downloads one track after another until the store shuts down (and is also
/// aborted then, so a download in flight stops at once).
pub(crate) async fn worker(inner: Arc<Inner>) {
    loop {
        // Registered before looking, so a change in between still wakes us.
        let notified = inner.wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let due = match inner.claim() {
            Next::Job(job) => {
                let result = run(&inner, &job).await;
                inner.complete(job, result);
                continue;
            }
            Next::Wait(due) => due,
            Next::Stop => return,
        };
        let wait = due.map_or(Duration::from_secs(60), |d| d.saturating_duration_since(Instant::now()));
        tokio::select! {
            _ = &mut notified => {}
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

async fn run(inner: &Inner, job: &Job) -> Result<u64, JobError> {
    if let Some(src) = &job.from_cache {
        // A copy across volumes can take a while: not on the async workers.
        let (src, size, part, dest) = (src.clone(), job.r.size, job.part.clone(), job.dest.clone());
        match blocking_tracked(&inner.io, move || promote(&src, size, &part, &dest)).await {
            Ok(size) => return Ok(size),
            Err(e) => tracing::debug!("promote {} from cache: {e}", job.r.id),
        }
    }
    let Some(url) = &job.url else { return Err(JobError::Failed("server unreachable".into())) };
    download(inner, job, url).await
}

/// Deletes `path` (if it's there) on the blocking pool.
async fn remove_file(inner: &Inner, path: &Path) {
    let path = path.to_owned();
    let _ = blocking_tracked(&inner.io, move || std::fs::remove_file(path)).await;
}

/// At open, for a track the pins don't record as done: whether its finished
/// file at `path` is there anyway, with its size.
///
/// A download renames its `.part` into place before the store records it, and
/// the pins reach the disk later still, so a crash in between leaves a whole
/// file the pins don't know is done. Without this it would be downloaded again
/// from byte 0. With the pin's size known, a file of that size is taken as
/// done. A file of another size is left for the download to replace.
///
/// Without a size there's nothing to check the file against, and it might be
/// a copy of an older version of the track. It goes back to being the
/// `.part`, unless one is already there. The resume then asks the server for
/// the last [`RESUME_OVERLAP`] bytes again and keeps the file only if they
/// match. That costs one small request instead of the whole file.
pub(crate) fn reconcile(path: &Path, size: Option<u64>) -> Option<u64> {
    let len = std::fs::metadata(path).ok().filter(std::fs::Metadata::is_file)?.len();
    match size {
        Some(size) => (len == size).then_some(len),
        None => {
            let mut part = path.as_os_str().to_owned();
            part.push(".part");
            let part = PathBuf::from(part);
            if !part.exists()
                && let Err(e) = std::fs::rename(path, &part)
            {
                tracing::warn!("offline: making {} a partial download again: {e}", path.display());
            }
            None
        }
    }
}

/// Takes a complete stream-cache file: a hard link where possible (same
/// volume; eviction there only unlinks its own name), else a copy.
fn promote(src: &Path, size: Option<u64>, part: &Path, dest: &Path) -> io::Result<u64> {
    let len = std::fs::metadata(src)?.len();
    if size.is_some_and(|s| s != len) {
        return Err(io::Error::other("cached file has the wrong size"));
    }
    let _ = std::fs::remove_file(dest);
    if std::fs::hard_link(src, dest).is_ok() {
        // A download started earlier isn't needed now.
        let _ = std::fs::remove_file(part);
    } else {
        std::fs::copy(src, part)?;
        std::fs::rename(part, dest)?;
    }
    Ok(len)
}

/// Every touch of the disk here runs on the blocking pool, counted in
/// `Inner::io` (see [`blocking_tracked`]).
async fn download(inner: &Inner, job: &Job, url: &str) -> Result<u64, JobError> {
    let part = job.part.clone();
    let mut have = blocking_tracked(&inner.io, move || std::fs::metadata(part)).await.map_or(0, |m| m.len());
    if job.r.size.is_some_and(|s| have > s) {
        have = 0;
    }
    loop {
        match fetch_into_part(inner, job, url, have).await? {
            Some(have) => return finish(inner, job, have).await,
            // Never twice: from byte 0 there's nothing to match.
            None => {
                tracing::info!("offline: {} changed on the server since its partial download; starting over", job.r.id);
                have = 0;
            }
        }
    }
}

/// Downloads into the `.part`, resuming after its first `have` bytes when the
/// server proves it's the same file: its size matches the pin's, and the last
/// [`RESUME_OVERLAP`] bytes of the `.part`, asked for again, match. Returns
/// the bytes now in the `.part`, or `None` when the resume didn't line up
/// (start over from 0).
async fn fetch_into_part(inner: &Inner, job: &Job, url: &str, mut have: u64) -> Result<Option<u64>, JobError> {
    let overlap = have.min(RESUME_OVERLAP);
    let mut body = match fetch::get_from(&inner.http, url, (have > 0).then(|| have - overlap)).await {
        Ok(b) => b,
        Err(FetchError::RangeNotSatisfiable) if have > 0 => {
            // Even the overlap lies past the end: the file shrank.
            return Ok(None);
        }
        Err(FetchError::RangeNotSatisfiable) => {
            // No `Range` was sent, so there was no resume to reject.
            remove_file(inner, &job.part).await;
            return Err(JobError::Failed("the server refused the request".into()));
        }
        Err(FetchError::BadRange(e)) => {
            // The server answered the resume from somewhere else: the partial
            // file can't be trusted to line up. Start over.
            remove_file(inner, &job.part).await;
            return Err(JobError::Failed(e));
        }
        Err(e) => return Err(JobError::Failed(e.to_string())),
    };
    // What the resumed body must start with.
    let mut expect: &[u8] = &[];
    let tail;
    if !body.ranged {
        // The server ignored the range (or there was none): start over.
        have = 0;
        if let (Some(expected), Some(total)) = (job.r.size, body.total)
            && total != expected
        {
            // The pin's size is stale (the file was replaced on the server).
            // Every attempt would download it whole and fail the same check, so
            // it fails now, for good, until a refresh brings the new size.
            remove_file(inner, &job.part).await;
            return Err(JobError::Stale(format!("the server's file is {total} bytes, not the {expected} expected")));
        }
    } else if have > 0 {
        if job.r.size.is_some_and(|s| body.total != Some(s)) {
            return Ok(None);
        }
        let part = job.part.clone();
        tail = blocking_tracked(&inner.io, move || read_tail(&part, overlap)).await?;
        expect = &tail;
    }
    let total = body.total;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true).append(have > 0).truncate(have == 0);
    let file = PartFile::open_tracked(options, job.part.clone(), inner.io.clone()).await?;
    let mut last_note = Instant::now();
    inner.progress(&job.r.id, have, total, true);
    loop {
        if job.cancel.load(Ordering::Relaxed) {
            return Err(JobError::Cancelled);
        }
        let chunk = match body.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => return Err(JobError::Failed(e.to_string())),
        };
        let mut chunk = chunk;
        if !expect.is_empty() {
            let n = expect.len().min(chunk.len());
            if chunk[..n] != expect[..n] {
                return Ok(None);
            }
            expect = &expect[n..];
            chunk = chunk.slice(n..);
        }
        let n = chunk.len() as u64;
        if n > 0 {
            file.write(chunk).await?;
        }
        have += n;
        let note = last_note.elapsed() >= PROGRESS_EVERY;
        if note {
            last_note = Instant::now();
        }
        inner.progress(&job.r.id, have, total, note);
    }
    file.sync_all().await?;
    if let Some(t) = total
        && have != t
    {
        return Err(JobError::Failed(format!("incomplete: {have} of {t} bytes")));
    }
    if !expect.is_empty() {
        return Err(JobError::Failed("the resume ended before it could be checked".into()));
    }
    Ok(Some(have))
}

/// The last `len` bytes of `path`.
fn read_tail(path: &Path, len: u64) -> io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::End(-(len as i64)))?;
    let mut buf = Vec::with_capacity(len as usize);
    f.take(len).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Checks the size and moves the finished `.part` into place.
async fn finish(inner: &Inner, job: &Job, have: u64) -> Result<u64, JobError> {
    if let Some(expected) = job.r.size
        && have != expected
    {
        remove_file(inner, &job.part).await;
        return Err(JobError::Failed(format!("size mismatch: got {have} bytes, server said {expected}")));
    }
    let (part, dest) = (job.part.clone(), job.dest.clone());
    blocking_tracked(&inner.io, move || std::fs::rename(part, dest)).await?;
    Ok(have)
}

pub(crate) fn backoff(attempts: u32) -> Duration {
    fetch::backoff(attempts, Duration::from_secs(4), Duration::from_secs(300))
}
