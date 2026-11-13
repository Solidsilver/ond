//! The column at the left of the full window, in Ond's sidebar's idiom: a
//! search field that opens the palette (⌘K), the places to browse under the
//! library's name, and the account at the foot, with its state (connected,
//! syncing, offline) and its menu, and the keyboard shortcuts beside it.
//! The foot is the kit's source switcher with every configured account
//! (active one at its live state, others online or signed out); clicking another
//! switches to it via `Action::SwitchAccount`, and Add source… opens the
//! first-run sheet over it (`Action::AddSource`), while the account menu's
//! rows are Rede's, drawn under it.
//!
//! The places are a list so Authors and Collections join Home, Library,
//! Series, Downloads and Stats as rows when they come. Downloads carries a ring
//! while books are on their way, as Ond's Offline place does. The account's libraries are listed
//! together on one shelf, so the heading names them rather than switching
//! between them; a switcher comes with per-library browsing.
//!
//! Collapsed (⌥⌘S, or the button by the heading) it's a rail of icons, as
//! Ond's sidebar becomes in a narrow window; a window too narrow for the full
//! column shows the rail whatever the preference, so nothing goes missing.

use super::nav::Route;
use super::view_ctx::ViewCtx;
use super::{Action, PrefsAction, RedeApp};
use accounts::{OfflineReason, Scope, SourceState};
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};
use shell::icons::{self, Icon};
use shell::kit;
use shell::theme::{Tokens, semibold};

/// The full column's width: Ond's sidebar at its default.
pub const SIDEBAR_W: f32 = 216.0;
/// The rail's: Ond's.
pub const RAIL_W: f32 = 52.0;
/// The page keeps at least this much beside the full column before it gives
/// way to the rail, as Ond's content does.
const CONTENT_MIN: f32 = 604.0;
/// The account's row at the foot (the switcher's two-line look), and the
/// buttons beside it.
const ACCOUNT_H: f32 = 40.0;
/// Room under the foot, in both the column and the rail: a little more than
/// Ond's 10, so the avatar stays clear of the window's rounded corner on
/// macOS.
pub(super) const FOOT_MARGIN: i8 = 12;
const FOOTER_BUTTON: f32 = 28.0;
const RAIL_BUTTON: f32 = 32.0;
const TOGGLE: f32 = 22.0;

/// The places to browse, in order: what they're called, where they lead, and
/// the rail's icon for them.
const PLACES: [(&str, Route, Icon); 5] = [
    ("Home", Route::Home, Icon::Home),
    ("Library", Route::Library, Icon::Grid),
    ("Series", Route::AllSeries, Icon::List),
    ("Downloads", Route::Downloads, Icon::Download),
    ("Stats", Route::Stats, Icon::Chart),
];
/// Room a place's download ring takes at its right end.
const MARK_ROOM: f32 = 16.0;

/// How wide the column draws in a window `width` wide: the full column, or
/// `None` for the rail (collapsed, or no room).
pub fn plan(width: f32, collapsed: bool) -> Option<f32> {
    (!collapsed && width >= SIDEBAR_W + CONTENT_MIN).then_some(SIDEBAR_W)
}

/// Whether a window `width` wide has room for the full column at all, so the
/// rail offers to expand only when it can.
fn can_expand(width: f32) -> bool {
    width >= SIDEBAR_W + CONTENT_MIN
}

impl RedeApp {
    /// The column at the left: `full` is its width, or `None` for the rail.
    /// `window` is the window's width, so the rail knows whether the full
    /// column would fit. Drawn before the chapters column and the page, so
    /// they take what's left.
    pub(super) fn sidebar_panel(&mut self, ui: &mut Ui, full: Option<f32>, window: f32) {
        let t = Tokens::of(ui);
        match full {
            Some(w) => {
                egui::Panel::left("sidebar")
                    .exact_size(w)
                    .resizable(false)
                    .frame(egui::Frame::new().fill(t.sidebar).inner_margin(egui::Margin { left: 10, right: 10, top: 12, bottom: FOOT_MARGIN }))
                    .show(ui, |ui| {
                        ui.set_max_width(ui.available_width());
                        self.view().sidebar(ui);
                    });
            }
            None => {
                egui::Panel::left("sidebar-rail")
                    .exact_size(RAIL_W)
                    .resizable(false)
                    .frame(egui::Frame::new().fill(t.sidebar).inner_margin(egui::Margin { left: 4, right: 4, top: 12, bottom: FOOT_MARGIN }))
                    .show(ui, |ui| self.view().sidebar_rail(ui, window));
            }
        }
    }
}

