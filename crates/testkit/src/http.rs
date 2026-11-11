//! A local HTTP/1.1 file server for download caches: Range support, a
//! trickle, a gate that holds the body part-way, the cut a broken network
//! makes, and the wrong answers a broken server or proxy gives. It keeps the
//! `Range` start of every request it answered.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// How the server answers. Start from [`Serve::fast`] and change what the test
/// is about.
#[derive(Clone, Copy)]
pub struct Serve {
    /// Honour `Range` with 206s (and 416 past the end) instead of answering 200.
    pub ranges: bool,
    /// Bytes per write.
    pub chunk: usize,
    /// How long to wait after each write.
    pub delay: Duration,
    /// Hold the answer at this byte until [`RangeServer::open_gate`], then send
    /// the rest at once. A request that starts past it is answered at once.
    pub gate_after: Option<usize>,
    /// Hold every response body before its first byte until [`RangeServer::open_gate`],
    /// including requests that seek past a [`Serve::gate_after`] cutoff.
    pub gate_before_body: bool,
    /// Drop the first connection after this many body bytes: a download that
    /// ends early.
    pub cut: Option<usize>,
    /// Claimed in `Content-Range` beyond the real length (an estimated
    /// transcode size).
    pub declared_extra: u64,
    /// `Content-Range: bytes a-b/*`: no total.
    pub star_total: bool,
    /// No `Content-Length`: the body ends when the connection closes.
    pub no_length: bool,
    /// Claimed in `Content-Length` beyond the bytes actually sent.
    pub short_by: u64,
    /// Answer with a Subsonic XML error envelope labelled `audio/flac`: only
    /// sniffing the body catches it.
    pub xml_error: bool,
    /// Answer a ranged request with a 206 from byte 0 regardless (a broken proxy).
    pub wrong_start: bool,
    /// The answer's `Content-Type`, when it should name one.
    pub content_type: Option<&'static str>,
}

impl Serve {
    /// Everything in order: Range honoured, 16 KB writes, no waiting.
    pub fn fast() -> Self {
        Self {
            ranges: true,
            chunk: 16 * 1024,
            delay: Duration::ZERO,
            gate_after: None,
            gate_before_body: false,
            cut: None,
            declared_extra: 0,
            star_total: false,
            no_length: false,
            short_by: 0,
            xml_error: false,
            wrong_start: false,
            content_type: None,
        }
    }

    /// [`Serve::fast`], writing a chunk every `delay`: a download that is still
    /// running when the test looks, without a gate to open.
    pub fn trickle(delay: Duration) -> Self {
        Self { delay, ..Self::fast() }
    }
}

/// A file server on a loopback port, one thread per connection.
pub struct RangeServer {
    base: String,
    requests: Arc<Mutex<Vec<Option<u64>>>>,
    gate: Arc<AtomicBool>,
}

impl RangeServer {
    /// Serves `body` on its own port. It answers for as long as the test
    /// process runs, so a test that needs only the URL can drop the handle.
    pub fn start(body: impl Into<Vec<u8>>, opts: Serve) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let gate = Arc::new(AtomicBool::new(false));
        let served = Arc::new(AtomicUsize::new(0));
        let (data, log, open) = (Arc::new(body.into()), requests.clone(), gate.clone());
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { break };
                let (data, log, gate) = (data.clone(), log.clone(), open.clone());
                let first = served.fetch_add(1, Ordering::Relaxed) == 0;
                thread::spawn(move || serve(conn, &data, opts, first, &log, &gate));
            }
        });
        Self { base, requests, gate }
    }

    /// The origin, `http://127.0.0.1:<port>`.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// The whole stream URL most caches ask for.
    pub fn url(&self) -> String {
        format!("{}/rest/stream?id=x&format=raw", self.base)
    }

    /// The `Range` start of every request answered so far, in arrival order
    /// (`None`: it asked for the whole body).
    pub fn requests(&self) -> Vec<Option<u64>> {
        self.requests.lock().unwrap().clone()
    }

    /// Lets held answers (see [`Serve::gate_after`] and [`Serve::gate_before_body`]) continue.
    pub fn open_gate(&self) {
        self.gate.store(true, Ordering::Relaxed);
    }
}

