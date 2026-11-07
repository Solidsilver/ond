//! A scrolling spectrogram for the Info tab: frequency up (log scale, as the
//! spectrum's bands), time to the right, loudness as the accent color's
//! strength. Columns go into a small texture one at a time (a partial
//! upload of a single column), and drawing rotates the ring so the newest
//! column is always at the right edge.

use super::spectrum::{BANDS, HIGH_HZ, LOW_HZ};
use egui::{Color32, ColorImage, Rect, TextureHandle, TextureOptions, pos2};

/// Columns kept: about eight seconds at 30 a second.
pub const COLUMNS: usize = 240;

#[derive(Default)]
pub struct Spectrogram {
    tex: Option<TextureHandle>,
    /// Where the next column goes (the oldest one is there now).
    next: usize,
    /// The pass a column was last added in (one per frame, however many draw it).
    pass: Option<u64>,
}

/// One column: the highest band at the top, each pixel white with the
/// band's strength as alpha (the accent tints it when drawn).
pub fn column(raw: &[f32; BANDS]) -> ColorImage {
    let pixels = raw.iter().rev().map(|v| Color32::from_white_alpha((v.clamp(0.0, 1.0).powf(1.6) * 255.0).round() as u8)).collect();
    ColorImage::new([1, BANDS], pixels)
}

impl Spectrogram {
    /// Adds `raw` as the newest column, once per pass.
    pub fn push(&mut self, ctx: &egui::Context, raw: &[f32; BANDS], pass: u64) {
        if self.pass == Some(pass) {
            return;
        }
        self.pass = Some(pass);
        let options = TextureOptions::LINEAR;
        let tex = self.tex.get_or_insert_with(|| ctx.load_texture("spectrogram", ColorImage::filled([COLUMNS, BANDS], Color32::TRANSPARENT), options));
        tex.set_partial([self.next, 0], column(raw), options);
        self.next = (self.next + 1) % COLUMNS;
    }

    /// Draws it into `rect` in `color`, oldest at the left; nothing before
    /// the first column.
    pub fn paint(&self, painter: &egui::Painter, rect: Rect, color: Color32) {
        let Some(tex) = &self.tex else { return };
        let split = self.next as f32 / COLUMNS as f32;
        // The ring from `next` to its end, then from its start up to `next`.
        let left_w = rect.width() * (1.0 - split);
        let parts = [
            (Rect::from_min_max(rect.min, pos2(rect.left() + left_w, rect.bottom())), Rect::from_min_max(pos2(split, 0.0), pos2(1.0, 1.0))),
            (Rect::from_min_max(pos2(rect.left() + left_w, rect.top()), rect.max), Rect::from_min_max(pos2(0.0, 0.0), pos2(split, 1.0))),
        ];
        for (r, uv) in parts {
            if r.width() > 0.0 {
                painter.add(egui::epaint::RectShape::filled(r, 0.0, color).with_texture(tex.id(), uv));
            }
        }
    }

    /// Forgets what it showed (the experiment was turned off).
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Where `hz` sits from the bottom (0) to the top (1) of the spectrogram.
pub fn height_of(hz: f32) -> f32 {
    ((hz / LOW_HZ).ln() / (HIGH_HZ / LOW_HZ).ln()).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_column_has_the_high_end_at_the_top() {
        let mut raw = [0.0; BANDS];
        raw[0] = 1.0;
        let c = column(&raw);
        assert_eq!(c.size, [1, BANDS]);
        assert_eq!(c.pixels[BANDS - 1].a(), 255, "the lowest band is the bottom pixel");
        assert_eq!(c.pixels[0].a(), 0);
    }

    #[test]
    fn columns_go_round_the_ring_once_per_pass() {
        let ctx = egui::Context::default();
        let mut s = Spectrogram::default();
        let raw = [0.5; BANDS];
        s.push(&ctx, &raw, 1);
        s.push(&ctx, &raw, 1);
        assert_eq!(s.next, 1, "a second visual in the same pass adds nothing");
        for pass in 2..=COLUMNS as u64 + 1 {
            s.push(&ctx, &raw, pass);
        }
        assert_eq!(s.next, 1, "wrapped round");
    }

    #[test]
    fn frequencies_sit_on_a_log_axis() {
        assert_eq!(height_of(LOW_HZ), 0.0);
        assert!((height_of(HIGH_HZ) - 1.0).abs() < 1e-5);
        // A decade takes the same height wherever it is.
        let decade = height_of(1000.0) - height_of(100.0);
        assert!((height_of(10_000.0) - height_of(1000.0) - decade).abs() < 1e-5);
    }
}
