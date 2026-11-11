//! Figures drawn as pictures, for a page that counts things: a [`card`] to
//! set each part apart (with its [`card_heading`]), a [`stat_tile`]
//! (a figure in large type, with what it is above and a quiet line under),
//! a [`bar_chart`] (one bar per bucket, the busiest in the accent at full
//! strength), a [`level_grid`] (cells shaded by how much each holds, as a
//! calendar of days is), and a [`share_bar`] (how one amount compares with
//! the largest).
//!
//! The kit knows nothing of what is counted: the caller gives the values, the
//! words each bar or cell says when hovered, and the name a screen reader
//! hears for the whole. Painted only, with nothing kept between frames;
//! hovering shows a tooltip and asks for no repaints of its own.

use crate::icons::{self, Icon};
use crate::theme::{Tokens, semibold};
use egui::{Color32, FontId, Rect, Response, Sense, Stroke, Ui, pos2, vec2};

/// A tile's height.
pub const STAT_TILE_H: f32 = 96.0;
const TILE_PAD: f32 = 16.0;
/// The corners of a card and a tile.
pub const CARD_RADIUS: f32 = 12.0;
/// Room inside a card's edge.
pub const CARD_PAD: f32 = 20.0;

/// What a card and a tile are filled with: a step off the page towards a
/// field, so what's drawn on a card in the field's colour (an empty cell, a
/// bar's track) still shows.
fn card_fill(t: &Tokens) -> Color32 {
    t.bg.lerp_to_gamma(t.field, 0.4)
}

/// A panel `width` wide that sets one part of a page apart from the next,
/// at least `min_height` tall (so cards side by side come out level).
/// `body` gets the width inside.
pub fn card<R>(ui: &mut Ui, width: f32, min_height: f32, body: impl FnOnce(&mut Ui, f32) -> R) -> R {
    let t = Tokens::of(ui);
    let inner = (width - 2.0 * (CARD_PAD + 1.0)).max(0.0);
    egui::Frame::new()
        .fill(card_fill(&t))
        .stroke(Stroke::new(1.0, t.hairline))
        .corner_radius(CARD_RADIUS)
        .inner_margin(CARD_PAD)
        .show(ui, |ui| {
            ui.set_width(inner);
            ui.set_min_height((min_height - 2.0 * (CARD_PAD + 1.0)).max(0.0));
            body(ui, inner)
        })
        .inner
}

/// The height a [`card`] adds around what's in it.
pub const CARD_CHROME: f32 = 2.0 * (CARD_PAD + 1.0);

/// A card's title, with a quiet line at its right (what the card comes to:
/// "Most on Tuesdays") when there's room for both.
pub fn card_heading(ui: &mut Ui, title: &str, note: Option<&str>) {
    let t = Tokens::of(ui);
    let width = ui.available_width();
    let title = ui.painter().layout_no_wrap(title.to_owned(), FontId::new(15.0, semibold()), t.text);
    let (rect, _) = ui.allocate_exact_size(vec2(width, CARD_HEADING_H), Sense::hover());
    let title_w = title.size().x;
    ui.painter().galley(pos2(rect.left(), rect.top()), title, t.text);
    if let Some(note) = note {
        let room = width - title_w - 16.0;
        if room > 40.0 {
            let note = super::elide(ui, note, FontId::proportional(12.5), t.muted, room);
            ui.painter().galley(pos2(rect.right() - note.size().x, rect.top() + 2.0), note, t.muted);
        }
    }
}

/// The room a [`card_heading`] takes, the space under it included.
pub const CARD_HEADING_H: f32 = 32.0;

/// What a [`stat_tile`] says.
pub struct Stat<'a> {
    /// What the figure is: "Listened", "Streak".
    pub label: &'a str,
    /// The figure itself, large.
    pub value: &'a str,
    /// A quiet line under it.
    pub note: Option<&'a str>,
    /// A small mark in the accent at the tile's corner.
    pub icon: Option<Icon>,
}

/// A figure on a tile `width` wide: what it is, the figure, a note under.
pub fn stat_tile(ui: &mut Ui, width: f32, stat: &Stat) -> Response {
    let t = Tokens::of(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, STAT_TILE_H), Sense::hover());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect(rect, CARD_RADIUS, card_fill(&t), Stroke::new(1.0, t.hairline), egui::StrokeKind::Inside);
        let room = rect.width() - 2.0 * TILE_PAD;
        let x = rect.left() + TILE_PAD;
        const ICON: f32 = 14.0;
        let label_room = if stat.icon.is_some() { room - ICON - 8.0 } else { room };
        let label = super::elide(ui, stat.label, FontId::new(12.0, semibold()), t.muted, label_room);
        let value = super::elide(ui, stat.value, FontId::new(26.0, semibold()), t.text, room);
        p.galley(pos2(x, rect.top() + TILE_PAD), label, t.muted);
        if let Some(icon) = stat.icon {
            let at = Rect::from_min_size(pos2(rect.right() - TILE_PAD - ICON, rect.top() + TILE_PAD), vec2(ICON, ICON));
            icons::paint(p, at, icon, t.accent);
        }
        p.galley(pos2(x, rect.top() + TILE_PAD + 18.0), value, t.text);
        if let Some(note) = stat.note {
            let note = super::elide(ui, note, FontId::proportional(12.0), t.muted, room);
            p.galley(pos2(x, rect.bottom() - TILE_PAD - note.size().y), note, t.muted);
        }
    }
    let name = match stat.note {
        Some(note) => format!("{}: {}, {note}", stat.label, stat.value),
        None => format!("{}: {}", stat.label, stat.value),
    };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &name));
    resp
}

