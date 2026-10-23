//! Minimal OpenSubsonic client for Navidrome.
//!
//! Auth uses the token + salt scheme (`t = md5(password + salt)`), with a fresh
//! salt per request. Every response is requested as JSON and unwrapped from the
//! `subsonic-response` envelope before the payload is decoded.

mod auth;
mod model;

pub use auth::{Credentials, Token};
pub use model::*;

use serde::de::DeserializeOwned;
use url::Url;

/// Protocol version we advertise. Navidrome speaks 1.16.1 plus OpenSubsonic extensions.
pub const API_VERSION: &str = "1.16.1";
pub const CLIENT_NAME: &str = "listener";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("invalid server url: {0}")]
    Url(#[from] url::ParseError),
    #[error("malformed response: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("server error {code}: {message}")]
    Api { code: i32, message: String },
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone)]
pub struct Client {
    base: Url,
    creds: Credentials,
    http: reqwest::Client,
}

impl Client {
    pub fn new(server: &str, creds: Credentials) -> Result<Self> {
        let mut base = Url::parse(server)?;
        // Url::join replaces the last path segment unless the base ends in '/'.
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let http = reqwest::Client::builder()
            .user_agent(concat!("listener/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self { base, creds, http })
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

    pub fn cover_art_url(&self, cover_id: &str, size: Option<u32>) -> Url {
        match size {
            Some(px) => self.endpoint("getCoverArt", &[("id", cover_id), ("size", &px.to_string())]),
            None => self.endpoint("getCoverArt", &[("id", cover_id)]),
        }
    }

    async fn call<T: DeserializeOwned>(&self, method: &str, params: &[(&str, &str)]) -> Result<(ServerInfo, T)> {
        let bytes = self
            .http
            .get(self.endpoint(method, params))
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
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

    /// Cover art bytes as the server sends them (usually JPEG), scaled server-side to `size` px.
    pub async fn cover_art(&self, cover_id: &str, size: Option<u32>) -> Result<Vec<u8>> {
        let resp = self.http.get(self.cover_art_url(cover_id, size)).send().await?.error_for_status()?;
        // Errors come back as a JSON envelope rather than an image.
        let is_json = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.contains("json"));
        let bytes = resp.bytes().await?;
        if is_json {
            parse_response::<serde_json::Value>(&bytes)?;
        }
        Ok(bytes.to_vec())
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

    pub async fn artists(&self) -> Result<Vec<Artist>> {
        let (_, p): (_, ArtistsPayload) = self.call("getArtists", &[]).await?;
        Ok(p.artists.index.into_iter().flat_map(|i| i.artist).collect())
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
