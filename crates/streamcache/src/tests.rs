use super::*;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::Instant;

/// A tiny HTTP/1.1 file server: optional Range support, throttled chunks, and a
/// log of the `Range` start each request asked for.
struct Server {
    url: String,
    requests: Arc<Mutex<Vec<Option<u64>>>>,
}

#[derive(Clone, Copy)]
struct Opts {
    ranges: bool,
    chunk: usize,
    delay: Duration,
    /// Claimed in `Content-Range` beyond the real length (an estimated transcode size).
    declared_extra: u64,
    /// `Content-Range: bytes a-b/*`: no total.
    star_total: bool,
    /// No `Content-Length`: the body ends when the connection closes.
    no_length: bool,
}

impl Server {
    fn start(data: Vec<u8>, opts: Opts) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/rest/stream?id=x&format=raw", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (data, log) = (Arc::new(data), requests.clone());
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { break };
                let (data, log) = (data.clone(), log.clone());
                thread::spawn(move || serve(conn, &data, opts, &log));
            }
        });
        Self { url, requests }
    }

    fn requests(&self) -> Vec<Option<u64>> {
        self.requests.lock().unwrap().clone()
    }
}

fn serve(conn: TcpStream, data: &[u8], opts: Opts, log: &Mutex<Vec<Option<u64>>>) {
    let mut reader = BufReader::new(conn.try_clone().unwrap());
    let mut range = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("range: bytes=") {
            range = v.trim().trim_end_matches('-').parse::<u64>().ok();
        }
    }
    log.lock().unwrap().push(range);
    let mut out = conn;
    let len = data.len() as u64;
    let total = if opts.star_total { "*".to_owned() } else { (len + opts.declared_extra).to_string() };
    let (status, start) = match range {
        Some(s) if opts.ranges && s >= len => {
            let _ = write!(out, "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return;
        }
        Some(s) if opts.ranges => (format!("206 Partial Content\r\nContent-Range: bytes {s}-{}/{total}", len - 1), s),
        _ => ("200 OK".to_owned(), 0),
    };
    let body = &data[start as usize..];
    let length = if opts.no_length { String::new() } else { format!("Content-Length: {}\r\n", body.len()) };
    let head = format!("HTTP/1.1 {status}\r\n{length}Accept-Ranges: bytes\r\nConnection: close\r\n\r\n");
    if out.write_all(head.as_bytes()).is_err() {
        return;
    }
    for chunk in body.chunks(opts.chunk) {
        if out.write_all(chunk).is_err() {
            return;
        }
        thread::sleep(opts.delay);
    }
}

fn data(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 % 251) as u8).collect()
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap()
}

fn never() -> Abort {
    Arc::new(|| false)
}

fn wait_until(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn read_at(src: &mut CachedSource, pos: u64, n: usize) -> Vec<u8> {
    src.seek(SeekFrom::Start(pos)).unwrap();
    let mut buf = vec![0; n];
    src.read_exact(&mut buf).unwrap();
    buf
}

const FAST: Opts = Opts {
    ranges: true,
    chunk: 16 * 1024,
    delay: Duration::ZERO,
    declared_extra: 0,
    star_total: false,
    no_length: false,
};

/// Reads `src` from `from` to the end on another thread, failing the test
/// instead of hanging if that takes longer than `timeout`. Hands the source back.
fn read_to_end_within(mut src: CachedSource, from: u64, timeout: Duration) -> (io::Result<Vec<u8>>, CachedSource) {
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let mut out = Vec::new();
        let r = src.seek(SeekFrom::Start(from)).and_then(|_| src.read_to_end(&mut out)).map(|_| out);
        let _ = tx.send((r, src));
    });
    rx.recv_timeout(timeout).expect("read timed out")
}

/// `got == want`, reported as lengths and the first differing offset rather
/// than megabytes of numbers.
#[track_caller]
fn assert_bytes(got: &[u8], want: &[u8]) {
    if got != want {
        let at = got.iter().zip(want).position(|(a, b)| a != b).unwrap_or(got.len().min(want.len()));
        panic!("bytes differ: got {} bytes, want {}, first difference at {at}", got.len(), want.len());
    }
}

