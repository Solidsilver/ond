//! The main window: the title bar that is also the player, the sidebar with
//! the places and the account, the message bar, and the page the trail leads
//! to (the library, a book). Views are in submodules; they
//! don't change state directly but push [`Action`]s, which are applied once the
//! frame is drawn.
//!
//! This file holds [`RedeApp`]: building it, the dispatch of bridge messages
//! and actions, and the frame (`main_ui`, `ui`). The app's parts and the
//! actions and messages each one handles are in their own modules:
//!
//! - `session`: signing in, resuming, and signing out
//! - `library`: the books the shelf has, in the order drawn
//! - `nav`: where the window is, and how it got there
//! - `playing`: playback actions, engine events and the listening worker
//! - `bookmarks`: the bookmarks a book has, the edits that sync them, and the
//!   position another device left behind
//! - `controls`: chapter seeking and the listening controls
//! - `title_bar`: the player in the title bar, and the window's controls
//! - `sidebar`: the places and the account, in the column at the left
//! - `sheets`: the keyboard shortcuts and the playback settings, over the
//!   window
//! - `panel`: the chapters column at the right
//! - `ui_state`: the notice line, the trail, and what a frame reads
//! - `prefs_view`: the preferences a frame reads, and the actions that change
//!   them
//! - `view_ctx` and the views: what a view gets, and the views themselves
//!
//! # Adding things
//!
//! A server call:
//! 1. A `Bridge` method that does the work in the background and posts the
//!    result. Use `self.scoped()` so a result for an account that signed out is
//!    dropped.
//! 2. A `Msg` variant for the result, inside the group it belongs to
//!    (`AccountMsg`, `LibraryMsg`). Failures go as a `Failure`, so only network
//!    ones are quietened.
//! 3. A handler for it in the matching `app/*.rs` (`session.rs` for signing in,
//!    `library.rs` for the books).
//!
//! A preference:
//! 1. A field in `Settings`, with `#[serde(default)]`.
//! 2. A field in `PrefsView` if a frame reads it, and a `PrefsAction` variant
//!    with its arm in `apply_prefs`.
//!
//! A view:
//! 1. `app/<view>.rs`, drawing from a `ViewCtx` and pushing `Action`s.
//! 2. A `Route` for it in `nav.rs`, and its arm in `main_ui`.
//! 3. An entry in `every_view_draws` in `app/tests.rs` and a route in
//!    `every_view_names_its_controls` in `app/ui_tests.rs` (every painted
//!    control needs a `widget_info`), and a scene in `snapshots.rs`.

mod book;
mod bookmarks;
mod controls;
mod library;
mod nav;
mod panel;
mod player_view;
mod playing;
mod prefs_view;
mod session;
mod sheets;
mod shortcuts;
mod sidebar;
mod sleep;
mod title_bar;
mod ui_state;
mod view_ctx;
mod virtual_list;
mod window;

#[cfg(test)]
mod fixture;
#[cfg(test)]
mod snapshots;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod ui_tests;

use crate::art::Art;
use crate::bridge::{AccountMsg, Bridge, LibraryMsg, Msg};
use crate::failure::Failure;
use crate::prefs::{Loaded, Prefs};
use crate::settings::Paths;
use crate::tokens::Keeper;
use shell::theme::{self, Tokens};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

pub use library::{Books, Details};
pub use nav::Nav;
pub use nav::Route;
pub use prefs_view::{PrefsAction, PrefsView};
pub use session::{LoginForm, Session};
pub use ui_state::{Notice, SyncState, UiState};

/// The app's tokio runtime, kept for the app's lifetime (the bridge holds a
/// handle to it).
///
/// Dropping a `Runtime` waits for every `spawn_blocking` call still running,
/// however long. Quitting gives them [`SHUTDOWN_WAIT`] and then goes: the work
/// there (shelf writes, keychain calls) is written to survive being cut off.
struct AppRuntime(Option<tokio::runtime::Runtime>);

const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

impl Drop for AppRuntime {
    fn drop(&mut self) {
        if let Some(rt) = self.0.take() {
            rt.shutdown_timeout(SHUTDOWN_WAIT);
        }
    }
}

