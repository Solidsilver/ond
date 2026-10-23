//! Favorites: loved songs, albums and artists, one kind at a time.

use super::album_page::ListOpts;
use super::browse::grid_metrics;
use super::widgets::{choice, elide};
use super::{Action, ListenerApp};
use crate::icons::{self, Icon};
use crate::nav::Route;
use crate::queue::Source;
use crate::theme::Tokens;
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FavTab {
    #[default]
    Songs,
    Albums,
    Artists,
}

/// The queue groups loved songs under this.
pub fn favorites_source() -> Source {
    Source::Playlist { id: "favorites".into(), name: "Favorites".into() }
}

impl ListenerApp {
    pub(super) fn favorites_page(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let v = self.favorites.view.clone();
        let label = |tab: FavTab| match tab {
            FavTab::Songs => format!("Songs {}", v.songs.len()),
            FavTab::Albums => format!("Albums {}", v.albums.len()),
            FavTab::Artists => format!("Artists {}", v.artists.len()),
        };
        ui.horizontal(|ui| {
            self.heading(ui, "Favorites", None);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Right-to-left layout: the pill itself still reads left to right.
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    choice(ui, &[FavTab::Songs, FavTab::Albums, FavTab::Artists], &mut self.fav_tab, None, label);
                });
            });
        });
        let empty = |ui: &mut Ui, text: &str| super::widgets::empty_state(ui, Icon::Heart, "Nothing loved yet", text);
        if !self.favorites.loaded && self.session.client.is_some() {
            ui.label(egui::RichText::new("Loading…").color(t.muted));
            return;
        }
        match self.fav_tab {
            FavTab::Songs => {
                if v.songs.is_empty() {
                    return empty(ui, "Songs you love show up here. Click the heart on a track, or press ⇧⌘L while one plays.");
                }
                ui.horizontal(|ui| {
                    if let Some(action) = super::browse::play_shuffle(ui) {
                        self.actions.push(Action::Tracks { tracks: v.songs.clone(), source: favorites_source(), action });
                    }
                });
                ui.add_space(14.0);
                egui::ScrollArea::vertical().id_salt("favorite-songs").auto_shrink(false).show(ui, |ui| {
                    self.track_list(ui, &v.songs, &favorites_source(), ListOpts { numbers: false, artist: true, playlist: None });
                });
            }
            FavTab::Albums => {
                let ids: Vec<usize> = v.albums.iter().filter_map(|id| self.data.by_id.get(id).copied()).collect();
                if ids.is_empty() {
                    return empty(ui, "Albums you love show up here. Use the heart next to Play on an album.");
                }
                let (cols, tile) = grid_metrics(ui.available_width() - 12.0);
                egui::ScrollArea::vertical().id_salt("favorite-albums").auto_shrink(false).show(ui, |ui| {
                    for chunk in ids.chunks(cols) {
                        self.grid_row(ui, chunk, tile);
                    }
                });
            }
            FavTab::Artists => {
                if v.artists.is_empty() {
                    return empty(ui, "Artists you love show up here. Use the heart on an artist's page.");
                }
                egui::ScrollArea::vertical().id_salt("favorite-artists").auto_shrink(false).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (id, name) in &v.artists {
                        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width().min(720.0), 32.0), Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(rect, 6.0, t.hover);
                        }
                        let g = elide(ui, name, FontId::proportional(13.0), t.text, rect.width() - 50.0);
                        ui.painter().galley(pos2(rect.left() + 10.0, rect.center().y - g.size().y / 2.0), g, t.text);
                        icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), vec2(11.0, 11.0)), Icon::HeartFilled, t.accent);
                        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
                        if resp.clicked() {
                            self.actions.push(Action::Go(Route::Artist { name: name.clone(), id: Some(id.clone()) }));
                        }
                    }
                });
            }
        }
    }
}