/// The download in progress for `key`, to look at its state.
fn active(cache: &StreamCache, key: &CacheKey) -> Option<Arc<Download>> {
    cache.inner.lock_index().active.get(&key.file_name()).cloned()
}

#[test]
fn sequential_read_then_cached() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let cache = StreamCache::new(CacheConfig::new(dir.path()), rt.handle().clone()).unwrap();
    let body = data(300_000);
    let server = Server::start(body.clone(), Opts { delay: Duration::from_millis(1), ..FAST });
    let key = CacheKey::new("t1", Some("flac".into()));

    let mut src = cache.open(&key, &server.url, never()).unwrap();
    assert_eq!(src.byte_len(), Some(300_000));
    let mut out = Vec::new();
    src.read_to_end(&mut out).unwrap();
    assert_eq!(out, body);

    wait_until("completion", Duration::from_secs(5), || cache.progress(&key).is_some_and(|p| p.complete));
    assert_eq!(std::fs::read(dir.path().join("t1.flac")).unwrap(), body);
    assert_eq!(cache.size(), 300_000);
    drop(src);

    // A second open is served from disk without a request.
    let before = server.requests().len();
    let mut again = cache.open(&key, &server.url, never()).unwrap();
    assert!(matches!(again, CachedSource::File { .. }));
    let mut out = Vec::new();
    again.read_to_end(&mut out).unwrap();
    assert_eq!(out, body);
    assert_eq!(server.requests().len(), before);
}

#[test]
fn seek_forward_past_download_point_then_backward() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let cache = StreamCache::new(CacheConfig::new(dir.path()), rt.handle().clone()).unwrap();
    // ~2 MB at 16 KB / 20 ms ≈ 2.6 s to arrive sequentially.
    let body = data(2_000_000);
    let server = Server::start(body.clone(), Opts { delay: Duration::from_millis(20), ..FAST });
    let key = CacheKey::new("t2", None);

    let mut src = cache.open(&key, &server.url, never()).unwrap();
    assert_eq!(read_at(&mut src, 0, 1024), body[..1024]);

    let t = Instant::now();
    assert_eq!(read_at(&mut src, 1_800_000, 64 * 1024), body[1_800_000..1_800_000 + 64 * 1024]);
    assert!(t.elapsed() < slack(Duration::from_secs(1)), "forward seek waited {:?}", t.elapsed());
    assert!(server.requests().contains(&Some(1_800_000)), "no ranged request: {:?}", server.requests());

    // Backward into what the first request already fetched.
    assert_eq!(read_at(&mut src, 100, 4096), body[100..4196]);

    // The skipped middle is filled in afterwards and the file completes.
    wait_until("gap fill", slack(Duration::from_secs(10)), || cache.progress(&key).is_some_and(|p| p.complete));
    let mut all = Vec::new();
    src.seek(SeekFrom::Start(0)).unwrap();
    src.read_to_end(&mut all).unwrap();
    assert_eq!(all, body);
    assert_eq!(std::fs::read(dir.path().join("t2")).unwrap(), body);
}

#[test]
fn server_ignoring_range_is_read_sequentially() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let cache = StreamCache::new(CacheConfig::new(dir.path()), rt.handle().clone()).unwrap();
    let body = data(400_000);
    let server = Server::start(body.clone(), Opts { ranges: false, delay: Duration::from_millis(2), ..FAST });
    let key = CacheKey::new("t3", Some("wav".into()));

    let mut src = cache.open(&key, &server.url, never()).unwrap();
    assert_eq!(src.byte_len(), Some(400_000));
    assert_eq!(read_at(&mut src, 390_000, 10_000), body[390_000..]);
    assert_eq!(read_at(&mut src, 0, 100), body[..100]);
    wait_until("completion", Duration::from_secs(5), || cache.progress(&key).is_some_and(|p| p.complete));
    // One request: no relocation attempts against a server that can't do them.
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn blocked_read_aborts_and_dropping_cancels() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let cache = StreamCache::new(CacheConfig::new(dir.path()), rt.handle().clone()).unwrap();
    let body = data(1_000_000);
    let server = Server::start(body, Opts { ranges: false, chunk: 1024, delay: Duration::from_millis(50), ..FAST });
    let key = CacheKey::new("t4", None);

    let abort = Arc::new(AtomicBool::new(false));
    let flag = abort.clone();
    let mut src = cache.open(&key, &server.url, Arc::new(move || flag.load(Ordering::Relaxed))).unwrap();
    src.seek(SeekFrom::Start(900_000)).unwrap();
    let reader = thread::spawn(move || {
        let mut buf = [0u8; 16];
        let started = Instant::now();
        let r = src.read(&mut buf);
        (r.is_err(), started.elapsed(), src)
    });
    thread::sleep(Duration::from_millis(150));
    abort.store(true, Ordering::Relaxed);
    let (failed, waited, src) = reader.join().unwrap();
    assert!(failed, "read should fail once aborted");
    assert!(waited < Duration::from_secs(1), "abort took {waited:?}");

    // The only holder goes away: the download stops and its partial file is removed.
    drop(src);
    wait_until("cancel", Duration::from_secs(2), || cache.progress(&key).is_none());
    wait_until("part cleanup", Duration::from_secs(2), || {
        std::fs::read_dir(dir.path()).unwrap().next().is_none()
    });
}

