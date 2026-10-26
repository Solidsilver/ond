use super::*;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::AtomicUsize;
use std::thread;

/// A tiny HTTP/1.1 file server. Logs each request's `Range` start; can drop the
/// first connection after `cut` bytes (a network failure mid-download).
struct Server {
    base: String,
    requests: Arc<Mutex<Vec<Option<u64>>>>,
}

#[derive(Clone, Copy)]
struct Opts {
    ranges: bool,
    cut: Option<usize>,
    delay: Duration,
    /// Answer with a Subsonic XML error envelope (labelled as audio, so only
    /// sniffing the body catches it).
    xml_error: bool,
    /// Answer a ranged request with a 206 from byte 0 regardless (a broken proxy).
    wrong_start: bool,
}

const OPTS: Opts = Opts { ranges: true, cut: None, delay: Duration::ZERO, xml_error: false, wrong_start: false };

impl Server {
    fn start(data: Vec<u8>, opts: Opts) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (data, log) = (Arc::new(data), requests.clone());
        let served = Arc::new(AtomicUsize::new(0));
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { break };
                let (data, log) = (data.clone(), log.clone());
                let first = served.fetch_add(1, Ordering::Relaxed) == 0;
                thread::spawn(move || serve(conn, &data, opts, first, &log));
            }
        });
        Self { base, requests }
    }

    fn url(&self) -> UrlFn {
        let base = self.base.clone();
        Arc::new(move |id| format!("{base}/rest/stream?id={id}"))
    }

    fn requests(&self) -> Vec<Option<u64>> {
        self.requests.lock().unwrap().clone()
    }
}

fn serve(conn: TcpStream, data: &[u8], opts: Opts, first: bool, log: &Mutex<Vec<Option<u64>>>) {
    let mut reader = BufReader::new(conn.try_clone().unwrap());
    let mut range = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
            range = v.trim().trim_end_matches('-').parse::<u64>().ok();
        }
    }
    log.lock().unwrap().push(range);
    let mut out = conn;
    let len = data.len() as u64;
    if opts.xml_error {
        let body = r#"<?xml version="1.0" encoding="UTF-8"?><subsonic-response status="failed"><error code="40" message="Wrong username or password"/></subsonic-response>"#;
        let _ = write!(out, "HTTP/1.1 200 OK\r\nContent-Type: audio/flac\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        return;
    }
    if opts.wrong_start && range.is_some() {
        let _ = write!(out, "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-{}/{len}\r\nContent-Type: audio/flac\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n", len - 1);
        let _ = out.write_all(data);
        return;
    }
    let (status, start) = match range {
        Some(s) if opts.ranges && s >= len => {
            let _ = write!(out, "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return;
        }
        Some(s) if opts.ranges => (format!("206 Partial Content\r\nContent-Range: bytes {s}-{}/{len}", len - 1), s),
        _ => ("200 OK".to_owned(), 0),
    };
    let body = &data[start as usize..];
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: audio/flac\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    if out.write_all(head.as_bytes()).is_err() {
        return;
    }
    let send = match opts.cut {
        Some(n) if first => &body[..n.min(body.len())],
        _ => body,
    };
    for chunk in send.chunks(16 * 1024) {
        if out.write_all(chunk).is_err() {
            return;
        }
        thread::sleep(opts.delay);
    }
    // A cut connection just closes early; the client sees a short body.
}

fn data(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 % 251) as u8).collect()
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap()
}

fn store(dir: &Path, rt: &tokio::runtime::Runtime) -> OfflineStore {
    OfflineStore::open(OfflineConfig { dir: dir.to_owned(), limit: None }, rt.handle().clone(), Arc::new(|| {})).unwrap()
}

fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < slack(Duration::from_secs(10)), "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn track(id: &str, len: usize) -> TrackRef {
    TrackRef::new(id, Some("flac".into()), Some(len as u64))
}

