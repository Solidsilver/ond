//! The header over a table (Songs, and Albums and Artists as lists): each
//! column's label, which sorts by that column when clicked (and turns the
//! order around when it's sorted by it already), the sorted one marked with
//! an arrow.

use crate::theme::{Tokens, semibold};
use egui::{Align2, FontId, Rect, Sense, Stroke, Ui, pos2};

/// The header's height.
pub const HEAD_H: f32 = 26.0;
/// Text starts this far in from a cell's left edge (or ends this far in from
/// its right).
const INSET: f32 = 10.0;

/// One column's header: its label and place, whether it's right-aligned, and
/// what clicking it sorts by (`None`: it doesn't sort).
pub struct Head<'a, C> {
    pub label: &'a str,
    pub rect: Rect,
    pub right: bool,
    pub sorts: Option<C>,
}

/// Draws `heads`, with a hairline under `area`. A click sorts by the clicked
/// column in its usual order, or turns that order around if it's sorted by
/// it already. Says whether the sort changed.
pub fn header<C: Copy + PartialEq>(ui: &mut Ui, area: Rect, id: &str, heads: &[Head<'_, C>], sort: &mut C, descending: &mut bool) -> bool {
    let t = Tokens::of(ui);
    let mut changed = false;
    for h in heads {
        let on = h.sorts == Some(*sort);
        let resp = h.sorts.map(|_| ui.interact(h.rect, ui.scope_id().with((id, h.label)), Sense::click()));
        let hovered = resp.as_ref().is_some_and(|r| r.hovered());
        let color = if on || hovered { t.text } else { t.muted };
        let font = FontId::new(11.0, semibold());
        let (anchor, x) = if h.right { (Align2::RIGHT_CENTER, h.rect.right() - INSET) } else { (Align2::LEFT_CENTER, h.rect.left() + INSET) };
        let text = ui.painter().text(pos2(x, h.rect.center().y), anchor, h.label, font, color);
        if on {
            let cx = if h.right { text.left() - 8.0 } else { text.right() + 8.0 };
            sort_arrow(ui, pos2(cx, h.rect.center().y), *descending, color);
        }
        let (Some(resp), Some(by)) = (resp, h.sorts) else { continue };
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, format!("Sort by {}", h.label)));
        if resp.clicked() {
            if on {
                *descending = !*descending;
            } else {
                *sort = by;
                *descending = false;
            }
            changed = true;
        }
    }
    ui.painter().hline(area.x_range(), area.bottom(), Stroke::new(1.0, t.hairline));
    changed
}

fn sort_arrow(ui: &Ui, at: egui::Pos2, descending: bool, color: egui::Color32) {
    let (w, h) = (3.5, 2.5);
    let pts = if descending {
        vec![pos2(at.x - w, at.y - h), pos2(at.x + w, at.y - h), pos2(at.x, at.y + h)]
    } else {
        vec![pos2(at.x - w, at.y + h), pos2(at.x + w, at.y + h), pos2(at.x, at.y - h)]
    };
    ui.painter().add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
}
