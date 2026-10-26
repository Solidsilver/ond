//! Songs: every synced track as a sortable table. Only visible rows are laid
//! out; the sorted order is cached until the tracks or the sort change.
//!
//! Rows act like the album page's: double-click (or ⏎) plays the track's
//! album from that track, ⌥⏎ plays next, ⇧⏎ adds, right-click has the rest.

use super::track_row::{TrackRow, col, length, title_font};
use super::view_ctx::ViewCtx;
use super::{Action, Focus, QueueAction};
use crate::queue::Source;
use crate::theme::{Tokens, semibold};
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};
use crate::memo::Memo;
use library::{Track, fold};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

const ROW_H: f32 = 32.0;
const TIME_W: f32 = 56.0;
const FORMAT_W: f32 = 96.0;

/// A Songs column, to sort by. Sorted by artist at first (not by title, the
/// first column): ties fall back to album order, so the list starts out as
/// each artist's albums, intact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Column {
    Title,
    #[default]
    Artist,
    Album,
    Time,
    Format,
}

#[derive(Default)]
pub struct SongsView {
    pub sort: Column,
    pub descending: bool,
    /// Folded sort keys, per (tracks, albums) version: built once, reused by
    /// every column.
    keys: Memo<(u64, u64), SortKeys>,
    /// Indices into `Data::tracks.all`, sorted; per versions and sort.
    order: Memo<(u64, u64, Column, bool), Vec<usize>>,
    pub selected: Option<usize>,
    /// The keys just moved the selection: bring it into view, once (every
    /// frame would pin it there and fight the scroll wheel).
    scroll_to_selected: bool,
}

/// Each track's text folded for sorting (case and accents ignored), computed
/// once so changing the sort column only compares.
pub struct SortKeys {
    title: Vec<String>,
    artist: Vec<String>,
    album: Vec<String>,
}

impl SortKeys {
    pub fn new(tracks: &[Track], album_names: &dyn Fn(&str) -> String) -> Self {
        let mut names: HashMap<&str, String> = HashMap::new();
        let album = tracks.iter().map(|t| names.entry(t.album_id.as_str()).or_insert_with(|| fold(&album_names(&t.album_id))).clone()).collect();
        Self {
            title: tracks.iter().map(|t| fold(&t.title)).collect(),
            artist: tracks.iter().map(|t| fold(t.artist.as_deref().unwrap_or(""))).collect(),
            album,
        }
    }
}

/// Sorted indices into `tracks` (folding the keys first).
#[cfg(test)]
pub fn sort_order(tracks: &[Track], album_names: &dyn Fn(&str) -> String, by: Column, descending: bool) -> Vec<usize> {
    sort_with(tracks, &SortKeys::new(tracks, album_names), by, descending)
}

/// Sorted indices into `tracks`. Ties fall back to album order (album, disc,
/// track), so sorting by artist lists each album intact.
pub fn sort_with(tracks: &[Track], keys: &SortKeys, by: Column, descending: bool) -> Vec<usize> {
    let album_order = |a: usize, b: usize| -> Ordering {
        let (x, y) = (&tracks[a], &tracks[b]);
        keys.album[a]
            .cmp(&keys.album[b])
            .then_with(|| x.album_id.cmp(&y.album_id))
            .then_with(|| x.disc.unwrap_or(1).cmp(&y.disc.unwrap_or(1)))
            .then_with(|| x.track.unwrap_or(0).cmp(&y.track.unwrap_or(0)))
    };
    let quality = |t: &Track| (t.bit_depth.unwrap_or(0), t.sample_rate.unwrap_or(0));
    let mut idx: Vec<usize> = (0..tracks.len()).collect();
    match by {
        Column::Title => idx.sort_by(|&a, &b| keys.title[a].cmp(&keys.title[b]).then_with(|| album_order(a, b))),
        Column::Artist => idx.sort_by(|&a, &b| keys.artist[a].cmp(&keys.artist[b]).then_with(|| album_order(a, b))),
        Column::Album => idx.sort_by(|&a, &b| album_order(a, b)),
        Column::Time => idx.sort_by(|&a, &b| tracks[a].duration_secs.unwrap_or(0).cmp(&tracks[b].duration_secs.unwrap_or(0)).then_with(|| album_order(a, b))),
        // Best quality first when descending: depth, then rate.
        Column::Format => idx.sort_by(|&a, &b| quality(&tracks[a]).cmp(&quality(&tracks[b])).then_with(|| album_order(a, b))),
    }
    if descending {
        idx.reverse();
    }
    idx
}