#[test]
fn downloads_and_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let rt = rt();
    let body = data(300_000);
    let server = Server::start(body.clone(), OPTS);
    let s = store(dir.path(), &rt);
    s.set_source(Some(server.url()));
    s.pin(Pin::Album("al".into()), "Album", vec![track("t1", body.len()), track("t2", body.len())]);
    wait_for("both downloads", || s.snapshot().pin(&Pin::Album("al".into())).is_some_and(PinSummary::complete));
    let path = s.local_path("t1").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), body);
    assert_eq!(s.snapshot().used_bytes, 2 * body.len() as u64);
    drop(s);

    // Reopened offline: still there, nothing to fetch.
    let s = store(dir.path(), &rt);
    assert!(s.is_pinned(&Pin::Album("al".into())));
    assert_eq!(s.local_path("t2").unwrap(), dir.path().join("t2.flac"));
    assert!(s.snapshot().is_done("t1"));
    assert_eq!(s.pin_tracks(&Pin::Album("al".into())).unwrap(), ("Album".to_owned(), vec!["t1".to_owned(), "t2".to_owned()]));
}

#[test]
fn resumes_with_a_range_after_the_connection_drops() {
    let dir = tempfile::tempdir().unwrap();
    let rt = rt();
    let body = data(400_000);
    let server = Server::start(body.clone(), Opts { cut: Some(150_000), ..OPTS });
    let s = store(dir.path(), &rt);
    s.set_source(Some(server.url()));
    s.pin(Pin::Track("t".into()), "T", vec![track("t", body.len())]);
    // The first attempt fails short; the retry waits out a backoff, so skip it.
    wait_for("a failed attempt", || server.requests().len() == 1 && s.snapshot().track("t") == Some(&TrackState::Queued));
    s.retry_failed();
    wait_for("the download", || s.snapshot().is_done("t"));
    assert_eq!(server.requests(), [None, Some(150_000)]);
    assert_eq!(std::fs::read(s.local_path("t").unwrap()).unwrap(), body);
}

#[test]
fn pause_keeps_the_partial_file_and_a_restart_resumes_it() {
    let dir = tempfile::tempdir().unwrap();
    let rt = rt();
    let body = data(2_000_000);
    let server = Server::start(body.clone(), Opts { delay: Duration::from_millis(5), ..OPTS });
    let s = store(dir.path(), &rt);
    s.set_source(Some(server.url()));
    s.pin(Pin::Track("t".into()), "T", vec![track("t", body.len())]);
    wait_for("some progress", || matches!(s.snapshot().track("t"), Some(TrackState::Downloading { done, .. }) if *done > 100_000));
    s.set_paused(true);
    wait_for("the pause", || s.snapshot().active == 0);
    let part = dir.path().join("t.flac.part");
    let kept = std::fs::metadata(&part).unwrap().len();
    assert!(kept > 0);
    drop(s);

    let s = store(dir.path(), &rt);
    assert!(s.snapshot().paused, "pause is remembered");
    s.set_source(Some(server.url()));
    s.set_paused(false);
    wait_for("the download", || s.snapshot().is_done("t"));
    assert!(server.requests()[1].is_some_and(|from| from >= kept - 64 * 1024), "{:?}", server.requests());
    assert_eq!(std::fs::read(s.local_path("t").unwrap()).unwrap(), body);
}

#[test]
fn takes_complete_stream_cache_files_without_downloading() {
    let dir = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let rt = rt();
    let body = data(50_000);
    std::fs::write(cache.path().join("t.flac"), &body).unwrap();
    let server = Server::start(body.clone(), OPTS);
    let s = store(dir.path(), &rt);
    let cache_dir = cache.path().to_owned();
    s.set_cache_lookup(Arc::new(move |r| Some(cache_dir.join(file_name(r)))));
    // Works while offline too: no URL at all.
    s.pin(Pin::Track("t".into()), "T", vec![track("t", body.len())]);
    wait_for("the promotion", || s.snapshot().is_done("t"));
    assert!(server.requests().is_empty());
    assert_eq!(std::fs::read(s.local_path("t").unwrap()).unwrap(), body);
    // The cache evicting its copy doesn't touch ours.
    std::fs::remove_file(cache.path().join("t.flac")).unwrap();
    assert_eq!(std::fs::read(s.local_path("t").unwrap()).unwrap(), body);
}

