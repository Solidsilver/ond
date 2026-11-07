//! Particles: after the later iTunes visualizer (Magnetosphere). A few
//! thousand glowing particles in 3D, pulled round a handful of charged
//! spheres that drift on slow orbits. A beat charges one of the spheres the
//! other way for a moment and sends its particles flying; then they fall back.
//! Some spheres are bright, some dark (they hide what's behind them). A few
//! particles draw long thin ribbons; the camera turns slowly; everything
//! leaves a short trail over a dark backdrop in the album's color.
//!
//! The simulation runs on the CPU: a few thousand particles against five
//! spheres is well under a millisecond.

use super::visualizer::{Dot, Field, Input, Look, Picture, Rng, Warp, hsv_to_rgb, push, rgb_to_hsv};
use std::collections::VecDeque;

/// Particles in all.
const COUNT: usize = 2500;
/// Of which this many draw ribbons.
const RIBBONS: usize = 10;
/// Points in a ribbon.
const RIBBON_LEN: usize = 44;
/// Spheres, and how many of them are dark.
const CHARGES: usize = 5;
const DARK: usize = 2;
/// Beyond this far from the middle, a particle starts again near a sphere.
const BOUND: f32 = 3.6;
/// The camera's distance from the middle, and its lens.
const CAMERA: f32 = 3.2;
const LENS: f32 = 2.6;

#[derive(Debug, Clone)]
struct Charge {
    /// Orbit: size, speed and phase on each axis.
    size: [f32; 3],
    speed: [f32; 3],
    phase: [f32; 3],
    /// The axis particles swirl round it.
    axis: [f32; 3],
    dark: bool,
    at: [f32; 3],
    /// 1 as a beat charges it the other way, falling back to 0.
    push: f32,
    color: usize,
}

#[derive(Debug, Clone)]
pub struct Particles {
    rng: Rng,
    t: f32,
    pos: Vec<[f32; 3]>,
    vel: Vec<[f32; 3]>,
    /// Each particle's color (an index into `colors`) and size.
    color: Vec<usize>,
    size: Vec<f32>,
    charges: Vec<Charge>,
    /// The ribbons' recent points, newest first.
    trails: Vec<VecDeque<[f32; 3]>>,
    colors: Vec<[f32; 3]>,
    backdrop: [f32; 3],
    /// The sphere the next beat charges.
    next: usize,
}

type V3 = [f32; 3];

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot3(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn unit(rng: &mut Rng) -> V3 {
    loop {
        let v = [rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), rng.range(-1.0, 1.0)];
        let l = dot3(v, v);
        if l > 0.01 && l <= 1.0 {
            return scale(v, 1.0 / l.sqrt());
        }
    }
}

