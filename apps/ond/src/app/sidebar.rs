//! Left column: search (opens the palette), library places, playlists, and
//! settings. In narrow windows it collapses to a rail of icons.

use super::view_ctx::ViewCtx;
use super::{Action, DragTracks, OndApp, PlaylistAction};
use crate::icons::{self, Icon};
use crate::nav::{Nav, Route};
use crate::theme::{Tokens, semibold};
use accounts::{AccountId, OfflineReason, Scope, SourceColor, SourceState};
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};
use shell::kit::{self, info_row};

/// What a source is called when it hasn't been given a name: a server's
/// host, a folder source's first folder.
pub(super) fn default_name(entry: &crate::settings::SourceEntry) -> String {
    match entry.kind {
        crate::settings::SourceKind::Subsonic => super::diagnostics::server_host(&entry.server).to_owned(),
        crate::settings::SourceKind::Local => {
            entry.folders.first().and_then(|f| f.path.file_name()).map_or_else(|| "Music folder".to_owned(), |n| n.to_string_lossy().into_owned())
        }
    }
}

/// Width of the icon rail (the full sidebar's is the user's, see `layout`).
pub const RAIL_W: f32 = 52.0;
/// The footer's buttons (and the server switcher's height, in its one-line
/// look).
const FOOTER_BUTTON: f32 = 28.0;
/// The notice that a newer Ond is out.
const UPDATE_H: f32 = 28.0;

/// In ⌘1…⌘6 order.
pub(super) const PLACES: [(&str, Route, Icon); 6] = [
    ("Recently added", Route::Recent, Icon::Clock),
    ("Artists", Route::Artists, Icon::Person),
    ("Albums", Route::Albums, Icon::Grid),
    ("Songs", Route::Songs, Icon::Note),
    ("Favorites", Route::Favorites, Icon::Heart),
    ("Offline", Route::Offline, Icon::Download),
];

