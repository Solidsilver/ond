//! The server connection and who's signed in: the login form, connecting,
//! going offline and coming back, and signing out.

use super::{OndApp, toolbar};
use crate::favorites::Favorites;
use crate::nav::Nav;
use crate::settings::Settings;
use crate::theme::{self, Tokens};
use std::sync::Arc;
use std::time::Duration;
use subsonic::Client;

#[derive(Default)]
pub(super) struct LoginForm {
    pub(super) server: String,
    pub(super) username: String,
    pub(super) password: String,
    pub(super) busy: bool,
    pub(super) error: Option<String>,
}

/// The server connection and who's signed in.
#[derive(Default)]
pub struct Session {
    pub client: Option<Client>,
    /// The server answered last time we asked. While it's unreachable only
    /// downloaded tracks play.
    pub online: bool,
    /// The server stopped answering and reconnecting runs in the background
    /// (as opposed to the first connect still under way).
    pub unreachable: bool,
    /// The login in use, for reconnecting.
    pub settings: Option<Settings>,
    /// "user · navidrome 0.64.2", or "user · offline".
    pub server_label: String,
    /// "navidrome 0.64.2", as the server reported it.
    pub server_info: String,
}

impl Session {
    /// The client, while the server answers.
    pub fn live(&self) -> Option<&Client> {
        self.client.as_ref().filter(|_| self.online)
    }
}

impl OndApp {
    /// Signed in: the session starts (or resumes after an outage).
    pub(super) fn connected(&mut self, client: Client, settings: Box<Settings>, server: String, new_account: bool) {
        if new_account {
            self.forget_library();
        }
        let limit = self.prefs.snapshot().offline_limit_gb.map(|gb| gb * 1_000_000_000);
        self.downloads.switch_account(settings.account_id(), limit, &self.bridge);
        self.session.server_label = format!("{} · {}", settings.username, server);
        self.session.server_info = server.clone();
        self.login = None;
        // Always a new pass, even if the flags say one runs: any still
        // running is from before the server went away, and the bridge
        // has this one replace it.
        self.sync.label = "syncing…".into();
        self.sync.albums = true;
        self.bridge.sync_albums(client.clone());
        self.bridge.fetch_home(client.clone());
        self.art.set_client(client.clone());
        self.now_playing.set_client(client.clone());
        self.output.connected(&settings.server);
        // A store still opening gets the server once it's open.
        self.connect_downloads(&client);
        let url = client.clone();
        self.analysis.runner.set_server(Some(Arc::new(move |id: &str| url.stream_url(id).to_string())));
        self.bridge.fetch_starred(Some(client.clone()));
        self.diagnostics.latency_asked();
        self.bridge.measure_latency(client.clone());
        self.session.unreachable = false;
        self.session.client = Some(client);
        self.prefs.set_login(&settings.server, &settings.username);
        self.session.settings = Some(*settings);
        if !self.session.online {
            self.session.online = true;
            // Lined up while offline; the server can supply the rest again.
            self.player.forget_next();
            self.sync_next();
        }
        self.resume_library_analysis();
    }

    /// The sign-in failed: back to the form, with the error.
    pub(super) fn connect_failed(&mut self, e: String) {
        self.set_offline();
        let form = self.login.get_or_insert_with(|| {
            let s = self.session.settings.clone().unwrap_or_default();
            LoginForm { server: s.server, username: s.username, ..Default::default() }
        });
        form.busy = false;
        form.error = Some(e);
    }

    /// The server stopped answering: carry on offline while reconnecting.
    pub(super) fn unreachable(&mut self, username: String, error: String) {
        tracing::info!("server unreachable: {error}");
        self.login = None;
        self.set_offline();
        self.session.unreachable = true;
        self.diagnostics.set_latency(None);
        self.session.server_label = format!("{username} · offline");
        self.sync.label = "reconnecting…".into();
    }

    /// Measures the server's round trip again if the last reading is old
    /// (and none is on its way).
    pub(super) fn refresh_latency(&mut self) {
        if self.session.online
            && self.diagnostics.latency_stale()
            && let Some(client) = self.session.client.clone()
        {
            self.diagnostics.latency_asked();
            self.bridge.measure_latency(client);
        }
    }

    /// Something failed; if it was the network, find out whether the server is gone.
    pub(super) fn probe(&self) {
        if self.session.online
            && let (Some(client), Some(settings)) = (&self.session.client, &self.session.settings)
        {
            self.bridge.probe(client.clone(), settings.clone());
        }
    }

    /// Another server or user signed in: what was shown came from the old one.
    /// The library database was cleared already; this drops the in-memory copy,
    /// the queue and the cached audio, which a sync then refills.
    pub(super) fn forget_library(&mut self) {
        self.with_player(|p, cx| p.reset(cx));
        self.data.clear();
        self.search.stale = true;
        self.pending.clear();
        self.favorites = Favorites::default();
        self.lyrics.clear();
        self.song_stats.clear();
        self.ui.nav = Nav::default();
        self.cache.clear();
    }

    /// The server's gone: browse what's stored, play what's downloaded.
    pub(super) fn set_offline(&mut self) {
        self.session.online = false;
        self.session.client = None;
        if let Some(store) = &self.downloads.store {
            store.set_source(None);
        }
        self.sync.tracks = false;
        self.sync.albums = false;
        self.sync.progress = None;
    }

