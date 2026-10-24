//! The main view's router and the album-grid places: Recently Added, Albums,
//! Artists, an artist's page, and albums by genre / year / label.

use super::view_ctx::ViewCtx;
use super::widgets::{Mark, choice, elide};
use super::{Action, AlbumSort, Data, Keep, OndApp, QueueAction};
use crate::icons::{self, Icon};
use crate::nav::{FilterKind, Route};
use crate::theme::{Tokens, semibold};
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};
use std::sync::Arc;

const TILE_MIN: f32 = 160.0;
const GAP_X: f32 = 20.0;
const GAP_Y: f32 = 24.0;
/// Title + artist lines under the art.
const LABEL_H: f32 = 40.0;

/// Short name for a place, for breadcrumbs.
pub fn route_label(route: &Route, data: &Data) -> String {
    match route {
        Route::Recent => "Recently added".into(),
        Route::Albums => "Albums".into(),
        Route::Artists => "Artists".into(),
        Route::Songs => "Songs".into(),
        Route::Favorites => "Favorites".into(),
        Route::Offline => "Offline".into(),
        Route::Artist { name, .. } => name.clone(),
        Route::Album(id) => data.album(id).map_or_else(|| "Album".into(), |a| a.name.clone()),
        Route::Playlist { name, .. } => name.clone(),
        Route::Filter { value, .. } => value.clone(),
    }
}

/// Indices into `data.albums` for a grid place, in display order.
pub fn grid_ids(route: &Route, sort: AlbumSort, data: &Data) -> Vec<usize> {
    let albums = &data.albums;
    let mut ids: Vec<usize> = match route {
        Route::Recent => {
            let from_server: Vec<usize> = data.newest.iter().filter_map(|id| data.by_id.get(id).copied()).collect();
            if !from_server.is_empty() {
                return from_server;
            }
            // Before the server answers: newest by the stored creation date.
            let mut ids: Vec<usize> = (0..albums.len()).collect();
            ids.sort_by(|&a, &b| albums[b].created.cmp(&albums[a].created));
            ids.truncate(120);
            return ids;
        }
        Route::Albums => (0..albums.len()).collect(),
        Route::Artist { name, id } => (0..albums.len()).filter(|&i| albums[i].has_artist(id.as_deref(), name)).collect(),
        Route::Filter { kind, value } => (0..albums.len())
            .filter(|&i| {
                let a = &albums[i];
                match kind {
                    FilterKind::Genre => a.all_genres().iter().any(|g| g.eq_ignore_ascii_case(value)),
                    FilterKind::Year => a.year.map(|y| y.to_string()).as_deref() == Some(value.as_str()),
                    FilterKind::Label => a.labels.iter().any(|l| l == value),
                }
            })
            .collect(),
        _ => Vec::new(),
    };
    let sort = match route {
        Route::Albums => sort,
        Route::Artist { .. } => AlbumSort::Year,
        _ => AlbumSort::Artist,
    };
    match sort {
        // The db already returns artist → year → title order.
        AlbumSort::Artist => {}
        AlbumSort::Title => ids.sort_by_cached_key(|&i| albums[i].name.to_lowercase()),
        AlbumSort::Year => ids.sort_by_key(|&i| (albums[i].year.unwrap_or(i32::MAX), albums[i].name.to_lowercase())),
        AlbumSort::Added => ids.sort_by(|&a, &b| albums[b].created.cmp(&albums[a].created)),
    }
    ids
}

