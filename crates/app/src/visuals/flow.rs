//! Flow: the classic visualizer, after the one iTunes shipped in the 2000s
//! (G-Force). Each frame, last frame's picture moves along a flow field
//! (zooming, turning, swirling), blurs a touch and fades; new wave shapes are
//! drawn on top in white (the waveform as lines, the spectrum as rings and
//! strings of dots); and the picture's brightness is shown through a colour
//! map, so the shapes smear into silk: orange, green or blue on black.
//!
//! The field, the shape and the colour map each change every so often,
//! morphing over a few seconds (a change that's nearly due waits for a beat),
//! and a beat kicks the field and brightens the shapes.

use super::spectrum::BANDS;
use super::visualizer::{Dot, Field, Input, Look, Morph, Palette, Picture, Rng, Warp, push};
use std::f32::consts::{PI, TAU};

/// Flow fields, wave shapes and colour maps to choose from.
const FIELDS: usize = 8;
const SHAPES: usize = 7;
/// The built-in colour maps; one more comes from the album.
const PALETTES: [Palette; 7] = [
    // Ember: orange smoke with pale highlights.
    Palette([[0.0, 0.0, 0.0, 0.0], [0.40, 0.06, 0.0, 0.16], [1.0, 0.42, 0.06, 0.48], [0.88, 0.93, 0.84, 1.0]]),
    // Emerald.
    Palette([[0.0, 0.0, 0.0, 0.0], [0.0, 0.16, 0.05, 0.16], [0.10, 0.62, 0.22, 0.48], [0.82, 1.0, 0.80, 1.0]]),
    // Cobalt.
    Palette([[0.0, 0.0, 0.0, 0.0], [0.02, 0.03, 0.32, 0.16], [0.16, 0.24, 0.95, 0.48], [0.90, 0.90, 1.0, 1.0]]),
    // Amethyst.
    Palette([[0.0, 0.0, 0.0, 0.0], [0.18, 0.02, 0.28, 0.16], [0.74, 0.24, 0.90, 0.48], [1.0, 0.86, 0.98, 1.0]]),
    // Lagoon.
    Palette([[0.0, 0.0, 0.0, 0.0], [0.0, 0.13, 0.16, 0.16], [0.08, 0.66, 0.70, 0.48], [0.85, 1.0, 1.0, 1.0]]),
    // Gold.
    Palette([[0.0, 0.0, 0.0, 0.0], [0.24, 0.11, 0.0, 0.16], [0.95, 0.70, 0.18, 0.48], [1.0, 1.0, 0.86, 1.0]]),
    // Crimson.
    Palette([[0.0, 0.0, 0.0, 0.0], [0.26, 0.0, 0.03, 0.16], [0.90, 0.10, 0.18, 0.48], [1.0, 0.84, 0.80, 1.0]]),
];
/// Brightness kept per 60th of a second: long, silky trails.
const FADE: f32 = 0.955;
/// The light a shape lays down each frame at full brightness (its area
/// times its brightness), whatever the shape: a ring of fat dots and a thin
/// line leave trails equally bright.
const INK: f32 = 0.045;

#[derive(Debug, Clone)]
pub struct Flow {
    rng: Rng,
    /// Scene time, seconds.
    t: f32,
    field: Morph,
    shape: Morph,
    palette: Morph,
    /// Each side's own variation (a direction, a count, a tilt), drawn when
    /// it's chosen: for the field and shape morphing from, and to.
    field_var: [f32; 2],
    shape_var: [[f32; 3]; 2],
    /// The album's colour map.
    album: Palette,
    /// The shapes' turn so far (faster with the music).
    spin: f32,
}

