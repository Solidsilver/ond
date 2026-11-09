//! A typed client for the Audiobookshelf API, for Rede.
//!
//! The shapes it speaks are in `docs/rede/abs-api.md`, checked against the
//! server source rather than the official docs, which are no longer
//! maintained.
//!
//! **Tokens.** Signing in (`POST /login` with `x-return-tokens: true`) gives
//! an access token, good for an hour by default, and a refresh token, good
//! for 30 days. The legacy `user.token` in the same reply never expires; it's
//! neither read nor kept. Requests carry the access token as a Bearer header.
//! A 401 means it expired: the client refreshes once and retries once. The
//! server rotates the refresh token on every refresh and counts refreshes
//! against the same 40-per-10-minutes limit as sign-ins, so refreshes are
//! single-flight per client: concurrent requests refused at the same time
//! share one refresh and its outcome (the `auth` module explains how). Every new pair
//! goes to the app's [`AuthHook`] before it's used, so the keyring always
//! holds a refresh token the server still honours. A refresh token the server
//! turns down is [`AuthError::SignedOut`]: only the user can fix that.
//!
//! **Files.** A play session's tracks carry a relative `contentUrl` with no
//! token. The stream cache and the offline store take a URL and nothing else,
//! so [`Client::file_url`] makes it absolute and puts the access token in
//! `?token=`, which every `/api` route accepts. Such a URL is as good as the
//! account for an hour: it never goes in a log or an error message, and
//! reqwest's errors are stripped of their URL before they're kept. Covers need
//! no token at all.
//!
//! A server address may carry a reverse-proxy path prefix
//! (`https://example.com/abs`); every path is joined onto it.

mod auth;
mod error;
mod lenient;
mod model;
mod openid;
pub use openid::{OpenId, OpenIdError};

pub use auth::{AuthEvent, AuthHook, Tokens};
pub use error::{AuthError, Error, Result};
pub use model::*;

use std::sync::Arc;
use std::time::Duration;

use reqwest::{Method, StatusCode, header};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use url::Url;

use auth::AuthState;

/// Give up connecting after this long.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Give up on a response that stops sending for this long.
pub const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Give up on an API call that hasn't finished in this long, even if it's
/// still trickling in.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// What this client tells the server it plays with, in `mediaPlayer`.
const MEDIA_PLAYER: &str = "rede";

/// A signed-in (or not yet signed-in) connection to one server. Cheap to
/// clone; clones share the tokens and the refresh lock.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

struct Inner {
    /// Ends in '/', so paths join onto it rather than replacing its last
    /// segment.
    base: Url,
    device: DeviceInfo,
    http: reqwest::Client,
    call_timeout: Duration,
    auth: AuthState,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("base", &self.inner.base.as_str())
            .field("device", &self.inner.device)
            .field("signed_in", &self.inner.auth.tokens().is_some())
            .finish_non_exhaustive()
    }
}

/// What a sign-in returns.
#[derive(Debug, Clone)]
pub struct Login {
    pub tokens: Tokens,
    pub user: User,
    pub default_library_id: Option<String>,
    /// The server's version, from its settings.
    pub server_version: Option<String>,
}

/// The sign-in and refresh reply. The tokens sit beside the user's own
/// fields; `user.token`, the legacy one, isn't read.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginReply {
    user: LoginUser,
    user_default_library_id: Option<String>,
    #[serde(default, deserialize_with = "lenient::opt")]
    server_settings: Option<ServerSettings>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginUser {
    access_token: Option<String>,
    refresh_token: Option<String>,
    #[serde(flatten)]
    user: User,
}

#[derive(Deserialize)]
struct ServerSettings {
    version: Option<String>,
}

impl LoginReply {
    fn tokens(&self) -> Result<Tokens> {
        match (&self.user.access_token, &self.user.refresh_token) {
            (Some(a), Some(r)) if !a.is_empty() && !r.is_empty() => Ok(Tokens::new(a, r)),
            _ => Err(Error::Decode("the server sent no access and refresh tokens; it may be older than 2.26".into())),
        }
    }
}

