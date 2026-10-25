//! The visualizer's scenes, and what they hand the GPU each frame.
//!
//! There are three, each in its own module: [`flow`](super::flow), the
//! classic one (shapes smeared into silk by a drifting flow field, through a
//! colour map); [`particles`](super::particles), thousands of glowing
//! particles pushed around by charged spheres; and
//! [`ribbons`](super::ribbons), ribbons of light in the album's colors.
//!
//! A scene is plain arithmetic, run on the UI thread each frame: it turns
//! the time step and the music's feel into a [`Picture`]: how last frame's
//! picture carries into this one, the lights to add on top, and how to show
//! the result. `gpu` draws that; the painter fallback draws its lights. A
//! scene is deterministic for a seed, a time step and its input, so a
//! snapshot can show a fixed frame.

use super::spectrum::{BANDS, Frame};
use super::{flow, particles, ribbons};
use egui::Color32;
use serde::{Deserialize, Serialize};

/// One light: a soft glow or a crisp dot, drawn out into a streak along
/// `axis`. Units: x right, y up, the window's shorter side spanning -1..1.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Dot {
    pub at: [f32; 2],
    /// Half the streak, from the centre to one end; zero for a round dot.
    pub axis: [f32; 2],
    pub radius: f32,
    /// 0 is a soft glow, 1 a crisp dot.
    pub core: f32,
    /// Color; alpha is brightness.
    pub color: [f32; 4],
    /// One stroke of a line: its ends blend into the next stroke's instead
    /// of rounding off (so a line doesn't bead where its strokes meet).
    pub joined: bool,
}

impl Dot {
    pub fn glow(at: [f32; 2], radius: f32, color: [f32; 4]) -> Self {
        Self { at, axis: [0.0; 2], radius, core: 0.0, color, joined: false }
    }

    /// A crisp stroke from `a` to `b`, `radius` thick on each side.
    pub fn line(a: [f32; 2], b: [f32; 2], radius: f32, color: [f32; 4]) -> Self {
        let at = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        Self { at, axis: [(b[0] - a[0]) / 2.0, (b[1] - a[1]) / 2.0], radius, core: 1.0, color, joined: true }
    }

    pub fn crisp(mut self) -> Self {
        self.core = 1.0;
        self
    }
}

/// A flow field: how last frame's picture moves into this one, per 60th of a
/// second. The picture is scaled up about `centre` by `zoom` (below 1 it
/// shrinks inward), turned by `turn`, plus `twist` for each unit out from the
/// centre (spiral arms) and `vortex` near it; `petals` lobes make the zoom
/// stronger in some directions than others (by `petal`), `ripples` rings make
/// it stronger at some distances (by `ripple`), and a wave of `waves` cycles
/// across the picture pushes it sideways (by `wave`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Field {
    pub zoom: f32,
    pub turn: f32,
    pub twist: f32,
    pub vortex: f32,
    pub petals: f32,
    pub petal: f32,
    pub ripples: f32,
    pub ripple: f32,
    pub waves: f32,
    pub wave: f32,
    pub centre: [f32; 2],
}

impl Default for Field {
    fn default() -> Self {
        Self { zoom: 1.0, turn: 0.0, twist: 0.0, vortex: 0.0, petals: 0.0, petal: 0.0, ripples: 0.0, ripple: 0.0, waves: 0.0, wave: 0.0, centre: [0.0; 2] }
    }
}

impl Field {
    /// The field for a frame `n` 60ths of a second long.
    pub fn over(self, n: f32) -> Self {
        Self {
            zoom: self.zoom.powf(n),
            turn: self.turn * n,
            twist: self.twist * n,
            vortex: self.vortex * n,
            petal: self.petal * n,
            ripple: self.ripple * n,
            wave: self.wave * n,
            ..self
        }
    }
}

