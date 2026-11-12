//! Favorites: loved songs, albums and artists, one kind at a time.

use super::Action;
use super::track_list::ListOpts;
use super::view_ctx::ViewCtx;
use super::virtual_list::VirtualList;
use crate::icons::{self, Icon};
use crate::nav::Route;
use crate::queue::Source;
use crate::theme::Tokens;
use crate::widgets::elide;
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};

/// A loved artist's row.
const ARTIST_ROW_H: f32 = 32.0;

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

impl ViewCtx<'_> {
    pub(super) fn favorites_page(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let v = self.favorites.view.clone();
        let label = |tab: FavTab| match tab {
            FavTab::Songs => format!("Songs {}", v.songs.len()),
            FavTab::Albums => format!("Albums {}", v.albums.len()),
            FavTab::Artists => format!("Artists {}", v.artists.len()),
        };
        let tabs = [FavTab::Songs, FavTab::Albums, FavTab::Artists];
        super::browse::heading_bar(ui, "Favorites", None, None, Some((&tabs[..], &mut self.ui.fav_tab)), label);
        let empty = |ui: &mut Ui, text: &str| crate::widgets::empty_state(ui, Icon::Heart, "Nothing loved yet", text);
        if !self.favorites.loaded && self.session.client.is_some() {
            crate::widgets::empty_state(ui, Icon::Heart, "Loading…", "");
            return;
        }
        match self.ui.fav_tab {
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
                let scroll = egui::ScrollArea::vertical().id_salt("favorite-songs").auto_shrink(false).content_margin(super::layout::gutter());
                super::layout::into_gutter(ui, |ui| {
                    self.track_list_rows(ui, scroll, &v.songs, &favorites_source(), ListOpts { numbers: false, artist: true, playlist: None })
                });
            }
            FavTab::Albums => {
                let ids: Vec<usize> = v.albums.iter().filter_map(|id| self.data.by_id.get(id).copied()).collect();
                if ids.is_empty() {
                    return empty(ui, "Albums you love show up here. Use the heart next to Play on an album.");
                }
                self.album_grid(ui, "favorite-albums", None, &ids);
            }
            FavTab::Artists => {
                if v.artists.is_empty() {
                    return empty(ui, "Artists you love show up here. Use the heart on an artist's page.");
                }
                VirtualList::table("favorite-artists", None, ARTIST_ROW_H, v.artists.len()).show(ui, |ui, range| {
                    for (id, name) in &v.artists[range] {
                        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width().min(shell::kit::LIST_W), ARTIST_ROW_H), Sense::click());
                        if ui.is_rect_visible(rect) {
                            if resp.hovered() {
                                ui.painter().rect_filled(rect, 6.0, t.hover);
                            }
                            let g = elide(ui, name, FontId::proportional(13.0), t.text, rect.width() - 50.0);
                            ui.painter().galley(pos2(rect.left() + 10.0, rect.center().y - g.size().y / 2.0), g, t.text);
                            icons::paint(
                                ui.painter(),
                                Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), vec2(11.0, 11.0)),
                                Icon::HeartFilled,
                                t.accent,
                            );
                        }
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
