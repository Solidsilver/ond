//! Sheets over the window: naming a playlist, confirming a delete or a sign-out, asking
//! whether to keep the tracks you added, the keyboard shortcut list (⌘/) and
//! settings (⌘,). One at a time; Esc or a click outside closes it.

use super::commands::PlaylistAction;
use super::{Action, OndApp};
use crate::keys::{BINDINGS, GROUPS};
use crate::theme::{Tokens, semibold};
use crate::widgets::{primary, secondary};
use egui::{FontId, Key, Modifiers, vec2};
use library::Track;
use shell::kit::{DIALOG_W, ShortcutGroup, confirm, shortcuts_sheet};

pub enum Overlay {
    Shortcuts,
    Settings,
    /// Name a new playlist holding `tracks` (empty for File › New Playlist).
    NewPlaylist {
        name: String,
        tracks: Vec<Track>,
    },
    Rename {
        id: String,
        name: String,
    },
    Delete {
        id: String,
        name: String,
    },
    /// Sign out of `host` (its name, without the scheme)? Asked from
    /// Settings, a No goes back there.
    SignOut {
        host: String,
        from_settings: bool,
    },
    /// Playing something new while the user has added tracks: keep them or
    /// clear them? (Settings › Queue › Playing something new: Ask.)
    KeepAdded {
        tracks: Vec<Track>,
        source: crate::queue::Source,
        action: super::QueueAction,
        added: usize,
    },
}

impl Overlay {
    pub fn new_playlist(tracks: Vec<Track>) -> Self {
        Overlay::NewPlaylist { name: String::new(), tracks }
    }
}

/// Whether the open name dialog has focused its field yet. Cleared whenever
/// no name dialog is up, however it closed, so the next one focuses again.
fn name_focused_id() -> egui::Id {
    egui::Id::unique("overlay-name-focused")
}

impl OndApp {
    pub(super) fn overlay_ui(&mut self, ctx: &egui::Context) {
        if !matches!(self.ui.overlay, Some(Overlay::NewPlaylist { .. } | Overlay::Rename { .. })) {
            ctx.data_mut(|d| d.remove::<bool>(name_focused_id()));
        }
        if matches!(self.ui.overlay, Some(Overlay::Settings)) {
            self.settings_window(ctx);
            return;
        }
        // Experimental keys are listed only while their experiment is on.
        let visualizer = self.ui.prefs.experimental.visualizer;
        let Some(overlay) = self.ui.overlay.as_mut() else { return };
        let id = egui::Id::unique("overlay");
        let frame = shell::kit::sheet_frame(ctx);
        let mut done: Option<Option<Action>> = None;
        let modal = egui::Modal::new(id).frame(frame).show(ctx, |ui| {
            let t = Tokens::of(ui);
            match overlay {
                Overlay::Shortcuts => shortcuts_ui(ui, visualizer),
                // Drawn by `settings_window` above, which returns first.
                Overlay::Settings => {}
                Overlay::NewPlaylist { name, tracks } => {
                    let what = match tracks.len() {
                        0 => String::new(),
                        1 => format!("With “{}”", tracks[0].title),
                        n => format!("With {n} songs"),
                    };
                    if let Some(ok) = name_dialog(ui, "New playlist", &what, name, "Create") {
                        done = Some(ok.then(|| Action::PlaylistEdit(PlaylistAction::Create { name: name.clone(), tracks: std::mem::take(tracks) })));
                    }
                }
                Overlay::Rename { id, name } => {
                    if let Some(ok) = name_dialog(ui, "Rename playlist", "", name, "Rename") {
                        done = Some(ok.then(|| Action::PlaylistEdit(PlaylistAction::Rename { id: id.clone(), name: name.clone() })));
                    }
                }
                Overlay::KeepAdded { tracks, source, action, added } => {
                    ui.set_width(DIALOG_W);
                    let what = if *added == 1 { "the track you added".to_owned() } else { format!("the {added} tracks you added") };
                    ui.label(egui::RichText::new("Keep what you added?").font(FontId::new(15.0, semibold())).color(t.text));
                    ui.add_space(6.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("Keep {what} to play after this, or clear them from the queue.")).size(12.0).color(t.muted),
                        )
                        .wrap(),
                    );
                    ui.add_space(18.0);
                    let enter = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
                    let mut answer = None;
                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if primary(ui, None, "Keep").clicked() || enter {
                                answer = Some(true);
                            }
                            ui.add_space(4.0);
                            if secondary(ui, "Clear").clicked() {
                                answer = Some(false);
                            }
                        });
                    });
                    if let Some(keep) = answer {
                        done = Some(Some(Action::TracksAnswered { tracks: std::mem::take(tracks), source: source.clone(), action: *action, keep }));
                    }
                }
                Overlay::SignOut { host, .. } => {
                    let title = if host.is_empty() { "Sign out?".to_owned() } else { format!("Sign out of {host}?") };
                    let body = "Playback stops and the queue clears. Ond forgets the password. Downloads stay on this computer.";
                    if let Some(yes) = confirm(ui, &title, body, "Sign out") {
                        done = Some(yes.then_some(Action::SignOut));
                    }
                }
                Overlay::Delete { id, name } => {
                    let body = "This deletes the playlist on the server. The songs stay in your library.";
                    if let Some(yes) = confirm(ui, &format!("Delete “{name}”?"), body, "Delete") {
                        done = Some(yes.then(|| Action::PlaylistEdit(PlaylistAction::Delete { id: id.clone() })));
                    }
                }
            }
        });
        if modal.should_close() {
            done = Some(None);
        }
        if let Some(action) = done {
            ctx.data_mut(|d| d.remove::<bool>(name_focused_id()));
            let back_to_settings = action.is_none() && matches!(self.ui.overlay, Some(Overlay::SignOut { from_settings: true, .. }));
            self.ui.overlay = back_to_settings.then_some(Overlay::Settings);
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
    let field = ui.add(egui::TextEdit::singleline(name).hint_text("Playlist name").desired_width(DIALOG_W).margin(vec2(10.0, 7.0)).background_color(t.field));
    // Focus once; after that the user may tab away.
    if !ui.data(|d| d.get_temp::<bool>(name_focused_id()).unwrap_or(false)) {
        field.request_focus();
        ui.data_mut(|d| d.insert_temp(name_focused_id(), true));
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
    out
}

/// The key map in the kit's shortcuts sheet, in two or three columns (three
/// when the window is wide). Rows with the same label merge their keys.
fn shortcuts_ui(ui: &mut egui::Ui, visualizer: bool) {
    let groups: Vec<ShortcutGroup<'_>> = GROUPS
        .iter()
        .map(|g| ShortcutGroup {
            name: g,
            rows: group_rows(g).iter().filter(|(l, _)| visualizer || *l != "Visualizer").map(|(l, keys)| (*l, keys.as_slice())).collect(),
        })
        .collect();
    shortcuts_sheet(ui, &groups, 2..=3, false);
}

/// A group's rows, worked out once (the key map is fixed).
fn group_rows(group: &str) -> &'static [(&'static str, Vec<&'static str>)] {
    type Rows = Vec<(&'static str, Vec<&'static str>)>;
    static ALL: std::sync::OnceLock<Vec<(&'static str, Rows)>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| GROUPS.iter().map(|g| (*g, rows(g))).collect()).iter().find(|(g, _)| *g == group).map_or(&[], |(_, r)| r.as_slice())
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
