//! Albums as a table: a row per album with its cover small, and its artist,
//! year, genre, songs, length and when it was added in columns. The pages of
//! albums show this or the grid (Settings keep which). On Albums, a column's
//! header sorts by it, as the sort pill does; the other pages keep their
//! own order.

use super::table::{HEAD_H, Head};
use super::track_row::{TrackRow, col};
use super::view_ctx::ViewCtx;
use super::widgets::{Mark, elide};
use super::{Action, AlbumSort, QueueAction};
use crate::nav::Route;
use crate::theme::Tokens;
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};

const ROW_H: f32 = 40.0;
const ART: f32 = 28.0;
const YEAR_W: f32 = 64.0;
const SONGS_W: f32 = 64.0;
const LENGTH_W: f32 = 80.0;
const ADDED_W: f32 = 112.0;
/// Rows past either end whose covers load ahead.
const PREFETCH_ROWS: usize = 8;

impl ViewCtx<'_> {
    pub(super) fn album_list(&mut self, ui: &mut Ui, route: &Route) {
        let t = Tokens::of(ui);
        let ids = self.cached_grid(route);
        if ids.is_empty() {
            ui.label(egui::RichText::new(if self.data.albums.is_empty() { "Loading your library…" } else { "No albums here." }).color(t.muted));
            return;
        }
        let marks = self.grid_marks(route);
        let rail_w = if marks.is_some() { super::alpha::RAIL_W } else { 0.0 };
        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width() - rail_w, HEAD_H), Sense::hover());
        let c = columns(head);
        // Only Albums sorts; the other pages keep their order.
        let sorts = |by: AlbumSort| (*route == Route::Albums).then_some(by);
        let heads = [
            Head { label: "Title", rect: c.title, right: false, sorts: sorts(AlbumSort::Title) },
            Head { label: "Artist", rect: c.artist, right: false, sorts: sorts(AlbumSort::Artist) },
            Head { label: "Year", rect: c.year, right: false, sorts: sorts(AlbumSort::Year) },
            Head { label: "Genre", rect: c.genre, right: false, sorts: None },
            Head { label: "Songs", rect: c.songs, right: true, sorts: None },
            Head { label: "Length", rect: c.length, right: true, sorts: None },
            Head { label: "Added", rect: c.added, right: false, sorts: sorts(AlbumSort::Added) },
        ];
        super::table::header(ui, head, "albums-col", &heads, &mut self.ui.sort, &mut self.ui.sort_desc);

        let list = ("albums-list", route);
        super::layout::into_gutter(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let area = ui.available_rect_before_wrap();
            let mut scroll = egui::ScrollArea::vertical().id_salt(list).auto_shrink(false);
            scroll = scroll.content_margin(if marks.is_some() { super::alpha::margin() } else { super::layout::gutter() });
            if let Some(y) = super::alpha::take_jump(ui, list) {
                scroll = scroll.vertical_scroll_offset(y);
            }
            let mut top = 0;
            let out = scroll.show_rows(ui, ROW_H, ids.len(), |ui, range| {
                top = range.start;
                // The covers just past either end, so scrolling finds them loaded.
                let near = range.start.saturating_sub(PREFETCH_ROWS)..range.start;
                for &i in ids[near].iter().chain(ids.iter().skip(range.end).take(PREFETCH_ROWS)) {
                    if let Some(cover) = self.data.albums[i].cover_art.as_deref() {
                        self.art.prefetch(cover, ART);
                    }
                }
                for &i in &ids[range] {
                    self.album_row(ui, &t, i);
                }
            });
            if let Some(marks) = marks {
                super::alpha::rail(ui, list, area, &out, &marks, top, |i| i, ROW_H);
            }
        });
    }

    /// One album's row. Click opens it, double-click plays it, right-click
    /// has the rest (as its tile does).
    fn album_row(&mut self, ui: &mut Ui, t: &Tokens, idx: usize) {
        let row = TrackRow::new(ui, ROW_H, Sense::click());
        // Borrowed from the library, not from `self`, so drawing can take `&mut self`.
        let data = self.data;
        let album = &data.albums[idx];
        let pin = self.album_pin(&album.id);
        if ui.is_rect_visible(row.rect) {
            row.fill(ui, t, false, row.pointer_on());
            let c = columns(row.rect);
            let art = Rect::from_center_size(pos2(c.title.left() + col::INSET + ART / 2.0, row.cy()), vec2(ART, ART));
            self.art.paint(ui, art, album.cover_art.as_deref(), col::ART_RADIUS);
            let x = art.right() + 10.0;
            // Only albums kept offline carry a mark, at the title's end.
            let mark = Mark::of_pin(pin);
            let mark_w = if mark.is_some() { 18.0 } else { 0.0 };
            let w = row.line(ui, x, c.title.right() - col::INSET - x - mark_w, &album.name, FontId::proportional(13.0), t.text);
            if let Some(m) = mark {
                super::widgets::mark(ui, pos2(x + w + 10.0, row.cy()), col::MARK, m);
            }
            let cell = |ui: &Ui, r: Rect, text: &str| {
                row.line(ui, r.left() + col::INSET, r.width() - 2.0 * col::INSET, text, FontId::proportional(13.0), t.muted);
            };
            cell(ui, c.artist, album.artist.as_deref().unwrap_or(""));
            cell(ui, c.year, &album.year.map(|y| y.to_string()).unwrap_or_default());
            cell(ui, c.genre, &album.all_genres().join(", "));
            row.right_text(ui, c.songs.right() - col::INSET, album.song_count.to_string(), FontId::proportional(12.0), t.muted);
            row.right_text(ui, c.length.right() - col::INSET, crate::fmt::clock(f64::from(album.duration_secs)), FontId::proportional(12.0), t.muted);
            let added = album.created.as_deref().map(crate::fmt::date).unwrap_or_default();
            let g = elide(ui, &added, FontId::proportional(12.0), t.muted, c.added.width() - 2.0 * col::INSET);
            ui.painter().galley(pos2(c.added.left() + col::INSET, row.cy() - g.size().y / 2.0), g, t.muted);
        }
        row.describe(false, || format!("{} by {}", album.name, album.artist.as_deref().unwrap_or("Unknown artist")));
        if row.resp.double_clicked() {
            self.actions.push(Action::Album(album.id.clone(), QueueAction::Play { start: 0 }));
        } else if row.resp.clicked() {
            self.actions.push(Action::Go(Route::Album(album.id.clone())));
        }
        let kept = pin.is_some();
        row.resp.context_menu(|ui| self.album_menu(ui, album, kept));
    }
}

struct Columns {
    title: Rect,
    artist: Rect,
    year: Rect,
    genre: Rect,
    songs: Rect,
    length: Rect,
    added: Rect,
}

/// Title, artist and genre share what's left after the fixed columns.
fn columns(row: Rect) -> Columns {
    let flex = (row.width() - YEAR_W - SONGS_W - LENGTH_W - ADDED_W).max(0.0);
    let col = |x: f32, w: f32| Rect::from_min_size(pos2(x, row.top()), vec2(w, row.height()));
    let title = col(row.left(), flex * 0.4);
    let artist = col(title.right(), flex * 0.32);
    let year = col(artist.right(), YEAR_W);
    let genre = col(year.right(), flex * 0.28);
    let songs = col(genre.right(), SONGS_W);
    let length = col(songs.right(), LENGTH_W);
    let added = col(length.right(), ADDED_W);
    Columns { title, artist, year, genre, songs, length, added }
}
