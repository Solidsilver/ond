//! The main view's router and the album-grid places: Recently Added, Albums,
//! Artists, an artist's page, and albums by genre / year / label.

use super::OfflineAction;
use super::offline_page::Mark;
use super::view_ctx::ViewCtx;
use super::virtual_list::VirtualList;
use super::{Action, AlbumSort, Data, Keep, OndApp, PrefsAction, QueueAction};
use crate::icons::{self, Icon};
use crate::nav::{FilterKind, Route};
use crate::settings::ViewMode;
use crate::theme::{Tokens, semibold};
use crate::widgets::{choice, elide, view_picker};
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};
use shell::kit::page_heading;
use std::sync::Arc;

const TILE_MIN: f32 = 160.0;
pub(super) const GAP_X: f32 = 20.0;
pub(super) const GAP_Y: f32 = 24.0;
/// Title + artist lines under the art.
pub(super) const LABEL_H: f32 = 40.0;
/// The photo beside an artist page's name.
const ARTIST_PHOTO: f32 = 96.0;

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

/// Indices into `data.albums` for a grid place, in display order (Albums'
/// turned around when `descending`).
pub fn grid_ids(route: &Route, sort: AlbumSort, descending: bool, data: &Data) -> Vec<usize> {
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
    if descending && *route == Route::Albums {
        ids.reverse();
    }
    ids
}

impl OndApp {
    pub(super) fn main_view(&mut self, ui: &mut Ui) {
        self.trail_bar(ui);
        let route = self.ui.nav.current().clone();
        match &route {
            Route::Recent | Route::Albums | Route::Filter { .. } => self.albums_place(ui, &route),
            Route::Artists => self.view().artists_page(ui),
            Route::Songs => self.view().songs_page(ui),
            Route::Artist { name, .. } => self.artist_page(ui, &route, name),
            Route::Album(id) => self.view().album_page(ui, id),
            Route::Playlist { id, name } => self.view().playlist_page(ui, id, name),
            Route::Favorites => self.view().favorites_page(ui),
            Route::Offline => self.view().offline_page(ui),
        }
    }

    /// A page of albums (Albums, Recently added, a genre, year or label): its
    /// heading, the sort (Albums only) and the grid/list picker, then the
    /// albums as tiles or as a table.
    fn albums_place(&mut self, ui: &mut Ui, route: &Route) {
        let (title, sub) = match route {
            Route::Albums => ("Albums".to_owned(), Some(self.data.albums.len().to_string())),
            Route::Filter { kind, value } => {
                let what = match kind {
                    FilterKind::Genre => "Genre",
                    FilterKind::Year => "Year",
                    FilterKind::Label => "Label",
                };
                (value.clone(), Some(what.to_owned()))
            }
            _ => ("Recently added".to_owned(), Some(self.view().cached_grid(route).len().to_string())),
        };
        let mut view = self.ui.prefs.album_view;
        // Only Albums sorts; the other pages keep their order. The list's
        // column headers sort too, so the pill shows only over the grid.
        let sort = (*route == Route::Albums && view == ViewMode::Grid).then_some((&AlbumSort::ALL[..], &mut self.ui.sort));
        if heading_bar(ui, &title, sub, Some(&mut view), sort, |s| s.label().into()) {
            self.ui.sort_desc = false;
        }
        if view != self.ui.prefs.album_view {
            self.actions.push(Action::Prefs(PrefsAction::AlbumView(view)));
        }
        match view {
            ViewMode::Grid => self.view().grid_place(ui, route),
            ViewMode::List => self.view().album_list(ui, route),
        }
    }

