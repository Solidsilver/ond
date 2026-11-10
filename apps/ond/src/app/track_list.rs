//! The track list the album, playlist and Favorites pages share: numbered
//! rows (with disc headings) or rows with the artist under each title, the
//! hover actions, dragging to reorder or onto a playlist, the track menu, and
//! the keys every list answers to.

use super::track_row::{TrackRow, col, length, title_font};
use super::view_ctx::ViewCtx;
use super::{Action, DragTracks, Focus, Keep, Overlay, PlaylistAction, QueueAction};
use crate::favorites::StarKind;
use crate::icons::{self, Icon};
use crate::nav::Route;
use crate::queue::Source;
use crate::theme::{Tokens, semibold};
use crate::widgets::link;
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};
use library::Track;

#[derive(Clone)]
pub(super) struct ListOpts {
    pub numbers: bool,
    pub artist: bool,
    /// The user's own playlist being shown: rows can be removed and dragged
    /// to reorder.
    pub playlist: Option<String>,
}

/// A track row's height: a title alone, or a title over its artist.
const ROW_H: f32 = 38.0;
const ROW_H_ARTIST: f32 = 44.0;
/// Room above a disc heading after the first.
const DISC_GAP: f32 = 12.0;

impl ListOpts {
    pub fn row_h(&self) -> f32 {
        if self.artist { ROW_H_ARTIST } else { ROW_H }
    }
}

/// What every row of one track list shares, worked out once per frame.
struct List<'l> {
    t: Tokens,
    tracks: &'l [Track],
    source: &'l Source,
    opts: &'l ListOpts,
    /// The playing track's id.
    current: Option<&'l str>,
}

impl<'l> List<'l> {
    fn new(ui: &Ui, player: &'l crate::player::Player, tracks: &'l [Track], source: &'l Source, opts: &'l ListOpts) -> Self {
        Self { t: Tokens::of(ui), tracks, source, opts, current: player.queue.current.as_ref().map(|c| c.track.id.as_str()) }
    }

    /// The line where a reorder drag would land, at `drop_at`'s y.
    fn drop_line(&self, ui: &Ui, drop_at: Option<(usize, f32)>) {
        if let Some((_, y)) = drop_at {
            let r = ui.min_rect();
            ui.painter().line_segment([pos2(r.left() + 4.0, y), pos2(r.right() - 4.0, y)], egui::Stroke::new(2.0, self.t.accent));
        }
    }
}

