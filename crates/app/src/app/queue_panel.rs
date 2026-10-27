//! The queue column. Quiet by default: groups are an icon and a name, long
//! groups fold to a count, and each item shows its duration, swapping in the
//! time it will start when hovered. What happens at the end sits at the bottom.
//! The same column can show the playing track's lyrics (in the full window)
//! or its details instead (Up Next · History · Lyrics · Info).

use super::QueueEdit;
use super::track_row::{TrackRow, col, length};
use super::view_ctx::ViewCtx;
use super::widgets::{choice, elide};
use super::{Action, Focus, QueueTab};
use crate::icons::{self, Icon};
use crate::queue::{EndAction, Item, Source, clock_label, end_offset, start_offsets};
use crate::theme::{Tokens, semibold};
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};

/// Groups longer than this start folded (except the first).
const FOLD_OVER: usize = 8;
/// Rows drawn at most; the rest are summarised.
const MAX_ROWS: usize = 300;
const ROW_H: f32 = 44.0;
/// A group heading: its height; the icon's centre and the name's start, in
/// from the left; the chevron's centre and the count's right edge, in from
/// the right; and the width the name leaves for the icon, count and chevron.
const GROUP_H: f32 = 26.0;
const GROUP_ICON_X: f32 = 14.0;
const GROUP_NAME_X: f32 = 26.0;
const GROUP_CHEVRON_FROM_RIGHT: f32 = 12.0;
const GROUP_COUNT_FROM_RIGHT: f32 = 22.0;
const GROUP_NAME_CLEAR: f32 = 80.0;

fn group_icon(s: &Source) -> Icon {
    match s {
        Source::Added => Icon::Person,
        Source::Album { .. } => Icon::Disc,
        Source::Playlist { .. } => Icon::List,
        Source::ArtistShuffle { .. } => Icon::Shuffle,
    }
}

