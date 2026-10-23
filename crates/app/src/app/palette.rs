//! ⌘K: search the library and act on a result without leaving the keyboard.
//! ⏎ opens · ⌘⏎ plays · ⌥⏎ plays next · ⇧⏎ adds to the queue.

use super::widgets::elide;
use super::{Action, ListenerApp, QueueAction};
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
}

const WIDTH: f32 = 640.0;
const ROW_H: f32 = 44.0;

impl ListenerApp {
    pub(super) fn palette_ui(&mut self, ctx: &egui::Context) {
        if self.palette.is_none() {
            return;
        }
        if self.search.is_none() {
            self.palette = None;
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
            ]
        });
        let [cmd_enter, alt_enter, shift_enter, enter, up, down] = keys;
        let mut verb = match (cmd_enter, alt_enter, shift_enter, enter) {
            (true, ..) => Some(Verb::Play),
            (_, true, ..) => Some(Verb::PlayNext),
            (_, _, true, _) => Some(Verb::Add),
            (.., true) => Some(Verb::Open),
            _ => None,
        };
        let Some(palette) = self.palette.as_mut() else { return };
        let n = palette.results.len();
        if n > 0 {
            if down {
                palette.selected = (palette.selected + 1) % n;
            }
            if up {
                palette.selected = (palette.selected + n - 1) % n;
            }
        }

        let modal = egui::Modal::new(egui::Id::unique("palette")).frame(egui::Frame::popup(&ctx.global_style()).inner_margin(0).corner_radius(12)).show(ctx, |ui| {
            let t = Tokens::of(ui);
            ui.set_width(WIDTH);
            ui.spacing_mut().item_spacing.y = 0.0;
            let palette = self.palette.as_mut()?;
            let index = self.search.as_mut()?;

            // Query field.
            ui.horizontal(|ui| {
                ui.set_height(52.0);
                ui.add_space(16.0);
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Search, t.muted);
                ui.add_space(4.0);
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut palette.query)
                        .hint_text("Search artists and albums")
                        .font(FontId::proportional(17.0))
                        .frame(egui::Frame::NONE)
                        .desired_width(WIDTH - 90.0),
                );
                edit.request_focus();
                if edit.changed() {
                    palette.results = index.search(&palette.query);
                    palette.selected = 0;
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
                egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    ui.add_space(6.0);
                    let mut row = 0;
                    let groups: [(&str, Vec<Hit>); 2] =
                        [("Artists", r.artists.iter().map(|&i| Hit::Artist(i)).collect()), ("Albums", r.albums.iter().map(|&i| Hit::Album(i)).collect())];
                    for (title, hits) in groups {
                        if hits.is_empty() {
                            continue;
                        }
                        ui.horizontal(|ui| {
                            ui.add_space(18.0);
                            ui.label(egui::RichText::new(title).font(FontId::new(11.0, semibold())).color(t.muted));
                        });
                        ui.add_space(4.0);
                        for hit in hits {
                            let selected = row == palette.selected;
                            let (rect, resp) = ui.allocate_exact_size(vec2(WIDTH, ROW_H), Sense::click());
                            let inner = rect.shrink2(vec2(8.0, 1.0));
                            if selected {
                                ui.painter().rect_filled(inner, 8.0, t.selected);
                                resp.scroll_to_me(None);
                            } else if resp.hovered() {
                                ui.painter().rect_filled(inner, 8.0, t.hover);
                            }
                            let tile = Rect::from_center_size(pos2(inner.left() + 22.0, inner.center().y), vec2(30.0, 30.0));
                            let (main, sub) = match hit {
                                Hit::Artist(i) => {
                                    let a = &index.artists[i];
                                    ui.painter().circle_filled(tile.center(), 15.0, t.field);
                                    icons::paint(ui.painter(), tile.shrink(8.0), Icon::Person, t.muted);
                                    (a.name.clone(), format!("Artist · {} album{}", a.albums, if a.albums == 1 { "" } else { "s" }))
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
            let (bar, _) = ui.allocate_exact_size(vec2(WIDTH, 34.0), Sense::hover());
            ui.painter().rect_filled(bar, egui::CornerRadius { nw: 0, ne: 0, sw: 12, se: 12 }, t.sidebar);
            ui.painter().hline(bar.x_range(), bar.top(), egui::Stroke::new(1.0, t.hairline));
            ui.painter().text(
                pos2(bar.left() + 18.0, bar.center().y),
                egui::Align2::LEFT_CENTER,
                "⏎ open    ⌘⏎ play    ⌥⏎ play next    ⇧⏎ add to queue    ↑↓ move",
                FontId::proportional(11.0),
                t.muted,
            );
            clicked
        });
        if let Some(c) = modal.inner {
            verb = Some(c.1);
            if let Some(p) = self.palette.as_mut() {
                // A click acts on what was clicked, not on the keyboard selection.
                let at = (0..p.results.len()).find(|&k| p.results.hit(k) == Some(c.0));
                p.selected = at.unwrap_or(p.selected);
            }
        }
        let close = modal.should_close();
        let Some(palette) = self.palette.as_ref() else { return };
        match (verb, palette.results.hit(palette.selected)) {
            (Some(v), Some(hit)) => {
                self.palette = None;
                self.run_hit(hit, v);
            }
            _ if close => self.palette = None,
            _ => {}
        }
    }

    fn run_hit(&mut self, hit: Hit, verb: Verb) {
        let Some(index) = self.search.as_ref() else { return };
        match hit {
            Hit::Artist(i) => {
                let name = index.artists[i].name.clone();
                let action = match verb {
                    Verb::Open => {
                        let id = self.data.albums.iter().find(|a| a.artist.as_deref() == Some(name.as_str())).and_then(|a| a.artist_ref.clone());
                        self.actions.push(Action::Go(Route::Artist { name, id }));
                        return;
                    }
                    Verb::Play => QueueAction::Play { start: 0 },
                    Verb::PlayNext => QueueAction::PlayNext,
                    Verb::Add => QueueAction::Add,
                };
                self.actions.push(Action::Artist(name, action));
            }
            Hit::Album(i) => {
                let id = index.albums[i].id.clone();
                self.actions.push(match verb {
                    Verb::Open => Action::Go(Route::Album(id)),
                    Verb::Play => Action::Album(id, QueueAction::Play { start: 0 }),
                    Verb::PlayNext => Action::Album(id, QueueAction::PlayNext),
                    Verb::Add => Action::Album(id, QueueAction::Add),
                });
            }
        }
    }
}
