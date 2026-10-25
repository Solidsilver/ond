//! Left column: search (opens the palette), library places, playlists, and
//! settings. In narrow windows it collapses to a rail of icons.

use super::{Action, DragTracks, OndApp, PlaylistAction};
use crate::icons::{self, Icon};
use crate::nav::Route;
use crate::theme::{Tokens, semibold};
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};

/// Width of the icon rail (the full sidebar's is the user's, see `layout`).
pub const RAIL_W: f32 = 52.0;
/// The footer's buttons (and the server status row's height).
const FOOTER_BUTTON: f32 = 28.0;

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
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
        ui.painter().rect_filled(rect, 6.0, if resp.hovered() { t.selected } else { t.field });
        icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.left() + 14.0, rect.center().y), vec2(11.0, 11.0)), Icon::Search, t.muted);
        ui.painter().text(pos2(rect.left() + 26.0, rect.center().y), egui::Align2::LEFT_CENTER, "Search", FontId::proportional(13.0), t.muted);
        ui.painter().text(pos2(rect.right() - 8.0, rect.center().y), egui::Align2::RIGHT_CENTER, crate::keys::shown("⌘K"), FontId::proportional(11.0), t.muted);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, crate::keys::shown("Search (⌘K)")));
        if resp.clicked() {
            self.actions.push(Action::OpenPalette);
        }
        ui.add_space(18.0);

        let active = self.sidebar_active();
        self.section(ui, "Library");
        for (label, route, _) in PLACES {
            let on = active.as_ref() == Some(&route);
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
        // "Playlists" with a + for a new one, in line with the section label.
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.set_height(18.0);
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Playlists").font(FontId::new(11.0, semibold())).color(t.muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::sized_button(ui, Icon::Plus, "New Playlist (⌘N)", vec2(20.0, 18.0), 9.0, Some(t.muted)).clicked() {
                    self.actions.push(Action::Command(crate::keys::Cmd::NewPlaylist));
                }
            });
        });
        ui.add_space(4.0);
        let bottom_h = 34.0;
        let dragging = egui::DragAndDrop::has_payload_of_type::<DragTracks>(ui.ctx());
        egui::ScrollArea::vertical().id_salt("playlists").max_height((ui.available_height() - bottom_h).max(40.0)).show(ui, |ui| {
            if self.data.playlists.is_empty() {
                ui.label(egui::RichText::new("  None yet").small().color(t.muted));
            }
            for p in self.data.playlists.clone() {
                let route = Route::Playlist { id: p.id.clone(), name: p.name.clone() };
                let on = active.as_ref() == Some(&route);
                let mine = self.view().owns(&p);
                let resp = self.place(ui, &p.name, on);
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
                    self.actions.push(Action::Go(route));
                }
                resp.context_menu(|ui| {
                    let kept = self.downloads.snap.pin(&offline::Pin::Playlist(p.id.clone())).is_some();
                    super::browse::collection_menu(ui, &super::browse::Collection::Playlist(p.id.clone()), true, kept, None, &mut self.actions);
                    if mine {
                        ui.separator();
                        self.view().playlist_edit_items(ui, &p.id, &p.name);
                    }
                });
            }
        });

        // The footer: the server's state on the left (click for details and
        // Sign out), keyboard shortcuts and Settings on the right.
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let status_w = (ui.available_width() - 2.0 * (FOOTER_BUTTON + 2.0) - 4.0).max(40.0);
                self.server_status(ui, status_w);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.settings_button(ui);
                    self.help_button(ui);
                });
            });
        });
    }

    /// The narrow-window sidebar: one icon per place, playlists behind a
    /// menu, settings at the bottom.
    pub(super) fn sidebar_rail(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        ui.spacing_mut().item_spacing.y = 4.0;
        let size = vec2(32.0, 32.0);
        let active = self.sidebar_active();
        ui.vertical_centered(|ui| {
            if icons::sized_button(ui, Icon::Search, "Search (⌘K)", size, 14.0, Some(t.muted)).clicked() {
                self.actions.push(Action::OpenPalette);
            }
            ui.add_space(12.0);
            for (label, route, icon) in PLACES {
                let on = active.as_ref() == Some(&route);
                let resp = icons::sized_button(ui, icon, label, size, 15.0, Some(if on { t.text } else { t.muted }));
                if on {
                    ui.painter().rect_filled(resp.rect, 6.0, t.selected);
                    icons::paint(ui.painter(), Rect::from_center_size(resp.rect.center(), vec2(15.0, 15.0)), icon, t.text);
                }
                if resp.clicked() {
                    self.actions.push(Action::Go(route));
                }
            }
            ui.add_space(12.0);
            let lists = icons::sized_button(ui, Icon::List, "Playlists", size, 14.0, Some(t.muted));
            egui::Popup::menu(&lists).show(|ui| {
                ui.set_width(220.0);
                if self.data.playlists.is_empty() {
                    ui.weak("No playlists yet");
                }
                for p in self.data.playlists.clone() {
                    if ui.button(&p.name).clicked() {
                        self.actions.push(Action::Go(Route::Playlist { id: p.id.clone(), name: p.name.clone() }));
                    }
                }
            });
        });
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            self.settings_button(ui);
            self.help_button(ui);
            self.rail_server_status(ui);
        });
    }

    /// The server's state in the footer: a dot and the host while connected,
    /// a ring and progress while syncing, a cloud while offline. Click for the
    /// details popover (latency, last sync, Sync now, Sign out).
    fn server_status(&mut self, ui: &mut Ui, width: f32) {
        let t = Tokens::of(ui);
        let state = self.server_state();
        let (rect, resp) = ui.allocate_exact_size(vec2(width, FOOTER_BUTTON), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(rect, 6.0, t.hover);
        }
        let glyph = egui::pos2(rect.left() + 12.0, rect.center().y);
        state.paint_glyph(ui, glyph);
        let g = crate::app::elide_text(ui, &state.label(), FontId::proportional(11.5), t.muted, rect.width() - 30.0);
        ui.painter().galley(egui::pos2(rect.left() + 24.0, rect.center().y - g.size().y / 2.0), g, t.muted);
        self.server_popover(&resp, &state);
        let tip = state.tooltip();
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &tip));
        resp.on_hover_text(tip);
    }

    /// The rail's version: just the glyph, the words in a tooltip; same popover.
    fn rail_server_status(&mut self, ui: &mut Ui) {
        let state = self.server_state();
        let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 28.0), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(rect, 6.0, Tokens::of(ui).hover);
        }
        state.paint_glyph(ui, rect.center());
        self.server_popover(&resp, &state);
        let tip = format!("{}\n{}", state.label(), state.tooltip());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &tip));
        resp.on_hover_text(tip);
    }

    fn server_state(&self) -> ServerState {
        let host = self.session.settings.as_ref().map(|s| super::diagnostics::server_host(&s.server)).unwrap_or_default();
        if let Some(p) = self.sync.progress {
            let fraction = (p.total > 0).then(|| p.done as f32 / p.total as f32);
            // Short enough for the footer; the counts are in the tooltip and popover.
            let label = match fraction {
                Some(f) => format!("Syncing · {:.0}%", 100.0 * f),
                None => "Syncing…".to_owned(),
            };
            let (done, total) = (crate::fmt::thousands(p.done), crate::fmt::thousands(p.total));
            let what = match p.unit {
                crate::sync::Unit::Tracks => "tracks",
                crate::sync::Unit::Albums => "albums",
            };
            let detail = format!("{}\n{done} of {total} {what}", p.label());
            return ServerState::Syncing { host, label, detail, fraction };
        }
        if self.session.online && (self.sync.albums || self.sync.tracks) {
            return ServerState::Syncing { host, label: "Syncing library…".into(), detail: "Fetching the album list".into(), fraction: None };
        }
        if self.session.online {
            return ServerState::Connected { host };
        }
        if self.session.unreachable {
            return ServerState::Offline { host };
        }
        ServerState::Connecting { host }
    }

    /// Details about the server, Sync now and Sign out, under `resp`.
    fn server_popover(&mut self, resp: &egui::Response, state: &ServerState) {
        egui::Popup::menu(resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            let t = Tokens::of(ui);
            ui.set_width(250.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            // Opening the popover refreshes the round trip if it's old.
            if self.session.online
                && self.diagnostics.latency_stale()
                && let Some(c) = self.session.client.clone()
            {
                self.diagnostics.latency_asked();
                self.bridge.measure_latency(c);
            }
            let (server, user) = self.session.settings.as_ref().map(|s| (s.server.clone(), s.username.clone())).unwrap_or_default();
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(state.host()).font(FontId::new(13.0, semibold())).color(t.text));
            });
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.add(egui::Label::new(egui::RichText::new(&server).size(11.0).color(t.muted)).truncate());
            });
            ui.add_space(10.0);
            let status = match state {
                ServerState::Connected { .. } | ServerState::Syncing { .. } => match self.diagnostics.latency() {
                    Some(rtt) => format!("Connected · {} ms", rtt.as_millis()),
                    None => "Connected".to_owned(),
                },
                ServerState::Offline { .. } => "Offline · reconnecting".to_owned(),
                ServerState::Connecting { .. } => "Connecting…".to_owned(),
            };
            let status_color = match state {
                ServerState::Offline { .. } => t.warn,
                _ => t.text,
            };
            info_row(ui, "Status", &status, status_color);
            if !user.is_empty() {
                info_row(ui, "User", &user, t.text);
            }
            if !self.session.server_info.is_empty() {
                info_row(ui, "Server", &self.session.server_info, t.text);
            }
            let synced = match state {
                ServerState::Syncing { detail, .. } => format!("Syncing {}", detail.lines().last().unwrap_or_default()),
                _ => match self.sync.last {
                    Some(at) => at.format("%-I:%M %p").to_string(),
                    None if self.session.client.is_some() => "At launch".to_owned(),
                    None => "Not yet".to_owned(),
                },
            };
            info_row(ui, "Last sync", &synced, t.text);
            let action = if self.sync.tracks {
                Some("Stop sync")
            } else if self.session.client.is_some() && self.session.online {
                Some("Sync now")
            } else {
                None
            };
            if let Some(label) = action {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    if super::widgets::link(ui, label, FontId::proportional(12.0), t.accent).clicked() {
                        if self.sync.tracks {
                            self.bridge.cancel_sync();
                        } else {
                            self.actions.push(Action::SyncNow);
                            ui.close();
                        }
                    }
                });
            }
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            if menu_item(ui, "Sign out", "") {
                self.actions.push(Action::SignOut);
            }
            ui.add_space(2.0);
        });
    }

    /// The gear: opens Settings.
    fn settings_button(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let button = icons::sized_button(ui, Icon::Gear, "Settings (⌘,)", vec2(FOOTER_BUTTON, FOOTER_BUTTON), 15.0, Some(t.muted));
        if button.clicked() {
            self.actions.push(Action::OpenSettings(self.ui.settings_section));
        }
    }

    /// A question mark in a circle: the keyboard shortcuts.
    fn help_button(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let button = icons::sized_button(ui, Icon::Help, "Keyboard shortcuts (⌘/)", vec2(FOOTER_BUTTON, FOOTER_BUTTON), 15.0, Some(t.muted));
        if button.clicked() {
            self.actions.push(Action::Command(crate::keys::Cmd::Shortcuts));
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
    fn sidebar_active(&self) -> Option<Route> {
        let current = self.ui.nav.current();
        if current.is_root() {
            return Some(current.clone());
        }
        self.ui.nav.trail().first().filter(|r| r.is_root()).map(|r| (*r).clone())
    }

    fn section(&self, ui: &mut Ui, text: &str) {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            super::widgets::section(ui, text);
        });
        ui.add_space(4.0);
    }

    pub(super) fn place(&self, ui: &mut Ui, label: &str, on: bool) -> egui::Response {
        let t = Tokens::of(ui);
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
        if ui.is_rect_visible(rect) {
            let fill = if on {
                t.selected
            } else if resp.hovered() {
                t.hover
            } else {
                egui::Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, 6.0, fill);
            let font = if on { FontId::new(13.0, semibold()) } else { FontId::proportional(13.0) };
            let g = super::widgets::elide(ui, label, font, t.text, rect.width() - 16.0);
            ui.painter().galley(pos2(rect.left() + 8.0, rect.center().y - g.size().y / 2.0), g, t.text);
        }
        let label = label.to_owned();
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::ListBoxOption, true, on, &label));
        resp
    }
}