/// Tiles across `width` with `gap` between them: as many as fit at `min`
/// wide, at most `count`, and how wide each then is.
pub fn tile_columns(width: f32, min: f32, gap: f32, count: usize) -> (usize, f32) {
    let fit = (((width + gap) / (min + gap)).floor() as usize).clamp(1, count.max(1));
    // Rows that come out even: four tiles go two by two rather than three
    // and one.
    let cols = if count > fit && fit > 1 && count.is_multiple_of(2) && fit % 2 == 1 { fit - 1 } else { fit };
    (cols, (width - gap * (cols - 1) as f32) / cols as f32)
}

/// What a [`bar_chart`] draws.
pub struct Bars<'a> {
    pub values: &'a [f64],
    /// A label under some bars (`None` for the rest, so a dense chart isn't
    /// crowded).
    pub label: &'a dyn Fn(usize) -> Option<String>,
    /// What hovering a bar says.
    pub tip: &'a dyn Fn(usize) -> String,
    /// What a screen reader hears for the whole chart.
    pub name: &'a str,
}

/// Room under the bars for their labels.
const LABEL_H: f32 = 18.0;

/// Bars across `width`, the tallest `height` high, labels under them. The
/// busiest bar is in the accent at full strength, the others quieter; a bar
/// with nothing is a hairline, so the scale's foot still shows.
pub fn bar_chart(ui: &mut Ui, width: f32, height: f32, bars: &Bars) -> Response {
    let t = Tokens::of(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height + LABEL_H), Sense::hover());
    let n = bars.values.len().max(1);
    let max = bars.values.iter().copied().fold(0.0, f64::max);
    let slot = rect.width() / n as f32;
    let gap = (slot * 0.25).clamp(1.0, 8.0);
    let hovered = resp.hover_pos().map(|p| (((p.x - rect.left()) / slot).floor() as usize).min(n - 1));
    if ui.is_rect_visible(rect) {
        let base = rect.top() + height;
        let p = ui.painter();
        for (i, v) in bars.values.iter().enumerate() {
            let x = rect.left() + slot * i as f32;
            let h = if max > 0.0 { (v / max) as f32 * height } else { 0.0 };
            let bar = Rect::from_min_max(pos2(x + gap / 2.0, base - h.max(2.0)), pos2(x + slot - gap / 2.0, base));
            let color = if hovered == Some(i) {
                t.text
            } else if *v <= 0.0 {
                t.hairline
            } else if *v >= max {
                t.accent
            } else {
                t.accent.gamma_multiply(0.55)
            };
            p.rect_filled(bar, (slot * 0.15).min(3.0), color);
            if let Some(label) = (bars.label)(i) {
                p.text(pos2(x + slot / 2.0, base + 4.0), egui::Align2::CENTER_TOP, label, FontId::proportional(10.5), t.muted);
            }
        }
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, bars.name));
    match hovered {
        Some(i) => resp.on_hover_text_at_pointer((bars.tip)(i)),
        None => resp,
    }
}

/// What a [`level_grid`] draws: `columns` of `rows` cells, filled down each
/// column first.
pub struct Grid<'a> {
    pub columns: usize,
    pub rows: usize,
    /// A cell's level, 0 (nothing) to [`LEVELS`] − 1, or `None` for a cell
    /// that isn't there (a day still to come).
    pub level: &'a dyn Fn(usize, usize) -> Option<u8>,
    pub tip: &'a dyn Fn(usize, usize) -> String,
    /// A label over some columns (a month's name over its first week).
    pub column_label: &'a dyn Fn(usize) -> Option<String>,
    /// A label beside some rows (a weekday's).
    pub row_label: &'a dyn Fn(usize) -> Option<String>,
    pub name: &'a str,
}

/// How many shades a [`level_grid`]'s cells come in, nothing among them.
pub const LEVELS: u8 = 5;
/// Room for the row labels at the left and the column labels on top.
const ROW_LABEL_W: f32 = 30.0;
const COLUMN_LABEL_H: f32 = 16.0;

/// The width a [`level_grid`] of `columns` takes with cells `cell` square and
/// `gap` apart, so a caller can fit as many columns as there's room for.
pub fn level_grid_width(columns: usize, cell: f32, gap: f32) -> f32 {
    ROW_LABEL_W + columns as f32 * (cell + gap) - gap
}

