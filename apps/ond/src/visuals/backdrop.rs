//! Blurred artwork behind the Player and Mini: a small, heavily blurred copy
//! of the cover (made once per cover, off the UI thread), drawn under a wash of
//! the theme's background that's just strong enough to keep text readable over
//! its brightest and darkest parts. A new cover fades in over the old.

use super::colors::contrast;
use egui::{Color32, ColorImage, TextureHandle};
use std::sync::Arc;

pub use shell::art::blurred;

/// How strongly to lay `wash` over a backdrop whose extremes are `extremes`,
/// so `text` stays at 7:1 and `muted` at 4.5:1 over every part of it.
pub fn wash_alpha(extremes: [Color32; 2], wash: Color32, text: Color32, muted: Color32) -> f32 {
    // Never weaker than this, so the backdrop stays a backdrop.
    let mut alpha = 0.55;
    while alpha < 1.0 {
        let ok = extremes.iter().all(|c| {
            let seen = c.lerp_to_gamma(wash, alpha);
            contrast(text, seen) >= 7.0 && contrast(muted, seen) >= 4.5
        });
        if ok {
            return alpha;
        }
        alpha += 0.025;
    }
    1.0
}

/// A cover's blurred copy, ready to draw: the texture, its pixels (to see
/// what lies under a control; see [`super::over_art`]) and its darkest and
/// brightest colors.
#[derive(Clone)]
pub struct Blurred {
    pub texture: TextureHandle,
    pub image: Arc<ColorImage>,
    pub extremes: [Color32; 2],
}

/// The backdrop shown, fading from the previous cover's.
#[derive(Default)]
pub struct Crossfade {
    current: Option<(String, Blurred)>,
    previous: Option<Blurred>,
    since: f64,
}

/// How long one cover's backdrop takes to fade into the next.
const FADE_SECS: f64 = 0.9;

/// One layer to draw, and how opaque.
pub struct Layer {
    pub blurred: Blurred,
    pub opacity: f32,
}

impl Crossfade {
    /// Shows `next` (a cover id and its blurred copy), fading from what was
    /// shown. `None` keeps what's there (still loading). Returns the layers
    /// to draw, oldest first, and whether it's mid-fade.
    pub fn step(&mut self, next: Option<(&str, &Blurred)>, now: f64) -> (Vec<Layer>, bool) {
        if let Some((cover, blurred)) = next
            && self.current.as_ref().is_none_or(|(c, _)| c != cover)
        {
            self.previous = self.current.take().map(|(_, b)| b);
            self.current = Some((cover.to_owned(), blurred.clone()));
            self.since = now;
        }
        let k = ((now - self.since) / FADE_SECS).clamp(0.0, 1.0) as f32;
        let k = k * k * (3.0 - 2.0 * k);
        if k >= 1.0 {
            self.previous = None;
        }
        let mut layers = Vec::new();
        if let Some(b) = &self.previous {
            layers.push(Layer { blurred: b.clone(), opacity: 1.0 });
        }
        if let Some((_, b)) = &self.current {
            layers.push(Layer { blurred: b.clone(), opacity: if self.previous.is_some() { k } else { 1.0 } });
        }
        (layers, self.previous.is_some())
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// The darkest and brightest colors seen in `uv` of the backdrop `layers`
/// under `wash` at `alpha` (either layer's, mid-fade). `None` without layers.
pub fn seen(layers: &[Layer], uv: egui::Rect, wash: Color32, alpha: f32) -> Option<[Color32; 2]> {
    layers.iter().map(|l| super::over_art::region(&l.blurred.image, uv).map(|c| c.lerp_to_gamma(wash, alpha))).reduce(super::over_art::union)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Tokens;

    #[test]
    fn the_wash_keeps_text_readable_over_any_backdrop() {
        for (t, extremes) in [
            (Tokens::LIGHT, [Color32::BLACK, Color32::WHITE]),
            (Tokens::LIGHT, [Color32::from_rgb(20, 30, 120), Color32::from_rgb(255, 230, 0)]),
            (Tokens::DARK, [Color32::BLACK, Color32::WHITE]),
            (Tokens::DARK, [Color32::from_rgb(240, 240, 250), Color32::from_rgb(255, 200, 200)]),
        ] {
            let a = wash_alpha(extremes, t.bg, t.text, t.muted);
            for c in extremes {
                let seen = c.lerp_to_gamma(t.bg, a);
                assert!(contrast(t.text, seen) >= 7.0 && contrast(t.muted, seen) >= 4.5, "{c:?} at {a}");
            }
        }
        // A backdrop already close to the theme's background needs only the minimum.
        let t = Tokens::DARK;
        assert_eq!(wash_alpha([t.bg, t.panel], t.bg, t.text, t.muted), 0.55);
    }
}
