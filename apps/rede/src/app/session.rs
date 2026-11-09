//! The account and the shelf: signing in, coming back to a saved login, and
//! signing out. The sign-in form pushes [`Action::SignIn`]; everything that
//! talks to the server is the bridge's.
//!
//! Signing out forgets local tokens and asks the server to end its login as
//! a best effort. The shelf and downloaded art stay where they are, so signing
//! back in finds the library at once.

use super::{Action, RedeApp};
use crate::failure::SIGNED_OUT;
use crate::settings::Account;
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
        ));
        self.prefs.edit(|s| s.put_account(account.clone()));
        if changed {
            self.books.clear();
            self.ui.nav = super::Nav::default();
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
            form.error = Some("Enter the server's address, for example https://books.example.com.".into());
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
            AccountMsg::Status { session, status, warning } if session == self.bridge.session() => {
                let Some(form) = self.login.as_mut() else { return };
                form.busy = false;
                form.warning = warning;
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
            form.error = Some("Enter the server's address, for example https://books.example.com.".into());
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
        self.session = Session::default();
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
                if network {
                    self.retry_in(Duration::from_secs(60));
                }
            }
        }
    }

    /// Tries again in `wait`, drawing the window once when the time comes
    /// rather than every frame until then.
    fn retry_in(&mut self, wait: Duration) {
        self.sync.retry_at = Some(Instant::now() + wait);
        self.ctx.request_repaint_after(wait);
    }

    /// What the sync says while it runs, for the title bar and the empty state.
    pub(super) fn sync_label(&self) -> Option<String> {
        if self.sync.running {
            return Some(if self.sync.applied > 0 { format!("Syncing… {} books", self.sync.applied) } else { "Syncing…".to_owned() });
        }
        self.session.connecting.then(|| "Connecting…".to_owned())
    }

    /// The server chooses which credential controls the form offers.
    pub(super) fn login_ui(&mut self, ui: &mut egui::Ui) {
        let Some(form) = self.login.as_mut() else { return };
        let t = Tokens::of(ui);
        let waiting = self.openid.pending.as_ref();
        let mut action = None;
        let status = form.status.as_ref().filter(|_| form.checked_server == form.server.trim());
        let local = status.is_some_and(|s| s.auth_methods.is_empty() || s.auth_methods.iter().any(|m| m == "local"));
        let oidc = status.is_some_and(|s| s.auth_methods.iter().any(|m| m == "openid"));
        let oidc_first = status.is_some_and(|s| s.auth_form_data.auth_open_id_auto_launch);
        let oidc_label = status.and_then(|s| s.auth_form_data.auth_open_id_button_text.as_deref()).unwrap_or("Sign in with OpenID");
        let custom_message = status.and_then(|s| s.auth_form_data.auth_login_custom_message.as_deref());
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg)).show(ui, |ui| {
            let strip = egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(ui.max_rect().width(), super::BAR_H));
            let bar = ui.interact(strip, ui.scope_id().with("login-titlebar"), egui::Sense::click_and_drag());
            if bar.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if bar.double_clicked() {
                shell::chrome::title_double_click(ui.ctx());
            }
            const W: f32 = 340.0;
            ui.add_space(((ui.available_height() - 420.0) * 0.42).max(24.0));
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("Rede").font(egui::FontId::new(28.0, theme::semibold())).color(t.text));
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Sign in to Audiobookshelf").size(13.0).color(t.muted));
                ui.add_space(24.0);
            });
            ui.horizontal(|ui| {
                ui.add_space(((ui.available_width() - W) / 2.0).max(0.0));
                ui.vertical(|ui| {
                    ui.set_width(W);
                    ui.spacing_mut().item_spacing.y = 8.0;
                    if let Some(pending) = waiting {
                        ui.label(egui::RichText::new("Finish signing in in your browser").color(t.text));
                        if !pending.remaining(std::time::SystemTime::now()).is_zero() {
                            ui.horizontal(|ui| {
                                let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                                shell::icons::paint(ui.painter(), rect, Icon::Spinner, t.muted);
                                ui.label(egui::RichText::new("Waiting for your browser…").size(12.0).color(t.muted));
                            });
                            ui.ctx().request_repaint_after(SPIN_FRAME);
                        }
                        if pending.flow.used_fallback() {
                            ui.add(egui::Label::new(egui::RichText::new(crate::openid::FALLBACK).size(12.0).color(t.muted)).wrap());
                        }
                        if ui.button("Cancel").clicked() {
                            action = Some(Action::CancelSignIn);
                        }
                        return;
                    }
                    if let Some(message) = custom_message.filter(|m| !m.is_empty()) {
                        ui.add(egui::Label::new(message).wrap());
                    }
                    let mut enter = false;
                    ui.add_enabled_ui(!form.busy, |ui| {
                        enter |= login_field(ui, &t, "Server", &mut form.server, false, !form.focused);
                        form.focused = true;
                        ui.label(egui::RichText::new("Your Audiobookshelf server's address").size(11.0).color(t.muted));
                        let openid_button = |ui: &mut egui::Ui| {
                            if ui.add_sized([W, 34.0], egui::Button::new(egui::RichText::new(oidc_label).color(t.on_accent)).fill(t.accent)).clicked() {
                                Some(Action::OpenId)
                            } else {
                                None
                            }
                        };
                        if oidc && (oidc_first || !local) {
                            action = openid_button(ui);
                        }
                        if local {
                            enter |= login_field(ui, &t, "Username", &mut form.username, false, false);
                            enter |= login_field(ui, &t, "Password", &mut form.password, true, false);
                            if ui.add_sized([W, 34.0], egui::Button::new(egui::RichText::new("Sign in").color(t.on_accent)).fill(t.accent)).clicked() || enter {
                                action = Some(Action::SignIn);
                            }
                        }
                        if oidc && local && !oidc_first {
                            action = openid_button(ui).or(action.take());
                        }
                        if status.is_none()
                            && (ui.add_sized([W, 34.0], egui::Button::new(egui::RichText::new("Sign in").color(t.on_accent)).fill(t.accent)).clicked() || enter)
                        {
                            action = Some(Action::CheckServer);
                        }
                        if status.is_some() && !local && !oidc {
                            ui.label("This server offers no supported sign-in method.");
                        }
                    });
                    if form.busy {
                        ui.label("Connecting…");
                        let now = ui.input(|i| i.time);
                        let since = ui.data_mut(|d| *d.get_temp_mut_or_insert_with(signing_in_since(), || now));
                        if now - since < CONNECT_SPIN_SECS {
                            ui.ctx().request_repaint_after(SPIN_FRAME);
                        }
                        if ui.button("Cancel").clicked() {
                            action = Some(Action::CancelSignIn);
                        }
                    } else {
                        ui.data_mut(|d| d.remove::<f64>(signing_in_since()));
                    }
                    if let Some(warning) = &form.warning {
                        ui.add(egui::Label::new(egui::RichText::new(warning).size(12.0).color(t.muted)).wrap());
                    }
                    if let Some(error) = &form.error {
                        ui.add(egui::Label::new(egui::RichText::new(error).size(12.0).color(ui.visuals().error_fg_color)).wrap());
                    }
                });
            });
        });
        if let Some(action) = action {
            self.actions.push(action);
        }
        for action in std::mem::take(&mut self.actions) {
            self.apply(action);
        }
    }
}

fn login_field(ui: &mut egui::Ui, t: &Tokens, label: &str, text: &mut String, password: bool, focus: bool) -> bool {
    ui.label(egui::RichText::new(label).font(egui::FontId::new(12.0, theme::semibold())).color(t.muted));
    let r = ui.add(egui::TextEdit::singleline(text).password(password).desired_width(f32::INFINITY).margin(egui::vec2(10.0, 7.0)).background_color(t.field));
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, label));
    if focus {
        r.request_focus();
    }
    r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}