impl OndApp {
    pub(super) fn sidebar(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        ui.spacing_mut().item_spacing.y = 1.0;

        // A search field that opens the palette (the palette is the real search).
        if shell::kit::search_field(ui, "⌘K").clicked() {
            self.actions.push(Action::OpenPalette);
        }
        ui.add_space(18.0);

        let active = active_place(&self.ui.nav);
        let caps = self.session.caps();
        self.section(ui, "Library");
        for (i, (label, route, _)) in PLACES.into_iter().enumerate() {
            if !place_shown(&route, caps) {
                continue;
            }
            let on = active == Some(i);
            let is_offline = route == Route::Offline;
            let resp = self.place(ui, label, on);
            if resp.clicked() {
                self.actions.push(Action::Go(route));
            }
            // While downloads run, a small ring says how far along they are.
            if is_offline && let Some(f) = self.download_progress() {
                icons::progress_ring(ui.painter(), pos2(resp.rect.right() - 12.0, resp.rect.center().y), 5.0, f, t.accent, t.hairline);
            }
        }
        ui.add_space(18.0);
        let bottom_h = if self.update.is_some() { 34.0 + UPDATE_H + 6.0 } else { 34.0 };
        if caps.playlists {
            self.playlists_section(ui, bottom_h);
        }

        // The footer: the server's state on the left (click for details and
        // Sign out), keyboard shortcuts and Settings on the right.
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let status_w = (ui.available_width() - 2.0 * (FOOTER_BUTTON + 2.0) - 4.0).max(40.0);
                self.server_switcher(ui, kit::SwitcherPlace::Foot { width: status_w });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.settings_button(ui);
                    self.help_button(ui);
                });
            });
            if let Some(v) = self.update {
                ui.add_space(6.0);
                self.update_notice(ui, v);
            }
        });
    }

    /// "Playlists" with a + for a new one, and the playlists under it, for a
    /// source that holds them.
    fn playlists_section(&mut self, ui: &mut Ui, bottom_h: f32) {
        let t = Tokens::of(ui);
        // "Playlists" with a + for a new one, in line with the section label.
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.set_height(18.0);
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Playlists").font(FontId::new(11.0, semibold())).color(t.muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::sized_button(ui, Icon::Plus, "New Playlist (⌘N)", vec2(20.0, 18.0), 9.0, Some(t.muted)).clicked() {
                    self.actions.push(Action::NewPlaylist);
                }
            });
        });
        ui.add_space(4.0);
        let dragging = egui::DragAndDrop::has_payload_of_type::<DragTracks>(ui.ctx());
        egui::ScrollArea::vertical()
            .id_salt("playlists")
            .max_height((ui.available_height() - bottom_h).max(40.0))
            .show(ui, |ui| self.view().sidebar_playlists(ui, dragging));
    }

    /// A newer Ond is out, above the footer: click for Settings › About,
    /// with its release notes, Download and Skip.
    fn update_notice(&mut self, ui: &mut Ui, v: crate::updates::Version) {
        let t = Tokens::of(ui);
        let label = format!("Ond {v} is available");
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), UPDATE_H), Sense::click());
        if ui.is_rect_visible(rect) {
            let p = ui.painter();
            p.rect_filled(rect, 6.0, t.accent.gamma_multiply(if resp.hovered() { 0.2 } else { 0.12 }));
            icons::paint(p, Rect::from_center_size(pos2(rect.left() + 16.0, rect.center().y), vec2(12.0, 12.0)), Icon::Download, t.accent);
            p.text(pos2(rect.left() + 30.0, rect.center().y), egui::Align2::LEFT_CENTER, &label, FontId::proportional(12.5), t.accent);
        }
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.actions.push(Action::OpenSettings(super::settings_page::Section::About));
        }
    }

    /// The narrow-window sidebar: one icon per place, playlists behind a
    /// menu, settings at the bottom.
    pub(super) fn sidebar_rail(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        ui.spacing_mut().item_spacing.y = 4.0;
        let size = vec2(32.0, 32.0);
        let active = active_place(&self.ui.nav);
        let caps = self.session.caps();
        ui.vertical_centered(|ui| {
            if icons::sized_button(ui, Icon::Search, "Search (⌘K)", size, 14.0, Some(t.muted)).clicked() {
                self.actions.push(Action::OpenPalette);
            }
            ui.add_space(12.0);
            for (i, (label, route, icon)) in PLACES.into_iter().enumerate() {
                if !place_shown(&route, caps) {
                    continue;
                }
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
            if !caps.playlists {
                return;
            }
            ui.add_space(12.0);
            let lists = icons::sized_button(ui, Icon::List, "Playlists", size, 14.0, Some(t.muted));
            egui::Popup::menu(&lists).show(|ui| {
                ui.set_width(220.0);
                if self.data.playlists.is_empty() {
                    ui.weak("No playlists yet");
                }
                for p in &self.data.playlists {
                    if ui.button(&p.name).clicked() {
                        self.actions.push(Action::Go(Route::Playlist { id: p.id.clone(), name: p.name.clone() }));
                    }
                }
            });
        });
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            self.rail_settings_button(ui);
            self.help_button(ui);
            self.server_switcher(ui, kit::SwitcherPlace::Rail);
        });
    }

    /// The server in the footer, through the kit's source switcher with
    /// the one account Ond signs in to: a check and the host while
    /// connected, a ring and progress while syncing, a cloud while offline,
    /// or the glyph alone in the rail (the words in a tooltip). Click for
    /// the details popover (latency, last sync, Sync now, Sign out).
    fn server_switcher(&mut self, ui: &mut Ui, place: kit::SwitcherPlace) {
        let state = self.server_state();
        let source_state = state.source_state();
        let id = self.source_id();
        let (name, color) = self.source_look(&state);
        let kind = self.session.kind();
        let sources = [kit::SourceInfo { id, name: &name, kind: kind.name, icon: kind.icon, color, state: &source_state, detail: Some(state.label()) }];
        let scope = Scope::only(id);
        let tooltip = state.tooltip(self.session.caps().network);
        let switcher =
            kit::SourceSwitcher { sources: &sources, scope: &scope, look: kit::SwitcherLook::Line, add: false, manage: true, tooltip: Some(&tooltip) };
        let picked = kit::source_switcher(ui, &switcher, place, |ui| self.server_menu(ui, &state));
        if picked == Some(kit::SourcePick::Manage) {
            return self.actions.push(Action::OpenSettings(super::settings_page::Section::Sources));
        }
        let Some(kit::SourcePick::App(row)) = picked else { return };
        self.actions.push(match row {
            ServerRow::MeasureLatency => Action::MeasureLatency,
            ServerRow::SyncNow => Action::SyncNow,
            ServerRow::StopSync => Action::StopSync,
            ServerRow::SignInAgain => Action::SignInAgain,
            ServerRow::WorkOffline(on) => Action::Offline(super::OfflineAction::WorkOffline(on)),
            ServerRow::SignOut => Action::AskSignOut,
        });
    }

    /// The server's account id. Before the first connect has given it one,
    /// a stand-in that stays put, so a spinner counts from when it started.
    pub(super) fn source_id(&self) -> AccountId {
        static NO_ACCOUNT: std::sync::LazyLock<AccountId> = std::sync::LazyLock::new(AccountId::new);
        self.session.account().unwrap_or(*NO_ACCOUNT)
    }

    /// What the server is called and its colour: the name it was given in
    /// the Sources tab, else its host.
    pub(super) fn source_look(&self, state: &ServerState) -> (String, SourceColor) {
        let saved = self.session.settings.as_ref();
        let name = saved.and_then(|s| s.source_name()).map(str::to_owned).unwrap_or_else(|| state.host().to_owned());
        (name, saved.map_or_else(SourceColor::default, |s| s.source_color()))
    }

    pub(super) fn server_state(&self) -> ServerState {
        // A server's host; for a source that isn't one, its default name.
        let host = self.session.settings.as_ref().and_then(|s| s.primary()).map(default_name).unwrap_or_default();
        let scans = self.session.kind().scans;
        if let Some(p) = self.sync.progress {
            let fraction = (p.total > 0).then(|| p.done as f32 / p.total as f32);
            let (done, total) = (crate::fmt::thousands(p.done), crate::fmt::thousands(p.total));
            // Short enough for the footer; the counts are in the tooltip and popover.
            let label = match fraction {
                _ if scans => format!("Scanning · {done} tracks"),
                Some(f) => format!("Syncing · {:.0}%", 100.0 * f),
                None => "Syncing…".to_owned(),
            };
            let what = match p.unit {
                crate::sync::Unit::Tracks => "tracks",
                crate::sync::Unit::Albums => "albums",
            };
            let detail =
                if scans { format!("Scanning the music folder\n{done} of {total} {what}") } else { format!("{}\n{done} of {total} {what}", p.label()) };
            return ServerState::Syncing { host, label, detail, fraction, scans };
        }
        if self.session.online && (self.sync.albums || self.sync.tracks) {
            let (label, detail) = if scans { ("Scanning…", "Looking for music") } else { ("Syncing library…", "Fetching the album list") };
            return ServerState::Syncing { host, label: label.into(), detail: detail.into(), fraction: None, scans };
        }
        if !self.session.folder_missing.is_empty() {
            return ServerState::FolderMissing { host };
        }
        if self.session.online {
            return ServerState::Connected { host };
        }
        if self.session.work_offline {
            return ServerState::WorkingOffline { host };
        }
        if self.session.sign_in.is_some() {
            return ServerState::SignIn { host };
        }
        if self.session.unreachable {
            return ServerState::Offline { host };
        }
        ServerState::Connecting { host }
    }

    /// The switcher's menu: details about the server, Sync now and Sign
    /// out. What's chosen comes back for the switcher to hand on.
    fn server_menu(&self, ui: &mut Ui, state: &ServerState) -> Option<ServerRow> {
        let t = Tokens::of(ui);
        // Opening the popover refreshes the round trip if it's old.
        let mut picked = (self.session.online && self.diagnostics.latency_stale()).then_some(ServerRow::MeasureLatency);
        let caps = self.session.caps();
        let kind = self.session.kind();
        let entry = self.session.settings.as_ref().and_then(|s| s.primary());
        // Under the name: a server's address and user, or a folder source's folders.
        let (server, user) = match entry {
            Some(e) if caps.network => (e.server.clone(), e.username.clone()),
            Some(e) => (e.folders.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>().join(", "), String::new()),
            None => Default::default(),
        };
        let (title, _) = self.source_look(state);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(title).font(FontId::new(13.0, semibold())).color(t.text));
        });
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.add(egui::Label::new(egui::RichText::new(&server).size(11.0).color(t.muted)).truncate());
        });
        ui.add_space(10.0);
        let status = match state {
            ServerState::Connected { .. } | ServerState::Syncing { .. } if !caps.network => "Ready".to_owned(),
            ServerState::Connected { .. } | ServerState::Syncing { .. } => match self.diagnostics.latency() {
                Some(rtt) => format!("Connected · {} ms", rtt.as_millis()),
                None => "Connected".to_owned(),
            },
            ServerState::Offline { .. } => "Can't reach the server · retrying".to_owned(),
            ServerState::FolderMissing { .. } => "Folder missing".to_owned(),
            ServerState::Connecting { .. } => "Connecting…".to_owned(),
            ServerState::SignIn { .. } => "Sign-in needed".to_owned(),
            ServerState::WorkingOffline { .. } => "Offline mode".to_owned(),
        };
        let status_color = match state {
            ServerState::Offline { .. } | ServerState::FolderMissing { .. } | ServerState::SignIn { .. } => t.warn,
            _ => t.text,
        };
        info_row(ui, "Status", &status, status_color);
        if let (ServerState::SignIn { .. }, Some(why)) = (state, &self.session.sign_in) {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.add(egui::Label::new(egui::RichText::new(why).size(11.0).color(t.muted)).wrap());
            });
        }
        if !user.is_empty() {
            info_row(ui, "User", &user, t.text);
        }
        if !self.session.server_info.is_empty() && caps.network {
            info_row(ui, "Server", &self.session.server_info, t.text);
        }
        let synced = match state {
            ServerState::Syncing { detail, scans, .. } => {
                format!("{} {}", if *scans { "Scanning" } else { "Syncing" }, detail.lines().last().unwrap_or_default())
            }
            _ => match self.sync.last {
                Some(at) => at.format("%-I:%M %p").to_string(),
                None if self.session.source.is_some() => "At launch".to_owned(),
                None => "Not yet".to_owned(),
            },
        };
        info_row(ui, if kind.scans { "Last scan" } else { "Last sync" }, &synced, t.text);
        let action = if self.sync.tracks {
            Some(if kind.scans { "Stop scan" } else { "Stop sync" })
        } else if self.session.source.is_some() && self.session.online {
            Some(kind.sync)
        } else {
            None
        };
        if let Some(label) = action {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                if crate::widgets::link(ui, label, FontId::proportional(12.0), t.accent).clicked() {
                    if self.sync.tracks {
                        picked = Some(ServerRow::StopSync);
                    } else {
                        picked = Some(ServerRow::SyncNow);
                        ui.close();
                    }
                }
            });
        }
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
        if matches!(state, ServerState::SignIn { .. }) && menu_item(ui, "Sign in…") {
            picked = Some(ServerRow::SignInAgain);
        }
        // A folder on this computer has no connection to drop and no
        // sign-in: neither row is offered for it.
        if !caps.network {
            ui.add_space(4.0);
            return picked;
        }
        let (label, on) = if self.session.work_offline { ("Go online", false) } else { ("Go offline", true) };
        if !self.prefs.managed("work_offline") && menu_item(ui, label) {
            picked = Some(ServerRow::WorkOffline(on));
        }
        // Apart from "Go offline", which it sits under: one is undone
        // with a click, the other with the password.
        ui.add_space(2.0);
        ui.separator();
        ui.add_space(2.0);
        if menu_item(ui, "Sign out…") {
            picked = Some(ServerRow::SignOut);
        }
        picked
    }

    /// The gear: opens Settings.
    fn settings_button(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let button = icons::sized_button(ui, Icon::Gear, "Settings (⌘,)", vec2(FOOTER_BUTTON, FOOTER_BUTTON), 15.0, Some(t.muted));
        if button.clicked() {
            self.actions.push(Action::OpenSettings(self.ui.settings_section));
        }
    }

    /// The rail's gear: with a dot while a newer Ond is out, and then it opens
    /// Settings › About.
    fn rail_settings_button(&mut self, ui: &mut Ui) {
        let Some(v) = self.update else { return self.settings_button(ui) };
        let t = Tokens::of(ui);
        let tip = format!("Settings (⌘,)\nOnd {v} is available");
        let button = icons::sized_button(ui, Icon::Gear, &tip, vec2(FOOTER_BUTTON, FOOTER_BUTTON), 15.0, Some(t.muted));
        let dot = pos2(button.rect.center().x + 6.0, button.rect.center().y - 6.0);
        ui.painter().circle_filled(dot, 4.0, t.sidebar);
        ui.painter().circle_filled(dot, 3.0, t.accent);
        if button.clicked() {
            self.actions.push(Action::OpenSettings(super::settings_page::Section::About));
        }
    }

    /// A question mark in a circle: the keyboard shortcuts.
    fn help_button(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let button = icons::sized_button(ui, Icon::Help, "Keyboard shortcuts (⌘/)", vec2(FOOTER_BUTTON, FOOTER_BUTTON), 15.0, Some(t.muted));
        if button.clicked() {
            self.actions.push(Action::ToggleShortcuts);
        }
    }

    /// Overall progress of unfinished downloads, while any are moving.
    fn download_progress(&self) -> Option<f32> {
        let s = &self.downloads.snap;
        if s.paused || s.active == 0 {
            return None;
        }
        let (done, total) = s.pins.iter().filter(|p| !p.complete()).fold((0u64, 0u64), |(d, t), p| (d + p.bytes_done, t + p.bytes_total));
        Some(if total > 0 { done as f32 / total as f32 } else { 0.0 })
    }

    /// The sidebar entry the current place belongs to.
    fn section(&self, ui: &mut Ui, text: &str) {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            crate::widgets::section(ui, text);
        });
        ui.add_space(4.0);
    }

    pub(super) fn place(&self, ui: &mut Ui, label: &str, on: bool) -> egui::Response {
        place(ui, label, on, None)
    }
}

