//! A local HTTP/1.1 fake for API clients: one request per connection, each
//! answered on its own thread by a closure (so concurrent requests really are
//! concurrent), and every request it saw kept for the test to check.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};

/// One request as the fake saw it.
#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    /// Path and query.
    pub target: String,
    /// Header names lowercased.
    pub headers: HashMap<String, String>,
    pub body: String,
}

impl Seen {
    /// The target without its query.
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or_default()
    }

    /// A header's value, by its lowercased name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    /// The body as JSON.
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{} {} body isn't JSON ({e}): {}", self.method, self.target, self.body))
    }
}

/// What the fake answers.
pub struct Answer {
    pub status: &'static str,
    pub headers: Vec<(&'static str, String)>,
    pub body: String,
}

impl Answer {
    pub fn new(status: &'static str, body: impl Into<String>) -> Self {
        Self { status, headers: Vec::new(), body: body.into() }
    }

    pub fn ok(body: impl Into<String>) -> Self {
        Self::new("200 OK", body)
    }

    pub fn with_header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }
}

/// A server on a loopback port answering `respond` for every request.
pub struct JsonFake {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl JsonFake {
    /// `respond` runs on the connection's thread: it can block (a test holding
    /// an answer back until it's ready) or count calls.
    pub fn start(respond: impl Fn(&Seen) -> Answer + Send + Sync + 'static) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let respond = Arc::new(respond);
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let (respond, log) = (respond.clone(), log.clone());
                std::thread::spawn(move || serve(stream, &*respond, &log));
            }
        });
        Self { base, seen }
    }

    /// The origin and a trailing slash: `{base}rest/...`.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Every request so far, in arrival order.
    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// How many requests went to `path` (without its query).
    pub fn count(&self, path: &str) -> usize {
        self.seen().iter().filter(|s| s.path() == path).count()
    }
}

fn serve(mut stream: TcpStream, respond: &(dyn Fn(&Seen) -> Answer + Send + Sync), log: &Mutex<Vec<Seen>>) {
    let mut req: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = req.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        // A request starts with an uppercase method. Anything else on the
        // port is a TLS handshake sent to a plain-http server, which a web
        // server answers at once rather than waiting for a head that's never
        // coming (a client trying https on a http port needs that answer).
        if req.first().is_some_and(|b| !b.is_ascii_uppercase()) {
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return;
        }
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => req.extend_from_slice(&buf[..n]),
        }
    };
    let head = String::from_utf8_lossy(&req[..head_end]).into_owned();
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or_default().split_whitespace();
    let (method, target) = (first.next().unwrap_or_default().to_owned(), first.next().unwrap_or_default().to_owned());
    let headers: HashMap<String, String> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned())).collect();
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    while req.len() < head_end + len {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => req.extend_from_slice(&buf[..n]),
        }
    }
    let body = String::from_utf8_lossy(&req[head_end..]).into_owned();
    let seen = Seen { method, target, headers, body };
    log.lock().unwrap().push(seen.clone());
    let a = respond(&seen);
    let mut out = format!("HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n", a.status, a.body.len());
    for (k, v) in &a.headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    out.push_str(&a.body);
    let _ = stream.write_all(out.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connection_that_isnt_http_is_turned_away_at_once() {
        // What a client trying https on a plain-http port sends: a TLS
        // handshake, whose first byte isn't a method's letter.
        let fake = JsonFake::start(|_| Answer::ok("{}"));
        let mut conn = TcpStream::connect(fake.base().trim_end_matches('/').trim_start_matches("http://")).unwrap();
        conn.write_all(&[0x16, 0x03, 0x01, 0x00, 0xf8, 0x01]).unwrap();
        let mut reply = String::new();
        conn.read_to_string(&mut reply).unwrap();
        assert!(reply.starts_with("HTTP/1.1 400"), "{reply}");
        assert!(fake.seen().is_empty(), "it never became a request");
    }

    #[test]
    fn a_request_is_answered_by_the_closure_and_kept_with_its_body() {
        let fake = JsonFake::start(|seen| Answer::ok(format!("{{\"path\":\"{}\"}}", seen.path())));
        let body = "{\"at\":3}";
        let mut conn = TcpStream::connect(fake.base().trim_end_matches('/').trim_start_matches("http://")).unwrap();
        write!(conn, "POST /api/items/a/play?x=1 HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        let mut reply = String::new();
        conn.read_to_string(&mut reply).unwrap();
        assert!(reply.ends_with("{\"path\":\"/api/items/a/play\"}"), "{reply}");
        let seen = fake.seen();
        assert_eq!((seen[0].method.as_str(), seen[0].target.as_str()), ("POST", "/api/items/a/play?x=1"));
        assert_eq!(seen[0].json()["at"], 3);
        assert_eq!(fake.count("/api/items/a/play"), 1);
        assert_eq!(fake.count("/api/items/b/play"), 0);
    }
}
