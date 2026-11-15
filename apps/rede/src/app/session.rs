//! The account and the shelf: signing in, coming back to a saved login, and
//! signing out. The sign-in form pushes [`Action::SignIn`]; everything that
//! talks to the server is the bridge's.
//!
//! Signing out forgets local tokens and asks the server to end its login as
//! a best effort. The shelf and downloaded art stay where they are, so signing
//! back in finds the library at once.
//!
//! Signing in while an account is in view (the switcher's Add source…) runs
//! beside it: each step goes through [`Bridge::beside`](crate::bridge::Bridge::beside),
//! so the account in view keeps its session and its login until the new one
//! signs in, and a cancel leaves it as it was.

use super::{Action, RedeApp};
use crate::failure::SIGNED_OUT;
use crate::settings::{Account, SignInMethod};
use crate::sync::Outcome;
use shelf::Shelf;
use shell::icons::Icon;
use shell::kit;
use shell::theme::{self, Tokens};
use sources::Source;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// When the sign-in spinner started turning, in egui's own memory.
fn signing_in_since() -> egui::Id {
    egui::Id::unique("signing-in-since")
}

/// How long the sign-in spinner turns before it goes still.
const CONNECT_SPIN_SECS: f64 = 15.0;
/// How often it turns.
const SPIN_FRAME: Duration = Duration::from_millis(50);

/// Who's signed in, and what their books are in.
#[derive(Default)]
pub struct Session {
    pub account: Option<Account>,
    pub source: Option<Arc<dyn Source>>,
    pub shelf: Option<Arc<Shelf>>,
    /// The saved login is being tried: the server should answer soon.
    pub connecting: bool,
}

impl Session {
    /// Signed in, with a shelf to read and a source to ask.
    pub fn signed_in(&self) -> bool {
        self.account.is_some() && self.shelf.is_some()
    }
}

/// The sign-in form. It keeps the account it's signing in, so a reconnect
/// after signing out keeps the id the shelf, the tokens and any future
/// downloads are keyed by, and the server can change address without losing
/// them.
#[derive(Default)]
pub struct LoginForm {
    pub server: String,
    pub username: String,
    pub password: String,
    pub busy: bool,
    pub error: Option<String>,
    pub account: Option<Account>,
    /// The first empty field was given focus: once, so the user can move it.
    pub focused: bool,
    pub status: Option<abs::ServerStatus>,
    pub checked_server: String,
    pub warning: Option<String>,
}

impl LoginForm {
    /// A form filled in from `account`, if there is one.
    pub fn for_account(account: Option<&Account>) -> Self {
        match account {
            Some(a) => Self { server: a.server.clone(), username: a.username.clone(), account: Some(a.clone()), ..Self::default() },
            None => Self::default(),
        }
    }
}

impl RedeApp {
    /// Signed in: the session starts and the shelf is read.
    pub(super) fn signed_in(&mut self, account: Account, source: Arc<dyn Source>, shelf: Arc<Shelf>, warning: Option<String>) {
        // Another account signed in: what's on screen came from the old one.
        let changed = self.session.account.as_ref().map(|a| a.id) != Some(account.id);
        // Signed in beside an account in view: its session goes, and its
        // work still in flight with it.
        let beside = self.session.signed_in();
        if beside || (changed && self.session.account.is_some()) {
            let finished = self.teardown_session();
            if beside {
                self.bridge.end_session();
            }
            self.bridge.track_finish(finished);
        }
        self.bridge.adopt_login();
        let account = Account { signed_out: false, ..account };
        self.session = Session { account: Some(account.clone()), source: Some(source.clone()), shelf: Some(shelf.clone()), connecting: false };
        self.bridge.close_downloads(self.downloads.store.clone());
        self.downloads = Default::default();
        self.bridge.open_downloads(account.id, self.shown.downloads.limit, self.stream_dir.clone(), self.downloads.store.clone());
        self.login = None;
        self.source_added();
        self.openid.pending = None;
        self.openid.exchanging = false;
        self.art.set_source(Some(source.clone()));
        self.listening = Some(self.bridge.listening(
            source,
            shelf.clone(),
            sources::DeviceInfo {
                device_id: account.device_id.clone(),
                client_name: "Rede".into(),
                client_version: env!("CARGO_PKG_VERSION").into(),
                ..Default::default()
            },
            self.downloads.store.clone(),
        ));
        self.prefs.edit(|s| s.put_account(account.clone()));
        self.shown = super::PrefsView::read(&self.prefs);
        if changed {
            self.stats = Default::default();
            self.books.clear();
            self.ui.nav = super::Nav::default();
            self.ui.landing = true;
        }
        if let Some(warning) = warning {
            self.show_notice(warning);
        }
        // What the shelf already has, while the server is asked what changed.
        self.bridge.load_books(shelf, account.libraries.clone());
        self.sync_now();
        // Opened on Stats before the account was back: ask now.
        if *self.ui.nav.current() == super::Route::Stats {
            self.load_stats();
        }
    }

