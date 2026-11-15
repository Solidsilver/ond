//! Albums as a table: a row per album with its cover small, and its artist,
//! year, genre, songs, length and when it was added in columns. The pages of
//! albums show this or the grid (Settings keep which). On Albums, a column's
//! header sorts by it, as the sort pill does; the other pages keep their
//! own order.

use super::offline_page::pin_mark;
use super::table::{Head, Width};
use super::track_row::{TrackRow, col};
use super::view_ctx::ViewCtx;
use super::virtual_list::{VirtualList, near};
use super::{Action, AlbumSort};
use crate::nav::Route;
use crate::theme::Tokens;
use crate::widgets::elide;
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};

const ROW_H: f32 = 40.0;
const ART: f32 = 28.0;
const YEAR_W: f32 = 64.0;
const SONGS_W: f32 = 64.0;
const LENGTH_W: f32 = 80.0;
const ADDED_W: f32 = 112.0;
/// The Source column, while the list mixes sources: room for a dot and a
/// short name, which is cut to fit.
pub(super) const SOURCE_W: f32 = 112.0;
/// What the shared columns (title, artist, genre) keep before the Source
/// column gives way to them.
const SOURCE_MIN_FLEX: f32 = 320.0;
/// Rows past either end whose covers load ahead.
const PREFETCH_ROWS: usize = 8;

impl ViewCtx<'_> {
    pub(super) fn album_list(&mut self, ui: &mut Ui, route: &Route) {
        let t = Tokens::of(ui);
        let ids = self.cached_grid(route);
        if ids.is_empty() {
            let title = if self.data.albums.is_empty() { "Loading your library…" } else { "No albums here." };
            crate::widgets::empty_state(ui, crate::icons::Icon::Disc, title, "");
            return;
        }
        let marks = self.grid_marks(route);
        let head = super::table::head_rect(ui, marks.is_some());
        let c = columns(head, self.mixed_sources());
        // Only Albums sorts; the other pages keep their order.
        let sorts = |by: AlbumSort| (*route == Route::Albums).then_some(by);
        let mut heads = vec![
            Head { label: "Title", rect: c.title, right: false, sorts: sorts(AlbumSort::Title) },
            Head { label: "Artist", rect: c.artist, right: false, sorts: sorts(AlbumSort::Artist) },
            Head { label: "Year", rect: c.year, right: false, sorts: sorts(AlbumSort::Year) },
            Head { label: "Genre", rect: c.genre, right: false, sorts: None },
            Head { label: "Songs", rect: c.songs, right: true, sorts: None },
            Head { label: "Length", rect: c.length, right: true, sorts: None },
            Head { label: "Added", rect: c.added, right: false, sorts: sorts(AlbumSort::Added) },
        ];
        if let Some(source) = c.source {
            heads.insert(2, Head { label: "Source", rect: source, right: false, sorts: None });
        }
        super::table::header(ui, head, "albums-col", &heads, &mut self.ui.sort, &mut self.ui.sort_desc);

        let sources = self.mixed_sources();
        VirtualList::table(("albums-list", route), marks.as_deref(), ROW_H, ids.len()).show(ui, |ui, range| {
            // The covers just past either end, so scrolling finds them loaded.
            for row in near(&range, PREFETCH_ROWS, ids.len()) {
                if let Some(cover) = self.data.albums[ids[row]].cover_art.as_deref() {
                    self.art.prefetch(cover, ART);
                }
            }
            for &i in &ids[range] {
                self.album_row(ui, &t, i, sources);
            }
        });
    }

    /// One album's row. Click opens it, right-click has the rest (Play
    /// among them) (as its tile does).
    fn album_row(&mut self, ui: &mut Ui, t: &Tokens, idx: usize, sources: bool) {
        let row = TrackRow::new(ui, ROW_H, Sense::click());
        // Borrowed from the library, not from `self`, so drawing can take `&mut self`.
        let data = self.data;
        let album = &data.albums[idx];
        let pin = self.album_pin(&album.id);
        if ui.is_rect_visible(row.rect) {
            row.fill(ui, t, false, row.pointer_on());
            let c = columns(row.rect, sources);
            let art = Rect::from_center_size(pos2(c.title.left() + col::INSET + ART / 2.0, row.cy()), vec2(ART, ART));
            self.art.paint_as(ui, art, album.cover_art.as_deref(), col::ART_RADIUS, &crate::art::album_cover(album));
            let x = art.right() + 10.0;
            // Only albums kept offline carry a mark, at the title's end.
            let mark = pin_mark(pin);
            let mark_w = if mark.is_some() { 18.0 } else { 0.0 };
            let w = row.line(ui, x, c.title.right() - col::INSET - x - mark_w, &album.name, FontId::proportional(13.0), t.text);
            if let Some(m) = mark {
                super::offline_page::mark(ui, pos2(x + w + 10.0, row.cy()), col::MARK, m);
            }
            let cell = |ui: &Ui, r: Rect, text: &str| {
                row.line(ui, r.left() + col::INSET, r.width() - 2.0 * col::INSET, text, FontId::proportional(13.0), t.muted);
            };
            cell(ui, c.artist, album.artist.as_deref().unwrap_or(""));
            if let Some(r) = c.source {
                let at = (r.left() + col::INSET, row.cy(), r.width() - 2.0 * col::INSET);
                super::source_look::source_cell(
                    ui,
                    at,
                    &self.album_sources(&album.id),
                    TrackRow::ground(t, false, row.pointer_on()),
                    FontId::proportional(13.0),
                    t.muted,
                );
            }
            cell(ui, c.year, &album.year.map(|y| y.to_string()).unwrap_or_default());
            cell(ui, c.genre, &album.all_genres().join(", "));
            row.right_text(ui, c.songs.right() - col::INSET, album.song_count.to_string(), FontId::proportional(12.0), t.muted);
            row.right_text(ui, c.length.right() - col::INSET, crate::fmt::clock(f64::from(album.duration_secs)), FontId::proportional(12.0), t.muted);
            let added = album.created.as_deref().map(crate::fmt::date).unwrap_or_default();
            let g = elide(ui, &added, FontId::proportional(12.0), t.muted, c.added.width() - 2.0 * col::INSET);
            ui.painter().galley(pos2(c.added.left() + col::INSET, row.cy() - g.size().y / 2.0), g, t.muted);
        }
        row.describe(false, || format!("{} by {}", album.name, album.artist.as_deref().unwrap_or("Unknown artist")));
        if row.resp.clicked() {
            self.actions.push(Action::Go(Route::Album(album.id.clone())));
        }
        let kept = pin.is_some();
        row.resp.context_menu(|ui| self.album_menu(ui, album, kept));
    }
}