    /// Back / forward and the breadcrumb trail of recent places.
    fn trail_bar(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let back = ui
                .add_enabled_ui(self.ui.nav.can_back(), |ui| icons::sized_button(ui, Icon::ChevronLeft, "Back (⌘[)", vec2(26.0, 26.0), 10.0, Some(t.text)))
                .inner;
            if back.clicked() {
                self.actions.push(Action::Back);
            }
            let fwd = ui
                .add_enabled_ui(self.ui.nav.can_forward(), |ui| {
                    icons::sized_button(ui, Icon::ChevronRight, "Forward (⌘])", vec2(26.0, 26.0), 10.0, Some(t.text))
                })
                .inner;
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
                    if crate::widgets::link(ui, label, FontId::proportional(12.0), t.muted).clicked() {
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
        let credited = credited_albums(&mut self.ui.memos.credited, route, name, &own, &self.data);
        let sub = match own.len() {
            0 => None,
            1 => Some("1 album".to_owned()),
            n => Some(format!("{n} albums")),
        };
        let id = match route {
            Route::Artist { id, .. } => id.as_deref(),
            _ => None,
        };
        // Only artists the server has a photo id for get one (or their initial).
        let photo = self.ui.prefs.artist_photos.then(|| self.search.index.as_ref()?.artist(id, name)?.cover.clone()).flatten();
        ui.horizontal(|ui| {
            if let Some(cover) = &photo {
                let (rect, _) = ui.allocate_exact_size(vec2(ARTIST_PHOTO, ARTIST_PHOTO), Sense::hover());
                super::artists::avatar(&mut self.art, ui, &t, rect, Some(cover), name);
                ui.add_space(14.0);
            }
            ui.vertical(|ui| self.artist_header(ui, route, name, sub, !own.is_empty()));
        });
        ui.add_space(14.0);
        if own.is_empty() && credited.is_empty() {
            let (title, body) = if self.sync.tracks { ("Nothing yet", "Credits fill in while tracks sync.") } else { ("No albums credit this name", "") };
            crate::widgets::empty_state(ui, Icon::Disc, title, body);
            return;
        }
        let (cols, tile) = grid_metrics(ui.available_width());
        let row_h = tile + LABEL_H + GAP_Y;
        // Two grids with a heading between: lines of two heights, so the
        // virtual list's variable-height layout rather than its grid.
        let (own_rows, credited_rows) = (own.len().div_ceil(cols), credited.len().div_ceil(cols));
        let head = if credited_rows > 0 { 1 } else { 0 };
        let mut heights = vec![row_h; own_rows + head + credited_rows];
        if head > 0 {
            heights[own_rows] = CREDITED_HEAD_H;
        }
        super::layout::into_gutter(ui, |ui| {
            egui::ScrollArea::vertical().id_salt(("artist", name)).auto_shrink(false).content_margin(super::layout::gutter()).show_viewport(
                ui,
                |ui, viewport| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    super::virtual_list::visible_lines(ui, viewport, &heights, |ui, k| {
                        ui.push_id(k, |ui| {
                            if k < own_rows {
                                self.view().grid_row(ui, &own[k * cols..((k + 1) * cols).min(own.len())], tile);
                            } else if k == own_rows && head > 0 {
                                ui.allocate_ui_with_layout(vec2(ui.available_width(), CREDITED_HEAD_H), egui::Layout::top_down(egui::Align::Min), |ui| {
                                    ui.set_min_height(CREDITED_HEAD_H);
                                    ui.add_space(6.0);
                                    ui.label(egui::RichText::new("Also credited on").font(FontId::new(15.0, semibold())).color(t.text));
                                });
                            } else {
                                let r = k - own_rows - head;
                                self.view().grid_row(ui, &credited[r * cols..((r + 1) * cols).min(credited.len())], tile);
                            }
                        });
                    });
                },
            )
        });
    }

    /// The artist's name and album count, and (when they have albums) the
    /// buttons to play them and love them.
    fn artist_header(&mut self, ui: &mut Ui, route: &Route, name: &str, sub: Option<String>, albums: bool) {
        self.view().heading(ui, name, sub);
        if albums {
            ui.horizontal(|ui| {
                // Play takes their albums in the page's order; Shuffle mixes them.
                if let Some(a) = play_shuffle(ui) {
                    self.actions.push(Action::Artist(name.to_owned(), a));
                }
                // Only artists the server knows by id can be loved.
                if let Route::Artist { id: Some(id), .. } = route {
                    let loved = self.favorites.is_on(crate::favorites::StarKind::Artist, id);
                    if crate::widgets::heart_button(ui, loved, 30.0, 14.0).clicked() {
                        self.actions.push(Action::ToggleStar {
                            kind: crate::favorites::StarKind::Artist,
                            id: id.clone(),
                            track: None,
                            name: Some(name.to_owned()),
                        });
                    }
                }
                let more = icons::sized_button(ui, Icon::More, "More", vec2(30.0, 30.0), 13.0, Some(Tokens::of(ui).accent));
                egui::Popup::menu(&more).show(|ui| collection_menu(ui, &Collection::Artist(name.to_owned()), false, false, None, &mut self.actions));
            });
        }
    }
}