/// A state change asked for by a view or the title bar, applied after the frame
/// is drawn. Changes that belong to one part of the app are grouped under it
/// ([`PrefsAction`]), and that part's module applies them.
pub enum Action {
    Go(Route),
    Back,
    /// Connect with what's typed in the sign-in form.
    SignIn,
    CheckServer,
    /// Start a browser sign-in with the server's provider.
    OpenId,
    /// Open the waiting browser sign-in's page again: the same attempt, not a
    /// new one.
    ReopenBrowser,
    CancelSignIn,
    /// Back to the sign-in screen; the saved tokens are forgotten. The shelf
    /// and its books stay.
    SignOut,
    /// List the libraries and write what changed. A sync already running
    /// isn't started twice.
    SyncNow,
    /// The library's order, and anything else a setting changes.
    Prefs(PrefsAction),
    PlayBook(String),
    #[allow(dead_code, reason = "constructed by the separate bookmarks UI")]
    AddBookmark {
        item: sources::ItemKey,
        time: f64,
        title: String,
    },
    #[allow(dead_code, reason = "constructed by the separate bookmarks UI")]
    RenameBookmark {
        item: sources::ItemKey,
        time: f64,
        title: String,
    },
    #[allow(dead_code, reason = "constructed by the separate bookmarks UI")]
    DeleteBookmark {
        item: sources::ItemKey,
        time: f64,
    },
    #[allow(dead_code, reason = "constructed by the separate bookmarks UI")]
    JumpToBookmark {
        item: sources::ItemKey,
        time: f64,
    },
    #[allow(dead_code, reason = "constructed by the separate bookmarks UI")]
    UndoReconcile,
    Playback(crate::player::Control),
    Mode(crate::window::Mode),
    /// The keyboard shortcuts' sheet: open it, or close it if it's open.
    Shortcuts,
    /// The playback settings' sheet (skips, rewinds, the timer's length).
    PlaybackSettings,
    /// Whichever sheet is open goes.
    CloseSheet,
    ToggleMini,
    /// The chapters column: shown, hidden, or on another tab.
    Panel(panel::PanelAction),
}

pub struct RedeApp {
    /// Must stay the first field. Fields drop in order, so the runtime shuts
    /// down (and its tasks are dropped) before anything declared after it.
    /// Nothing declared below may need the runtime in its `Drop`: by then it
    /// is gone, and a `block_on` or `spawn` there would panic or do nothing.
    _rt: AppRuntime,
    /// The window's egui context (for theme changes and window commands).
    ctx: egui::Context,
    bridge: Bridge,
    inbox: Receiver<Msg>,
    /// The user's preferences: the one copy, saved in the background.
    prefs: Prefs,
    /// This frame's copy of the preferences (read in `logic`, before drawing).
    shown: PrefsView,
    /// Covers, fetched as they're drawn.
    art: Art,

    /// Who's signed in, and what their shelf is.
    session: Session,
    login: Option<LoginForm>,
    openid: crate::openid::State,
    /// The shelf's books, and the order they're drawn in.
    books: Books,
    /// Where the running sync is.
    sync: SyncState,
    /// The window: where it is, the notice line, the toasts' state.
    ui: UiState,
    actions: Vec<Action>,
    player: crate::player::Player,
    mode: crate::window::Mode,
    window: window::State,
    /// The sheet over the window, if one is open.
    sheet: Option<sheets::Sheet>,
    engine: Option<audio::EngineHandle>,
    #[cfg(target_os = "macos")]
    now_playing: crate::now_playing::Publisher,
    listening: Option<crate::listening::Listening>,
    playback_generation: u64,
    opening: bool,
    resume_pending: bool,
    /// When a resume pull has waited long enough to play from the local
    /// position instead.
    resume_deadline: Option<Instant>,
    sleep_wake: std::sync::Arc<std::sync::atomic::AtomicBool>,
    positions: std::collections::HashMap<String, sources::Progress>,
    stream_dir: std::path::PathBuf,
    pub bookmarks: bookmarks::State,
    pub reconciliation: Option<crate::listening::Reconciled>,
}

