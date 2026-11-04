//! Artists: every artist, as round photos in a grid or as a table of their
//! genre, album count and years (Settings keep which), sorted by name, by
//! how many albums, or by when they started. The photos are the server's
//! (Settings › Appearance › Lists can turn them off).

use super::browse::{Collection, GAP_X, GAP_Y, LABEL_H, collection_menu, grid_metrics};
use super::table::{Head, Width};
use super::track_row::{TrackRow, col};
use super::view_ctx::ViewCtx;
use super::virtual_list::{VirtualList, near};
use super::{Action, ArtistSort, PrefsAction, QueueAction};
use crate::art::Art;
use crate::icons::Icon;
use crate::memo::Memo;
use crate::nav::Route;
use crate::search::ArtistEntry;
use crate::settings::ViewMode;
use crate::theme::{Tokens, semibold};
use crate::widgets::{choice, elide, view_picker};
use egui::{Align2, FontId, Rect, Sense, Ui, pos2, vec2};
use std::sync::Arc;

/// A row of the table, and the photo in it.
const ROW_H: f32 = 40.0;
const AVATAR: f32 = 28.0;
const ALBUMS_W: f32 = 80.0;
const YEARS_W: f32 = 120.0;
/// Rows past either end of the table whose photos load ahead.
const PREFETCH_ROWS: usize = 8;

