//! Minimal OpenSubsonic client for Navidrome.
//!
//! Auth uses the token + salt scheme (`t = md5(password + salt)`), with a fresh
//! salt per request. Every response is requested as JSON and unwrapped from the
//! `subsonic-response` envelope before the payload is decoded.

mod auth;
mod lenient;
mod model;

pub use auth::{Credentials, Token};
pub use model::*;

use serde::de::DeserializeOwned;
use url::Url;

/// Protocol version we advertise. Navidrome speaks 1.16.1 plus OpenSubsonic extensions.
pub const API_VERSION: &str = "1.16.1";
pub const CLIENT_NAME: &str = "ond";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The request failed, or the server answered with an HTTP error status.
    #[error("http: {0}")]
    Http(HttpError),
    #[error("invalid server url: {0}")]
    Url(#[from] url::ParseError),
    #[error("malformed response: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("server error {code}: {message}")]
    Api { code: i32, message: String },
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Http(HttpError { source: e.without_url(), message: None })
    }
}

/// A failed request: what reqwest reports, plus what the server said in the
/// body of an HTTP error status (a Subsonic error message, an error page's
/// title, or the first line of a plain-text body). Derefs to the
/// [`reqwest::Error`], so `is_timeout()`, `status()` and the like work on it.
#[derive(Debug)]
pub struct HttpError {
    /// Without its URL: reqwest appends " for url (...)" to its messages, and
    /// every request URL carries `u`, `t` and `s`, which must never reach a
    /// toast or a log.
    source: reqwest::Error,
    message: Option<String>,
}

impl HttpError {
    /// What the server said along with an HTTP error status, if anything.
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }
}

impl std::ops::Deref for HttpError {
    type Target = reqwest::Error;
    fn deref(&self) -> &reqwest::Error {
        &self.source
    }
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.message {
            Some(m) => write!(f, "{}: {m}", self.source),
            None => self.source.fmt(f),
        }
    }
}

impl std::error::Error for HttpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// Turns an HTTP error status into [`Error::Http`] carrying what the server
/// said in the body; passes a success through.
async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response> {
    let Err(e) = resp.error_for_status_ref() else { return Ok(resp) };
    let source = e.without_url();
    let body = resp.bytes().await.unwrap_or_default();
    Err(Error::Http(HttpError { source, message: status_text(&body) }))
}

/// The readable part of an HTTP error body: a Subsonic error's message, an
/// HTML page's `<title>`, or the first line of anything else, at most 200
/// characters. `None` when the body says nothing.
fn status_text(body: &[u8]) -> Option<String> {
    let found = if let Err(Error::Api { message, .. }) = parse_response::<serde_json::Value>(body) {
        message
    } else {
        let text = String::from_utf8_lossy(&body[..body.len().min(4096)]);
        let text = text.trim();
        if text.starts_with('<') {
            let lower = text.to_ascii_lowercase();
            let start = lower.find("<title>")? + "<title>".len();
            let end = lower[start..].find("</title>").map_or(text.len(), |i| start + i);
            text[start..end].to_owned()
        } else {
            text.lines().next().unwrap_or_default().to_owned()
        }
    };
    let found: String = found.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(200).collect();
    (!found.is_empty()).then_some(found)
}

pub type Result<T> = std::result::Result<T, Error>;

