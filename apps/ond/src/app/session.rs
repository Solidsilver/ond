//! The server connection and who's signed in: the login form (drawn in the
//! first-run sheet, `onboarding`), connecting, going offline and coming
//! back, and signing out.

use super::OndApp;
use crate::favorites::Favorites;
use crate::nav::Nav;
use crate::settings::Settings;
use crate::source::Src;
use crate::theme::Tokens;
use shell::kit;
use std::sync::Arc;
use std::time::Duration;

#[derive(Default)]
pub(super) struct LoginForm {
    pub(super) server: String,
    pub(super) username: String,
    pub(super) password: String,
    pub(super) busy: bool,
    pub(super) error: Option<String>,
    /// Opened over the library (signing in again): it can be closed to keep
    /// browsing what's stored.
    pub(super) closable: bool,
    /// The first empty field was given focus: once, so the user can move it.
    pub(super) focused: bool,
}

/// The server connection and who's signed in.
#[derive(Default)]
pub struct SourceSession {
    pub sync: super::SyncState,
    pub source: Option<Src>,
    /// The server answered last time we asked. While it's unreachable only
    /// downloaded tracks play.
    pub online: bool,
    /// The server stopped answering and reconnecting runs in the background
    /// (as opposed to the first connect still under way).
    pub unreachable: bool,
    /// The saved login is being tried: the server should answer soon, so
    /// nothing is treated as offline yet.
    pub connecting: bool,
    /// The server turned the saved login away (password changed, nothing in
    /// the keychain): why. The library stays up; the footer asks to sign in.
    pub sign_in: Option<String>,
    /// The login in use, for reconnecting.
    pub settings: Option<Settings>,
    /// "user · navidrome 0.64.2", or "user · offline".
    pub server_label: String,
    /// "navidrome 0.64.2", as the server reported it.
    pub server_info: String,
    /// Local source folders missing or unreadable on the last scan.
    pub folder_missing: Vec<std::path::PathBuf>,
    /// Files the last scan of local folders couldn't read.
    pub skipped: usize,
}

/// All configured sources stay live independently. `focus` selects the source
/// whose controls are being handled; browsing scope never ends a session.
#[derive(Default)]
pub struct Session {
    pub accounts: std::collections::BTreeMap<accounts::AccountId, SourceSession>,
    pub focus: Option<accounts::AccountId>,
    pub work_offline: bool,
    pub scope: accounts::Scope,
    fallback: SourceSession,
}

impl Session {
    pub fn new(settings: Option<Settings>, connecting: bool, work_offline: bool) -> Self {
        Self {
            work_offline,
            scope: settings.as_ref().map(|s| s.scope.clone()).unwrap_or_default(),
            fallback: SourceSession { settings, connecting, ..Default::default() },
            ..Default::default()
        }
    }

    pub fn get(&self, account: accounts::AccountId) -> Option<&SourceSession> {
        self.accounts.get(&account).or_else(|| (self.fallback.account() == Some(account)).then_some(&self.fallback))
    }

    pub fn key(&self, id: &str) -> Option<accounts::ItemKey> {
        accounts::ItemKey::parse(id).ok().or_else(|| Some(accounts::ItemKey::new(self.account()?, id)))
    }

    pub fn for_item(&self, id: &str) -> &SourceSession {
        crate::source::identity::account(id).and_then(|a| self.get(a)).unwrap_or(self)
    }
}

impl std::ops::Deref for Session {
    type Target = SourceSession;
    fn deref(&self) -> &SourceSession {
        self.focus.and_then(|a| self.accounts.get(&a)).unwrap_or(&self.fallback)
    }
}

impl std::ops::DerefMut for Session {
    fn deref_mut(&mut self) -> &mut SourceSession {
        match self.focus {
            Some(a) => self.accounts.get_mut(&a).expect("configured focus"),
            None => &mut self.fallback,
        }
    }
}