#[test]
fn a_track_taken_from_the_stream_cache_leaves_no_partial_download_behind() {
    let dir = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let rt = rt();
    let body = data(50_000);
    std::fs::write(cache.path().join("t.flac"), &body).unwrap();
    let s = store(dir.path(), &rt);
    // Half downloaded earlier, then played to the end.
    s.pin(Pin::Track("t".into()), "T", vec![track("t", body.len())]);
    std::fs::write(dir.path().join("t.flac.part"), &body[..1000]).unwrap();
    let cache_dir = cache.path().to_owned();
    s.set_cache_lookup(Arc::new(move |r| Some(cache_dir.join(file_name(r)))));
    wait_for("the promotion", || s.snapshot().is_done("t"));
    assert!(!dir.path().join("t.flac.part").exists());
    assert_eq!(std::fs::read(s.local_path("t").unwrap()).unwrap(), body);
}

#[test]
fn unpinning_deletes_only_unshared_files() {
    let dir = tempfile::tempdir().unwrap();
    let rt = rt();
    let body = data(10_000);
    let server = Server::start(body.clone(), OPTS);
    let s = store(dir.path(), &rt);
    s.set_source(Some(server.url()));
    s.pin(Pin::Album("a".into()), "A", vec![track("1", body.len()), track("2", body.len())]);
    s.pin(Pin::Playlist("p".into()), "P", vec![track("2", body.len())]);
    wait_for("downloads", || s.snapshot().is_done("1") && s.snapshot().is_done("2"));
    s.unpin(&Pin::Album("a".into()));
    wait_for("1.flac to go", || !dir.path().join("1.flac").exists());
    assert!(dir.path().join("2.flac").exists());
    assert!(s.local_path("1").is_none());
    s.unpin(&Pin::Playlist("p".into()));
    wait_for("2.flac to go", || !dir.path().join("2.flac").exists());
    assert_eq!(s.snapshot().used_bytes, 0);
}

#[test]
fn unpinning_a_big_playlist_returns_at_once_and_cleans_up_behind() {
    let dir = tempfile::tempdir().unwrap();
    let rt = rt();
    let s = store(dir.path(), &rt);
    let n = 3_000;
    let tracks: Vec<TrackRef> = (0..n).map(|i| track(&format!("t{i}"), 10)).collect();
    // No source: nothing downloads; the files stand in for finished ones.
    s.pin(Pin::Playlist("big".into()), "Big", tracks);
    s.pin(Pin::Track("keep".into()), "K", vec![track("keep", 1)]);
    for i in 0..n {
        std::fs::write(dir.path().join(format!("t{i}.flac")), b"x").unwrap();
    }
    std::fs::write(dir.path().join("keep.flac"), b"x").unwrap();
    let t = Instant::now();
    s.unpin(&Pin::Playlist("big".into()));
    assert!(t.elapsed() < slack(Duration::from_millis(200)), "unpin took {:?}", t.elapsed());
    // A track pinned again before the deleter gets to it keeps its file: as if
    // it was unpinned and pinned again while the deleter had the big
    // playlist's files ahead of it in its queue.
    s.inner.delete(vec!["keep".into()]);
    // Then one that does go, to know the deleter got past "keep".
    std::fs::write(dir.path().join("last.flac"), b"x").unwrap();
    s.inner.delete(vec!["last".into()]);
    let gone = |e: &std::fs::DirEntry| e.file_name() != "keep.flac" && e.file_name().to_string_lossy().ends_with(".flac");
    wait_for("the files to go", || !std::fs::read_dir(dir.path()).unwrap().flatten().any(|e| gone(&e)));
    assert!(dir.path().join("keep.flac").exists());
}

