//! Left column: search (opens the palette), library places, playlists, and
//! settings. In narrow windows it collapses to a rail of icons.

use super::{Action, DragTracks, ListenerApp, PlaylistAction};
use crate::icons::{self, Icon};
use crate::nav::Route;
use crate::theme::{Tokens, semibold};
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};

/// Width of the full sidebar and of the icon rail.
pub const SIDEBAR_W: f32 = 216.0;
pub const RAIL_W: f32 = 52.0;

/// In ⌘1…⌘6 order.
pub(super) const PLACES: [(&str, Route, Icon); 6] = [
    ("Recently Added", Route::Recent, Icon::Clock),
    ("Artists", Route::Artists, Icon::Person),
    ("Albums", Route::Albums, Icon::Grid),
    ("Songs", Route::Songs, Icon::Note),
    ("Favorites", Route::Favorites, Icon::Heart),
    ("Offline", Route::Offline, Icon::Download),
];

impl ListenerApp {
    pub(super) fn sidebar(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        ui.spacing_mut().item_spacing.y = 1.0;

        // A search field that opens the palette (the palette is the real search).
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
        ui.painter().rect_filled(rect, 6.0, if resp.hovered() { t.selected } else { t.field });
        icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.left() + 14.0, rect.center().y), vec2(11.0, 11.0)), Icon::Search, t.muted);
        ui.painter().text(pos2(rect.left() + 26.0, rect.center().y), egui::Align2::LEFT_CENTER, "Search", FontId::proportional(13.0), t.muted);
        ui.painter().text(pos2(rect.right() - 8.0, rect.center().y), egui::Align2::RIGHT_CENTER, "⌘K", FontId::proportional(11.0), t.muted);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Search (⌘K)"));
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
                let mine = self.owns(&p);
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
                    self.playlist_menu(ui, &p.id);
                    if mine {
                        ui.separator();
                        self.playlist_edit_items(ui, &p.id, &p.name);
                    }
                });
            }
        });

        // Settings, pinned to the bottom, with sync or offline state beside it
        // while there is any.
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.horizontal(|ui| {
                self.settings_button(ui);
                ui.add_space(2.0);
                self.sidebar_status(ui);
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
            self.rail_status(ui);
        });
    }

    /// What the app is doing in the background, if anything: syncing (a ring
    /// with progress, Stop on hover) or offline (a quiet pill). Nothing when idle.
    fn sidebar_status(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let w = ui.available_width();
        if let Some(state) = self.background_state() {
            let (rect, resp) = ui.allocate_exact_size(vec2(w, 24.0), Sense::click());
            let painter = ui.painter();
            let ring = egui::pos2(rect.left() + 8.0, rect.center().y);
            let font = egui::FontId::proportional(11.5);
            match &state {
                Background::Syncing { label, fraction, .. } => {
                    match fraction {
                        Some(f) => icons::progress_ring(painter, ring, 5.5, *f, t.accent, t.hairline),
                        None => {
                            icons::paint(painter, Rect::from_center_size(ring, vec2(12.0, 12.0)), Icon::Spinner, t.muted);
                            ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
                        }
                    }
                    let stop_w = if resp.hovered() && self.track_syncing { 34.0 } else { 0.0 };
                    let g = crate::app::elide_text(ui, label, font, t.muted, rect.width() - 22.0 - stop_w);
                    painter.galley(egui::pos2(rect.left() + 20.0, rect.center().y - g.size().y / 2.0), g, t.muted);
                    if stop_w > 0.0 {
                        let stop = Rect::from_min_max(egui::pos2(rect.right() - stop_w, rect.top()), rect.max);
                        let hot = ui.rect_contains_pointer(stop);
                        painter.text(stop.right_center(), egui::Align2::RIGHT_CENTER, "Stop", egui::FontId::proportional(11.5), if hot { t.text } else { t.accent });
                        if resp.clicked() && hot {
                            self.bridge.cancel_sync();
                        }
                    }
                }
                Background::Offline => {
                    let label = "Offline";
                    let g = painter.layout_no_wrap(label.into(), font, t.muted);
                    let pill = Rect::from_min_size(egui::pos2(rect.left(), rect.center().y - 10.0), vec2(g.size().x + 30.0, 20.0));
                    painter.rect_filled(pill, 10.0, t.field);
                    icons::paint(painter, Rect::from_center_size(egui::pos2(pill.left() + 12.0, pill.center().y), vec2(11.0, 11.0)), Icon::Offline, t.muted);
                    painter.galley(egui::pos2(pill.left() + 22.0, pill.center().y - g.size().y / 2.0), g, t.muted);
                }
            }
            let tip = state.tooltip();
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Status, true, &tip));
            resp.on_hover_text(tip);
        }
    }

    /// The rail's version of [`Self::sidebar_status`]: just the ring or the
    /// offline glyph, with the words in a tooltip.
    fn rail_status(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let Some(state) = self.background_state() else { return };
        let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 24.0), Sense::hover());
        let c = rect.center();
        match &state {
            Background::Syncing { fraction: Some(f), .. } => icons::progress_ring(ui.painter(), c, 5.5, *f, t.accent, t.hairline),
            Background::Syncing { fraction: None, .. } => {
                icons::paint(ui.painter(), Rect::from_center_size(c, vec2(12.0, 12.0)), Icon::Spinner, t.muted);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
            }
            Background::Offline => icons::paint(ui.painter(), Rect::from_center_size(c, vec2(13.0, 13.0)), Icon::Offline, t.muted),
        }
        let tip = state.tooltip();
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Status, true, &tip));
        resp.on_hover_text(tip);
    }

    fn background_state(&self) -> Option<Background> {
        if let Some(p) = self.sync_progress {
            let fraction = (p.total > 0).then(|| p.done as f32 / p.total as f32);
            let (done, total) = (thousands(p.done), thousands(p.total));
            let label = match p.unit {
                crate::sync::Unit::Tracks => format!("Syncing {done} of {total}"),
                crate::sync::Unit::Albums => format!("Syncing album {done} of {total}"),
            };
            return Some(Background::Syncing { label, detail: p.label(), fraction });
        }
        if self.album_syncing || (self.track_syncing && self.online) {
            return Some(Background::Syncing { label: "Syncing library…".into(), detail: "Fetching the album list".into(), fraction: None });
        }
        if !self.online && self.settings.is_some() {
            return Some(Background::Offline);
        }
        None
    }

    fn playlist_menu(&mut self, ui: &mut Ui, id: &str) {
        if ui.button("Play").clicked() {
            self.actions.push(Action::Playlist(id.to_owned(), super::QueueAction::Play { start: 0 }));
        }
        if ui.button("Shuffle").clicked() {
            self.actions.push(Action::Playlist(id.to_owned(), super::QueueAction::Shuffle));
        }
        if ui.button("Play next").clicked() {
            self.actions.push(Action::Playlist(id.to_owned(), super::QueueAction::PlayNext));
        }
        if ui.button("Add to queue").clicked() {
            self.actions.push(Action::Playlist(id.to_owned(), super::QueueAction::Add));
        }
    }

    /// The gear: a small menu for Settings, the shortcut list and signing out.
    fn settings_button(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let button = icons::sized_button(ui, Icon::Gear, "Settings", vec2(28.0, 28.0), 15.0, Some(t.muted));
        egui::Popup::menu(&button).show(|ui| {
            ui.set_width(220.0);
            if menu_item(ui, "Settings…", "⌘,") {
                self.actions.push(Action::OpenSettings(self.settings_section));
            }
            if menu_item(ui, "Keyboard Shortcuts", "⌘/") {
                self.actions.push(Action::Command(crate::keys::Cmd::Shortcuts));
            }
            if !self.server_label.is_empty() {
                ui.separator();
                if menu_item(ui, "Sign Out", "") {
                    self.actions.push(Action::SignOut);
                }
            }
        });
    }

    /// Overall progress of unfinished downloads, while any are moving.
    fn download_progress(&self) -> Option<f32> {
        let s = &self.offline_snap;
        if s.paused || s.active == 0 {
            return None;
        }
        let (done, total) = s.pins.iter().filter(|p| !p.complete()).fold((0u64, 0u64), |(d, t), p| (d + p.bytes_done, t + p.bytes_total));
        Some(if total > 0 { done as f32 / total as f32 } else { 0.0 })
    }

    /// The sidebar entry the current place belongs to.
    fn sidebar_active(&self) -> Option<Route> {
        let current = self.nav.current();
        if current.is_root() {
            return Some(current.clone());
        }
        self.nav.trail().first().filter(|r| r.is_root()).map(|r| (*r).clone())
    }

    fn section(&self, ui: &mut Ui, text: &str) {
        let t = Tokens::of(ui);
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(text).font(FontId::new(11.0, semibold())).color(t.muted));
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

/// Background work worth a glance in the sidebar footer.
enum Background {
    Syncing { label: String, detail: String, fraction: Option<f32> },
    Offline,
}

impl Background {
    fn tooltip(&self) -> String {
        match self {
            Background::Syncing { detail, .. } => format!("{detail}\nBrowsing works meanwhile; new albums and tracks appear as they arrive."),
            Background::Offline => "Can't reach the server. Downloaded music still plays; reconnecting in the background.".into(),
        }
    }
}

/// 1866 → "1,866".
fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
