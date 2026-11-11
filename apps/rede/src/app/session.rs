//! The account and the shelf: signing in, coming back to a saved login, and
//! signing out. The sign-in form pushes [`Action::SignIn`]; everything that
//! talks to the server is the bridge's.
//!
//! Signing out forgets local tokens and asks the server to end its login as
//! a best effort. The shelf and downloaded art stay where they are, so signing
//! back in finds the library at once.

use super::{Action, RedeApp};
use crate::failure::SIGNED_OUT;
use crate::settings::{Account, SignInMethod};
use crate::sync::Outcome;
use shelf::Shelf;
use shell::icons::Icon;
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
        self.session = Session { account: Some(account.clone()), source: Some(source.clone()), shelf: Some(shelf.clone()), connecting: false };
        self.downloads.store.clear();
        self.downloads = Default::default();
        self.bridge.open_downloads(account.id, self.shown.downloads.limit, self.stream_dir.clone());
        self.login = None;
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
        if changed {
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
        let Some(form) = self.login.as_mut().filter(|f| !f.busy) else { return };
        if form.server.trim().is_empty() {
            form.error = Some("Enter the server's address, for example books.example.com.".into());
            return;
        }
        form.busy = true;
        form.status = None;
        form.error = None;
        form.checked_server = form.server.trim().to_owned();
        self.bridge.server_status(Account::new(&form.checked_server, ""));
    }

    pub(super) fn begin_openid(&mut self) {
        let Some(form) = self.login.as_mut().filter(|f| !f.busy && f.status.is_some() && f.server.trim() == f.checked_server) else { return };
        form.busy = true;
        form.error = None;
        let account =
            form.account.clone().map(|a| Account { server: form.checked_server.clone(), ..a }).unwrap_or_else(|| Account::new(&form.checked_server, ""));
        self.bridge.begin_openid(account, form.warning.clone());
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
                let launch = status.auth_methods.iter().any(|m| m == "openid") && status.auth_form_data.auth_open_id_auto_launch;
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
                let account = if account.username.is_empty() || account.username == username {
                    Account { username, ..account }
                } else {
                    Account::new(account.server, username)
                };
                let account = Account { sign_in: Some(SignInMethod::OpenId), ..account };
                self.bridge.accept_openid(account, client, login.tokens, warning);
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
        self.bridge.cancel_login();
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
        // The same user on a new address is the same account (its shelf and
        // progress stay); another user is another one.
        let account = match form.account.clone() {
            Some(a) if a.username == username => Account { server, username, ..a },
            _ => Account::new(server, username),
        };
        let account = Account { sign_in: Some(SignInMethod::Password), ..account };
        form.account = Some(account.clone());
        self.session.connecting = true;
        self.bridge.sign_in(account, form.password.clone());
    }

    /// Signs out: the tokens go, and nothing else. The sign-in screen comes
    /// back with the account filled in, and anything in flight is dropped.
    pub(super) fn sign_out(&mut self) {
        let Some(account) = self.session.account.clone().or_else(|| self.prefs.get(|s| s.account().cloned())) else { return };
        self.set_mode(crate::window::Mode::Full);
        self.playback_control(crate::player::Control::Stop);
        self.playback_generation += 1;
        self.opening = false;
        let finished = self.listening.take().map(crate::listening::Listening::finish);
        self.bridge.sign_out_after(account.clone(), self.session.source.clone(), finished);
        if let Some(engine) = self.engine.take() {
            engine.shutdown(Duration::from_secs(2));
        }
        self.player = crate::player::Player::default();
        self.positions.clear();
        self.bookmarks = super::bookmarks::State::default();
        self.reconciliation = None;
        self.ui.palette = None;
        self.session = Session::default();
        self.downloads.store.clear();
        self.downloads = Default::default();
        self.books.clear();
        self.art.set_source(None);
        self.sync = super::SyncState::default();
        self.ui.nav = super::Nav::default();
        self.ui.notice = None;
        self.login = Some(LoginForm::for_account(Some(&account)));
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

    /// The sign-in screen. The server chooses which ways in the form offers;
    /// when it offers its provider, that leads (most accounts use it) and the
    /// password form follows under a divider, so one button is plainly the
    /// main one.
    pub(super) fn login_ui(&mut self, ui: &mut egui::Ui) {
        let Some(form) = self.login.as_mut() else { return };
        let t = Tokens::of(ui);
        let waiting = self.openid.pending.as_ref();
        let exchanging = self.openid.exchanging;
        let password_busy = self.session.connecting;
        let mut action = None;
        let status = form.status.as_ref().filter(|_| form.checked_server == form.server.trim());
        let local = status.is_some_and(|s| s.auth_methods.is_empty() || s.auth_methods.iter().any(|m| m == "local"));
        let oidc = status.is_some_and(|s| s.auth_methods.iter().any(|m| m == "openid"));
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
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg)).show(ui, |ui| {
            let strip = egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(ui.max_rect().width(), super::BAR_H));
            let bar = ui.interact(strip, ui.scope_id().with("login-titlebar"), egui::Sense::click_and_drag());
            if bar.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if bar.double_clicked() {
                shell::chrome::title_double_click(ui.ctx());
            }
            // Roughly how tall the column is, so it sits a little above the
            // middle whatever the server offers.
            let form_h = if let Some(pending) = waiting {
                if pending.flow.used_fallback() { 360.0 } else { 260.0 }
            } else if oidc && local {
                400.0
            } else if local {
                320.0
            } else {
                250.0
            };
            let room = ui.available_height();
            // A short window keeps the buttons in view: a smaller heading, and
            // no help for an address the server has already answered at.
            let compact = room < form_h + 160.0;
            // Shorter still, it scrolls rather than cutting the error off.
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.add_space(((room - form_h) * 0.42).max(if compact { 12.0 } else { 24.0 }));
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("Rede").font(egui::FontId::new(if compact { 22.0 } else { 28.0 }, theme::semibold())).color(t.text));
                    if !compact {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new("Sign in to Audiobookshelf").size(13.0).color(t.muted));
                    }
                    ui.add_space(if compact { 14.0 } else { 24.0 });
                });
                ui.horizontal(|ui| {
                    ui.add_space(((ui.available_width() - FORM_W) / 2.0).max(0.0));
                    ui.vertical(|ui| {
                        ui.set_width(FORM_W);
                        ui.spacing_mut().item_spacing.y = 4.0;
                        if let Some(pending) = waiting {
                            action = waiting_ui(ui, &t, pending, exchanging, spin);
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
                        ui.add_enabled_ui(!form.busy, |ui| {
                            enter |= login_field(ui, &t, Field::Server, &mut form.server, "books.example.com", !form.focused, on == Some(Field::Server));
                            form.focused = true;
                            if on == Some(Field::Server)
                                && let Some(error) = &error
                            {
                                field_error(ui, &t, Field::Server, error);
                            }
                            if !(compact && status.is_some()) {
                                ui.label(egui::RichText::new("Your Audiobookshelf server's address").size(11.0).color(t.muted));
                            }
                        });
                        // The button that's waiting on the server says so; the
                        // rest are greyed out until it answers.
                        let form_busy = form.busy;
                        let button = |ui: &mut egui::Ui, label: &str, primary: bool, busy: bool| {
                            if form_busy && busy {
                                form_button(ui, label, primary, Some(spin));
                                false
                            } else {
                                ui.add_enabled_ui(!form_busy, |ui| form_button(ui, label, primary, None)).inner.clicked()
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
                                or_divider(ui, &t);
                                ui.add_space(if compact { 2.0 } else { 6.0 });
                            } else {
                                ui.add_space(10.0);
                            }
                            ui.add_enabled_ui(!form.busy, |ui| {
                                enter |= login_field(ui, &t, Field::Username, &mut form.username, "", false, on == Some(Field::Username));
                                if on == Some(Field::Username)
                                    && let Some(error) = &error
                                {
                                    field_error(ui, &t, Field::Username, error);
                                }
                                ui.add_space(10.0);
                                enter |= login_field(ui, &t, Field::Password, &mut form.password, "", false, on == Some(Field::Password));
                                if on == Some(Field::Password)
                                    && let Some(error) = &error
                                {
                                    field_error(ui, &t, Field::Password, error);
                                }
                            });
                            ui.add_space(gap);
                            if button(ui, "Sign in", !oidc, password_busy || !oidc) || enter {
                                action = Some(Action::SignIn);
                            }
                        }
                        if status.is_some() && !local && !oidc {
                            ui.add_space(14.0);
                            notice(
                                ui,
                                &t,
                                "No way to sign in",
                                "This server offers no sign-in method Rede supports. Ask its admin to turn on passwords or OpenID.",
                            );
                        }
                        if let Some(error) = &form.error
                            && error_field(error).is_none()
                        {
                            ui.add_space(12.0);
                            let shown = ui.add(egui::Label::new(egui::RichText::new(error).size(12.0).color(t.danger)).wrap());
                            shown.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, format!("Sign-in error: {error}")));
                        }
                        if let Some(warning) = &form.warning {
                            ui.add_space(14.0);
                            notice(ui, &t, "Update recommended", warning);
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
                    });
                });
                ui.add_space(24.0);
            });
        });
        if let Some(action) = action {
            self.actions.push(action);
        }
        for action in std::mem::take(&mut self.actions) {
            self.apply(action);
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

/// The sign-in column's width, Ond's.
const FORM_W: f32 = 320.0;

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
        notice(ui, t, "Rede's link isn't allowed on this server", crate::openid::FALLBACK);
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

/// One of the form's full-width buttons: the accent fill for the main way in,
/// the quiet field fill for the other. `busy` is `Some` on the one waiting
/// for the server, which says so (with a spinner while `Some(true)`) and takes
/// no clicks.
fn form_button(ui: &mut egui::Ui, label: &str, primary: bool, busy: Option<bool>) -> egui::Response {
    let t = Tokens::of(ui);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), if busy.is_some() { egui::Sense::hover() } else { egui::Sense::click() });
    let hovered = resp.hovered() && busy.is_none();
    let (fill, ink) = match (primary, busy.is_some(), hovered) {
        (true, true, _) => (t.accent.gamma_multiply(0.6), t.on_accent),
        (true, false, true) => (t.accent.gamma_multiply(0.88), t.on_accent),
        (true, false, false) => (t.accent, t.on_accent),
        (false, _, true) => (t.selected, t.accent),
        (false, _, false) => (t.field, t.accent),
    };
    let text = if busy.is_some() { "Connecting…" } else { label };
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect_filled(rect, 7.0, fill);
        // A long provider name is cut rather than spilling out of the button.
        let galley = crate::widgets::elide(ui, text, egui::FontId::new(13.0, theme::semibold()), ink, rect.width() - 56.0);
        p.galley(rect.center() - galley.size() / 2.0, galley, ink);
        if busy == Some(true) {
            let r = egui::Rect::from_center_size(egui::pos2(rect.left() + 18.0, rect.center().y), egui::vec2(14.0, 14.0));
            shell::icons::paint(p, r, Icon::Spinner, ink);
        }
        shell::kit::focus_ring(ui, &resp, rect, 7.0);
    }
    let (text, enabled) = (text.to_owned(), ui.is_enabled() && busy.is_none());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &text));
    if busy.is_none() { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
}