impl SourceSession {
    pub fn state(&self, work_offline: bool) -> accounts::SourceState {
        use accounts::{OfflineReason, SourceState};
        if !self.folder_missing.is_empty() {
            SourceState::FolderMissing
        } else if self.source_caps().network && work_offline {
            SourceState::Offline(OfflineReason::ByChoice)
        } else if self.sign_in.is_some() {
            SourceState::SignedOut
        } else if self.connecting {
            SourceState::Connecting
        } else if self.unreachable {
            SourceState::Offline(OfflineReason::Unreachable)
        } else if self.sync.albums || self.sync.tracks {
            if self.kind().scans {
                SourceState::Scanning
            } else {
                SourceState::Syncing(self.sync.progress.and_then(|p| (p.total > 0).then_some(p.done as f32 / p.total as f32)))
            }
        } else if self.online {
            SourceState::Online
        } else {
            SourceState::Offline(OfflineReason::Unreachable)
        }
    }

    pub fn account(&self) -> Option<accounts::AccountId> {
        self.settings.as_ref().and_then(|s| s.account())
    }

    /// What the source in use is called and how it syncs, from its
    /// configured kind (a Navidrome server before anything is configured).
    pub fn kind(&self) -> crate::source::Kind {
        let kind = self.settings.as_ref().and_then(|s| s.primary()).map(|e| e.kind).unwrap_or_default();
        crate::source::Kind::of(kind)
    }

    /// What the source in use can do natively, before Ond's own store fills
    /// in hearts, ratings and playlists.
    pub fn source_caps(&self) -> crate::source::Capabilities {
        self.source.as_ref().map_or_else(|| self.kind().capabilities, |s| s.capabilities())
    }

    /// What the source in use can do, together with Ond's own store for what
    /// the source itself lacks (hearts, ratings, playlists).
    pub fn caps(&self) -> crate::source::Capabilities {
        self.source_caps().with_ond_store()
    }

    /// The source, while the server answers.
    pub fn live(&self) -> Option<&Src> {
        self.source.as_ref().filter(|_| self.online)
    }

    /// Online, or about to be as far as anyone knows.
    pub fn expected(&self) -> bool {
        self.online || self.connecting
    }
}