    /// The sign-in failed. From the form: back to it, with the plain reason.
    /// From a saved login whose shelf wouldn't open: the form, filled in.
    pub(super) fn sign_in_failed(&mut self, error: String) {
        self.session.connecting = false;
        self.openid.pending = None;
        self.openid.exchanging = false;
        self.bridge.clear_openid();
        if let Some(form) = self.login.as_mut() {
            form.busy = false;
            form.password.clear();
            form.error = Some(error);
            return;
        }
        self.needs_sign_in(Some(error));
    }

    /// The sign-in form, filled in from the account on file (if any) and saying
    /// `error` when there is one.
    pub(super) fn needs_sign_in(&mut self, error: Option<String>) {
        self.set_mode(crate::window::Mode::Full);
        let account = self.session.account.clone().or_else(|| self.prefs.get(|s| s.account().cloned()));
        self.login = Some(LoginForm { error, ..LoginForm::for_account(account.as_ref()) });
        self.session.connecting = false;
    }

    pub(super) fn check_server(&mut self) {
        let bridge = self.sign_in_bridge();
        let Some(form) = self.login.as_mut().filter(|f| !f.busy) else { return };
        if form.server.trim().is_empty() {
            form.error = Some("Enter the server's address, for example books.example.com.".into());
            return;
        }
        form.busy = true;
        form.status = None;
        form.error = None;
        form.checked_server = form.server.trim().to_owned();
        bridge.server_status(Account::new(&form.checked_server, ""));
    }

    /// The bridge for a sign-in step: beside the account in view, if one
    /// is.
    fn sign_in_bridge(&self) -> crate::bridge::Bridge {
        self.bridge.beside(self.session.signed_in())
    }

    pub(super) fn begin_openid(&mut self) {
        let bridge = self.sign_in_bridge();
        let Some(form) = self.login.as_mut().filter(|f| !f.busy && f.status.is_some() && f.server.trim() == f.checked_server) else { return };
        if !crate::openid::SUPPORTED {
            form.error = Some("Browser sign-in is only available on macOS for now. Use an Audiobookshelf password on Linux.".into());
            return;
        }
        form.busy = true;
        form.error = None;
        let account =
            form.account.clone().map(|a| Account { server: form.checked_server.clone(), ..a }).unwrap_or_else(|| Account::new(&form.checked_server, ""));
        bridge.begin_openid(account, form.warning.clone());
    }

