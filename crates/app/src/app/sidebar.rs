//! Left column: search (opens the palette), library places, playlists, and
//! settings. In narrow windows it collapses to a rail of icons.

use super::widgets::toggle;
use super::{Action, ListenerApp};
use crate::icons::{self, Icon};
use crate::nav::Route;
use crate::theme::{ThemePref, Tokens, semibold};
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};

/// Width of the full sidebar and of the icon rail.
pub const SIDEBAR_W: f32 = 216.0;
pub const RAIL_W: f32 = 52.0;

const PLACES: [(&str, Route, Icon); 5] = [
    ("Recently Added", Route::Recent, Icon::Clock),
    ("Artists", Route::Artists, Icon::Person),
    ("Albums", Route::Albums, Icon::Grid),
    ("Songs", Route::Songs, Icon::Note),
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
            if self.place(ui, label, on).clicked() {
                self.actions.push(Action::Go(route));
            }
        }
        ui.add_space(18.0);
        self.section(ui, "Playlists");
        let bottom_h = 34.0;
        egui::ScrollArea::vertical().id_salt("playlists").max_height((ui.available_height() - bottom_h).max(40.0)).show(ui, |ui| {
            if self.data.playlists.is_empty() {
                ui.label(egui::RichText::new("  None yet").small().color(t.muted));
            }
            for p in self.data.playlists.clone() {
                let route = Route::Playlist { id: p.id.clone(), name: p.name.clone() };
                let on = active.as_ref() == Some(&route);
                let resp = self.place(ui, &p.name, on);
                if resp.clicked() {
                    self.actions.push(Action::Go(route));
                }
                resp.context_menu(|ui| self.playlist_menu(ui, &p.id));
            }
        });

        // Settings, pinned to the bottom.
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| self.settings_button(ui));
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
                ui.set_min_width(200.0);
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
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| self.settings_button(ui));
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

    /// The gear and its popover: appearance, scrobbling, the account.
    fn settings_button(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let button = icons::sized_button(ui, Icon::Gear, "Settings", vec2(28.0, 28.0), 15.0, Some(t.muted));
        egui::Popup::menu(&button)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .width(SETTINGS_W)
            .show(|ui| self.settings_ui(ui));
    }

    pub(super) fn settings_ui(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        ui.set_width(SETTINGS_W);
        ui.spacing_mut().item_spacing.y = 0.0;
        let margin = |ui: &mut Ui, f: &mut dyn FnMut(&mut Ui)| {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.set_width(SETTINGS_W - 16.0);
                    f(ui);
                });
            });
        };
        ui.add_space(6.0);
        margin(ui, &mut |ui| {
            ui.label(egui::RichText::new("Appearance").font(FontId::new(11.0, semibold())).color(t.muted));
            ui.add_space(6.0);
            let prefs = [ThemePref::System, ThemePref::Light, ThemePref::Dark];
            let at = prefs.iter().position(|p| *p == self.theme).unwrap_or(0);
            if let Some(i) = super::widgets::segmented_wide(ui, &["System", "Light", "Dark"], at, SETTINGS_W - 16.0) {
                self.actions.push(Action::SetTheme(prefs[i]));
            }
            ui.add_space(12.0);
            let mut scrobble = self.scrobble;
            if toggle(ui, &mut scrobble, "Scrobble plays").on_hover_text("Report plays to the server, which passes them on to Last.fm / ListenBrainz").changed() {
                self.actions.push(Action::SetScrobble(scrobble));
            }
        });
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        margin(ui, &mut |ui| {
            ui.horizontal(|ui| {
                let account = if self.server_label.is_empty() { "Not connected".to_owned() } else { self.server_label.clone() };
                let w = ui.available_width() - 64.0;
                let g = super::widgets::elide(ui, &account, FontId::proportional(12.0), t.muted, w);
                let (r, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
                ui.painter().galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, t.muted);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !self.server_label.is_empty() && super::widgets::link(ui, "Sign out", FontId::proportional(12.0), t.accent).clicked() {
                        self.actions.push(Action::SignOut);
                        ui.close();
                    }
                });
            });
        });
        ui.add_space(4.0);
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

    fn place(&self, ui: &mut Ui, label: &str, on: bool) -> egui::Response {
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

const SETTINGS_W: f32 = 260.0;
