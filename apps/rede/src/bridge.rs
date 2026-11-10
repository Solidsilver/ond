//! Background work for the UI. Every function here returns immediately; results
//! arrive later as [`Msg`]s, each followed by a repaint request.
//!
//! A result belongs to the session it was started in. Signing out (or signing
//! in again) bumps [`Bridge::session`], and every copy of the bridge made with
//! [`scoped`](Bridge::scoped) for an older session sends nothing more and writes
//! nothing more, so a listing that was still running for the last account can't
//! land in the next one's shelf.

use crate::failure::Failure;
use crate::settings::{Account, KnownLibrary, Paths, Settings};
use crate::sync::{self, Outcome};
use crate::tokens::Keeper;
use abs::{AuthError, Version};
use shelf::Shelf;
use sources::{AbsSource, Item, ItemKey, ItemSummary, Source};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;

/// What background work tells the UI. Messages for one part of the app are
/// grouped under it ([`AccountMsg`], [`LibraryMsg`]), and that part's handler
/// matches its own group whole, so a message added to a group can't be left
/// unhandled.
pub enum Msg {
    Account(AccountMsg),
    Library(LibraryMsg),
    Bookmarks {
        session: u64,
        event: BookmarkMsg,
    },
    Playback {
        session: u64,
        event: crate::listening::Event,
    },
    Engine {
        session: u64,
        event: audio::Event,
    },
    /// The engine clock wakes logic even when no UI is painted.
    PlaybackTick {
        session: u64,
        report: bool,
    },
    #[cfg(target_os = "macos")]
    Remote(shell::now_playing::Remote),
    #[cfg(target_os = "macos")]
    Artwork {
        session: u64,
        id: String,
        bytes: Vec<u8>,
    },
    /// Something the user asked for failed.
    Error(Failure),
}

/// Signing in, and the connection it made.
pub enum AccountMsg {
    /// Signed in (or resumed from saved tokens): the account, its source and
    /// its shelf. `warning` is something to say about the server itself.
    SignedIn {
        /// The bridge's session when the sign-in began; the app drops the
        /// message if the user signed out since.
        session: u64,
        account: Account,
        source: Arc<dyn Source>,
        shelf: Arc<Shelf>,
        warning: Option<String>,
    },
    /// The sign-in didn't work, and `error` says why in plain words.
    SignInFailed {
        session: u64,
        error: String,
    },
    /// The account is on file but its tokens aren't: sign in again.
    NoTokens {
        session: u64,
    },
    Status {
        session: u64,
        status: abs::ServerStatus,
        warning: Option<String>,
    },
    OpenIdReady {
        session: u64,
        pending: Box<crate::openid::Pending>,
    },
    OpenIdRestored {
        session: u64,
        pending: Option<Box<crate::openid::Pending>>,
    },
    #[cfg(any(target_os = "macos", test))]
    Redirect(String),
    OpenIdAuthenticated {
        session: u64,
        account: Account,
        client: abs::Client,
        login: Box<abs::Login>,
        warning: Option<String>,
    },
}

/// The library: the shelf's books, the sync that filled it, and a book's
/// details.
pub enum LibraryMsg {
    /// The shelf's books, sorted by title.
    Books(Vec<ItemSummary>),
    /// How many books the running sync has written so far.
    SyncProgress(usize),
    /// How the sync ended. `Done` carries the libraries it listed, which the
    /// account keeps for the next start.
    SyncDone(Outcome),
    /// A book's details (tracks, chapters, description), stored on the way.
    Book { key: ItemKey, item: Box<Item> },
    /// A book's details couldn't be loaded. The page still shows what the
    /// listing had.
    BookFailed { key: ItemKey, error: Failure },
}

/// Local optimistic snapshots and the subsequent server result use one group.
pub enum BookmarkMsg {
    Updated(shelf::BookmarkSnapshot),
    Failed(Failure),
}

impl From<AccountMsg> for Msg {
    fn from(m: AccountMsg) -> Self {
        Msg::Account(m)
    }
}

impl From<LibraryMsg> for Msg {
    fn from(m: LibraryMsg) -> Self {
        Msg::Library(m)
    }
}

/// What the player decodes, sent with every play session: without it, or
/// without `forceDirectPlay`, Audiobookshelf transcodes to HLS instead of
/// handing over the files. The engine decodes these formats directly.
const MIME_TYPES: [&str; 7] = ["audio/mpeg", "audio/mp4", "audio/aac", "audio/flac", "audio/ogg", "audio/webm", "audio/x-matroska"];

/// Wakes the UI so it reads what was sent (a repaint, in the app).
pub type Notify = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub struct Bridge {
    rt: tokio::runtime::Handle,
    openid_store: Arc<std::sync::Mutex<OpenIdSlot>>,
    tx: Sender<Msg>,
    notify: Notify,
    /// Persists the tokens the client rotates, off the task that rotated them.
    keeper: Keeper,
    /// Where each account's shelf lives.
    paths: Arc<Paths>,
    /// Bumped on sign-out: work for an older session stops and its results are
    /// dropped.
    session: Arc<AtomicU64>,
    /// Bumped by each sync: an older pass still running (from before a
    /// sign-in, say) stops storing once a newer one starts.
    pass: Arc<AtomicU64>,
    /// Set on a copy made by [`scoped`](Self::scoped): the session its work
    /// belongs to. Once that session ends, the copy's messages are dropped.
    scope: Option<u64>,
}

