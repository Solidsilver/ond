//! What can go wrong talking to a server, sorted by what the app does about
//! it: [`Network`](Error::Network) means check on the server and try later,
//! [`Auth`](Error::Auth) means the account needs the user,
//! [`Server`](Error::Server) and [`NotApi`](Error::NotApi) are shown as they
//! are, and [`Decode`](Error::Decode) is a reply this crate doesn't
//! understand.
//!
//! Errors are `Clone` so the outcome of one refresh can be handed to every
//! request that waited on it. None of them carries a URL or a token: reqwest's
//! errors lose their URL before they're kept, and a sign-in reply that doesn't
//! decode is reported by position only, since serde quotes the values it
//! rejects.

use std::sync::Arc;
use std::time::Duration;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    #[error("invalid server address: {0}")]
    Url(#[from] url::ParseError),
    /// The server couldn't be reached, or stopped answering: no connection,
    /// a timeout, a connection dropped mid-reply.
    #[error("can't reach the server: {0}")]
    Network(Arc<reqwest::Error>),
    /// The server answered with an HTTP error status. `message` is what it
    /// said in the body (its `error` field, a page's title, or the first line
    /// of plain text), if anything.
    #[error("{}", match message {
        Some(m) => format!("server error {status}: {m}"),
        None => format!("server error {status}"),
    })]
    Server { status: u16, message: Option<String> },
    #[error(transparent)]
    Auth(#[from] AuthError),
    /// A reply that should have been JSON of a known shape and wasn't.
    #[error("malformed response: {0}")]
    Decode(String),
    /// A success status with a body that isn't JSON at all. That's usually a
    /// web page, like the login page of a reverse proxy or a single sign-on
    /// gate in front of the server. Holds what the page says (its `<title>`,
    /// or its first line) when it says anything.
    #[error("{}", match .0 {
        Some(said) => format!("the server answered with a page instead of the API: {said}"),
        None => "the server answered with something other than the API".to_owned(),
    })]
    NotApi(Option<String>),
}

/// Why the account can't be used until the user does something.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    /// There's no session: never signed in, signed out, or the refresh token
    /// was expired or revoked. Signing in again is the only way back.
    #[error("signed out; sign in again")]
    SignedOut,
    /// The server turned down the username and password.
    #[error("wrong username or password")]
    BadCredentials,
    /// Too many sign-ins and refreshes from this address (the server allows
    /// 40 in 10 minutes, counting successful ones). `retry_after` is when the
    /// server said to try again, if it said.
    #[error("too many sign-in attempts; try again later")]
    RateLimited { retry_after: Option<Duration> },
}

impl Error {
    /// A failure to reach the server rather than an answer from it.
    pub fn is_network(&self) -> bool {
        matches!(self, Error::Network(_))
    }

    /// The account is signed out and needs the user to sign in again.
    pub fn is_signed_out(&self) -> bool {
        matches!(self, Error::Auth(AuthError::SignedOut))
    }

    /// The server answered 404: an item, session, progress row or bookmark
    /// that isn't there (play sessions vanish when the server restarts).
    pub fn is_not_found(&self) -> bool {
        matches!(self, Error::Server { status: 404, .. })
    }

    /// The HTTP status the server answered with, for an error that came with
    /// one.
    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Server { status, .. } => Some(*status),
            Error::Auth(AuthError::RateLimited { .. }) => Some(429),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        // reqwest appends " for url (...)" to its messages; a file URL carries
        // the access token.
        Error::Network(Arc::new(e.without_url()))
    }
}

/// The readable part of an HTTP error body: the `error` field of a JSON
/// reply, an HTML page's `<title>`, or the first line of anything else, at
/// most 200 characters. `None` when the body says nothing.
pub(crate) fn status_text(body: &[u8]) -> Option<String> {
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_slice::<serde_json::Value>(body) {
        return map.get("error").or_else(|| map.get("message")).and_then(serde_json::Value::as_str).and_then(tidy);
    }
    page_text(body)
}

/// What a body that isn't the API says: an HTML page's `<title>`, or the
/// first line of anything else, at most 200 characters.
pub(crate) fn page_text(body: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(&body[..body.len().min(4096)]);
    let text = text.trim();
    if text.starts_with('<') {
        let lower = text.to_ascii_lowercase();
        let start = lower.find("<title>")? + "<title>".len();
        let end = lower[start..].find("</title>").map_or(text.len(), |i| start + i);
        tidy(&text[start..end])
    } else {
        tidy(text.lines().next().unwrap_or_default())
    }
}

/// `text` on one line, its whitespace collapsed, at most 200 characters.
fn tidy(text: &str) -> Option<String> {
    let found: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(200).collect();
    (!found.is_empty()).then_some(found)
}

/// The body starts like a JSON object or array, so a parse error is a broken
/// reply rather than something else answering.
pub(crate) fn looks_like_json(bytes: &[u8]) -> bool {
    let start = bytes.strip_prefix(b"\xEF\xBB\xBF".as_slice()).unwrap_or(bytes);
    matches!(start.iter().find(|b| !b.is_ascii_whitespace()), Some(b'{' | b'['))
}

/// Decodes a JSON reply. A body that isn't JSON at all is [`Error::NotApi`].
pub(crate) fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes)
        .map_err(|e| if (e.is_syntax() || e.is_eof()) && !looks_like_json(bytes) { Error::NotApi(page_text(bytes)) } else { Error::Decode(e.to_string()) })
}

/// [`decode`] for a reply that carries tokens: a failure names where the
/// reply went wrong but not what serde found there, which could be a token.
pub(crate) fn decode_secret<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| {
        if (e.is_syntax() || e.is_eof()) && !looks_like_json(bytes) {
            Error::NotApi(page_text(bytes))
        } else {
            Error::Decode(format!("the sign-in reply didn't decode (line {}, column {})", e.line(), e.column()))
        }
    })
}