/// How a request is authenticated, and what a 401 means for it.
#[derive(Clone, Copy, PartialEq)]
enum Auth {
    /// Bearer, refreshing and retrying once on a 401.
    Bearer,
    /// None needed.
    Open,
}

impl Client {
    /// A client for the server at `server` (with any reverse-proxy path), not
    /// yet signed in.
    pub fn new(server: &str, device: DeviceInfo) -> Result<Self> {
        let mut base = Url::parse(server.trim())?;
        // "abs.example.com:13378" parses, with "abs.example.com" as its
        // scheme, but no path can be joined onto it.
        if !matches!(base.scheme(), "http" | "https") || base.cannot_be_a_base() {
            return Err(Error::Url(url::ParseError::RelativeUrlWithoutBase));
        }
        base.set_query(None);
        base.set_fragment(None);
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let http = reqwest::Client::builder()
            .user_agent(format!("{}/{}", device.client_name, device.client_version))
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()?;
        Ok(Self { inner: Arc::new(Inner { base, device, http, call_timeout: CALL_TIMEOUT, auth: AuthState::new(None) }) })
    }

    /// Starts from tokens the app kept from an earlier sign-in. Doesn't
    /// announce them to the hook: they're already stored.
    pub fn with_tokens(self, tokens: Tokens) -> Self {
        self.inner.auth.restore(tokens);
        self
    }

