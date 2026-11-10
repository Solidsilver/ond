//! The column at the left of the full window, in Ond's sidebar's idiom: a
//! search field that opens the palette (⌘K), the places to browse under the
//! library's name, and the account at the foot, with its state (connected,
//! syncing, offline) and its menu, and the keyboard shortcuts beside it.
//!
//! The places are a list so Series, Authors, Collections and Downloads join
//! Library as rows when they come. The account's libraries are listed
//! together on one shelf, so the heading names them rather than switching
//! between them; a switcher comes with per-library browsing.
//!
//! Collapsed (⌥⌘S, or the button by the heading) it's a rail of icons, as
//! Ond's sidebar becomes in a narrow window; a window too narrow for the full
//! column shows the rail whatever the preference, so nothing goes missing.

use super::nav::Route;
use super::view_ctx::ViewCtx;
use super::{Action, PrefsAction, RedeApp};
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};
use shell::icons::{self, Icon};
use shell::kit;
use shell::theme::{Tokens, semibold};
use std::time::Duration;

/// The full column's width: Ond's sidebar at its default.
pub const SIDEBAR_W: f32 = 216.0;
/// The rail's: Ond's.
pub const RAIL_W: f32 = 52.0;
/// The page keeps at least this much beside the full column before it gives
/// way to the rail, as Ond's content does.
const CONTENT_MIN: f32 = 604.0;
/// The account's row at the foot, and the buttons beside it.
const ACCOUNT_H: f32 = 40.0;
/// Room under the foot, in both the column and the rail: a little more than
/// Ond's 10, so the avatar stays clear of the window's rounded corner on
/// macOS.
pub(super) const FOOT_MARGIN: i8 = 12;
const FOOTER_BUTTON: f32 = 28.0;
const RAIL_BUTTON: f32 = 32.0;
const TOGGLE: f32 = 22.0;
/// The account menu's width: Ond's server popover's.
const ACCOUNT_MENU_W: f32 = 250.0;

/// The places to browse, in order: what they're called, where they lead, and
/// the rail's icon for them.
const PLACES: [(&str, Route, Icon); 1] = [("Library", Route::Library, Icon::Grid)];

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

    /// The small glyph at `c`: a dot, a spinner, or a cloud.
    fn paint_glyph(&self, ui: &Ui, c: egui::Pos2) {
        let t = Tokens::of(ui);
        match self {
            Self::Connected => {
                ui.painter().circle_filled(c, 3.2, t.ok);
            }
            Self::Syncing(_) | Self::Connecting => {
                icons::paint(ui.painter(), Rect::from_center_size(c, vec2(11.0, 11.0)), Icon::Spinner, t.muted);
                // A sync that stalls, or a connect that hangs, can't hold the
                // window at 20 frames a second for ever: the spinner stops.
                let now = ui.input(|i| i.time);
                let since = ui.data_mut(|d| *d.get_temp_mut_or_insert_with(spinning_since_id(), || now));
                if now - since < SPIN_SECS {
                    ui.ctx().request_repaint_after(SPIN_FRAME);
                }
            }
            Self::Offline => icons::paint(ui.painter(), Rect::from_center_size(c, vec2(12.0, 12.0)), Icon::Offline, t.muted),
        }
        if !matches!(self, Self::Syncing(_) | Self::Connecting) {
            ui.data_mut(|d| d.remove::<f64>(spinning_since_id()));
        }
    }
}

/// How long the spinner turns before it goes still.
const SPIN_SECS: f64 = 30.0;
const SPIN_FRAME: Duration = Duration::from_millis(50);

