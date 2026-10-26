//! The alphabet rail: beside a long list sorted by name, the letters its
//! entries start with, top to bottom in the list's own order (so a list
//! sorted Z to A reads Z to A). Clicking a letter, or dragging along the rail,
//! jumps the list to the first entry under it; the letter of the entry at the
//! top of the list is lit.

use crate::theme::{Tokens, semibold};
use egui::scroll_area::ScrollAreaOutput;
use egui::{Align2, CursorIcon, FontId, Id, Rect, Sense, Stroke, Ui, pos2, vec2};
use std::hash::Hash;

/// How much narrower the list gets for the rail. The letters sit mostly in
/// the gutter, just left of the scroll bar, with the gap on the list's side.
pub const RAIL_W: f32 = 12.0;
/// The letters' centre, in from the scroll area's right edge, and how wide
/// the strip that takes the pointer is (clear of the scroll bar).
const RAIL_X: f32 = 15.0;
const HIT_W: f32 = 14.0;
/// Letters sit this far apart at most; closer than `MIN_STEP`, only some are
/// spelled out and the rest are dots (dragging still reaches every one).
const MAX_STEP: f32 = 17.0;
const MIN_STEP: f32 = 13.0;
/// Fewer letters than this and there's nothing to jump between.
const MIN_MARKS: usize = 3;
/// What letters of other scripts file under; drawn as a dot, never as text.
const OTHER: char = '~';

/// The letter a sort key files under: A to Z, `#` for digits and symbols,
/// [`OTHER`] for letters of other scripts. Case and accents don't count
/// ("Émile" is an E).
pub fn letter(key: &str) -> char {
    let Some(c) = key.trim_start().chars().next() else { return '#' };
    match library::fold(c.encode_utf8(&mut [0; 4])).chars().next() {
        Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase(),
        Some(c) if c.is_alphabetic() => OTHER,
        _ => '#',
    }
}

/// Where each letter starts in a list: the letter and the index of its first
/// entry, in the list's order.
#[derive(Debug, Default, PartialEq)]
pub struct Marks(Vec<(char, usize)>);

impl Marks {
    /// From each entry's sort key, in the list's order. A letter met again
    /// further down (a key out of order) keeps its first place.
    pub fn new<S: AsRef<str>>(keys: impl IntoIterator<Item = S>) -> Self {
        let mut marks: Vec<(char, usize)> = Vec::new();
        for (i, key) in keys.into_iter().enumerate() {
            let c = letter(key.as_ref());
            if marks.last().is_none_or(|m| m.0 != c) && !marks.iter().any(|m| m.0 == c) {
                marks.push((c, i));
            }
        }
        Self(marks)
    }

    /// Which mark the entry at `i` falls under.
    fn under(&self, i: usize) -> Option<usize> {
        self.0.partition_point(|m| m.1 <= i).checked_sub(1)
    }
}

/// The scroll area's margin: the gutter (see `layout`) and room for the rail.
pub fn margin() -> egui::Margin {
    let mut m = super::layout::gutter();
    m.right += RAIL_W as i8;
    m
}

fn jump_id(list: impl Hash + std::fmt::Debug) -> Id {
    Id::unique("alpha-jump").with(list)
}

/// Where the rail last asked `list` to scroll to: give it to the list's
/// scroll area (`vertical_scroll_offset`) as it's built.
pub fn take_jump(ui: &Ui, list: impl Hash + std::fmt::Debug) -> Option<f32> {
    ui.data_mut(|d| d.remove_temp(jump_id(list)))
}

