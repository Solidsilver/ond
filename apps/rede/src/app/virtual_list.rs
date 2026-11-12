//! The scroll area under the library: a grid of tiles, of which only the rows
//! on screen are laid out. A library of a few thousand books costs a screenful
//! per frame, and only the covers in view are asked for.

use egui::Ui;
use std::hash::Hash;
use std::ops::Range;

/// A list of `rows` rows, each `row_h` tall (without spacing), with `per_row`
/// entries in a row.
pub struct VirtualList<L> {
    /// Names the list: its scroll area.
    list: L,
    row_h: f32,
    rows: usize,
}

impl<L: Hash + std::fmt::Debug + Copy> VirtualList<L> {
    /// `rows` rows of `row_h`, in the ui's own spacing.
    pub fn grid(list: L, row_h: f32, rows: usize) -> Self {
        Self { list, row_h, rows }
    }

    /// Lays out the rows on screen, `body` drawing them (it's given the range
    /// of rows).
    pub fn show(self, ui: &mut Ui, body: impl FnOnce(&mut Ui, Range<usize>)) {
        egui::ScrollArea::vertical().id_salt(self.list).auto_shrink(false).show_rows(ui, self.row_h, self.rows, |ui, range| body(ui, range));
    }
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
}
