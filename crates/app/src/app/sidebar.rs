//! Left column: search (opens the palette), library places, playlists, and
//! the appearance setting.

use super::{Action, ListenerApp};
use crate::icons::{self, Icon};
use crate::nav::Route;
use crate::theme::{ThemePref, Tokens, semibold};
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};

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
        for (label, route) in [("Recently Added", Route::Recent), ("Artists", Route::Artists), ("Albums", Route::Albums), ("Songs", Route::Songs), ("Offline", Route::Offline)] {
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
                resp.context_menu(|ui| {
                    if ui.button("Play").clicked() {
                        self.actions.push(Action::Playlist(p.id.clone(), super::QueueAction::Play { start: 0 }));
                    }
                    if ui.button("Shuffle").clicked() {
                        self.actions.push(Action::Playlist(p.id.clone(), super::QueueAction::Shuffle));
                    }
                    if ui.button("Play next").clicked() {
                        self.actions.push(Action::Playlist(p.id.clone(), super::QueueAction::PlayNext));
                    }
                    if ui.button("Add to queue").clicked() {
                        self.actions.push(Action::Playlist(p.id.clone(), super::QueueAction::Add));
                    }
                });
            }
        });

        // Appearance, pinned to the bottom.
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            let button = icons::sized_button(ui, Icon::Sliders, "Appearance", vec2(28.0, 26.0), 13.0, Some(t.muted));
            egui::Popup::menu(&button).show(|ui| {
                ui.label(egui::RichText::new("Appearance").font(FontId::new(12.0, semibold())));
                for (pref, label) in [(ThemePref::System, "Match system"), (ThemePref::Light, "Light"), (ThemePref::Dark, "Dark")] {
                    if ui.radio(self.theme == pref, label).clicked() {
                        self.actions.push(Action::SetTheme(pref));
                    }
                }
                ui.separator();
                let mut scrobble = self.scrobble;
                if ui.checkbox(&mut scrobble, "Scrobble plays").on_hover_text("Report plays to the server, which passes them on to Last.fm / ListenBrainz").changed() {
                    self.actions.push(Action::SetScrobble(scrobble));
                }
            });
        });
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
