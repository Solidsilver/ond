//! The queue column. Quiet by default: groups are an icon and a name, long
//! groups fold to a count, and each item shows its duration, swapping in the
//! time it will start when hovered. What happens at the end sits at the bottom.
//! The same column can show the playing track's lyrics (in the full window)
//! or its details instead (Up Next · History · Lyrics · Info).

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
        match self.ui.queue_ui.tab {
            QueueTab::Lyrics => return self.sidebar_lyrics(ui),
            // The Player already shows the song large right above.
            QueueTab::Info => return self.sidebar_info(ui, lyrics_tab),
            QueueTab::UpNext | QueueTab::History => {}
        }

        let footer_h = 40.0;
        let list_h = (ui.available_height() - footer_h).max(60.0);
        egui::ScrollArea::vertical().id_salt("queue").max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            match self.ui.queue_ui.tab {
                QueueTab::UpNext => self.up_next(ui, now, remaining),
                QueueTab::History | QueueTab::Lyrics | QueueTab::Info => self.history(ui),
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
                    self.actions.push(Action::SetEnd(end));
                }
                if !self.player.queue.upcoming.is_empty() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(8.0);
                        if super::widgets::link(ui, "Clear", FontId::proportional(12.0), t.muted).on_hover_text("Remove everything up next").clicked() {
                            self.actions.push(Action::ClearQueue);
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
        let starts = start_offsets(remaining, &self.player.queue.upcoming);
        let groups = self.player.queue.groups();
        let mut drawn = 0;
        for (gi, g) in groups.iter().enumerate() {
            let key = g.source.key();
            let foldable = g.len > FOLD_OVER && gi > 0;
            let open = *self.ui.queue_ui.open.get(&key).unwrap_or(&!foldable);
            // Group header: icon + name, and a count with a chevron when it folds.
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
            if ui.is_rect_visible(rect) {
                if resp.hovered() {
                    ui.painter().rect_filled(rect, 5.0, t.hover);
                }
                icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.left() + 14.0, rect.center().y), vec2(10.0, 10.0)), group_icon(&g.source), t.muted);
                let name = elide(ui, g.source.label(), FontId::new(11.5, semibold()), t.muted, rect.width() - 80.0);
                ui.painter().galley(pos2(rect.left() + 26.0, rect.center().y - name.size().y / 2.0), name, t.muted);
                let chevron = if open { Icon::ChevronDown } else { Icon::ChevronRight };
                icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.right() - 12.0, rect.center().y), vec2(8.0, 8.0)), chevron, t.muted);
                ui.painter().text(pos2(rect.right() - 22.0, rect.center().y), egui::Align2::RIGHT_CENTER, g.len.to_string(), FontId::proportional(11.0), t.muted);
            }
            let label = format!("{}, {} tracks, {}", g.source.label(), g.len, if open { "expanded" } else { "collapsed" });
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::DisclosureTriangle, true, &label));
            if resp.clicked() {
                self.ui.queue_ui.open.insert(key, !open);
            }
            if !open {
                continue;
            }
            for (i, start) in starts.iter().enumerate().skip(g.start).take(g.len) {
                if drawn == MAX_ROWS {
                    let more = self.player.queue.upcoming.len() - i;
                    ui.label(egui::RichText::new(format!("and {more} more")).size(11.0).color(t.muted));
                    return;
                }
                let item = self.player.queue.upcoming[i].clone();
                self.queue_row(ui, &item, Some(i), Some(clock_label(now, *start)));
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
        let items: Vec<Item> = self.player.queue.history.iter().rev().take(MAX_ROWS).cloned().collect();
        for item in &items {
            self.queue_row(ui, item, None, None);
        }
    }

    /// One queue item. `index` is its place in `upcoming` (None in history).
    /// Drag to reorder; double-click plays; right-click has the actions.
    pub(super) fn queue_row(&mut self, ui: &mut Ui, item: &Item, index: Option<usize>, starts_at: Option<String>) {
        let t = Tokens::of(ui);
        let sense = if index.is_some() { Sense::click_and_drag() } else { Sense::click() };
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), sense);
        let selected = self.ui.queue_ui.selected == Some(item.uid);
        if ui.is_rect_visible(rect) {
            // A selection the keyboard isn't on reads as a hover.
            let focused = selected && self.ui.focus == Focus::Queue;
            ui.painter().rect_filled(rect, 6.0, super::widgets::row_fill(&t, focused, resp.hovered() || selected));
            let art = Rect::from_min_size(pos2(rect.left() + 8.0, rect.center().y - 16.0), vec2(32.0, 32.0));
            let cover = self.data.cover_for(&item.track).map(str::to_owned);
            self.art.paint(ui, art, cover.as_deref(), 3.0);
            // Offline, songs that aren't downloaded will be skipped: say so on hover, dim them otherwise.
            let available = self.playable(&item.track);
            let right = match (&starts_at, resp.hovered()) {
                (_, true) if !available => "not downloaded".to_owned(),
                (Some(at), true) => format!("at {at}"),
                _ => item.track.duration_secs.map(|d| crate::fmt::clock(f64::from(d))).unwrap_or_default(),
            };
            let right_g = ui.painter().layout_no_wrap(right, FontId::proportional(11.0), t.muted);
            let text_w = rect.right() - 10.0 - right_g.size().x - 8.0 - (art.right() + 10.0);
            ui.painter().galley(pos2(rect.right() - 10.0 - right_g.size().x, rect.center().y - right_g.size().y / 2.0), right_g, t.muted);
            let x = art.right() + 10.0;
            let color = super::widgets::track_color(&t, false, available);
            let title = elide(ui, &item.track.title, FontId::proportional(12.5), color, text_w);
            ui.painter().galley(pos2(x, rect.center().y - title.size().y), title, color);
            let sub = elide(ui, item.track.artist.as_deref().unwrap_or(""), FontId::proportional(11.0), t.muted, text_w);
            ui.painter().galley(pos2(x, rect.center().y + 1.0), sub, t.muted);
        }
        let label = format!("{} by {}", item.track.title, item.track.artist.as_deref().unwrap_or("unknown artist"));
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::ListBoxOption, true, selected, &label));

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
                self.actions.push(Action::Move(*uid, to));
            }
        }
        if resp.clicked() {
            self.ui.queue_ui.selected = Some(item.uid);
            self.ui.focus = Focus::Queue;
        }
        if resp.double_clicked() {
            self.actions.push(if index.is_some() { Action::JumpTo(item.uid) } else { Action::Replay(item.uid) });
        }
        resp.context_menu(|ui| {
            if index.is_some() {
                if ui.button("Play now").clicked() {
                    self.actions.push(Action::JumpTo(item.uid));
                }
                if ui.button("Move to top").clicked() {
                    self.actions.push(Action::Move(item.uid, 0));
                }
                if ui.button("Remove").clicked() {
                    self.actions.push(Action::Remove(item.uid));
                }
            } else if ui.button("Play again").clicked() {
                self.actions.push(Action::Replay(item.uid));
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
            self.actions.push(Action::MoveBy(uid, -1));
        }
        if k.move_down {
            self.actions.push(Action::MoveBy(uid, 1));
        }
        if k.remove {
            self.actions.push(Action::Remove(uid));
        }
        if k.enter {
            self.actions.push(Action::JumpTo(uid));
        }
        if k.escape {
            self.ui.queue_ui.selected = None;
            self.ui.focus = Focus::Main;
        }
    }
}