#[test]
fn lru_eviction_keeps_recent_files() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let cfg = CacheConfig { max_bytes: 3_000, ..CacheConfig::new(dir.path()) };
    let cache = StreamCache::new(cfg, rt.handle().clone()).unwrap();
    let server = Server::start(data(1_200), FAST);
    // Waits for the index, not the download's progress: a download reports
    // complete just before it's indexed (and stamped as used).
    let fetch = |id: &str, size_after: u64| {
        let key = CacheKey::new(id, None);
        let mut src = cache.open(&key, &server.url, never()).unwrap();
        let mut out = Vec::new();
        src.read_to_end(&mut out).unwrap();
        wait_until("indexed", slack(Duration::from_secs(5)), || cache.size() == size_after);
    };

    fetch("a", 1_200);
    thread::sleep(Duration::from_millis(5));
    fetch("b", 2_400);
    thread::sleep(Duration::from_millis(5));
    fetch("a", 2_400); // touches a, making b the least recently used
    thread::sleep(Duration::from_millis(5));
    fetch("c", 2_400); // 3 600 bytes > 3 000: b goes

    // The evicted file is deleted just after the index lets it go.
    wait_until("eviction", slack(Duration::from_secs(5)), || !dir.path().join("b").exists());
    assert!(dir.path().join("a").exists());
    assert!(dir.path().join("c").exists());
    assert_eq!(cache.size(), 2_400);
}

#[test]
fn startup_indexes_files_and_drops_partials() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("done.flac"), [1u8; 500]).unwrap();
    std::fs::write(dir.path().join("half.flac.0.part"), [1u8; 200]).unwrap();
    let cache = StreamCache::new(CacheConfig::new(dir.path()), rt.handle().clone()).unwrap();
    assert_eq!(cache.size(), 500);
    assert!(!dir.path().join("half.flac.0.part").exists());
    let p = cache.progress(&CacheKey::new("done", Some("flac".into()))).unwrap();
    assert!(p.complete);
}

#[test]
fn prefetch_downloads_without_a_reader_and_replaces_the_previous_one() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let cache = StreamCache::new(CacheConfig::new(dir.path()), rt.handle().clone()).unwrap();
    let slow = Server::start(data(1_000_000), Opts { chunk: 1024, delay: Duration::from_millis(50), ..FAST });
    let fast = Server::start(data(100_000), FAST);
    let (k1, k2) = (CacheKey::new("p1", None), CacheKey::new("p2", None));

    cache.prefetch(&k1, &slow.url);
    wait_until("p1 started", Duration::from_secs(2), || cache.progress(&k1).is_some_and(|p| p.downloaded > 0));
    // A reader attaching to the prefetch shares the same download.
    let src = cache.open(&k1, &slow.url, never()).unwrap();
    assert_eq!(slow.requests().len(), 1);
    drop(src);

    cache.prefetch(&k2, &fast.url);
    wait_until("p1 cancelled", Duration::from_secs(2), || cache.progress(&k1).is_none());
    wait_until("p2 complete", Duration::from_secs(5), || cache.progress(&k2).is_some_and(|p| p.complete));
}

