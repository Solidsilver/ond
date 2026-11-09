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
        self.art.set_source(Some(source));
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
        let account = self.session.account.clone().or_else(|| self.prefs.get(|s| s.account().cloned()));
        self.login = Some(LoginForm { error, ..LoginForm::for_account(account.as_ref()) });
        self.session.connecting = false;
    }

    /// Connects with what the form holds.
    pub(super) fn sign_in(&mut self) {
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
        self.bridge.sign_out(account.clone(), self.session.source.clone());
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

    /// The sign-in screen: the app's name, three fields and the button, in the
    /// family's quiet look.
    pub(super) fn login_ui(&mut self, ui: &mut egui::Ui) {
        let locked = false;
        let Some(form) = self.login.as_mut() else { return };
        let t = Tokens::of(ui);
        let mut submit = false;
        let first = if !form.focused && !form.busy {
            form.focused = true;
            Some(if form.server.trim().is_empty() {
                0
            } else if form.username.trim().is_empty() {
                1
            } else {
                2
            })
        } else {
            None
        };
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg)).show(ui, |ui| {
            // The top strip is the title bar here too.
            let strip = egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(ui.max_rect().width(), super::BAR_H));
            let bar = ui.interact(strip, ui.scope_id().with("login-titlebar"), egui::Sense::click_and_drag());
            if bar.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if bar.double_clicked() {
                shell::chrome::title_double_click(ui.ctx());
            }
            const W: f32 = 320.0;
            const FORM_H: f32 = 330.0;
            ui.add_space(((ui.available_height() - FORM_H) * 0.42).max(24.0));
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
                    ui.spacing_mut().item_spacing.y = 4.0;
                    let mut enter = false;
                    ui.add_enabled_ui(!form.busy, |ui| {
                        let field = |ui: &mut egui::Ui, label: &str, text: &mut String, hint: &str, password: bool, focus: bool| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(label).font(egui::FontId::new(12.0, theme::semibold())).color(t.muted));
                                if locked {
                                    ui.label(egui::RichText::new("Managed by your system settings").size(11.0).color(t.muted));
                                }
                            });
                            if locked {
                                egui::Frame::new()
                                    .fill(t.field)
                                    .corner_radius(ui.visuals().widgets.inactive.corner_radius)
                                    .inner_margin(egui::Margin::symmetric(10, 7))
                                    .show(ui, |ui| {
                                        ui.set_width(W - 20.0);
                                        ui.add(egui::Label::new(egui::RichText::new(text.as_str()).color(t.text)).truncate());
                                    });
                                return false;
                            }
                            let r = ui.add(
                                egui::TextEdit::singleline(text)
                                    .hint_text(hint)
                                    .password(password)
                                    .desired_width(W)
                                    .margin(egui::vec2(10.0, 7.0))
                                    .background_color(t.field),
                            );
                            // The label above is painted text; a field still
                            // needs its own name where a screen reader looks.
                            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, label));
                            if focus {
                                r.request_focus();
                            }
                            r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        };
                        enter |= field(ui, "Server", &mut form.server, "https://books.example.com", false, first == Some(0));
                        if !locked {
                            ui.label(egui::RichText::new("Your server's address, for example https://books.example.com").size(11.0).color(t.muted));
                        }
                        ui.add_space(10.0);
                        enter |= field(ui, "Username", &mut form.username, "", false, first == Some(1));
                        ui.add_space(10.0);
                        enter |= field(ui, "Password", &mut form.password, "", true, first == Some(2));
                    });
                    ui.add_space(18.0);
                    let label = if form.busy { "Connecting…" } else { "Sign in" };
                    // Painted as Ond paints Connect: the label centred, a
                    // lighter fill on hover, a faded one while connecting.
                    let sense = if form.busy { egui::Sense::hover() } else { egui::Sense::click() };
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(W, 34.0), sense);
                    let fill = if form.busy {
                        t.accent.gamma_multiply(0.6)
                    } else if resp.hovered() {
                        t.accent.gamma_multiply(0.88)
                    } else {
                        t.accent
                    };
                    ui.painter().rect_filled(rect, 7.0, fill);
                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::new(13.0, theme::semibold()), t.on_accent);
                    let ready = !form.busy;
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ready, label));
                    if form.busy {
                        let r = egui::Rect::from_center_size(egui::pos2(resp.rect.left() + 18.0, resp.rect.center().y), egui::vec2(14.0, 14.0));
                        shell::icons::paint(ui.painter(), r, Icon::Spinner, t.on_accent);
                        // A server that takes the connection and says nothing
                        // gives up after its timeout; until then the spinner
                        // turns, but not for ever.
                        let now = ui.input(|i| i.time);
                        let since = ui.data_mut(|d| *d.get_temp_mut_or_insert_with(signing_in_since(), || now));
                        if now - since < CONNECT_SPIN_SECS {
                            ui.ctx().request_repaint_after(SPIN_FRAME);
                        }
                    } else {
                        ui.data_mut(|d| d.remove::<f64>(signing_in_since()));
                    }
                    submit = resp.clicked() || enter;
                    if let Some(e) = &form.error {
                        ui.add_space(12.0);
                        ui.add(egui::Label::new(egui::RichText::new(e).size(12.0).color(ui.visuals().error_fg_color)).wrap());
                    }
                });
            });
        });
        if submit && !form.busy {
            self.actions.push(Action::SignIn);
        }
        // What the form asked for, applied once it's drawn.
        for a in std::mem::take(&mut self.actions) {
            self.apply(a);
        }
    }
}