/// A menu row with a right-aligned shortcut hint. True when chosen.
fn menu_item(ui: &mut Ui, label: &str, keys: &str) -> bool {
    let t = Tokens::of(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
    if ui.is_rect_visible(rect) {
        if resp.hovered() {
            ui.painter().rect_filled(rect, 5.0, t.hover);
        }
        ui.painter().text(pos2(rect.left() + 8.0, rect.center().y), egui::Align2::LEFT_CENTER, label, FontId::proportional(13.0), t.text);
        if !keys.is_empty() {
            ui.painter().text(pos2(rect.right() - 8.0, rect.center().y), egui::Align2::RIGHT_CENTER, keys, FontId::proportional(12.0), t.muted);
        }
    }
    let label = label.to_owned();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::MenuItem, true, &label));
    if resp.clicked() {
        ui.close();
        return true;
    }
    false
}

/// A label and value in the server popover.
fn info_row(ui: &mut Ui, label: &str, value: &str, color: egui::Color32) {
    let t = Tokens::of(ui);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let (r, _) = ui.allocate_exact_size(vec2(64.0, 20.0), Sense::hover());
        ui.painter().text(r.left_center(), egui::Align2::LEFT_CENTER, label, FontId::proportional(12.0), t.muted);
        ui.add(egui::Label::new(egui::RichText::new(value).size(12.0).color(color)).truncate());
    });
}