/// A hairline either side of a small "or", between the provider and the
/// password.
fn or_divider(ui: &mut egui::Ui, t: &Tokens) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 18.0), egui::Sense::hover());
    let p = ui.painter();
    let galley = p.layout_no_wrap("or".to_owned(), egui::FontId::proportional(11.0), t.muted);
    let gap = galley.size().x / 2.0 + 10.0;
    let (y, mid) = (rect.center().y, rect.center().x);
    let stroke = egui::Stroke::new(1.0, t.hairline);
    p.hline(rect.left()..=mid - gap, y, stroke);
    p.hline(mid + gap..=rect.right(), y, stroke);
    p.galley(rect.center() - galley.size() / 2.0, galley, t.muted);
}

/// Something the form wants read but can't act on, outlined as Ond outlines
/// its notices: a short title, then the detail in muted text.
fn notice(ui: &mut egui::Ui, t: &Tokens, title: &str, body: &str) {
    egui::Frame::new().stroke(egui::Stroke::new(1.0, t.hairline)).corner_radius(8).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(egui::RichText::new(title).font(egui::FontId::new(12.5, theme::semibold())).color(t.text));
        ui.add(egui::Label::new(egui::RichText::new(body).size(12.0).color(t.muted)).wrap());
    });
}

fn login_field(ui: &mut egui::Ui, t: &Tokens, field: Field, text: &mut String, hint: &str, focus: bool, refused: bool) -> bool {
    let label = field.label();
    ui.label(egui::RichText::new(label).font(egui::FontId::new(12.0, theme::semibold())).color(t.muted));
    let r = ui.add(
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .password(field == Field::Password)
            .desired_width(f32::INFINITY)
            .margin(egui::vec2(10.0, 7.0))
            .background_color(t.field),
    );
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, label));
    // The border says which field the error below it is about.
    if refused {
        ui.painter().rect_stroke(r.rect, 4.0, egui::Stroke::new(1.0, t.danger), egui::StrokeKind::Inside);
    }
    if focus {
        r.request_focus();
    }
    r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
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

/// What went wrong with one field, under it in the danger colour and named
/// with the field, so a screen reader says which one it's about.
fn field_error(ui: &mut egui::Ui, t: &Tokens, field: Field, error: &str) {
    let resp = ui.add(egui::Label::new(egui::RichText::new(error).size(12.0).color(t.danger)).wrap());
    let name = format!("{}: {error}", field.label());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &name));
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