/// Serializes blocking keyring operations. A delayed cleanup may remove an
/// older attempt, but never one saved by a newer login session.
struct OpenIdSlot {
    generation: u64,
    store: Box<dyn crate::openid::Store>,
}

impl Bridge {
    pub fn new(rt: tokio::runtime::Handle, tx: Sender<Msg>, notify: Notify, keeper: Keeper, paths: Arc<Paths>) -> Self {
        Self {
            rt,
            openid_store: Arc::new(std::sync::Mutex::new(OpenIdSlot { generation: 0, store: Box::new(crate::openid::Keyring) })),
            tx,
            notify,
            keeper,
            paths,
            session: Arc::default(),
            pass: Arc::default(),
            scope: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn use_openid_store(&mut self, store: impl crate::openid::Store + 'static) {
        self.openid_store = Arc::new(std::sync::Mutex::new(OpenIdSlot { generation: 0, store: Box::new(store) }));
    }

    fn playback(&self, event: crate::listening::Event) {
        let session = self.scope.unwrap_or_else(|| self.session());
        if let crate::listening::Event::Bookmarks(result) = event {
            let event = match result {
                Ok(bookmarks) => BookmarkMsg::Updated(bookmarks),
                Err(e) => BookmarkMsg::Failed(e),
            };
            self.send(Msg::Bookmarks { session, event });
        } else {
            self.send(Msg::Playback { session, event });
        }
    }

    pub fn listening(&self, source: Arc<dyn Source>, shelf: Arc<Shelf>, device: sources::DeviceInfo) -> crate::listening::Listening {
        let this = self.scoped();
        let live = this.clone();
        crate::listening::spawn(&self.rt, source, shelf, device, move || live.current(), move |event| this.playback(event))
    }

    #[cfg(target_os = "macos")]
    pub fn media_keys(&self) {
        let tx = self.tx.clone();
        let notify = self.notify.clone();
        shell::now_playing::install(
            Arc::new(move |command| {
                let _ = tx.send(Msg::Remote(command));
                notify();
            }),
            true,
        );
    }

    #[cfg(target_os = "macos")]
    pub fn artwork(&self, source: Arc<dyn Source>, id: String, cover: String) {
        let this = self.scoped();
        self.rt.spawn(async move {
            if let Ok(bytes) = source.cover(&cover, Some(512)).await {
                this.send(Msg::Artwork { session: this.scope.expect("artwork is scoped"), id, bytes: bytes.to_vec() });
            }
        });
    }

    pub fn engine_events(&self, engine: &audio::EngineHandle, ctx: egui::Context, timer: Arc<std::sync::atomic::AtomicBool>) {
        let this = self.scoped();
        let events = engine.events.clone();
        let status = engine.status.clone();
        self.rt.spawn_blocking(move || {
            let mut clock = crate::player::ReportClock::new(std::time::Instant::now());
            while this.current() {
                let state = status.state();
                if let Some(report) = clock.poll_for(state, std::time::Instant::now(), timer.load(Ordering::Relaxed))
                    && this.current()
                {
                    let _ = this.tx.send(Msg::PlaybackTick { session: this.scope.expect("engine events are scoped"), report });
                    // Coalesce with the visible bar's existing repaint deadline;
                    // hidden windows need only these clock and engine wakeups.
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
                match events.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(event) => this.send(Msg::Engine { session: this.scope.unwrap_or_else(|| this.session()), event }),
                    Err(error) if error.is_disconnected() => break,
                    Err(_) => {}
                }
            }
        });
    }

    /// The current login session (see [`end_session`](Self::end_session)).
    pub fn session(&self) -> u64 {
        self.session.load(Ordering::Acquire)
    }

    /// Signing out: sign-ins, syncs and fetches in flight give up, and anything
    /// they still deliver is recognised as stale.
    pub fn end_session(&self) {
        self.session.fetch_add(1, Ordering::AcqRel);
    }

    fn live(&self, session: u64) -> bool {
        self.session() == session
    }

    /// A copy for work on the current session's behalf (see `scope`).
    fn scoped(&self) -> Self {
        self.scoped_to(self.scope.unwrap_or_else(|| self.session()))
    }

    /// A copy for work on `session`'s behalf, which may have ended already.
    fn scoped_to(&self, session: u64) -> Self {
        Self { scope: Some(session), ..self.clone() }
    }

    /// False once the session this copy works for has ended.
    fn current(&self) -> bool {
        self.scope.is_none_or(|s| self.live(s))
    }

    fn send(&self, msg: impl Into<Msg>) {
        if !self.current() {
            return;
        }
        let _ = self.tx.send(msg.into());
        (self.notify)();
    }

    /// Resumes the account with the tokens the keyring kept. A keychain that
    /// has none (or that fails) sends the app to the sign-in form; a server that
    /// has forgotten them turns up as a signed-out sync.
    pub fn start(&self, account: Account) {
        let session = self.session();
        let this = self.scoped_to(session);
        self.keeper.use_account(account.clone());
        let hook = self.keeper.hook();
        self.rt.spawn(async move {
            let wanted = account.clone();
            let settings = Settings::default();
            let tokens = tokio::task::spawn_blocking(move || settings.load_tokens(&wanted)).await.ok().flatten();
            if !this.live(session) {
                return;
            }
            let Some(tokens) = tokens else { return this.send(AccountMsg::NoTokens { session }) };
            let client = match abs::Client::new(&account.server, account.device()) {
                Ok(c) => c.with_tokens(tokens),
                Err(e) => return this.send(AccountMsg::SignInFailed { session, error: sign_in_error(&e) }),
            };
            client.set_auth_hook(hook);
            this.ready(session, account, client, None).await;
        });
    }

    /// Discovers authentication before showing credential controls.
    pub fn server_status(&self, account: Account) {
        self.end_session();
        let this = self.scoped();
        self.rt.spawn(async move {
            let session = this.scope.unwrap();
            let result = async {
                let client = abs::Client::new(&account.server, account.device())?;
                let status = client.status().await?;
                if status.app.as_deref().is_some_and(|app| app != "audiobookshelf") {
                    return Err(abs::Error::Decode("not Audiobookshelf".into()));
                }
                Ok(status)
            }
            .await;
            match result {
                Ok(status) => {
                    let warning = match check_version(status.version()) {
                        VersionCheck::TooOld(error) => return this.send(AccountMsg::SignInFailed { session, error }),
                        VersionCheck::Warn(why) => Some(why),
                        VersionCheck::Fine => None,
                    };
                    this.send(AccountMsg::Status { session, status, warning });
                }
                Err(e) => this.send(AccountMsg::SignInFailed { session, error: sign_in_error(&e) }),
            }
        });
    }

    pub fn begin_openid(&self, account: Account, warning: Option<String>) {
        self.end_session();
        let this = self.scoped();
        self.rt.spawn(async move {
            let session = this.scope.unwrap();
            let result = async {
                let client = abs::Client::new(&account.server, account.device())?;
                client.begin_openid().await
            }
            .await;
            let flow = match result {
                Ok(flow) => flow,
                Err(e) => return this.send(AccountMsg::SignInFailed { session, error: sign_in_error(&e) }),
            };
            let pending = crate::openid::Pending { account, flow, started: std::time::SystemTime::now(), warning };
            let save = this.clone();
            let saved = pending.clone();
            let result = tokio::task::spawn_blocking(move || {
                let mut slot = save.openid_store.lock().unwrap_or_else(|e| e.into_inner());
                if !save.current() {
                    return Ok(());
                }
                slot.generation = session;
                slot.store.save(&saved)
            })
            .await;
            match result {
                Ok(Ok(())) => this.send(AccountMsg::OpenIdReady { session, pending: Box::new(pending) }),
                _ => {
                    this.send(AccountMsg::SignInFailed { session, error: "Couldn't keep the browser sign-in in the keyring. Unlock it and try again.".into() })
                }
            }
        });
    }

    /// Read once at startup. Early Apple Events wait in the app until this
    /// ordered message arrives, so a cold redirect cannot overtake recovery.
    pub fn restore_openid(&self) {
        let this = self.clone();
        let session = this.session();
        self.rt.spawn_blocking(move || {
            let slot = this.openid_store.lock().unwrap_or_else(|e| e.into_inner());
            let store = &slot.store;
            let pending = if this.live(session) {
                let pending = store.load().filter(|p| !p.remaining(std::time::SystemTime::now()).is_zero());
                if pending.is_none() {
                    store.clear();
                }
                pending
            } else {
                None
            };
            // The app must learn that recovery finished even if a user action
            // already invalidated the transaction loaded here.
            this.send(AccountMsg::OpenIdRestored { session, pending: pending.map(Box::new) });
        });
    }

    pub fn cancel_login(&self) {
        self.end_session();
        self.keeper.cancel_login();
    }

    pub fn clear_openid(&self) {
        let this = self.clone();
        let generation = self.scope.unwrap_or_else(|| self.session());
        self.rt.spawn_blocking(move || {
            let slot = this.openid_store.lock().unwrap_or_else(|e| e.into_inner());
            if slot.generation <= generation {
                slot.store.clear();
            }
        });
    }

    pub fn complete_openid(&self, pending: crate::openid::Pending, callback: String) {
        let this = self.scoped();
        let account = pending.account;
        self.rt.spawn(async move {
            let session = this.scope.unwrap();
            let client = match abs::Client::new(&account.server, account.device()) {
                Ok(c) => c,
                Err(e) => return this.send(AccountMsg::SignInFailed { session, error: sign_in_error(&e) }),
            };
            // The server chooses the username. Persist only after identifying
            // which local account owns these tokens.
            let login = match client.complete_openid(pending.flow, &callback).await {
                Ok(login) => login,
                Err(e) => return this.send(AccountMsg::SignInFailed { session, error: sign_in_error(&e) }),
            };
            if !this.current() {
                let _ = client.logout().await;
                return;
            }
            this.send(AccountMsg::OpenIdAuthenticated { session, account, client, login: Box::new(login), warning: pending.warning });
        });
    }

    /// The UI has identified the account and accepted this session's result.
    pub fn accept_openid(&self, account: Account, client: abs::Client, tokens: abs::Tokens, warning: Option<String>) {
        let this = self.scoped();
        self.keeper.use_account(account.clone());
        let hook = self.keeper.hook();
        hook(abs::AuthEvent::Tokens(tokens));
        client.set_auth_hook(hook);
        self.clear_openid();
        self.rt.spawn(async move {
            this.ready(this.scope.unwrap(), account, client, warning).await;
        });
    }

    pub fn discard_login(&self, client: abs::Client) {
        self.rt.spawn(async move {
            let _ = client.logout().await;
        });
    }

    #[cfg(target_os = "macos")]
    pub fn receive_urls(&self) {
        let this = self.clone();
        crate::openid::macos::attach(Arc::new(move |url| this.send(AccountMsg::Redirect(url))));
    }

    /// Signs in with a username and password. The server is asked its version
    /// first: Rede needs 2.26 (refresh tokens) and warns below 2.36 (the
    /// refresh token was accepted as an access token until then).
    pub fn sign_in(&self, account: Account, password: String) {
        let session = self.session();
        let this = self.scoped_to(session);
        self.keeper.use_account(account.clone());
        let hook = self.keeper.hook();
        self.rt.spawn(async move {
            let failed = |error: String| Msg::Account(AccountMsg::SignInFailed { session, error });
            let client = match abs::Client::new(&account.server, account.device()) {
                Ok(c) => c,
                Err(e) => return this.send(failed(sign_in_error(&e))),
            };
            client.set_auth_hook(hook);
            let status = match client.status().await {
                Ok(s) => s,
                Err(e) => return this.send(failed(sign_in_error(&e))),
            };
            if status.app.as_deref().is_some_and(|app| app != "audiobookshelf") {
                return this.send(failed("That address answered, but not like an Audiobookshelf server.".into()));
            }
            let version = status.version();
            let warning = match check_version(version) {
                VersionCheck::TooOld(why) => return this.send(failed(why)),
                VersionCheck::Warn(why) => Some(why),
                VersionCheck::Fine => None,
            };
            if let Err(e) = client.login(&account.username, &password).await {
                return this.send(failed(sign_in_error(&e)));
            }
            if !this.live(session) {
                // Signed out while the password was on its way. The tokens the
                // sign-in just got are for an account nobody wants.
                let _ = client.logout().await;
                return;
            }
            this.ready(session, account, client, warning).await;
        });
    }

    /// Signs out: the tokens go (and only the tokens: the shelf and its books
    /// stay), and anything in flight stops. The answer to a sign-in that was
    /// still running is dropped with the session.
    #[cfg(test)]
    pub fn sign_out(&self, account: Account, source: Option<Arc<dyn Source>>) {
        self.sign_out_after(account, source, None);
    }

    pub fn sign_out_after(&self, account: Account, source: Option<Arc<dyn Source>>, finished: Option<tokio::sync::oneshot::Receiver<()>>) {
        self.end_session();
        // A refresh from the old client may still be in flight; its new pair
        // mustn't be written back over the sign-out.
        self.keeper.forget(account);
        if let Some(source) = source {
            self.rt.spawn(async move {
                if let Some(finished) = finished {
                    let _ = finished.await;
                }
                if let Err(e) = source.logout().await {
                    tracing::info!("ending the server login: {e}");
                }
            });
        }
    }

    /// Opens the shelf and tells the app the account is ready. The shelf is
    /// this account's own file, so a book's place in it never collides with
    /// another account's.
    async fn ready(&self, session: u64, account: Account, client: abs::Client, warning: Option<String>) {
        let this = self.scoped_to(session);
        if !this.current() {
            let _ = client.logout().await;
            return;
        }
        let path = this.paths.shelf(&account);
        let opened = tokio::task::spawn_blocking(move || open_shelf(&path)).await;
        let shelf = match opened {
            Ok(Ok(shelf)) => Arc::new(shelf),
            Ok(Err(e)) => return this.send(AccountMsg::SignInFailed { session, error: format!("Couldn't open this account's library: {e}") }),
            Err(e) => return this.send(AccountMsg::SignInFailed { session, error: format!("Couldn't open this account's library: {e}") }),
        };
        if !this.current() {
            let _ = client.logout().await;
            return;
        }
        let source: Arc<dyn Source> = Arc::new(AbsSource::new(account.id, client, MIME_TYPES.iter().map(|m| (*m).to_owned()).collect()));
        this.send(AccountMsg::SignedIn { session, account, source, shelf, warning });
    }

    pub fn positions(&self, shelf: Arc<Shelf>, keys: Vec<ItemKey>) {
        let this = self.scoped();
        self.rt.spawn_blocking(move || {
            let mut positions = Vec::new();
            for key in keys {
                match shelf.position(&key) {
                    Ok(Some(position)) => positions.push(position),
                    Ok(None) => {}
                    Err(e) => {
                        this.send(Msg::Error(Failure::Local(format!("Couldn't read listening progress: {e}"))));
                        return;
                    }
                }
            }
            this.playback(crate::listening::Event::Positions(positions));
        });
    }

    /// Reads the shelf's books without the network, for the first frame after a
    /// start: the library is there before the server answers.
    pub fn load_books(&self, shelf: Arc<Shelf>, libraries: Vec<KnownLibrary>) {
        let this = self.scoped();
        self.rt.spawn_blocking(move || match sync::stored_books(&shelf, &libraries) {
            Ok(books) => this.send(LibraryMsg::Books(books)),
            Err(e) => this.send(Msg::Error(e)),
        });
    }

    /// Lists every book library and writes what changed to the shelf, then
    /// sends the shelf's books. Replaces a pass already running: that one stops
    /// storing.
    pub fn sync(&self, source: Arc<dyn Source>, shelf: Arc<Shelf>) {
        let pass = self.pass.fetch_add(1, Ordering::AcqRel) + 1;
        let this = self.scoped();
        self.rt.spawn(async move {
            let live = || this.current() && this.pass.load(Ordering::Acquire) == pass;
            if !live() {
                return;
            }
            let outcome = sync::run(source.as_ref(), &shelf, sources::Stamp::from(std::time::SystemTime::now()), |n| {
                if live() {
                    this.send(LibraryMsg::SyncProgress(n));
                }
            })
            .await;
            // A newer pass (or another account) owns the shelf and the screen.
            if !live() {
                return;
            }
            if let Outcome::Done(report) = &outcome {
                let libraries = report.libraries.clone();
                match sync::stored_books(&shelf, &libraries) {
                    Ok(books) => this.send(LibraryMsg::Books(books)),
                    Err(e) => this.send(Msg::Error(e)),
                }
            }
            this.send(LibraryMsg::SyncDone(outcome));
        });
    }

    /// Fetches a book's details and stores them, then tells the page. What the
    /// shelf already has is sent first, so the page fills in at once.
    pub fn open_book(&self, source: Option<Arc<dyn Source>>, shelf: Arc<Shelf>, key: ItemKey) {
        let this = self.scoped();
        self.rt.spawn(async move {
            let (asked, stored) = (key.clone(), shelf.clone());
            match tokio::task::spawn_blocking(move || stored.item(&asked)).await {
                Ok(Ok(Some(item))) => this.send(LibraryMsg::Book { key: key.clone(), item: Box::new(item) }),
                Ok(Ok(None)) => {}
                Ok(Err(e)) => tracing::warn!("reading a book from the shelf: {e}"),
                Err(e) => tracing::warn!("reading a book from the shelf: {e}"),
            }
            let Some(source) = source else { return };
            let item = match source.item(&key.item).await {
                Ok(item) => item,
                Err(e) => {
                    // Not a failure worth a banner: the page has what the
                    // listing stored, and a podcast item is simply not shown.
                    tracing::info!("loading book {}: {e}", key.item);
                    return;
                }
            };
            let (saved, book) = (shelf.clone(), key.clone());
            let to_store = item.clone();
            match tokio::task::spawn_blocking(move || saved.set_details(&book, &to_store)).await {
                Ok(Ok(())) => this.send(LibraryMsg::Book { key, item: Box::new(item) }),
                Ok(Err(e)) => this.send(LibraryMsg::BookFailed { key, error: Failure::Local(format!("Couldn't save the book: {e}")) }),
                Err(e) => this.send(LibraryMsg::BookFailed { key, error: Failure::Local(format!("Couldn't save the book: {e}")) }),
            }
        });
    }
}

/// What a server's version means for signing in to it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum VersionCheck {
    /// Older than the refresh tokens Rede signs in with: there's nothing to do
    /// but tell the user to update it.
    TooOld(String),
    /// New enough, with a known security problem closed in [`Version::REFRESH_FIX`].
    Warn(String),
    Fine,
}

/// Audiobookshelf 2.26 signs in with access and refresh tokens; before 2.36 it
/// accepted a refresh token where an access token belongs (CVE-2026-73085), so
/// an old one works and is worth a word. A server that doesn't say its version
/// is given the benefit of the doubt, as Ond does.
fn check_version(version: Option<Version>) -> VersionCheck {
    match version {
        Some(v) if v < Version::TOKENS => VersionCheck::TooOld(format!("This server is version {v}. Rede needs Audiobookshelf 2.26 or newer.")),
        Some(v) if v < Version::REFRESH_FIX => VersionCheck::Warn("This server version has a known security issue; update it.".to_owned()),
        _ => VersionCheck::Fine,
    }
}

/// Opens (or makes) an account's shelf. The folder is made here rather than
/// only at start-up: the first run after an upgrade, or a user who tidied
/// their data folder, would otherwise be told the library can't be opened and
/// nothing else.
fn open_shelf(path: &std::path::Path) -> Result<Shelf, String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    Shelf::open(path).map_err(|e| e.to_string())
}

