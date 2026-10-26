//! One track's row in a list: the album page and playlists (with Favorites),
//! the Songs table and the queue. Each list lays its row out its own way;
//! the parts they share (the background, the playing mark, a title over its
//! artist, the length, the download mark, what a screen reader hears) are
//! drawn here, and where they sit is named in [`col`].

use super::widgets::{Mark, elide, row_fill};
use crate::icons::{self, Icon};
use crate::theme::{Tokens, semibold};
use egui::{Align2, Color32, FontId, Rect, Response, Sense, Ui, pos2, vec2};
use library::Track;

/// Where a row's parts sit, in points.
pub(super) mod col {
    /// Text starts this far in from a row's (or a cell's) left edge, and the
    /// length ends this far in from its right edge.
    pub const INSET: f32 = 10.0;
    /// A row's corner radius.
    pub const RADIUS: f32 = 6.0;
    /// The playing triangle before a title (no number column): its size, and
    /// the room it takes.
    pub const PLAYING: f32 = 8.0;
    pub const PLAYING_W: f32 = 16.0;
    /// The download mark's size.
    pub const MARK: f32 = 10.0;
    /// Between a title and what follows it on the line (a hint, the composer).
    pub const GAP: f32 = 10.0;

    /// The album page's track number column: its width, the number's right
    /// edge in it, and the playing triangle (a little larger) in its place.
    pub const NUMBER_W: f32 = 34.0;
    pub const NUMBER_RIGHT: f32 = 18.0;
    pub const NUMBER_PLAYING: f32 = 9.0;
    pub const NUMBER_PLAYING_X: f32 = 12.0;

    /// A track list's right end, measured in from the row's right edge: the
    /// download mark's centre, the heart's centre (and its box), where the
    /// text stops, and the hover actions' room left of that.
    pub const MARK_FROM_RIGHT: f32 = 48.0;
    pub const HEART_FROM_RIGHT: f32 = 70.0;
    pub const HEART_BOX: f32 = 24.0;
    pub const TEXT_FROM_RIGHT: f32 = 86.0;
    pub const ACTIONS_W: f32 = 150.0;
    /// The hover actions end this much short of the text's end.
    pub const ACTIONS_END: f32 = 84.0;

    /// The queue: the cover's inset from the row's left edge and its size;
    /// the gap before the length (or start time) at the right.
    pub const ART_INSET: f32 = 8.0;
    pub const ART: f32 = 32.0;
    pub const ART_RADIUS: f32 = 3.0;
    pub const RIGHT_GAP: f32 = 8.0;
}

/// A title's font in a list: semibold while it plays.
pub(super) fn title_font(size: f32, playing: bool) -> FontId {
    if playing { FontId::new(size, semibold()) } else { FontId::proportional(size) }
}

/// A song's length as the lists show it ("6:23"), empty when unknown.
pub(super) fn length(track: &Track) -> String {
    track.duration_secs.map(|d| crate::fmt::clock(f64::from(d))).unwrap_or_default()
}

/// One row: its place and response, and the shared parts drawn into it.
pub(super) struct TrackRow {
    pub rect: Rect,
    pub resp: Response,
}

impl TrackRow {
    /// A row `height` high across the room left.
    pub fn new(ui: &mut Ui, height: f32, sense: Sense) -> Self {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), sense);
        Self { rect, resp }
    }

    /// The pointer is on the row, or on something drawn over it (its heart,
    /// its buttons), which takes the hover from the row itself.
    pub fn pointer_on(&self) -> bool {
        self.resp.hovered() || self.resp.contains_pointer()
    }

    pub fn cy(&self) -> f32 {
        self.rect.center().y
    }

    /// The background: the selection, else the hover tint, else none.
    pub fn fill(&self, ui: &Ui, t: &Tokens, selected: bool, hovered: bool) {
        ui.painter().rect_filled(self.rect, col::RADIUS, row_fill(t, selected, hovered));
    }

    /// The playing triangle at `x`; returns where the title starts after it.
    pub fn playing(&self, ui: &Ui, t: &Tokens, x: f32) -> f32 {
        icons::paint(ui.painter(), Rect::from_center_size(pos2(x + col::PLAYING / 2.0, self.cy()), vec2(col::PLAYING, col::PLAYING)), Icon::Play, t.accent);
        x + col::PLAYING_W
    }

    /// One line at `x`, cut to `width`, centred on the row. Returns its width.
    pub fn line(&self, ui: &Ui, x: f32, width: f32, text: &str, font: FontId, color: Color32) -> f32 {
        let g = elide(ui, text, font, color, width);
        let w = g.size().x;
        ui.painter().galley(pos2(x, self.cy() - g.size().y / 2.0), g, color);
        w
    }

    /// A title over a second line (the artist), both cut to `width`, meeting
    /// at the row's middle.
    pub fn two_lines(&self, ui: &Ui, x: f32, width: f32, title: (&str, FontId, Color32), sub: (&str, FontId, Color32)) {
        let cy = self.cy();
        let g = elide(ui, title.0, title.1, title.2, width);
        ui.painter().galley(pos2(x, cy - g.size().y), g, title.2);
        let g = elide(ui, sub.0, sub.1, sub.2, width);
        ui.painter().galley(pos2(x, cy + 1.0), g, sub.2);
    }

    /// Text ending at `right`, centred on the row (the length).
    pub fn right_text(&self, ui: &Ui, right: f32, text: String, font: FontId, color: Color32) {
        ui.painter().text(pos2(right, self.cy()), Align2::RIGHT_CENTER, text, font, color);
    }

    /// The download mark centred at `x`, when the song has one.
    pub fn mark(&self, ui: &Ui, x: f32, state: Option<&offline::TrackState>) {
        if let Some(m) = Mark::of_track(state) {
            super::widgets::mark(ui, pos2(x, self.cy()), col::MARK, m);
        }
    }

    /// Names the row for a screen reader: an option in a list box.
    pub fn describe(&self, selected: bool, label: impl Fn() -> String) {
        self.resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::ListBoxOption, true, selected, label()));
    }
}