/// Give up connecting after this long.
pub const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Give up on a response that stops sending for this long (half-open
/// connections would otherwise hang a request forever).
pub const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Give up on an API call (or a cover) that hasn't finished in this long, even
/// if it's still trickling in. Streams have no such limit: they take as long
/// as the file does.
pub const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

#[derive(Clone)]
pub struct Client {
    base: Url,
    creds: Credentials,
    http: reqwest::Client,
    call_timeout: std::time::Duration,
}

impl Client {
    pub fn new(server: &str, creds: Credentials) -> Result<Self> {
        let mut base = Url::parse(server)?;
        // "music.example.com:4533" parses, with "music.example.com" as its
        // scheme, but no path can be joined onto it.
        if !matches!(base.scheme(), "http" | "https") || base.cannot_be_a_base() {
            return Err(Error::Url(url::ParseError::RelativeUrlWithoutBase));
        }
        // A query or fragment on the address means nothing to the API.
        base.set_query(None);
        base.set_fragment(None);
        // Url::join replaces the last path segment unless the base ends in '/'.
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let http = reqwest::Client::builder()
            .user_agent(concat!("ond/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()?;
        Ok(Self { base, creds, http, call_timeout: CALL_TIMEOUT })
    }

    /// Replaces [`CALL_TIMEOUT`] for this client.
    pub fn with_call_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.call_timeout = timeout;
        self
    }

    pub fn username(&self) -> &str {
        &self.creds.username
    }

    /// Builds a signed endpoint URL; `params` are appended after the auth params.
    pub fn endpoint(&self, method: &str, params: &[(&str, &str)]) -> Url {
        let mut url = self
            .base
            .join(&format!("rest/{method}"))
            .expect("method names are valid path segments");
        let token = self.creds.token();
        url.query_pairs_mut()
            .append_pair("u", &self.creds.username)
            .append_pair("t", &token.token)
            .append_pair("s", &token.salt)
            .append_pair("v", API_VERSION)
            .append_pair("c", CLIENT_NAME)
            .append_pair("f", "json")
            .extend_pairs(params);
        url
    }

    /// URL for the original file bytes (no server-side transcode).
    pub fn stream_url(&self, song_id: &str) -> Url {
        self.endpoint("stream", &[("id", song_id), ("format", "raw")])
    }

    /// URL for a server-side transcode to `format` (e.g. `aac`, `mp3`) at up to
    /// `max_kbps`. The server needs a transcoding configured for that format.
    pub fn transcoded_stream_url(&self, song_id: &str, format: &str, max_kbps: u32) -> Url {
        self.endpoint("stream", &[("id", song_id), ("format", format), ("maxBitRate", &max_kbps.to_string())])
    }

    pub fn cover_art_url(&self, cover_id: &str, size: Option<u32>) -> Url {
        match size {
            Some(px) => self.endpoint("getCoverArt", &[("id", cover_id), ("size", &px.to_string())]),
            None => self.endpoint("getCoverArt", &[("id", cover_id)]),
        }
    }

    async fn call<T: DeserializeOwned>(&self, method: &str, params: &[(&str, &str)]) -> Result<(ServerInfo, T)> {
        let resp = self.http.get(self.endpoint(method, params)).timeout(self.call_timeout).send().await?;
        let bytes = check_status(resp).await?.bytes().await?;
        parse_response(&bytes)
    }

    pub async fn ping(&self) -> Result<ServerInfo> {
        let (info, _): (ServerInfo, serde_json::Value) = self.call("ping", &[]).await?;
        Ok(info)
    }

    /// One page of `getAlbumList2`. Navidrome caps `size` at 500.
    pub async fn album_list(&self, kind: AlbumListType, size: u32, offset: u32) -> Result<Vec<Album>> {
        let (_, p): (_, AlbumList2Payload) = self
            .call(
                "getAlbumList2",
                &[("type", kind.as_str()), ("size", &size.to_string()), ("offset", &offset.to_string())],
            )
            .await?;
        Ok(p.album_list2.album)
    }

    pub async fn album(&self, id: &str) -> Result<AlbumWithSongs> {
        let (_, p): (_, AlbumPayload) = self.call("getAlbum", &[("id", id)]).await?;
        Ok(p.album)
    }

    /// One song as the server has it now, with the user's play count and
    /// last-played time.
    pub async fn song(&self, id: &str) -> Result<Song> {
        let (_, p): (_, SongPayload) = self.call("getSong", &[("id", id)]).await?;
        Ok(p.song)
    }

    /// Cover art bytes as the server sends them (usually JPEG), scaled server-side to `size` px.
    pub async fn cover_art(&self, cover_id: &str, size: Option<u32>) -> Result<Vec<u8>> {
        let resp = self.http.get(self.cover_art_url(cover_id, size)).timeout(self.call_timeout).send().await?;
        let resp = check_status(resp).await?;
        let kind = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let bytes = resp.bytes().await?;
        // Errors come back as an envelope rather than an image.
        if is_error_reply(kind.as_deref(), &bytes) {
            return Err(match parse_response::<serde_json::Value>(&bytes) {
                Err(e @ Error::Api { .. }) => e,
                _ => Error::Api { code: -1, message: error_text(&bytes) },
            });
        }
        Ok(bytes.to_vec())
    }

    /// OpenSubsonic `getLyricsBySongId`: every lyrics version the server has
    /// for the song (embedded or sidecar `.lrc`), synced where available.
    pub async fn lyrics_by_song(&self, id: &str) -> Result<Vec<StructuredLyrics>> {
        let (_, p): (_, LyricsListPayload) = self.call("getLyricsBySongId", &[("id", id)]).await?;
        Ok(p.lyrics_list.structured_lyrics)
    }

    /// Plain `getLyrics`, matched by artist and title; `None` when there are none.
    pub async fn lyrics(&self, artist: &str, title: &str) -> Result<Option<String>> {
        let (_, p): (_, LyricsPayload) = self.call("getLyrics", &[("artist", artist), ("title", title)]).await?;
        Ok(p.lyrics.and_then(|l| l.value).filter(|v| !v.trim().is_empty()))
    }

    pub async fn playlists(&self) -> Result<Vec<Playlist>> {
        let (_, p): (_, PlaylistsPayload) = self.call("getPlaylists", &[]).await?;
        Ok(p.playlists.playlist)
    }

    pub async fn playlist(&self, id: &str) -> Result<PlaylistWithSongs> {
        let (_, p): (_, PlaylistPayload) = self.call("getPlaylist", &[("id", id)]).await?;
        Ok(p.playlist)
    }

    /// Reports a play to the server. `submission = false` marks it as "now playing";
    /// `true` records the play (Navidrome forwards both to Last.fm / ListenBrainz).
    /// `time_ms` is when playback started, in milliseconds since the Unix epoch.
    pub async fn scrobble(&self, id: &str, time_ms: u64, submission: bool) -> Result<()> {
        let (_, _): (_, serde_json::Value) = self
            .call(
                "scrobble",
                &[("id", id), ("time", &time_ms.to_string()), ("submission", if submission { "true" } else { "false" })],
            )
            .await?;
        Ok(())
    }

    /// One page of songs from `search3`. With an empty `query` Navidrome
    /// returns the whole library, `count` at a time from `offset`.
    pub async fn search_songs(&self, query: &str, count: u32, offset: u32) -> Result<Vec<Song>> {
        let (_, p): (_, SearchResult3Payload) = self
            .call(
                "search3",
                &[
                    ("query", query),
                    ("artistCount", "0"),
                    ("albumCount", "0"),
                    ("songCount", &count.to_string()),
                    ("songOffset", &offset.to_string()),
                ],
            )
            .await?;
        Ok(p.search_result3.song)
    }

    /// Everything the user starred.
    pub async fn starred(&self) -> Result<Starred2> {
        let (_, p): (_, Starred2Payload) = self.call("getStarred2", &[]).await?;
        Ok(p.starred2)
    }

    /// Stars (`on`) or unstars a song, album or artist.
    pub async fn set_starred(&self, kind: StarKind, id: &str, on: bool) -> Result<()> {
        let (_, _): (_, serde_json::Value) = self.call(if on { "star" } else { "unstar" }, &[(kind.param(), id)]).await?;
        Ok(())
    }

    /// Sets the user's rating of a song: 1–5, or 0 to clear it.
    pub async fn set_rating(&self, id: &str, rating: u8) -> Result<()> {
        let (_, _): (_, serde_json::Value) = self.call("setRating", &[("id", id), ("rating", &rating.min(5).to_string())]).await?;
        Ok(())
    }

    /// Creates a playlist holding `song_ids`, in order.
    pub async fn create_playlist(&self, name: &str, song_ids: &[String]) -> Result<PlaylistWithSongs> {
        let mut params = vec![("name", name)];
        params.extend(song_ids.iter().map(|id| ("songId", id.as_str())));
        let (_, p): (_, PlaylistPayload) = self.call("createPlaylist", &params).await?;
        Ok(p.playlist)
    }

    /// Replaces a playlist's songs with `song_ids`, in order. This is how a
    /// reorder is saved: `updatePlaylist` can only append and remove.
    pub async fn replace_playlist(&self, id: &str, song_ids: &[String]) -> Result<()> {
        let mut params = vec![("playlistId", id)];
        params.extend(song_ids.iter().map(|id| ("songId", id.as_str())));
        let (_, _): (_, serde_json::Value) = self.call("createPlaylist", &params).await?;
        Ok(())
    }

    /// Renames a playlist and/or appends songs and/or removes the songs at
    /// `remove_at` (indices into the playlist as it was before the call).
    pub async fn update_playlist(&self, id: &str, name: Option<&str>, add: &[String], remove_at: &[usize]) -> Result<()> {
        let remove: Vec<String> = remove_at.iter().map(usize::to_string).collect();
        let mut params = vec![("playlistId", id)];
        if let Some(n) = name {
            params.push(("name", n));
        }
        params.extend(add.iter().map(|s| ("songIdToAdd", s.as_str())));
        params.extend(remove.iter().map(|s| ("songIndexToRemove", s.as_str())));
        let (_, _): (_, serde_json::Value) = self.call("updatePlaylist", &params).await?;
        Ok(())
    }

    pub async fn delete_playlist(&self, id: &str) -> Result<()> {
        let (_, _): (_, serde_json::Value) = self.call("deletePlaylist", &[("id", id)]).await?;
        Ok(())
    }

    pub async fn artists(&self) -> Result<Vec<Artist>> {
        let (_, p): (_, ArtistsPayload) = self.call("getArtists", &[]).await?;
        Ok(p.artists.index.into_iter().flat_map(|i| i.artist).collect())
    }
}

/// Whether a response is a Subsonic error envelope (JSON or XML) rather than
/// the file asked for. Subsonic sends those with status 200, so a download
/// must check before keeping the bytes. `head` is the start of the body.
pub fn is_error_reply(content_type: Option<&str>, head: &[u8]) -> bool {
    if content_type.is_some_and(|ct| ct.contains("json") || ct.contains("xml")) {
        return true;
    }
    let text = String::from_utf8_lossy(&head[..head.len().min(512)]);
    let text = text.trim_start();
    (text.starts_with('{') && text.contains("subsonic-response")) || text.starts_with("<?xml") || text.starts_with("<subsonic-response")
}

/// A short, readable form of an error envelope for messages: the server's
/// `message` when there is one, else the start of the body.
pub fn error_text(body: &[u8]) -> String {
    if let Err(Error::Api { message, .. }) = parse_response::<serde_json::Value>(body) {
        return message;
    }
    let text = String::from_utf8_lossy(&body[..body.len().min(200)]);
    match text.split_once("message=\"").and_then(|(_, rest)| rest.split_once('"')) {
        Some((message, _)) => message.to_owned(),
        None => text.trim().to_owned(),
    }
}

/// Unwraps the `subsonic-response` envelope, surfacing API-level failures as [`Error::Api`].
pub fn parse_response<T: DeserializeOwned>(bytes: &[u8]) -> Result<(ServerInfo, T)> {
    let env: Envelope = serde_json::from_slice(bytes)?;
    let body = env.response;
    if body.status != "ok" {
        let err = body.error.unwrap_or(ApiError { code: -1, message: "unknown error".into() });
        return Err(Error::Api { code: err.code, message: err.message });
    }
    let payload = serde_json::from_value(serde_json::Value::Object(body.rest))?;
    Ok((body.info, payload))
}

#[cfg(test)]
mod tests;