/// A page's heading with its controls at the right: the grid/list picker
/// when the page has one (`view`), and just left of it a pill of `tabs` (a
/// sort, or which list to show). True when the pill changed.
pub(super) fn heading_bar<T: Copy + PartialEq>(
    ui: &mut Ui,
    title: &str,
    sub: Option<String>,
    view: Option<&mut ViewMode>,
    tabs: Option<(&[T], &mut T)>,
    label: impl Fn(T) -> String,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        page_heading(ui, title, sub);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let picker = view.is_some();
            if let Some(view) = view {
                view_picker(ui, view);
            }
            if let Some((all, current)) = tabs {
                if picker {
                    ui.add_space(8.0);
                }
                // Right-to-left layout: the pill itself still reads left to right.
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| changed = choice(ui, all, current, None, label));
            }
        });
    });
    changed
}

/// The "Also credited on" line between an artist's own albums and the rest.
const CREDITED_HEAD_H: f32 = 34.0;

/// The artist page's memo key for the albums crediting them: the grid's own
/// route and library version, and a hash of the credited ids, which arrive
/// without bumping the version.
pub(super) type CreditedKey = (Route, u64, u64);

/// The albums that credit `name` and aren't among their `own`, built once
/// per route, library version and credit list.
fn credited_albums(memo: &mut crate::memo::Memo<CreditedKey, Vec<usize>>, route: &Route, name: &str, own: &[usize], data: &Data) -> Arc<Vec<usize>> {
    let ids = data.credited.get(name);
    let hash = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        ids.hash(&mut h);
        h.finish()
    };
    memo.get((route.clone(), data.version, hash), || {
        let own_ids: std::collections::HashSet<&str> = own.iter().map(|&i| data.albums[i].id.as_str()).collect();
        ids.map(|ids| ids.iter().filter(|id| !own_ids.contains(id.as_str())).filter_map(|id| data.by_id.get(id).copied()).collect()).unwrap_or_default()
    })
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
            // An artist's menu offers the shuffle across their albums.
            Collection::Artist(_) => items.push(("Shuffle all", QueueAction::Shuffle)),
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
    if crate::widgets::primary(ui, Some(Icon::Play), "Play").clicked() {
        picked = Some(QueueAction::Play { start: 0 });
    }
    ui.add_space(4.0);
    if crate::widgets::secondary(ui, "Shuffle").clicked() {
        picked = Some(QueueAction::Shuffle);
    }
    picked
}

/// The menu item that keeps something offline: "Make available offline" /
/// "Remove download" (`kept`), with the key that does it where there is one.
/// `what` names it as a [`Keep`], built only when clicked.
pub fn offline_item(ui: &mut Ui, kept: bool, shortcut: Option<&str>, what: impl FnOnce() -> Keep, actions: &mut Vec<Action>) {
    let label = if kept { "Remove download" } else { "Make available offline" };
    let mut button = egui::Button::new(label);
    if let Some(keys) = shortcut {
        button = button.shortcut_text(crate::keys::shown(keys));
    }
    if ui.add(button).clicked() {
        actions.push(Action::Offline(OfflineAction::Keep(what(), !kept)));
    }
}