#[test]
fn a_size_mismatch_fails_and_a_server_ignoring_ranges_starts_over() {
    let dir = tempfile::tempdir().unwrap();
    let rt = rt();
    let body = data(30_000);
    let server = Server::start(body.clone(), Opts { ranges: false, ..OPTS });
    let s = store(dir.path(), &rt);
    s.set_source(Some(server.url()));
    s.pin(Pin::Track("bad".into()), "Bad", vec![track("bad", body.len() + 1)]);
    wait_for("a failed attempt", || !server.requests().is_empty() && s.snapshot().track("bad") == Some(&TrackState::Queued));
    assert!(s.local_path("bad").is_none());
    assert!(!dir.path().join("bad.flac.part").exists(), "a wrong-size file is discarded");

    // A stale partial file with a server that answers 200: rewritten from the start.
    std::fs::write(dir.path().join("ok.flac.part"), vec![0u8; 1000]).unwrap();
    s.pin(Pin::Track("ok".into()), "Ok", vec![track("ok", body.len())]);
    wait_for("the download", || s.snapshot().is_done("ok"));
    assert_eq!(std::fs::read(s.local_path("ok").unwrap()).unwrap(), body);
}

#[test]
fn the_limit_holds_back_downloads() {
    let dir = tempfile::tempdir().unwrap();
    let rt = rt();
    let body = data(10_000);
    let server = Server::start(body.clone(), OPTS);
    let s = OfflineStore::open(OfflineConfig { dir: dir.path().to_owned(), limit: Some(15_000) }, rt.handle().clone(), Arc::new(|| {})).unwrap();
    s.set_source(Some(server.url()));
    s.pin(Pin::Album("a".into()), "A", vec![track("1", body.len()), track("2", body.len())]);
    wait_for("the first download and the limit", || s.snapshot().is_done("1") && s.snapshot().limit_reached);
    assert_eq!(s.snapshot().track("2"), Some(&TrackState::Queued));
    s.set_limit(None);
    wait_for("the second download", || s.snapshot().is_done("2"));
}

#[test]
fn strays_are_removed_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let rt = rt();
    let s = store(dir.path(), &rt);
    s.pin(Pin::Track("t".into()), "T", vec![track("t", 1)]);
    drop(s);
    // Left behind by a crash between an unpin and its delete.
    std::fs::write(dir.path().join("orphan.flac"), b"x").unwrap();
    std::fs::write(dir.path().join("orphan.flac.part"), b"x").unwrap();
    std::fs::write(dir.path().join("t.flac.part"), b"x").unwrap();
    let s = store(dir.path(), &rt);
    assert!(!dir.path().join("orphan.flac").exists());
    assert!(!dir.path().join("orphan.flac.part").exists());
    assert!(dir.path().join("t.flac.part").exists(), "a pinned track's partial download stays");
    assert_eq!(s.snapshot().used_bytes, 0);
}

#[test]
fn a_moved_store_opens_with_its_downloads() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let to = b.path().join("Offline");
    let rt = rt();
    let body = data(20_000);
    let server = Server::start(body.clone(), OPTS);
    let s = store(a.path(), &rt);
    s.set_source(Some(server.url()));
    s.pin(Pin::Track("t".into()), "T", vec![track("t", body.len())]);
    wait_for("the download", || s.snapshot().is_done("t"));
    s.set_source(None);
    drop(s);
    move_store(a.path(), &to).unwrap();
    let s = store(&to, &rt);
    assert!(s.snapshot().is_done("t"));
    assert_eq!(std::fs::read(s.local_path("t").unwrap()).unwrap(), body);
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn pinning_many_tracks_at_once_returns_at_once_and_is_saved() {
    let dir = tempfile::tempdir().unwrap();
    let rt = rt();
    let s = store(dir.path(), &rt);
    let pins: Vec<(Pin, String, Vec<TrackRef>)> =
        (0..500).map(|i| (Pin::Track(format!("t{i}")), format!("Song {i}"), vec![track(&format!("t{i}"), 10)])).collect();
    let t = Instant::now();
    // No source: nothing downloads, so this measures pinning alone.
    s.pin_many(pins);
    assert!(t.elapsed() < slack(Duration::from_millis(50)), "pin_many took {:?}", t.elapsed());
    assert!(s.is_pinned(&Pin::Track("t499".into())));
    drop(s);
    // Saved in the background, and complete once the store is gone.
    let s = store(dir.path(), &rt);
    assert_eq!(s.snapshot().pins.len(), 500);
    assert_eq!(s.pin_tracks(&Pin::Track("t250".into())).unwrap(), ("Song 250".to_owned(), vec!["t250".to_owned()]));
    s.unpin_many(&(0..500).map(|i| Pin::Track(format!("t{i}"))).collect::<Vec<_>>());
    drop(s);
    assert!(store(dir.path(), &rt).snapshot().pins.is_empty());
}