/// Draws the rail beside `list`, shown with [`margin`] in `area` (the rect its
/// scroll area was given), with `top` the entry at the top of the list, and
/// while it's clicked or dragged along asks the list to jump (see
/// [`take_jump`]). `row_of` turns an entry into the row it's in, rows `row_h`
/// apart (with spacing).
#[expect(clippy::too_many_arguments)]
pub fn rail<R>(ui: &mut Ui, list: impl Hash + std::fmt::Debug, area: Rect, out: &ScrollAreaOutput<R>, marks: &Marks, top: usize, row_of: impl Fn(usize) -> usize, row_h: f32) {
    let n = marks.0.len();
    // Nothing to jump between, or nowhere to scroll.
    if n < MIN_MARKS || out.content_size.y <= out.inner_rect.height() {
        return;
    }
    let t = Tokens::of(ui);
    let step = ((out.inner_rect.height() - 16.0) / n as f32).min(MAX_STEP);
    let column = Rect::from_center_size(pos2(area.right() - RAIL_X, out.inner_rect.center().y), vec2(HIT_W, step * n as f32));
    let resp = ui.interact(column, ui.scope_id().with("alpha-rail"), Sense::click_and_drag()).on_hover_cursor(CursorIcon::PointingHand);
    let pointed = resp.hover_pos().or(resp.interact_pointer_pos()).map(|p| (((p.y - column.top()) / step).floor().max(0.0) as usize).min(n - 1));
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, "Jump to letter"));
    let held = resp.is_pointer_button_down_on();
    let lit = marks.under(top);
    let painter = ui.painter();
    // Tight on room: every `every`th letter spelled out, dots between.
    let every = (MIN_STEP / step).ceil().max(1.0) as usize;
    for (k, &(c, _)) in marks.0.iter().enumerate() {
        let at = pos2(column.center().x, column.top() + step * (k as f32 + 0.5));
        let color = if Some(k) == lit {
            t.accent
        } else if resp.hovered() && Some(k) == pointed {
            t.text
        } else {
            t.muted
        };
        if c == OTHER {
            painter.circle_filled(at, 2.5, color);
        } else if k % every == 0 {
            painter.text(at, Align2::CENTER_CENTER, c, FontId::new(10.5, semibold()), color);
        } else {
            painter.circle_filled(at, 1.5, color);
        }
    }
    let (true, Some(k)) = (held, pointed) else { return };
    // The letter under the pointer, large, beside the rail.
    let y = (column.top() + step * (k as f32 + 0.5)).clamp(out.inner_rect.top() + 24.0, out.inner_rect.bottom() - 24.0);
    let bubble = Rect::from_center_size(pos2(column.left() - 34.0, y), vec2(44.0, 44.0));
    painter.rect(bubble, 10.0, t.field, Stroke::new(1.0, t.hairline), egui::StrokeKind::Inside);
    match marks.0[k].0 {
        OTHER => {
            painter.circle_filled(bubble.center(), 5.0, t.text);
        }
        c => {
            painter.text(bubble.center(), Align2::CENTER_CENTER, c, FontId::new(22.0, semibold()), t.text);
        }
    }
    let to = row_of(marks.0[k].1) as f32 * row_h;
    if (out.state.clamped_offset().y - to).abs() > 0.5 {
        ui.data_mut(|d| d.insert_temp(jump_id(list), to));
        ui.ctx().request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_ignore_case_and_accents() {
        assert_eq!(letter("abba"), 'A');
        assert_eq!(letter("Émile"), 'E');
        assert_eq!(letter("Øystein"), 'O');
        assert_eq!(letter("12 Ensemble"), '#');
        assert_eq!(letter("(Www.ECRD.Com)"), '#');
        assert_eq!(letter("“Heroes”"), '#');
        assert_eq!(letter("坂本龍一"), OTHER);
        assert_eq!(letter(""), '#');
    }

    #[test]
    fn marks_are_where_each_letter_starts() {
        let m = Marks::new(["12 ensemble", "2 chainz", "a-ha", "abba", "bjork", "Björk", "zz top"]);
        assert_eq!(m.0, [('#', 0), ('A', 2), ('B', 4), ('Z', 6)]);
        assert_eq!((m.under(0), m.under(3), m.under(5), m.under(9)), (Some(0), Some(1), Some(2), Some(3)));
        // Z to A reads Z to A; a stray key out of order keeps the first place.
        let m = Marks::new(["zz top", "bjork", "abba", "beck"]);
        assert_eq!(m.0, [('Z', 0), ('B', 1), ('A', 2)]);
        assert_eq!(Marks::new(Vec::<String>::new()).0, []);
    }
}