/// How last frame's picture carries into this one: moved along one field,
/// or part way from one to another (each point lands between where the two
/// would put it), blurred a little and faded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Warp {
    pub from: Field,
    pub to: Field,
    /// 0 is all `from`, 1 all `to`.
    pub mix: f32,
    /// Brightness kept, for this frame.
    pub fade: f32,
    /// Blur, in texels of the offscreen picture.
    pub blur: f32,
}

impl Default for Warp {
    fn default() -> Self {
        Self { from: Field::default(), to: Field::default(), mix: 0.0, fade: 0.9, blur: 0.0 }
    }
}

/// A colour map: brightness 0..=1 to color, through four stops (rgb, and
/// where the stop sits; the first at 0, increasing).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette(pub [[f32; 4]; 4]);

impl Palette {
    /// The color for brightness `i`.
    pub fn at(&self, i: f32) -> [f32; 3] {
        let s = &self.0;
        let k = if i <= s[1][3] { 0 } else if i <= s[2][3] { 1 } else { 2 };
        let (a, b) = (s[k], s[k + 1]);
        let f = ((i - a[3]) / (b[3] - a[3]).max(1e-4)).clamp(0.0, 1.0);
        std::array::from_fn(|c| a[c] + (b[c] - a[c]) * f)
    }
}

/// How the picture is shown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Look {
    /// Its brightness through a colour map, part way from one to the next.
    Mapped { from: Palette, to: Palette, mix: f32 },
    /// The light as drawn, over a backdrop shading from `centre` to `edge`.
    Lit { centre: [f32; 3], edge: [f32; 3] },
}

/// What one frame draws.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    pub warp: Warp,
    /// Lights added to the picture.
    pub dots: Vec<Dot>,
    /// Dark spheres over those (their color is ignored; alpha is how solid).
    pub shades: Vec<Dot>,
    /// Lights added over the shades.
    pub front: Vec<Dot>,
    pub look: Look,
    /// Brightness of the whole, 0..=1 (fading in after a switch).
    pub bright: f32,
    /// Scene time, seconds, for the flow field's own motion.
    pub time: f32,
}

impl Picture {
    pub fn new(warp: Warp, look: Look, time: f32) -> Self {
        Self { warp, dots: Vec::new(), shades: Vec::new(), front: Vec::new(), look, bright: 1.0, time }
    }

    /// A light's color as shown (for the painter, which has no colour map pass).
    pub fn shown(&self, d: &Dot) -> [f32; 3] {
        match self.look {
            Look::Mapped { from, to, mix } => {
                let i = d.color[0].max(d.color[1]).max(d.color[2]);
                let (a, b) = (from.at(i), to.at(i));
                std::array::from_fn(|c| a[c] + (b[c] - a[c]) * mix)
            }
            Look::Lit { .. } => [d.color[0], d.color[1], d.color[2]],
        }
    }
}

/// The visualizers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Flow,
    Particles,
    Ribbons,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Flow, Kind::Particles, Kind::Ribbons];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Flow => "Flow",
            Kind::Particles => "Particles",
            Kind::Ribbons => "Ribbons",
        }
    }

    /// The next one along (`by` 1) or back (-1), round the list.
    pub fn step(self, by: i32) -> Self {
        let n = Self::ALL.len() as i32;
        let at = Self::ALL.iter().position(|k| *k == self).unwrap_or(0) as i32;
        Self::ALL[(at + by).rem_euclid(n) as usize]
    }
}

/// Which visualizer to show (Settings › Experimental): one of them, or a
/// different one for each track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pick {
    #[default]
    Flow,
    Particles,
    Ribbons,
    Shuffle,
}

impl Pick {
    pub const ALL: [Pick; 4] = [Pick::Flow, Pick::Particles, Pick::Ribbons, Pick::Shuffle];