impl OndApp {
    /// Signed in: the session starts (or resumes after an outage).
    pub(super) fn connected(
        &mut self,
        source: Src,
        stores: Option<crate::storage::Stores>,
        mut settings: Box<Settings>,
        server: String,
        new_account: bool,
        password_saved: bool,
    ) {
        if let Some(entry) = settings.primary_mut() {
            entry.id = source.account();
        } else {
            settings.sources.push(crate::settings::SourceEntry::subsonic(source.account(), "", ""));
        }
        let stores_changed = stores.is_some();
        if new_account || stores.is_some() {
            // Drop the old account's UI state before sync fills the new mirror.
            // A second library read could arrive after sync and blank its albums.
            self.bridge.end_session();
            self.forget_library();
        }
        if let Some(stores) = stores {
            // Another configured source keeps its library on disk: it's only
            // not the one in use. One that's gone from the list is removed.
            let keep: Vec<accounts::AccountId> = settings.sources.iter().map(|s| s.id).collect();
            let store = self.bridge.install_storage(source.account(), stores, &keep);
            self.analysis.records.clear();
            self.analysis.version += 1;
            self.analysis.loaded = false;
            self.analysis.library_run = false;
            self.analysis.runner.set_store(store.clone());
            self.bridge.load_analyses(store);
        }
        if !password_saved {
            self.show_notice("Ond couldn't save your password in the system keychain, so it will ask for it again next time.");
        }
        let limit = self.prefs.snapshot().offline_limit_gb.map(|gb| gb * 1_000_000_000);
        self.downloads.switch_account(settings.account_id(), Some(source.account()), limit, &self.bridge);
        // The server's address and user, when a server is the source in use.
        let login = settings.primary().filter(|e| e.kind == crate::settings::SourceKind::Subsonic).map(|e| (e.server.clone(), e.username.clone()));
        let (address, user) = login.clone().unwrap_or_default();
        self.session.server_label =
            if user.is_empty() { settings.source_name().map(str::to_owned).unwrap_or_else(|| server.clone()) } else { format!("{user} · {server}") };
        self.session.server_info = server.clone();
        self.login = None;
        self.source_added(&address, source.account());
        // Always a new pass, even if the flags say one runs: any still
        // running is from before the server went away, and the bridge
        // has this one replace it.
        self.session.sync.label = "syncing…".into();
        self.session.sync.albums = true;
        self.bridge.sync_source(&source);
        self.bridge.fetch_home(source.clone());
        self.art.set_source(source.clone());
        self.now_playing.set_source(source.clone());
        self.output.connected(&address);
        // A store still opening gets the server once it's open.
        self.connect_downloads(&source);
        let url = source.clone();
        self.analysis.runner.set_server(source.capabilities().downloads.then(|| {
            Arc::new(move |key: &crate::source::TrackKey| (key.account == url.account()).then(|| url.download_url(&key.item)).flatten())
                as crate::analyze::StreamUrl
        }));
        self.bridge.fetch_starred(Some(source.clone()));
        if source.capabilities().network {
            self.diagnostics.latency_asked();
            self.bridge.measure_latency(source.clone());
        } else {
            self.diagnostics.set_latency(None);
        }
        self.session.unreachable = false;
        self.session.connecting = false;
        self.session.sign_in = None;
        self.session.folder_missing.clear();
        if stores_changed {
            self.session.skipped = 0;
        }
        self.session.source = Some(source.clone());
        match (&login, settings.primary().cloned()) {
            (Some((server, user)), _) => self.prefs.set_login(server, user, source.account()),
            (None, Some(entry)) => self.prefs.edit(|s| {
                if !s.sources.iter().any(|e| e.id == entry.id) {
                    s.sources.insert(0, entry.clone());
                }
                s.make_primary(entry.id);
                s.signed_out = false;
            }),
            (None, None) => {}
        }
        self.session.settings = Some(*settings);
        if !self.session.online {
            self.session.online = true;
            // Lined up while offline; the server can supply the rest again.
            self.player.forget_next();
            self.sync_next();
        }
        // Asked for while connecting.
        self.with_player(|p, cx| p.resume_held(cx));
        self.resume_library_analysis();
    }

    /// The sign-in failed. From the form: back to it, with the error. The
    /// saved login (at startup or reconnecting) leaves the library up, and
    /// the footer asks to sign in again.
    pub(super) fn connect_failed(&mut self, e: String) {
        self.set_offline();
        self.session.connecting = false;
        match self.login.as_mut() {
            Some(form) => {
                form.busy = false;
                form.password.clear();
                form.error = Some(e);
            }
            None => {
                tracing::info!("saved login refused: {e}");
                self.session.unreachable = false;
                self.session.sign_in = Some(e);
            }
        }
        self.server_gone();
    }

    /// Works offline (stops connecting, shows only what can play) or goes
    /// back online.
    pub(super) fn set_work_offline(&mut self, on: bool) {
        if self.session.work_offline == on {
            return;
        }
        if !self.multi.bridges.is_empty() {
            return self.work_offline_for_sources(on);
        }
        if self.session.work_offline == on {
            return;
        }
        self.session.work_offline = on;
        self.prefs.edit(|s| s.work_offline = on);
        if on {
            // Connects and reconnects in flight stop; what they'd deliver is dropped.
            self.bridge.end_session();
            self.forget_in_flight();
            self.forget_waiting();
            self.set_offline();
            self.session.connecting = false;
            self.session.unreachable = false;
            self.diagnostics.set_latency(None);
            self.server_gone();
        } else if let Some(settings) = self.session.settings.clone() {
            self.session.connecting = true;
            self.bridge.connect(settings, None);
        }
        self.load_local();
    }