    pub(super) fn handle_sign_in(&mut self, msg: crate::bridge::AccountMsg) {
        use crate::bridge::AccountMsg;
        match msg {
            AccountMsg::Status { session, status, warning, server } if session == self.bridge.session() => {
                let Some(form) = self.login.as_mut() else { return };
                form.busy = false;
                form.warning = warning;
                // The address that answered, with its scheme: a bare host typed
                // in is shown, and kept, as what it turned out to be.
                form.server = server.clone();
                form.checked_server = server;
                let launch = crate::openid::SUPPORTED && status.auth_methods.iter().any(|m| m == "openid") && status.auth_form_data.auth_open_id_auto_launch;
                form.status = Some(status);
                if launch {
                    self.begin_openid();
                }
            }
            AccountMsg::OpenIdReady { session, pending } if session == self.bridge.session() => {
                self.ctx.open_url(egui::OpenUrl::new_tab(pending.flow.browser_url()));
                self.wait_for_openid(*pending);
            }
            AccountMsg::OpenIdRestored { session, pending } => {
                self.openid.restored = true;
                if session == 0
                    && self.bridge.session() == session
                    && let Some(pending) = pending
                {
                    self.bridge.end_session();
                    self.session = Session::default();
                    self.wait_for_openid(*pending);
                }
                if let Some(url) = self.openid.early_redirect.take() {
                    self.openid_redirect(url);
                }
            }
            #[cfg(any(target_os = "macos", test))]
            AccountMsg::Redirect(url) => self.openid_redirect(url),
            AccountMsg::OpenIdAuthenticated { session, account, client, login, warning } => {
                if session != self.bridge.session() {
                    self.bridge.discard_login(client);
                    return;
                }
                let username = login.user.username.unwrap_or_default();
                let account = self.prefs.get(|s| s.account_for_sign_in(account.server, username, Some(SignInMethod::OpenId)));
                self.sign_in_bridge().accept_openid(account, client, login.tokens, warning);
            }
            _ => {}
        }
    }

    fn wait_for_openid(&mut self, pending: crate::openid::Pending) {
        let form = self.login.get_or_insert_with(|| LoginForm::for_account(Some(&pending.account)));
        if form.server != pending.account.server {
            form.status = None;
            form.checked_server.clear();
        }
        form.server = pending.account.server.clone();
        form.account = Some(pending.account.clone());
        form.busy = true;
        form.password.clear();
        form.error = None;
        self.session.connecting = false;
        self.ctx.request_repaint_after(pending.remaining(std::time::SystemTime::now()));
        self.openid.exchanging = false;
        self.openid.pending = Some(pending);
    }

    fn openid_redirect(&mut self, url: String) {
        if !self.openid.restored {
            self.openid.early_redirect = Some(url);
            return;
        }
        self.expire_openid(std::time::SystemTime::now());
        let Some(pending) = &self.openid.pending else {
            tracing::info!("ignored an OpenID redirect without a pending sign-in");
            return;
        };
        match pending.flow.validate_callback(&url) {
            Ok(()) => {
                if self.openid.exchanging {
                    return;
                }
                self.openid.exchanging = true;
                self.bridge.complete_openid(pending.clone(), url);
            }
            Err(abs::Error::OpenId(abs::OpenIdError::StateMismatch)) => tracing::warn!("ignored an OpenID redirect with an unmatched state or target"),
            Err(e) => self.cancel_sign_in(match e {
                abs::Error::OpenId(e) => e,
                _ => abs::OpenIdError::Server { status: 400 },
            }),
        }
    }

    pub(super) fn cancel_sign_in(&mut self, reason: abs::OpenIdError) {
        self.sign_in_bridge().cancel_login();
        self.openid.exchanging = false;
        self.openid.pending = None;
        self.openid.early_redirect = None;
        self.sign_in_failed(reason.to_string());
    }

    pub(super) fn expire_openid(&mut self, now: std::time::SystemTime) {
        if self.openid.pending.as_ref().is_some_and(|p| p.remaining(now).is_zero()) {
            self.cancel_sign_in(abs::OpenIdError::TimedOut);
        }
    }