impl ViewCtx<'_> {
    pub(super) fn songs_order(&mut self) -> Arc<Vec<usize>> {
        let data = &self.data;
        let versions = (data.tracks_version, data.version);
        let keys = self.ui.songs.keys.get(versions, || {
            let names = |id: &str| data.album(id).map(|a| a.name.clone()).unwrap_or_default();
            SortKeys::new(&data.tracks.all, &names)
        });
        let (sort, descending) = (self.ui.songs.sort, self.ui.songs.descending);
        self.ui.songs.order.get((versions.0, versions.1, sort, descending), || sort_with(&data.tracks.all, &keys, sort, descending))
    }

    pub(super) fn songs_page(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let n = self.data.tracks.all.len();
        self.heading(ui, "Songs", (n > 0).then(|| n.to_string()));
        if n == 0 {
            let (title, body) = if self.sync.tracks {
                ("Syncing your tracks…", "Songs appear here as the library comes in.")
            } else {
                ("No songs yet", "They appear once the library has synced with your server.")
            };
            super::widgets::empty_state(ui, crate::icons::Icon::Note, title, body);
            return;
        }
        let order = self.songs_order();
        self.songs_keys(ui, &order);

        // Header.
        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
        let cols = columns(head);
        for (col, label, rect, right) in [
            (Column::Title, "Title", cols.title, false),
            (Column::Artist, "Artist", cols.artist, false),
            (Column::Album, "Album", cols.album, false),
            (Column::Time, "Time", cols.time, true),
            (Column::Format, "Format", cols.format, false),
        ] {
            let resp = ui.interact(rect, ui.scope_id().with(("songs-col", label)), Sense::click());
            let on = self.ui.songs.sort == col;
            let color = if on || resp.hovered() { t.text } else { t.muted };
            let font = FontId::new(11.0, semibold());
            let (anchor, x) = if right { (egui::Align2::RIGHT_CENTER, rect.right() - 10.0) } else { (egui::Align2::LEFT_CENTER, rect.left() + 10.0) };
            let text_rect = ui.painter().text(pos2(x, rect.center().y), anchor, label, font, color);
            if on {
                let cx = if right { text_rect.left() - 8.0 } else { text_rect.right() + 8.0 };
                sort_arrow(ui, pos2(cx, rect.center().y), self.ui.songs.descending, color);
            }
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, format!("Sort by {label}")));
            if resp.clicked() {
                if on {
                    self.ui.songs.descending = !self.ui.songs.descending;
                } else {
                    self.ui.songs.sort = col;
                    self.ui.songs.descending = false;
                }
            }
        }
        ui.painter().hline(head.x_range(), head.bottom(), egui::Stroke::new(1.0, t.hairline));

        // Borrowed from the library and player, not from `self`, so rows can
        // take `&mut self`.
        let (data, player) = (self.data, self.player);
        let tracks = &data.tracks.all;
        let current = player.queue.current.as_ref().map(|c| c.track.id.as_str());
        super::layout::into_gutter(ui, |ui| {
            // Rows sit flush: the scroll area places them by the same height.
            ui.spacing_mut().item_spacing.y = 0.0;
            egui::ScrollArea::vertical().id_salt("songs").auto_shrink(false).content_margin(super::layout::gutter()).show_rows(ui, ROW_H, order.len(), |ui, range| {
                // The keys moved the selection: bring its row into view, once. It
                // may be off screen (and not laid out), so place it by its index.
                if std::mem::take(&mut self.ui.songs.scroll_to_selected)
                    && let Some(sel) = self.ui.songs.selected
                {
                    let top = ui.cursor().top() + (sel as f32 - range.start as f32) * ROW_H;
                    ui.scroll_to_rect(Rect::from_min_size(pos2(ui.cursor().left(), top), vec2(ui.available_width(), ROW_H)), None);
                }
                for i in range {
                    self.songs_row(ui, &t, &tracks[order[i]], i, current);
                }
            })
        });
    }

    /// The `i`th row in the sorted order: a cell per column.
    fn songs_row(&mut self, ui: &mut Ui, t: &Tokens, tr: &Track, i: usize, current: Option<&str>) {
        let row = TrackRow::new(ui, ROW_H, Sense::click());
        let selected = self.ui.focus == Focus::Main && self.ui.songs.selected == Some(i);
        let playing = current == Some(tr.id.as_str());
        if ui.is_rect_visible(row.rect) {
            row.fill(ui, t, selected, row.pointer_on());
            let c = columns(row.rect);
            let color = super::widgets::track_color(t, playing, self.playable(tr));
            row.mark(ui, c.time.left() + col::INSET, self.downloads.snap.track(&tr.id));
            let mut title_x = c.title.left() + col::INSET;
            if playing {
                title_x = row.playing(ui, t, title_x);
            }
            // A cell's text, inset at both ends.
            let cell = |ui: &Ui, text: &str, x: f32, w: f32, font: FontId, color| {
                row.line(ui, x, w - 2.0 * col::INSET, text, font, color);
            };
            cell(ui, &tr.title, title_x, c.title.right() - title_x + col::INSET, title_font(13.0, playing), color);
            cell(ui, tr.artist.as_deref().unwrap_or(""), c.artist.left() + col::INSET, c.artist.width(), FontId::proportional(13.0), t.muted);
            let album = self.data.album(&tr.album_id).map_or("", |a| a.name.as_str());
            cell(ui, album, c.album.left() + col::INSET, c.album.width(), FontId::proportional(13.0), t.muted);
            row.right_text(ui, c.time.right() - col::INSET, length(tr), FontId::proportional(12.0), t.muted);
            cell(ui, &crate::fmt::quality(tr), c.format.left() + col::INSET, c.format.width(), FontId::proportional(11.0), t.muted);
        }
        row.describe(selected, || format!("{}, {}{}", tr.title, tr.artist.as_deref().unwrap_or(""), if playing { ", playing" } else { "" }));
        if row.resp.double_clicked() {
            self.play_from_album(tr, QueueAction::Play { start: 0 });
        } else if row.resp.clicked() {
            self.ui.songs.selected = Some(i);
            self.ui.focus = Focus::Main;
        }
        row.resp.context_menu(|ui| {
            let source = self.album_source(&tr.album_id);
            if ui.button("Play").clicked() {
                self.play_from_album(tr, QueueAction::Play { start: 0 });
            }
            self.track_menu(ui, tr, &source);
            if ui.button("Go to album").clicked() {
                self.actions.push(Action::Go(crate::nav::Route::Album(tr.album_id.clone())));
            }
        });
    }

    /// ↑/↓ select, ⏎ play, ⌥⏎ play next, ⇧⏎ add.
    pub(super) fn songs_keys(&mut self, ui: &Ui, order: &[usize]) {
        if order.is_empty() {
            return;
        }
        let Some(k) = self.list_keys(ui, Focus::Main, false, false) else { return };
        if k.up || k.down {
            self.ui.songs.selected = k.step(self.ui.songs.selected, order.len());
            self.ui.songs.scroll_to_selected = true;
        }
        if !(k.enter || k.play_next || k.add) {
            return;
        }
        let Some(sel) = self.ui.songs.selected.filter(|s| *s < order.len()) else { return };
        let data = self.data;
        let tr = &data.tracks.all[order[sel]];
        if k.enter {
            self.play_from_album(tr, QueueAction::Play { start: 0 });
        }
        if k.play_next {
            self.play_from_album(tr, QueueAction::PlayNext);
        }
        if k.add {
            self.play_from_album(tr, QueueAction::Add);
        }
    }
}