/// The account as the foot of the column shows it.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum AccountState {
    /// Signed in and answering: the server's host.
    Connected,
    /// A sync is running, with what it says.
    Syncing(String),
    /// The saved login is being tried.
    Connecting,
    /// The last sync couldn't reach the server; it's tried again shortly.
    Offline,
}

impl AccountState {
    /// The status line under the name.
    fn line(&self, host: &str) -> String {
        match self {
            Self::Connected => host.to_owned(),
            Self::Syncing(label) => label.clone(),
            Self::Connecting => "Connecting…".to_owned(),
            Self::Offline => "Offline · retrying".to_owned(),
        }
    }

    fn tooltip(&self, host: &str) -> String {
        match self {
            Self::Connected => format!("Connected to {host}. Click for the account."),
            Self::Syncing(label) => format!("{label}\nYou can browse and listen while it runs."),
            Self::Connecting => format!("Connecting to {host}…"),
            Self::Offline => "Can't reach the server. Rede keeps trying, and saves your place to send when it's back.".to_owned(),
        }
    }

    /// The state as the kit's switcher draws it.
    fn source_state(&self) -> SourceState {
        match self {
            Self::Connected => SourceState::Online,
            Self::Syncing(_) => SourceState::Syncing(None),
            Self::Connecting => SourceState::Connecting,
            Self::Offline => SourceState::Offline(OfflineReason::Unreachable),
        }
    }
}

/// The server's host, without its scheme or path: what the footer names.
pub(super) fn host(server: &str) -> &str {
    let rest = server.split_once("://").map_or(server, |(_, rest)| rest);
    rest.split('/').next().unwrap_or(rest)
}