impl Flow {
    pub fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let field = Morph::new(FIELDS, (10.0, 22.0), 4.5, &mut rng);
        let shape = Morph::new(SHAPES, (7.0, 15.0), 2.5, &mut rng);
        let palette = Morph::new(PALETTES.len() + 1, (14.0, 30.0), 5.0, &mut rng);
        let v = rng.range(-1.0, 1.0);
        let sv = [rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), rng.range(-1.0, 1.0)];
        Self { rng, t: 0.0, field, shape, palette, field_var: [v, v], shape_var: [sv, sv], album: album_palette([0.6, 0.7, 0.9]), spin: 0.0 }
    }

    /// Keeps to one field, shape and colour map (for snapshots of each).
    #[cfg(test)]
    pub fn hold(&mut self, field: usize, shape: usize, palette: usize) {
        self.field.hold(field % FIELDS);
        self.shape.hold(shape % SHAPES);
        self.palette.hold(palette % (PALETTES.len() + 1));
    }

    pub fn set_colors(&mut self, glow: &[[f32; 3]]) {
        if let Some(c) = glow.first() {
            self.album = album_palette(*c);
        }
    }

    pub fn step(&mut self, input: &Input<'_>) -> Picture {
        let dt = input.dt;
        let feel = input.feel();
        self.t += dt;
        self.spin += dt * (0.15 + 0.5 * feel.mid + 0.8 * feel.beat);
        if self.field.step(dt, feel.onset, &mut self.rng) {
            self.field_var = [self.field_var[1], self.rng.range(-1.0, 1.0)];
        }
        if self.shape.step(dt, feel.onset, &mut self.rng) {
            self.shape_var = [self.shape_var[1], [self.rng.range(-1.0, 1.0), self.rng.range(-1.0, 1.0), self.rng.range(-1.0, 1.0)]];
        }
        self.palette.step(dt, false, &mut self.rng);

        let kick = feel.beat;
        let fields = [(self.field.from, self.field_var[0]), (self.field.to, self.field_var[1])].map(|(k, v)| kicked(field(k, self.t, v), kick, v).over(input.n()));
        let warp = Warp { from: fields[0], to: fields[1], mix: self.field.mix(), fade: FADE.powf(input.n()), blur: 0.55 };
        let look = Look::Mapped { from: self.colors(self.palette.from), to: self.colors(self.palette.to), mix: self.palette.mix() };
        let mut picture = Picture::new(warp, look, self.t);

        let ctx = Ctx {
            t: self.t,
            spin: self.spin,
            half: input.half,
            wave: &feel.wave,
            bands: input.bands(),
            bass: feel.bass,
            mid: feel.mid,
            kick,
            // Quiet still draws, dimly; the music brightens it.
            bright: 0.35 + 0.65 * feel.level + 0.5 * kick,
        };
        let mix = self.shape.mix();
        if mix < 1.0 {
            shape(self.shape.from, &ctx, self.shape_var[0], 1.0 - mix, &mut picture.dots);
        }
        if mix > 0.0 || self.shape.from == self.shape.to {
            shape(self.shape.to, &ctx, self.shape_var[1], if self.shape.from == self.shape.to { 1.0 } else { mix }, &mut picture.dots);
        }
        picture
    }

    fn colors(&self, i: usize) -> Palette {
        PALETTES.get(i).copied().unwrap_or(self.album)
    }
}

/// A colour map in the album's strongest color.
fn album_palette(c: [f32; 3]) -> Palette {
    let deep = c.map(|v| v * 0.3);
    let top = c.map(|v| v + (1.0 - v) * 0.75);
    Palette([[0.0, 0.0, 0.0, 0.0], [deep[0], deep[1], deep[2], 0.16], [c[0], c[1], c[2], 0.48], [top[0], top[1], top[2], 1.0]])
}

