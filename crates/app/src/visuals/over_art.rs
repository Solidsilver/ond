//! Controls drawn over artwork or the blurred backdrop: one place decides
//! their colors from what is actually under them, so a near-white cover can't
//! swallow a light icon (or a near-black one a dark icon). The theme's own
//! color stays when it reads; else light or dark, whichever does; and only
//! when neither reaches the contrast on the art alone, a scrim under it, just
//! as strong as it needs to be.

use super::colors::contrast;
use egui::{Color32, ColorImage, Rect, Ui, pos2};

/// The light foreground over art (the dark theme's text, a touch softer).
pub const LIGHT: Color32 = Color32::from_rgb(0xE8, 0xE9, 0xEC);
/// The dark foreground over art (the light theme's text).
pub const DARK: Color32 = Color32::from_rgb(0x1C, 0x1C, 0x1E);
/// Under a light foreground that needs help.
const SHADE_DARK: Color32 = Color32::from_rgb(12, 13, 15);
/// Under a dark foreground that needs help.
const SHADE_LIGHT: Color32 = Color32::from_rgb(0xF4, 0xF4, 0xF5);

/// Small glyphs and text: 4.5:1 (WCAG AA).
pub const ICON: f32 = 4.5;
/// Larger graphics (a progress line): 3:1 (WCAG non-text contrast).
pub const GRAPHIC: f32 = 3.0;

/// How to draw a control over art.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OnArt {
    pub fg: Color32,
    /// Laid under the control (a disc, a band) when no foreground reaches
    /// the contrast on the art alone; `None` when the color does it.
    pub scrim: Option<Color32>,
}

impl OnArt {
    /// `fg` over a surface known to suit it (a plain theme background, or a scrim).
    pub fn plain(fg: Color32) -> Self {
        Self { fg, scrim: None }
    }

    /// For a control over a region whose colors, as seen, run from
    /// `under[0]` to `under[1]` (darkest and brightest; see [`region`]):
    /// `preferred` if it reaches `need` against both, else [`LIGHT`] or
    /// [`DARK`]; when neither does, the one that comes closer over just
    /// enough of a scrim in the other. `need` is a WCAG contrast ratio
    /// ([`ICON`] 4.5, [`GRAPHIC`] 3).
    pub fn pick(under: [Color32; 2], preferred: Color32, need: f32) -> Self {
        let worst = |fg: Color32| under.iter().map(|c| contrast(fg, *c)).fold(f32::INFINITY, f32::min);
        if let Some(fg) = [preferred, LIGHT, DARK].into_iter().find(|fg| worst(*fg) >= need) {
            return Self::plain(fg);
        }
        let (fg, shade) = if worst(LIGHT) >= worst(DARK) { (LIGHT, SHADE_DARK) } else { (DARK, SHADE_LIGHT) };
        let mut alpha: f32 = 0.1;
        while alpha < 1.0 && under.iter().any(|c| contrast(fg, c.lerp_to_gamma(shade, alpha)) < need) {
            alpha += 0.05;
        }
        Self { fg, scrim: Some(shade.gamma_multiply(alpha.min(1.0))) }
    }

    /// A faint fill in the foreground's own tone, for a hovered (or pressed)
    /// button: light over dark art, dark over light.
    pub fn hover(&self, pressed: bool) -> Color32 {
        self.fg.gamma_multiply(if pressed { 0.26 } else { 0.16 })
    }

    /// Buttons in `ui` hover in the foreground's tone instead of the theme's.
    pub fn apply(&self, ui: &mut Ui) {
        let w = &mut ui.visuals_mut().widgets;
        w.hovered.weak_bg_fill = self.hover(false);
        w.active.weak_bg_fill = self.hover(true);
        w.inactive.weak_bg_fill = Color32::TRANSPARENT;
    }
}

/// The darkest and brightest pixels of `image` within `uv` (0..1 each way;
/// at least the one pixel nearest its centre).
pub fn region(image: &ColorImage, uv: Rect) -> [Color32; 2] {
    let [w, h] = image.size;
    if w == 0 || h == 0 {
        return [Color32::BLACK, Color32::WHITE];
    }
    let span = |lo: f32, hi: f32, n: usize| {
        let a = ((lo.clamp(0.0, 1.0) * n as f32).floor() as usize).min(n - 1);
        let b = ((hi.clamp(0.0, 1.0) * n as f32).ceil() as usize).clamp(a + 1, n);
        a..b
    };
    let (xs, ys) = (span(uv.min.x, uv.max.x, w), span(uv.min.y, uv.max.y, h));
    let lum = |c: &Color32| c.intensity();
    let mut darkest = image.pixels[ys.start * w + xs.start];
    let mut brightest = darkest;
    for y in ys {
        for c in &image.pixels[y * w + xs.start..y * w + xs.end] {
            if lum(c) < lum(&darkest) {
                darkest = *c;
            }
            if lum(c) > lum(&brightest) {
                brightest = *c;
            }
        }
    }
    [darkest, brightest]
}

/// Where `part` of a picture drawn into `whole` (showing `crop` of it, in
/// texture coordinates) falls in the picture.
pub fn uv_of(part: Rect, whole: Rect, crop: Rect) -> Rect {
    let at = |p: egui::Pos2| {
        let f = (p - whole.min) / whole.size();
        pos2(crop.min.x + f.x * crop.width(), crop.min.y + f.y * crop.height())
    };
    Rect::from_min_max(at(part.min), at(part.max))
}

