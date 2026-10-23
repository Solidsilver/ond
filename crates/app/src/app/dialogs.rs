//! Sheets over the window: naming a playlist, confirming a delete, the
//! keyboard shortcut list (⌘/) and settings (⌘,). One at a time; Esc or a
//! click outside closes it.

use super::commands::PlaylistAction;
use super::widgets::{danger, primary, secondary};
use super::{Action, ListenerApp};
use crate::keys::{BINDINGS, GROUPS};
use crate::theme::{Tokens, semibold};
use egui::{FontId, Key, Modifiers, vec2};
use library::Track;

pub enum Overlay {
    Shortcuts,
    Settings,
    /// Name a new playlist holding `tracks` (empty for File › New Playlist).
    NewPlaylist { name: String, tracks: Vec<Track> },
    Rename { id: String, name: String },
    Delete { id: String, name: String },
}

impl Overlay {
    pub fn new_playlist(tracks: Vec<Track>) -> Self {
        Overlay::NewPlaylist { name: String::new(), tracks }
    }
}

const DIALOG_W: f32 = 340.0;

impl ListenerApp {
    pub(super) fn overlay_ui(&mut self, ctx: &egui::Context) {
        if matches!(self.overlay, Some(Overlay::Settings)) {
            // The gear popover's contents, as a sheet; `settings_ui` needs `self`.
            self.settings_sheet(ctx);
            return;
        }
        let Some(overlay) = self.overlay.as_mut() else { return };
        let id = egui::Id::unique("overlay");
        let frame = egui::Frame::popup(&ctx.global_style()).inner_margin(egui::Margin::same(20)).corner_radius(12);
        let mut done: Option<Option<Action>> = None;
        let modal = egui::Modal::new(id).frame(frame).show(ctx, |ui| {
            let t = Tokens::of(ui);
            match overlay {
                Overlay::Shortcuts | Overlay::Settings => shortcuts_ui(ui),
                Overlay::NewPlaylist { name, tracks } => {
                    let what = match tracks.len() {
                        0 => String::new(),
                        1 => format!("With “{}”", tracks[0].title),
                        n => format!("With {n} songs"),
                    };
                    if let Some(ok) = name_dialog(ui, "New Playlist", &what, name, "Create") {
                        done = Some(ok.then(|| Action::PlaylistEdit(PlaylistAction::Create { name: name.clone(), tracks: std::mem::take(tracks) })));
                    }
                }
                Overlay::Rename { id, name } => {
                    if let Some(ok) = name_dialog(ui, "Rename Playlist", "", name, "Rename") {
                        done = Some(ok.then(|| Action::PlaylistEdit(PlaylistAction::Rename { id: id.clone(), name: name.clone() })));
                    }
                }
                Overlay::Delete { id, name } => {
                    ui.set_width(DIALOG_W);
                    ui.label(egui::RichText::new(format!("Delete “{name}”?")).font(FontId::new(15.0, semibold())).color(t.text));
                    ui.add_space(6.0);
                    ui.add(egui::Label::new(egui::RichText::new("The playlist is removed from the server. The songs stay in your library.").size(12.0).color(t.muted)).wrap());
                    ui.add_space(18.0);
                    let enter = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if danger(ui, "Delete").clicked() || enter {
                                done = Some(Some(Action::PlaylistEdit(PlaylistAction::Delete { id: id.clone() })));
                            }
                            ui.add_space(4.0);
                            if secondary(ui, "Cancel").clicked() {
                                done = Some(None);
                            }
                        });
                    });
                }
            }
        });
        if modal.should_close() {
            done = Some(None);
        }
        if let Some(action) = done {
            self.overlay = None;
            if let Some(a) = action {
                self.actions.push(a);
            }
        }
    }

    fn settings_sheet(&mut self, ctx: &egui::Context) {
        let frame = egui::Frame::popup(&ctx.global_style()).inner_margin(egui::Margin::symmetric(12, 14)).corner_radius(12);
        let modal = egui::Modal::new(egui::Id::unique("settings-sheet")).frame(frame).show(ctx, |ui| {
            let t = Tokens::of(ui);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Settings").font(FontId::new(15.0, semibold())).color(t.text));
            });
            ui.add_space(8.0);
            self.settings_ui(ui);
        });
        if modal.should_close() {
            self.overlay = None;
        }
    }
}