#[test]
fn an_xml_error_reply_is_never_kept_as_a_track() {
    let dir = tempfile::tempdir().unwrap();
    let rt = rt();
    let server = Server::start(data(10_000), Opts { xml_error: true, ..OPTS });
    let s = store(dir.path(), &rt);
    s.set_source(Some(server.url()));
    s.pin(Pin::Track("t".into()), "T", vec![track("t", 10_000)]);
    wait_for("a failed attempt", || !server.requests().is_empty() && s.snapshot().track("t") == Some(&TrackState::Queued));
    assert!(s.local_path("t").is_none());
    assert!(!dir.path().join("t.flac").exists());
    assert!(!dir.path().join("t.flac.part").exists(), "nothing of the error page is written");
}

#[test]
fn a_resume_answered_from_the_wrong_offset_starts_over() {
    let dir = tempfile::tempdir().unwrap();
    let rt = rt();
    let body = data(40_000);
    let server = Server::start(body.clone(), Opts { wrong_start: true, ..OPTS });
    let s = store(dir.path(), &rt);
    // Pinned with no source yet, then a partial file from before appears: the
    // resume asks for byte 1000 and gets byte 0.
    s.pin(Pin::Track("t".into()), "T", vec![track("t", body.len())]);
    std::fs::write(dir.path().join("t.flac.part"), vec![7u8; 1000]).unwrap();
    s.set_source(Some(server.url()));
    wait_for("the refused resume", || !server.requests().is_empty() && s.snapshot().track("t") == Some(&TrackState::Queued));
    assert!(!dir.path().join("t.flac.part").exists(), "the partial file is dropped, not appended to");
    s.retry_failed();
    wait_for("the download", || s.snapshot().is_done("t"));
    assert_eq!(server.requests(), [Some(1000), None]);
    assert_eq!(std::fs::read(s.local_path("t").unwrap()).unwrap(), body);
}

#[test]
fn each_account_keeps_its_own_downloads_and_old_stores_are_adopted() {
    let base = tempfile::tempdir().unwrap();
    let rt = rt();
    let body = data(20_000);
    let server = Server::start(body.clone(), OPTS);
    // A store kept straight in the downloads folder, as older versions did.
    let s = store(base.path(), &rt);
    s.set_source(Some(server.url()));
    s.pin(Pin::Track("t".into()), "T", vec![track("t", body.len())]);
    wait_for("the download", || s.snapshot().is_done("t"));
    s.set_source(None);
    drop(s);

    // The account signed in at the time takes it over, files and all.
    let mine = account_store(base.path(), 1).unwrap();
    let s = store(&mine, &rt);
    assert!(s.snapshot().is_done("t"));
    assert_eq!(std::fs::read(s.local_path("t").unwrap()).unwrap(), body);
    drop(s);

    // Another account starts empty and never sees those pins.
    let theirs = account_store(base.path(), 2).unwrap();
    assert_ne!(mine, theirs);
    let s = store(&theirs, &rt);
    assert!(s.snapshot().pins.is_empty());
    drop(s);

    // Back to the first account: everything is still there.
    let s = store(&account_store(base.path(), 1).unwrap(), &rt);
    assert!(s.snapshot().is_done("t"));
    // Moving the whole downloads folder brings every account along.
    drop(s);
    let to = tempfile::tempdir().unwrap();
    let moved = to.path().join("Offline");
    move_store(base.path(), &moved).unwrap();
    let s = store(&account_store(&moved, 1).unwrap(), &rt);
    assert!(s.snapshot().is_done("t"));
    assert_eq!(server.requests().len(), 1);
}

