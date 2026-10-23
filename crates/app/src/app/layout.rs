//! The full window's columns: how wide the sidebar and the queue column are
//! (the user drags their inner edges), and which of them fit at a given
//! window width. The content between them always keeps room to breathe.

use crate::theme::Tokens;
use egui::{CursorIcon, Id, Rect, Sense, Stroke, Ui, pos2};

pub const LEFT_DEFAULT: f32 = 216.0;
pub const LEFT_MIN: f32 = 180.0;
pub const LEFT_MAX: f32 = 360.0;
pub const RIGHT_DEFAULT: f32 = 320.0;
pub const RIGHT_MIN: f32 = 280.0;
pub const RIGHT_MAX: f32 = 520.0;
/// The content keeps at least this much with the queue docked beside it…
const DOCKED_CONTENT_MIN: f32 = 564.0;
/// …and this much before the sidebar gives way to the icon rail.
const RAIL_CONTENT_MIN: f32 = 604.0;

/// The column widths the user chose (clamped to their ranges).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Columns {
    pub left: f32,
    pub right: f32,
}

impl Default for Columns {
    fn default() -> Self {
        Self { left: LEFT_DEFAULT, right: RIGHT_DEFAULT }
    }
}

impl Columns {
    /// From saved widths; anything missing or out of range is mended.
    pub fn restore(left: Option<f32>, right: Option<f32>) -> Self {
        Self { left: clamp_left(left.unwrap_or(LEFT_DEFAULT)), right: clamp_right(right.unwrap_or(RIGHT_DEFAULT)) }
    }

    /// What fits in a window `width` wide.
    pub fn plan(&self, width: f32) -> Plan {
        let rail = width < self.left + RAIL_CONTENT_MIN;
        let left = if rail { None } else { Some(self.left) };
        let queue_docked = width >= self.left + self.right + DOCKED_CONTENT_MIN;
        Plan { left, queue_docked }
    }

    /// The widest the sidebar may get by dragging in a window `width` wide,
    /// keeping a docked queue docked.
    pub fn left_limit(&self, width: f32, queue_docked: bool) -> f32 {
        let others = if queue_docked { self.right + DOCKED_CONTENT_MIN } else { RAIL_CONTENT_MIN };
        clamp_left((width - others).max(LEFT_MIN))
    }

    /// The widest the queue column may get by dragging, staying docked.
    pub fn right_limit(&self, width: f32) -> f32 {
        clamp_right((width - self.left - DOCKED_CONTENT_MIN).max(RIGHT_MIN))
    }
}

/// The columns shown at one window width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan {
    /// The full sidebar's width, or `None` for the icon rail.
    pub left: Option<f32>,
    /// The queue sits beside the content (else it's a sheet over it).
    pub queue_docked: bool,
}

pub fn clamp_left(w: f32) -> f32 {
    w.clamp(LEFT_MIN, LEFT_MAX)
}

pub fn clamp_right(w: f32) -> f32 {
    w.clamp(RIGHT_MIN, RIGHT_MAX)
}

/// What happened at a column edge this frame.
#[derive(Debug, Default, Clone, Copy)]
pub struct Drag {
    /// Horizontal pointer movement while dragging.
    pub dx: f32,
    /// Let go after dragging: time to save the width.
    pub released: bool,
    /// Double-clicked: back to the default width.
    pub reset: bool,
}

/// A draggable column edge at `x`, from `top` to `bottom`: a few points wide,
/// a resize cursor, and a hairline that lights up on hover.
pub fn edge_handle(ui: &Ui, id: Id, x: f32, top: f32, bottom: f32) -> Drag {
    let rect = Rect::from_x_y_ranges(x - 3.0..=x + 3.0, top..=bottom);
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let active = resp.hovered() || resp.dragged();
    if active {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        let t = Tokens::of(ui);
        let color = if resp.dragged() { t.accent.gamma_multiply(0.8) } else { t.muted.gamma_multiply(0.6) };
        // Painted after the columns, so it lies over their edge.
        ui.painter().line_segment([pos2(x, top), pos2(x, bottom)], Stroke::new(2.0, color));
    }
    Drag { dx: if resp.dragged() { resp.drag_delta().x } else { 0.0 }, released: resp.drag_stopped(), reset: resp.double_clicked() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_widths_are_mended() {
        assert_eq!(Columns::restore(None, None), Columns::default());
        let c = Columns::restore(Some(50.0), Some(9_000.0));
        assert_eq!((c.left, c.right), (LEFT_MIN, RIGHT_MAX));
        let c = Columns::restore(Some(250.0), Some(400.0));
        assert_eq!((c.left, c.right), (250.0, 400.0));
    }

    #[test]
    fn default_widths_keep_the_old_breakpoints() {
        let c = Columns::default();
        assert!(c.plan(1100.0).queue_docked);
        assert!(!c.plan(1099.0).queue_docked);
        assert_eq!(c.plan(820.0).left, Some(LEFT_DEFAULT));
        assert_eq!(c.plan(819.0).left, None);
    }

    #[test]
    fn wider_columns_give_way_sooner() {
        let c = Columns { left: 300.0, right: 480.0 };
        assert!(!c.plan(1300.0).queue_docked);
        assert!(c.plan(1344.0).queue_docked);
        assert_eq!(c.plan(903.0).left, None);
    }

    #[test]
    fn dragging_never_squeezes_the_content() {
        let c = Columns::default();
        // 1200 wide: the queue may only grow until the content hits its minimum.
        assert_eq!(c.right_limit(1200.0), 1200.0 - LEFT_DEFAULT - DOCKED_CONTENT_MIN);
        assert_eq!(c.right_limit(3000.0), RIGHT_MAX);
        assert_eq!(c.right_limit(900.0), RIGHT_MIN);
        assert_eq!(c.left_limit(1200.0, true), LEFT_MIN.max(1200.0 - RIGHT_DEFAULT - DOCKED_CONTENT_MIN));
        assert_eq!(c.left_limit(3000.0, false), LEFT_MAX);
    }
}