impl ViewCtx<'_> {
    pub(super) fn heading(&self, ui: &mut Ui, title: &str, sub: Option<String>) {
        page_heading(ui, title, sub);
    }

    pub(super) fn cached_grid(&mut self, route: &Route) -> Arc<Vec<usize>> {
        let (sort, desc, data) = (self.ui.sort, self.ui.sort_desc, &self.data);
        self.ui.grid.get((route.clone(), sort, desc, data.version), || grid_ids(route, sort, desc, data))
    }

    /// A full-height, virtualized grid: only visible rows are laid out, so only
    /// visible covers are requested.
    pub(super) fn grid_place(&mut self, ui: &mut Ui, route: &Route) {
        let ids = self.cached_grid(route);
        if ids.is_empty() {
            let title = if self.data.albums.is_empty() { "Loading your library…" } else { "No albums here." };
            crate::widgets::empty_state(ui, Icon::Disc, title, "");
            return;
        }
        let marks = self.grid_marks(route);
        self.album_grid(ui, ("grid", route), marks.as_deref(), &ids);
    }

    /// `ids` as a virtualized grid named `list`, the rail beside it when
    /// there are `marks`.
    pub(super) fn album_grid<L: std::hash::Hash + std::fmt::Debug + Copy>(&mut self, ui: &mut Ui, list: L, marks: Option<&super::alpha::Marks>, ids: &[usize]) {
        let margin = super::virtual_list::margin(marks.is_some());
        let (cols, tile) = grid_metrics(ui.available_width() - f32::from(margin.right) + super::layout::GUTTER);
        let rows = ids.len().div_ceil(cols);
        let row_h = tile + LABEL_H + GAP_Y;
        VirtualList::grid(list, marks, row_h, rows, cols).show(ui, |ui, range| {
            let row_ids = |row: usize| &ids[row * cols..((row + 1) * cols).min(ids.len())];
            for row in range.clone() {
                self.grid_row(ui, row_ids(row), tile);
            }
            // The rows just above and below load next, after what's on screen.
            for row in super::virtual_list::near(&range, 1, rows) {
                for &i in row_ids(row) {
                    if let Some(cover) = self.data.albums[i].cover_art.as_deref() {
                        self.art.prefetch(cover, tile);
                    }
                }
            }
        });
    }

    /// Where each letter starts in a grid sorted by name (albums by artist or
    /// by title), for the alphabet rail; `None` for one sorted otherwise, or
    /// with the rail off.
    pub(super) fn grid_marks(&mut self, route: &Route) -> Option<Arc<super::alpha::Marks>> {
        if !self.ui.prefs.alpha_rail {
            return None;
        }
        let sort = match route {
            Route::Albums => self.ui.sort,
            Route::Filter { .. } => AlbumSort::Artist,
            _ => return None,
        };
        let by_title = match sort {
            AlbumSort::Artist => false,
            AlbumSort::Title => true,
            _ => return None,
        };
        let ids = self.cached_grid(route);
        let albums = &self.data.albums;
        Some(self.ui.grid_marks.get((route.clone(), sort, self.ui.sort_desc, self.data.version), || {
            super::alpha::Marks::new(ids.iter().map(|&i| if by_title { albums[i].name.as_str() } else { albums[i].artist.as_deref().unwrap_or("") }))
        }))
    }

    /// One grid row of tiles, followed by the row gap.
    pub(super) fn grid_row(&mut self, ui: &mut Ui, ids: &[usize], tile: f32) {
        let t = Tokens::of(ui);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = GAP_X;
            for &i in ids {
                self.album_tile(ui, &t, i, tile);
            }
        });
        ui.add_space(GAP_Y - ui.spacing().item_spacing.y);
    }

    /// Cover, title and artist. Click opens; the hover play button plays; right-click has the queue actions.
    pub(super) fn album_tile(&mut self, ui: &mut Ui, t: &Tokens, idx: usize, tile: f32) {
        let (rect, resp) = ui.allocate_exact_size(vec2(tile, tile + LABEL_H), Sense::click());
        // Borrowed from the library, not from `self`, so drawing can take `&mut self`.
        let data = self.data;
        let album = &data.albums[idx];
        let id = album.id.as_str();
        let pin = self.album_pin(id);
        if ui.is_rect_visible(rect) {
            let art = Rect::from_min_size(rect.min, vec2(tile, tile));
            self.art.paint(ui, art, album.cover_art.as_deref(), 6.0);
            // Only albums kept offline carry a mark; nothing otherwise.
            let mark = Mark::of_pin(pin);
            let mark_w = if mark.is_some() { 16.0 } else { 0.0 };
            let title = elide(ui, &album.name, FontId::proportional(13.0), t.text, tile);
            ui.painter().galley(pos2(rect.left(), art.bottom() + 7.0), title, t.text);
            let sub = elide(ui, album.artist.as_deref().unwrap_or_default(), FontId::proportional(12.0), t.muted, tile - mark_w);
            ui.painter().galley(pos2(rect.left(), art.bottom() + 24.0), sub, t.muted);
            if let Some(m) = mark {
                super::offline_page::mark(ui, pos2(rect.right() - 6.0, art.bottom() + 31.0), 11.0, m);
            }
            let play = Rect::from_center_size(pos2(art.left() + 24.0, art.bottom() - 24.0), vec2(32.0, 32.0));
            if crate::widgets::tile_button(ui, t, play, ui.scope_id().with(("tile-play", id)), resp.hovered(), Icon::Play, "Play album") {
                self.actions.push(Action::Album(id.to_owned(), QueueAction::Play { start: 0 }));
                return;
            }
        }
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{} by {}", album.name, album.artist.as_deref().unwrap_or("Unknown artist")))
        });
        if resp.clicked() {
            self.actions.push(Action::Go(Route::Album(id.to_owned())));
        }
        let kept = pin.is_some();
        resp.context_menu(|ui| self.album_menu(ui, album, kept));
    }

    /// An album's right-click menu, on its tile or its row.
    pub(super) fn album_menu(&mut self, ui: &mut Ui, album: &library::Album, kept: bool) {
        collection_menu(ui, &Collection::Album(album.id.clone()), true, kept, None, self.actions);
        if let Some(name) = &album.artist {
            ui.separator();
            if ui.button(format!("Go to {name}")).clicked() {
                self.actions.push(Action::Go(Route::Artist { name: name.clone(), id: album.artist_ref.clone() }));
            }
        }
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
        assert_eq!(names(&d, &grid_ids(&Route::Recent, AlbumSort::Artist, false, &d)), ["Portrait in Jazz", "Explorations", "Kind of Blue"]);
        d.newest = vec!["b".into(), "missing".into(), "a".into()];
        assert_eq!(names(&d, &grid_ids(&Route::Recent, AlbumSort::Artist, false, &d)), ["Kind of Blue", "Portrait in Jazz"]);
    }

    #[test]
    fn credits_that_arrive_later_show_on_an_open_artist_page() {
        let mut d = data();
        let mut memo = crate::memo::Memo::default();
        let artist = Route::Artist { name: "Bill Evans".into(), id: None };
        let own = grid_ids(&artist, AlbumSort::Title, false, &d);
        assert!(credited_albums(&mut memo, &artist, "Bill Evans", &own, &d).is_empty());
        // Credits come in on their own, without a new library version.
        d.credited.insert("Bill Evans".into(), vec!["a".into(), "b".into()]);
        let credited = credited_albums(&mut memo, &artist, "Bill Evans", &own, &d);
        assert_eq!(names(&d, &credited), ["Kind of Blue"], "their own albums aren't counted twice");
    }

    #[test]
    fn sorts_and_filters() {
        let d = data();
        assert_eq!(names(&d, &grid_ids(&Route::Albums, AlbumSort::Title, false, &d)), ["Explorations", "Kind of Blue", "Portrait in Jazz"]);
        assert_eq!(names(&d, &grid_ids(&Route::Albums, AlbumSort::Year, false, &d))[0], "Kind of Blue");
        let artist = Route::Artist { name: "Bill Evans".into(), id: None };
        assert_eq!(names(&d, &grid_ids(&artist, AlbumSort::Title, false, &d)), ["Portrait in Jazz", "Explorations"]);
        let year = Route::Filter { kind: FilterKind::Year, value: "1959".into() };
        assert_eq!(names(&d, &grid_ids(&year, AlbumSort::Artist, false, &d)), ["Kind of Blue"]);
        let genre = Route::Filter { kind: FilterKind::Genre, value: "jazz".into() };
        assert_eq!(grid_ids(&genre, AlbumSort::Artist, false, &d).len(), 3);
        let label = Route::Filter { kind: FilterKind::Label, value: "Blue Note".into() };
        assert!(grid_ids(&label, AlbumSort::Artist, false, &d).is_empty());
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