    /// Connects with what the form holds.
    pub(super) fn sign_in(&mut self) {
        if self.login.as_ref().is_some_and(|f| f.status.is_none() || f.server.trim() != f.checked_server) {
            self.check_server();
            return;
        }
        let bridge = self.sign_in_bridge();
        let Some(form) = self.login.as_mut().filter(|f| !f.busy) else { return };
        let server = form.server.trim().to_owned();
        let username = form.username.trim().to_owned();
        if server.is_empty() {
            form.error = Some("Enter the server's address, for example books.example.com.".into());
            return;
        }
        if username.is_empty() {
            form.error = Some("Enter your username.".into());
            return;
        }
        if form.password.is_empty() {
            form.error = Some("Enter your password.".into());
            return;
        }
        form.busy = true;
        form.error = None;
        let account = self.prefs.get(|s| s.account_for_sign_in(server, username, Some(SignInMethod::Password)));
        form.account = Some(account.clone());
        self.session.connecting = true;
        bridge.sign_in(account, form.password.clone());
    }

    /// Stops playback, finishes the listening worker and clears the active
    /// account's in-memory state so either signing out or switching starts clean.
    pub(super) fn teardown_session(&mut self) -> Option<tokio::sync::oneshot::Receiver<()>> {
        self.set_mode(crate::window::Mode::Full);
        self.playback_control(crate::player::Control::Stop);
        self.playback_generation += 1;
        self.opening = false;
        let finished = self.listening.take().map(crate::listening::Listening::finish);
        if let Some(engine) = self.engine.take() {
            engine.shutdown(Duration::from_secs(2));
        }
        self.player = crate::player::Player::default();
        self.positions.clear();
        self.bookmarks = super::bookmarks::State::default();
        self.reconciliation = None;
        self.ui.palette = None;
        self.session = Session::default();
        self.bridge.close_downloads(self.downloads.store.clone());
        self.downloads = Default::default();
        self.stats = Default::default();
        self.books.clear();
        self.art.set_source(None);
        self.sync = super::SyncState::default();
        self.ui.nav = super::Nav::default();
        self.ui.notice = None;
        finished
    }

    /// Switches to `id`, stopping playback and flushing pending progress for
    /// the current account before opening the target shelf and session.
    pub(super) fn switch_account(&mut self, id: accounts::AccountId) {
        let Some(account) = self.prefs.get(|s| s.account_by_id(id).cloned()) else { return };
        if self.session.account.as_ref().map(|a| a.id) == Some(id) && (self.session.signed_in() || self.session.connecting) && !account.signed_out {
            return;
        }
        let finished = self.teardown_session();
        self.openid.pending = None;
        self.openid.exchanging = false;
        self.prefs.edit(|s| s.scope.choose_one(id));
        self.shown = super::PrefsView::read(&self.prefs);
        if account.signed_out {
            self.bridge.end_session();
            self.bridge.track_finish(finished);
            self.login = Some(LoginForm::for_account(Some(&account)));
        } else {
            self.login = None;
            self.session = Session { connecting: true, ..Session::default() };
            self.bridge.switch_account(account, finished);
        }
    }

    /// Signs out: the tokens go, and nothing else. The sign-in screen comes
    /// back with the account filled in, and anything in flight is dropped.
    pub(super) fn sign_out(&mut self) {
        let Some(mut account) = self.session.account.clone().or_else(|| self.prefs.get(|s| s.account().cloned())) else { return };
        account.signed_out = true;
        let source = self.session.source.clone();
        self.prefs.edit(|s| s.set_signed_out(account.id, true));
        self.shown = super::PrefsView::read(&self.prefs);
        let finished = self.teardown_session();
        self.bridge.sign_out_after(account.clone(), source, finished);
        self.login = Some(LoginForm::for_account(Some(&account)));
        self.onboarding = Some(kit::OnboardingStep::Kinds);
    }

    /// Signs out and says why: the server turned the saved login down.
    fn signed_out_and_say(&mut self, why: &str) {
        self.sign_out();
        if let Some(form) = self.login.as_mut() {
            form.error = Some(why.to_owned());
        }
    }