impl ViewCtx<'_> {
    pub(super) fn artists_page(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let Some(index) = &self.search.index else {
            self.heading(ui, "Artists", None);
            ui.label(egui::RichText::new("Loading your library…").color(t.muted));
            return;
        };
        // The server's artists (with ids) once synced, else album-artist names.
        let artists = Arc::clone(&index.artists);
        let mut view = self.ui.prefs.artist_view;
        ui.horizontal(|ui| {
            self.heading(ui, "Artists", Some(artists.len().to_string()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if view_picker(ui, &mut view) {
                    self.actions.push(Action::Prefs(PrefsAction::ArtistView(view)));
                }
                ui.add_space(8.0);
                // Right-to-left layout: the pill itself still reads left to right.
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    if choice(ui, &ArtistSort::ALL, &mut self.ui.artist_sort, None, |s| s.label().into()) {
                        self.ui.artist_desc = false;
                    }
                });
            });
        });
        let order = self.artist_order(&artists);
        let marks = self.artist_marks(&artists, &order);
        match view {
            ViewMode::Grid => self.artist_grid(ui, &artists, &order, marks),
            ViewMode::List => self.artist_table(ui, &artists, &order, marks),
        }
    }

    fn artist_order(&mut self, artists: &Arc<Vec<ArtistEntry>>) -> Arc<Vec<usize>> {
        artist_order(&mut self.ui.artist_order, artists, self.ui.artist_sort, self.ui.artist_desc)
    }

    /// Where each letter starts, sorted by name; `None` sorted otherwise, or
    /// with the rail off.
    fn artist_marks(&mut self, artists: &Arc<Vec<ArtistEntry>>, order: &[usize]) -> Option<Arc<super::alpha::Marks>> {
        if !self.ui.prefs.alpha_rail || self.ui.artist_sort != ArtistSort::Name {
            return None;
        }
        let key = (SameList(Arc::clone(artists)), self.ui.artist_desc);
        Some(self.ui.artist_marks.get(key, || super::alpha::Marks::new(order.iter().map(|&i| artists[i].sort.as_str()))))
    }

    /// Round photos with the name and albums under each, a row of them at a
    /// time (only the rows on screen are laid out).
    fn artist_grid(&mut self, ui: &mut Ui, artists: &[ArtistEntry], order: &[usize], marks: Option<Arc<super::alpha::Marks>>) {
        let t = Tokens::of(ui);
        let photos = self.ui.prefs.artist_photos;
        let margin = super::virtual_list::margin(marks.is_some());
        let (cols, tile) = grid_metrics(ui.available_width() - f32::from(margin.right) + super::layout::GUTTER);
        let rows = order.len().div_ceil(cols);
        let row_h = tile + LABEL_H + GAP_Y;
        VirtualList::grid("artists-grid", marks.as_deref(), row_h, rows, cols).show(ui, |ui, range| {
            let row_ids = |row: usize| &order[row * cols..((row + 1) * cols).min(order.len())];
            for row in range.clone() {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP_X;
                    for &i in row_ids(row) {
                        self.artist_tile(ui, &t, &artists[i], tile);
                    }
                });
                ui.add_space(GAP_Y - ui.spacing().item_spacing.y);
            }
            // The rows just above and below load next, after what's on screen.
            if photos {
                for row in near(&range, 1, rows) {
                    for cover in row_ids(row).iter().filter_map(|&i| artists[i].cover.as_deref()) {
                        self.art.prefetch(cover, tile);
                    }
                }
            }
        });
    }

    /// An artist's photo, name and albums. Click opens them; the hover button
    /// (or a double-click) shuffles them; right-click has the rest.
    fn artist_tile(&mut self, ui: &mut Ui, t: &Tokens, a: &ArtistEntry, tile: f32) {
        let (rect, resp) = ui.allocate_exact_size(vec2(tile, tile + LABEL_H), Sense::click());
        if ui.is_rect_visible(rect) {
            let photo = Rect::from_min_size(rect.min, vec2(tile, tile));
            let cover = a.cover.as_deref().filter(|_| self.ui.prefs.artist_photos);
            avatar(self.art, ui, t, photo, cover, &a.name);
            let name = elide(ui, &a.name, FontId::proportional(13.0), t.text, tile);
            ui.painter().galley(pos2(rect.center().x - name.size().x / 2.0, photo.bottom() + 7.0), name, t.text);
            let sub = elide(ui, &albums_and_years(a), FontId::proportional(12.0), t.muted, tile);
            ui.painter().galley(pos2(rect.center().x - sub.size().x / 2.0, photo.bottom() + 24.0), sub, t.muted);
            // On the circle's edge, low on the left, where the album tiles have theirs.
            let at = photo.center() + vec2(-1.0, 1.0) * (tile / 2.0 * std::f32::consts::FRAC_1_SQRT_2);
            let shuffle = Rect::from_center_size(at, vec2(32.0, 32.0));
            let id = ui.scope_id().with(("artist-shuffle", &a.name));
            if crate::widgets::tile_button(ui, t, shuffle, id, resp.hovered(), Icon::Shuffle, "Shuffle artist") {
                self.actions.push(Action::Artist(a.name.clone(), QueueAction::Play { start: 0 }));
                return;
            }
        }
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &a.name));
        if resp.double_clicked() {
            self.actions.push(Action::Artist(a.name.clone(), QueueAction::Play { start: 0 }));
        } else if resp.clicked() {
            self.open_artist(a);
        }
        resp.context_menu(|ui| collection_menu(ui, &Collection::Artist(a.name.clone()), true, false, None, self.actions));
    }

    /// The table: name (with the photo), genre, albums and years.
    fn artist_table(&mut self, ui: &mut Ui, artists: &[ArtistEntry], order: &[usize], marks: Option<Arc<super::alpha::Marks>>) {
        let t = Tokens::of(ui);
        let photos = self.ui.prefs.artist_photos;
        let head = super::table::head_rect(ui, marks.is_some());
        let c = columns(head);
        let heads = [
            Head { label: "Name", rect: c.name, right: false, sorts: Some(ArtistSort::Name) },
            Head { label: "Genre", rect: c.genre, right: false, sorts: None },
            Head { label: "Albums", rect: c.albums, right: true, sorts: Some(ArtistSort::Albums) },
            Head { label: "Years", rect: c.years, right: false, sorts: Some(ArtistSort::Year) },
        ];
        super::table::header(ui, head, "artists-col", &heads, &mut self.ui.artist_sort, &mut self.ui.artist_desc);
        VirtualList::table("artists", marks.as_deref(), ROW_H, order.len()).show(ui, |ui, range| {
            if photos {
                // The photos just past either end, so scrolling finds them loaded.
                for cover in near(&range, PREFETCH_ROWS, order.len()).filter_map(|row| artists[order[row]].cover.as_deref()) {
                    self.art.prefetch(cover, AVATAR);
                }
            }
            for &i in &order[range] {
                self.artist_row(ui, &t, &artists[i], photos);
            }
        });
    }

    fn artist_row(&mut self, ui: &mut Ui, t: &Tokens, a: &ArtistEntry, photos: bool) {
        let row = TrackRow::new(ui, ROW_H, Sense::click());
        if ui.is_rect_visible(row.rect) {
            row.fill(ui, t, false, row.pointer_on());
            let c = columns(row.rect);
            let mut x = c.name.left() + col::INSET;
            if photos {
                let face = Rect::from_center_size(pos2(x + AVATAR / 2.0, row.cy()), vec2(AVATAR, AVATAR));
                avatar(self.art, ui, t, face, a.cover.as_deref(), &a.name);
                x = face.right() + 10.0;
            }
            row.line(ui, x, c.name.right() - col::INSET - x, &a.name, FontId::proportional(13.0), t.text);
            let cell = |ui: &Ui, r: Rect, text: &str| {
                row.line(ui, r.left() + col::INSET, r.width() - 2.0 * col::INSET, text, FontId::proportional(13.0), t.muted);
            };
            cell(ui, c.genre, a.genre.as_deref().unwrap_or(""));
            row.right_text(ui, c.albums.right() - col::INSET, a.albums.to_string(), FontId::proportional(12.0), t.muted);
            cell(ui, c.years, &years(a).unwrap_or_default());
        }
        row.describe(false, || a.name.clone());
        if row.resp.double_clicked() {
            self.actions.push(Action::Artist(a.name.clone(), QueueAction::Play { start: 0 }));
        } else if row.resp.clicked() {
            self.open_artist(a);
        }
        row.resp.context_menu(|ui| collection_menu(ui, &Collection::Artist(a.name.clone()), true, false, None, self.actions));
    }

    /// Goes to the artist's page. One known only by name (no artists from
    /// the server yet) takes the id its albums give it.
    fn open_artist(&mut self, a: &ArtistEntry) {
        let id = a.id.clone().or_else(|| self.data.albums.iter().find(|al| al.artist.as_deref() == Some(a.name.as_str())).and_then(|al| al.artist_ref.clone()));
        self.actions.push(Action::Go(Route::Artist { name: a.name.clone(), id }));
    }
}