impl ViewCtx<'_> {
    /// The Add to Playlist submenu: New Playlist…, then recently used, then the rest.
    pub(super) fn add_to_playlist_menu(&mut self, ui: &mut Ui, tracks: &[Track]) {
        ui.menu_button("Add to playlist", |ui| {
            if ui.button("New playlist…").clicked() {
                self.actions.push(Action::Overlay(Overlay::new_playlist(tracks.to_vec())));
            }
            let mine: Vec<subsonic::Playlist> = self.data.playlists.iter().filter(|p| self.owns(p)).cloned().collect();
            let recent: Vec<&subsonic::Playlist> = self.ui.recent_playlists.iter().filter_map(|id| mine.iter().find(|p| &p.id == id)).collect();
            let mut rest: Vec<&subsonic::Playlist> = mine.iter().filter(|p| !self.ui.recent_playlists.contains(&p.id)).collect();
            rest.sort_by_cached_key(|p| p.name.to_lowercase());
            for group in [recent, rest] {
                if group.is_empty() {
                    continue;
                }
                ui.separator();
                for p in group {
                    if ui.button(&p.name).clicked() {
                        self.actions.push(Action::PlaylistEdit(PlaylistAction::Add { id: p.id.clone(), tracks: tracks.to_vec() }));
                    }
                }
            }
        });
    }

    /// Rows of tracks, all laid out (the album page: short, with disc
    /// headings, inside the page's own scroll area). Click selects,
    /// double-click (or ⏎) plays from there; ⌥⏎ plays next, ⇧⏎ adds to the
    /// queue. Hovering a row shows its composer and the queue actions; a
    /// loved track keeps its heart. Rows drag onto a sidebar playlist. No
    /// wider than [`LIST_W`](shell::kit::LIST_W), from the left.
    pub(super) fn track_list(&mut self, ui: &mut Ui, tracks: &[Track], source: &Source, opts: ListOpts) {
        let w = ui.available_width().min(shell::kit::LIST_W);
        ui.allocate_ui_with_layout(vec2(w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_max_width(w);
            self.track_list_in(ui, tracks, source, opts);
        });
    }

    fn track_list_in(&mut self, ui: &mut Ui, tracks: &[Track], source: &Source, opts: ListOpts) {
        let moved = self.track_list_keys(ui, tracks, source, opts.playlist.as_deref());
        let list = List::new(ui, self.player, tracks, source, &opts);
        let discs = if opts.numbers { tracks.iter().filter_map(|t| t.disc).max().unwrap_or(1) } else { 1 };
        let mut last_disc = None;
        let mut drop_at = None;
        ui.spacing_mut().item_spacing.y = 0.0;
        for (i, tr) in tracks.iter().enumerate() {
            if discs > 1 && tr.disc != last_disc {
                last_disc = tr.disc;
                ui.add_space(if i == 0 { 0.0 } else { DISC_GAP });
                ui.label(egui::RichText::new(format!("Disc {}", tr.disc.unwrap_or(1))).font(FontId::new(12.0, semibold())).color(list.t.muted));
                ui.add_space(4.0);
            }
            let row = TrackRow::new(ui, opts.row_h(), Sense::click_and_drag());
            if self.take_reveal(tr, i) {
                ui.scroll_to_rect(row.rect, Some(egui::Align::Center));
            } else if moved && self.ui.track_sel == Some(i) {
                ui.scroll_to_rect(row.rect, None);
            }
            // Off screen: the space is kept, nothing else to do.
            if ui.is_rect_visible(row.rect) {
                drop_at = self.track_row(ui, &list, i, &row).or(drop_at);
            }
        }
        list.drop_line(ui, drop_at);
    }

    /// [`track_list`](Self::track_list) in its own scroll area, laying out
    /// only the rows on screen (a playlist or Favorites can run to thousands):
    /// every row is the same height, and none has a disc heading. As wide as
    /// [`track_list`](Self::track_list)'s. Within the
    /// user's own playlist, rows also drag to a new place in it.
    pub(super) fn track_list_rows(&mut self, ui: &mut Ui, scroll: egui::ScrollArea, tracks: &[Track], source: &Source, opts: ListOpts) {
        let moved = self.track_list_keys(ui, tracks, source, opts.playlist.as_deref());
        // A row to bring into view: the one asked to be revealed (centred), or
        // the selection the keys just moved. It may be off screen, and so not
        // laid out: it's placed by its index.
        let revealed = self.ui.reveal.as_deref().and_then(|id| tracks.iter().position(|t| t.id == id));
        if let Some(i) = revealed {
            self.take_reveal(&tracks[i], i);
        }
        let bring = revealed.map(|i| (i, Some(egui::Align::Center))).or_else(|| self.ui.track_sel.filter(|_| moved).map(|i| (i, None)));
        let list = List::new(ui, self.player, tracks, source, &opts);
        let h = opts.row_h();
        // Rows sit flush: the scroll area places them by the same height.
        ui.spacing_mut().item_spacing.y = 0.0;
        scroll.show_rows(ui, h, tracks.len(), |ui, range| {
            // The scroll bar stays at the window's edge; the rows stop short.
            ui.set_max_width(ui.available_width().min(shell::kit::LIST_W));
            if let Some((i, align)) = bring {
                let top = ui.cursor().top() + (i as f32 - range.start as f32) * h;
                ui.scroll_to_rect(Rect::from_min_size(pos2(ui.cursor().left(), top), vec2(ui.available_width(), h)), align);
            }
            let mut drop_at = None;
            for i in range {
                let row = TrackRow::new(ui, h, Sense::click_and_drag());
                drop_at = self.track_row(ui, &list, i, &row).or(drop_at);
            }
            list.drop_line(ui, drop_at);
        });
    }

    /// When `tr` (at `i`) is the track asked to be revealed: selects it and
    /// says so, once (the caller scrolls to it).
    fn take_reveal(&mut self, tr: &Track, i: usize) -> bool {
        if self.ui.reveal.as_deref() != Some(tr.id.as_str()) {
            return false;
        }
        self.ui.reveal = None;
        self.ui.track_sel = Some(i);
        self.ui.focus = Focus::Main;
        true
    }

    /// The `i`th row of a track list. Returns where a reorder drag over it
    /// would land: (index to insert at, y of the line).
    fn track_row(&mut self, ui: &mut Ui, list: &List, i: usize, row: &TrackRow) -> Option<(usize, f32)> {
        let tr = &list.tracks[i];
        let playing = list.current == Some(tr.id.as_str());
        let selected = self.ui.focus == Focus::Main && self.ui.track_sel == Some(i);
        let loved = self.favorites.is_on(StarKind::Song, &tr.id);
        let drop_at = self.track_row_drag(ui, list, i, row);
        if ui.is_rect_visible(row.rect) {
            self.paint_track_row(ui, list, i, row, playing, selected, loved);
        }
        row.describe(selected, || format!("{}{}{}", tr.title, if loved { ", loved" } else { "" }, if playing { ", playing" } else { "" }));
        let resp = &row.resp;
        if resp.double_clicked() {
            self.actions.push(Action::Tracks { tracks: list.tracks.to_vec(), source: list.source.clone(), action: QueueAction::Play { start: i } });
        } else if resp.clicked() {
            self.ui.track_sel = Some(i);
            self.ui.focus = Focus::Main;
        }
        resp.context_menu(|ui| self.track_row_menu(ui, list, i));
        drop_at
    }

    /// Dragging the `i`th row: its track to a playlist; within one's own
    /// playlist, also to a new place in it. Returns where a reorder dropped
    /// on this row would land.
    fn track_row_drag(&mut self, ui: &Ui, list: &List, i: usize, row: &TrackRow) -> Option<(usize, f32)> {
        let (resp, rect, playlist) = (&row.resp, row.rect, &list.opts.playlist);
        // Built only when the drag starts, the one frame egui takes it: a
        // copy per row per frame adds up on long lists.
        if resp.drag_started() {
            resp.dnd_set_drag_payload(DragTracks { tracks: vec![list.tracks[i].clone()], from: playlist.clone().map(|p| (p, i)) });
        }
        let pid = playlist.as_ref()?;
        let mut drop_at = None;
        if let Some(drag) = resp.dnd_hover_payload::<DragTracks>()
            && drag.from.as_ref().is_some_and(|(p, _)| p == pid)
            && let Some(p) = ui.ctx().pointer_hover_pos()
        {
            let below = p.y > rect.center().y;
            drop_at = Some((if below { i + 1 } else { i }, if below { rect.bottom() } else { rect.top() }));
        }
        if let Some(drag) = resp.dnd_release_payload::<DragTracks>()
            && let Some((p, from)) = &drag.from
            && p == pid
            && let Some(pos) = ui.ctx().pointer_interact_pos()
        {
            let mut to = if pos.y > rect.center().y { i + 1 } else { i };
            if to > *from {
                to -= 1;
            }
            self.actions.push(Action::PlaylistEdit(PlaylistAction::Move { id: pid.clone(), from: *from, to }));
        }
        drop_at
    }

    /// Draws the `i`th row: number or playing mark, title (and artist), and at
    /// the right end the length, the download mark and the heart; on hover,
    /// the composer (or why it can't play) and the actions.
    #[allow(clippy::too_many_arguments)]
    fn paint_track_row(&mut self, ui: &mut Ui, list: &List, i: usize, row: &TrackRow, playing: bool, selected: bool, loved: bool) {
        let (t, opts, tr) = (&list.t, list.opts, &list.tracks[i]);
        let (rect, cy) = (row.rect, row.cy());
        let hovered = row.pointer_on();
        row.fill(ui, t, selected, hovered);
        let mut x = rect.left() + col::INSET;
        // Offline, what isn't downloaded is dimmed rather than hidden.
        let available = self.playable(tr);
        let color = crate::widgets::track_color(t, playing, available);
        row.mark(ui, rect.right() - col::MARK_FROM_RIGHT, self.downloads.snap.track(&tr.id));
        if opts.numbers {
            if playing {
                let side = col::NUMBER_PLAYING;
                icons::paint(ui.painter(), Rect::from_center_size(pos2(x + col::NUMBER_PLAYING_X, cy), vec2(side, side)), Icon::Play, t.accent);
            } else {
                let n = tr.track.map(|n| n.to_string()).unwrap_or_default();
                row.right_text(ui, x + col::NUMBER_RIGHT, n, FontId::proportional(12.0), t.muted);
            }
            x += col::NUMBER_W;
        } else {
            // A gutter of its own, so titles line up whether or not one is playing.
            if playing {
                row.playing(ui, t, x);
            }
            x += col::PLAYING_W;
        }
        row.right_text(ui, rect.right() - col::INSET, length(tr), FontId::proportional(12.0), t.muted);
        // The heart sits just left of the duration: always when loved,
        // on hover otherwise.
        let heart = Rect::from_center_size(pos2(rect.right() - col::HEART_FROM_RIGHT, cy), vec2(col::HEART_BOX, col::HEART_BOX));
        if loved || hovered {
            let label = if loved { "Unlove" } else { "Love" };
            let hr = crate::widgets::hover_heart(ui, heart, ui.scope_id().with(("heart", &tr.id, i)), loved, true, 12.0, label);
            if hr.on_hover_text(label).clicked() {
                self.actions.push(Action::ToggleStar { kind: StarKind::Song, id: tr.id.clone(), track: Some(Box::new(tr.clone())), name: None });
            }
        }
        let actions_w = if hovered { col::ACTIONS_W } else { 0.0 };
        let text_w = rect.right() - col::TEXT_FROM_RIGHT - actions_w - x;
        let title_font = title_font(13.0, playing);
        if opts.artist {
            row.two_lines(ui, x, text_w, (&tr.title, title_font, color), (tr.artist.as_deref().unwrap_or(""), FontId::proportional(12.0), t.muted));
        } else {
            let title_w = row.line(ui, x, text_w, &tr.title, title_font, color);
            if hovered {
                self.track_row_aside(ui, t, tr, row, x + title_w + col::GAP, text_w - title_w - col::GAP, available);
            }
        }
        if hovered {
            let r = Rect::from_min_max(pos2(rect.right() - col::TEXT_FROM_RIGHT - actions_w, rect.top()), pos2(rect.right() - col::ACTIONS_END, rect.bottom()));
            self.track_row_actions(ui, t, list, i, r);
        }
    }

    /// After a hovered title, in `width` from `x`: "Not downloaded" when it
    /// can't play now, else its composer as a link.
    #[allow(clippy::too_many_arguments)]
    fn track_row_aside(&mut self, ui: &mut Ui, t: &Tokens, tr: &Track, row: &TrackRow, x: f32, width: f32, available: bool) {
        let cy = row.cy();
        if !available {
            let g = ui.painter().layout_no_wrap("Not downloaded".into(), FontId::proportional(12.0), t.muted);
            if g.size().x < width {
                ui.painter().galley(pos2(x, cy - g.size().y / 2.0), g, t.muted);
            }
            return;
        }
        let Some(composer) = composer_of(tr) else { return };
        let at = Rect::from_min_size(pos2(x, cy - 8.0), vec2(width.max(0.0), 16.0));
        // Too little room for "by" and a name worth reading.
        if at.width() <= 40.0 {
            return;
        }
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(at).layout(egui::Layout::left_to_right(egui::Align::Center)));
        child.spacing_mut().item_spacing.x = 0.0;
        child.label(egui::RichText::new("by ").size(12.0).color(t.muted));
        if link(&mut child, &composer, FontId::proportional(12.0), t.muted).clicked() {
            let id = tr.contributors.iter().find(|c| c.name == composer).and_then(|c| c.artist_id.clone());
            self.actions.push(Action::Go(Route::Artist { name: composer.clone(), id }));
        }
    }

    /// A hovered row's buttons, right to left in `r`: ⋯ (the row's menu) and
    /// Play Next.
    fn track_row_actions(&mut self, ui: &mut Ui, t: &Tokens, list: &List, i: usize, r: Rect) {
        let tr = &list.tracks[i];
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::right_to_left(egui::Align::Center)));
        child.spacing_mut().item_spacing.x = 4.0;
        let more = icons::sized_button(&mut child, Icon::More, "More", vec2(24.0, 22.0), 11.0, Some(t.muted));
        egui::Popup::menu(&more).show(|ui| self.track_row_menu(ui, list, i));
        if child.add(egui::Button::new(egui::RichText::new("Play next").size(11.0)).corner_radius(11.0).stroke(egui::Stroke::new(1.0, t.hairline))).clicked() {
            self.actions.push(Action::Tracks { tracks: vec![tr.clone()], source: list.source.clone(), action: QueueAction::PlayNext });
        }
    }

    /// [`track_menu`](Self::track_menu), plus Remove from Playlist in the user's own.
    fn track_row_menu(&mut self, ui: &mut Ui, list: &List, index: usize) {
        let tr = &list.tracks[index];
        if ui.button("Play").clicked() {
            self.actions.push(Action::Tracks { tracks: list.tracks.to_vec(), source: list.source.clone(), action: QueueAction::Play { start: index } });
        }
        self.track_menu(ui, tr, list.source);
        if let Some(pid) = list.opts.playlist.as_deref() {
            ui.separator();
            if ui.button("Remove from playlist").clicked() {
                self.actions.push(Action::PlaylistEdit(PlaylistAction::Remove { id: pid.to_owned(), index }));
            }
        }
    }

    pub(super) fn track_menu(&mut self, ui: &mut Ui, tr: &Track, source: &Source) {
        if ui.button("Play next").clicked() {
            self.actions.push(Action::Tracks { tracks: vec![tr.clone()], source: source.clone(), action: QueueAction::PlayNext });
        }
        if ui.button("Add to queue").clicked() {
            self.actions.push(Action::Tracks { tracks: vec![tr.clone()], source: source.clone(), action: QueueAction::Add });
        }
        ui.separator();
        let loved = self.favorites.is_on(StarKind::Song, &tr.id);
        if ui.button(if loved { "Unlove" } else { "Love" }).clicked() {
            self.actions.push(Action::ToggleStar { kind: StarKind::Song, id: tr.id.clone(), track: Some(Box::new(tr.clone())), name: None });
        }
        ui.menu_button("Rating", |ui| {
            let current = self.favorites.rating(&tr.id);
            for r in 1..=5u8 {
                if crate::widgets::rating_choice(ui, r, current == r).clicked() {
                    self.actions.push(Action::Rate(tr.id.clone(), r));
                    ui.close();
                }
            }
            ui.separator();
            if ui.add_enabled(current > 0, egui::Button::new("Clear rating")).clicked() {
                self.actions.push(Action::Rate(tr.id.clone(), 0));
            }
        });
        self.add_to_playlist_menu(ui, std::slice::from_ref(tr));
        if let Some(artist) = &tr.artist {
            ui.separator();
            if ui.button(format!("Go to {artist}")).clicked()
                && let Some(route) = Route::artist_of(tr)
            {
                self.actions.push(Action::Go(route));
            }
        }
        if !matches!(source, Source::Album { .. }) && ui.button("Go to album").clicked() {
            self.actions.push(Action::Go(Route::Album(tr.album_id.clone())));
        }
        // Kept on its own: can be removed on its own. Kept with an album or
        // playlist: that's where it's removed.
        let alone = self.downloads.snap.pin(&offline::Pin::Track(tr.id.clone())).is_some();
        if alone || !self.downloads.snap.is_done(&tr.id) {
            ui.separator();
            super::browse::offline_item(ui, alone, None, || Keep::Tracks(vec![tr.clone()]), self.actions);
        }
    }

    /// Whether a list may take keys this frame: no palette or sheet is open
    /// over it and no text field is being typed into.
    pub(super) fn list_keys_free(&self, ui: &Ui) -> bool {
        self.ui.palette.is_none() && self.ui.overlay.is_none() && !ui.ctx().memory(|m| m.focused().is_some())
    }

    /// ⇧⌘D, when nothing else has the keyboard.
    pub(super) fn download_key(&self, ui: &Ui) -> bool {
        if !self.list_keys_free(ui) {
            return false;
        }
        ui.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::D)))
    }

    /// The list keys pressed this frame, consumed, when the list on `focus`
    /// may take them (see [`list_keys_free`](Self::list_keys_free)); `None`
    /// otherwise. `edit` also takes ⌥↑/⌥↓ and ⌫ (lists the user can change);
    /// `escape` takes Esc.
    pub(super) fn list_keys(&self, ui: &Ui, focus: Focus, edit: bool, escape: bool) -> Option<ListKeys> {
        if self.ui.focus != focus || !self.list_keys_free(ui) {
            return None;
        }
        use egui::{Key, Modifiers};
        Some(ui.input_mut(|i| {
            let mut k = ListKeys {
                up: i.consume_key(Modifiers::NONE, Key::ArrowUp),
                down: i.consume_key(Modifiers::NONE, Key::ArrowDown),
                enter: i.consume_key(Modifiers::NONE, Key::Enter),
                play_next: i.consume_key(Modifiers::ALT, Key::Enter),
                add: i.consume_key(Modifiers::SHIFT, Key::Enter),
                ..ListKeys::default()
            };
            if edit {
                k.move_up = i.consume_key(Modifiers::ALT, Key::ArrowUp);
                k.move_down = i.consume_key(Modifiers::ALT, Key::ArrowDown);
                k.remove = i.consume_key(Modifiers::NONE, Key::Backspace) | i.consume_key(Modifiers::NONE, Key::Delete);
            }
            if escape {
                k.escape = i.consume_key(Modifiers::NONE, Key::Escape);
            }
            k
        }))
    }

    /// ↑/↓ select, ⏎ play from the selection, ⌥⏎ play next, ⇧⏎ add. In the
    /// user's own playlist, ⌥↑/⌥↓ move the selected song and ⌫ removes it.
    /// Returns whether ↑/↓ moved the selection (to bring it into view).
    pub(super) fn track_list_keys(&mut self, ui: &Ui, tracks: &[Track], source: &Source, playlist: Option<&str>) -> bool {
        if tracks.is_empty() {
            return false;
        }
        let Some(k) = self.list_keys(ui, Focus::Main, playlist.is_some(), false) else { return false };
        let last = tracks.len() - 1;
        self.ui.track_sel = k.step(self.ui.track_sel, tracks.len());
        let moved = k.up || k.down;
        let Some(sel) = self.ui.track_sel.filter(|s| *s <= last) else { return moved };
        if let Some(pid) = playlist {
            if k.move_up && sel > 0 {
                self.actions.push(Action::PlaylistEdit(PlaylistAction::Move { id: pid.to_owned(), from: sel, to: sel - 1 }));
            }
            if k.move_down && sel < last {
                self.actions.push(Action::PlaylistEdit(PlaylistAction::Move { id: pid.to_owned(), from: sel, to: sel + 1 }));
            }
            if k.remove {
                self.actions.push(Action::PlaylistEdit(PlaylistAction::Remove { id: pid.to_owned(), index: sel }));
            }
        }
        if k.enter {
            self.actions.push(Action::Tracks { tracks: tracks.to_vec(), source: source.clone(), action: QueueAction::Play { start: sel } });
        }
        if k.play_next {
            self.actions.push(Action::Tracks { tracks: vec![tracks[sel].clone()], source: source.clone(), action: QueueAction::PlayNext });
        }
        if k.add {
            self.actions.push(Action::Tracks { tracks: vec![tracks[sel].clone()], source: source.clone(), action: QueueAction::Add });
        }
        moved
    }
}