    /// Lists the libraries and writes what changed to the shelf. One sync runs
    /// at a time: a second asked for while one runs is dropped.
    pub(super) fn sync_now(&mut self) {
        let (Some(source), Some(shelf)) = (self.session.source.clone(), self.session.shelf.clone()) else { return };
        if self.sync.running {
            return;
        }
        if let Some(listening) = &self.listening {
            listening.send(crate::listening::Request::Reconcile);
        }
        self.sync.running = true;
        self.sync.applied = 0;
        self.sync.last = Some(Instant::now());
        self.sync.retry_at = None;
        self.bridge.sync(source, shelf);
    }

    /// The sync's outcome. Only what the user can act on is said; a rate limit
    /// and a network that went away are waited out quietly.
    pub(super) fn sync_done(&mut self, outcome: Outcome) {
        self.sync.running = false;
        self.sync.last = Some(Instant::now());
        match outcome {
            Outcome::Done(report) => {
                if self.sync.offline {
                    self.downloads.refreshed = false;
                }
                self.sync.offline = false;
                self.sync.synced = Some(std::time::SystemTime::now());
                // The libraries are the account's own: the shelf is read back
                // by them at the next start, before the server answers.
                if let Some(account) = self.session.account.as_mut() {
                    account.libraries = report.libraries.clone();
                }
                if let Some(id) = self.session.account.as_ref().map(|a| a.id) {
                    let libraries = report.libraries;
                    self.prefs.edit(move |s| {
                        if let Some(account) = s.account_by_id_mut(id) {
                            account.libraries = libraries;
                        }
                    });
                }
            }
            Outcome::Busy { retry_after } => {
                let wait = retry_after.unwrap_or(Duration::from_secs(60)).clamp(Duration::from_secs(5), Duration::from_secs(15 * 60));
                self.retry_in(wait);
            }
            Outcome::SignedOut => self.signed_out_and_say(SIGNED_OUT),
            Outcome::Failed(f) => {
                let network = f.is_network();
                self.show_failure(f);
                self.sync.offline = network;
                if network {
                    self.retry_in(Duration::from_secs(60));
                }
            }
        }
        self.download_connectivity();
    }

    /// Tries again in `wait`, drawing the window once when the time comes
    /// rather than every frame until then.
    fn retry_in(&mut self, wait: Duration) {
        self.sync.retry_at = Some(Instant::now() + wait);
        self.ctx.request_repaint_after(wait);
    }