    pub fn label(self) -> &'static str {
        match self.kind() {
            Some(k) => k.label(),
            None => "Shuffle",
        }
    }

    /// The one visualizer picked; `None` for shuffle.
    pub fn kind(self) -> Option<Kind> {
        match self {
            Pick::Flow => Some(Kind::Flow),
            Pick::Particles => Some(Kind::Particles),
            Pick::Ribbons => Some(Kind::Ribbons),
            Pick::Shuffle => None,
        }
    }

    pub fn of(kind: Kind) -> Self {
        match kind {
            Kind::Flow => Pick::Flow,
            Kind::Particles => Pick::Particles,
            Kind::Ribbons => Pick::Ribbons,
        }
    }
}

/// Reads a pick this build doesn't know (from a newer one) as the default.
pub fn lenient_pick<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Pick, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

/// What a scene hears this frame.
#[derive(Debug, Clone, Copy)]
pub struct Input<'a> {
    /// Seconds since the last frame.
    pub dt: f32,
    /// Half the window in scene units: (aspect, 1) when it's wider than tall.
    pub half: [f32; 2],
    /// `None` when nothing plays (the scene idles).
    pub audio: Option<&'a Frame>,
}

impl Input<'_> {
    /// This frame in 60ths of a second (flow fields and fades are per 60th).
    pub fn n(&self) -> f32 {
        self.dt * 60.0
    }

    pub fn bands(&self) -> &[f32; BANDS] {
        const QUIET: [f32; BANDS] = [0.0; BANDS];
        self.audio.map_or(&QUIET, |f| &f.bands)
    }

    pub fn feel(&self) -> std::borrow::Cow<'_, super::pulse::Feel> {
        match self.audio {
            Some(f) => std::borrow::Cow::Borrowed(&f.feel),
            None => std::borrow::Cow::Owned(Default::default()),
        }
    }
}

/// The scene showing.
#[derive(Debug, Clone)]
pub enum Scene {
    Flow(Box<flow::Flow>),
    Particles(Box<particles::Particles>),
    Ribbons(Box<ribbons::Ribbons>),
}

impl Scene {
    pub fn new(kind: Kind, seed: u64) -> Self {
        match kind {
            Kind::Flow => Scene::Flow(Box::new(flow::Flow::new(seed))),
            Kind::Particles => Scene::Particles(Box::new(particles::Particles::new(seed))),
            Kind::Ribbons => Scene::Ribbons(Box::new(ribbons::Ribbons::new(seed))),
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            Scene::Flow(_) => Kind::Flow,
            Scene::Particles(_) => Kind::Particles,
            Scene::Ribbons(_) => Kind::Ribbons,
        }
    }

    /// Takes the album's colors (strongest first; none: the defaults).
    pub fn set_colors(&mut self, colors: &[Color32]) {
        let glow = vivid_colors(colors);
        match self {
            Scene::Flow(s) => s.set_colors(&glow),
            Scene::Particles(s) => s.set_colors(&glow),
            Scene::Ribbons(s) => s.set_colors(&glow),
        }
    }

    pub fn step(&mut self, input: &Input<'_>) -> Picture {
        let input = Input { dt: input.dt.clamp(0.0, 0.05), ..*input };
        match self {
            Scene::Flow(s) => s.step(&input),
            Scene::Particles(s) => s.step(&input),
            Scene::Ribbons(s) => s.step(&input),
        }
    }
}

/// Colors used when the cover has none: quiet blue, violet and teal.
pub const DEFAULT_COLORS: [Color32; 3] = [Color32::from_rgb(0x6F, 0xA8, 0xDC), Color32::from_rgb(0x9B, 0x8A, 0xE6), Color32::from_rgb(0x5C, 0xC8, 0xB8)];