/// Flow field `kind` at time `t`, with its variation `v` (-1..1: its
/// direction, and a little of its character).
fn field(kind: usize, t: f32, v: f32) -> Field {
    let s = if v < 0.0 { -1.0 } else { 1.0 };
    let drift = |speed: f32, phase: f32| (t * speed + phase).sin();
    let still = Field::default();
    match kind {
        // Petals: a flower opening outward, its lobes turning.
        0 => Field { zoom: 1.010, turn: 0.002 * s + 0.0015 * drift(0.11, 0.0), petals: 5.0 + (v.abs() * 3.0).floor(), petal: 0.005, ..still },
        // Vortex: everything spirals into the middle.
        1 => Field { zoom: 0.993, turn: 0.002 * s, vortex: 0.011 * s, waves: 3.0, wave: 0.0008 * drift(0.2, 1.0), ..still },
        // Spiral: turning faster further out.
        2 => Field { zoom: 1.006, twist: 0.016 * s * (0.8 + 0.3 * drift(0.07, 2.0)), ..still },
        // Ripples: rings racing outward.
        3 => Field { zoom: 1.005, turn: 0.0015 * s, ripples: 12.0 + 6.0 * v.abs(), ripple: 0.008, ..still },
        // Silk: a slow wind of waves across the picture.
        4 => Field {
            zoom: 1.003,
            turn: 0.001 * drift(0.05, 0.0),
            waves: 2.5 + 2.0 * v.abs(),
            wave: 0.005,
            centre: [0.15 * drift(0.09, 0.0), 0.1 * drift(0.07, 1.0)],
            ..still
        },
        // Tunnel: rushing toward the viewer, turning.
        5 => Field { zoom: 1.02, turn: 0.007 * s, waves: 5.0, wave: 0.001, ..still },
        // Turbulence: churning in place.
        6 => Field { zoom: 0.999, vortex: 0.004 * s, waves: 6.0 + 3.0 * v.abs(), wave: 0.0035, ..still },
        // Eddy: a whirl off to one side, wandering.
        _ => Field { zoom: 1.006, twist: 0.012 * s, centre: [0.45 * drift(0.06, v * 3.0), 0.3 * drift(0.05, 1.0 + v)], ..still },
    }
}

/// A beat pushes the field outward and gives it a turn.
fn kicked(f: Field, kick: f32, v: f32) -> Field {
    Field { zoom: f.zoom * (1.0 + 0.014 * kick), turn: f.turn + 0.004 * kick * v.signum(), ..f }
}

/// What a shape is drawn from.
struct Ctx<'a> {
    t: f32,
    spin: f32,
    half: [f32; 2],
    wave: &'a [f32],
    bands: &'a [f32; BANDS],
    bass: f32,
    mid: f32,
    kick: f32,
    bright: f32,
}

impl Ctx<'_> {
    /// The waveform at `i` of `n` points along it.
    fn wave_at(&self, i: usize, n: usize) -> f32 {
        self.wave.get(i * self.wave.len() / n.max(1)).copied().unwrap_or(0.0)
    }

    /// The spectrum at `x` (0..1, low to high).
    fn band(&self, x: f32) -> f32 {
        self.bands[((x.clamp(0.0, 1.0) * (BANDS - 1) as f32) as usize).min(BANDS - 1)]
    }
}

fn rot(p: [f32; 2], a: f32) -> [f32; 2] {
    let (s, c) = a.sin_cos();
    [p[0] * c - p[1] * s, p[0] * s + p[1] * c]
}

fn white(bright: f32) -> [f32; 4] {
    [1.0, 1.0, 1.0, bright]
}

/// Joined strokes through `points`.
fn stroke(points: impl IntoIterator<Item = [f32; 2]>, radius: f32, bright: f32, alpha: f32, out: &mut Vec<Dot>) {
    let mut last = None;
    for p in points {
        if let Some(a) = last {
            push(out, Dot::line(a, p, radius, white(bright)), alpha);
        }
        last = Some(p);
    }
}

/// Draws wave shape `kind` with its variation `v`, at `alpha`, laying down
/// [`INK`] at the frame's brightness.
fn shape(kind: usize, c: &Ctx<'_>, v: [f32; 3], alpha: f32, out: &mut Vec<Dot>) {
    let start = out.len();
    match kind {
        0 => scope(c, v, 1.0, out),
        1 => ring(c, v, 1.0, out),
        2 => dotted_rings(c, v, 1.0, out),
        3 => strings(c, v, 1.0, out),
        4 => helix(c, v, 1.0, out),
        5 => spokes(c, v, 1.0, out),
        _ => chain(c, v, 1.0, out),
    }
    let drawn = &mut out[start..];
    let ink: f32 = drawn.iter().map(|d| (2.0 * d.axis[0].hypot(d.axis[1]) + 2.0 * d.radius) * 2.0 * d.radius * d.color[3]).sum();
    let k = (INK * c.bright / ink.max(1e-6)).clamp(0.2, 3.0) * alpha;
    for d in drawn {
        d.color[3] *= k;
    }
}