    /// Stops playback, forgets the saved password and shows the login screen
    /// with the server and username filled in.
    pub(super) fn sign_out(&mut self) {
        self.with_player(|p, cx| p.reset(cx));
        self.ui.palette = None;
        self.ui.overlay = None;
        self.favorites = Favorites::default();
        // Another account may sign in next (ids can collide), and a fetch still
        // in flight now delivers nothing, so its `Loading` would stay for good.
        self.lyrics.clear();
        self.song_stats.clear();
        self.bridge.cancel_sync();
        // Reconnect loops and connects in flight stop; anything they still
        // deliver is ignored. Downloads lose their server (and its credentials).
        self.bridge.end_session();
        // So does analysis: the old account's queue goes (a library-wide run
        // resumes for whoever signs in next).
        self.analysis.runner.set_server(None);
        self.analysis.runner.clear();
        self.analysis.library_run = false;
        self.set_offline();
        // Not in `set_offline`: going unreachable sets these for the outage,
        // and a failed sign-in leaves them be.
        self.session.unreachable = false;
        self.session.server_label.clear();
        let saved = self.prefs.snapshot();
        if self.prefs.has_login() {
            let s = saved.clone();
            std::thread::spawn(move || s.forget_password());
        }
        self.prefs.edit(|s| s.signed_out = true);
        self.login = Some(LoginForm { server: saved.server, username: saved.username, ..Default::default() });
    }

    pub(super) fn login_ui(&mut self, ui: &mut egui::Ui) {
        let Some(form) = self.login.as_mut() else { return };
        let locked = (self.prefs.managed("server"), self.prefs.managed("username"));
        let mut submit = false;
        let t = Tokens::of(ui);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg)).show(ui, |ui| {
            // The top strip is the title bar here too.
            let strip = egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(ui.max_rect().width(), toolbar::TOOLBAR_H));
            let bar = ui.interact(strip, ui.scope_id().with("login-titlebar"), egui::Sense::click_and_drag());
            if bar.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if bar.double_clicked() {
                crate::chrome::title_double_click(ui.ctx());
            }
            const W: f32 = 320.0;
            const FORM_H: f32 = 330.0;
            ui.add_space(((ui.available_height() - FORM_H) * 0.42).max(24.0));
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("Ond").font(egui::FontId::new(28.0, theme::semibold())).color(t.text));
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Connect to your Navidrome server").size(13.0).color(t.muted));
                ui.add_space(24.0);
            });
            ui.horizontal(|ui| {
                ui.add_space(((ui.available_width() - W) / 2.0).max(0.0));
                ui.vertical(|ui| {
                    ui.set_width(W);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    let mut enter = false;
                    ui.add_enabled_ui(!form.busy, |ui| {
                        let field = |ui: &mut egui::Ui, label: &str, text: &mut String, hint: &str, password: bool, locked: bool| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(label).font(egui::FontId::new(12.0, theme::semibold())).color(t.muted));
                                if locked {
                                    ui.label(egui::RichText::new(crate::managed::LOCKED).size(11.0).color(t.muted));
                                }
                            });
                            let r = ui.add_enabled(
                                !locked,
                                egui::TextEdit::singleline(text)
                                    .hint_text(hint)
                                    .password(password)
                                    .desired_width(W)
                                    .margin(egui::vec2(10.0, 7.0))
                                    .background_color(t.field),
                            );
                            r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        };
                        enter |= field(ui, "Server", &mut form.server, "https://music.example.com", false, locked.0);
                        ui.label(egui::RichText::new("Your Navidrome address, for example https://music.example.com").size(11.0).color(t.muted));
                        ui.add_space(10.0);
                        enter |= field(ui, "Username", &mut form.username, "", false, locked.1);
                        ui.add_space(10.0);
                        enter |= field(ui, "Password", &mut form.password, "", true, false);
                    });
                    ui.add_space(18.0);
                    let label = if form.busy { "Connecting…" } else { "Connect" };
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(W, 34.0), if form.busy { egui::Sense::hover() } else { egui::Sense::click() });
                    let fill = if form.busy { t.accent.gamma_multiply(0.6) } else if resp.hovered() { t.accent.gamma_multiply(0.88) } else { t.accent };
                    ui.painter().rect_filled(rect, 7.0, fill);
                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::new(13.0, theme::semibold()), t.on_accent);
                    if form.busy {
                        let r = egui::Rect::from_center_size(egui::pos2(rect.left() + 18.0, rect.center().y), egui::vec2(14.0, 14.0));
                        crate::icons::paint(ui.painter(), r, crate::icons::Icon::Spinner, t.on_accent);
                        ui.ctx().request_repaint_after(Duration::from_millis(33));
                    }
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
                    submit = resp.clicked() || enter;
                    if let Some(e) = &form.error {
                        ui.add_space(12.0);
                        ui.add(egui::Label::new(egui::RichText::new(e).size(12.0).color(ui.visuals().error_fg_color)).wrap());
                    }
                });
            });
        });
        if submit && !form.busy {
            form.busy = true;
            form.error = None;
            let settings = Settings {
                server: form.server.trim().to_owned(),
                username: form.username.trim().to_owned(),
                signed_out: false,
                ..self.prefs.snapshot()
            };
            self.bridge.connect(settings, Some(std::mem::take(&mut form.password)));
        }
    }
}