/// The album's colors made into light: full brightness and clearly colored
/// (saturation 0.55 to 0.85), strongest first. Greys are left out; a cover
/// with no colors gives the defaults, and one with a single hue gets two
/// neighbours of it, so there's always some variety.
pub fn vivid_colors(colors: &[Color32]) -> Vec<[f32; 3]> {
    let mut hues: Vec<(f32, f32)> = colors.iter().map(|c| rgb_to_hsv([c.r(), c.g(), c.b()].map(|v| f32::from(v) / 255.0))).filter(|&[_, s, v]| s > 0.12 && v > 0.08).map(|[h, s, _]| (h, s.clamp(0.55, 0.85))).collect();
    if hues.is_empty() {
        hues = DEFAULT_COLORS.iter().map(|c| rgb_to_hsv([c.r(), c.g(), c.b()].map(|v| f32::from(v) / 255.0))).map(|[h, s, _]| (h, s.clamp(0.55, 0.85))).collect();
    }
    let (h0, s0) = hues[0];
    if hues.iter().all(|(h, _)| hue_gap(*h, h0) < 0.08) {
        hues.truncate(1);
        hues.extend([((h0 + 0.1).fract(), s0), ((h0 + 0.92).fract(), s0 * 0.85)]);
    }
    hues.into_iter().map(|(h, s)| hsv_to_rgb([h, s, 1.0])).collect()
}

/// How far apart two hues are round the wheel, 0..=0.5.
fn hue_gap(a: f32, b: f32) -> f32 {
    let d = (a - b).abs().fract();
    d.min(1.0 - d)
}

/// Hue (0..1), saturation and value.
pub fn rgb_to_hsv([r, g, b]: [f32; 3]) -> [f32; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= 1e-6 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    [h, if max > 0.0 { d / max } else { 0.0 }, max]
}

pub fn hsv_to_rgb([h, s, v]: [f32; 3]) -> [f32; 3] {
    let f = |n: f32| {
        let k = (n + h * 6.0) % 6.0;
        v - v * s * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    [f(5.0), f(3.0), f(1.0)]
}

/// A small, seedable random source (SplitMix64): the same seed gives the
/// same scene, for tests and snapshots.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// 0..1.
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    /// 0..n.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
}

/// One of a scene's slowly changing choices (a flow field, a shape, a colour
/// map): it holds one for a while, then morphs to another over a few seconds.
#[derive(Debug, Clone)]
pub struct Morph {
    pub from: usize,
    pub to: usize,
    /// 0..=1 through the morph; 1 when settled on `to`.
    progress: f32,
    /// Seconds until the next change, once settled.
    wait: f32,
    count: usize,
    /// How long to hold one (seconds, least and most) and to morph.
    hold: (f32, f32),
    length: f32,
}

impl Morph {
    pub fn new(count: usize, hold: (f32, f32), length: f32, rng: &mut Rng) -> Self {
        let first = rng.below(count);
        Self { from: first, to: first, progress: 1.0, wait: rng.range(hold.0, hold.1), count, hold, length }
    }

    /// How far from `from` to `to`, eased: 0..=1.
    pub fn mix(&self) -> f32 {
        let p = self.progress;
        p * p * (3.0 - 2.0 * p)
    }

    /// Moves on by `dt`. A change that's nearly due starts on a beat
    /// (`onset`), so the picture turns with the music. True when a change starts.
    pub fn step(&mut self, dt: f32, onset: bool, rng: &mut Rng) -> bool {
        if self.progress < 1.0 {
            self.progress = (self.progress + dt / self.length).min(1.0);
            return false;
        }
        self.wait -= dt;
        if self.wait > 0.0 && !(onset && self.wait < 2.5) {
            return false;
        }
        self.change(rng);
        true
    }

    /// Starts a morph to another choice now.
    pub fn change(&mut self, rng: &mut Rng) {
        self.from = self.to;
        if self.count > 1 {
            self.to = (self.from + 1 + rng.below(self.count - 1)) % self.count;
        }
        self.progress = 0.0;
        self.wait = rng.range(self.hold.0, self.hold.1);
    }
}

/// How visible (0..=1) something that appeared `since` seconds ago is, when
/// it fades in over `fade_in`, holds for `hold` and fades out over `fade_out`.
pub fn shown_for(since: f64, fade_in: f64, hold: f64, fade_out: f64) -> f32 {
    let a = if since < 0.0 {
        0.0
    } else if since < fade_in {
        since / fade_in
    } else if since < fade_in + hold {
        1.0
    } else {
        1.0 - (since - fade_in - hold) / fade_out
    };
    a.clamp(0.0, 1.0) as f32
}