/// The artists' list as a memo key: equal only to the same list. It holds
/// the list, so while an order built from it is cached its address can't be
/// handed to the next index the library builds (a stale order would index a
/// different, maybe shorter, list).
pub(crate) struct SameList(Arc<Vec<ArtistEntry>>);

impl PartialEq for SameList {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Indices into `artists` in the order Artists is sorted by. By name it's
/// the list's own; ties keep that order too.
fn artist_order(memo: &mut Memo<(SameList, ArtistSort, bool), Vec<usize>>, artists: &Arc<Vec<ArtistEntry>>, by: ArtistSort, desc: bool) -> Arc<Vec<usize>> {
    memo.get((SameList(Arc::clone(artists)), by, desc), || {
        let mut order: Vec<usize> = (0..artists.len()).collect();
        match by {
            ArtistSort::Name => {}
            ArtistSort::Albums => order.sort_by_key(|&i| std::cmp::Reverse(artists[i].albums)),
            // Those without a year last.
            ArtistSort::Year => order.sort_by_key(|&i| artists[i].years.map_or(i32::MAX, |y| y.0)),
        }
        if desc {
            order.reverse();
        }
        order
    })
}

/// "1979–1990", or the one year; `None` without any.
fn years(a: &ArtistEntry) -> Option<String> {
    a.years.map(|(from, to)| if from == to { from.to_string() } else { format!("{from}–{to}") })
}

/// "3 albums · 1979–1990", under an artist's tile.
fn albums_and_years(a: &ArtistEntry) -> String {
    let albums = crate::fmt::plural(a.albums, "album");
    match years(a) {
        Some(y) => format!("{albums} · {y}"),
        None => albums,
    }
}

/// An artist's photo in a circle, or their initial when the server has no
/// photo of them (or it's still loading), or there's no `cover` to ask for.
pub(super) fn avatar(art: &mut Art, ui: &Ui, t: &Tokens, rect: Rect, cover: Option<&str>, name: &str) {
    let radius = egui::CornerRadius::same((rect.width() / 2.0).min(255.0) as u8);
    match cover.and_then(|c| art.portrait(c, rect.width())) {
        Some(tex) => {
            let photo = egui::epaint::RectShape::filled(rect, radius, egui::Color32::WHITE).with_texture(tex.id(), crate::art::crop_uv(1.0));
            ui.painter().add(photo);
        }
        None => {
            ui.painter().rect_filled(rect, radius, t.placeholder);
            let initial = name.chars().find(|c| c.is_alphanumeric()).map_or('#', |c| c.to_uppercase().next().unwrap_or(c));
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, initial, FontId::new((rect.width() * 0.42).min(44.0), semibold()), t.muted);
        }
    }
}

struct Columns {
    name: Rect,
    genre: Rect,
    albums: Rect,
    years: Rect,
}

/// Name and genre share what's left after the albums and years.
fn columns(row: Rect) -> Columns {
    let [name, genre, albums, years] = super::table::columns(row, [Width::Share(0.55), Width::Share(0.45), Width::Fixed(ALBUMS_W), Width::Fixed(YEARS_W)]);
    Columns { name, genre, albums, years }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, albums: usize) -> ArtistEntry {
        ArtistEntry { id: None, name: name.into(), sort: name.into(), albums, cover: None, years: None, genre: None }
    }

    #[test]
    fn a_new_artist_list_never_reuses_the_old_order() {
        let mut memo = Memo::default();
        let first = Arc::new(vec![entry("A", 1), entry("B", 3), entry("C", 2)]);
        assert_eq!(*artist_order(&mut memo, &first, ArtistSort::Albums, false), [1, 2, 0]);
        // The index is rebuilt and the old list let go of. A same-sized
        // allocation is the allocator's first pick for the new one, so an
        // order keyed by address alone would come back for it.
        drop(first);
        let second = Arc::new(vec![entry("D", 1)]);
        assert_eq!(*artist_order(&mut memo, &second, ArtistSort::Albums, false), [0]);
    }
}