impl ViewCtx<'_> {
    /// The account's state, from the session and the sync.
    pub(super) fn account_state(&self) -> AccountState {
        if self.sync.running {
            let label = if self.sync.applied > 0 { format!("Syncing… {} books", self.sync.applied) } else { "Syncing…".to_owned() };
            AccountState::Syncing(label)
        } else if self.session.connecting {
            AccountState::Connecting
        } else if self.sync.offline {
            AccountState::Offline
        } else {
            AccountState::Connected
        }
    }

    /// The full column: the library's places, then the account at the foot.
    pub(super) fn sidebar(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        ui.spacing_mut().item_spacing.y = 1.0;
        // A field that opens the palette, as on Ond's sidebar.
        if kit::search_field(ui, "⌘K").clicked() {
            self.actions.push(Action::OpenPalette);
        }
        ui.add_space(14.0);
        // The heading names the libraries the shelf holds, with the button
        // that folds the column away at its end.
        ui.horizontal(|ui| {
            ui.set_height(TOGGLE);
            ui.add_space(8.0);
            let (heading, all) = self.libraries_heading();
            let room = ui.available_width() - TOGGLE - 4.0;
            let g = crate::widgets::elide(ui, &heading, FontId::new(11.0, semibold()), t.muted, room);
            let resp = ui.add(egui::Label::new(g).selectable(false).sense(Sense::hover()));
            if let Some(all) = all {
                resp.on_hover_text(all);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::sized_button(ui, Icon::Sidebar, "Collapse sidebar (⌥⌘S)", vec2(TOGGLE, TOGGLE), 13.0, Some(t.muted)).clicked() {
                    self.actions.push(Action::Prefs(PrefsAction::CollapseSidebar(true)));
                }
            });
        });
        ui.add_space(4.0);
        let active = self.active_place();
        let coming = self.downloads_coming();
        for (i, (label, route, _)) in PLACES.into_iter().enumerate() {
            let mark = coming.filter(|_| route == Route::Downloads);
            let (room, name) = match mark {
                Some(m) => (MARK_ROOM, format!("{label}, {}", m.hint().to_lowercase())),
                None => (0.0, label.to_owned()),
            };
            let resp = shell::kit::sidebar_row(ui, label, active == Some(i), room, &name);
            if let Some(m) = mark
                && ui.is_rect_visible(resp.rect)
            {
                kit::download_mark(ui, pos2(resp.rect.right() - 8.0 - MARK_ROOM / 2.0 + 4.0, resp.rect.center().y), 11.0, m);
            }
            if resp.clicked() {
                self.actions.push(Action::Go(route));
            }
        }
        // The foot: the account, and the shortcuts and Settings beside it. Its row is
        // given its height up front: a `horizontal` in a bottom-up layout
        // starts a control's height tall and grows downwards, which put the
        // account's second line under the window's edge.
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            let size = vec2(ui.available_width(), ACCOUNT_H);
            ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let w = (ui.available_width() - 2.0 * (FOOTER_BUTTON + 2.0)).max(40.0);
                self.account_switcher(ui, kit::SwitcherPlace::Foot { width: w });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.settings_button(ui);
                    self.help_button(ui, FOOTER_BUTTON);
                });
            });
        });
    }

    /// The rail: the toggle that brings the column back (when the window has
    /// room for it), an icon per place, and Settings, the shortcuts and the
    /// account at the foot, as on Ond's.
    pub(super) fn sidebar_rail(&mut self, ui: &mut Ui, width: f32) {
        let t = Tokens::of(ui);
        ui.spacing_mut().item_spacing.y = 4.0;
        let size = vec2(RAIL_BUTTON, RAIL_BUTTON);
        let active = self.active_place();
        ui.vertical_centered(|ui| {
            if can_expand(width) {
                if icons::sized_button(ui, Icon::Sidebar, "Expand sidebar (⌥⌘S)", vec2(RAIL_BUTTON, TOGGLE), 13.0, Some(t.muted)).clicked() {
                    self.actions.push(Action::Prefs(PrefsAction::CollapseSidebar(false)));
                }
                ui.add_space(8.0);
            }
            if icons::sized_button(ui, Icon::Search, "Search (⌘K)", size, 14.0, Some(t.muted)).clicked() {
                self.actions.push(Action::OpenPalette);
            }
            ui.add_space(8.0);
            for (i, (label, route, icon)) in PLACES.into_iter().enumerate() {
                let on = active == Some(i);
                let resp = icons::sized_button(ui, icon, label, size, 15.0, Some(if on { t.text } else { t.muted }));
                if on {
                    ui.painter().rect_filled(resp.rect, 6.0, t.selected);
                    icons::paint(ui.painter(), Rect::from_center_size(resp.rect.center(), vec2(15.0, 15.0)), icon, t.text);
                }
                if resp.clicked() {
                    self.actions.push(Action::Go(route));
                }
            }
        });
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            self.settings_button(ui);
            self.help_button(ui, FOOTER_BUTTON);
            self.account_switcher(ui, kit::SwitcherPlace::Rail);
        });
    }

    /// How far along the books on their way are, together, while any are:
    /// what the Downloads place's ring shows.
    fn downloads_coming(&self) -> Option<kit::DownloadMark> {
        let snapshot = &self.downloads.snapshot;
        let coming: Vec<&offline::PinSummary> = snapshot.pins.iter().filter(|p| matches!(p.pin, offline::Pin::Book(_)) && !p.complete()).collect();
        if coming.is_empty() && self.downloads.preparing.is_empty() {
            return None;
        }
        if !self.downloads.preparing.is_empty() || coming.iter().any(|p| p.downloading) || snapshot.queued > 0 {
            let (done, total) = coming.iter().fold((0u64, 0u64), |(d, t), p| (d + p.bytes_done, t + p.bytes_total));
            let fraction = if total > 0 { done as f32 / total as f32 } else { 0.0 };
            return Some(kit::DownloadMark::Progress(fraction));
        }
        // Nothing moving and something left: it gave up.
        coming.iter().any(|p| p.failed > 0).then_some(kit::DownloadMark::Failed)
    }

    /// Which place the page belongs to: for a book, the one it was opened
    /// from.
    fn active_place(&self) -> Option<usize> {
        let route = self.nav.place();
        PLACES.iter().position(|(_, r, _)| r == route)
    }

    /// The heading over the places: the library's name, or that several are
    /// shown together, with their names for the tooltip.
    fn libraries_heading(&self) -> (String, Option<String>) {
        let libraries = self.session.account.as_ref().map_or(&[][..], |a| a.libraries.as_slice());
        match libraries {
            [] => ("Library".to_owned(), None),
            [one] => (one.name.clone(), None),
            many => {
                let names: Vec<&str> = many.iter().map(|l| l.name.as_str()).collect();
                (format!("{} libraries", many.len()), Some(format!("Shown together: {}", names.join(", "))))
            }
        }
    }

    /// The account in view: the session's, else the one the scope names,
    /// else the first.
    pub(super) fn active_account(&self) -> Option<crate::settings::Account> {
        self.session
            .account
            .clone()
            .or_else(|| self.scope.single().and_then(|id| self.accounts.iter().find(|a| a.id == id).cloned()))
            .or_else(|| self.accounts.first().cloned())
    }

    /// Every account as the switcher and Settings' Sources tab list it: the
    /// one in view at its live state with that state's line, the others
    /// signed out or not, under their server's host.
    pub(super) fn source_entries(&self) -> Vec<SourceEntry> {
        let Some(account) = self.active_account() else { return Vec::new() };
        let state = self.account_state();
        let line = state.line(host(&account.server));
        let live = || SourceEntry {
            id: account.id,
            name: account.display_name().to_owned(),
            color: account.color,
            state: state.source_state(),
            detail: line.clone(),
            live: true,
            signed_out: false,
        };
        let mut entries: Vec<SourceEntry> = self
            .accounts
            .iter()
            .map(|saved| {
                if saved.id == account.id {
                    live()
                } else {
                    SourceEntry {
                        id: saved.id,
                        name: saved.display_name().to_owned(),
                        color: saved.color,
                        state: if saved.signed_out { SourceState::SignedOut } else { SourceState::Online },
                        detail: host(&saved.server).to_owned(),
                        live: false,
                        signed_out: saved.signed_out,
                    }
                }
            })
            .collect();
        if !entries.iter().any(|e| e.id == account.id) {
            entries.push(live());
        }
        entries
    }

    /// The account at the foot of the column, through the kit's source
    /// switcher: who, and the server's state, in the column (`Some` width)
    /// or the rail. Click for its menu.
    fn account_switcher(&mut self, ui: &mut Ui, place: kit::SwitcherPlace) {
        let t = Tokens::of(ui);
        let Some(account) = self.active_account() else {
            if matches!(place, kit::SwitcherPlace::Foot { .. }) {
                ui.label(egui::RichText::new("Not signed in").size(12.0).color(t.muted));
            }
            return;
        };
        let state = self.account_state();
        let tooltip = state.tooltip(host(&account.server));
        let entries = self.source_entries();
        let sources: Vec<_> = entries.iter().map(SourceEntry::info).collect();
        let scope = Scope::only(account.id);
        // Another account is added beside the one in view, so only once
        // it's in.
        let add = self.session.signed_in();
        let switcher = kit::SourceSwitcher { sources: &sources, scope: &scope, look: kit::SwitcherLook::Card, add, manage: true, tooltip: Some(&tooltip) };
        let picked = kit::source_switcher(ui, &switcher, place, |ui| self.account_menu(ui, &account, &state));
        match picked {
            Some(kit::SourcePick::App(row)) => self.actions.push(match row {
                AccountRow::SyncNow => Action::SyncNow,
                AccountRow::Settings => Action::OpenSettings(self.settings),
                AccountRow::Shortcuts => Action::Shortcuts,
                AccountRow::SignOut => Action::AskSignOut,
            }),
            Some(kit::SourcePick::Source(id)) => self.actions.push(Action::SwitchAccount(id)),
            Some(kit::SourcePick::Add) => self.actions.push(Action::AddSource),
            Some(kit::SourcePick::Manage) => self.actions.push(Action::OpenSettings(super::settings_page::Tab::Sources)),
            Some(kit::SourcePick::All) | None => {}
        }
    }

    /// The account menu's rows under the switcher: who's signed in and
    /// where, the server's state, then Sync now, Settings (on the tab it was
    /// last on), the shortcuts, and Sign out…: Ond's server popover, from
    /// the kit.
    fn account_menu(&self, ui: &mut Ui, account: &crate::settings::Account, state: &AccountState) -> Option<AccountRow> {
        let t = Tokens::of(ui);
        let mut picked = None;
        kit::padded(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let (r, _) = ui.allocate_exact_size(vec2(AVATAR + 4.0, AVATAR + 4.0), Sense::hover());
                paint_avatar(ui, r.center(), &account.username);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let room = ui.available_width();
                    let name = crate::widgets::elide(ui, &account.username, FontId::new(13.0, semibold()), t.text, room);
                    ui.add(egui::Label::new(name).selectable(false));
                    let server = crate::widgets::elide(ui, &account.server, FontId::proportional(11.0), t.muted, room);
                    ui.add(egui::Label::new(server).selectable(false));
                });
            });
        });
        ui.add_space(8.0);
        let (status, color) = match state {
            AccountState::Connected => ("Connected".to_owned(), t.text),
            AccountState::Syncing(label) => (label.clone(), t.text),
            AccountState::Connecting => ("Connecting…".to_owned(), t.text),
            AccountState::Offline => ("Can't reach the server · retrying".to_owned(), t.warn),
        };
        kit::info_row(ui, "Status", &status, color);
        let libraries = account.libraries.len();
        if libraries > 1 {
            kit::info_row(ui, "Libraries", &libraries.to_string(), t.text);
        }
        kit::popover_rule(ui, 6.0);
        let can_sync = self.session.source.is_some() && !self.sync.running;
        if can_sync && kit::menu_row(ui, "Sync now") {
            picked = Some(AccountRow::SyncNow);
        }
        if kit::menu_row(ui, "Settings…") {
            picked = Some(AccountRow::Settings);
        }
        if kit::menu_row(ui, "Keyboard shortcuts") {
            picked = Some(AccountRow::Shortcuts);
        }
        kit::popover_rule(ui, 6.0);
        if kit::menu_row(ui, "Sign out…") {
            picked = Some(AccountRow::SignOut);
        }
        picked
    }

    /// The gear: Settings, on the tab it was last on.
    fn settings_button(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        if icons::sized_button(ui, Icon::Gear, "Settings (⌘,)", vec2(FOOTER_BUTTON, FOOTER_BUTTON), 15.0, Some(t.muted)).clicked() {
            self.actions.push(Action::OpenSettings(self.settings));
        }
    }

    /// A question mark in a circle: the keyboard shortcuts, as on Ond's
    /// sidebar.
    fn help_button(&mut self, ui: &mut Ui, side: f32) {
        let t = Tokens::of(ui);
        if icons::sized_button(ui, Icon::Help, "Keyboard shortcuts (⌘/)", vec2(side, side), 15.0, Some(t.muted)).clicked() {
            self.actions.push(Action::Shortcuts);
        }
    }
}

