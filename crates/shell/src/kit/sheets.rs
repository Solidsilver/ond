//! Sheets over the window: the [`sheet_frame`] they're drawn in, the
//! [`sheet_header`] with its ×, a [`confirm`] that asks before something
//! that can't be taken back, and the one [`shortcuts_sheet`] both apps fill
//! with their own keys (and the [`fade_below`] it scrolls with).

use crate::icons::Icon;
use crate::theme::{Tokens, semibold};
use egui::{FontId, Ui, vec2};

/// The frame every sheet over the window is drawn in (a dialog, the
/// keyboard shortcuts): the popup's fill, stroke and shadow, rounder, with
/// room inside.
pub fn sheet_frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::popup(&ctx.global_style()).inner_margin(egui::Margin::same(20)).corner_radius(12)
}

/// A sheet's heading: its title at 15 pt, and a × at the right end that
/// closes it (as Esc does). True when the × was clicked.
pub fn sheet_header(ui: &mut Ui, title: &str) -> bool {
    let t = Tokens::of(ui);
    let mut close = false;
    ui.horizontal(|ui| {
        ui.set_height(24.0);
        ui.label(egui::RichText::new(title).font(FontId::new(15.0, semibold())).color(t.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            close = crate::icons::sized_button(ui, Icon::Close, &crate::keys::shown("Close (esc)"), vec2(24.0, 24.0), 10.0, Some(t.muted)).clicked();
        });
    });
    close
}

/// A dialog's width.
pub const DIALOG_W: f32 = 340.0;

/// A question in a sheet: `title`, what happens in `body`, and Cancel beside
/// the red `verb`, which ⏎ answers too. `Some(true)` for `verb`,
/// `Some(false)` for Cancel.
pub fn confirm(ui: &mut Ui, title: &str, body: &str, verb: &str) -> Option<bool> {
    let t = Tokens::of(ui);
    ui.set_width(DIALOG_W);
    ui.label(egui::RichText::new(title).font(FontId::new(15.0, semibold())).color(t.text));
    ui.add_space(6.0);
    ui.add(egui::Label::new(egui::RichText::new(body).size(12.0).color(t.muted)).wrap());
    ui.add_space(18.0);
    let enter = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    let mut answer = None;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if super::danger(ui, verb).clicked() || enter {
                answer = Some(true);
            }
            ui.add_space(4.0);
            if super::secondary(ui, "Cancel").clicked() {
                answer = Some(false);
            }
        });
    });
    answer
}

