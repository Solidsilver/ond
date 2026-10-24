//! Colors from the album art: a small palette taken from the cover once per
//! cover, and the tint it gives the app (the accent, the now-playing display,
//! the Player's background), checked for contrast against the theme's text.
//! A cover with no real color in it (black and white, near-grey) gives no
//! palette, and the theme's own colors stay.

use crate::theme::Tokens;
use egui::{Color32, ColorImage};

/// What a cover's colors come down to.
#[derive(Debug, Clone, PartialEq)]
pub struct ArtPalette {
    /// The cover's overall color (a mean of every pixel), for surfaces.
    pub average: Color32,
    /// Its most prominent colors, strongest first (at least one).
    pub colors: Vec<Color32>,
}

impl ArtPalette {
    /// The most prominent color.
    pub fn accent(&self) -> Color32 {
        self.colors[0]
    }
}

/// Hue bins for grouping a cover's colorful pixels.
const HUES: usize = 12;
/// At most this many pixels a side are looked at; covers are smooth enough.
const GRID: usize = 48;
/// Below this share of colorful pixels the cover counts as grey.
const MIN_COLORFUL: f32 = 0.04;

/// The palette of `image`, or `None` when it has too little color to use.
pub fn extract(image: &ColorImage) -> Option<ArtPalette> {
    let [w, h] = image.size;
    if w == 0 || h == 0 {
        return None;
    }
    let (sx, sy) = (w.div_ceil(GRID).max(1), h.div_ceil(GRID).max(1));
    let mut sum = [0f64; 3];
    let mut n = 0f64;
    // Per hue bin: weight and weighted color sums.
    let mut bins = [(0f64, [0f64; 3]); HUES];
    let mut colorful = 0f64;
    for y in (0..h).step_by(sy) {
        for x in (0..w).step_by(sx) {
            let [r, g, b, a] = image.pixels[y * w + x].to_srgba_unmultiplied();
            if a < 128 {
                continue;
            }
            let rgb = [f64::from(r), f64::from(g), f64::from(b)];
            for (s, c) in sum.iter_mut().zip(rgb) {
                *s += c;
            }
            n += 1.0;
            let (hue, sat, val) = hsv([r, g, b]);
            // Washed-out, near-black and near-white pixels say little about the cover's color.
            if sat < 0.2 || !(0.18..=0.97).contains(&val) {
                continue;
            }
            colorful += 1.0;
            let weight = f64::from(sat * (0.4 + 0.6 * val));
            let bin = &mut bins[((hue * HUES as f32) as usize).min(HUES - 1)];
            bin.0 += weight;
            for (s, c) in bin.1.iter_mut().zip(rgb) {
                *s += c * weight;
            }
        }
    }
    if n == 0.0 || (colorful / n) < f64::from(MIN_COLORFUL) {
        return None;
    }
    let avg = |s: [f64; 3], d: f64| Color32::from_rgb((s[0] / d).round() as u8, (s[1] / d).round() as u8, (s[2] / d).round() as u8);
    let mut ranked: Vec<(f64, Color32)> = bins.iter().filter(|(wt, _)| *wt > 0.0).map(|(wt, s)| (*wt, avg(*s, *wt))).collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    // A bin far smaller than the strongest is noise, not a color of the cover.
    let top = ranked.first()?.0;
    let colors = ranked.into_iter().filter(|(wt, _)| *wt >= top * 0.12).take(4).map(|(_, c)| c).collect();
    Some(ArtPalette { average: avg(sum, n), colors })
}