impl ViewCtx<'_> {
    /// The playlists under the places: a row each, a drop target for songs
    /// (`dragging`: songs are being dragged), and a menu.
    fn sidebar_playlists(&mut self, ui: &mut Ui, dragging: bool) {
        let t = Tokens::of(ui);
        // Borrowed from the library, not from `self`, so rows can take `&mut self`.
        let data = self.data;
        if data.playlists.is_empty() {
            ui.label(egui::RichText::new("  None yet").small().color(t.muted));
        }
        let active = match active_route(&self.ui.nav) {
            Some(Route::Playlist { id, .. }) => data.playlists.iter().position(|p| p.id == *id),
            _ => None,
        };
        for (i, p) in data.playlists.iter().enumerate() {
            let mine = owns(self.session, p);
            // Downloaded (or on its way): the same mark as on album cards.
            let mark = super::offline_page::pin_mark(self.downloads.snap.pin(&offline::Pin::Playlist(p.id.clone())));
            let resp = place(ui, &p.name, active == Some(i), mark);
            // A drop target for songs dragged from any list.
            if mine && dragging {
                let over = resp.dnd_hover_payload::<DragTracks>().is_some_and(|d| d.from.as_ref().is_none_or(|(from, _)| *from != p.id));
                if over {
                    ui.painter().rect_stroke(resp.rect, 6.0, egui::Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
                }
                if let Some(drag) = resp.dnd_release_payload::<DragTracks>()
                    && drag.from.as_ref().is_none_or(|(from, _)| *from != p.id)
                {
                    self.actions.push(Action::PlaylistEdit(PlaylistAction::Add { id: p.id.clone(), tracks: drag.tracks.clone() }));
                }
            }
            if resp.clicked() {
                self.actions.push(Action::Go(Route::Playlist { id: p.id.clone(), name: p.name.clone() }));
            }
            resp.context_menu(|ui| {
                let kept = self.downloads.snap.pin(&offline::Pin::Playlist(p.id.clone())).is_some();
                super::browse::collection_menu(
                    ui,
                    &super::browse::Collection::Playlist(p.id.clone()),
                    true,
                    self.session.caps().downloads.then_some(kept),
                    None,
                    self.actions,
                );
                if mine {
                    ui.separator();
                    self.playlist_edit_items(ui, &p.id, &p.name);
                }
            });
        }
    }
}