#[test]
fn key_file_names_are_safe() {
    assert_eq!(CacheKey::new("ab/../c d", Some("FLAC".into())).file_name(), "ab%2F%2E%2E%2Fc%20d.flac");
    assert_eq!(CacheKey::new("x", Some("".into())).file_name(), "x");
}

#[test]
fn a_subsonic_error_reply_is_not_cached_as_audio() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    // A stale error reply from an older version is purged at startup.
    std::fs::write(dir.path().join("old.flac"), br#"{"subsonic-response":{"status":"failed","error":{"code":70}}}"#).unwrap();
    let cache = StreamCache::new(CacheConfig::new(dir.path()), rt.handle().clone()).unwrap();
    assert!(!dir.path().join("old.flac").exists());

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/rest/stream?id=x", listener.local_addr().unwrap());
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { break };
            let mut reader = BufReader::new(conn.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
                line.clear();
            }
            let body: &[u8] = br#"{"subsonic-response":{"status":"failed","error":{"code":40}}}"#;
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = conn.write_all(head.as_bytes());
            let _ = conn.write_all(body);
        }
    });
    let key = CacheKey::new("bad", Some("flac".into()));
    // Either the open or the first read reports the failure; neither yields audio.
    if let Ok(mut src) = cache.open(&key, &url, never()) {
        let mut out = Vec::new();
        assert!(src.read_to_end(&mut out).is_err(), "read {} bytes of an error reply as audio", out.len());
    }
    thread::sleep(Duration::from_millis(100));
    assert!(!dir.path().join("bad.flac").exists());
    assert!(!cache.progress(&key).is_some_and(|p| p.complete));
}

/// A connection that goes quiet mid-body (half-open TCP, a stuck proxy) must
/// not hang the download: the read timeout fires and a ranged request resumes.
#[test]
fn a_stalled_response_times_out_and_resumes() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let cfg = CacheConfig { read_timeout: Duration::from_millis(300), ..CacheConfig::new(dir.path()) };
    let cache = StreamCache::new(cfg, rt.handle().clone()).unwrap();
    let body = Arc::new(data(200_000));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/rest/stream?id=s", listener.local_addr().unwrap());
    let starts = Arc::new(Mutex::new(Vec::new()));
    let (served, log) = (body.clone(), starts.clone());
    thread::spawn(move || {
        for (n, conn) in listener.incoming().enumerate() {
            let Ok(conn) = conn else { break };
            let (data, log) = (served.clone(), log.clone());
            thread::spawn(move || {
                let mut reader = BufReader::new(conn.try_clone().unwrap());
                let mut start = 0u64;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        start = v.trim().trim_end_matches('-').parse().unwrap_or(0);
                    }
                }
                log.lock().unwrap().push(start);
                let len = data.len() as u64;
                let rest = &data[start as usize..];
                let mut out = conn;
                let _ = write!(
                    out,
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{len}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    len - 1,
                    rest.len()
                );
                if n == 0 {
                    // Half the body, then silence with the socket held open.
                    let _ = out.write_all(&rest[..rest.len() / 2]);
                    let _ = out.flush();
                    thread::sleep(Duration::from_secs(10));
                } else {
                    let _ = out.write_all(rest);
                }
            });
        }
    });
    let key = CacheKey::new("stall", None);
    let mut src = cache.open(&key, &url, never()).unwrap();
    let t = Instant::now();
    let mut all = Vec::new();
    src.read_to_end(&mut all).unwrap();
    assert_eq!(all, *body);
    assert!(t.elapsed() < slack(Duration::from_secs(5)), "took {:?}", t.elapsed());
    let starts = starts.lock().unwrap().clone();
    assert!(starts.len() >= 2 && starts[1] > 0, "resumed with a Range request: {starts:?}");
}