/// The waveform as a line across the screen, slowly turning; sometimes two.
fn scope(c: &Ctx<'_>, v: [f32; 3], alpha: f32, out: &mut Vec<Dot>) {
    const N: usize = 256;
    let w = c.half[0] * 1.15;
    let amp = 0.2 + 0.15 * c.mid + 0.12 * c.kick;
    let lines = if v[2] > 0.0 { 2 } else { 1 };
    for l in 0..lines {
        let a = v[0] * PI + c.t * 0.09 * v[1].signum() + l as f32 * 0.7;
        let points = (0..N).map(|i| rot([-w + 2.0 * w * i as f32 / (N - 1) as f32, c.wave_at(i, N) * amp * (1.0 - l as f32 * 0.4)], a));
        stroke(points, 0.0045, 0.9 * c.bright, alpha, out);
    }
}

/// The waveform round a ring that swells with the bass.
fn ring(c: &Ctx<'_>, v: [f32; 3], alpha: f32, out: &mut Vec<Dot>) {
    const N: usize = 200;
    let r = (0.36 + 0.12 * c.bass) * (1.0 + 0.15 * c.kick);
    let squash = 1.0 + 0.45 * v[0];
    let tilt = c.t * 0.05 * v[1].signum() + v[2];
    let points = (0..=N).map(|i| {
        let a = i as f32 / N as f32 * TAU + c.spin * 0.3;
        // The ends of the waveform taper to meet.
        let taper = (PI * (i % N) as f32 / N as f32).sin().sqrt();
        let rr = r + c.wave_at(i % N, N) * 0.12 * taper;
        rot([a.cos() * rr * squash, a.sin() * rr], tilt)
    });
    stroke(points, 0.0045, 0.9 * c.bright, alpha, out);
}

/// The spectrum as dots round three turning ellipses: each dot as big and
/// bright as its band (low at the top, high at the bottom, mirrored).
fn dotted_rings(c: &Ctx<'_>, v: [f32; 3], alpha: f32, out: &mut Vec<Dot>) {
    const N: usize = 72;
    for k in 0..3 {
        let kf = k as f32;
        let r = (0.2 + 0.25 * kf) * (1.0 + 0.2 * c.bass + 0.12 * c.kick);
        let squash = 0.55 + 0.35 * (c.t * 0.13 + kf + v[0] * 3.0).sin();
        let tilt = c.spin * (0.3 + 0.15 * kf) * v[1].signum() + kf;
        for j in 0..N {
            let a = j as f32 / N as f32 * TAU;
            let level = c.band((a.cos() * 0.5 + 0.5) * 0.9);
            let at = rot([a.cos() * r, a.sin() * r * squash], tilt);
            let dot = Dot::glow(at, 0.004 + 0.012 * level * level, white((0.3 + 0.8 * level) * c.bright)).crisp();
            push(out, dot, alpha);
        }
    }
}

/// Strings of dots along wandering curves, each dot sized by its band.
fn strings(c: &Ctx<'_>, v: [f32; 3], alpha: f32, out: &mut Vec<Dot>) {
    const STRANDS: usize = 5;
    const PER: usize = 30;
    for k in 0..STRANDS {
        let kf = k as f32;
        let (fx, fy) = (1.0 + (kf + v[0].abs() * 2.0) % 3.0, 2.0 + kf % 2.0);
        for j in 0..PER {
            let s = j as f32 / PER as f32 * 1.4 + c.t * (0.12 + 0.03 * kf) + kf * 1.3 + v[1];
            let at = [(fx * s).sin() * c.half[0] * 0.7, (fy * s + v[2]).cos() * 0.7];
            let level = c.band((k * PER + j) as f32 / (STRANDS * PER) as f32);
            push(out, Dot::glow(at, 0.005 + 0.016 * level, white((0.4 + 0.7 * level) * c.bright)).crisp(), alpha);
        }
    }
}

