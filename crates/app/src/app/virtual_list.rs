//! The scroll area under every long list (Songs, the album and artist tables
//! and grids): only the rows on screen are laid out, it reaches into the
//! gutter, and the alphabet rail sits beside it when the list is sorted by
//! name. Each page draws its own rows; this is the frame around them.

use super::alpha::{self, Marks};
use egui::{Margin, Rect, Ui, UiBuilder};
use std::hash::Hash;
use std::ops::Range;

/// A list of `rows` rows, each `row_h` tall (without spacing), with
/// `per_row` entries in a row: one in a table, the columns in a grid.
pub struct VirtualList<'a, L> {
    /// Names the list: its scroll area, and where the rail's jumps go.
    list: L,
    marks: Option<&'a Marks>,
    row_h: f32,
    rows: usize,
    per_row: usize,
    /// Rows sit flush (a table): no spacing between them.
    flush: bool,
}

impl<'a, L: Hash + std::fmt::Debug + Copy> VirtualList<'a, L> {
    /// A table: an entry per row, the rows flush.
    pub fn table(list: L, marks: Option<&'a Marks>, row_h: f32, rows: usize) -> Self {
        Self { list, marks, row_h, rows, per_row: 1, flush: true }
    }

    /// A grid: `cols` entries per row, the ui's own spacing between rows.
    pub fn grid(list: L, marks: Option<&'a Marks>, row_h: f32, rows: usize, cols: usize) -> Self {
        Self { list, marks, row_h, rows, per_row: cols, flush: false }
    }

    /// Lays out the rows on screen, `body` drawing them (it's given the
    /// range of rows), then the rail when there are marks.
    pub fn show(self, ui: &mut Ui, body: impl FnOnce(&mut Ui, Range<usize>)) {
        super::layout::into_gutter(ui, |ui| {
            if self.flush {
                ui.spacing_mut().item_spacing.y = 0.0;
            }
            let area = ui.available_rect_before_wrap();
            let row_spacing = ui.spacing().item_spacing.y;
            let mut scroll = egui::ScrollArea::vertical().id_salt(self.list).auto_shrink(false).content_margin(margin(self.marks.is_some()));
            if let Some(y) = alpha::take_jump(ui, self.list) {
                scroll = scroll.vertical_scroll_offset(y);
            }
            let mut top = 0;
            let out = scroll.show_rows(ui, self.row_h, self.rows, |ui, range| {
                top = range.start * self.per_row;
                body(ui, range);
            });
            if let Some(marks) = self.marks {
                let per_row = self.per_row;
                alpha::rail(ui, self.list, area, &out, marks, top, |i| i / per_row, self.row_h + row_spacing);
            }
        });
    }
}

/// For a list whose lines differ in height (the queue: group headings,
/// tracks, gaps), inside a scroll area's `show_viewport`: sizes the content
/// for every line, then lays out only those in `viewport`, calling `draw`
/// with each one's index. Lines are laid out from the first one shown, so
/// their auto ids shift as the list scrolls: give each one its own
/// (`push_id`).
pub fn visible_lines(ui: &mut Ui, viewport: Rect, heights: &[f32], mut draw: impl FnMut(&mut Ui, usize)) {
    let top = ui.max_rect().top();
    let total: f32 = heights.iter().sum();
    ui.set_height(total);
    // Where the first line on screen starts.
    let mut y = 0.0;
    let mut first = heights.len();
    for (k, &h) in heights.iter().enumerate() {
        if y + h > viewport.min.y {
            first = k;
            break;
        }
        y += h;
    }
    let shown = Rect::from_x_y_ranges(ui.max_rect().x_range(), top + y..=top + total);
    ui.scope_builder(UiBuilder::new().max_rect(shown), |ui| {
        for (k, &h) in heights.iter().enumerate().skip(first) {
            if y >= viewport.max.y {
                break;
            }
            draw(ui, k);
            y += h;
        }
    });
}

/// The scroll area's content margin: the gutter, and room for the rail when
/// there is one.
pub fn margin(rail: bool) -> Margin {
    if rail { alpha::margin() } else { super::layout::gutter() }
}

/// The rows just past either end of `shown` (up to `n` each side, within
/// `rows`), above first: what to load ahead so scrolling finds it ready.
pub fn near(shown: &Range<usize>, n: usize, rows: usize) -> impl Iterator<Item = usize> {
    (shown.start.saturating_sub(n)..shown.start).chain(shown.end..(shown.end + n).min(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_rows_stop_at_either_end_of_the_list() {
        assert_eq!(near(&(0..5), 2, 10).collect::<Vec<_>>(), [5, 6]);
        assert_eq!(near(&(4..8), 2, 10).collect::<Vec<_>>(), [2, 3, 8, 9]);
        assert_eq!(near(&(8..10), 2, 10).collect::<Vec<_>>(), [6, 7]);
    }

    #[test]
    fn only_the_lines_on_screen_are_laid_out_each_in_its_place() {
        // Headings among tracks, as in the queue.
        let heights: Vec<f32> = (0..1000).map(|k| if k % 10 == 0 { 26.0 } else { 44.0 }).collect();
        let starts: Vec<f32> = heights.iter().scan(0.0, |y, h| Some(std::mem::replace(y, *y + h))).collect();
        let ctx = egui::Context::default();
        let mut drawn = Vec::new();
        for _ in 0..2 {
            drawn.clear();
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 800.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                let scroll = egui::ScrollArea::vertical().max_height(300.0).vertical_scroll_offset(1000.0);
                scroll.show_viewport(ui, |ui, viewport| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let top = ui.max_rect().top();
                    visible_lines(ui, viewport, &heights, |ui, k| {
                        drawn.push((k, ui.cursor().top() - top));
                        ui.allocate_exact_size(egui::vec2(10.0, heights[k]), egui::Sense::hover());
                    });
                });
            });
            // No renderer here to upload textures to.
            out.textures_delta.clear();
        }
        let (first, at) = drawn[0];
        assert!(starts[first] <= 1000.0 && 1000.0 < starts[first] + heights[first], "first line {first} starts at {}", starts[first]);
        assert!(drawn.len() < 12, "{} lines laid out", drawn.len());
        for &(k, y) in &drawn {
            assert_eq!(y, starts[k], "line {k}");
        }
        assert_eq!(at, starts[first]);
        assert!(starts[drawn.last().unwrap().0] < 1300.0);
    }
}