#[test]
fn clearing_forgets_and_deletes_every_file() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let cache = StreamCache::new(CacheConfig::new(dir.path()), rt.handle().clone()).unwrap();
    let server = Server::start(data(1_200), FAST);
    for id in ["a", "b"] {
        let key = CacheKey::new(id, None);
        let mut src = cache.open(&key, &server.url, never()).unwrap();
        let mut out = Vec::new();
        src.read_to_end(&mut out).unwrap();
        wait_until("completion", Duration::from_secs(5), || cache.progress(&key).is_some_and(|p| p.complete));
    }
    assert_eq!(cache.size(), 2_400);
    cache.clear();
    assert_eq!(cache.size(), 0);
    assert!(cache.complete_path(&CacheKey::new("a", None)).is_none());
    wait_until("deletion", Duration::from_secs(5), || !dir.path().join("a").exists() && !dir.path().join("b").exists());
}

/// The audio engine cancels everything and then opens the next track, while
/// the old decoder may still hold its reader. The new open must not attach to
/// the download that was just cancelled.
#[test]
fn cancel_all_then_open_starts_a_fresh_download() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let cache = StreamCache::new(CacheConfig::new(dir.path()), rt.handle().clone()).unwrap();
    let body = data(600_000);
    let server = Server::start(body.clone(), Opts { delay: Duration::from_millis(5), ..FAST });
    let key = CacheKey::new("c1", None);

    let mut old = cache.open(&key, &server.url, never()).unwrap();
    assert_eq!(read_at(&mut old, 0, 1024), body[..1024]);
    cache.cancel_all();
    let new = cache.open(&key, &server.url, never()).unwrap();
    let (out, _new) = read_to_end_within(new, 0, slack(Duration::from_secs(10)));
    assert_bytes(&out.unwrap(), &body);

    // The old reader letting go doesn't cancel its replacement.
    drop(old);
    wait_until("completion", slack(Duration::from_secs(5)), || cache.progress(&key).is_some_and(|p| p.complete));
    wait_until("indexed", slack(Duration::from_secs(5)), || cache.size() == 600_000);
    assert_bytes(&std::fs::read(dir.path().join("c1")).unwrap(), &body);
}

#[test]
fn a_200_without_content_length_is_read_to_its_end() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let cache = StreamCache::new(CacheConfig::new(dir.path()), rt.handle().clone()).unwrap();
    let body = data(300_000);
    let server =
        Server::start(body.clone(), Opts { ranges: false, no_length: true, delay: Duration::from_millis(1), ..FAST });
    let key = CacheKey::new("chunked", None);

    let src = cache.open(&key, &server.url, never()).unwrap();
    assert_eq!(src.byte_len(), None);
    let (out, src) = read_to_end_within(src, 0, slack(Duration::from_secs(5)));
    assert_bytes(&out.unwrap(), &body);
    assert_eq!(src.byte_len(), Some(300_000));
    wait_until("completion", slack(Duration::from_secs(5)), || cache.progress(&key).is_some_and(|p| p.complete));
    assert_eq!(cache.progress(&key).unwrap().len, Some(300_000));
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn a_backward_seek_into_a_gap_fetches_it() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let cache = StreamCache::new(CacheConfig::new(dir.path()), rt.handle().clone()).unwrap();
    let body = data(2_000_000);
    let server = Server::start(body.clone(), Opts { delay: Duration::from_millis(20), ..FAST });
    let key = CacheKey::new("gap", None);

    let mut src = cache.open(&key, &server.url, never()).unwrap();
    assert_eq!(read_at(&mut src, 0, 1024), body[..1024]);
    assert_eq!(read_at(&mut src, 1_500_000, 4096), body[1_500_000..1_504_096]);
    // Behind the relocated stream and past what the first one fetched.
    let t = Instant::now();
    assert_eq!(read_at(&mut src, 800_000, 4096), body[800_000..804_096]);
    assert!(t.elapsed() < slack(Duration::from_secs(1)), "backward seek waited {:?}", t.elapsed());
    assert!(server.requests().contains(&Some(800_000)), "no ranged request: {:?}", server.requests());

    let (out, _src) = read_to_end_within(src, 0, slack(Duration::from_secs(10)));
    assert_bytes(&out.unwrap(), &body);
}

