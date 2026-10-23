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
    let (status, start) = match range {
        Some(s) if opts.ranges && s >= len => {
            let _ = write!(out, "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return;
        }
        Some(s) if opts.ranges => (format!("206 Partial Content\r\nContent-Range: bytes {s}-{}/{len}", len - 1), s),
        _ => ("200 OK".to_owned(), 0),
    };
    let body = &data[start as usize..];
    let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n", body.len());
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

const FAST: Opts = Opts { ranges: true, chunk: 16 * 1024, delay: Duration::ZERO };

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
    assert!(t.elapsed() < Duration::from_secs(1), "forward seek waited {:?}", t.elapsed());
    assert!(server.requests().contains(&Some(1_800_000)), "no ranged request: {:?}", server.requests());

    // Backward into what the first request already fetched.
    assert_eq!(read_at(&mut src, 100, 4096), body[100..4196]);

    // The skipped middle is filled in afterwards and the file completes.
    wait_until("gap fill", Duration::from_secs(10), || cache.progress(&key).is_some_and(|p| p.complete));
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
    let server = Server::start(body.clone(), Opts { ranges: false, chunk: 16 * 1024, delay: Duration::from_millis(2) });
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
    let server = Server::start(body, Opts { ranges: false, chunk: 1024, delay: Duration::from_millis(50) });
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
    let cfg = CacheConfig { dir: dir.path().to_owned(), max_bytes: 3_000 };
    let cache = StreamCache::new(cfg, rt.handle().clone()).unwrap();
    let server = Server::start(data(1_200), FAST);
    let fetch = |id: &str| {
        let key = CacheKey::new(id, None);
        let mut src = cache.open(&key, &server.url, never()).unwrap();
        let mut out = Vec::new();
        src.read_to_end(&mut out).unwrap();
        wait_until("completion", Duration::from_secs(5), || cache.progress(&key).is_some_and(|p| p.complete));
    };

    fetch("a");
    thread::sleep(Duration::from_millis(5));
    fetch("b");
    thread::sleep(Duration::from_millis(5));
    fetch("a"); // touches a, making b the least recently used
    thread::sleep(Duration::from_millis(5));
    fetch("c"); // 3 600 bytes > 3 000: b goes

    assert!(dir.path().join("a").exists());
    assert!(!dir.path().join("b").exists());
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
    let slow = Server::start(data(1_000_000), Opts { ranges: true, chunk: 1024, delay: Duration::from_millis(50) });
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
