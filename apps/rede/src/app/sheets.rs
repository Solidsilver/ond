//! The sheets over the window, one at a time, in the frame Ond's dialogs
//! use: the keyboard shortcuts (⌘/) and the playback settings. Each has its
//! title and a × at the top; Esc or a click outside closes it too.
//!
//! The playback settings are a sheet of their own because they're
//! preferences, not controls: how far the skips go, the rewinds after a long
//! pause and after the sleep timer, and a new timer's length. The Playback
//! popover has the speed (and, with M3, voice boost and skip silence); it and
//! the account menu both lead here. When Rede has a Settings window, this
//! sheet's groups become its Playback page.

use super::view_ctx::ViewCtx;
use super::{Action, PrefsAction, RedeApp};
use egui::{FontId, Ui};
use shell::kit;
use shell::theme::{Tokens, semibold};

/// Which sheet is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sheet {
    Shortcuts,
    PlaybackSettings,
}

/// A group of shortcuts: its name, and each row's description and keys.
type Group = (&'static str, &'static [(&'static str, &'static [&'static str])]);

/// The keyboard shortcuts, grouped, each with the keys that do it. Laid out
/// in two columns: Playback and Chapters, then Window and Navigation.
const SHORTCUTS: [Group; 4] = [
    ("Playback", &[("Play / pause", &["Space"]), ("Skip back", &["←"]), ("Skip forward", &["→"]), ("Slower", &["["]), ("Faster", &["]"])]),
    ("Chapters", &[("Previous chapter", &["⇧←"]), ("Next chapter", &["⇧→"]), ("Show / hide chapters", &["⌥⌘U"])]),
    ("Window", &[("Player, and back", &["⇧⌘N"]), ("Mini, and back", &["⇧⌘M"]), ("Full, Player, Mini", &["⌥⌘1", "⌥⌘2", "⌥⌘3"])]),
    ("Navigation", &[("Collapse / expand sidebar", &["⌥⌘S"]), ("Keyboard shortcuts", &["⌘/"]), ("Close a sheet or menu", &["esc"])]),
];
const COL_W: f32 = 250.0;
const COL_GAP: f32 = 32.0;
const ROW_H: f32 = 26.0;

/// The playback settings sheet's width.
const SETTINGS_W: f32 = 380.0;
/// The skips' lengths, and the rewinds' (0 is off), in seconds; a new
/// timer's, in minutes.
const SKIPS: [u32; 11] = [5, 10, 15, 20, 30, 45, 60, 90, 120, 180, 300];
const REWINDS: [u32; 10] = [0, 1, 2, 3, 5, 10, 15, 20, 30, 60];
const TIMER: [u32; 10] = [5, 10, 15, 20, 30, 45, 60, 75, 90, 120];

impl RedeApp {
    /// The open sheet, if any, over everything else.
    pub(super) fn sheet_ui(&mut self, ctx: &egui::Context) {
        let Some(sheet) = self.sheet else { return };
        let modal = egui::Modal::new(egui::Id::unique("sheet").with(sheet)).frame(kit::sheet_frame(ctx)).show(ctx, |ui| {
            let mut view = self.view();
            let close = match sheet {
                Sheet::Shortcuts => view.shortcuts_sheet(ui),
                Sheet::PlaybackSettings => view.playback_settings(ui),
            };
            if close {
                view.actions.push(Action::CloseSheet);
            }
        });
        if modal.should_close() {
            self.actions.push(Action::CloseSheet);
        }
    }
}

impl ViewCtx<'_> {
    /// The keyboard shortcuts in two columns, or one in a narrow window (the
    /// Player). True when its × was clicked.
    fn shortcuts_sheet(&mut self, ui: &mut Ui) -> bool {
        let t = Tokens::of(ui);
        let two = ui.ctx().content_rect().width() >= 2.0 * COL_W + COL_GAP + 80.0;
        let width = if two { 2.0 * COL_W + COL_GAP } else { COL_W };
        ui.set_width(width);
        let close = kit::sheet_header(ui, "Keyboard shortcuts");
        ui.add_space(14.0);
        let group = |ui: &mut Ui, (name, rows): Group| {
            ui.label(egui::RichText::new(name).font(FontId::new(11.0, semibold())).color(t.muted));
            ui.add_space(4.0);
            for (label, keys) in rows {
                ui.horizontal(|ui| {
                    ui.set_height(ROW_H);
                    ui.label(egui::RichText::new(*label).size(13.0).color(t.text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for k in keys.iter().rev() {
                            kit::key_cap(ui, &shell::keys::shown(k));
                        }
                    });
                });
            }
        };
        let column = |ui: &mut Ui, groups: &[Group]| {
            ui.vertical(|ui| {
                ui.set_width(COL_W);
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, g) in groups.iter().enumerate() {
                    if i > 0 {
                        ui.add_space(16.0);
                    }
                    group(ui, *g);
                }
            });
        };
        let max_h = (ui.ctx().content_rect().height() * 0.85 - 90.0).max(160.0);
        let out = egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
            if two {
                ui.horizontal_top(|ui| {
                    column(ui, &SHORTCUTS[..2]);
                    ui.add_space(COL_GAP);
                    column(ui, &SHORTCUTS[2..]);
                });
            } else {
                column(ui, &SHORTCUTS);
            }
        });
        // When it scrolls (one column, in the Player), the last rows fade
        // into the sheet, as Ond's do, so it's plain there's more.
        let inner = out.inner_rect;
        if out.content_size.y - (out.state.clamped_offset().y + inner.height()) > 1.0 {
            let fade = egui::Rect::from_min_max(egui::pos2(inner.left(), inner.bottom() - 28.0), inner.max);
            let bg = ui.visuals().window_fill;
            let mut mesh = egui::Mesh::default();
            for (at, alpha) in [(fade.left_top(), 0.0), (fade.right_top(), 0.0), (fade.left_bottom(), 1.0), (fade.right_bottom(), 1.0)] {
                mesh.colored_vertex(at, bg.gamma_multiply(alpha));
            }
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(1, 2, 3);
            ui.painter().add(mesh);
        }
        close
    }

    /// The playback settings, in Ond's settings rows: each a name and a hint
    /// at the left, a stepper at the right. True when its × was clicked.
    fn playback_settings(&mut self, ui: &mut Ui) -> bool {
        ui.set_width(SETTINGS_W);
        let close = kit::sheet_header(ui, "Playback settings");
        ui.add_space(14.0);
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut prefs = self.playback;
        let seconds = |s: u32| format!("{s} s");
        let or_off = |s: u32| if s == 0 { "Off".to_owned() } else { format!("{s} s") };
        kit::group(ui, "Skipping", |ui| {
            kit::setting(ui, "Skip back", "", |ui| kit::stepper(ui, &mut prefs.skip_back, &SKIPS, "skip back", seconds));
            kit::setting(ui, "Skip forward", "", |ui| kit::stepper(ui, &mut prefs.skip_forward, &SKIPS, "skip forward", seconds));
        });
        kit::group(ui, "Resuming", |ui| {
            let hint = "After a pause of 5 minutes or more, so you hear where you left off.";
            kit::setting(ui, "Rewind after a pause", hint, |ui| kit::stepper(ui, &mut prefs.rewind, &REWINDS, "rewind after a pause", or_off));
            let hint = "When the sleep timer stops playback.";
            kit::setting(ui, "Rewind after sleep", hint, |ui| kit::stepper(ui, &mut prefs.sleep_rewind, &REWINDS, "rewind after sleep", or_off));
        });
        kit::group(ui, "Sleep timer", |ui| {
            let hint = "What Custom starts with; starting a timer remembers its length.";
            kit::setting(ui, "Timer length", hint, |ui| kit::stepper(ui, &mut prefs.sleep_minutes, &TIMER, "timer length", |m| format!("{m} min")));
        });
        if prefs != self.playback {
            self.actions.push(Action::Prefs(PrefsAction::Playback(prefs)));
        }
        close
    }
}