impl OndApp {
    pub(super) fn main_view(&mut self, ui: &mut Ui) {
        self.trail_bar(ui);
        let route = self.ui.nav.current().clone();
        match &route {
            Route::Recent => {
                self.view().heading(ui, "Recently added", None);
                self.view().grid_place(ui, &route);
            }
            Route::Albums => {
                let n = self.data.albums.len();
                ui.horizontal(|ui| {
                    self.view().heading(ui, "Albums", Some(format!("{n}")));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Right-to-left layout: the pill itself still reads left to right.
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            choice(ui, &AlbumSort::ALL, &mut self.ui.sort, None, |s| s.label().into());
                        });
                    });
                });
                self.view().grid_place(ui, &route);
            }
            Route::Artists => self.view().artists_page(ui),
            Route::Songs => self.view().songs_page(ui),
            Route::Artist { name, .. } => self.artist_page(ui, &route, name),
            Route::Album(id) => self.view().album_page(ui, id),
            Route::Playlist { id, name } => self.view().playlist_page(ui, id, name),
            Route::Favorites => self.view().favorites_page(ui),
            Route::Filter { kind, value } => {
                let what = match kind {
                    FilterKind::Genre => "Genre",
                    FilterKind::Year => "Year",
                    FilterKind::Label => "Label",
                };
                self.view().heading(ui, value, Some(what.to_owned()));
                self.view().grid_place(ui, &route);
            }
            Route::Offline => self.view().offline_page(ui),
        }
    }

    /// Back / forward and the breadcrumb trail of recent places.
    fn trail_bar(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let back = ui.add_enabled_ui(self.ui.nav.can_back(), |ui| icons::sized_button(ui, Icon::ChevronLeft, "Back (⌘[)", vec2(26.0, 26.0), 10.0, Some(t.text))).inner;
            if back.clicked() {
                self.actions.push(Action::Back);
            }
            let fwd = ui.add_enabled_ui(self.ui.nav.can_forward(), |ui| icons::sized_button(ui, Icon::ChevronRight, "Forward (⌘])", vec2(26.0, 26.0), 10.0, Some(t.text))).inner;
            if fwd.clicked() {
                self.actions.push(Action::Forward);
            }
            ui.add_space(8.0);
            ui.spacing_mut().item_spacing.x = 6.0;
            let trail = self.ui.nav.trail();
            let n = trail.len();
            let labels: Vec<String> = trail.iter().map(|r| route_label(r, &self.data)).collect();
            for (i, label) in labels.iter().enumerate() {
                if i + 1 == n {
                    ui.label(egui::RichText::new(label).size(12.0).color(t.text));
                } else {
                    if super::widgets::link(ui, label, FontId::proportional(12.0), t.muted).clicked() {
                        self.actions.push(Action::BackBy(n - 1 - i));
                    }
                    let (r, _) = ui.allocate_exact_size(vec2(8.0, 12.0), Sense::hover());
                    icons::paint(ui.painter(), r.shrink(1.5), Icon::ChevronRight, t.muted);
                }
            }
        });
        ui.add_space(6.0);
    }

    /// An artist's albums, then the albums that credit them elsewhere (from
    /// tracks synced so far).
    fn artist_page(&mut self, ui: &mut Ui, route: &Route, name: &str) {
        let t = Tokens::of(ui);
        let own = self.view().cached_grid(route);
        let own_ids: std::collections::HashSet<&str> = own.iter().map(|&i| self.data.albums[i].id.as_str()).collect();
        let credited: Vec<usize> = self
            .data
            .credited
            .get(name)
            .map(|ids| ids.iter().filter(|id| !own_ids.contains(id.as_str())).filter_map(|id| self.data.by_id.get(id).copied()).collect())
            .unwrap_or_default();
        let sub = match own.len() {
            0 => None,
            1 => Some("1 album".to_owned()),
            n => Some(format!("{n} albums")),
        };
        self.view().heading(ui, name, sub);
        if !own.is_empty() {
            ui.horizontal(|ui| {
                if super::widgets::primary(ui, Some(Icon::Shuffle), "Shuffle").clicked() {
                    self.actions.push(Action::Artist(name.to_owned(), QueueAction::Play { start: 0 }));
                }
                ui.add_space(4.0);
                if super::widgets::secondary(ui, "Play next").clicked() {
                    self.actions.push(Action::Artist(name.to_owned(), QueueAction::PlayNext));
                }
                // Only artists the server knows by id can be loved.
                if let Route::Artist { id: Some(id), .. } = route {
                    let loved = self.favorites.is_on(crate::favorites::StarKind::Artist, id);
                    if super::widgets::heart_button(ui, loved, 30.0, 14.0).clicked() {
                        self.actions.push(Action::ToggleStar { kind: crate::favorites::StarKind::Artist, id: id.clone(), track: None, name: Some(name.to_owned()) });
                    }
                }
            });
            ui.add_space(14.0);
        }
        let (cols, tile) = grid_metrics(ui.available_width());
        super::layout::into_gutter(ui, |ui| egui::ScrollArea::vertical().id_salt(("artist", name)).auto_shrink(false).content_margin(super::layout::gutter()).show(ui, |ui| {
            for chunk in own.chunks(cols) {
                self.view().grid_row(ui, chunk, tile);
            }
            if !credited.is_empty() {
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Also credited on").font(FontId::new(15.0, semibold())).color(t.text));
                ui.add_space(8.0);
                for chunk in credited.chunks(cols) {
                    self.view().grid_row(ui, chunk, tile);
                }
            }
            if own.is_empty() && credited.is_empty() {
                let text = if self.sync.tracks { "Nothing yet. Credits fill in while tracks sync." } else { "No albums credit this name." };
                ui.label(egui::RichText::new(text).color(t.muted));
            }
        }));
    }
}