    /// The sign-in form, in the first-run sheet's column. The server
    /// chooses which ways in the form offers; when it offers its provider,
    /// that leads (most accounts use it) and the password form follows under
    /// a divider, so one button is plainly the main one. `compact` is a short
    /// window: less space, and no help for an address the server has already
    /// answered at.
    pub(super) fn login_form(&mut self, ui: &mut egui::Ui, compact: bool) {
        let Some(form) = self.login.as_mut() else { return };
        let t = Tokens::of(ui);
        let waiting = self.openid.pending.as_ref();
        let exchanging = self.openid.exchanging;
        let password_busy = self.session.connecting;
        let mut action = None;
        let status = form.status.as_ref().filter(|_| form.checked_server == form.server.trim());
        let local = status.is_some_and(|s| s.auth_methods.is_empty() || s.auth_methods.iter().any(|m| m == "local"));
        let oidc = crate::openid::SUPPORTED && status.is_some_and(|s| s.auth_methods.iter().any(|m| m == "openid"));
        let oidc_label = status.and_then(|s| s.auth_form_data.auth_open_id_button_text.as_deref()).unwrap_or("Sign in with OpenID");
        let custom_message = status.and_then(|s| s.auth_form_data.auth_login_custom_message.as_deref()).filter(|m| !m.is_empty());
        // Which spinner turns, if any: each phase gets its own few seconds.
        let phase = match waiting {
            Some(_) if exchanging => Some(Phase::Finishing),
            Some(_) => Some(Phase::Browser),
            None if form.busy => Some(Phase::Server),
            None => None,
        };
        let spin = spin(ui, phase);
        if let Some(pending) = waiting {
            self.actions.extend(waiting_ui(ui, &t, pending, exchanging, spin));
            return;
        }
        if let Some(message) = custom_message {
            ui.add(egui::Label::new(egui::RichText::new(message).size(12.0).color(t.muted)).wrap());
            ui.add_space(14.0);
        }
        let mut enter = false;
        // The error, if there is one, goes under the field it's
        // about; one about the sign-in as a whole waits below
        // the buttons.
        let error = form.error.clone();
        let on = error.as_deref().and_then(error_field);
        let about = |field: Field| error.as_deref().filter(|_| on == Some(field));
        ui.add_enabled_ui(!form.busy, |ui| {
            enter |= kit::TextField::new(Field::Server.label())
                .hint("books.example.com")
                .focus(!form.focused)
                .error(about(Field::Server))
                .help((!(compact && status.is_some())).then_some("Your Audiobookshelf server's address"))
                .show(ui, &mut form.server);
            form.focused = true;
        });
        // The button that's waiting on the server says so; the
        // rest are greyed out until it answers.
        let form_busy = form.busy;
        let button = |ui: &mut egui::Ui, label: &str, primary: bool, busy: bool| {
            if form_busy && busy {
                kit::form_button(ui, "Connecting…", primary, Some(spin));
                false
            } else {
                ui.add_enabled_ui(!form_busy, |ui| kit::form_button(ui, label, primary, None)).inner.clicked()
            }
        };
        // The space above a button, less when room is short.
        let gap = if compact { 12.0 } else { 18.0 };
        if status.is_none() {
            ui.add_space(gap);
            if button(ui, "Sign in", true, true) || enter {
                action = Some(Action::CheckServer);
            }
        }
        if oidc {
            ui.add_space(gap);
            if button(ui, oidc_label, true, !password_busy) {
                action = Some(Action::OpenId);
            }
            if !local {
                ui.add_space(6.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("You'll finish signing in in your browser.").size(11.0).color(t.muted));
                });
            }
        }
        if local {
            if oidc {
                ui.add_space(if compact { 8.0 } else { 14.0 });
                kit::or_divider(ui);
                ui.add_space(if compact { 2.0 } else { 6.0 });
            } else {
                ui.add_space(10.0);
            }
            ui.add_enabled_ui(!form.busy, |ui| {
                enter |= kit::TextField::new(Field::Username.label()).error(about(Field::Username)).show(ui, &mut form.username);
                ui.add_space(10.0);
                enter |= kit::TextField::new(Field::Password.label()).password(true).error(about(Field::Password)).show(ui, &mut form.password);
            });
            ui.add_space(gap);
            if button(ui, "Sign in", !oidc, password_busy || !oidc) || enter {
                action = Some(Action::SignIn);
            }
        }
        if status.is_some() && !local && !oidc {
            ui.add_space(14.0);
            kit::notice(
                ui,
                "No way to sign in",
                if crate::openid::SUPPORTED {
                    "This server offers no sign-in method Rede supports. Ask its admin to turn on passwords or OpenID."
                } else {
                    "Browser sign-in is only available on macOS for now. Ask your admin to enable an Audiobookshelf password for this account."
                },
            );
        }
        if let Some(error) = &form.error
            && error_field(error).is_none()
        {
            ui.add_space(12.0);
            kit::field_error(ui, "Sign-in error", error);
        }
        if let Some(warning) = &form.warning {
            ui.add_space(14.0);
            kit::notice(ui, "Update recommended", warning);
        }
        // A server that takes the connection and then says
        // nothing would hold the form until the request times out.
        if form.busy {
            ui.add_space(16.0);
            ui.vertical_centered(|ui| {
                if crate::widgets::link(ui, "Cancel", egui::FontId::proportional(12.0), t.accent).clicked() {
                    action = Some(Action::CancelSignIn);
                }
            });
        }
        self.actions.extend(action);
    }

    /// Roughly how tall the sign-in form is, whatever the server offers.
    pub(super) fn login_form_height(&self) -> f32 {
        let Some(form) = self.login.as_ref() else { return 0.0 };
        let status = form.status.as_ref().filter(|_| form.checked_server == form.server.trim());
        let local = status.is_some_and(|s| s.auth_methods.is_empty() || s.auth_methods.iter().any(|m| m == "local"));
        let oidc = crate::openid::SUPPORTED && status.is_some_and(|s| s.auth_methods.iter().any(|m| m == "openid"));
        if let Some(pending) = self.openid.pending.as_ref() {
            if pending.flow.used_fallback() { 300.0 } else { 200.0 }
        } else if oidc && local {
            340.0
        } else if local {
            260.0
        } else {
            190.0
        }
    }

    /// Opens the pending browser sign-in's page again, for a tab that was
    /// closed or lost. It's the same transaction (the same state and
    /// verifier), so whichever tab finishes, the redirect still matches.
    pub(super) fn reopen_browser(&mut self) {
        if let Some(pending) = self.openid.pending.as_ref().filter(|_| !self.openid.exchanging) {
            self.ctx.open_url(egui::OpenUrl::new_tab(pending.flow.browser_url()));
        }
    }
}