    /// Reads the library from disk again: all of it, or working offline only
    /// what can play (once downloads are open: until then nothing is known
    /// to be downloaded, and their opening calls this again).
    pub(super) fn load_local(&mut self) {
        if !self.multi.bridges.is_empty() {
            let accounts: Vec<_> = self.multi.bridges.keys().copied().collect();
            for account in accounts {
                self.with_source(account, |app| app.load_source_local(account));
            }
            return;
        }
        self.data.served = Default::default();
        if !self.session.work_offline {
            return self.bridge.load_local(None);
        }
        if self.downloads.opening {
            return;
        }
        let keep = crate::local::Playable {
            account: self.session.account(),
            downloads_account: self.downloads.owner,
            downloads: self.downloads.snap.clone(),
            cache: self.cache.clone(),
            transcode: self.output.transcode(),
        };
        self.bridge.load_local(Some(keep));
    }

    /// Opens the sign-in form over the library, filled in, with why it's
    /// needed.
    pub(super) fn sign_in_again(&mut self) {
        let s = self.session.settings.clone().unwrap_or_default();
        self.login = Some(LoginForm {
            server: s.server().to_owned(),
            username: s.username().to_owned(),
            error: self.session.sign_in.clone(),
            closable: true,
            ..Default::default()
        });
    }

    /// The server stopped answering: carry on offline while reconnecting.
    pub(super) fn unreachable(&mut self, username: String, error: String) {
        tracing::info!("server unreachable: {error}");
        self.login = None;
        self.set_offline();
        self.session.unreachable = true;
        self.session.connecting = false;
        self.diagnostics.set_latency(None);
        self.session.server_label = format!("{username} · offline");
        self.session.sync.label = "reconnecting…".into();
        self.server_gone();
    }

    /// Measures the server's round trip again if the last reading is old
    /// (and none is on its way).
    pub(super) fn refresh_latency(&mut self) {
        if self.session.online
            && self.diagnostics.latency_stale()
            && let Some(source) = self.session.source.clone()
            && source.capabilities().network
        {
            self.diagnostics.latency_asked();
            self.multi.bridges.get(&source.account()).unwrap_or(&self.bridge).measure_latency(source);
        }
    }

    /// Something failed; if it was the network, find out whether the server is gone.
    pub(super) fn probe(&self) {
        if self.session.online
            && let (Some(source), Some(settings)) = (&self.session.source, &self.session.settings)
            && source.capabilities().network
        {
            self.multi.bridges.get(&source.account()).unwrap_or(&self.bridge).probe(source.clone(), settings.clone());
        }
    }

    /// Another server or user signed in: what was shown came from the old one.
    /// The library database was cleared already; this drops the in-memory copy,
    /// the queue and the cached audio, which a sync then refills.
    pub(super) fn forget_library(&mut self) {
        self.with_player(|p, cx| p.reset(cx));
        self.data.clear();
        self.search.stale = true;
        self.forget_waiting();
        self.favorites = Favorites::default();
        self.lyrics.clear();
        self.song_stats.clear();
        self.ui.nav = Nav::default();
        self.cache.clear();
    }

    /// Drops plays and pins still waiting for an album's or a playlist's
    /// tracks. Whatever was loading them belonged to a session that's over
    /// and delivers nothing, so they'd wait until the user next opened that
    /// list and then play or download it unbidden.
    pub(super) fn forget_waiting(&mut self) {
        self.pending.clear();
        self.downloads.pending_pins.clear();
    }

    /// Forgets lyrics and play counts still on their way. Called when the
    /// bridge session ends without a change of account: the fetches deliver
    /// nothing now, so their `Loading` would stay for good. Lyrics that
    /// arrived stay; play counts can't tell "loading" from "the server
    /// couldn't say" (both `None`), so those are asked for again.
    pub(super) fn forget_in_flight(&mut self) {
        self.lyrics.retain(|_, f| *f != crate::lyrics::Fetch::Loading);
        self.song_stats.retain(|_, s| s.is_some());
    }