/// A held answer is let go with the server, so a test that never opens its
/// gate leaves no thread behind waiting on it.
impl Drop for RangeServer {
    fn drop(&mut self) {
        self.open_gate();
    }
}

fn serve(conn: TcpStream, data: &[u8], opts: Serve, first: bool, log: &Mutex<Vec<Option<u64>>>, gate: &AtomicBool) {
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
        let kind = content_type(opts);
        let _ =
            write!(out, "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-{}/{len}\r\n{kind}Content-Length: {len}\r\nConnection: close\r\n\r\n", len - 1);
        let _ = out.write_all(data);
        return;
    }
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
    let length = if opts.no_length { String::new() } else { format!("Content-Length: {}\r\n", body.len() as u64 + opts.short_by) };
    let kind = content_type(opts);
    let head = format!("HTTP/1.1 {status}\r\n{kind}{length}Accept-Ranges: bytes\r\nConnection: close\r\n\r\n");
    if out.write_all(head.as_bytes()).is_err() {
        return;
    }
    if opts.gate_before_body {
        while !gate.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(1));
        }
    }
    let send = match opts.cut {
        Some(n) if first => &body[..n.min(body.len())],
        _ => body,
    };
    let held = opts.gate_after.map_or(data.len(), |at| at.min(data.len())).max(start as usize);
    let paced = |out: &mut TcpStream, bytes: &[u8]| {
        for chunk in bytes.chunks(opts.chunk.max(1)) {
            if out.write_all(chunk).is_err() {
                return false;
            }
            thread::sleep(opts.delay);
        }
        true
    };
    if opts.gate_after.is_some_and(|at| (start as usize) < at.min(data.len())) {
        if !paced(&mut out, &send[..held - start as usize]) {
            return;
        }
        while !gate.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(1));
        }
        let _ = out.write_all(&send[held - start as usize..]);
    } else {
        paced(&mut out, send);
    }
}

fn content_type(opts: Serve) -> String {
    opts.content_type.map(|t| format!("Content-Type: {t}\r\n")).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{ErrorKind, Read};

    #[test]
    fn a_body_gate_holds_whole_and_ranged_responses_until_opened() {
        let body: Vec<u8> = (0..128).collect();
        let server = RangeServer::start(body.clone(), Serve { gate_before_body: true, gate_after: Some(1), ..Serve::fast() });
        let mut readers = Vec::new();
        for start in [None, Some(64)] {
            let mut conn = TcpStream::connect(server.base().strip_prefix("http://").unwrap()).unwrap();
            conn.set_read_timeout(Some(crate::slack(crate::WAIT))).unwrap();
            let range = start.map_or(String::new(), |at| format!("Range: bytes={at}-\r\n"));
            write!(conn, "GET / HTTP/1.1\r\nHost: localhost\r\n{range}Connection: close\r\n\r\n").unwrap();
            let mut reader = BufReader::new(conn);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert!(line.starts_with(if start.is_some() { "HTTP/1.1 206" } else { "HTTP/1.1 200" }), "{line}");
            loop {
                line.clear();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
            }
            reader.get_mut().set_nonblocking(true).unwrap();
            assert_eq!(reader.read(&mut [0]).unwrap_err().kind(), ErrorKind::WouldBlock);
            reader.get_mut().set_nonblocking(false).unwrap();
            readers.push((start.unwrap_or(0) as usize, reader));
        }
        server.open_gate();
        for (start, mut reader) in readers {
            let mut got = Vec::new();
            reader.read_to_end(&mut got).unwrap();
            assert_eq!(got, body[start..]);
        }
    }
}