impl Particles {
    pub fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let charges: Vec<Charge> = (0..CHARGES)
            .map(|i| Charge {
                size: [rng.range(0.7, 1.4), rng.range(0.4, 0.9), rng.range(0.5, 1.1)],
                speed: [rng.range(0.05, 0.16), rng.range(0.04, 0.12), rng.range(0.05, 0.14)],
                phase: [rng.range(0.0, 6.3), rng.range(0.0, 6.3), rng.range(0.0, 6.3)],
                axis: unit(&mut rng),
                dark: i >= CHARGES - DARK,
                at: [0.0; 3],
                push: 0.0,
                color: i,
            })
            .collect();
        let mut s = Self {
            rng,
            t: 0.0,
            pos: Vec::with_capacity(COUNT),
            vel: Vec::with_capacity(COUNT),
            color: Vec::with_capacity(COUNT),
            size: Vec::with_capacity(COUNT),
            charges,
            trails: vec![VecDeque::with_capacity(RIBBON_LEN); RIBBONS],
            colors: Vec::new(),
            backdrop: [0.0; 3],
            next: 0,
        };
        s.set_colors(&super::visualizer::vivid_colors(&[]));
        s.orbit(0.0);
        for _ in 0..COUNT {
            let (c, p, v) = s.spawn();
            s.pos.push(p);
            s.vel.push(v);
            s.color.push(c);
            // Mostly fine points, some larger soft ones.
            let big = s.rng.unit() < 0.1;
            s.size.push(if big { s.rng.range(2.5, 5.0) } else { s.rng.range(0.6, 1.5) });
        }
        s
    }

    pub fn set_colors(&mut self, glow: &[[f32; 3]]) {
        // The album's colors, and a paler shade of the first for the sparkle.
        let mut colors = glow.to_vec();
        colors.push(glow[0].map(|c| c + (1.0 - c) * 0.6));
        self.colors = colors;
        // A dark backdrop in the album's color.
        let [h, s, _] = rgb_to_hsv(glow[0]);
        self.backdrop = hsv_to_rgb([h, s.min(0.65), 0.11]);
    }

    /// A new particle near a random sphere, circling it: color, place, velocity.
    fn spawn(&mut self) -> (usize, V3, V3) {
        let c = &self.charges[self.rng.below(CHARGES)];
        let (at, axis) = (c.at, c.axis);
        let off = scale(unit(&mut self.rng), self.rng.range(0.3, 1.0));
        let vel = scale(cross(axis, off), self.rng.range(0.6, 1.3));
        let color = if self.rng.unit() < 0.25 { self.rng.below(self.colors.len()) } else { c.color % self.colors.len() };
        (color, add(at, off), vel)
    }

    /// Moves the spheres along their orbits to time `t`.
    fn orbit(&mut self, t: f32) {
        for c in &mut self.charges {
            c.at = std::array::from_fn(|k| c.size[k] * (t * c.speed[k] * std::f32::consts::TAU + c.phase[k]).sin());
        }
    }

    pub fn step(&mut self, input: &Input<'_>) -> Picture {
        let dt = input.dt;
        let feel = input.feel();
        self.t += dt * (0.6 + 0.6 * feel.mid);
        self.orbit(self.t);
        if feel.onset {
            let n = self.next % CHARGES;
            self.charges[n].push = feel.beat.max(0.6);
            self.next = self.next.wrapping_add(1 + self.rng.below(2));
        }
        for c in &mut self.charges {
            c.push = (c.push - dt * 5.0).max(0.0);
        }
        self.simulate(dt, feel.bass);
        self.picture(input, feel.level, feel.treble, feel.beat)
    }

    fn simulate(&mut self, dt: f32, bass: f32) {
        let pull = 0.2 + 0.3 * bass;
        let drag = (-dt * 0.9).exp();
        for i in 0..COUNT {
            let p = self.pos[i];
            let mut acc = [0.0; 3];
            for c in &self.charges {
                let d = sub(c.at, p);
                let r2 = dot3(d, d) + 0.1;
                let inv = 1.0 / (r2 * r2.sqrt());
                // Pulled in; a beat's push sends it away hard.
                let k = pull * (1.0 - 9.0 * c.push);
                acc = add(acc, scale(d, k * inv));
                acc = add(acc, scale(cross(c.axis, d), 0.3 * inv));
            }
            // A third are held loosely and drift wide, like dust.
            if i % 3 == 0 {
                acc = scale(acc, 0.35);
            }
            let a2 = dot3(acc, acc);
            if a2 > 900.0 {
                acc = scale(acc, 30.0 / a2.sqrt());
            }
            let mut v = add(scale(self.vel[i], drag), scale(acc, dt));
            let v2 = dot3(v, v);
            if v2 > 16.0 {
                v = scale(v, 4.0 / v2.sqrt());
            }
            let p = add(p, scale(v, dt));
            if dot3(p, p) > BOUND * BOUND {
                let (_, np, nv) = self.spawn();
                (self.pos[i], self.vel[i]) = (np, nv);
                if i < RIBBONS {
                    self.trails[i].clear();
                }
            } else {
                (self.pos[i], self.vel[i]) = (p, v);
            }
        }
        for (i, trail) in self.trails.iter_mut().enumerate() {
            trail.push_front(self.pos[i]);
            trail.truncate(RIBBON_LEN);
        }
    }

    /// Where the camera is: slowly turning round the middle, tilting a little.
    fn camera(&self) -> impl Fn(V3) -> ([f32; 2], f32, f32) + use<> {
        let (sy, cy) = (self.t * 0.08 + 0.5 * (self.t * 0.031).sin()).sin_cos();
        let (sp, cp) = (0.3 * (self.t * 0.05).sin()).sin_cos();
        move |p: V3| {
            let x = p[0] * cy + p[2] * sy;
            let z = -p[0] * sy + p[2] * cy;
            let y = p[1] * cp - z * sp;
            let z = p[1] * sp + z * cp + CAMERA;
            let k = LENS / z.max(0.3);
            ([x * k, y * k], k, z)
        }
    }

    fn picture(&self, input: &Input<'_>, level: f32, treble: f32, beat: f32) -> Picture {
        let n = input.n();
        let warp = Warp { from: Field::default().over(n), to: Field::default(), mix: 0.0, fade: 0.62f32.powf(n), blur: 0.3 };
        let look = Look::Lit { centre: self.backdrop, edge: self.backdrop.map(|v| v * 0.3) };
        let mut pic = Picture::new(warp, look, self.t);
        let see = self.camera();
        // Dark spheres on screen: centre, radius, depth.
        let shades: Vec<([f32; 2], f32, f32)> = self
            .charges
            .iter()
            .filter(|c| c.dark)
            .map(|c| {
                let (at, k, z) = see(c.at);
                (at, 0.2 * k, z)
            })
            .collect();
        let hidden = |at: [f32; 2], z: f32| shades.iter().any(|(c, r, cz)| z > *cz && (at[0] - c[0]).hypot(at[1] - c[1]) < *r);
        let glow = 0.3 + 0.5 * level + 0.2 * beat;
        for i in 0..COUNT {
            let (at, k, z) = see(self.pos[i]);
            // Drawn out along its path only when it's fast.
            let ahead = see(add(self.pos[i], scale(self.vel[i], 0.012)));
            let mut axis = [(ahead.0[0] - at[0]), (ahead.0[1] - at[1])];
            let len = axis[0].hypot(axis[1]);
            if len > 0.12 {
                axis = axis.map(|a| a * 0.12 / len);
            }
            let speed = dot3(self.vel[i], self.vel[i]).sqrt();
            let c = self.colors[self.color[i] % self.colors.len()];
            // A shimmer with the highs, different for each particle.
            let shimmer = 1.0 + treble * 0.8 * ((i as f32 * 12.9898 + self.t * 9.0).sin() * 0.5 + 0.5);
            let big = self.size[i] > 2.0;
            let bright = (glow * (0.45 + 0.15 * speed) * shimmer * if big { 0.45 } else { 1.0 }).min(1.2);
            let r = 0.005 * self.size[i] * k;
            let out = if hidden(at, z) { &mut pic.dots } else { &mut pic.front };
            push(out, Dot { at, axis, radius: r, core: if big { 0.0 } else { 0.55 }, color: [c[0], c[1], c[2], bright], joined: false }, 1.0);
            // A soft halo: the bloom.
            push(out, Dot::glow(at, r * 3.0, [c[0], c[1], c[2], bright * 0.06]), 1.0);
        }
        self.ribbons(&see, &hidden, glow, &mut pic);
        for c in &self.charges {
            let (at, k, z) = see(c.at);
            if c.dark {
                pic.shades.push(Dot { at, radius: 0.2 * k, core: 0.85, color: [0.0, 0.0, 0.0, 1.0], ..Dot::default() });
                continue;
            }
            let col = self.colors[c.color % self.colors.len()].map(|v| v + (1.0 - v) * 0.6);
            let out = if hidden(at, z) { &mut pic.dots } else { &mut pic.front };
            let swell = 1.0 + 0.6 * c.push + 0.3 * level;
            push(out, Dot::glow(at, 0.14 * k * swell, [col[0], col[1], col[2], 0.25 * glow]), 1.0);
            push(out, Dot::glow(at, 0.035 * k * swell, [1.0, 1.0, 1.0, 0.9]), 1.0);
        }
        pic
    }

    /// Thin twisting ribbons along the first few particles' paths.
    fn ribbons(&self, see: &impl Fn(V3) -> ([f32; 2], f32, f32), hidden: &impl Fn([f32; 2], f32) -> bool, glow: f32, pic: &mut Picture) {
        for (i, trail) in self.trails.iter().enumerate() {
            let c = self.colors[self.color[i] % self.colors.len()];
            let mut last: Option<([f32; 2], f32)> = None;
            for (j, p) in trail.iter().enumerate() {
                let (at, k, z) = see(*p);
                if let Some((prev, _)) = last {
                    let fade = 1.0 - j as f32 / RIBBON_LEN as f32;
                    // The width swells and narrows along it, like a ribbon turning.
                    let w = (0.0015 + 0.006 * (j as f32 * 0.22 + self.t * 2.0 + i as f32).sin().abs()) * k;
                    let out = if hidden(at, z) { &mut pic.dots } else { &mut pic.front };
                    let mut d = Dot::line(prev, at, w, [c[0], c[1], c[2], 0.5 * glow * fade]);
                    d.core = 0.4;
                    push(out, d, 1.0);
                }
                last = Some((at, k));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visuals::visualizer::tests::music;

    fn spread(s: &Particles) -> f32 {
        s.pos.iter().map(|p| dot3(*p, *p).sqrt()).sum::<f32>() / COUNT as f32
    }

    #[test]
    fn a_beat_sends_particles_flying_and_they_come_back() {
        let mut s = Particles::new(2);
        let quiet = music(1, 1000);
        for _ in 0..240 {
            s.step(&Input { dt: 1.0 / 60.0, half: [1.6, 1.0], audio: Some(&quiet) });
        }
        let speed = |s: &Particles| s.vel.iter().map(|v| dot3(*v, *v).sqrt()).sum::<f32>() / COUNT as f32;
        let before = speed(&s);
        // Beats on every sphere at once.
        let beat = music(0, 1000);
        for _ in 0..CHARGES {
            s.step(&Input { dt: 1.0 / 60.0, half: [1.6, 1.0], audio: Some(&beat) });
        }
        for _ in 0..6 {
            s.step(&Input { dt: 1.0 / 60.0, half: [1.6, 1.0], audio: Some(&quiet) });
        }
        assert!(speed(&s) > 1.3 * before, "{} vs {before}", speed(&s));
        for _ in 0..300 {
            s.step(&Input { dt: 1.0 / 60.0, half: [1.6, 1.0], audio: Some(&quiet) });
        }
        assert!(spread(&s) < 2.0, "they gather again: {}", spread(&s));
        assert!(s.pos.iter().all(|p| dot3(*p, *p) <= BOUND * BOUND));
    }

    #[test]
    fn dark_spheres_hide_what_is_behind_them() {
        let (_, pic) = crate::visuals::visualizer::tests::run(super::super::visualizer::Kind::Particles, 4, 120, true);
        assert_eq!(pic.shades.len(), DARK);
        assert!(!pic.dots.is_empty() && !pic.front.is_empty(), "some behind, some in front");
    }
}