/// Time budgets in these tests are tight on purpose (they guard real
/// performance), but shared CI runners are slower and noisier: allow 10× there.
fn slack(d: std::time::Duration) -> std::time::Duration {
    if std::env::var_os("CI").is_some() { d * 10 } else { d }
}

#[test]
fn an_older_save_arriving_late_doesnt_overwrite_a_newer_one() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let flush = Arc::new(Flush::default());
    let saver = {
        let (dir, flush) = (dir.path().to_owned(), flush.clone());
        thread::spawn(move || saver(dir, rx, flush))
    };
    tx.send(Save { revision: 2, bytes: b"two".to_vec() }).unwrap();
    flush.wait_for(2, Duration::from_secs(5));
    // The commit that made revision 1 lost the race to hand it over.
    tx.send(Save { revision: 1, bytes: b"one".to_vec() }).unwrap();
    drop(tx);
    saver.join().unwrap();
    assert_eq!(std::fs::read(dir.path().join(STATE_FILE)).unwrap(), b"two");
}

/// Names in `dir` starting with `prefix`.
fn names_starting(dir: &Path, prefix: &str) -> Vec<String> {
    std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(prefix)).collect()
}

#[test]
fn a_corrupt_state_file_is_set_aside_and_no_download_is_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let rt = rt();
    std::fs::write(dir.path().join("t1.flac"), b"one").unwrap();
    std::fs::write(dir.path().join("t2.flac.part"), b"tw").unwrap();
    std::fs::write(dir.path().join(STATE_FILE), b"{\"pins\": [tor").unwrap();
    let s = store(dir.path(), &rt);
    assert!(dir.path().join("t1.flac").exists() && dir.path().join("t2.flac.part").exists(), "files stay");
    assert!(!dir.path().join(STATE_FILE).exists(), "the bad file is moved aside");
    let aside = names_starting(dir.path(), &format!("{STATE_FILE}.bad-"));
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert_eq!(std::fs::read(dir.path().join(&aside[0])).unwrap(), b"{\"pins\": [tor");
    // A new state file that doesn't know the old files doesn't make them strays
    // either, as long as the bad one is still beside them.
    s.pin(Pin::Track("x".into()), "X", vec![track("x", 1)]);
    drop(s);
    let s = store(dir.path(), &rt);
    assert!(s.is_pinned(&Pin::Track("x".into())));
    assert!(dir.path().join("t1.flac").exists() && dir.path().join("t2.flac.part").exists(), "files still stay");
}

#[test]
fn an_unreadable_state_file_deletes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let rt = rt();
    std::fs::write(dir.path().join("t1.flac"), b"one").unwrap();
    // Reading a folder fails with something other than "not found", as a
    // permissions error would (which root, running the tests, would get past).
    std::fs::create_dir(dir.path().join(STATE_FILE)).unwrap();
    let s = store(dir.path(), &rt);
    assert!(dir.path().join("t1.flac").exists());
    assert!(s.snapshot().pins.is_empty());
    assert_eq!(names_starting(dir.path(), &format!("{STATE_FILE}.bad-")).len(), 1);
}