/// Waiting for the browser: what's happening, a way out, and a way back to
/// the page if its tab went missing. Centred, since there's nothing to fill
/// in.
fn waiting_ui(ui: &mut egui::Ui, t: &Tokens, pending: &crate::openid::Pending, exchanging: bool, spin: bool) -> Option<Action> {
    let mut action = None;
    ui.vertical_centered(|ui| {
        ui.add_space(4.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
        // Still after a while: the browser may take minutes, and the window
        // shouldn't draw all that time to say so.
        shell::icons::paint(ui.painter(), rect, if spin { Icon::Spinner } else { Icon::Clock }, t.muted);
        ui.add_space(14.0);
        let title = if exchanging { "Signing in…" } else { "Finish signing in in your browser" };
        ui.label(egui::RichText::new(title).font(egui::FontId::new(15.0, theme::semibold())).color(t.text));
        ui.add_space(4.0);
        let host = pending.account.server.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/');
        let body = format!("Signing in to {host}. When you're done, your browser brings you back here.");
        ui.add(egui::Label::new(egui::RichText::new(body).size(12.0).color(t.muted)).wrap());
        ui.add_space(20.0);
        if shell::kit::secondary(ui, "Cancel").clicked() {
            action = Some(Action::CancelSignIn);
        }
        if !exchanging {
            ui.add_space(12.0);
            if crate::widgets::link(ui, "Open the browser again", egui::FontId::proportional(12.0), t.accent).clicked() {
                action = Some(Action::ReopenBrowser);
            }
        }
    });
    if pending.flow.used_fallback() {
        ui.add_space(22.0);
        kit::notice(ui, "Rede's link isn't allowed on this server", crate::openid::FALLBACK);
    }
    action
}

/// What the sign-in is waiting on.
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    /// The server, about its methods or a password.
    Server,
    /// The user, in the browser.
    Browser,
    /// The server, turning the browser's code into a login.
    Finishing,
}

/// Whether the sign-in spinner turns this frame. Each `phase` (checking the
/// server, waiting for the browser, finishing) turns it for a few seconds and
/// then lets it go still, so a request that never answers doesn't keep the
/// window drawing. `None` is nothing to wait for.
fn spin(ui: &egui::Ui, phase: Option<Phase>) -> bool {
    let key = signing_in_since();
    let Some(phase) = phase else {
        ui.data_mut(|d| d.remove::<(Phase, f64)>(key));
        return false;
    };
    let now = ui.input(|i| i.time);
    let (_, since) = ui.data_mut(|d| {
        let entry = d.get_temp_mut_or_insert_with(key, || (phase, now));
        if entry.0 != phase {
            *entry = (phase, now);
        }
        *entry
    });
    let turning = now - since < CONNECT_SPIN_SECS;
    if turning {
        ui.ctx().request_repaint_after(SPIN_FRAME);
    }
    turning
}