/// Columns and tile width that fill `width`.
pub fn grid_metrics(width: f32) -> (usize, f32) {
    let cols = (((width + GAP_X) / (TILE_MIN + GAP_X)).floor() as usize).max(1);
    let tile = ((width - GAP_X * (cols - 1) as f32) / cols as f32).max(80.0);
    (cols, tile)
}

/// Something that plays as a whole: an album, a playlist, or all of an artist.
#[derive(Debug, Clone)]
pub enum Collection {
    Album(String),
    Playlist(String),
    Artist(String),
}

impl Collection {
    fn action(&self, a: QueueAction) -> Action {
        match self {
            Collection::Album(id) => Action::Album(id.clone(), a),
            Collection::Playlist(id) => Action::Playlist(id.clone(), a),
            Collection::Artist(name) => Action::Artist(name.clone(), a),
        }
    }

    /// What "Make available offline" keeps; an artist isn't downloaded whole.
    fn keep(&self) -> Option<Keep> {
        match self {
            Collection::Album(id) => Some(Keep::Album(id.clone())),
            Collection::Playlist(id) => Some(Keep::Playlist(id.clone())),
            Collection::Artist(_) => None,
        }
    }
}

/// The same queue and download items wherever a collection has a menu. With
/// `play` the menu leads with Play and Shuffle (context menus); header menus
/// skip them, having those buttons beside. `shortcut` labels the download item.
pub fn collection_menu(ui: &mut Ui, c: &Collection, play: bool, kept: bool, shortcut: Option<&str>, actions: &mut Vec<Action>) {
    let mut items = Vec::new();
    if play {
        match c {
            // Playing an artist shuffles across their albums.
            Collection::Artist(_) => items.push(("Shuffle all", QueueAction::Play { start: 0 })),
            _ => items.extend([("Play", QueueAction::Play { start: 0 }), ("Shuffle", QueueAction::Shuffle)]),
        }
    }
    items.extend([("Play next", QueueAction::PlayNext), ("Add to queue", QueueAction::Add)]);
    for (label, action) in items {
        if ui.button(label).clicked() {
            actions.push(c.action(action));
        }
    }
    if let Some(keep) = c.keep() {
        ui.separator();
        offline_item(ui, kept, shortcut, || keep, actions);
    }
}