    /// Replaces [`CALL_TIMEOUT`]. Only before the client is cloned or used.
    pub fn with_call_timeout(mut self, timeout: Duration) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.call_timeout = timeout;
        }
        self
    }

    /// Sets the hook told about every new token pair and every sign-out.
    pub fn set_auth_hook(&self, hook: impl Fn(AuthEvent) + Send + Sync + 'static) {
        self.inner.auth.set_hook(Some(Arc::new(hook)));
    }

    /// The server address, ending in '/'.
    pub fn base(&self) -> &Url {
        &self.inner.base
    }

    pub fn device(&self) -> &DeviceInfo {
        &self.inner.device
    }

    /// The current tokens, if signed in.
    pub fn tokens(&self) -> Option<Tokens> {
        self.inner.auth.tokens()
    }

    pub fn is_signed_in(&self) -> bool {
        self.inner.auth.tokens().is_some()
    }

    /// `path` (like `"api/items"`, no leading '/') joined onto the server
    /// address, with `ids` appended as path segments, each percent-encoded.
    fn url(&self, path: &'static str, ids: &[&str]) -> Url {
        let mut url = self.inner.base.join(path).expect("a fixed path joins onto a base URL");
        if !ids.is_empty() {
            url.path_segments_mut().expect("the base URL takes a path").pop_if_empty().extend(ids);
        }
        url
    }

    // --- Signing in and out ---------------------------------------------

    /// `GET /status`: the server's version and sign-in methods. Needs no
    /// sign-in, so it also tells whether an address is an Audiobookshelf
    /// server at all.
    pub async fn status(&self) -> Result<ServerStatus> {
        let bytes = self.send(Method::GET, self.url("status", &[]), None, Auth::Open).await?;
        error::decode(&bytes)
    }

    /// Signs in with a username and password, and keeps the tokens. The
    /// password goes to the server and nowhere else.
    pub async fn login(&self, username: &str, password: &str) -> Result<Login> {
        let body = serde_json::json!({ "username": username, "password": password });
        let resp = self.inner.http.post(self.url("login", &[])).header("x-return-tokens", "true").json(&body).timeout(self.inner.call_timeout).send().await?;
        let status = resp.status();
        let bytes = match status {
            StatusCode::UNAUTHORIZED => return Err(AuthError::BadCredentials.into()),
            _ => checked(resp).await?,
        };
        let reply: LoginReply = error::decode_secret(&bytes)?;
        let tokens = reply.tokens()?;
        self.inner.auth.replace(Some(tokens.clone())).await;
        tracing::debug!("signed in to Audiobookshelf");
        Ok(Login {
            tokens,
            user: reply.user.user,
            default_library_id: reply.user_default_library_id,
            server_version: reply.server_settings.and_then(|s| s.version),
        })
    }

    /// Renews the tokens now, sharing a refresh already in flight. Requests
    /// do this on their own when the access token is refused; call it to
    /// refresh ahead of a long stream.
    pub async fn refresh(&self) -> Result<Tokens> {
        let ticket = self.inner.auth.ticket()?;
        self.refreshed(ticket.generation).await
    }

    async fn refreshed(&self, generation: u64) -> Result<Tokens> {
        self.inner.auth.refreshed(generation, |held| self.post_refresh(held)).await
    }

    /// `POST /auth/refresh` with the refresh token in `x-refresh-token`.
    async fn post_refresh(&self, held: Tokens) -> Result<Tokens> {
        tracing::debug!("refreshing the Audiobookshelf session");
        let resp =
            self.inner.http.post(self.url("auth/refresh", &[])).header("x-refresh-token", held.refresh()).timeout(self.inner.call_timeout).send().await?;
        if resp.status() == StatusCode::UNAUTHORIZED {
            // Expired, revoked, or rotated more than 10 minutes ago.
            let said = error::status_text(&resp.bytes().await.unwrap_or_default());
            tracing::info!("the server turned down the refresh token ({}); signed out", said.as_deref().unwrap_or("no reason given"));
            return Err(AuthError::SignedOut.into());
        }
        let bytes = checked(resp).await?;
        error::decode_secret::<LoginReply>(&bytes)?.tokens()
    }

    /// Signs out: ends this session on the server and forgets the tokens.
    /// The tokens are forgotten even if the server can't be reached.
    pub async fn logout(&self) -> Result<()> {
        let held = self.inner.auth.tokens();
        self.inner.auth.replace(None).await;
        let Some(held) = held else { return Ok(()) };
        let resp = self.inner.http.post(self.url("logout", &[])).header("x-refresh-token", held.refresh()).timeout(self.inner.call_timeout).send().await?;
        checked(resp).await.map(drop)
    }

    // --- Requests ---------------------------------------------------------

    /// Sends a request and returns the body of a success. With
    /// [`Auth::Bearer`], a 401 refreshes the tokens (or waits for the refresh
    /// in flight) and sends the request once more.
    async fn send(&self, method: Method, url: Url, body: Option<Vec<u8>>, auth: Auth) -> Result<Vec<u8>> {
        let mut ticket = match auth {
            Auth::Bearer => Some(self.inner.auth.ticket()?),
            Auth::Open => None,
        };
        let mut retried = false;
        loop {
            let mut req = self.inner.http.request(method.clone(), url.clone()).timeout(self.inner.call_timeout);
            if let Some(t) = &ticket {
                req = req.bearer_auth(t.tokens.access());
            }
            if let Some(body) = &body {
                req = req.header(header::CONTENT_TYPE, "application/json").body(body.clone());
            }
            let resp = req.send().await?;
            tracing::debug!("{method} {} -> {}", url.path(), resp.status().as_u16());
            if resp.status() == StatusCode::UNAUTHORIZED
                && let Some(t) = &ticket
                && !retried
            {
                retried = true;
                let generation = t.generation;
                self.refreshed(generation).await?;
                ticket = Some(self.inner.auth.ticket()?);
                continue;
            }
            return checked(resp).await;
        }
    }

    async fn get<T: DeserializeOwned>(&self, url: Url) -> Result<T> {
        error::decode(&self.send(Method::GET, url, None, Auth::Bearer).await?)
    }

    /// Sends `body` as JSON and returns the reply's body.
    async fn send_json(&self, method: Method, url: Url, body: &impl serde::Serialize) -> Result<Vec<u8>> {
        let body = serde_json::to_vec(body).map_err(|e| Error::Decode(e.to_string()))?;
        self.send(method, url, Some(body), Auth::Bearer).await
    }

    // --- Libraries and items --------------------------------------------

    /// `GET /api/libraries`: the libraries this user can see.
    pub async fn libraries(&self) -> Result<Vec<Library>> {
        Ok(self.get::<model::LibrariesReply>(self.url("api/libraries", &[])).await?.libraries)
    }

    /// One page of a library's items, oldest added first so pages don't
    /// overlap. `page` is 0-based; a `limit` of 0 returns everything at once.
    /// Items come minified: counts but no tracks or chapters.
    pub async fn library_items(&self, library_id: &str, page: u32, limit: u32) -> Result<ItemsPage> {
        let mut url = self.url("api/libraries", &[library_id, "items"]);
        url.query_pairs_mut().append_pair("sort", "addedAt").append_pair("limit", &limit.to_string()).append_pair("page", &page.to_string());
        self.get(url).await
    }

    /// `GET /api/items/:id?expanded=1&include=progress`: one item with its
    /// tracks, chapters, audio files and the user's progress.
    pub async fn item(&self, item_id: &str) -> Result<LibraryItem> {
        let mut url = self.url("api/items", &[item_id]);
        url.query_pairs_mut().append_pair("expanded", "1").append_pair("include", "progress");
        self.get(url).await
    }

    /// The cover of an item, `width` pixels wide (the server's default is
    /// 400). Covers need no token.
    pub fn cover_url(&self, item_id: &str, width: Option<u32>) -> Url {
        let mut url = self.url("api/items", &[item_id, "cover"]);
        if let Some(w) = width {
            url.query_pairs_mut().append_pair("width", &w.to_string());
        }
        url
    }

    /// `GET /api/items/:id/cover`: the cover image itself. Covers need no
    /// token, so this works on a client that isn't signed in.
    pub async fn cover(&self, item_id: &str, width: Option<u32>) -> Result<Vec<u8>> {
        self.send(Method::GET, self.cover_url(item_id, width), None, Auth::Open).await
    }

    /// An absolute URL for a track's relative `contentUrl`, with the current
    /// access token in `?token=`, for the stream cache and the offline store.
    ///
    /// The URL is a secret: don't log it. It stops working when the access
    /// token expires (an hour by default); after a 401, [`refresh`] and build
    /// it again.
    ///
    /// [`refresh`]: Self::refresh
    pub fn file_url(&self, content_url: &str) -> Result<Url> {
        let tokens = self.inner.auth.tokens().ok_or(AuthError::SignedOut)?;
        let mut url = self.resolve(content_url)?;
        url.query_pairs_mut().append_pair("token", tokens.access());
        Ok(url)
    }

    /// `content_url` relative to the server address. The server sends it
    /// without the reverse-proxy prefix; one that already has it isn't given
    /// it twice.
    fn resolve(&self, content_url: &str) -> Result<Url> {
        if content_url.contains("://") {
            // Only relative paths come from the server; an absolute URL here
            // could send the token to another host.
            return Err(Error::Decode("a track's contentUrl wasn't a relative path".into()));
        }
        let base = &self.inner.base;
        let path = content_url.trim_start_matches('/');
        let prefix = base.path().trim_start_matches('/');
        let path = match path.strip_prefix(prefix) {
            Some(rest) if !prefix.is_empty() => rest,
            _ => path,
        };
        Ok(base.join(path)?)
    }

    // --- Playing ----------------------------------------------------------

    /// `POST /api/items/:id/play`: opens a play session, which closes any
    /// other open session for this user on this device. Always asks for
    /// direct play: the files themselves, not an HLS transcode.
    /// `supported_mime_types` is what the player decodes; `episode_id`
    /// picks a podcast episode.
    pub async fn play(&self, item_id: &str, episode_id: Option<&str>, supported_mime_types: &[String]) -> Result<PlaybackSession> {
        let url = match episode_id {
            Some(ep) => self.url("api/items", &[item_id, "play", ep]),
            None => self.url("api/items", &[item_id, "play"]),
        };
        let body = model::PlayRequest {
            device_info: &self.inner.device,
            supported_mime_types,
            force_direct_play: true,
            force_transcode: false,
            media_player: MEDIA_PLAYER,
        };
        error::decode(&self.send_json(Method::POST, url, &body).await?)
    }

    /// `POST /api/session/:id/sync`: the position now and the time listened
    /// since the last sync. Writes the user's progress as it is, without
    /// comparing times. A 404 means the session is gone (the server
    /// restarted): open a new one.
    pub async fn sync_session(&self, session_id: &str, sync: &SessionSync) -> Result<()> {
        self.send_json(Method::POST, self.url("api/session", &[session_id, "sync"]), sync).await.map(drop)
    }

    /// `POST /api/session/:id/close`, with a last sync if there is one.
    pub async fn close_session(&self, session_id: &str, sync: Option<&SessionSync>) -> Result<()> {
        let url = self.url("api/session", &[session_id, "close"]);
        match sync {
            Some(s) => self.send_json(Method::POST, url, s).await,
            None => self.send_json(Method::POST, url, &serde_json::json!({})).await,
        }
        .map(drop)
    }

    /// `POST /api/session/local`: listening done offline. The server keeps
    /// its own progress if that changed after `session.updated_at`.
    pub async fn sync_local_session(&self, session: &LocalSession) -> Result<()> {
        let body = self.local_body(session);
        self.send_json(Method::POST, self.url("api/session/local", &[]), &body).await.map(drop)
    }

    /// `POST /api/session/local-all`: several offline sessions at once, with
    /// what became of each.
    pub async fn sync_local_sessions(&self, sessions: &[LocalSession]) -> Result<Vec<LocalSyncResult>> {
        let body = model::LocalAllBody { device_info: &self.inner.device, sessions: sessions.iter().map(|s| self.local_body(s)).collect() };
        let reply: model::LocalAllReply = error::decode(&self.send_json(Method::POST, self.url("api/session/local-all", &[]), &body).await?)?;
        Ok(reply.results)
    }

    fn local_body<'a>(&'a self, session: &'a LocalSession) -> model::LocalSessionBody<'a> {
        model::LocalSessionBody {
            session,
            device_info: &self.inner.device,
            media_type: if session.episode_id.is_some() { MediaType::Podcast } else { MediaType::Book },
            play_method: 3,
            media_player: MEDIA_PLAYER,
        }
    }

    // --- Progress and bookmarks -------------------------------------------

    /// `GET /api/me`: the user, with all their progress and bookmarks.
    pub async fn me(&self) -> Result<User> {
        self.get(self.url("api/me", &[])).await
    }

    /// Every progress row the user has, for every item and episode. Read
    /// from `/api/me`, which every server version has.
    pub async fn progress(&self) -> Result<Vec<MediaProgress>> {
        Ok(self.me().await?.media_progress)
    }

    /// The user's progress in one item (or episode), or `None` if there is
    /// none.
    pub async fn item_progress(&self, item_id: &str, episode_id: Option<&str>) -> Result<Option<MediaProgress>> {
        let url = match episode_id {
            Some(ep) => self.url("api/me/progress", &[item_id, ep]),
            None => self.url("api/me/progress", &[item_id]),
        };
        match self.get(url).await {
            Ok(p) => Ok(Some(p)),
            Err(e) if e.is_not_found() => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `GET /api/me/items-in-progress`: items started and not finished, most
    /// recent first, across all libraries.
    pub async fn items_in_progress(&self, limit: u32) -> Result<Vec<LibraryItem>> {
        let mut url = self.url("api/me/items-in-progress", &[]);
        url.query_pairs_mut().append_pair("limit", &limit.to_string());
        Ok(self.get::<model::ItemsInProgressReply>(url).await?.library_items)
    }

    /// `PATCH /api/me/progress/:id`: overwrites the user's progress in one
    /// item. See [`ProgressUpdate`] for the server's rules.
    pub async fn update_progress(&self, item_id: &str, episode_id: Option<&str>, update: &ProgressUpdate) -> Result<()> {
        let url = match episode_id {
            Some(ep) => self.url("api/me/progress", &[item_id, ep]),
            None => self.url("api/me/progress", &[item_id]),
        };
        self.send_json(Method::PATCH, url, update).await.map(drop)
    }

    /// `PATCH /api/me/progress/batch/update`. The server answers 200 even
    /// when some entries fail, so read the progress back if it matters.
    /// Nothing is sent for an empty batch.
    pub async fn update_progress_batch(&self, updates: &[ItemProgressUpdate]) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        self.send_json(Method::PATCH, self.url("api/me/progress/batch/update", &[]), &updates).await.map(drop)
    }

    /// `DELETE /api/me/progress/:id`, by the progress row's own id
    /// ([`MediaProgress::id`]), not the item's.
    pub async fn delete_progress(&self, progress_id: &str) -> Result<()> {
        self.send(Method::DELETE, self.url("api/me/progress", &[progress_id]), None, Auth::Bearer).await.map(drop)
    }

    /// Every bookmark the user has. Read from `/api/me`, which every server
    /// version has.
    pub async fn bookmarks(&self) -> Result<Vec<Bookmark>> {
        Ok(self.me().await?.bookmarks)
    }

    /// Adds a bookmark at `time` seconds, or retitles the one already there.
    pub async fn create_bookmark(&self, item_id: &str, time: f64, title: &str) -> Result<Bookmark> {
        let body = model::BookmarkBody { time, title };
        error::decode(&self.send_json(Method::POST, self.url("api/me/item", &[item_id, "bookmark"]), &body).await?)
    }

    /// Retitles the bookmark at exactly `time`. A bookmark's time can't
    /// change: delete it and create another.
    pub async fn rename_bookmark(&self, item_id: &str, time: f64, title: &str) -> Result<Bookmark> {
        let body = model::BookmarkBody { time, title };
        error::decode(&self.send_json(Method::PATCH, self.url("api/me/item", &[item_id, "bookmark"]), &body).await?)
    }

    /// Deletes the bookmark at exactly `time` (as the server returned it).
    pub async fn delete_bookmark(&self, item_id: &str, time: f64) -> Result<()> {
        // Rust writes the shortest form that reads back as the same f64, as
        // JavaScript does, so the server's Number() gets the stored value.
        let time = time.to_string();
        self.send(Method::DELETE, self.url("api/me/item", &[item_id, "bookmark", &time]), None, Auth::Bearer).await.map(drop)
    }
}

/// The body of a success, or the error the status means.
async fn checked(resp: reqwest::Response) -> Result<Vec<u8>> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp.bytes().await?.to_vec());
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err(AuthError::RateLimited { retry_after: retry_after(resp.headers()) }.into());
    }
    let body = resp.bytes().await.unwrap_or_default();
    Err(Error::Server { status: status.as_u16(), message: error::status_text(&body) })
}

/// `Retry-After`, or the rate limiter's `RateLimit-Reset`, both in seconds.
fn retry_after(headers: &header::HeaderMap) -> Option<Duration> {
    [header::RETRY_AFTER.as_str(), "ratelimit-reset"].iter().find_map(|h| headers.get(*h)?.to_str().ok()?.trim().parse::<u64>().ok()).map(Duration::from_secs)
}

#[cfg(test)]
mod tests;