/// The field a sign-in error is about, so it can be shown under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Field {
    Server,
    Username,
    Password,
}

impl Field {
    fn label(self) -> &'static str {
        match self {
            Field::Server => "Server",
            Field::Username => "Username",
            Field::Password => "Password",
        }
    }
}

/// Which field an error is about. Only a mistyped or refused address or name
/// goes under a field; a rate limit, a server too old, a sign-in that timed
/// out or an account whose shelf wouldn't open is about the sign-in as a
/// whole and stays under the buttons.
fn error_field(error: &str) -> Option<Field> {
    if error == "Wrong username or password." || error == "Enter your password." {
        return Some(Field::Password);
    }
    if error == "Enter your username." {
        return Some(Field::Username);
    }
    let address = error.starts_with("Enter the server's address")
        || error.starts_with("That doesn't look like a server address")
        || error.starts_with("Couldn't reach the server")
        || error.contains("not like an Audiobookshelf server")
        || error.contains("instead of the server's API");
    address.then_some(Field::Server)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each way signing in fails, as `sign_in_error` words it, and the field
    /// it belongs under.
    #[test]
    fn each_kind_of_sign_in_error_finds_its_field() {
        let bad = crate::bridge::sign_in_error(&abs::Error::Auth(abs::AuthError::BadCredentials));
        assert_eq!(error_field(&bad), Some(Field::Password));
        let limited = crate::bridge::sign_in_error(&abs::Error::Auth(abs::AuthError::RateLimited { retry_after: Some(Duration::from_secs(90)) }));
        assert_eq!(error_field(&limited), None, "a rate limit is about the sign-in, not a field");
        let refused = crate::bridge::sign_in_error(&abs::Error::Server { status: 502, message: None });
        assert_eq!(error_field(&refused), None, "the server is up; it just refused");
        let signed_out = crate::bridge::sign_in_error(&abs::Error::Auth(abs::AuthError::SignedOut));
        assert_eq!(error_field(&signed_out), None);
        let url = crate::bridge::sign_in_error(&abs::Client::new("books example", abs::DeviceInfo::new("device", "0")).err().unwrap());
        assert_eq!(error_field(&url), Some(Field::Server));
        let page = crate::bridge::sign_in_error(&abs::Error::NotApi(Some("Sign in".into())));
        assert_eq!(error_field(&page), Some(Field::Server));
        let not_abs = crate::bridge::sign_in_error(&abs::Error::Decode("not Audiobookshelf".into()));
        assert_eq!(error_field(&not_abs), Some(Field::Server), "the address is the one to change");
        // The network, in the words `sign_in_error` gives it.
        assert_eq!(error_field("Couldn't reach the server. Check the address and that it's running."), Some(Field::Server));
        assert_eq!(error_field("Couldn't reach the server: connection reset"), Some(Field::Server));
        // What the form itself says about an empty field.
        assert_eq!(error_field("Enter the server's address, for example books.example.com."), Some(Field::Server));
        assert_eq!(error_field("Enter your username."), Some(Field::Username));
        assert_eq!(error_field("Enter your password."), Some(Field::Password));
        // And what is about the sign-in as a whole, which stays under the buttons.
        assert_eq!(error_field("This server is version 2.19.0. Rede needs Audiobookshelf 2.26 or newer."), None);
        assert_eq!(error_field(crate::failure::SIGNED_OUT), None);
        assert_eq!(error_field("Couldn't open this account's library: no room"), None);
        assert_eq!(error_field("cancelled"), None);
    }
}