/// Two readers on one download, far apart, each reading on from where it is.
/// Both get their bytes and the download completes. (There's one request
/// stream, so it alternates between them, a few chunks per request. Nothing in
/// the app blocks two readers on one unfinished download today.)
#[test]
fn two_readers_seek_independently() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let cache = StreamCache::new(CacheConfig::new(dir.path()), rt.handle().clone()).unwrap();
    let body = data(2_000_000);
    let server = Server::start(body.clone(), Opts { delay: Duration::from_millis(5), ..FAST });
    let key = CacheKey::new("two", None);

    let a = cache.open(&key, &server.url, never()).unwrap();
    let b = cache.open(&key, &server.url, never()).unwrap();
    let from_b = 1_200_000;
    let b_thread = thread::spawn(move || read_to_end_within(b, from_b, slack(Duration::from_secs(10))).0);
    let (out_a, _a) = read_to_end_within(a, 0, slack(Duration::from_secs(10)));
    assert_bytes(&out_a.unwrap(), &body);
    assert_bytes(&b_thread.join().unwrap().unwrap(), &body[from_b as usize..]);
    wait_until("completion", slack(Duration::from_secs(5)), || cache.progress(&key).is_some_and(|p| p.complete));
    wait_until("indexed", slack(Duration::from_secs(5)), || cache.size() == 2_000_000);
    assert_bytes(&std::fs::read(dir.path().join("two")).unwrap(), &body);
}

/// A relocation asked for while the download is between requests (here in
/// the wait after a broken response) is acted on as soon as the next response
/// brings its first bytes, even if that response then stalls.
#[test]
fn a_seek_while_reconnecting_is_not_lost() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let cfg = CacheConfig { read_timeout: Duration::from_secs(3), ..CacheConfig::new(dir.path()) };
    let cache = StreamCache::new(cfg, rt.handle().clone()).unwrap();
    let body = Arc::new(data(2_000_000));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/rest/stream?id=r", listener.local_addr().unwrap());
    let starts = Arc::new(Mutex::new(Vec::new()));
    let gate = Arc::new(AtomicBool::new(false));
    let (served, log, open_gate) = (body.clone(), starts.clone(), gate.clone());
    thread::spawn(move || {
        for (n, conn) in listener.incoming().enumerate() {
            let Ok(conn) = conn else { break };
            let (data, log, gate) = (served.clone(), log.clone(), open_gate.clone());
            thread::spawn(move || {
                let mut reader = BufReader::new(conn.try_clone().unwrap());
                let mut start = 0u64;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        start = v.trim().trim_end_matches('-').parse().unwrap_or(0);
                    }
                }
                log.lock().unwrap().push(start);
                let len = data.len() as u64;
                let rest = &data[start as usize..];
                let mut out = conn;
                if n == 1 {
                    // The reconnect: answer only once the test says so.
                    while !gate.load(Ordering::Relaxed) {
                        thread::sleep(Duration::from_millis(5));
                    }
                }
                let _ = write!(
                    out,
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{len}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    len - 1,
                    rest.len()
                );
                match n {
                    // 64 KB, then the connection drops: the download backs off and reconnects.
                    0 => {
                        let _ = out.write_all(&rest[..64 * 1024]);
                    }
                    // A little, then silence with the socket held open.
                    1 => {
                        let _ = out.write_all(&rest[..16 * 1024]);
                        let _ = out.flush();
                        thread::sleep(Duration::from_secs(30));
                    }
                    _ => {
                        let _ = out.write_all(rest);
                    }
                }
            });
        }
    });
    let key = CacheKey::new("reconnect", None);
    let src = cache.open(&key, &url, never()).unwrap();
    wait_until("reconnect", slack(Duration::from_secs(5)), || starts.lock().unwrap().len() >= 2);
    // The download is now waiting on the reconnect's answer. A reader asks for a far offset.
    let far = 1_500_000;
    let reader = thread::spawn(move || {
        let mut src = src;
        let got = read_at(&mut src, far, 64 * 1024);
        (got, src)
    });
    let dl = active(&cache, &key).unwrap();
    wait_until("reader waiting", slack(Duration::from_secs(5)), || dl.lock().wanted == Some(far));
    let t = Instant::now();
    gate.store(true, Ordering::Relaxed);
    let (got, _src) = reader.join().unwrap();
    assert_eq!(got, body[far as usize..far as usize + 64 * 1024]);
    assert!(t.elapsed() < Duration::from_secs(3), "the reader waited out the stall: {:?}", t.elapsed());
    assert_eq!(starts.lock().unwrap()[2], far);
}