/// The keys a list reacts to, as read (and consumed) by `list_keys`.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct ListKeys {
    pub up: bool,
    pub down: bool,
    pub enter: bool,
    pub play_next: bool,
    pub add: bool,
    pub move_up: bool,
    pub move_down: bool,
    pub remove: bool,
    pub escape: bool,
}

impl ListKeys {
    /// The selection after ↑/↓ in a list of `len` rows: the first row when
    /// nothing was selected, clamped at both ends.
    pub fn step(&self, sel: Option<usize>, len: usize) -> Option<usize> {
        let last = len.checked_sub(1)?;
        let mut sel = sel;
        if self.down {
            sel = Some(sel.map_or(0, |s| (s + 1).min(last)));
        }
        if self.up {
            sel = Some(sel.map_or(0, |s| s.saturating_sub(1)));
        }
        sel
    }
}

fn composer_of(t: &Track) -> Option<String> {
    t.composer.clone().or_else(|| t.contributors.iter().find(|c| c.role.eq_ignore_ascii_case("composer")).map(|c| c.name.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_keys_step_and_clamp() {
        let down = ListKeys { down: true, ..ListKeys::default() };
        let up = ListKeys { up: true, ..ListKeys::default() };
        assert_eq!(down.step(None, 3), Some(0), "the first press selects the first row");
        assert_eq!(down.step(Some(2), 3), Some(2), "stops at the end");
        assert_eq!(up.step(Some(0), 3), Some(0), "stops at the top");
        assert_eq!(up.step(Some(2), 3), Some(1));
        assert_eq!(ListKeys::default().step(Some(1), 3), Some(1));
        assert_eq!(down.step(None, 0), None, "an empty list has nothing to select");
    }
}
