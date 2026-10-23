//! HTTP helpers shared by everything that downloads files from the server: the
//! stream cache, offline downloads, and anything else fetching a whole track.
//!
//! One place for the client's timeouts, `Range` handling, the Subsonic
//! error-body check and retry delays, so the downloaders can't drift apart.

use reqwest::StatusCode;
use reqwest::header::{CONTENT_RANGE, CONTENT_TYPE, RANGE};
use std::fmt;
use std::time::Duration;

pub use bytes::Bytes;

/// Give up connecting after this long.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A response that sends nothing for this long counts as broken: the download
/// retries and, where the server supports Range, resumes where it stopped.
pub const READ_TIMEOUT: Duration = Duration::from_secs(20);

/// An HTTP client for downloads, with a connect timeout and a read timeout
/// (time allowed between received bytes, not for the whole body).
pub fn client(read_timeout: Duration) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder().connect_timeout(CONNECT_TIMEOUT).read_timeout(read_timeout).build()
}

/// Why a request didn't produce a usable body. Never carries the URL: it
/// holds the auth token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// No answer, or the connection broke: worth retrying.
    Network(String),
    /// The server answered with an error status.
    Status(StatusCode),
    /// Asked to start past the end of the file (`416`).
    RangeNotSatisfiable,
    /// A Subsonic error envelope (bad auth, failed transcode) instead of the file.
    Server(String),
    /// A `206` the download can't use: no `Content-Range`, or a different start.
    BadRange(String),
}

impl FetchError {
    /// Trying again later might work: network trouble or a server-side failure.
    pub fn retryable(&self) -> bool {
        match self {
            Self::Network(_) | Self::BadRange(_) => true,
            Self::Status(s) => s.is_server_error() || *s == StatusCode::REQUEST_TIMEOUT || *s == StatusCode::TOO_MANY_REQUESTS,
            Self::RangeNotSatisfiable | Self::Server(_) => false,
        }
    }
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(e) => write!(f, "fetch: {e}"),
            Self::Status(s) => write!(f, "http {s}"),
            Self::RangeNotSatisfiable => f.write_str("range not satisfiable"),
            Self::Server(e) => write!(f, "server error: {e}"),
            Self::BadRange(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for FetchError {}

fn network(e: reqwest::Error) -> FetchError {
    FetchError::Network(e.without_url().to_string())
}

/// A response that's been checked and is ready to stream.
pub struct Opened {
    resp: reqwest::Response,
    /// The first chunk, read early to make sure it isn't an error envelope.
    first: Option<Bytes>,
    /// Offset of the first body byte in the file.
    pub start: u64,
    /// The file's full size, where the server said.
    pub total: Option<u64>,
    /// The server honoured `Range` (`206`); `false` for a whole-file `200`.
    pub ranged: bool,
}

impl Opened {
    /// The next piece of the body; `None` at the end.
    pub async fn chunk(&mut self) -> Result<Option<Bytes>, FetchError> {
        if let Some(first) = self.first.take() {
            return Ok(Some(first));
        }
        self.resp.chunk().await.map_err(network)
    }
}

/// GETs `url`, from byte `from` (sent as `Range: bytes=<from>-`) or, with
/// `None`, without a `Range` header.
///
/// - `206`: the start must be `from`, else [`FetchError::BadRange`]; `total`
///   comes from `Content-Range`.
/// - `200`: the whole file from byte 0 (the server ignored `Range`); `total`
///   is the `Content-Length`.
/// - `416` and other error statuses become errors.
/// - A Subsonic error envelope (by content type, or sniffed from the first
///   chunk) is [`FetchError::Server`], never a body to keep.
pub async fn get_from(client: &reqwest::Client, url: &str, from: Option<u64>) -> Result<Opened, FetchError> {
    let mut req = client.get(url);
    if let Some(from) = from {
        req = req.header(RANGE, format!("bytes={from}-"));
    }
    let mut resp = req.send().await.map_err(network)?;
    let status = resp.status();
    let (start, total, ranged) = match status {
        StatusCode::PARTIAL_CONTENT => {
            let (first, total) = resp
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_content_range)
                .ok_or_else(|| FetchError::BadRange("206 without a usable Content-Range".into()))?;
            let asked = from.unwrap_or(0);
            if first != asked {
                return Err(FetchError::BadRange(format!("asked for byte {asked}, got {first}")));
            }
            (first, total, true)
        }
        StatusCode::RANGE_NOT_SATISFIABLE => return Err(FetchError::RangeNotSatisfiable),
        s if s.is_success() => (0, resp.content_length(), false),
        s => return Err(FetchError::Status(s)),
    };
    let kind = resp.headers().get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let first = resp.chunk().await.map_err(network)?;
    let head = first.as_deref().unwrap_or_default();
    if subsonic::is_error_reply(kind.as_deref(), head) {
        // The envelope is small; read the rest so the message is whole.
        let mut body = head.to_vec();
        while body.len() < 4096
            && let Ok(Some(more)) = resp.chunk().await
        {
            body.extend_from_slice(&more);
        }
        return Err(FetchError::Server(subsonic::error_text(&body)));
    }
    Ok(Opened { resp, first, start, total, ranged })
}

/// `bytes 100-199/1000` → (100, Some(1000)); `bytes 100-199/*` → (100, None).
pub fn parse_content_range(v: &str) -> Option<(u64, Option<u64>)> {
    let rest = v.trim().strip_prefix("bytes ")?;
    let (span, total) = rest.split_once('/')?;
    let (first, _) = span.split_once('-')?;
    let total = if total == "*" { None } else { Some(total.parse().ok()?) };
    Some((first.parse().ok()?, total))
}

/// The wait before retry number `attempt` (1-based): `base`, doubling each
/// time, never more than `cap`.
pub fn backoff(attempt: u32, base: Duration, cap: Duration) -> Duration {
    base.saturating_mul(1u32 << attempt.saturating_sub(1).min(16)).min(cap)
}

/// A URL fit for logs: the path only. The query holds the auth token.
pub fn redact(url: &str) -> &str {
    url.split_once('?').map_or(url, |(path, _)| path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range() {
        assert_eq!(parse_content_range("bytes 0-99/1000"), Some((0, Some(1000))));
        assert_eq!(parse_content_range("bytes 500-999/*"), Some((500, None)));
        assert_eq!(parse_content_range("items 0-1/2"), None);
    }

    #[test]
    fn backoff_doubles_up_to_the_cap() {
        let (base, cap) = (Duration::from_millis(250), Duration::from_secs(2));
        let waits: Vec<_> = (1..=6).map(|n| backoff(n, base, cap).as_millis()).collect();
        assert_eq!(waits, [250, 500, 1000, 2000, 2000, 2000]);
        assert_eq!(backoff(40, Duration::from_secs(4), Duration::from_secs(300)), Duration::from_secs(300));
    }

    #[test]
    fn redacted_urls_keep_no_query() {
        assert_eq!(redact("http://h/rest/stream?id=1&t=secret"), "http://h/rest/stream");
        assert_eq!(redact("http://h/x"), "http://h/x");
    }
}