struct Columns {
    title: Rect,
    artist: Rect,
    album: Rect,
    time: Rect,
    format: Rect,
}

/// Title gets what's left after fixed time/format and proportional artist/album.
fn columns(row: Rect) -> Columns {
    let flex = (row.width() - TIME_W - FORMAT_W).max(0.0);
    let title_w = flex * 0.4;
    let other_w = flex * 0.3;
    let col = |x: f32, w: f32| Rect::from_min_size(pos2(x, row.top()), vec2(w, row.height()));
    let title = col(row.left(), title_w);
    let artist = col(title.right(), other_w);
    let album = col(artist.right(), other_w);
    let time = col(album.right(), TIME_W);
    let format = col(time.right(), FORMAT_W);
    Columns { title, artist, album, time, format }
}

fn sort_arrow(ui: &Ui, at: egui::Pos2, descending: bool, color: egui::Color32) {
    let (w, h) = (3.5, 2.5);
    let pts = if descending {
        vec![pos2(at.x - w, at.y - h), pos2(at.x + w, at.y - h), pos2(at.x, at.y + h)]
    } else {
        vec![pos2(at.x - w, at.y + h), pos2(at.x + w, at.y + h), pos2(at.x, at.y - h)]
    };
    ui.painter().add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
}

impl ViewCtx<'_> {
    pub(super) fn album_source(&self, album_id: &str) -> Source {
        Source::Album { id: album_id.to_owned(), name: self.data.album(album_id).map(|a| a.name.clone()).unwrap_or_default() }
    }

    /// Plays `tr`'s album starting at `tr` (`Play`), or queues just `tr`.
    pub(super) fn play_from_album(&mut self, tr: &Track, action: QueueAction) {
        let source = self.album_source(&tr.album_id);
        match action {
            QueueAction::Play { .. } | QueueAction::Shuffle => {
                let album = self.data.tracks.album(&tr.album_id).map(|s| s.to_vec()).unwrap_or_else(|| vec![tr.clone()]);
                let start = album.iter().position(|t| t.id == tr.id).unwrap_or(0);
                self.actions.push(Action::Tracks { tracks: album, source, action: QueueAction::Play { start } });
            }
            QueueAction::PlayNext | QueueAction::Add => self.actions.push(Action::Tracks { tracks: vec![tr.clone()], source, action }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str, album: &str, title: &str, artist: &str, n: u32, secs: u32, depth: u32) -> Track {
        Track {
            id: id.into(),
            album_id: album.into(),
            title: title.into(),
            artist: Some(artist.into()),
            artist_ref: None,
            track: Some(n),
            disc: Some(1),
            duration_secs: Some(secs),
            suffix: Some("flac".into()),
            content_type: None,
            size: None,
            bit_rate_kbps: None,
            bit_depth: Some(depth),
            sample_rate: Some(44_100),
            channels: Some(2),
            track_gain: None,
            album_gain: None,
            track_peak: None,
            album_peak: None,
            composer: None,
            contributors: Vec::new(),
            cover_art: None,
            artists: Vec::new(),
        }
    }

    fn titles(tracks: &[Track], order: &[usize]) -> Vec<String> {
        order.iter().map(|&i| tracks[i].title.clone()).collect()
    }

    #[test]
    fn sorts_by_column_with_album_order_as_tiebreak() {
        let tracks = vec![
            track("1", "b", "Zebra", "Miles Davis", 2, 300, 24),
            track("2", "a", "apple", "Bill Evans", 1, 200, 16),
            track("3", "b", "Mango", "Miles Davis", 1, 100, 16),
            track("4", "a", "Éclair", "bill evans", 2, 400, 24),
        ];
        let names = |id: &str| if id == "a" { "Portrait in Jazz".to_owned() } else { "Kind of Blue".to_owned() };
        // Artist: case-insensitive, then album, then track number.
        assert_eq!(titles(&tracks, &sort_order(&tracks, &names, Column::Artist, false)), ["apple", "Éclair", "Mango", "Zebra"]);
        // Title folds accents: "Éclair" sorts with E.
        assert_eq!(titles(&tracks, &sort_order(&tracks, &names, Column::Title, false)), ["apple", "Éclair", "Mango", "Zebra"]);
        assert_eq!(titles(&tracks, &sort_order(&tracks, &names, Column::Album, false)), ["Mango", "Zebra", "apple", "Éclair"]);
        assert_eq!(titles(&tracks, &sort_order(&tracks, &names, Column::Time, true)), ["Éclair", "Zebra", "apple", "Mango"]);
        let mut best = titles(&tracks, &sort_order(&tracks, &names, Column::Format, true))[..2].to_vec();
        best.sort();
        assert_eq!(best, ["Zebra", "Éclair"]);
    }
}