/// Hue (0..1), saturation and value of an sRGB color.
fn hsv([r, g, b]: [u8; 3]) -> (f32, f32, f32) {
    let (r, g, b) = (f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let s = if max > 0.0 { d / max } else { 0.0 };
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    (h, s, max)
}

fn from_hsv(h: f32, s: f32, v: f32) -> Color32 {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    let (r, g, b) = match (i as i32).rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    Color32::from_rgb((r * 255.0).round() as u8, (g * 255.0).round() as u8, (b * 255.0).round() as u8)
}

/// WCAG relative luminance.
fn luminance(c: Color32) -> f32 {
    let lin = |u: u8| {
        let v = f32::from(u) / 255.0;
        if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG contrast ratio between two opaque colors, 1 to 21.
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Body text and links need 4.5:1 (WCAG AA).
const AA: f32 = 4.5;

/// The colors the art changes: everything else stays the theme's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tint {
    pub accent: Color32,
    pub on_accent: Color32,
    /// The now-playing display's fill.
    pub display: Color32,
    /// The Player's background.
    pub stage: Color32,
}

impl Tint {
    /// The theme's own colors, untinted.
    pub fn plain(t: &Tokens) -> Self {
        Self { accent: t.accent, on_accent: t.on_accent, display: t.sidebar, stage: t.bg }
    }

    /// The tint `palette` gives theme `t` (`dark` or light). Each color is
    /// held to the contrast the theme's own has; any that can't be falls back
    /// to the theme's.
    pub fn from_palette(palette: &ArtPalette, t: &Tokens, dark: bool) -> Self {
        let plain = Self::plain(t);
        // Surfaces take a little of the cover's overall color, as much as the
        // text on them allows (never less contrast than the theme's own, and AA for muted text).
        let surface = |base: Color32, most: f32| {
            let floor_text = (contrast(t.text, base) * 0.9).max(7.0);
            let mut amount = most;
            while amount > 0.01 {
                let c = base.lerp_to_gamma(palette.average, amount);
                if contrast(t.text, c) >= floor_text && contrast(t.muted, c) >= AA {
                    return c;
                }
                amount -= 0.02;
            }
            base
        };
        let (display_most, stage_most) = if dark { (0.22, 0.20) } else { (0.16, 0.12) };
        let display = surface(t.sidebar, display_most);
        let stage = surface(t.bg, stage_most);
        let Some(accent) = readable_accent(palette.accent(), &[t.bg, t.sidebar, t.panel, stage, display], dark) else {
            return Self { display, stage, ..plain };
        };
        // The theme's own on-accent if it still reads; else whichever of near-white and near-black does.
        let on_accent = [t.on_accent, Color32::from_rgb(0xFF, 0xFF, 0xFF), Color32::from_rgb(0x10, 0x12, 0x16)]
            .into_iter()
            .find(|c| contrast(*c, accent) >= AA);
        match on_accent {
            Some(on_accent) => Self { accent, on_accent, display, stage },
            None => Self { display, stage, ..plain },
        }
    }

    pub fn lerp(&self, to: &Tint, t: f32) -> Tint {
        Tint {
            accent: self.accent.lerp_to_gamma(to.accent, t),
            on_accent: self.on_accent.lerp_to_gamma(to.on_accent, t),
            display: self.display.lerp_to_gamma(to.display, t),
            stage: self.stage.lerp_to_gamma(to.stage, t),
        }
    }

    /// `t` with this tint's accent.
    pub fn apply(&self, t: Tokens) -> Tokens {
        Tokens { accent: self.accent, on_accent: self.on_accent, ..t }
    }
}

/// `c` as an accent: calmed (no neon), then darkened (light theme) or
/// lightened (dark theme) until it reads at AA on every one of `against`.
/// `None` when even that can't make it (it would have to turn grey).
fn readable_accent(c: Color32, against: &[Color32], dark: bool) -> Option<Color32> {
    let (h, s, v) = hsv([c.r(), c.g(), c.b()]);
    // Quiet: limited saturation, and a value that suits the theme.
    let (s, v) = if dark { (s.clamp(0.25, 0.55), v.max(0.75)) } else { (s.clamp(0.3, 0.65), v.min(0.6)) };
    for step in 0..=20 {
        let k = step as f32 / 20.0;
        // Toward the theme's text: darker on light, lighter on dark, losing a little saturation.
        let v = if dark { v + (1.0 - v) * k } else { v * (1.0 - k * 0.8) };
        let s = s * (1.0 - k * 0.5);
        let candidate = from_hsv(h, s, v);
        if against.iter().all(|bg| contrast(candidate, *bg) >= AA) {
            return Some(candidate);
        }
    }
    None
}

/// Both themes' tints for one palette, worked out once per cover.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tints {
    pub light: Tint,
    pub dark: Tint,
}

impl Tints {
    pub fn new(palette: &ArtPalette) -> Self {
        Self { light: Tint::from_palette(palette, &Tokens::LIGHT, false), dark: Tint::from_palette(palette, &Tokens::DARK, true) }
    }

    pub fn plain() -> Self {
        Self { light: Tint::plain(&Tokens::LIGHT), dark: Tint::plain(&Tokens::DARK) }
    }

    pub fn of(&self, dark: bool) -> Tint {
        if dark { self.dark } else { self.light }
    }

    pub fn lerp(&self, to: &Tints, t: f32) -> Tints {
        Tints { light: self.light.lerp(&to.light, t), dark: self.dark.lerp(&to.dark, t) }
    }
}

/// The tint in use, eased from one cover's to the next.
#[derive(Debug, Default)]
pub struct Easing {
    from: Option<Tints>,
    to: Option<Tints>,
    /// Frame time the ease began.
    since: f64,
}

/// How long a change of cover takes to show in the colors.
const EASE_SECS: f64 = 0.8;

impl Easing {
    /// Heads for `target` (`None`: the plain theme) from wherever it is now,
    /// and returns the tint to draw this frame (`None`: nothing tinted, the
    /// eased-out state) and whether it's still moving.
    pub fn step(&mut self, target: Option<Tints>, now: f64) -> (Option<Tints>, bool) {
        if target != self.to {
            self.from = self.current(now);
            self.to = target;
            self.since = now;
        }
        let k = ((now - self.since) / EASE_SECS).clamp(0.0, 1.0) as f32;
        let moving = k < 1.0 && self.from != self.to;
        let current = self.current(now);
        if !moving {
            // Arrived: nothing left to ease from.
            self.from = self.to;
        }
        (current, moving)
    }

    /// Where it's heading.
    pub fn target(&self) -> Option<Tints> {
        self.to
    }

    /// Something is tinted, or was until this ease began.
    pub fn showing(&self) -> bool {
        self.from.is_some() || self.to.is_some()
    }

    fn current(&self, now: f64) -> Option<Tints> {
        let k = ((now - self.since) / EASE_SECS).clamp(0.0, 1.0) as f32;
        // Smoothstep: eases in and out.
        let k = k * k * (3.0 - 2.0 * k);
        match (self.from, self.to) {
            (_, to) if k >= 1.0 => to,
            (None, None) => None,
            (from, to) => Some(from.unwrap_or_else(Tints::plain).lerp(&to.unwrap_or_else(Tints::plain), k)),
        }
    }
}

/// Where the tint lives for [`Tokens::of`] to find: egui's per-context memory.
pub fn id() -> egui::Id {
    egui::Id::unique("ond-art-tint")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(f: impl Fn(usize, usize) -> Color32) -> ColorImage {
        let n = 64;
        ColorImage::new([n, n], (0..n * n).map(|i| f(i % n, i / n)).collect())
    }

    #[test]
    fn contrast_matches_the_wcag_figures() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 0.001);
        // The theme's own muted text on white is 5.3:1, as `theme` says.
        assert!((contrast(Tokens::LIGHT.muted, Tokens::LIGHT.bg) - 5.3).abs() < 0.1);
    }

    #[test]
    fn a_colored_cover_gives_its_colors_strongest_first() {
        // Mostly a deep red, a band of blue, a little white text.
        let p = extract(&image(|x, y| {
            if y < 12 {
                Color32::from_rgb(40, 60, 170)
            } else if x < 4 {
                Color32::WHITE
            } else {
                Color32::from_rgb(170, 30, 40)
            }
        }))
        .expect("a colorful cover");
        let (h, s, _) = hsv([p.accent().r(), p.accent().g(), p.accent().b()]);
        assert!(!(0.05..=0.95).contains(&h) && s > 0.6, "red first: {:?}", p.accent());
        assert_eq!(p.colors.len(), 2, "red and blue, nothing else: {:?}", p.colors);
        assert!(p.average.r() > p.average.b());
    }

    #[test]
    fn a_grey_cover_gives_nothing() {
        assert!(extract(&image(|x, y| Color32::from_gray(((x + y) * 2) as u8))).is_none());
        assert!(extract(&image(|_, _| Color32::BLACK)).is_none());
        assert!(extract(&ColorImage::new([0, 0], Vec::new())).is_none());
        // A speck of color on grey isn't enough either.
        assert!(extract(&image(|x, y| if x < 2 && y < 2 { Color32::RED } else { Color32::from_gray(90) })).is_none());
    }

    #[test]
    fn tints_keep_text_readable_in_both_themes() {
        // Every hue at a few strengths, including the hard ones (yellow on
        // white, deep blue on near-black).
        for k in 0..24 {
            for (s, v) in [(1.0, 1.0), (0.8, 0.35), (0.5, 0.9)] {
                let c = from_hsv(k as f32 / 24.0, s, v);
                let palette = ArtPalette { average: c, colors: vec![c] };
                for (t, dark) in [(Tokens::LIGHT, false), (Tokens::DARK, true)] {
                    let tint = Tint::from_palette(&palette, &t, dark);
                    for bg in [t.bg, t.sidebar, t.panel, tint.stage, tint.display] {
                        assert!(contrast(tint.accent, bg) >= AA, "accent {:?} on {bg:?} for {c:?} (dark {dark})", tint.accent);
                    }
                    assert!(contrast(tint.on_accent, tint.accent) >= AA, "on-accent for {c:?}");
                    for surface in [tint.stage, tint.display] {
                        assert!(contrast(t.text, surface) >= 7.0, "text on {surface:?} for {c:?}");
                        assert!(contrast(t.muted, surface) >= AA, "muted on {surface:?} for {c:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn an_accent_that_cannot_be_made_readable_falls_back_to_the_theme() {
        // Nothing passes against both pure black and pure white.
        assert_eq!(readable_accent(Color32::from_rgb(200, 60, 60), &[Color32::WHITE, Color32::BLACK], false), None);
        let plain = Tint::plain(&Tokens::LIGHT);
        assert_eq!(plain.accent, Tokens::LIGHT.accent);
        assert_eq!(plain.apply(Tokens::LIGHT).accent, Tokens::LIGHT.accent);
    }

    #[test]
    fn the_tint_eases_between_covers_and_settles() {
        let red = Tints::new(&ArtPalette { average: Color32::from_rgb(150, 30, 30), colors: vec![Color32::from_rgb(170, 30, 40)] });
        let mut e = Easing::default();
        assert_eq!(e.step(None, 0.0), (None, false), "off: nothing tinted, nothing moving");
        let (start, moving) = e.step(Some(red), 1.0);
        assert!(moving);
        assert_eq!(start.unwrap().light.accent, Tokens::LIGHT.accent, "starts from the theme's own");
        let (mid, _) = e.step(Some(red), 1.0 + EASE_SECS / 2.0);
        let mid = mid.unwrap().light.accent;
        assert!(mid != Tokens::LIGHT.accent && mid != red.light.accent, "halfway is between");
        assert_eq!(e.step(Some(red), 1.0 + EASE_SECS), (Some(red), false), "and settles");
        // Turned off: eases back out, then nothing is tinted at all.
        let (_, moving) = e.step(None, 5.0);
        assert!(moving);
        assert_eq!(e.step(None, 5.0 + EASE_SECS), (None, false));
    }
}