/// The server's state as the footer shows it.
#[derive(Debug, Clone, PartialEq)]
enum ServerState {
    Connected { host: String },
    Syncing { host: String, label: String, detail: String, fraction: Option<f32> },
    /// Not answering; reconnecting in the background.
    Offline { host: String },
    /// The first connect is still under way.
    Connecting { host: String },
}

impl ServerState {
    fn host(&self) -> &str {
        match self {
            ServerState::Connected { host } | ServerState::Syncing { host, .. } | ServerState::Offline { host } | ServerState::Connecting { host } => host,
        }
    }

    /// What the footer says.
    fn label(&self) -> String {
        match self {
            ServerState::Connected { host } => host.clone(),
            ServerState::Syncing { label, .. } => label.clone(),
            ServerState::Offline { .. } => "Offline".into(),
            ServerState::Connecting { .. } => "Connecting…".into(),
        }
    }

    fn tooltip(&self) -> String {
        match self {
            ServerState::Connected { host } => format!("Connected to {host}. Click for details."),
            ServerState::Syncing { detail, .. } => format!("{detail}\nYou can browse while it runs. New albums and tracks appear as they arrive."),
            ServerState::Offline { .. } => "Can't reach the server. Downloaded music still plays, and Ond keeps trying to reconnect.".into(),
            ServerState::Connecting { host } => format!("Connecting to {host}…"),
        }
    }

    /// The small glyph at `c`: a dot, a progress ring or spinner, or a cloud.
    fn paint_glyph(&self, ui: &Ui, c: egui::Pos2) {
        let t = Tokens::of(ui);
        let painter = ui.painter();
        match self {
            ServerState::Connected { .. } => {
                painter.circle_filled(c, 3.2, t.ok);
            }
            ServerState::Syncing { fraction: Some(f), .. } => icons::progress_ring(painter, c, 5.5, *f, t.accent, t.hairline),
            ServerState::Syncing { fraction: None, .. } | ServerState::Connecting { .. } => {
                icons::paint(painter, Rect::from_center_size(c, vec2(12.0, 12.0)), Icon::Spinner, t.muted);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
            }
            ServerState::Offline { .. } => icons::paint(painter, Rect::from_center_size(c, vec2(13.0, 13.0)), Icon::Offline, t.muted),
        }
    }
}