/// One group in the [`shortcuts_sheet`]: its name, and each row's
/// description and the keys that do it, as the platform's keys are written
/// before [`crate::keys::shown`].
pub struct ShortcutGroup<'a> {
    pub name: &'a str,
    pub rows: Vec<(&'a str, &'a [&'a str])>,
}

/// A shortcuts column's width, and the room between columns.
const SHORTCUT_COL_W: f32 = 260.0;
const SHORTCUT_GAP: f32 = 32.0;
/// A shortcut row's height.
const SHORTCUT_ROW_H: f32 = 22.0;

/// The keyboard shortcuts (⌘/): `groups` in balanced columns, as many as
/// the window has room for within `columns`. It's sized to fit; only a
/// really short window scrolls, with a soft fade at the bottom. With
/// `close_button` its heading has a × (as [`sheet_header`]); without, a quiet
/// "esc" says how to close it. True when the × was clicked.
pub fn shortcuts_sheet(ui: &mut Ui, groups: &[ShortcutGroup<'_>], columns: std::ops::RangeInclusive<usize>, close_button: bool) -> bool {
    let t = Tokens::of(ui);
    let screen = ui.ctx().content_rect();
    let fits = |n: usize| screen.width() >= SHORTCUT_COL_W * n as f32 + SHORTCUT_GAP * (n - 1) as f32 + 120.0;
    let cols = columns.clone().rev().find(|&n| n > 0 && fits(n)).unwrap_or(*columns.start()).max(1);
    let width = SHORTCUT_COL_W * cols as f32 + SHORTCUT_GAP * (cols - 1) as f32;
    ui.set_width(width);
    let mut close = false;
    if close_button {
        close = sheet_header(ui, "Keyboard shortcuts");
    } else {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Keyboard shortcuts").font(FontId::new(15.0, semibold())).color(t.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new("esc").size(11.0).color(t.muted));
            });
        });
    }
    ui.add_space(16.0);
    let split = balance(groups, cols);
    let column = |ui: &mut Ui, groups: &[&ShortcutGroup<'_>]| {
        ui.vertical(|ui| {
            ui.set_width(SHORTCUT_COL_W);
            for (gi, group) in groups.iter().enumerate() {
                if gi > 0 {
                    ui.add_space(16.0);
                }
                super::section(ui, group.name);
                ui.add_space(6.0);
                for (label, keys) in &group.rows {
                    ui.horizontal(|ui| {
                        ui.set_height(SHORTCUT_ROW_H);
                        ui.label(egui::RichText::new(*label).size(13.0).color(t.text));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for k in keys.iter().rev() {
                                super::key_cap(ui, &crate::keys::shown(k));
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
    let tallest = split.iter().map(|c| c.iter().map(|g| group_height(g)).sum::<f32>()).fold(0.0, f32::max);
    let out = egui::ScrollArea::vertical().max_height(max_h).min_scrolled_height(tallest.min(max_h)).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            for (k, groups) in split.iter().enumerate() {
                if k > 0 {
                    ui.add_space(SHORTCUT_GAP);
                }
                column(ui, groups);
            }
        });
    });
    fade_below(ui, &out);
    close
}

/// When a scroll area has more below, its last rows fade into the sheet, so
/// it's plain there's more.
pub fn fade_below<R>(ui: &Ui, out: &egui::scroll_area::ScrollAreaOutput<R>) {
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

/// A group's height in the sheet: its rows plus header and gap.
fn group_height(group: &ShortcutGroup<'_>) -> f32 {
    group.rows.len() as f32 * (SHORTCUT_ROW_H + 8.0) + 44.0
}

/// Splits `groups` (in order) into `cols` columns of roughly equal height.
fn balance<'g, 'a>(groups: &'g [ShortcutGroup<'a>], cols: usize) -> Vec<Vec<&'g ShortcutGroup<'a>>> {
    let total: f32 = groups.iter().map(group_height).sum();
    let target = total / cols as f32;
    let mut out: Vec<Vec<&ShortcutGroup<'a>>> = vec![Vec::new()];
    let mut acc = 0.0;
    for (i, g) in groups.iter().enumerate() {
        let h = group_height(g);
        let left = groups.len() - i;
        let open = cols - out.len();
        // Start a new column when this one is full, or when every remaining
        // group needs a column of its own.
        if !out.last().is_some_and(Vec::is_empty) && open > 0 && (acc + h / 2.0 > target || left <= open) {
            out.push(Vec::new());
            acc = 0.0;
        }
        out.last_mut().expect("starts with a column").push(g);
        acc += h;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn groups(sizes: &[usize]) -> Vec<ShortcutGroup<'static>> {
        const KEYS: &[&str] = &["⌘K"];
        sizes.iter().map(|&n| ShortcutGroup { name: "Group", rows: vec![("Row", KEYS); n] }).collect()
    }

    #[test]
    fn columns_keep_every_group_in_order() {
        let all = groups(&[5, 3, 6, 2, 4, 3, 7]);
        for cols in [1, 2, 3] {
            let split = balance(&all, cols);
            assert!(split.len() <= cols && split.iter().all(|c| !c.is_empty()));
            let flat: Vec<*const ShortcutGroup<'_>> = split.into_iter().flatten().map(|g| g as *const _).collect();
            let order: Vec<*const ShortcutGroup<'_>> = all.iter().map(|g| g as *const _).collect();
            assert_eq!(flat, order);
        }
    }

    #[test]
    fn a_column_each_when_there_are_as_many_columns_as_groups() {
        let all = groups(&[9, 1, 1]);
        assert_eq!(balance(&all, 3).iter().map(Vec::len).collect::<Vec<_>>(), [1, 1, 1]);
    }
}