#[test]
fn dropping_the_store_stops_it_and_it_never_writes_over_one_reopened_there() {
    let dir = tempfile::tempdir().unwrap();
    let rt = rt();
    let body = data(2_000_000);
    let server = Server::start(body.clone(), Opts { delay: Duration::from_millis(5), ..OPTS });
    let s = store(dir.path(), &rt);
    s.set_source(Some(server.url()));
    s.pin(Pin::Track("t".into()), "T", vec![track("t", body.len())]);
    let progress = |s: &OfflineStore| match s.snapshot().track("t") {
        Some(TrackState::Downloading { done, .. }) => *done,
        _ => 0,
    };
    wait_for("some progress", || progress(&s) > 0);
    // Dropping a copy isn't dropping the store: it keeps going.
    let at = progress(&s);
    drop(s.clone());
    wait_for("more progress", || progress(&s) > at);

    let old = Arc::downgrade(&s.inner);
    drop(s);
    wait_for("the dropped store's tasks to end", || old.upgrade().is_none());

    // Signed back in to the same account: the old store's download can't
    // finish behind this one's back and save its pins over these.
    let s = store(dir.path(), &rt);
    assert!(s.is_pinned(&Pin::Track("t".into())));
    s.unpin(&Pin::Track("t".into()));
    wait_for("the partial file to go", || !dir.path().join("t.flac.part").exists());
    drop(s);
    let s = store(dir.path(), &rt);
    assert!(s.snapshot().pins.is_empty());
    assert!(!dir.path().join("t.flac").exists());
}

#[test]
fn a_track_repinned_with_another_suffix_while_downloading_is_found_where_it_landed() {
    let dir = tempfile::tempdir().unwrap();
    let rt = rt();
    let body = data(1_000_000);
    let server = Server::start(body.clone(), Opts { delay: Duration::from_millis(5), ..OPTS });
    let s = store(dir.path(), &rt);
    s.set_source(Some(server.url()));
    s.pin(Pin::Track("t".into()), "T", vec![track("t", body.len())]);
    wait_for("the download to start", || matches!(s.snapshot().track("t"), Some(TrackState::Downloading { .. })));
    // The server now reports another format (transcoding switched on, say).
    s.pin(Pin::Playlist("p".into()), "P", vec![TrackRef::new("t", Some("mp3".into()), Some(10))]);
    wait_for("the download", || s.snapshot().is_done("t"));
    let path = s.local_path("t").unwrap();
    assert_eq!(path, dir.path().join("t.flac"));
    assert_eq!(std::fs::read(path).unwrap(), body);
    assert_eq!(s.snapshot().used_bytes, body.len() as u64);
}

#[test]
fn a_track_pinned_again_while_its_files_are_being_deleted_downloads_after() {
    let dir = tempfile::tempdir().unwrap();
    let rt = rt();
    let body = data(10_000);
    let server = Server::start(body.clone(), OPTS);
    let s = store(dir.path(), &rt);
    // The deleter, with an unpinned track's files, past its check that the
    // track isn't pinned again...
    let mut ids = vec!["t".to_owned()];
    assert!(s.inner.deleting(&mut ids));
    assert_eq!(ids, ["t"]);
    // ...and the track pinned again while it removes them: a download now
    // would write a partial file the delete unlinks.
    s.pin(Pin::Track("t".into()), "T", vec![track("t", body.len())]);
    s.set_source(Some(server.url()));
    // Nothing to wait on for something that mustn't happen: give it a moment.
    thread::sleep(Duration::from_millis(100));
    assert!(server.requests().is_empty(), "no download while the delete runs");
    s.inner.deleted(&ids);
    wait_for("the download", || s.snapshot().is_done("t"));
    assert_eq!(std::fs::read(s.local_path("t").unwrap()).unwrap(), body);
}

#[test]
fn unpinning_mid_download_deletes_the_partial_file() {
    let dir = tempfile::tempdir().unwrap();
    let rt = rt();
    let body = data(2_000_000);
    let server = Server::start(body.clone(), Opts { delay: Duration::from_millis(5), ..OPTS });
    let s = store(dir.path(), &rt);
    s.set_source(Some(server.url()));
    s.pin(Pin::Track("t".into()), "T", vec![track("t", body.len())]);
    wait_for("some progress", || matches!(s.snapshot().track("t"), Some(TrackState::Downloading { done, .. }) if *done > 0));
    s.unpin(&Pin::Track("t".into()));
    wait_for("the download to stop", || s.snapshot().active == 0);
    wait_for("the partial file to go", || !dir.path().join("t.flac.part").exists());
    assert!(!dir.path().join("t.flac").exists());
    assert!(s.snapshot().tracks.is_empty());
}

