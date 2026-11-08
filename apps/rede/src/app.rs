//! The main window: a title bar with the account, the message bar, and the
//! page the trail leads to (the library, a book). Views are in submodules; they
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
mod library;
mod nav;
mod prefs_view;
mod session;
mod ui_state;
mod view_ctx;
mod virtual_list;

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
    /// Back to the sign-in screen; the saved tokens are forgotten. The shelf
    /// and its books stay.
    SignOut,
    /// List the libraries and write what changed. A sync already running
    /// isn't started twice.
    SyncNow,
    /// The library's order, and anything else a setting changes.
    Prefs(PrefsAction),
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
    /// The shelf's books, and the order they're drawn in.
    books: Books,
    /// Where the running sync is.
    sync: SyncState,
    /// The window: where it is, the notice line, the toasts' state.
    ui: UiState,
    actions: Vec<Action>,
}

impl RedeApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        // The window opened from this plan (see `main`); our title bar follows it.
        crate::window().apply_frame(&cc.egui_ctx);
        Self::build(&cc.egui_ctx, Paths::resolve()?)
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
            books: Books::default(),
            sync: SyncState::default(),
            ui,
            actions: Vec::new(),
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
                AccountMsg::NoTokens => self.needs_sign_in(None),
            },
            Msg::Library(m) => match m {
                LibraryMsg::Books(books) => self.books.set(books),
                LibraryMsg::SyncProgress(items) => self.sync.applied = items,
                LibraryMsg::SyncDone(outcome) => self.sync_done(outcome),
                LibraryMsg::Book { key, item } => self.books.loaded(&key.item, *item),
                LibraryMsg::BookFailed { key, error } => self.books.failed(&key.item, error),
            },
            Msg::Error(e) => self.show_failure(e),
        }
    }

    /// Shows what failed. A read or write on this computer says nothing about
    /// the server, so only what came from the network is retried quietly.
    pub(crate) fn show_failure(&mut self, e: Failure) {
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
            Action::SignOut => self.sign_out(),
            Action::SyncNow => self.sync_now(),
            Action::Prefs(a) => self.apply_prefs(a),
        }
    }

    /// A change to a preference, saved once the burst of changes ends.
    fn apply_prefs(&mut self, action: PrefsAction) {
        match action {
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

    /// The title bar: the account's menu at the right, and the window's own
    /// drag and double-click everywhere else.
    fn title_bar(&mut self, ui: &mut egui::Ui) {
        let mut full = ui.available_rect_before_wrap();
        shell::csd::set_bar(ui.ctx(), full, None);
        // The bar is the window's title bar too: its empty parts move the
        // window, and a double-click zooms it. Widgets drawn after this take
        // the pointer where they are.
        let whole = full.expand2(egui::vec2(12.0, 0.0));
        let bar = ui.interact(whole, ui.scope_id().with("titlebar"), egui::Sense::click_and_drag());
        if bar.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if bar.double_clicked() {
            shell::chrome::title_double_click(ui.ctx());
        }
        // Our own window buttons sit at one end on Linux; the traffic lights at
        // the other on macOS.
        use shell::csd::{Side, reserved};
        full.max.x -= reserved(ui.ctx(), Side::Right);
        let right = egui::Rect::from_min_max(egui::pos2(full.right() - 240.0, full.top()), full.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(right).layout(egui::Layout::right_to_left(egui::Align::Center)), |ui| {
            self.title_right(ui);
        });
    }

    /// The syncing indicator and the account menu, at the right of the bar.
    fn title_right(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::of(ui);
        ui.spacing_mut().item_spacing.x = 10.0;
        if let Some(label) = self.sync_label() {
            ui.label(egui::RichText::new(label).size(12.0).color(t.muted));
            syncing_glyph(ui, &t);
        } else {
            ui.data_mut(|d| d.remove::<f64>(syncing_since_id()));
        }
        let Some(account) = self.session.account.clone() else {
            ui.label(egui::RichText::new("Not signed in").size(12.0).color(t.muted));
            return;
        };
        let label = egui::RichText::new(&account.username).size(12.0).color(t.muted);
        ui.menu_button(label, |ui| {
            ui.set_min_width(180.0);
            ui.label(egui::RichText::new(&account.server).size(11.0).color(t.muted));
            ui.separator();
            if ui.button("Sign out").clicked() {
                ui.close();
                self.actions.push(Action::SignOut);
            }
        });
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
        egui::Panel::top("title-bar").exact_size(crate::app::BAR_H).resizable(false).frame(egui::Frame::new().fill(t.bg)).show(ui, |ui| self.title_bar(ui));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg).inner_margin(egui::Margin { left: 28, right: 28, top: 4, bottom: 0 })).show(
            ui,
            |ui| {
                self.notice_bar(ui);
                let route = self.ui.nav.current().clone();
                match &route {
                    Route::Library => self.view().library_page(ui),
                    Route::Book(id) => {
                        let id = id.clone();
                        self.view().book_page(ui, &id);
                    }
                }
            },
        );
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
        // The window came to the front: catch up with the server, at most every
        // few minutes. Idle windows never ask.
        if self.session.signed_in()
            && !self.sync.running
            && ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(true))))
            && self.sync.due()
        {
            self.actions.push(Action::SyncNow);
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
        shell::chrome::sync(ctx, shell::chrome::Bar { height: Some(BAR_H) });
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
        shell::csd::overlay(ui.ctx(), Some(BAR_H));
    }

    fn on_exit(&mut self) {
        self.prefs.flush();
    }
}

/// The title bar's height, as Ond's toolbar: the traffic lights (macOS) and our
/// own window buttons (Linux) sit centred in it.
pub const BAR_H: f32 = 52.0;

impl Drop for RedeApp {
    fn drop(&mut self) {
        // Work in flight belongs to a session nobody is using any more.
        self.bridge.end_session();
        self.prefs.flush();
    }
}

/// The sync spinner: it turns while a sync runs, then stops after
/// [`SYNC_SPIN`], so a listing that stalls can't hold the window at 20 frames a
/// second for ever. The start is kept in egui's own memory, per window.
fn syncing_glyph(ui: &mut egui::Ui, t: &Tokens) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
    shell::icons::paint(ui.painter(), rect, shell::icons::Icon::Spinner, t.muted);
    let now = ui.input(|i| i.time);
    let since = ui.data_mut(|d| *d.get_temp_mut_or_insert_with(syncing_since_id(), || now));
    if now - since < SYNC_SPIN {
        ui.ctx().request_repaint_after(SPIN_FRAME);
    }
}

/// How long the spinner turns before it goes still.
const SYNC_SPIN: f64 = 30.0;
const SPIN_FRAME: Duration = Duration::from_millis(50);

fn syncing_since_id() -> egui::Id {
    egui::Id::unique("sync-since")
}