/// Cells `cell` square, `gap` apart, shaded by level from the field's colour
/// (nothing) to the accent (the most).
pub fn level_grid(ui: &mut Ui, cell: f32, gap: f32, grid: &Grid) -> Response {
    let t = Tokens::of(ui);
    let size = vec2(level_grid_width(grid.columns, cell, gap), COLUMN_LABEL_H + grid.rows as f32 * (cell + gap) - gap);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let origin = pos2(rect.left() + ROW_LABEL_W, rect.top() + COLUMN_LABEL_H);
    let step = cell + gap;
    let hovered = resp.hover_pos().and_then(|p| {
        let (c, r) = ((p.x - origin.x) / step, (p.y - origin.y) / step);
        (c >= 0.0 && r >= 0.0).then_some((c as usize, r as usize)).filter(|(c, r)| *c < grid.columns && *r < grid.rows && (grid.level)(*c, *r).is_some())
    });
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let font = FontId::proportional(10.5);
        for r in 0..grid.rows {
            if let Some(label) = (grid.row_label)(r) {
                p.text(pos2(rect.left(), origin.y + r as f32 * step + cell / 2.0), egui::Align2::LEFT_CENTER, label, font.clone(), t.muted);
            }
        }
        for c in 0..grid.columns {
            let x = origin.x + c as f32 * step;
            if let Some(label) = (grid.column_label)(c) {
                p.text(pos2(x, rect.top()), egui::Align2::LEFT_TOP, label, font.clone(), t.muted);
            }
            for r in 0..grid.rows {
                let Some(level) = (grid.level)(c, r) else { continue };
                let cell_rect = Rect::from_min_size(pos2(x, origin.y + r as f32 * step), vec2(cell, cell));
                p.rect_filled(cell_rect, cell * 0.22, shade(&t, level));
                if hovered == Some((c, r)) {
                    p.rect_stroke(cell_rect, cell * 0.22, egui::Stroke::new(1.5, t.text), egui::StrokeKind::Middle);
                }
            }
        }
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, grid.name));
    match hovered {
        Some((c, r)) => resp.on_hover_text_at_pointer((grid.tip)(c, r)),
        None => resp,
    }
}

/// "Less ▢▢▢▢▢ More": what a [`level_grid`]'s shades mean, at the right of
/// a row `width` wide (under the grid's right edge).
pub fn level_legend(ui: &mut Ui, cell: f32, width: f32) {
    let t = Tokens::of(ui);
    let font = FontId::proportional(10.5);
    let less = ui.painter().layout_no_wrap("Less".into(), font.clone(), t.muted);
    let more = ui.painter().layout_no_wrap("More".into(), font, t.muted);
    let (gap, step) = (5.0, cell + 3.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, cell.max(less.size().y)), Sense::hover());
    let p = ui.painter();
    let mut x = rect.right() - more.size().x;
    let y = rect.center().y;
    p.galley(pos2(x, y - more.size().y / 2.0), more, t.muted);
    x -= gap + f32::from(LEVELS) * step - 3.0;
    for level in 0..LEVELS {
        let r = Rect::from_min_size(pos2(x + f32::from(level) * step, y - cell / 2.0), vec2(cell, cell));
        p.rect_filled(r, cell * 0.22, shade(&t, level));
    }
    x -= gap + less.size().x;
    p.galley(pos2(x, y - less.size().y / 2.0), less, t.muted);
}

fn shade(t: &Tokens, level: u8) -> Color32 {
    match level {
        0 => t.field,
        l => t.accent.gamma_multiply(0.2 + 0.8 * f32::from(l.min(LEVELS - 1)) / f32::from(LEVELS - 1)),
    }
}

/// A thin bar `width` wide filled to `fraction`: how one amount in a list
/// compares with the largest.
pub fn share_bar(ui: &Ui, rect: Rect, fraction: f32) {
    let t = Tokens::of(ui);
    let p = ui.painter();
    p.rect_filled(rect, rect.height() / 2.0, t.field);
    let filled = Rect::from_min_size(rect.min, vec2(rect.width() * fraction.clamp(0.0, 1.0), rect.height()));
    if filled.width() > 0.0 {
        p.rect_filled(filled, rect.height() / 2.0, t.accent.gamma_multiply(0.75));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_fill_the_width_in_even_rows() {
        let (cols, w) = tile_columns(800.0, 150.0, 12.0, 4);
        assert_eq!(cols, 4);
        assert!((w * 4.0 + 36.0 - 800.0).abs() < 0.01);
        assert_eq!(tile_columns(500.0, 150.0, 12.0, 4).0, 2, "three would leave one on its own");
        assert_eq!(tile_columns(100.0, 150.0, 12.0, 4).0, 1);
        assert_eq!(tile_columns(2000.0, 150.0, 12.0, 4).0, 4, "never more columns than tiles");
    }
}