impl ViewCtx<'_> {
    /// The column: Up Next or History, plus a Lyrics tab when `lyrics_tab`
    /// (the full window; the Player has its lyrics drawer instead).
    pub(super) fn queue_panel(&mut self, ui: &mut Ui, lyrics_tab: bool) {
        let t = Tokens::of(ui);
        if !lyrics_tab && self.ui.queue_ui.tab == QueueTab::Lyrics {
            self.ui.queue_ui.tab = QueueTab::UpNext;
        }
        let now = chrono::Local::now().time();
        let remaining = match (self.player.queue.current.as_ref(), self.duration_secs()) {
            (Some(_), Some(d)) => d - self.position_secs(),
            _ => 0.0,
        };
        // Up Next · History · Lyrics · Info; the Player has its lyrics drawer
        // instead of a Lyrics tab.
        let mut tabs = vec![QueueTab::UpNext, QueueTab::History];
        if lyrics_tab {
            tabs.push(QueueTab::Lyrics);
        }
        tabs.push(QueueTab::Info);
        let until = (self.player.queue.current.is_some() && !self.player.queue.upcoming.is_empty() && self.ui.queue_ui.tab == QueueTab::UpNext)
            .then(|| format!("until {}", clock_label(now, end_offset(remaining, &self.player.queue.upcoming))));
        let mut until_below = false;
        ui.horizontal(|ui| {
            choice(ui, &tabs, &mut self.ui.queue_ui.tab, None, |t| t.label().into());
            if let Some(until) = &until {
                // Beside the switch when there's room, else on its own line.
                let w = ui.painter().layout_no_wrap(until.clone(), FontId::proportional(11.0), t.muted).size().x;
                if ui.available_width() >= w + 8.0 {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(until).size(11.0).color(t.muted)).on_hover_text("When the queue runs out");
                    });
                } else {
                    until_below = true;
                }
            }
        });
        if let Some(until) = until.filter(|_| until_below) {
            ui.add_space(4.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(until).size(11.0).color(t.muted)).on_hover_text("When the queue runs out");
            });
        }
        ui.add_space(6.0);
        let history = match self.ui.queue_ui.tab {
            QueueTab::Lyrics => return self.sidebar_lyrics(ui),
            // The Player already shows the song large right above.
            QueueTab::Info => return self.sidebar_info(ui, lyrics_tab),
            QueueTab::UpNext => false,
            QueueTab::History => true,
        };

        let footer_h = 40.0;
        let list_h = (ui.available_height() - footer_h).max(60.0);
        egui::ScrollArea::vertical().id_salt("queue").max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if history {
                self.history(ui);
            } else {
                self.up_next(ui, now, remaining);
            }
        });
        self.queue_keys(ui);

        // Footer: what happens when the queue runs out.
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.horizontal(|ui| {
                ui.add_space(2.0);
                let label = |e: EndAction| match e {
                    EndAction::Stop => "Stop",
                    EndAction::RepeatQueue => "Repeat queue",
                };
                let mut end = self.player.queue.end;
                let button = super::widgets::menu_label(ui, &format!("Then: {}", label(end)), t.text)
                    .on_hover_text("What happens when the queue runs out");
                egui::Popup::menu(&button).show(|ui| {
                    ui.selectable_value(&mut end, EndAction::Stop, label(EndAction::Stop));
                    ui.selectable_value(&mut end, EndAction::RepeatQueue, label(EndAction::RepeatQueue));
                });
                if end != self.player.queue.end {
                    self.actions.push(Action::Queue(QueueEdit::SetEnd(end)));
                }
                if self.player.queue.can_clear() {
                    let hint = match self.player.queue.prefs.clear {
                        crate::queue::ClearWhat::Everything => "Remove everything up next",
                        crate::queue::ClearWhat::Added => "Remove the tracks you added",
                    };
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(8.0);
                        if super::widgets::link(ui, "Clear", FontId::proportional(12.0), t.muted).on_hover_text(hint).clicked() {
                            self.actions.push(Action::Queue(QueueEdit::Clear));
                        }
                    });
                }
            });
            ui.add_space(2.0);
            ui.separator();
        });
    }

    pub(super) fn up_next(&mut self, ui: &mut Ui, now: chrono::NaiveTime, remaining: f64) {
        let t = Tokens::of(ui);
        if self.player.queue.upcoming.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("Nothing up next").color(t.muted));
                ui.label(egui::RichText::new(crate::keys::shown("⌥⏎ play next · ⇧⏎ add to queue")).size(11.0).color(t.muted));
            });
            return;
        }
        // Borrowed from the player, not from `self`, so rows can take `&mut self`.
        let queue = &self.player.queue;
        let starts = start_offsets(remaining, &queue.upcoming);
        let split = queue.added_is_split();
        let mut drawn = 0;
        for (gi, g) in queue.groups().enumerate() {
            let source = g.source;
            let len = g.len;
            let key = source.key();
            let foldable = len > FOLD_OVER && gi > 0;
            let open = *self.ui.queue_ui.open.get(&key).unwrap_or(&!foldable);
            // Group header: icon + name, and a count with a chevron when it folds.
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), GROUP_H), Sense::click());
            if ui.is_rect_visible(rect) {
                if resp.hovered() {
                    ui.painter().rect_filled(rect, 5.0, t.hover);
                }
                icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.left() + GROUP_ICON_X, rect.center().y), vec2(10.0, 10.0)), group_icon(source), t.muted);
                let name = elide(ui, source.label(), FontId::new(11.5, semibold()), t.muted, rect.width() - GROUP_NAME_CLEAR);
                ui.painter().galley(pos2(rect.left() + GROUP_NAME_X, rect.center().y - name.size().y / 2.0), name, t.muted);
                let chevron = if open { Icon::ChevronDown } else { Icon::ChevronRight };
                icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.right() - GROUP_CHEVRON_FROM_RIGHT, rect.center().y), vec2(8.0, 8.0)), chevron, t.muted);
                ui.painter().text(pos2(rect.right() - GROUP_COUNT_FROM_RIGHT, rect.center().y), egui::Align2::RIGHT_CENTER, len.to_string(), FontId::proportional(11.0), t.muted);
            }
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::DisclosureTriangle, true, format!("{}, {len} tracks, {}", source.label(), if open { "expanded" } else { "collapsed" })));
            if resp.clicked() {
                self.ui.queue_ui.open.insert(key, !open);
            }
            if split && *source == Source::Added {
                resp.context_menu(|ui| {
                    if ui.button("Bring together").on_hover_text("Everything you added, in one group").clicked() {
                        self.actions.push(Action::Queue(QueueEdit::GatherAdded));
                    }
                });
            }
            if !open {
                continue;
            }
            for (i, start) in starts.iter().enumerate().skip(g.start).take(len) {
                if drawn == MAX_ROWS {
                    let more = queue.upcoming.len() - i;
                    ui.label(egui::RichText::new(format!("and {more} more")).size(11.0).color(t.muted));
                    return;
                }
                self.queue_row(ui, &t, &queue.upcoming[i], Some(i), Some((now, *start)));
                drawn += 1;
            }
            ui.add_space(4.0);
        }
    }

    pub(super) fn history(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        if self.player.queue.history.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| ui.label(egui::RichText::new("Nothing played yet").color(t.muted)));
            return;
        }
        let history = &self.player.queue.history;
        for item in history.iter().rev().take(MAX_ROWS) {
            self.queue_row(ui, &t, item, None, None);
        }
    }

    /// One queue item, in the list's colors `t`. `index` is its place in
    /// `upcoming` (None in history); `starts` is (now, seconds until it
    /// starts), shown on hover. Drag to reorder; double-click plays;
    /// right-click has the actions.
    pub(super) fn queue_row(&mut self, ui: &mut Ui, t: &Tokens, item: &Item, index: Option<usize>, starts: Option<(chrono::NaiveTime, f64)>) {
        let sense = if index.is_some() { Sense::click_and_drag() } else { Sense::click() };
        let row = TrackRow::new(ui, ROW_H, sense);
        let (rect, resp) = (row.rect, &row.resp);
        let selected = self.ui.queue_ui.selected == Some(item.uid);
        if ui.is_rect_visible(rect) {
            // A selection the keyboard isn't on reads as a hover.
            let focused = selected && self.ui.focus == Focus::Queue;
            row.fill(ui, t, focused, resp.hovered() || selected);
            let art = Rect::from_min_size(pos2(rect.left() + col::ART_INSET, row.cy() - col::ART / 2.0), vec2(col::ART, col::ART));
            let data = self.data;
            self.art.paint(ui, art, data.cover_for(&item.track), col::ART_RADIUS);
            // Offline, songs that aren't downloaded will be skipped: say so on hover, dim them otherwise.
            let available = self.playable(&item.track);
            let right = match (starts, resp.hovered()) {
                (_, true) if !available => "not downloaded".to_owned(),
                (Some((now, offset)), true) => format!("at {}", clock_label(now, offset)),
                _ => length(&item.track),
            };
            // The length (or start time) at the right end; the text takes the rest.
            let right_g = ui.painter().layout_no_wrap(right, FontId::proportional(11.0), t.muted);
            let x = art.right() + col::INSET;
            let text_w = rect.right() - col::INSET - right_g.size().x - col::RIGHT_GAP - x;
            ui.painter().galley(pos2(rect.right() - col::INSET - right_g.size().x, row.cy() - right_g.size().y / 2.0), right_g, t.muted);
            let color = super::widgets::track_color(t, false, available);
            row.two_lines(ui, x, text_w, (&item.track.title, FontId::proportional(12.5), color), (item.track.artist.as_deref().unwrap_or(""), FontId::proportional(11.0), t.muted));
        }
        row.describe(selected, || format!("{} by {}", item.track.title, item.track.artist.as_deref().unwrap_or("unknown artist")));

        if let Some(index) = index {
            // Reordering by drag and drop.
            if resp.drag_started() {
                resp.dnd_set_drag_payload(item.uid);
            }
            if let (Some(_), Some(p)) = (resp.dnd_hover_payload::<u64>(), ui.ctx().pointer_interact_pos()) {
                let below = p.y > rect.center().y;
                let y = if below { rect.bottom() } else { rect.top() };
                ui.painter().line_segment([pos2(rect.left() + 8.0, y), pos2(rect.right() - 8.0, y)], egui::Stroke::new(2.0, t.accent));
            }
            if let (Some(uid), Some(p)) = (resp.dnd_release_payload::<u64>(), ui.ctx().pointer_interact_pos()) {
                let below = p.y > rect.center().y;
                let from = self.player.queue.position(*uid).unwrap_or(usize::MAX);
                let mut to = index + usize::from(below);
                if from < to {
                    to -= 1;
                }
                self.actions.push(Action::Queue(QueueEdit::Move(*uid, to)));
            }
        }
        if resp.clicked() {
            self.ui.queue_ui.selected = Some(item.uid);
            self.ui.focus = Focus::Queue;
        }
        if resp.double_clicked() {
            self.actions.push(if index.is_some() { Action::Queue(QueueEdit::JumpTo(item.uid)) } else { Action::Queue(QueueEdit::Replay(item.uid)) });
        }
        resp.context_menu(|ui| {
            if index.is_some() {
                if ui.button("Play now").clicked() {
                    self.actions.push(Action::Queue(QueueEdit::JumpTo(item.uid)));
                }
                if ui.button("Move to top").clicked() {
                    self.actions.push(Action::Queue(QueueEdit::Move(item.uid, 0)));
                }
                if ui.button("Remove").clicked() {
                    self.actions.push(Action::Queue(QueueEdit::Remove(item.uid)));
                }
            } else if ui.button("Play again").clicked() {
                self.actions.push(Action::Queue(QueueEdit::Replay(item.uid)));
            }
            ui.separator();
            if ui.button("Go to album").clicked() {
                self.actions.push(Action::Go(crate::nav::Route::Album(item.track.album_id.clone())));
            }
        });
    }

    /// With a queue item selected: ↑/↓ select, ⌥↑/⌥↓ move it, ⌫ remove, ⏎ play.
    pub(super) fn queue_keys(&mut self, ui: &Ui) {
        let Some(uid) = self.ui.queue_ui.selected else { return };
        let Some(at) = self.player.queue.position(uid) else { return };
        let Some(k) = self.list_keys(ui, Focus::Queue, true, true) else { return };
        if let Some(to) = k.step(Some(at), self.player.queue.upcoming.len()) {
            self.ui.queue_ui.selected = Some(self.player.queue.upcoming[to].uid);
        }
        if k.move_up {
            self.actions.push(Action::Queue(QueueEdit::MoveBy(uid, -1)));
        }
        if k.move_down {
            self.actions.push(Action::Queue(QueueEdit::MoveBy(uid, 1)));
        }
        if k.remove {
            self.actions.push(Action::Queue(QueueEdit::Remove(uid)));
        }
        if k.enter {
            self.actions.push(Action::Queue(QueueEdit::JumpTo(uid)));
        }
        if k.escape {
            self.ui.queue_ui.selected = None;
            self.ui.focus = Focus::Main;
        }
    }
}