/// The Play and Shuffle buttons at the head of a list; which was clicked.
pub fn play_shuffle(ui: &mut Ui) -> Option<QueueAction> {
    let mut picked = None;
    if super::widgets::primary(ui, Some(Icon::Play), "Play").clicked() {
        picked = Some(QueueAction::Play { start: 0 });
    }
    ui.add_space(4.0);
    if super::widgets::secondary(ui, "Shuffle").clicked() {
        picked = Some(QueueAction::Shuffle);
    }
    picked
}

/// "Make available offline" / "Remove download", with the key that does it
/// where there is one.
pub fn offline_item(ui: &mut Ui, kept: bool, shortcut: Option<&str>, what: impl FnOnce() -> Keep, actions: &mut Vec<Action>) {
    let label = if kept { "Remove download" } else { "Make available offline" };
    let mut button = egui::Button::new(label);
    if let Some(keys) = shortcut {
        button = button.shortcut_text(crate::keys::shown(keys));
    }
    if ui.add(button).clicked() {
        actions.push(Action::KeepOffline(what(), !kept));
    }
}


impl ViewCtx<'_> {
    pub(super) fn heading(&self, ui: &mut Ui, title: &str, sub: Option<String>) {
        let t = Tokens::of(ui);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(title).font(FontId::new(24.0, semibold())).color(t.text));
            if let Some(sub) = sub {
                ui.label(egui::RichText::new(sub).size(13.0).color(t.muted));
            }
        });
        ui.add_space(10.0);
    }

    pub(super) fn cached_grid(&mut self, route: &Route) -> Arc<Vec<usize>> {
        let (sort, data) = (self.ui.sort, &self.data);
        self.ui.grid.get((route.clone(), sort, data.version), || grid_ids(route, sort, data))
    }

    /// A full-height, virtualized grid: only visible rows are laid out, so only
    /// visible covers are requested.
    pub(super) fn grid_place(&mut self, ui: &mut Ui, route: &Route) {
        let ids = self.cached_grid(route);
        if ids.is_empty() {
            let t = Tokens::of(ui);
            ui.label(egui::RichText::new(if self.data.albums.is_empty() { "Loading your library…" } else { "No albums here." }).color(t.muted));
            return;
        }
        let (cols, tile) = grid_metrics(ui.available_width());
        let rows = ids.len().div_ceil(cols);
        super::layout::into_gutter(ui, |ui| egui::ScrollArea::vertical().id_salt(("grid", format!("{route:?}"))).auto_shrink(false).content_margin(super::layout::gutter()).show_rows(ui, tile + LABEL_H + GAP_Y, rows, |ui, range| {
            for row in range {
                let slice = &ids[row * cols..((row + 1) * cols).min(ids.len())];
                self.grid_row(ui, slice, tile);
            }
        }));
    }

    /// One grid row of tiles, followed by the row gap.
    pub(super) fn grid_row(&mut self, ui: &mut Ui, ids: &[usize], tile: f32) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = GAP_X;
            for &i in ids {
                self.album_tile(ui, i, tile);
            }
        });
        ui.add_space(GAP_Y - ui.spacing().item_spacing.y);
    }

    /// Cover, title and artist. Click opens; double-click or the hover play
    /// button plays; right-click has the queue actions.
    pub(super) fn album_tile(&mut self, ui: &mut Ui, idx: usize, tile: f32) {
        let t = Tokens::of(ui);
        let (rect, resp) = ui.allocate_exact_size(vec2(tile, tile + LABEL_H), Sense::click());
        let album = &self.data.albums[idx];
        let id = album.id.clone();
        if ui.is_rect_visible(rect) {
            let art = Rect::from_min_size(rect.min, vec2(tile, tile));
            let cover = album.cover_art.clone();
            let (name, artist) = (album.name.clone(), album.artist.clone().unwrap_or_default());
            self.art.paint(ui, art, cover.as_deref(), 6.0);
            // Only albums kept offline carry a mark; nothing otherwise.
            let mark = Mark::of_pin(self.downloads.snap.pin(&offline::Pin::Album(id.clone())));
            let mark_w = if mark.is_some() { 16.0 } else { 0.0 };
            let title = elide(ui, &name, FontId::proportional(13.0), t.text, tile);
            ui.painter().galley(pos2(rect.left(), art.bottom() + 7.0), title, t.text);
            let sub = elide(ui, &artist, FontId::proportional(12.0), t.muted, tile - mark_w);
            ui.painter().galley(pos2(rect.left(), art.bottom() + 24.0), sub, t.muted);
            if let Some(m) = mark {
                super::widgets::mark(ui, pos2(rect.right() - 6.0, art.bottom() + 31.0), 11.0, m);
            }
            if resp.hovered() {
                let play = Rect::from_center_size(pos2(art.left() + 24.0, art.bottom() - 24.0), vec2(32.0, 32.0));
                let play_resp = ui.interact(play, ui.scope_id().with(("tile-play", &id)), Sense::click());
                let fill = if play_resp.hovered() { t.accent } else { t.bg.gamma_multiply(0.92) };
                let fg = if play_resp.hovered() { t.on_accent } else { t.text };
                ui.painter().circle_filled(play.center(), 16.0, fill);
                icons::paint(ui.painter(), Rect::from_center_size(play.center() + vec2(1.0, 0.0), vec2(12.0, 12.0)), Icon::Play, fg);
                play_resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Play album"));
                if play_resp.clicked() {
                    self.actions.push(Action::Album(id.clone(), QueueAction::Play { start: 0 }));
                    return;
                }
            }
        }
        let album = &self.data.albums[idx];
        let label = format!("{} by {}", album.name, album.artist.as_deref().unwrap_or("Unknown artist"));
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
        let artist = album.artist.clone();
        let artist_ref = album.artist_ref.clone();
        if resp.double_clicked() {
            self.actions.push(Action::Album(id.clone(), QueueAction::Play { start: 0 }));
        } else if resp.clicked() {
            self.actions.push(Action::Go(Route::Album(id.clone())));
        }
        let kept = self.downloads.snap.pin(&offline::Pin::Album(id.clone())).is_some();
        resp.context_menu(|ui| {
            collection_menu(ui, &Collection::Album(id.clone()), true, kept, None, self.actions);
            if let Some(name) = artist {
                ui.separator();
                if ui.button(format!("Go to {name}")).clicked() {
                    self.actions.push(Action::Go(Route::Artist { name, id: artist_ref }));
                }
            }
        });
    }

    pub(super) fn artists_page(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let Some(index) = &self.search.index else {
            self.heading(ui, "Artists", None);
            ui.label(egui::RichText::new("Loading your library…").color(t.muted));
            return;
        };
        // The server's artists (with ids) once synced, else album-artist names.
        let artists = Arc::clone(&index.artists);
        self.heading(ui, "Artists", Some(artists.len().to_string()));
        super::layout::into_gutter(ui, |ui| egui::ScrollArea::vertical().id_salt("artists").auto_shrink(false).content_margin(super::layout::gutter()).show_rows(ui, 32.0, artists.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for a in &artists[range] {
                let (name, id, n) = (&a.name, &a.id, &a.albums);
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width().min(720.0), 32.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(rect, 6.0, t.hover);
                }
                let g = elide(ui, name, FontId::proportional(13.0), t.text, rect.width() - 100.0);
                ui.painter().galley(pos2(rect.left() + 10.0, rect.center().y - g.size().y / 2.0), g, t.text);
                let count = crate::fmt::plural(*n, "album");
                ui.painter().text(pos2(rect.right() - 10.0, rect.center().y), egui::Align2::RIGHT_CENTER, count, FontId::proportional(12.0), t.muted);
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
                if resp.clicked() {
                    let id = id.clone().or_else(|| self.data.albums.iter().find(|a| a.artist.as_deref() == Some(name)).and_then(|a| a.artist_ref.clone()));
                    self.actions.push(Action::Go(Route::Artist { name: name.clone(), id }));
                }
                resp.context_menu(|ui| collection_menu(ui, &Collection::Artist(name.clone()), true, false, None, self.actions));
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use library::Album;

    fn album(id: &str, name: &str, artist: &str, year: i32, created: &str) -> Album {
        Album {
            id: id.into(),
            name: name.into(),
            sort: None,
            artist: Some(artist.into()),
            artist_ref: None,
            year: Some(year),
            genre: Some("Jazz".into()),
            song_count: 1,
            duration_secs: 1,
            cover_art: None,
            created: Some(created.into()),
            labels: vec!["Riverside".into()],
            genres: Vec::new(),
            artists: Vec::new(),
        }
    }

    fn data() -> Data {
        let mut d = Data::default();
        d.set_albums(vec![
            album("a", "Portrait in Jazz", "Bill Evans", 1960, "2024-01-03"),
            album("b", "Kind of Blue", "Miles Davis", 1959, "2024-01-01"),
            album("c", "Explorations", "Bill Evans", 1961, "2024-01-02"),
        ]);
        d
    }

    fn names(d: &Data, ids: &[usize]) -> Vec<String> {
        ids.iter().map(|&i| d.albums[i].name.clone()).collect()
    }

    #[test]
    fn recent_prefers_server_order_then_created() {
        let mut d = data();
        assert_eq!(names(&d, &grid_ids(&Route::Recent, AlbumSort::Artist, &d)), ["Portrait in Jazz", "Explorations", "Kind of Blue"]);
        d.newest = vec!["b".into(), "missing".into(), "a".into()];
        assert_eq!(names(&d, &grid_ids(&Route::Recent, AlbumSort::Artist, &d)), ["Kind of Blue", "Portrait in Jazz"]);
    }

    #[test]
    fn sorts_and_filters() {
        let d = data();
        assert_eq!(names(&d, &grid_ids(&Route::Albums, AlbumSort::Title, &d)), ["Explorations", "Kind of Blue", "Portrait in Jazz"]);
        assert_eq!(names(&d, &grid_ids(&Route::Albums, AlbumSort::Year, &d))[0], "Kind of Blue");
        let artist = Route::Artist { name: "Bill Evans".into(), id: None };
        assert_eq!(names(&d, &grid_ids(&artist, AlbumSort::Title, &d)), ["Portrait in Jazz", "Explorations"]);
        let year = Route::Filter { kind: FilterKind::Year, value: "1959".into() };
        assert_eq!(names(&d, &grid_ids(&year, AlbumSort::Artist, &d)), ["Kind of Blue"]);
        let genre = Route::Filter { kind: FilterKind::Genre, value: "jazz".into() };
        assert_eq!(grid_ids(&genre, AlbumSort::Artist, &d).len(), 3);
        let label = Route::Filter { kind: FilterKind::Label, value: "Blue Note".into() };
        assert!(grid_ids(&label, AlbumSort::Artist, &d).is_empty());
    }

    #[test]
    fn grid_fills_width() {
        assert_eq!(grid_metrics(100.0).0, 1);
        let (cols, tile) = grid_metrics(900.0);
        assert_eq!(cols, 5);
        assert!((tile * cols as f32 + GAP_X * (cols - 1) as f32 - 900.0).abs() < 0.01);
    }

    #[test]
    fn albums_and_playlists_download_but_artists_do_not() {
        assert!(matches!(Collection::Album("a".into()).keep(), Some(Keep::Album(id)) if id == "a"));
        assert!(matches!(Collection::Playlist("p".into()).keep(), Some(Keep::Playlist(id)) if id == "p"));
        assert!(Collection::Artist("Bill Evans".into()).keep().is_none());
        assert!(matches!(Collection::Playlist("p".into()).action(QueueAction::Add), Action::Playlist(id, QueueAction::Add) if id == "p"));
    }
}