fn spinning_since_id() -> egui::Id {
    egui::Id::unique("account-spinning-since")
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
        for (i, (label, route, _)) in PLACES.into_iter().enumerate() {
            if shell::kit::sidebar_row(ui, label, active == Some(i), 0.0, label).clicked() {
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
                self.account_row(ui, w);
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
            self.rail_account(ui);
        });
    }

    /// Which place the page belongs to: the library, for a book opened from
    /// it.
    fn active_place(&self) -> Option<usize> {
        let route = match self.nav.current() {
            Route::Book(_) => &Route::Library,
            route => route,
        };
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

    /// The account at the foot of the column: who, and the server's state.
    /// Click for its menu.
    fn account_row(&mut self, ui: &mut Ui, width: f32) {
        let t = Tokens::of(ui);
        let Some(account) = self.session.account.clone() else {
            ui.label(egui::RichText::new("Not signed in").size(12.0).color(t.muted));
            return;
        };
        let state = self.account_state();
        let host = host(&account.server).to_owned();
        let (rect, resp) = ui.allocate_exact_size(vec2(width, ACCOUNT_H), Sense::click());
        if ui.is_rect_visible(rect) {
            let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
            let fill = if resp.is_pointer_button_down_on() || open {
                t.selected
            } else if resp.hovered() {
                t.hover
            } else {
                egui::Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, 6.0, fill);
            shell::kit::focus_ring(ui, &resp, rect, 6.0);
            let avatar = egui::pos2(rect.left() + 8.0 + AVATAR / 2.0, rect.center().y);
            paint_avatar(ui, avatar, &account.username);
            let x = avatar.x + AVATAR / 2.0 + 8.0;
            let room = rect.right() - x - 6.0;
            let name = crate::widgets::elide(ui, &account.username, FontId::new(12.5, semibold()), t.text, room);
            let line = crate::widgets::elide(ui, &state.line(&host), FontId::proportional(11.0), t.muted, room - 12.0);
            let top = rect.center().y - (name.size().y + line.size().y) / 2.0;
            let line_y = top + name.size().y;
            ui.painter().galley(pos2(x, top), name, t.text);
            state.paint_glyph(ui, pos2(x + 4.0, line_y + line.size().y / 2.0));
            ui.painter().galley(pos2(x + 12.0, line_y), line, t.muted);
        }
        let label = format!("{}, {}", account.username, state.line(&host));
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
        let tip = state.tooltip(&host);
        let resp = resp.on_hover_text(tip);
        self.account_menu(&resp, &state);
    }

    /// The rail's account: the avatar, with the state's glyph as a badge, and
    /// the same menu.
    fn rail_account(&mut self, ui: &mut Ui) {
        let Some(account) = self.session.account.clone() else { return };
        let t = Tokens::of(ui);
        let state = self.account_state();
        let host = host(&account.server).to_owned();
        let (rect, resp) = ui.allocate_exact_size(vec2(RAIL_BUTTON, RAIL_BUTTON), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(rect, 6.0, t.hover);
        }
        shell::kit::focus_ring(ui, &resp, rect, 6.0);
        paint_avatar(ui, rect.center(), &account.username);
        let badge = rect.center() + vec2(AVATAR / 2.0 - 1.0, AVATAR / 2.0 - 1.0);
        ui.painter().circle_filled(badge, 5.5, t.sidebar);
        state.paint_glyph(ui, badge);
        let label = format!("{}, {}", account.username, state.line(&host));
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
        let tip = format!("{label}\n{}", state.tooltip(&host));
        let resp = resp.on_hover_text(tip);
        self.account_menu(&resp, &state);
    }

    /// Who's signed in and where, the server's state, then Sync now,
    /// Settings (on the tab it was last on), the shortcuts, and Sign out…, under
    /// `resp`: Ond's server popover, from the kit.
    fn account_menu(&mut self, resp: &egui::Response, state: &AccountState) {
        let Some(account) = self.session.account.clone() else { return };
        // Up from the foot of the window, where there's room for it.
        kit::popover_at(resp, ACCOUNT_MENU_W, egui::RectAlign::TOP_START, |ui| {
            let t = Tokens::of(ui);
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
                self.actions.push(Action::SyncNow);
            }
            if kit::menu_row(ui, "Settings…") {
                self.actions.push(Action::OpenSettings(self.settings));
            }
            if kit::menu_row(ui, "Keyboard shortcuts") {
                self.actions.push(Action::Shortcuts);
            }
            kit::popover_rule(ui, 6.0);
            if kit::menu_row(ui, "Sign out…") {
                self.actions.push(Action::AskSignOut);
            }
        });
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

/// The account's avatar: its initial in a quiet circle.
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