/// Whether a source that can do `caps` has anything at `route`: Favorites
/// for one that holds hearts, Offline for one whose music downloads.
pub(super) fn place_shown(route: &Route, caps: crate::source::Capabilities) -> bool {
    match route {
        Route::Favorites => caps.stars,
        Route::Offline => caps.downloads,
        _ => true,
    }
}

/// The root place the page belongs to: itself, or where its trail started.
fn active_route(nav: &Nav) -> Option<&Route> {
    let current = nav.current();
    if current.is_root() {
        return Some(current);
    }
    nav.trail().first().copied().filter(|r| r.is_root())
}

/// Which of [`PLACES`] the page belongs to.
fn active_place(nav: &Nav) -> Option<usize> {
    let active = active_route(nav)?;
    PLACES.iter().position(|(_, r, _)| r == active)
}

/// Playlists the user can change (others' public ones are read-only).
pub(super) fn owns(session: &super::Session, p: &crate::source::Playlist) -> bool {
    match (&p.owner, &session.source, &session.settings) {
        (Some(owner), Some(_), Some(settings)) => owner == settings.username(),
        _ => true,
    }
}

/// Room a place's download mark takes at its right end.
const MARK_ROOM: f32 = 16.0;

/// A place in the sidebar: its name, highlighted when `on`, and a download
/// mark at the right end when it has one.
fn place(ui: &mut Ui, label: &str, on: bool, mark: Option<super::offline_page::Mark>) -> egui::Response {
    let room = if mark.is_some() { MARK_ROOM } else { 0.0 };
    let name = match mark {
        Some(m) => format!("{label}, {}", m.hint().to_lowercase()),
        None => label.to_owned(),
    };
    let resp = crate::widgets::sidebar_row(ui, label, on, room, &name);
    if let Some(m) = mark
        && ui.is_rect_visible(resp.rect)
    {
        super::offline_page::mark(ui, pos2(resp.rect.right() - 8.0 - MARK_ROOM / 2.0 + 4.0, resp.rect.center().y), 11.0, m);
    }
    resp
}