/// The track's cover and names, shown when a track starts: how visible
/// (0..=1) at `since` seconds after it started.
pub fn caption_alpha(since: f64) -> f32 {
    shown_for(since, 0.6, 4.0, 1.5)
}

/// The visualizer's name, shown when it's switched to.
pub fn name_alpha(since: f64) -> f32 {
    shown_for(since, 0.15, 1.2, 0.8)
}

/// Adds `dot` with its brightness scaled by `alpha`, if it shows at all.
pub fn push(out: &mut Vec<Dot>, mut dot: Dot, alpha: f32) {
    dot.color[3] *= alpha;
    if dot.color[3] > 0.002 {
        out.push(dot);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::visuals::pulse::Feel;

    /// A frame of steady music with a beat now and then (every `every`th frame).
    pub fn music(frame: usize, every: usize) -> Frame {
        let onset = frame.is_multiple_of(every);
        let wave = (0..crate::visuals::pulse::WAVE).map(|i| (i as f32 * 0.07 + frame as f32 * 0.3).sin() * 0.8).collect();
        let bands = std::array::from_fn(|b| 0.75 - b as f32 / BANDS as f32 * 0.5);
        let feel = Feel { bass: 0.8, mid: 0.6, treble: 0.4, level: 0.7, onset, beat: if onset { 1.0 } else { 0.3 }, beats: (frame / every) as u64, wave };
        Frame { bands, raw: bands, feel, fresh: true }
    }

    /// Runs `kind` for `frames` 60ths of a second with `audio` (`None`: silence).
    pub fn run(kind: Kind, seed: u64, frames: usize, audio: bool) -> (Scene, Picture) {
        let mut s = Scene::new(kind, seed);
        let mut last = None;
        for i in 0..frames {
            let f = music(i, 30);
            last = Some(s.step(&Input { dt: 1.0 / 60.0, half: [1.6, 1.0], audio: audio.then_some(&f) }));
        }
        (s, last.expect("ran"))
    }

    fn brightness(p: &Picture) -> f32 {
        p.dots.iter().chain(&p.front).map(|d| d.color[3] * d.radius).sum()
    }

    #[test]
    fn the_same_seed_gives_the_same_frames_and_another_differs() {
        for kind in Kind::ALL {
            let (_, a) = run(kind, 7, 200, true);
            let (_, b) = run(kind, 7, 200, true);
            assert_eq!(a, b, "{kind:?}");
            let (_, c) = run(kind, 8, 200, true);
            assert_ne!(a, c, "{kind:?}: another seed, another picture");
        }
    }

    #[test]
    fn music_brightens_every_scene_and_everything_stays_near_the_screen() {
        for kind in Kind::ALL {
            let (_, quiet) = run(kind, 3, 120, false);
            let (_, loud) = run(kind, 3, 120, true);
            assert!(brightness(&loud) > 1.5 * brightness(&quiet), "{kind:?}: {} vs {}", brightness(&loud), brightness(&quiet));
            for d in loud.dots.iter().chain(&loud.front).chain(&loud.shades) {
                assert!(d.at[0].abs() < 4.0 && d.at[1].abs() < 3.0, "{kind:?}: {d:?}");
                assert!(d.color.iter().all(|c| c.is_finite()) && d.radius.is_finite());
            }
            assert!((0.0..=1.0).contains(&loud.warp.fade));
        }
    }

    #[test]
    fn kinds_step_round_and_picks_read_leniently() {
        assert_eq!(Kind::Flow.step(1), Kind::Particles);
        assert_eq!(Kind::Flow.step(-1), Kind::Ribbons);
        assert_eq!(Kind::Ribbons.step(1), Kind::Flow);
        #[derive(Deserialize)]
        struct S {
            #[serde(deserialize_with = "lenient_pick")]
            p: Pick,
        }
        assert_eq!(serde_json::from_str::<S>(r#"{"p": "particles"}"#).unwrap().p, Pick::Particles);
        assert_eq!(serde_json::from_str::<S>(r#"{"p": "hologram"}"#).unwrap().p, Pick::Flow);
        assert_eq!(serde_json::to_string(&Pick::Shuffle).unwrap(), r#""shuffle""#);
    }

    #[test]
    fn a_morph_holds_then_changes_smoothly_and_a_beat_brings_it_forward() {
        let mut rng = Rng::new(1);
        let mut m = Morph::new(5, (10.0, 10.0), 4.0, &mut rng);
        let first = m.to;
        for _ in 0..(9 * 60) {
            assert!(!m.step(1.0 / 60.0, false, &mut rng));
        }
        assert_eq!((m.from, m.to, m.mix()), (first, first, 1.0));
        // Within the last stretch of the wait, a beat starts the change.
        for _ in 0..30 {
            m.step(1.0 / 60.0, false, &mut rng);
        }
        assert!(m.step(1.0 / 60.0, true, &mut rng));
        assert_ne!(m.to, first);
        assert_eq!(m.mix(), 0.0);
        m.step(2.0, false, &mut rng);
        assert!(m.mix() > 0.3 && m.mix() < 0.7, "half way through, half mixed");
    }

    #[test]
    fn a_palette_runs_through_its_stops() {
        let p = Palette([[0.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.25], [1.0, 1.0, 0.0, 0.5], [1.0, 1.0, 1.0, 1.0]]);
        assert_eq!(p.at(0.0), [0.0; 3]);
        assert_eq!(p.at(0.25), [1.0, 0.0, 0.0]);
        assert_eq!(p.at(0.75), [1.0, 1.0, 0.5]);
        assert_eq!(p.at(4.0), [1.0; 3]);
    }

    #[test]
    fn album_colors_become_clear_bright_light() {
        // A dull red and a near-black: the red at full brightness, a little
        // calmer; the black left out, and two neighbours of the red added.
        let c = vivid_colors(&[Color32::from_rgb(120, 10, 10), Color32::from_rgb(10, 10, 10)]);
        assert_eq!(c.len(), 3);
        let [h, s, v] = rgb_to_hsv(c[0]);
        assert!(h.abs() < 0.01 && (s - 0.85).abs() < 1e-3 && (v - 1.0).abs() < 1e-4, "{:?}", c[0]);
        assert!(hue_gap(rgb_to_hsv(c[1])[0], h) > 0.05 && hue_gap(rgb_to_hsv(c[2])[0], h) > 0.05);
        // Greys only: the defaults. Several hues: kept as they are.
        assert_eq!(vivid_colors(&[Color32::GRAY]).len(), DEFAULT_COLORS.len());
        assert_eq!(vivid_colors(&[Color32::from_rgb(200, 40, 40), Color32::from_rgb(40, 40, 200)]).len(), 2);
        for rgb in [[0.2, 0.5, 0.9], [0.9, 0.1, 0.4], [0.3, 0.8, 0.2]] {
            let back = hsv_to_rgb(rgb_to_hsv(rgb));
            assert!(rgb.iter().zip(back).all(|(a, b)| (a - b).abs() < 1e-5), "{rgb:?} → {back:?}");
        }
    }

    #[test]
    fn the_caption_and_name_fade_in_hold_and_fade_out() {
        assert_eq!(caption_alpha(-1.0), 0.0);
        assert!(caption_alpha(0.3) > 0.4 && caption_alpha(0.3) < 0.6);
        assert_eq!(caption_alpha(2.0), 1.0);
        assert_eq!(caption_alpha(10.0), 0.0);
        assert_eq!(name_alpha(1.0), 1.0);
        assert_eq!(name_alpha(3.0), 0.0);
    }
}