impl RedeApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        // The window opened from this plan (see `main`); our title bar follows it.
        crate::window().apply_frame(&cc.egui_ctx);
        let app = Self::build(&cc.egui_ctx, Paths::resolve()?)?;
        #[cfg(target_os = "macos")]
        {
            app.bridge.media_keys();
            app.bridge.receive_urls();
        }
        app.bridge.restore_openid();
        Ok(app)
    }

    fn build(ctx: &egui::Context, paths: Paths) -> anyhow::Result<Self> {
        // Two workers: the shelf's writes and the server's calls, so a listing
        // doesn't hold up a book's details.
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
        let paths = std::sync::Arc::new(paths);
        let (prefs, loaded) = Prefs::open(&paths.config_file);
        let (tx, inbox) = channel();
        let repaint = ctx.clone();
        let bridge = Bridge::new(rt.handle().clone(), tx, std::sync::Arc::new(move || repaint.request_repaint()), Keeper::start(), paths.clone());
        let saved = prefs.snapshot();
        theme::install(ctx, saved.theme);
        let art = Art::new(rt.handle().clone(), ctx.clone(), paths.art_cache.clone());
        let shown = PrefsView::read(&prefs);
        let mut ui = UiState::new();
        if let Loaded::Unreadable { aside } = loaded {
            ui.notice =
                Some(Notice::error(format!("Your settings couldn't be read, so Rede started fresh. Sign in again. The old file is at {}.", aside.display())));
        }
        // A saved account is resumed with the tokens the keyring kept; without
        // one, the sign-in form starts empty.
        let login = saved.account().is_none().then(LoginForm::default);
        #[cfg(target_os = "macos")]
        shell::now_playing::intervals(f64::from(saved.playback.skip_back), f64::from(saved.playback.skip_forward));
        if let Some(account) = saved.account() {
            bridge.start(account.clone());
        }
        Ok(Self {
            _rt: AppRuntime(Some(rt)),
            ctx: ctx.clone(),
            bridge,
            inbox,
            prefs,
            shown,
            art,
            session: Session { connecting: saved.account().is_some(), ..Session::default() },
            login,
            openid: Default::default(),
            books: Books::default(),
            sync: SyncState::default(),
            ui,
            actions: Vec::new(),
            player: crate::player::Player::default(),
            mode: saved.window.mode,
            window: window::State::default(),
            sheet: None,
            engine: None,
            #[cfg(target_os = "macos")]
            now_playing: Default::default(),
            listening: None,
            playback_generation: 0,
            opening: false,
            resume_pending: false,
            resume_deadline: None,
            sleep_wake: Default::default(),
            positions: std::collections::HashMap::new(),
            bookmarks: bookmarks::State::default(),
            reconciliation: None,
            stream_dir: paths.art_cache.with_file_name("streams"),
        })
    }

    /// Applies one message from the background. Sign-in results from before a
    /// sign-out are dropped: they belong to an account nobody is using.
    fn handle(&mut self, msg: impl Into<Msg>) {
        match msg.into() {
            Msg::Account(m) => match m {
                AccountMsg::SignedIn { session, account, source, shelf, warning } => {
                    if session != self.bridge.session() {
                        return;
                    }
                    self.signed_in(account, source, shelf, warning);
                }
                AccountMsg::SignInFailed { session, error } => {
                    if session != self.bridge.session() {
                        return;
                    }
                    self.sign_in_failed(error);
                }
                AccountMsg::NoTokens { session } => {
                    if session == self.bridge.session() {
                        self.needs_sign_in(None);
                    }
                }
                m => self.handle_sign_in(m),
            },
            Msg::Library(m) => match m {
                LibraryMsg::Books(books) => {
                    if self.session.source.is_some()
                        && let (Some(account), Some(shelf)) = (&self.session.account, &self.session.shelf)
                    {
                        self.bridge.positions(shelf.clone(), books.iter().map(|book| sources::ItemKey::new(account.id, &book.id)).collect());
                    }
                    self.books.set(books);
                }
                LibraryMsg::SyncProgress(items) => self.sync.applied = items,
                LibraryMsg::SyncDone(outcome) => self.sync_done(outcome),
                LibraryMsg::Book { key, item } => self.books.loaded(&key.item, *item),
                LibraryMsg::BookFailed { key, error } => self.books.failed(&key.item, error),
            },
            Msg::Bookmarks { session, event } if session == self.bridge.session() => self.bookmark_event(event),
            Msg::Bookmarks { .. } => {}
            Msg::Playback { session, event } if session == self.bridge.session() => self.playback_event(event),
            Msg::Playback { .. } => {}
            Msg::Engine { session, event } if session == self.bridge.session() => self.engine_event(event),
            Msg::Engine { .. } => {}
            Msg::PlaybackTick { session, report } if session == self.bridge.session() => self.report_playback(report),
            Msg::PlaybackTick { .. } => {}
            #[cfg(target_os = "macos")]
            Msg::Remote(remote) => {
                if let Some(control) = crate::now_playing::control(remote, &self.player, self.shown.playback.skip_back, self.shown.playback.skip_forward) {
                    self.playback_control(control);
                }
            }
            #[cfg(target_os = "macos")]
            Msg::Artwork { session, id, bytes } => {
                if session == self.bridge.session() && self.player.book.as_ref().is_some_and(|b| b.key.to_string() == id) {
                    shell::now_playing::set_artwork(id, bytes);
                }
            }
            Msg::Error(e) => self.show_failure(e),
        }
    }

    /// Shows what failed. A read or write on this computer says nothing about
    /// the server, so only what came from the network is retried quietly.
    pub(crate) fn show_failure(&mut self, e: Failure) {
        if e.is_network() {
            let offline = self.sync.offline;
            self.sync.offline = true;
            if self.sync.retry_at.is_none() {
                let wait = Duration::from_secs(30);
                self.sync.retry_at = Some(Instant::now() + wait);
                self.ctx.request_repaint_after(wait);
            }
            if offline {
                return;
            }
        }
        self.show_error(e.into_message());
    }

    pub(crate) fn show_error(&mut self, text: impl Into<String>) {
        self.ui.notice = Some(Notice::error(text.into()));
    }

    pub(crate) fn show_notice(&mut self, text: impl Into<String>) {
        self.ui.notice = Some(Notice { text: text.into(), error: false });
    }

    /// Applies one action. Most go to the part of the app they belong to; an
    /// action never asks for another one later, it applies it at once.
    fn apply(&mut self, action: Action) {
        match action {
            Action::Go(route) => self.go(route),
            Action::Back => self.back(),
            Action::SignIn => self.sign_in(),
            Action::CheckServer => self.check_server(),
            Action::OpenId => self.begin_openid(),
            Action::ReopenBrowser => self.reopen_browser(),
            Action::CancelSignIn => self.cancel_sign_in(abs::OpenIdError::Cancelled),
            Action::SignOut => self.sign_out(),
            Action::SyncNow => self.sync_now(),
            Action::Prefs(a) => self.apply_prefs(a),
            Action::PlayBook(id) => self.play_book(id, None),
            Action::AddBookmark { item, time, title } => self.edit_bookmark(item, time, title, shelf::BookmarkChange::Create),
            Action::RenameBookmark { item, time, title } => self.edit_bookmark(item, time, title, shelf::BookmarkChange::Rename),
            Action::DeleteBookmark { item, time } => self.edit_bookmark(item, time, String::new(), shelf::BookmarkChange::Delete),
            Action::JumpToBookmark { item, time } => self.jump_to_bookmark(item, time),
            Action::UndoReconcile => self.undo_reconcile(),
            Action::Playback(control) => self.playback_control(control),
            Action::Mode(mode) => self.set_mode(mode),
            Action::Shortcuts => {
                self.sheet = (self.sheet != Some(sheets::Sheet::Shortcuts)).then_some(sheets::Sheet::Shortcuts);
            }
            Action::PlaybackSettings => self.sheet = Some(sheets::Sheet::PlaybackSettings),
            Action::CloseSheet => self.sheet = None,
            Action::ToggleMini => self.set_mode(self.prefs.get(|s| s.window.mini())),
            Action::Panel(a) => self.apply_panel(a),
        }
    }

    /// A change to a preference, saved once the burst of changes ends.
    fn apply_prefs(&mut self, action: PrefsAction) {
        match action {
            PrefsAction::Playback(mut playback) => {
                playback.speed = crate::timeline::normalize_speed(playback.speed);
                playback.skip_back = playback.skip_back.clamp(1, 300);
                playback.skip_forward = playback.skip_forward.clamp(1, 300);
                playback.rewind = playback.rewind.min(60);
                playback.sleep_minutes = playback.sleep_minutes.clamp(5, 120);
                playback.sleep_rewind = playback.sleep_rewind.min(60);
                self.prefs.edit(|s| s.playback = playback);
                self.shown.playback = playback;
                if let Some(engine) = &self.engine {
                    crate::player::apply_speech(playback, engine);
                }
                #[cfg(target_os = "macos")]
                shell::now_playing::intervals(f64::from(playback.skip_back), f64::from(playback.skip_forward));
            }
            PrefsAction::VoiceBoost(enabled) => {
                self.apply_prefs(PrefsAction::Playback(crate::settings::PlaybackSettings { voice_boost: enabled, ..self.shown.playback }));
            }
            PrefsAction::SkipSilence(enabled) => {
                self.apply_prefs(PrefsAction::Playback(crate::settings::PlaybackSettings { skip_silence: enabled, ..self.shown.playback }));
            }
            PrefsAction::Sort(sort) => {
                if self.prefs.get(|s| s.sort) == sort {
                    return;
                }
                self.prefs.edit(|s| s.sort = sort);
                // This frame's copy too, so the grid draws in the new order
                // without waiting for the next frame's read.
                self.shown.sort = sort;
                self.ctx.request_repaint();
            }
            PrefsAction::ShowPanel(show) => {
                self.prefs.edit(|s| s.show_panel = show);
                self.shown.show_panel = show;
                self.ctx.request_repaint();
            }
            PrefsAction::CollapseSidebar(collapsed) => {
                self.prefs.edit(|s| s.sidebar_collapsed = collapsed);
                self.shown.sidebar_collapsed = collapsed;
                self.ctx.request_repaint();
            }
        }
    }

    fn go(&mut self, route: Route) {
        self.ui.nav.go(route);
        self.on_arrive();
    }

    fn back(&mut self) {
        self.ui.nav.back();
        self.on_arrive();
    }

    /// Loads what the place just arrived at needs. A book's page asks for its
    /// details unless they're here already.
    fn on_arrive(&mut self) {
        let Route::Book(id) = self.ui.nav.current().clone() else { return };
        if self.books.details.contains_key(&id) {
            return;
        }
        let Some(account) = self.session.account.clone() else { return };
        let Some(shelf) = self.session.shelf.clone() else { return };
        self.books.details.insert(id.clone(), Details::Loading);
        self.bridge.open_book(self.session.source.clone(), shelf, sources::ItemKey::new(account.id, id));
    }

    fn notice_bar(&mut self, ui: &mut egui::Ui) {
        let Some(notice) = self.ui.notice.clone() else { return };
        let t = Tokens::of(ui);
        let color = if notice.error { t.danger } else { t.muted };
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(notice.text).size(12.0).color(color));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if shell::icons::sized_button(ui, shell::icons::Icon::Close, "Dismiss", egui::vec2(20.0, 20.0), 11.0, Some(t.muted)).clicked() {
                    self.ui.notice = None;
                }
            });
        });
        ui.add_space(6.0);
    }

    fn main_ui(&mut self, ui: &mut egui::Ui) {
        // This frame's copy of the preferences, taken once before anything is
        // drawn (views never lock them).
        self.shown = PrefsView::read(&self.prefs);
        let t = Tokens::of(ui);
        self.art.poll();
        // Narrow windows give the page the room: the sidebar folds to its
        // rail, and the chapters column becomes a sheet over the page. Known
        // before the title bar, whose toggle says which.
        // The window's own width: what decides the columns, wherever the
        // root ui starts (the test harness draws inside a margin).
        let width = ui.ctx().content_rect().width();
        let sidebar = sidebar::plan(width, self.shown.sidebar_collapsed);
        self.ui.panel.narrow = !panel::docks(width - sidebar.unwrap_or(sidebar::RAIL_W));
        if !self.ui.panel.narrow {
            self.ui.panel.sheet = false;
        }
        if self.mode != crate::window::Mode::Mini {
            egui::Panel::top("title-bar")
                .exact_size(crate::app::BAR_H)
                .resizable(false)
                .frame(egui::Frame::new().fill(t.bg).inner_margin(egui::Margin::symmetric(12, 0)))
                .show(ui, |ui| self.title_bar(ui));
        }
        self.window_keys(ui.ctx());
        self.playback_keys(ui.ctx());
        match self.mode {
            crate::window::Mode::Player => self.player_window(ui),
            crate::window::Mode::Mini => self.mini_window(ui),
            crate::window::Mode::Full => {
                self.sidebar_panel(ui, sidebar, width);
                self.side_panel(ui);
                let content = egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(t.bg).inner_margin(egui::Margin { left: 28, right: 28, top: 4, bottom: 0 }))
                    .show(ui, |ui| {
                        let within = ui.max_rect();
                        self.notice_bar(ui);
                        let route = self.ui.nav.current().clone();
                        match &route {
                            Route::Library => self.view().library_page(ui),
                            Route::Book(id) => {
                                let id = id.clone();
                                self.view().book_page(ui, &id);
                            }
                        }
                        // Floats over the foot of the page.
                        self.view().sleep_notice(ui, within);
                    });
                self.panel_sheet(ui.ctx(), content.response.rect);
            }
        }
        self.sheet_ui(ui.ctx());
        // What the views asked for this frame. Applying an action never queues
        // another, so one pass takes them all.
        for a in std::mem::take(&mut self.actions) {
            self.apply(a);
        }
    }

    fn logic(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.inbox.try_recv() {
            self.handle(msg);
        }
        self.expire_openid(std::time::SystemTime::now());
        self.playback_tick(ctx);
        // The window came to the front: replay what's queued and pull the
        // server's positions and bookmarks. The library listing has its own,
        // slower throttle and pulls too when it's due, so only one asks.
        if self.session.signed_in() && ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(true)))) {
            if self.sync.running || !self.sync.due() {
                self.pull_listening();
            } else {
                self.send_activity();
                self.actions.push(Action::SyncNow);
            }
        }
        // A sync the server asked us to hold off on, or one to try again after
        // the network failed.
        if let Some(at) = self.sync.retry_at
            && Instant::now() >= at
        {
            self.sync.retry_at = None;
            if self.session.signed_in() {
                self.actions.push(Action::SyncNow);
            }
        }
        for a in std::mem::take(&mut self.actions) {
            self.apply(a);
        }
        self.track_window(ctx);
        shell::chrome::sync(ctx, shell::chrome::Bar { height: (self.mode != crate::window::Mode::Mini).then_some(BAR_H) });
    }
}

impl eframe::App for RedeApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        RedeApp::logic(self, ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.login.is_some() {
            self.login_ui(ui);
        } else {
            self.main_ui(ui);
        }
        // Window buttons and resize edges where the system draws none (Linux).
        shell::csd::overlay(ui.ctx(), (self.mode != crate::window::Mode::Mini).then_some(BAR_H));
    }

    fn on_exit(&mut self) {
        if let Some(seen) = self.window.seen {
            self.prefs.edit(|s| s.window.remember(seen));
        }
        #[cfg(target_os = "macos")]
        shell::now_playing::publish(None, shell::now_playing::State::Stopped, 0.0);
        self.finish_playback();
        self.prefs.flush();
    }
}

/// The title bar's height, as Ond's toolbar: the traffic lights (macOS) and our
/// own window buttons (Linux) sit centred in it.
pub const BAR_H: f32 = 52.0;

impl Drop for RedeApp {
    fn drop(&mut self) {
        // Work in flight belongs to a session nobody is using any more.
        self.finish_playback();
        self.bridge.end_session();
        self.prefs.flush();
    }
}