    /// The server's gone: browse what's stored, play what's downloaded.
    pub(super) fn set_offline(&mut self) {
        if let Some(account) = self.session.focus.filter(|a| self.multi.bridges.contains_key(a)) {
            self.art.offline_source(account);
            self.now_playing.remove_source(account);
            self.analysis.runner.cancel_account(account);
        }
        self.session.online = false;
        self.session.source = None;
        if let Some(store) = &self.downloads.store {
            store.set_source(None);
        }
        self.session.sync.tracks = false;
        self.session.sync.albums = false;
        self.session.sync.progress = None;
        if !self.multi.bridges.is_empty() {
            self.refresh_analysis_sources();
        }
    }

    /// Asks before signing out: it stops what's playing and the password has
    /// to be typed again.
    pub(super) fn ask_sign_out(&mut self) {
        if let Some(account) = self.session.focus.filter(|a| self.multi.bridges.contains_key(a) && self.session.accounts.len() > 1) {
            self.ui.overlay = Some(super::Overlay::SignOutSource { account });
            return;
        }
        let server = self.session.settings.as_ref().map_or_else(|| self.prefs.snapshot().server().to_owned(), |s| s.server().to_owned());
        let host = super::diagnostics::server_host(&server);
        self.ui.palette = None;
        let from_settings = matches!(self.ui.overlay, Some(super::dialogs::Overlay::Settings));
        self.ui.overlay = Some(super::dialogs::Overlay::SignOut { host, from_settings });
    }

    /// Stops playback, forgets the saved password and shows the login screen
    /// with the server and username filled in.
    pub(super) fn sign_out(&mut self) {
        if let Some(account) = self.session.focus.filter(|a| self.multi.bridges.contains_key(a)) {
            return self.sign_out_source(account);
        }
        self.with_player(|p, cx| p.reset(cx));
        self.ui.palette = None;
        self.ui.overlay = None;
        self.favorites = Favorites::default();
        // Another account may sign in next (ids can collide), and a fetch still
        // in flight now delivers nothing, so its `Loading` would stay for good.
        self.lyrics.clear();
        self.song_stats.clear();
        self.forget_waiting();
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
        self.session.connecting = false;
        self.session.sign_in = None;
        self.session.folder_missing.clear();
        // Whoever signs in next is online.
        if self.session.work_offline {
            self.session.work_offline = false;
            self.prefs.edit(|s| s.work_offline = false);
            self.load_local();
        }
        self.session.server_label.clear();
        let saved = self.prefs.snapshot();
        if self.prefs.has_login() {
            let s = saved.clone();
            std::thread::spawn(move || s.forget_password());
        }
        self.prefs.edit(|s| {
            s.signed_out = true;
        });
        self.now_playing.clear_source();
        self.art.clear_source();
        self.login = Some(LoginForm { server: saved.server().to_owned(), username: saved.username().to_owned(), ..Default::default() });
        self.onboarding = Some(super::onboarding::Onboard::default());
    }