#[test]
fn a_pin_shrinking_on_the_server_mid_download_drops_the_track_it_lost() {
    let dir = tempfile::tempdir().unwrap();
    let rt = rt();
    let body = data(2_000_000);
    let server = Server::start(body.clone(), Opts { delay: Duration::from_millis(5), ..OPTS });
    let s = store(dir.path(), &rt);
    s.set_source(Some(server.url()));
    let pin = Pin::Playlist("p".into());
    s.pin(pin.clone(), "P", vec![track("a", body.len()), track("b", body.len())]);
    // Two at a time: both are downloading.
    wait_for("both downloads", || s.snapshot().active == 2);
    s.update(&pin, "P", vec![track("a", body.len())]);
    wait_for("b's partial file to go", || !dir.path().join("b.flac.part").exists() && s.snapshot().active <= 1);
    assert!(s.snapshot().track("b").is_none());
    wait_for("a", || s.snapshot().is_done("a"));
    assert_eq!(s.pin_tracks(&pin).unwrap().1, ["a"]);
    assert!(!dir.path().join("b.flac").exists());
}

#[test]
fn extend_grows_a_pin_and_never_shrinks_it() {
    let dir = tempfile::tempdir().unwrap();
    let rt = rt();
    let s = store(dir.path(), &rt);
    let pin = Pin::Album("a".into());
    s.pin(pin.clone(), "A", vec![track("1", 10), track("2", 10)]);
    // A partial list (the library mid-sync) leaves the rest alone.
    s.extend(&pin, vec![track("2", 10)]);
    assert_eq!(s.pin_tracks(&pin).unwrap(), ("A".to_owned(), vec!["1".to_owned(), "2".to_owned()]));
    s.extend(&pin, vec![track("3", 10), track("1", 10)]);
    assert_eq!(s.pin_tracks(&pin).unwrap().1, ["1", "2", "3"]);
    assert_eq!(s.snapshot().track("3"), Some(&TrackState::Queued));
    // Only pins that exist grow.
    s.extend(&Pin::Album("b".into()), vec![track("4", 10)]);
    assert!(!s.is_pinned(&Pin::Album("b".into())));
    assert!(s.snapshot().track("4").is_none());
}

#[test]
fn the_limit_lets_unknown_sizes_through_only_until_it_is_reached() {
    let dir = tempfile::tempdir().unwrap();
    let rt = rt();
    let body = data(10_000);
    let server = Server::start(body.clone(), OPTS);
    let s = OfflineStore::open(OfflineConfig { dir: dir.path().to_owned(), limit: Some(5_000) }, rt.handle().clone(), Arc::new(|| {})).unwrap();
    s.set_source(Some(server.url()));
    let unknown = |id: &str| TrackRef::new(id, Some("flac".into()), None);
    // Nothing tells it this one won't fit.
    s.pin(Pin::Track("1".into()), "1", vec![unknown("1")]);
    wait_for("the first download", || s.snapshot().is_done("1"));
    assert_eq!(s.snapshot().used_bytes, 10_000);
    // Over the limit now: nothing more starts, whatever its size.
    s.pin(Pin::Track("2".into()), "2", vec![unknown("2")]);
    wait_for("the limit", || s.snapshot().limit_reached);
    assert_eq!(s.snapshot().track("2"), Some(&TrackState::Queued));
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn a_missing_state_file_beside_downloads_deletes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let rt = rt();
    std::fs::write(dir.path().join("t1.flac"), b"one").unwrap();
    let s = store(dir.path(), &rt);
    s.pin(Pin::Track("x".into()), "X", vec![track("x", 1)]);
    drop(s);
    drop(store(dir.path(), &rt));
    assert!(dir.path().join("t1.flac").exists());
}
