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
            self.settings_window(ctx);
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

/// The key map, grouped into two or three balanced columns (three when the
/// window is wide). Rows with the same label merge their keys. It's sized to
/// fit; only a really short window scrolls, with a soft fade at the bottom.
fn shortcuts_ui(ui: &mut egui::Ui) {
    let t = Tokens::of(ui);
    const COL_W: f32 = 260.0;
    const GAP: f32 = 32.0;
    let screen = ui.ctx().content_rect();
    let cols = if screen.width() >= COL_W * 3.0 + GAP * 2.0 + 120.0 { 3 } else { 2 };
    let width = COL_W * cols as f32 + GAP * (cols - 1) as f32;
    ui.set_width(width);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Keyboard Shortcuts").font(FontId::new(15.0, semibold())).color(t.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new("esc").size(11.0).color(t.muted));
        });
    });
    ui.add_space(16.0);
    let columns = balance(&GROUPS, cols);
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
                        ui.set_height(ROW_H);
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
    // Up to ~85% of the window (less the title and the frame's padding).
    let max_h = (screen.height() * 0.85 - 80.0).max(200.0);
    // The modal's area only offers the height it had last frame, so ask for
    // the columns' full height outright (capped), or it never grows to fit.
    let tallest = columns.iter().map(|c| c.iter().map(|g| group_height(g)).sum::<f32>()).fold(0.0, f32::max);
    let out = egui::ScrollArea::vertical().max_height(max_h).min_scrolled_height(tallest.min(max_h)).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            for (k, groups) in columns.iter().enumerate() {
                if k > 0 {
                    ui.add_space(GAP);
                }
                column(ui, groups);
            }
        });
    });
    // When it does scroll, fade the last rows into the background.
    let inner = out.inner_rect;
    let hidden_below = out.content_size.y - (out.state.clamped_offset().y + inner.height());
    if hidden_below > 1.0 {
        let fade = egui::Rect::from_min_max(egui::pos2(inner.left(), inner.bottom() - 28.0), inner.max);
        let bg = ui.visuals().window_fill;
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(fade.left_top(), bg.gamma_multiply(0.0));
        mesh.colored_vertex(fade.right_top(), bg.gamma_multiply(0.0));
        mesh.colored_vertex(fade.left_bottom(), bg);
        mesh.colored_vertex(fade.right_bottom(), bg);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 2, 3);
        ui.painter().add(mesh);
    }
}

const ROW_H: f32 = 22.0;

/// A group's height in the list: its rows plus header and gap.
fn group_height(group: &str) -> f32 {
    rows(group).len() as f32 * (ROW_H + 8.0) + 44.0
}

/// Splits `groups` (in order) into `cols` columns of roughly equal height.
fn balance<'a>(groups: &[&'a str], cols: usize) -> Vec<Vec<&'a str>> {
    let height = group_height;
    let total: f32 = groups.iter().map(|g| height(g)).sum();
    let target = total / cols as f32;
    let mut out: Vec<Vec<&str>> = vec![Vec::new()];
    let mut acc = 0.0;
    for (i, g) in groups.iter().enumerate() {
        let h = height(g);
        let left = groups.len() - i;
        let open = cols - out.len();
        // Start a new column when this one is full, or when every remaining
        // group needs a column of its own.
        if !out.last().unwrap().is_empty() && open > 0 && (acc + h / 2.0 > target || left <= open) {
            out.push(Vec::new());
            acc = 0.0;
        }
        out.last_mut().unwrap().push(g);
        acc += h;
    }
    out
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

    #[test]
    fn columns_keep_every_group_in_order() {
        for cols in [2, 3] {
            let split = balance(&GROUPS, cols);
            assert!(split.len() <= cols && split.iter().all(|c| !c.is_empty()));
            let flat: Vec<&str> = split.into_iter().flatten().collect();
            assert_eq!(flat, GROUPS);
        }
    }
}
