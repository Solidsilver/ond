//! ⌘K: search the library and act on a result without leaving the keyboard.
//! ⏎ opens · ⌘⏎ plays · ⌥⏎ plays next · ⇧⏎ adds to the queue.

use super::view_ctx::ViewCtx;
use super::widgets::elide;
use super::{Action, QueueAction};
use crate::icons::{self, Icon};
use crate::nav::Route;
use crate::search::{Hit, Results, SearchIndex};
use crate::theme::{Tokens, semibold};
use egui::{FontId, Rect, Sense, pos2, vec2};

#[derive(Default)]
pub struct Palette {
    pub(super) query: String,
    results: Results,
    selected: usize,
    /// The selection moved by keyboard: scroll it into view once.
    follow: bool,
}

impl Palette {
    /// Re-runs the query after the index changed.
    pub fn refresh(&mut self, index: &mut SearchIndex) {
        self.results = index.search(&self.query);
        self.selected = self.selected.min(self.results.len().saturating_sub(1));
    }
}

#[derive(Clone, Copy)]
enum Verb {
    Open,
    Play,
    PlayNext,
    Add,
    /// Toggle keeping it offline.
    Download,
}

const WIDTH: f32 = 640.0;
const ROW_H: f32 = 44.0;
const QUERY_H: f32 = 52.0;
const HINTS_H: f32 = 34.0;
/// A group's title line.
const GROUP_H: f32 = 24.0;