    /// The sign-in form, in the first-run sheet's column: the server, the
    /// username and the password (a field managed.json sets is shown, not
    /// edited), Connect, and what went wrong.
    pub(super) fn login_form(&mut self, ui: &mut egui::Ui) {
        let Some(form) = self.login.as_mut() else { return };
        let locked = (self.ui.prefs.server_managed, self.ui.prefs.username_managed);
        let (mut close, mut cancel) = (false, false);
        // The first field that wants typing, focused when the form appears.
        let first = if !form.focused && !form.busy {
            form.focused = true;
            Some(if form.server.trim().is_empty() && !locked.0 {
                0
            } else if form.username.trim().is_empty() && !locked.1 {
                1
            } else {
                2
            })
        } else {
            None
        };
        let t = Tokens::of(ui);
        let w = ui.available_width();
        let mut enter = false;
        ui.add_enabled_ui(!form.busy, |ui| {
            let field = |ui: &mut egui::Ui, label: &str, text: &mut String, hint: &str, password: bool, locked: bool, focus: bool| {
                // Not the kit's own label: a managed field says so beside it.
                ui.horizontal(|ui| {
                    kit::field_label(ui, label);
                    if locked {
                        ui.label(egui::RichText::new(crate::managed::LOCKED).size(11.0).color(t.muted));
                    }
                });
                if locked {
                    // Set by the system's configuration: a value to read, not
                    // a greyed-out field that looks empty.
                    egui::Frame::new()
                        .fill(t.field)
                        .corner_radius(ui.visuals().widgets.inactive.corner_radius)
                        .inner_margin(egui::Margin::symmetric(10, 7))
                        .show(ui, |ui| {
                            ui.set_width(w - 20.0);
                            ui.add(egui::Label::new(egui::RichText::new(text.as_str()).color(t.text)).truncate());
                        });
                    return false;
                }
                kit::TextField::new(label).hint(hint).password(password).focus(focus).show_input(ui, text)
            };
            enter |= field(ui, "Server", &mut form.server, "https://music.example.com", false, locked.0, first == Some(0));
            if !locked.0 {
                kit::field_help(ui, "Your server's address, for example https://music.example.com");
            }
            ui.add_space(10.0);
            enter |= field(ui, "Username", &mut form.username, "", false, locked.1, first == Some(1));
            ui.add_space(10.0);
            enter |= field(ui, "Password", &mut form.password, "", true, false, first == Some(2));
        });
        ui.add_space(18.0);
        let label = if form.busy { "Connecting…" } else { "Connect" };
        let resp = kit::form_button(ui, label, true, form.busy.then_some(true));
        if form.busy {
            ui.ctx().request_repaint_after(Duration::from_millis(33));
        }
        let submit = resp.clicked() || enter;
        if let Some(e) = &form.error {
            ui.add_space(12.0);
            kit::field_error(ui, "Sign-in error", e);
        }
        // A server that takes the connection and then says nothing
        // would hold the form until the request times out.
        if form.busy {
            ui.add_space(16.0);
            ui.vertical_centered(|ui| {
                if crate::widgets::link(ui, "Cancel", egui::FontId::proportional(12.0), t.accent).clicked() {
                    cancel = true;
                }
            });
        }
        if form.closable {
            ui.add_space(if form.busy { 8.0 } else { 16.0 });
            ui.vertical_centered(|ui| {
                if crate::widgets::link(ui, "Keep browsing offline", egui::FontId::proportional(12.0), t.accent).clicked() {
                    close = true;
                }
            });
        }
        let busy = form.busy;
        if close {
            if busy {
                self.actions.push(super::Action::CancelSignIn);
            }
            self.actions.push(super::Action::CloseLogin);
        } else if cancel {
            self.actions.push(super::Action::CancelSignIn);
        } else if submit && !busy {
            self.actions.push(super::Action::SignIn);
        }
    }

    /// Connects with what the login form holds (its Connect button).
    pub(super) fn sign_in(&mut self) {
        let Some(form) = self.login.as_mut().filter(|f| !f.busy) else { return };
        form.busy = true;
        form.error = None;
        let settings = self.prefs.snapshot().sign_in(form.server.trim(), form.username.trim());
        // Kept in the form until it's answered, so a cancelled connect can be tried again as typed.
        let password = form.password.clone();
        self.connect_source_form(settings, password);
    }

    /// Gives up on the login form's connect. The session ends, as on signing
    /// out: the connect's result is stale when it arrives and is dropped
    /// (`send` and the session check in `handle`), and a password it was about
    /// to save in the keychain isn't saved.
    pub(super) fn cancel_sign_in(&mut self) {
        let Some(form) = self.login.as_mut().filter(|f| f.busy) else { return };
        form.busy = false;
        if let Some(account) = self.multi.signing_in.take() {
            if let Some(bridge) = self.multi.bridges.get(&account) {
                bridge.end_session();
            }
            if let Some(state) = self.session.accounts.get_mut(&account) {
                state.connecting = false;
            }
            self.forget_source_requests(account);
            return;
        }
        self.bridge.end_session();
        // Work for the ended session delivers nothing now.
        self.forget_in_flight();
        self.forget_waiting();
        self.session.connecting = false;
    }
}
