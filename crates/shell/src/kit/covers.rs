//! What's laid over a cover, or under a card, to say how far something has
//! got: a thin line along the cover's foot, a round badge in its corner, and
//! the same line on its own as a small bar. Painted only; the tile or card
//! they sit on says the same in its name for a screen reader.

use crate::icons::{self, Icon};
use crate::theme::Tokens;
use egui::{Color32, Painter, Rect, pos2, vec2};

/// How tall the line along a cover's foot is.
pub const COVER_LINE_H: f32 = 4.0;
/// The badge's diameter, and how far it sits in from the cover's corner.
const BADGE: f32 = 22.0;
const BADGE_INSET: f32 = 7.0;

/// A track under the cover's foot, its first `fraction` in the accent. The
/// band follows the cover's rounded corners: it's the cover's own shape,
/// clipped to its last few points.
pub fn cover_progress(painter: &Painter, cover: Rect, radius: f32, fraction: f32, t: &Tokens) {
    let band = Rect::from_min_max(pos2(cover.left(), cover.bottom() - COVER_LINE_H), cover.max);
    // The muted ink, half see-through: grey on any cover, and still there on
    // a placeholder in either theme, so the cover never looks cut short.
    painter.with_clip_rect(band.intersect(painter.clip_rect())).rect_filled(cover, radius, t.muted.gamma_multiply(0.6));
    let done = Rect::from_min_max(band.min, pos2(band.left() + band.width() * fraction.clamp(0.0, 1.0), band.bottom()));
    painter.with_clip_rect(done.intersect(painter.clip_rect())).rect_filled(cover, radius, t.accent);
}

/// A round accent badge with `icon` in it, in the cover's top right corner (a
/// check mark on a finished book).
pub fn cover_badge(painter: &Painter, cover: Rect, icon: Icon, t: &Tokens) {
    let c = pos2(cover.right() - BADGE_INSET - BADGE / 2.0, cover.top() + BADGE_INSET + BADGE / 2.0);
    // A thin ring of shadow keeps it apart from a cover in the same colour.
    painter.circle_filled(c, BADGE / 2.0 + 1.0, Color32::from_black_alpha(40));
    painter.circle_filled(c, BADGE / 2.0, t.accent);
    icons::paint(painter, Rect::from_center_size(c, vec2(11.0, 11.0)), icon, t.on_accent);
}

/// A thin rounded bar, `fraction` of it in the accent, across `rect` (a
/// card's progress under its text).
pub fn progress_bar(painter: &Painter, rect: Rect, fraction: f32, t: &Tokens) {
    let r = rect.height() / 2.0;
    painter.rect_filled(rect, r, t.muted.gamma_multiply(0.28));
    let fraction = fraction.clamp(0.0, 1.0);
    if fraction > 0.0 {
        // Never narrower than the bar is tall, so a book just begun still
        // shows a round dot rather than a sliver.
        let w = (rect.width() * fraction).max(rect.height());
        painter.rect_filled(Rect::from_min_size(rect.min, vec2(w, rect.height())), r, t.accent);
    }
}