struct Columns {
    title: Rect,
    artist: Rect,
    /// While the list mixes sources and there's room for it.
    source: Option<Rect>,
    year: Rect,
    genre: Rect,
    songs: Rect,
    length: Rect,
    added: Rect,
}

/// Title, artist and genre share what's left after the fixed columns. The
/// Source column (`sources`) comes after the artist, and is the first to
/// go when the shared columns would be left too little.
fn columns(row: Rect, sources: bool) -> Columns {
    let fixed = YEAR_W + SONGS_W + LENGTH_W + ADDED_W;
    if sources && row.width() - fixed - SOURCE_W >= SOURCE_MIN_FLEX {
        let [title, artist, source, year, genre, songs, length, added] = super::table::columns(
            row,
            [
                // The title keeps more of what's shared, which is less.
                Width::Share(0.46),
                Width::Share(0.3),
                Width::Fixed(SOURCE_W),
                Width::Fixed(YEAR_W),
                Width::Share(0.24),
                Width::Fixed(SONGS_W),
                Width::Fixed(LENGTH_W),
                Width::Fixed(ADDED_W),
            ],
        );
        return Columns { title, artist, source: Some(source), year, genre, songs, length, added };
    }
    let [title, artist, year, genre, songs, length, added] = super::table::columns(
        row,
        [Width::Share(0.4), Width::Share(0.32), Width::Fixed(YEAR_W), Width::Share(0.28), Width::Fixed(SONGS_W), Width::Fixed(LENGTH_W), Width::Fixed(ADDED_W)],
    );
    Columns { title, artist, source: None, year, genre, songs, length, added }
}