/// What the user reads when signing in fails: the gist, without the plumbing.
pub fn sign_in_error(e: &abs::Error) -> String {
    match e {
        abs::Error::OpenId(e) => e.to_string(),
        abs::Error::Url(_) => "That doesn't look like a server address. Try something like https://books.example.com.".into(),
        abs::Error::Network(h) if h.is_connect() || h.is_timeout() => "Couldn't reach the server. Check the address and that it's running.".into(),
        abs::Error::Network(h) => format!("Couldn't reach the server: {h}"),
        abs::Error::Auth(AuthError::BadCredentials) => "Wrong username or password.".into(),
        abs::Error::Auth(AuthError::RateLimited { retry_after }) => match retry_after {
            Some(wait) => format!("Too many sign-in attempts. Try again in {} seconds.", wait.as_secs().max(1)),
            None => "Too many sign-in attempts. Try again in a few minutes.".into(),
        },
        abs::Error::Auth(AuthError::SignedOut) => crate::failure::SIGNED_OUT.into(),
        abs::Error::Server { status, message } => match message {
            Some(m) => format!("The server answered with an error ({status}): {m}"),
            None => format!("The server answered with an error ({status})."),
        },
        abs::Error::Decode(said) => format!("That address answered, but not like an Audiobookshelf server ({said})."),
        abs::Error::NotApi(Some(said)) => format!("That address answered with a page instead of the server's API: {said}"),
        abs::Error::NotApi(None) => "That address answered, but not like an Audiobookshelf server.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, channel};
    use std::time::Duration;
    use testkit::{Answer, Deadline, JsonFake, Seen, WAIT, slack};

    fn account() -> Account {
        Account::new("https://books.example.com", "sam")
    }

    struct TokenStore {
        forgotten: Sender<()>,
    }

    impl crate::tokens::TokenStore for TokenStore {
        fn store(&self, _: &Account, _: &abs::Tokens) {}
        fn forget(&self, _: &Account) {
            let _ = self.forgotten.send(());
        }
    }

    #[test]
    fn sign_out_ends_the_server_login_even_when_logout_is_refused() {
        for status in ["200 OK", "500 Internal Server Error"] {
            let (requested, request) = channel();
            let (release, released) = channel::<()>();
            let released = std::sync::Mutex::new(released);
            let server = JsonFake::start(move |seen| {
                requested.send(seen.clone()).unwrap();
                released.lock().unwrap().recv_timeout(slack(WAIT)).unwrap();
                Answer::new(status, "")
            });
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let (tx, rx) = channel();
            let (forgotten, forgot) = channel();
            let keeper = Keeper::with_store(TokenStore { forgotten });
            let dir = tempfile::tempdir().unwrap();
            let bridge = Bridge::new(
                rt.handle().clone(),
                tx,
                Arc::new(|| {}),
                keeper.clone(),
                Arc::new(Paths { config_file: dir.path().join("settings.json"), shelves: dir.path().join("accounts"), art_cache: dir.path().join("art") }),
            );
            let account = Account::new(server.base().trim_end_matches('/'), "sam");
            keeper.use_account(account.clone());
            let client = abs::Client::new(&account.server, account.device()).unwrap().with_tokens(abs::Tokens::new("access", "refresh"));
            client.set_auth_hook(keeper.hook());
            let source = Arc::new(AbsSource::new(account.id, client, Vec::new()));
            bridge.sign_out(account, Some(source.clone()));
            // The server hasn't even received logout yet: local deletion must
            // not depend on its response or on the async runtime being driven.
            forgot.recv_timeout(slack(WAIT)).unwrap();
            let seen: Seen = rt.block_on(async { tokio::task::spawn_blocking(move || request.recv_timeout(slack(WAIT)).unwrap()).await.unwrap() });
            assert_eq!((seen.method.as_str(), seen.path()), ("POST", "/logout"));
            assert_eq!(seen.header("x-refresh-token"), Some("refresh"));
            assert!(source.client().tokens().is_none());
            release.send(()).unwrap();
            assert!(rx.try_iter().next().is_none(), "logout does not deliver errors to the signed-out screen");
        }
    }

    #[test]
    fn an_unreachable_server_does_not_prevent_local_sign_out() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let account = Account::new(format!("http://{}", listener.local_addr().unwrap()), "sam");
        drop(listener);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (tx, rx) = channel();
        let (forgotten, forgot) = channel();
        let keeper = Keeper::with_store(TokenStore { forgotten });
        let dir = tempfile::tempdir().unwrap();
        let bridge = Bridge::new(
            rt.handle().clone(),
            tx,
            Arc::new(|| {}),
            keeper.clone(),
            Arc::new(Paths { config_file: dir.path().join("settings.json"), shelves: dir.path().join("accounts"), art_cache: dir.path().join("art") }),
        );
        let client = abs::Client::new(&account.server, account.device()).unwrap().with_tokens(abs::Tokens::new("access", "refresh"));
        let source = Arc::new(AbsSource::new(account.id, client, Vec::new()));
        bridge.sign_out(account, Some(source.clone()));
        forgot.recv_timeout(slack(WAIT)).unwrap();
        rt.block_on(async {
            tokio::time::timeout(slack(WAIT), async {
                while source.client().tokens().is_some() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        });
        assert_eq!(bridge.session(), 1);
        assert!(rx.try_iter().next().is_none());
    }

    #[test]
    fn sign_in_errors_read_plainly() {
        assert_eq!(sign_in_error(&abs::Error::Auth(AuthError::BadCredentials)), "Wrong username or password.");
        let limited = abs::Error::Auth(AuthError::RateLimited { retry_after: Some(Duration::from_secs(90)) });
        assert_eq!(sign_in_error(&limited), "Too many sign-in attempts. Try again in 90 seconds.");
        assert_eq!(
            sign_in_error(&abs::Error::Server { status: 502, message: Some("Navidrome is restarting".into()) }),
            "The server answered with an error (502): Navidrome is restarting"
        );
        let url = abs::Client::new("not a url", account().device()).err().unwrap();
        assert!(sign_in_error(&url).contains("server address"));
    }

    /// A bridge on one thread, and what it sends.
    fn rig(dir: &std::path::Path) -> (tokio::runtime::Runtime, Bridge, Receiver<Msg>, Keeper) {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (tx, rx) = channel();
        let keeper = Keeper::start();
        let paths = crate::settings::Paths { config_file: dir.join("settings.json"), shelves: dir.join("accounts"), art_cache: dir.join("art") };
        let bridge = Bridge::new(rt.handle().clone(), tx, Arc::new(|| {}), keeper.clone(), Arc::new(paths));
        (rt, bridge, rx, keeper)
    }

    /// Runs the bridge's tasks until `done` sees what it wants, collecting
    /// everything sent.
    fn until(rt: &tokio::runtime::Runtime, rx: &Receiver<Msg>, got: &mut Vec<Msg>, mut done: impl FnMut(&[Msg]) -> bool) {
        rt.block_on(async {
            let deadline = Deadline::new();
            loop {
                got.extend(rx.try_iter());
                if done(got) {
                    return;
                }
                assert!(!deadline.passed(), "timed out; got {} messages", got.len());
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
    }

    #[test]
    fn a_server_too_old_to_sign_in_to_is_turned_away_and_an_old_one_warned_about() {
        assert_eq!(
            check_version(Version::parse("2.25.1")),
            VersionCheck::TooOld("This server is version 2.25.1. Rede needs Audiobookshelf 2.26 or newer.".into())
        );
        // The first version Rede signs in to is also one to warn about: the
        // refresh-token fix came ten minor versions later.
        assert_eq!(check_version(Version::parse("2.26.0")), VersionCheck::Warn("This server version has a known security issue; update it.".into()));
        assert_eq!(check_version(Version::parse("2.35.9")), VersionCheck::Warn("This server version has a known security issue; update it.".into()));
        assert_eq!(check_version(Version::parse("2.36.0")), VersionCheck::Fine);
        assert_eq!(check_version(Version::parse("2.37.2")), VersionCheck::Fine);
        assert_eq!(check_version(None), VersionCheck::Fine, "a server that doesn't say its version is given the benefit of the doubt");
    }

    #[test]
    fn an_accounts_shelf_folder_is_made_when_it_isnt_there() {
        let dir = tempfile::tempdir().unwrap();
        // A folder that no earlier run made, as on a first start.
        let path = dir.path().join("accounts").join("shelf.redb");
        assert!(!path.parent().unwrap().exists());
        open_shelf(&path).expect("the folder is made and the shelf opens");
        assert!(path.exists());
        // Opening it again is fine, and so is a file a newer build made.
        assert!(open_shelf(&path).is_ok());
    }

    #[test]
    fn work_for_a_session_that_ended_sends_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (_rt, bridge, rx, _keeper) = rig(dir.path());
        let scoped = bridge.scoped();
        scoped.send(Msg::Error(Failure::Local("before".into())));
        bridge.end_session();
        scoped.send(Msg::Error(Failure::Local("after".into())));
        // Copies of a copy keep the session they were made for.
        scoped.scoped().send(Msg::Error(Failure::Local("after, again".into())));
        let sent = rx.try_iter().filter_map(|m| if let Msg::Error(e) = m { Some(e.into_message()) } else { None }).collect::<Vec<_>>();
        assert_eq!(sent, ["before"]);
    }

    #[test]
    fn an_account_without_tokens_asks_to_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        let (rt, bridge, rx, _keeper) = rig(dir.path());
        bridge.start(account());
        let mut got = Vec::new();
        // The keyring has nothing for this account (it was never signed in), so
        // the app is told rather than left waiting.
        until(&rt, &rx, &mut got, |m| !m.is_empty());
        assert!(matches!(got[0], Msg::Account(AccountMsg::NoTokens { .. })));
    }

    /// The hook a client gets is `Send + Sync`, as `set_auth_hook` needs; it
    /// hands the event on without waiting for the keyring.
    #[test]
    fn the_token_hook_passes_events_on_without_blocking() {
        fn send_sync<T: Send + Sync>(_: &T) {}
        let dir = tempfile::tempdir().unwrap();
        let (_rt, bridge, rx, keeper) = rig(dir.path());
        send_sync(&keeper.hook());
        keeper.use_account(account());
        (keeper.hook())(abs::AuthEvent::SignedOut);
        // The keeper's thread takes it from there; nothing was sent to the UI.
        assert!(rx.try_iter().next().is_none());
        drop(bridge);
    }

    #[test]
    fn browser_sign_in_uses_the_keyring_transaction_and_the_existing_token_keeper() {
        use crate::openid::Store;
        let fake = JsonFake::start(|seen| {
            let target = seen.target.as_str();
            if target == "/status" {
                Answer::ok(
                    r#"{"app":"audiobookshelf","serverVersion":"2.35.0","authMethods":["openid"],"authFormData":{"authOpenIDButtonText":"Sign in with Book Club"}}"#,
                )
            } else if target.starts_with("/auth/openid?") {
                let state = target.split("state=").nth(1).unwrap().split('&').next().unwrap();
                Answer::new("302 Found", "")
                    .with_header("Location", format!("https://id.example/authorize?state={state}"))
                    .with_header("Set-Cookie", "connect.sid=session; HttpOnly")
                    .with_header("Set-Cookie", "auth_method=openid-mobile; HttpOnly")
            } else {
                Answer::ok(r#"{"user":{"id":"user-1","username":"sam","accessToken":"access","refreshToken":"refresh"},"serverSettings":{"version":"2.35.0"}}"#)
            }
        });
        let server = fake.base().trim_end_matches('/').to_owned();
        struct Saved(Sender<(Account, abs::Tokens)>);
        impl crate::tokens::TokenStore for Saved {
            fn store(&self, a: &Account, t: &abs::Tokens) {
                self.0.send((a.clone(), t.clone())).unwrap();
            }
            fn forget(&self, _: &Account) {}
        }
        let (stored, tokens) = channel();
        let keeper = Keeper::with_store(Saved(stored));
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (tx, rx) = channel();
        let dir = tempfile::tempdir().unwrap();
        let mut bridge = Bridge::new(
            rt.handle().clone(),
            tx,
            Arc::new(|| {}),
            keeper,
            Arc::new(Paths { config_file: dir.path().join("settings.json"), shelves: dir.path().join("accounts"), art_cache: dir.path().join("art") }),
        );
        let pending_store = crate::openid::TestStore::default();
        bridge.use_openid_store(pending_store.clone());
        let account = Account::new(server, "");
        bridge.server_status(account.clone());
        let mut got = Vec::new();
        until(&rt, &rx, &mut got, |m| m.iter().any(|m| matches!(m, Msg::Account(AccountMsg::Status { .. }))));
        let Some(Msg::Account(AccountMsg::Status { status, warning, .. })) = got.pop() else {
            panic!("expected discovery");
        };
        assert_eq!(status.auth_form_data.auth_open_id_button_text.as_deref(), Some("Sign in with Book Club"));
        assert!(warning.is_some());
        bridge.begin_openid(account, warning);
        until(&rt, &rx, &mut got, |m| m.iter().any(|m| matches!(m, Msg::Account(AccountMsg::OpenIdReady { .. }))));
        let Some(Msg::Account(AccountMsg::OpenIdReady { pending, .. })) = got.pop() else {
            panic!("expected browser preparation");
        };
        let recovered = pending_store.load().expect("stored before opening the browser");
        assert_eq!(pending.flow.browser_url(), recovered.flow.browser_url());
        let state = recovered.flow.browser_url().split("state=").nth(1).unwrap();
        let callback = format!("rede://oauth?code=provider-code&state={state}");
        bridge.complete_openid(recovered, callback);
        until(&rt, &rx, &mut got, |m| m.iter().any(|m| matches!(m, Msg::Account(AccountMsg::OpenIdAuthenticated { .. }))));
        let Some(Msg::Account(AccountMsg::OpenIdAuthenticated { mut account, client, login, warning, .. })) = got.pop() else {
            panic!("expected authentication");
        };
        account.username = login.user.username.unwrap();
        bridge.accept_openid(account, client, login.tokens, warning);
        until(&rt, &rx, &mut got, |m| m.iter().any(|m| matches!(m, Msg::Account(AccountMsg::SignedIn { .. }))));
        let (account, tokens) = tokens.recv_timeout(slack(WAIT)).unwrap();
        assert_eq!(account.username, "sam");
        assert_eq!(tokens.refresh(), "refresh");
        assert!(pending_store.load().is_none());
        let seen = fake.seen();
        assert_eq!(seen.len(), 3, "{seen:?}");
        let callback = &seen[2];
        assert_eq!(callback.path(), "/auth/openid/callback");
        assert!(callback.target.contains("code_verifier="));
        assert_eq!(callback.header("cookie"), Some("connect.sid=session; auth_method=openid-mobile"));
    }

    #[test]
    fn expired_keyring_transactions_are_cleared_and_not_resumed() {
        use crate::openid::Store;
        let dir = tempfile::tempdir().unwrap();
        let (rt, mut bridge, rx, _) = rig(dir.path());
        let store = crate::openid::TestStore::default();
        let mut pending = crate::openid::test_pending();
        pending.started = std::time::SystemTime::now() - crate::openid::TIMEOUT;
        store.save(&pending).unwrap();
        bridge.use_openid_store(store.clone());
        bridge.restore_openid();
        let mut got = Vec::new();
        until(&rt, &rx, &mut got, |m| !m.is_empty());
        assert!(matches!(&got[0], Msg::Account(AccountMsg::OpenIdRestored { pending: None, .. })));
        assert!(store.load().is_none());
    }

    #[test]
    fn delayed_cleanup_for_a_cancelled_attempt_cannot_delete_a_newer_attempt() {
        use crate::openid::Store;
        for newer in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let (rt, mut bridge, _, _) = rig(dir.path());
            let store = crate::openid::TestStore::default();
            bridge.use_openid_store(store.clone());
            let old = bridge.scoped();
            bridge.end_session();
            if newer {
                bridge.openid_store.lock().unwrap().generation = bridge.session();
            }
            store.save(&crate::openid::test_pending()).unwrap();
            old.clear_openid();
            // Runtime shutdown joins blocking storage work, with no sleep or
            // assumptions about which worker acquired the lock first.
            drop(rt);
            assert_eq!(store.load().is_some(), newer);
        }
    }
}
