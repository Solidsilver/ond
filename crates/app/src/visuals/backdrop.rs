//! Blurred artwork behind the Player and Mini: a small, heavily blurred copy
//! of the cover (made once per cover, off the UI thread), drawn under a wash of
//! the theme's background that's just strong enough to keep text readable over
//! its brightest and darkest parts. A new cover fades in over the old.

use super::colors::contrast;
use egui::{Color32, ColorImage, TextureHandle};

/// The blurred copy's side, in pixels: the blur hides any lack of detail.
const SIDE: usize = 48;
/// Box blur radius in pixels (of [`SIDE`]); three passes make it near-Gaussian.
const RADIUS: usize = 4;

/// The blurred copy of `image` and its darkest and brightest colors.
pub fn blurred(image: &ColorImage) -> Option<(ColorImage, [Color32; 2])> {
    let [w, h] = image.size;
    if w == 0 || h == 0 {
        return None;
    }
    // Downscale by averaging into SIDE × SIDE cells.
    let mut px = vec![[0f32; 3]; SIDE * SIDE];
    let mut counts = vec![0f32; SIDE * SIDE];
    for y in 0..h {
        for x in 0..w {
            let cell = (y * SIDE / h) * SIDE + x * SIDE / w;
            let c = image.pixels[y * w + x];
            for (p, v) in px[cell].iter_mut().zip([c.r(), c.g(), c.b()]) {
                *p += f32::from(v);
            }
            counts[cell] += 1.0;
        }
    }
    for (p, n) in px.iter_mut().zip(&counts) {
        for v in p.iter_mut() {
            *v /= n.max(1.0);
        }
    }
    for _ in 0..3 {
        box_pass(&mut px, true);
        box_pass(&mut px, false);
    }
    let pixels: Vec<Color32> = px.iter().map(|p| Color32::from_rgb(p[0].round() as u8, p[1].round() as u8, p[2].round() as u8)).collect();
    let lum = |c: &&Color32| c.intensity();
    let darkest = *pixels.iter().min_by(|a, b| lum(a).total_cmp(&lum(b)))?;
    let brightest = *pixels.iter().max_by(|a, b| lum(a).total_cmp(&lum(b)))?;
    Some((ColorImage::new([SIDE, SIDE], pixels), [darkest, brightest]))
}

/// One horizontal (or vertical) box blur pass, edges clamped.
fn box_pass(px: &mut [[f32; 3]], horizontal: bool) {
    let src = px.to_vec();
    let at = |line: usize, i: usize| if horizontal { line * SIDE + i } else { i * SIDE + line };
    let n = (2 * RADIUS + 1) as f32;
    for line in 0..SIDE {
        for i in 0..SIDE {
            let mut sum = [0f32; 3];
            for k in 0..=2 * RADIUS {
                let j = (i + k).saturating_sub(RADIUS).min(SIDE - 1);
                for (s, v) in sum.iter_mut().zip(src[at(line, j)]) {
                    *s += v;
                }
            }
            px[at(line, i)] = sum.map(|s| s / n);
        }
    }
}

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

/// The backdrop shown, fading from the previous cover's.
#[derive(Default)]
pub struct Crossfade {
    current: Option<(String, TextureHandle, [Color32; 2])>,
    previous: Option<(TextureHandle, [Color32; 2])>,
    since: f64,
}

/// How long one cover's backdrop takes to fade into the next.
const FADE_SECS: f64 = 0.9;

/// One layer to draw: the texture, how opaque, and its extremes.
pub type Layer = (TextureHandle, f32, [Color32; 2]);

impl Crossfade {
    /// Shows `next` (a cover id, its blurred texture and extremes), fading
    /// from what was shown. `None` keeps what's there (still loading).
    /// Returns the layers to draw, oldest first, and whether it's mid-fade.
    pub fn step(&mut self, next: Option<(&str, &TextureHandle, [Color32; 2])>, now: f64) -> (Vec<Layer>, bool) {
        if let Some((cover, tex, ext)) = next
            && self.current.as_ref().is_none_or(|(c, ..)| c != cover)
        {
            self.previous = self.current.take().map(|(_, t, e)| (t, e));
            self.current = Some((cover.to_owned(), tex.clone(), ext));
            self.since = now;
        }
        let k = ((now - self.since) / FADE_SECS).clamp(0.0, 1.0) as f32;
        let k = k * k * (3.0 - 2.0 * k);
        if k >= 1.0 {
            self.previous = None;
        }
        let mut layers = Vec::new();
        if let Some((t, e)) = &self.previous {
            layers.push((t.clone(), 1.0, *e));
        }
        if let Some((_, t, e)) = &self.current {
            layers.push((t.clone(), if self.previous.is_some() { k } else { 1.0 }, *e));
        }
        (layers, self.previous.is_some())
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Tokens;

    #[test]
    fn the_blur_is_small_smooth_and_keeps_the_colors() {
        // Left half red, right half blue: a hard edge.
        let n = 128;
        let img = ColorImage::new([n, n], (0..n * n).map(|i| if i % n < n / 2 { Color32::from_rgb(200, 20, 20) } else { Color32::from_rgb(20, 20, 200) }).collect());
        let (b, [dark, bright]) = blurred(&img).unwrap();
        assert_eq!(b.size, [SIDE, SIDE]);
        let row = &b.pixels[SIDE * (SIDE / 2)..SIDE * (SIDE / 2 + 1)];
        // Far from the edge the colors are as they were; across it they blend.
        assert!(row[0].r() > 190 && row[SIDE - 1].b() > 190);
        let mid = row[SIDE / 2];
        assert!(mid.r() > 60 && mid.b() > 60, "the edge is soft: {mid:?}");
        // No step between neighbours is anywhere near the original jump.
        assert!(row.windows(2).all(|w| (i32::from(w[0].r()) - i32::from(w[1].r())).abs() < 40));
        assert!(dark.intensity() <= bright.intensity());
        assert!(blurred(&ColorImage::new([0, 0], Vec::new())).is_none());
    }

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