/// Two strands winding round each other across the screen, the waveform
/// rippling along them.
fn helix(c: &Ctx<'_>, v: [f32; 3], alpha: f32, out: &mut Vec<Dot>) {
    const N: usize = 200;
    let w = c.half[0] * 1.15;
    let amp = 0.4 * (0.75 + 0.25 * c.bass) * (1.0 + 0.2 * c.kick);
    let tilt = v[0] * 0.8 + 0.15 * (c.t * 0.1).sin();
    for strand in [1.0, -1.0] {
        let points = (0..N).map(|i| {
            let x = -w + 2.0 * w * i as f32 / (N - 1) as f32;
            let y = strand * amp * (2.2 * x + c.t * 0.8 * v[1].signum()).sin() + c.wave_at(i, N) * 0.07;
            rot([x, y], tilt)
        });
        stroke(points, 0.004, 0.85 * c.bright, alpha, out);
    }
}

/// The spectrum as spokes round the middle, low bands at the top.
fn spokes(c: &Ctx<'_>, v: [f32; 3], alpha: f32, out: &mut Vec<Dot>) {
    const N: usize = 96;
    let r0 = 0.1 + 0.08 * c.bass + 0.05 * c.kick;
    for j in 0..N {
        let a = j as f32 / N as f32 * TAU + c.spin * 0.25 * v[0].signum();
        let level = c.band((j as f32 / N as f32 * 2.0 - 1.0).abs());
        let len = 0.04 + 0.45 * level * level;
        let (s, co) = a.sin_cos();
        push(out, Dot::line([co * r0, s * r0], [co * (r0 + len), s * (r0 + len)], 0.004, white((0.3 + 0.7 * level) * c.bright)), alpha);
    }
}

/// A chain of dotted circles along a slow curve, each as big as its part of
/// the spectrum.
fn chain(c: &Ctx<'_>, v: [f32; 3], alpha: f32, out: &mut Vec<Dot>) {
    const CIRCLES: usize = 7;
    const PER: usize = 30;
    for k in 0..CIRCLES {
        let th = c.t * 0.2 * v[0].signum() + k as f32 * 0.38 + v[1] * 3.0;
        let centre = [th.cos() * c.half[0] * 0.55, (2.0 * th + v[2]).sin() * 0.45];
        let level = c.band(k as f32 / CIRCLES as f32);
        let r = 0.05 + 0.22 * level + 0.05 * c.kick;
        for j in 0..PER {
            let a = j as f32 / PER as f32 * TAU + c.spin;
            push(out, Dot::glow([centre[0] + a.cos() * r, centre[1] + a.sin() * r], 0.0045, white((0.45 + 0.6 * level) * c.bright)).crisp(), alpha);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visuals::visualizer::tests::music;

    #[test]
    fn every_field_shape_and_colour_map_gets_its_turn() {
        let mut s = Flow::new(5);
        let (mut fields, mut shapes, mut palettes) = (vec![false; FIELDS], vec![false; SHAPES], vec![false; PALETTES.len() + 1]);
        let mut last_mix = s.field.mix();
        // Ten minutes of music.
        for i in 0..(600 * 60) {
            let f = music(i, 30);
            s.step(&Input { dt: 1.0 / 60.0, half: [1.6, 1.0], audio: Some(&f) });
            fields[s.field.to] = true;
            shapes[s.shape.to] = true;
            palettes[s.palette.to] = true;
            // Never a cut: the mix moves a little at a time, or restarts from 0 when a morph begins.
            let mix = s.field.mix();
            assert!((mix - last_mix).abs() < 0.02 || mix == 0.0, "{last_mix} → {mix}");
            last_mix = mix;
        }
        assert!(fields.iter().all(|&b| b), "{fields:?}");
        assert!(shapes.iter().all(|&b| b), "{shapes:?}");
        assert!(palettes.iter().all(|&b| b), "{palettes:?}");
    }

    #[test]
    fn a_beat_kicks_the_field_outward() {
        let base = field(0, 0.0, 0.5);
        assert!(kicked(base, 1.0, 0.5).zoom > base.zoom + 0.01);
        assert_eq!(kicked(base, 0.0, 0.5), base);
        // A frame twice as long moves twice as far.
        let two = base.over(2.0);
        assert!((two.zoom - base.zoom * base.zoom).abs() < 1e-6 && (two.turn - 2.0 * base.turn).abs() < 1e-6);
    }
}