/// A server that declares more than it has (an estimated transcode length, a
/// truncated file) answers 416 for the missing tail. The file ends where the
/// data does, instead of re-requesting the tail forever.
#[test]
fn a_declared_length_past_the_real_end_ends_at_the_data() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let cache = StreamCache::new(CacheConfig::new(dir.path()), rt.handle().clone()).unwrap();
    let body = data(300_000);
    let server = Server::start(body.clone(), Opts { declared_extra: 50_000, ..FAST });
    let key = CacheKey::new("long", None);

    let src = cache.open(&key, &server.url, never()).unwrap();
    assert_eq!(src.byte_len(), Some(350_000));
    let (out, src) = read_to_end_within(src, 0, slack(Duration::from_secs(5)));
    assert_bytes(&out.unwrap(), &body);
    assert_eq!(src.byte_len(), Some(300_000), "the reader learns the real length");

    wait_until("indexed", slack(Duration::from_secs(5)), || cache.size() == 300_000);
    assert_bytes(&std::fs::read(dir.path().join("long")).unwrap(), &body);
    assert_eq!(server.requests(), [Some(0), Some(300_000)]);
}

/// `Content-Range: bytes a-b/*` never says how long the file is. A relocated
/// request running to the end tells us, but the part skipped before it must
/// still be fetched before the file counts as complete.
#[test]
fn ranges_without_a_total_fill_the_skipped_part() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let cache = StreamCache::new(CacheConfig::new(dir.path()), rt.handle().clone()).unwrap();
    let body = data(2_000_000);
    let server = Server::start(body.clone(), Opts { star_total: true, delay: Duration::from_millis(5), ..FAST });
    let key = CacheKey::new("star", None);

    let mut src = cache.open(&key, &server.url, never()).unwrap();
    assert_eq!(src.byte_len(), None);
    assert_eq!(read_at(&mut src, 0, 1024), body[..1024]);
    assert_eq!(read_at(&mut src, 1_500_000, 64 * 1024), body[1_500_000..1_500_000 + 64 * 1024]);
    assert!(server.requests().contains(&Some(1_500_000)), "no ranged request: {:?}", server.requests());

    let (out, _src) = read_to_end_within(src, 0, slack(Duration::from_secs(10)));
    assert_bytes(&out.unwrap(), &body);
    wait_until("completion", slack(Duration::from_secs(5)), || cache.progress(&key).is_some_and(|p| p.complete));
    wait_until("indexed", slack(Duration::from_secs(5)), || cache.size() == 2_000_000);
    assert_bytes(&std::fs::read(dir.path().join("star")).unwrap(), &body);
}

/// With no total, a seek past the real end is only found out by the server's
/// 416. The reader gets the end of the file, and the download still completes
/// at the right length.
#[test]
fn a_seek_past_the_end_of_a_file_without_a_total_reads_as_the_end() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let cache = StreamCache::new(CacheConfig::new(dir.path()), rt.handle().clone()).unwrap();
    let body = data(1_000_000);
    let server = Server::start(body.clone(), Opts { star_total: true, delay: Duration::from_millis(5), ..FAST });
    let key = CacheKey::new("past", None);

    let mut src = cache.open(&key, &server.url, never()).unwrap();
    assert_eq!(read_at(&mut src, 0, 1024), body[..1024]);
    let (tail, src) = read_to_end_within(src, 1_800_000, slack(Duration::from_secs(5)));
    assert_eq!(tail.unwrap().len(), 0);
    assert!(server.requests().contains(&Some(1_800_000)), "no ranged request: {:?}", server.requests());

    let (out, _src) = read_to_end_within(src, 0, slack(Duration::from_secs(10)));
    assert_bytes(&out.unwrap(), &body);
    wait_until("indexed", slack(Duration::from_secs(5)), || cache.size() == 1_000_000);
    assert_bytes(&std::fs::read(dir.path().join("past")).unwrap(), &body);
}

/// Time budgets in these tests are tight on purpose (they guard real
/// performance), but shared CI runners are slower and noisier: allow 10× there.
fn slack(d: std::time::Duration) -> std::time::Duration {
    if std::env::var_os("CI").is_some() { d * 10 } else { d }
}