/// Title, a name field (focused, ⏎ submits) and Cancel / `verb`.
/// `Some(true)` submits, `Some(false)` cancels.
fn name_dialog(ui: &mut egui::Ui, title: &str, sub: &str, name: &mut String, verb: &str) -> Option<bool> {
    let t = Tokens::of(ui);
    ui.set_width(DIALOG_W);
    ui.label(egui::RichText::new(title).font(FontId::new(15.0, semibold())).color(t.text));
    if !sub.is_empty() {
        ui.add_space(2.0);
        ui.label(egui::RichText::new(sub).size(12.0).color(t.muted));
    }
    ui.add_space(14.0);
    let field = ui.add(
        egui::TextEdit::singleline(name)
            .hint_text("Playlist name")
            .desired_width(DIALOG_W)
            .margin(vec2(10.0, 7.0))
            .background_color(t.field),
    );
    // Focus once; after that the user may tab away.
    let focused_once = ui.scope_id().with("focused");
    if !ui.data(|d| d.get_temp::<bool>(focused_once).unwrap_or(false)) {
        field.request_focus();
        ui.data_mut(|d| d.insert_temp(focused_once, true));
    }
    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    ui.add_space(18.0);
    let mut out = None;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if primary(ui, None, verb).clicked() || enter {
                out = Some(true);
            }
            ui.add_space(4.0);
            if secondary(ui, "Cancel").clicked() {
                out = Some(false);
            }
        });
    });
    if out.is_some() {
        ui.data_mut(|d| d.remove::<bool>(focused_once));
    }
    out
}

/// The key map, grouped, two columns. Rows with the same label merge their keys.
fn shortcuts_ui(ui: &mut egui::Ui) {
    let t = Tokens::of(ui);
    const COL_W: f32 = 270.0;
    ui.set_width(COL_W * 2.0 + 36.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Keyboard Shortcuts").font(FontId::new(15.0, semibold())).color(t.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new("esc").size(11.0).color(t.muted));
        });
    });
    ui.add_space(14.0);
    let column = |ui: &mut egui::Ui, groups: &[&str]| {
        ui.vertical(|ui| {
            ui.set_width(COL_W);
            for (gi, group) in groups.iter().enumerate() {
                if gi > 0 {
                    ui.add_space(16.0);
                }
                ui.label(egui::RichText::new(*group).font(FontId::new(11.0, semibold())).color(t.muted));
                ui.add_space(6.0);
                for (label, keys) in rows(group) {
                    ui.horizontal(|ui| {
                        ui.set_height(22.0);
                        ui.label(egui::RichText::new(label).size(13.0).color(t.text));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for k in keys.iter().rev() {
                                key_cap(ui, k);
                            }
                        });
                    });
                }
            }
        });
    };
    // Short windows scroll the list rather than cut it off.
    let max_h = (ui.ctx().content_rect().height() - 140.0).max(200.0);
    egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            column(ui, &GROUPS[..2]);
            ui.add_space(36.0);
            column(ui, &GROUPS[2..]);
        });
    });
}

/// One group's rows: (label, keys), keys merged for repeated labels.
fn rows(group: &str) -> Vec<(&'static str, Vec<&'static str>)> {
    let mut out: Vec<(&str, Vec<&str>)> = Vec::new();
    for b in BINDINGS.iter().filter(|b| b.group == group) {
        match out.iter_mut().find(|(l, _)| *l == b.label) {
            Some((_, keys)) => keys.push(b.keys),
            None => out.push((b.label, vec![b.keys])),
        }
    }
    out
}

/// A key combination in a quiet rounded box.
fn key_cap(ui: &mut egui::Ui, keys: &str) {
    let t = Tokens::of(ui);
    let g = ui.painter().layout_no_wrap(keys.to_owned(), FontId::proportional(12.0), t.text);
    let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 12.0, 20.0), egui::Sense::hover());
    ui.painter().rect(r, 5.0, t.field, egui::Stroke::new(1.0, t.hairline), egui::StrokeKind::Inside);
    ui.painter().galley(r.center() - g.size() / 2.0, g, t.text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_labels_merge_their_keys() {
        let go = rows("Go");
        let search = go.iter().find(|(l, _)| *l == "Search").unwrap();
        assert_eq!(search.1, ["⌘K", "⌘F", "/"]);
        // Every group has something to show.
        assert!(GROUPS.iter().all(|g| !rows(g).is_empty()));
    }
}