/// The darker and brighter of two regions' extremes, for a region seen
/// through more than one picture (a cross-fade).
pub fn union(a: [Color32; 2], b: [Color32; 2]) -> [Color32; 2] {
    let lum = |c: Color32| c.intensity();
    [if lum(a[0]) <= lum(b[0]) { a[0] } else { b[0] }, if lum(a[1]) >= lum(b[1]) { a[1] } else { b[1] }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Tokens;

    fn image(f: impl Fn(usize, usize) -> Color32) -> ColorImage {
        let n = 48;
        ColorImage::new([n, n], (0..n * n).map(|i| f(i % n, i / n)).collect())
    }

    /// What `on` looks like where it's darkest and brightest against `under`.
    fn reads(on: OnArt, under: [Color32; 2], need: f32) -> bool {
        under.iter().all(|c| {
            let shade = if on.fg == LIGHT { SHADE_DARK } else { SHADE_LIGHT };
            let seen = on.scrim.map_or(*c, |s| c.lerp_to_gamma(shade, f32::from(s.a()) / 255.0));
            contrast(on.fg, seen) >= need - 0.01
        })
    }

    #[test]
    fn controls_read_over_light_and_dark_covers() {
        let near_white = [Color32::from_rgb(236, 238, 240), Color32::from_rgb(252, 252, 250)];
        let near_black = [Color32::from_rgb(4, 4, 6), Color32::from_rgb(30, 28, 34)];
        for preferred in [Tokens::LIGHT.muted, Tokens::DARK.muted, LIGHT] {
            let on_white = OnArt::pick(near_white, preferred, ICON);
            assert!(reads(on_white, near_white, ICON), "{on_white:?}");
            assert_eq!(on_white.scrim, None, "color alone does it on a plain light cover");
            assert!(contrast(on_white.fg, Color32::WHITE) > contrast(on_white.fg, Color32::BLACK), "dark on light");
            let on_black = OnArt::pick(near_black, preferred, ICON);
            assert!(reads(on_black, near_black, ICON), "{on_black:?}");
            assert_eq!(on_black.scrim, None);
        }
        // The theme's own color stays when it already reads.
        assert_eq!(OnArt::pick([Tokens::LIGHT.bg; 2], Tokens::LIGHT.muted, ICON), OnArt::plain(Tokens::LIGHT.muted));
    }

    #[test]
    fn a_busy_cover_gets_a_scrim_just_strong_enough() {
        // Black and white under the same button: no color reads on both.
        let busy = [Color32::BLACK, Color32::WHITE];
        for preferred in [Tokens::LIGHT.muted, LIGHT, DARK] {
            let on = OnArt::pick(busy, preferred, ICON);
            let scrim = on.scrim.expect("a scrim");
            assert!(reads(on, busy, ICON), "{on:?}");
            assert!(scrim.a() < 255, "only as strong as it needs to be: {scrim:?}");
        }
        // A larger graphic needs less: the theme's muted grey reads on both at 3:1.
        assert_eq!(OnArt::pick(busy, Tokens::LIGHT.muted, GRAPHIC), OnArt::plain(Tokens::LIGHT.muted));
        // Mid grey: neither light nor dark reaches 4.5:1 alone.
        let grey = [Color32::from_gray(118), Color32::from_gray(124)];
        let on = OnArt::pick(grey, LIGHT, ICON);
        assert!(on.scrim.is_some() && reads(on, grey, ICON));
    }

    #[test]
    fn a_region_is_sampled_where_the_control_sits() {
        // A white top half over a black bottom half.
        let img = image(|_, y| if y < 24 { Color32::WHITE } else { Color32::BLACK });
        let top = region(&img, Rect::from_min_max(pos2(0.6, 0.0), pos2(1.0, 0.2)));
        assert_eq!(top, [Color32::WHITE, Color32::WHITE]);
        let bottom = region(&img, Rect::from_min_max(pos2(0.0, 0.8), pos2(0.3, 1.0)));
        assert_eq!(bottom, [Color32::BLACK, Color32::BLACK]);
        assert_eq!(region(&img, Rect::from_min_max(pos2(0.0, 0.4), pos2(1.0, 0.6))), [Color32::BLACK, Color32::WHITE]);
        // A zero-sized region still sees a pixel; one outside the picture is clamped to it.
        assert_eq!(region(&img, Rect::from_min_max(pos2(0.5, 0.1), pos2(0.5, 0.1))), [Color32::WHITE; 2]);
        assert_eq!(region(&img, Rect::from_min_max(pos2(0.2, 1.2), pos2(0.4, 1.5))), [Color32::BLACK; 2]);
        // The top-right corner of a window showing the middle of a square picture.
        let whole = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(200.0, 100.0));
        let uv = uv_of(Rect::from_min_max(pos2(150.0, 0.0), pos2(200.0, 20.0)), whole, Rect::from_min_max(pos2(0.0, 0.25), pos2(1.0, 0.75)));
        assert!((uv.min.x - 0.75).abs() < 1e-5 && (uv.min.y - 0.25).abs() < 1e-5 && (uv.max.y - 0.35).abs() < 1e-5, "{uv:?}");
    }
}