impl ViewCtx<'_> {
    pub(super) fn palette_ui(&mut self, ctx: &egui::Context) {
        if self.ui.palette.is_none() {
            return;
        }
        if self.search.index.is_none() {
            self.ui.palette = None;
            return;
        }
        use egui::{Key, Modifiers};
        let keys = ctx.input_mut(|i| {
            [
                i.consume_key(Modifiers::COMMAND, Key::Enter),
                i.consume_key(Modifiers::ALT, Key::Enter),
                i.consume_key(Modifiers::SHIFT, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_shortcut(&egui::KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::D)),
            ]
        });
        let [cmd_enter, alt_enter, shift_enter, enter, up, down, download] = keys;
        let mut verb = match (cmd_enter, alt_enter, shift_enter, enter) {
            (true, ..) => Some(Verb::Play),
            (_, true, ..) => Some(Verb::PlayNext),
            (_, _, true, _) => Some(Verb::Add),
            (.., true) => Some(Verb::Open),
            _ if download => Some(Verb::Download),
            _ => None,
        };
        let Some(palette) = self.ui.palette.as_mut() else { return };
        let n = palette.results.len();
        if n > 0 {
            if down {
                palette.selected = (palette.selected + 1) % n;
                palette.follow = true;
            }
            if up {
                palette.selected = (palette.selected + n - 1) % n;
                palette.follow = true;
            }
        }

        // Anchored near the top so the field stays put while results grow and
        // shrink, as in Spotlight.
        let screen = ctx.content_rect();
        let top = (screen.height() * 0.12).clamp(24.0, 120.0);
        let list_max = (screen.height() * 0.72 - QUERY_H - HINTS_H).max(ROW_H * 3.0);
        let id = egui::Id::unique("palette");
        let area = egui::Modal::default_area(id).anchor(egui::Align2::CENTER_TOP, vec2(0.0, top));
        let modal = egui::Modal::new(id).area(area).frame(egui::Frame::popup(&ctx.global_style()).inner_margin(0).corner_radius(12)).show(ctx, |ui| {
            let t = Tokens::of(ui);
            ui.set_width(WIDTH);
            ui.spacing_mut().item_spacing.y = 0.0;
            let palette = self.ui.palette.as_mut()?;
            let index = self.search.index.as_mut()?;

            // Query field.
            ui.horizontal(|ui| {
                ui.set_height(QUERY_H);
                ui.add_space(16.0);
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Search, t.muted);
                ui.add_space(4.0);
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut palette.query)
                        .hint_text("Search artists, albums and songs")
                        .font(FontId::proportional(17.0))
                        .frame(egui::Frame::NONE)
                        .desired_width(WIDTH - 90.0),
                );
                edit.request_focus();
                if edit.changed() {
                    palette.results = index.search(&palette.query);
                    palette.selected = 0;
                    palette.follow = true;
                }
                ui.label(egui::RichText::new("esc").size(11.0).color(t.muted));
            });
            ui.painter().hline(ui.min_rect().x_range(), ui.min_rect().bottom(), egui::Stroke::new(1.0, t.hairline));

            let mut clicked = None;
            let r = palette.results.clone();
            if r.len() == 0 {
                if !palette.query.trim().is_empty() {
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        ui.add_space(18.0);
                        ui.label(egui::RichText::new("No matches").color(t.muted));
                    });
                    ui.add_space(14.0);
                }
            } else {
                let groups: [(&str, Vec<Hit>); 4] = [
                    ("Top hit", r.top.into_iter().collect()),
                    ("Artists", r.artists.iter().map(|&i| Hit::Artist(i)).collect()),
                    ("Albums", r.albums.iter().map(|&i| Hit::Album(i)).collect()),
                    ("Songs", r.tracks.iter().map(|&i| Hit::Track(i)).collect()),
                ];
                // Tall enough for everything when it fits, so groups aren't
                // hidden behind a scroll; scrolls only past ~70% of the window.
                let shown = groups.iter().filter(|(_, h)| !h.is_empty());
                let content_h = 6.0 + shown.map(|(_, h)| GROUP_H + ROW_H * h.len() as f32 + 6.0).sum::<f32>();
                let follow = std::mem::take(&mut palette.follow);
                egui::ScrollArea::vertical().min_scrolled_height(content_h.min(list_max)).max_height(list_max).show(ui, |ui| {
                    ui.add_space(6.0);
                    let mut row = 0;
                    for (title, hits) in groups {
                        if hits.is_empty() {
                            continue;
                        }
                        let (head, _) = ui.allocate_exact_size(vec2(WIDTH, GROUP_H), Sense::hover());
                        ui.painter().text(
                            pos2(head.left() + 18.0, head.bottom() - 4.0),
                            egui::Align2::LEFT_BOTTOM,
                            title,
                            FontId::new(11.0, semibold()),
                            t.muted,
                        );
                        for hit in hits {
                            let selected = row == palette.selected;
                            let (rect, resp) = ui.allocate_exact_size(vec2(WIDTH, ROW_H), Sense::click());
                            let inner = rect.shrink2(vec2(8.0, 1.0));
                            if selected {
                                ui.painter().rect_filled(inner, 8.0, t.selected);
                                if follow {
                                    resp.scroll_to_me(None);
                                }
                            } else if resp.hovered() {
                                ui.painter().rect_filled(inner, 8.0, t.hover);
                            }
                            let tile = Rect::from_center_size(pos2(inner.left() + 22.0, inner.center().y), vec2(30.0, 30.0));
                            let (main, sub) = match hit {
                                Hit::Artist(i) => {
                                    let a = &index.artists[i];
                                    ui.painter().circle_filled(tile.center(), 15.0, t.field);
                                    icons::paint(ui.painter(), tile.shrink(8.0), Icon::Person, t.muted);
                                    (a.name.clone(), format!("Artist · {}", crate::fmt::plural(a.albums, "album")))
                                }
                                Hit::Album(i) => {
                                    let a = &index.albums[i];
                                    let cover = self.data.album(&a.id).and_then(|al| al.cover_art.clone());
                                    self.art.paint(ui, tile, cover.as_deref(), 4.0);
                                    let sub = match a.year {
                                        Some(y) => format!("{} · {y}", a.artist),
                                        None => a.artist.clone(),
                                    };
                                    (a.title.clone(), sub)
                                }
                                Hit::Track(i) => {
                                    let tr = &index.tracks[i];
                                    let cover = self.data.album(&tr.album_id).and_then(|al| al.cover_art.clone());
                                    self.art.paint(ui, tile, cover.as_deref(), 4.0);
                                    (tr.title.clone(), format!("Song · {} · {}", tr.artist, tr.album))
                                }
                            };
                            let x = tile.right() + 12.0;
                            let hint_w = if selected { 170.0 } else { 0.0 };
                            let w = inner.right() - x - hint_w - 12.0;
                            let g = elide(ui, &main, FontId::proportional(13.0), t.text, w);
                            ui.painter().galley(pos2(x, inner.center().y - g.size().y), g, t.text);
                            let g = elide(ui, &sub, FontId::proportional(12.0), t.muted, w);
                            ui.painter().galley(pos2(x, inner.center().y + 1.0), g, t.muted);
                            if selected {
                                ui.painter().text(
                                    pos2(inner.right() - 12.0, inner.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    "⏎ open    ⌘⏎ play",
                                    FontId::proportional(11.0),
                                    t.muted,
                                );
                            }
                            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::ListBoxOption, true, selected, &main));
                            if resp.clicked() {
                                let play = ui.input(|i| i.modifiers.command);
                                clicked = Some((hit, if play { Verb::Play } else { Verb::Open }));
                            }
                            row += 1;
                        }
                        ui.add_space(6.0);
                    }
                });
            }

            // Key hints.
            let (bar, _) = ui.allocate_exact_size(vec2(WIDTH, HINTS_H), Sense::hover());
            ui.painter().rect_filled(bar, egui::CornerRadius { nw: 0, ne: 0, sw: 12, se: 12 }, t.sidebar);
            ui.painter().hline(bar.x_range(), bar.top(), egui::Stroke::new(1.0, t.hairline));
            ui.painter().text(
                pos2(bar.left() + 18.0, bar.center().y),
                egui::Align2::LEFT_CENTER,
                "⏎ open    ⌘⏎ play    ⌥⏎ play next    ⇧⏎ add to queue    ⇧⌘D download",
                FontId::proportional(11.0),
                t.muted,
            );
            clicked
        });
        if let Some(c) = modal.inner {
            verb = Some(c.1);
            if let Some(p) = self.ui.palette.as_mut() {
                // A click acts on what was clicked, not on the keyboard selection.
                let at = (0..p.results.len()).find(|&k| p.results.hit(k) == Some(c.0));
                p.selected = at.unwrap_or(p.selected);
            }
        }
        let close = modal.should_close();
        let Some(palette) = self.ui.palette.as_ref() else { return };
        match (verb, palette.results.hit(palette.selected)) {
            (Some(v), Some(hit)) => {
                self.ui.palette = None;
                self.run_hit(hit, v);
            }
            _ if close => self.ui.palette = None,
            _ => {}
        }
    }

    fn run_hit(&mut self, hit: Hit, verb: Verb) {
        let Some(index) = self.search.index.as_ref() else { return };
        match hit {
            Hit::Artist(i) => {
                let name = index.artists[i].name.clone();
                let known = index.artists[i].id.clone();
                let action = match verb {
                    Verb::Open => {
                        let id = known.or_else(|| self.data.albums.iter().find(|a| a.artist.as_deref() == Some(name.as_str())).and_then(|a| a.artist_ref.clone()));
                        self.actions.push(Action::Go(Route::Artist { name, id }));
                        return;
                    }
                    Verb::Play => QueueAction::Play { start: 0 },
                    Verb::PlayNext => QueueAction::PlayNext,
                    Verb::Add => QueueAction::Add,
                    // A whole artist is a lot to download by accident; do it per album.
                    Verb::Download => return,
                };
                self.actions.push(Action::Artist(name, action));
            }
            Hit::Album(i) => {
                let id = index.albums[i].id.clone();
                let kept = self.downloads.snap.pin(&offline::Pin::Album(id.clone())).is_some();
                self.actions.push(match verb {
                    Verb::Open => Action::Go(Route::Album(id)),
                    Verb::Play => Action::Album(id, QueueAction::Play { start: 0 }),
                    Verb::PlayNext => Action::Album(id, QueueAction::PlayNext),
                    Verb::Add => Action::Album(id, QueueAction::Add),
                    Verb::Download => Action::KeepOffline(super::Keep::Album(id), !kept),
                });
            }
            Hit::Track(i) => {
                let (id, album_id) = (index.tracks[i].id.clone(), index.tracks[i].album_id.clone());
                let Some(tr) = self.data.tracks.album(&album_id).and_then(|ts| ts.iter().find(|t| t.id == id)).cloned() else { return };
                match verb {
                    Verb::Open => self.actions.push(Action::Go(Route::Album(album_id))),
                    // Plays the album from this track, as double-clicking it there would.
                    Verb::Play => self.play_from_album(&tr, QueueAction::Play { start: 0 }),
                    Verb::PlayNext => self.play_from_album(&tr, QueueAction::PlayNext),
                    Verb::Add => self.play_from_album(&tr, QueueAction::Add),
                    Verb::Download => {
                        let kept = self.downloads.snap.pin(&offline::Pin::Track(tr.id.clone())).is_some();
                        self.actions.push(Action::KeepOffline(super::Keep::Tracks(vec![tr]), !kept));
                    }
                }
            }
        }
    }
}