/// A row in the server menu. True when chosen.
fn menu_item(ui: &mut Ui, label: &str) -> bool {
    crate::widgets::menu_row(ui, label)
}

/// The server's state as the footer shows it.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum ServerState {
    Connected {
        host: String,
    },
    Syncing {
        host: String,
        label: String,
        detail: String,
        fraction: Option<f32>,
        /// A scan of folders on this computer, not a server's sync.
        scans: bool,
    },
    /// Not answering; reconnecting in the background.
    Offline {
        host: String,
    },
    /// The first connect is still under way.
    Connecting {
        host: String,
    },
    /// The server turned the saved login down.
    SignIn {
        host: String,
    },
    /// Offline by choice: not connecting.
    WorkingOffline {
        host: String,
    },
    /// A configured local folder is missing or unreadable.
    FolderMissing {
        host: String,
    },
}

impl ServerState {
    pub(super) fn host(&self) -> &str {
        match self {
            ServerState::Connected { host }
            | ServerState::Syncing { host, .. }
            | ServerState::Offline { host }
            | ServerState::FolderMissing { host }
            | ServerState::Connecting { host }
            | ServerState::SignIn { host }
            | ServerState::WorkingOffline { host } => host,
        }
    }

    /// What the footer says.
    pub(super) fn label(&self) -> &str {
        match self {
            ServerState::Connected { host } => host,
            ServerState::Syncing { label, .. } => label,
            ServerState::Offline { .. } => "Can't reach the server",
            ServerState::FolderMissing { .. } => "Folder missing",
            ServerState::Connecting { .. } => "Connecting…",
            ServerState::SignIn { .. } => "Sign in again",
            ServerState::WorkingOffline { .. } => "Offline mode",
        }
    }