/// An account as the switcher and the Sources tab show it.
pub(super) struct SourceEntry {
    pub id: accounts::AccountId,
    pub name: String,
    pub color: accounts::SourceColor,
    pub state: SourceState,
    /// The server's host, or the account in view's live line.
    pub detail: String,
    /// The account in view.
    pub live: bool,
    pub signed_out: bool,
}

impl SourceEntry {
    /// The entry as the kit's widgets take it.
    pub fn info(&self) -> kit::SourceInfo<'_> {
        kit::SourceInfo {
            id: self.id,
            name: &self.name,
            kind: "Audiobookshelf",
            icon: Icon::Headphones,
            color: self.color,
            state: &self.state,
            detail: Some(&self.detail),
        }
    }
}

/// A row of the account menu, as it answers through the switcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccountRow {
    SyncNow,
    Settings,
    Shortcuts,
    SignOut,
}

/// The account's avatar in its menu: its initial in a quiet circle, as the
/// switcher draws it at the foot.
const AVATAR: f32 = 24.0;

fn paint_avatar(ui: &Ui, c: egui::Pos2, name: &str) {
    let t = Tokens::of(ui);
    ui.painter().circle_filled(c, AVATAR / 2.0, t.accent.gamma_multiply(0.18));
    let initial: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    ui.painter().text(c, egui::Align2::CENTER_CENTER, initial, FontId::new(12.0, semibold()), t.accent);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_column_gives_way_to_the_rail_when_collapsed_or_out_of_room() {
        assert_eq!(plan(1100.0, false), Some(SIDEBAR_W));
        assert_eq!(plan(1100.0, true), None, "collapsed");
        assert_eq!(plan(SIDEBAR_W + CONTENT_MIN, false), Some(SIDEBAR_W));
        assert_eq!(plan(SIDEBAR_W + CONTENT_MIN - 1.0, false), None, "a narrow window keeps the page its room");
        assert_eq!(plan(crate::MIN_SIZE[0], false), None);
    }

    #[test]
    fn the_footer_names_the_server_by_its_host() {
        assert_eq!(host("https://books.example.com"), "books.example.com");
        assert_eq!(host("http://10.0.0.2:13378/abs/"), "10.0.0.2:13378");
        assert_eq!(host("books.local"), "books.local");
    }
}