    fn tooltip(&self, network: bool) -> String {
        match self {
            ServerState::Connected { host } if !network => format!("{host}: music on this computer. Click for details."),
            ServerState::Connected { host } => format!("Connected to {host}. Click for details."),
            ServerState::Syncing { detail, .. } => format!("{detail}\nYou can browse while it runs. New albums and tracks appear as they arrive."),
            ServerState::Offline { .. } => "Can't reach the server. Downloaded music still plays, and Ond keeps trying to reconnect.".into(),
            ServerState::FolderMissing { .. } => "A configured music folder is missing or unreadable. Existing albums stay in your library.".into(),
            ServerState::Connecting { host } => format!("Connecting to {host}…"),
            ServerState::SignIn { .. } => "The server didn't accept the saved login. Downloaded music still plays. Click to sign in.".into(),
            ServerState::WorkingOffline { .. } => {
                "Offline mode: Ond doesn't connect to the server, and shows only music that plays without it. Click to go online.".into()
            }
        }
    }

    /// The state as the kit's switcher draws it.
    pub(super) fn source_state(&self) -> SourceState {
        match self {
            ServerState::Connected { .. } => SourceState::Online,
            ServerState::Syncing { scans: true, .. } => SourceState::Scanning,
            ServerState::Syncing { fraction, .. } => SourceState::Syncing(*fraction),
            ServerState::Offline { .. } => SourceState::Offline(OfflineReason::Unreachable),
            ServerState::FolderMissing { .. } => SourceState::FolderMissing,
            ServerState::Connecting { .. } => SourceState::Connecting,
            ServerState::SignIn { .. } => SourceState::SignedOut,
            ServerState::WorkingOffline { .. } => SourceState::Offline(OfflineReason::ByChoice),
        }
    }
}

/// A row of the server menu, as it answers through the switcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerRow {
    /// Not a row: the menu opened with an old round trip to refresh.
    MeasureLatency,
    SyncNow,
    StopSync,
    SignInAgain,
    WorkOffline(bool),
    SignOut,
}
