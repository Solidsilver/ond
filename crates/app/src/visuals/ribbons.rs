//! Ribbons: ribbons of light in the album's colors that wind round the
//! middle of the screen, one pair each for the low, middle and high end
//! (each pair mirrored through the centre), swelling with their part of the
//! music and kicking on the beat. Last frame's picture fades, turns and curls
//! outward behind them, and a beat throws sparks off the ribbons' heads.

use super::visualizer::{Dot, Field, Input, Look, Picture, Rng, Warp, push};

/// Points along a ribbon.
const PER_RIBBON: usize = 36;
/// Low, middle and high: one pair each.
const BANDS: usize = 3;
/// Sparks a beat throws.
const SPARKS: usize = 36;

#[derive(Debug, Clone)]
struct Spark {
    at: [f32; 2],
    vel: [f32; 2],
    /// Seconds left.
    life: f32,
    color: [f32; 3],
}

#[derive(Debug, Clone)]
pub struct Ribbons {
    rng: Rng,
    t: f32,
    /// Each ribbon's progress along its path, so speed changes don't jump.
    phase: [f32; BANDS],
    colors: Vec<[f32; 3]>,
    sparks: Vec<Spark>,
}

impl Ribbons {
    pub fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let phase = std::array::from_fn(|i| i as f32 * 2.1 + rng.range(0.0, 1.0));
        Self { rng, t: 0.0, phase, colors: super::visualizer::vivid_colors(&[]), sparks: Vec::new() }
    }

    pub fn set_colors(&mut self, glow: &[[f32; 3]]) {
        self.colors = glow.to_vec();
    }

    /// Ribbon `r`'s color: a cover with fewer colors than ribbons gives the
    /// others lighter shades of them.
    fn color(&self, r: usize) -> [f32; 3] {
        let n = self.colors.len();
        let lift = if r < n { 0.0 } else { 0.25 * (r / n) as f32 };
        self.colors[r % n].map(|c| c + (1.0 - c) * lift)
    }

    /// Where ribbon `r` is at `s` along its path, at `size`.
    fn path(r: usize, s: f32, size: f32) -> [f32; 2] {
        let (fx, fy) = [(1.0, 2.0), (3.0, 2.0), (2.0, 5.0)][r];
        [(fx * s + r as f32).sin() * size * 1.25, (fy * s + 0.7 * r as f32).cos() * size]
    }

    pub fn step(&mut self, input: &Input<'_>) -> Picture {
        let dt = input.dt;
        let feel = input.feel();
        self.t += dt;
        let energy = [feel.bass, feel.mid, feel.treble];
        let beat = feel.beat;
        let n = input.n();
        let field = Field { zoom: 1.004 + 0.012 * beat, turn: 0.0015 + 0.004 * feel.mid, twist: 0.004, waves: 1.5, wave: 0.0007 * (self.t * 0.13).sin(), ..Field::default() };
        let warp = Warp { from: field.over(n), to: field.over(n), mix: 0.0, fade: (0.93 + 0.03 * (1.0 - feel.level)).powf(n), blur: 0.3 };
        let base = self.color(0);
        let look = Look::Lit { centre: base.map(|c| c * 0.035), edge: [0.0; 3] };
        let mut pic = Picture::new(warp, look, self.t);
        for (r, &e) in energy.iter().enumerate() {
            // Louder moves faster; quiet still drifts.
            self.phase[r] += dt * (0.25 + 1.1 * e + 0.6 * beat);
            let color = self.color(r);
            let size = 0.35 + 0.45 * e + 0.15 * beat;
            let points: Vec<[f32; 2]> = (0..PER_RIBBON).map(|k| Self::path(r, self.phase[r] - k as f32 * 0.025, size)).collect();
            for mirror in [1.0, -1.0] {
                for (k, w) in points.windows(2).enumerate() {
                    // The head of the ribbon is widest and brightest.
                    let head = 1.0 - k as f32 / PER_RIBBON as f32;
                    let radius = (0.008 + 0.022 * e + 0.014 * beat) * (0.35 + 0.65 * head);
                    let bright = (0.05 + 0.3 * e + 0.2 * beat) * head * if mirror > 0.0 { 1.0 } else { 0.75 };
                    let mut d = Dot::line(w[0].map(|v| v * mirror), w[1].map(|v| v * mirror), radius, [color[0], color[1], color[2], bright.min(1.0)]);
                    d.core = 0.0;
                    push(&mut pic.dots, d, 1.0);
                }
                if feel.onset {
                    let head = points[0].map(|v| v * mirror);
                    for _ in 0..SPARKS / 6 {
                        let a = self.rng.range(0.0, std::f32::consts::TAU);
                        let v = self.rng.range(0.3, 1.2) * (0.6 + 0.4 * beat);
                        self.sparks.push(Spark { at: head, vel: [a.cos() * v, a.sin() * v], life: self.rng.range(0.4, 0.9), color: color.map(|c| c + (1.0 - c) * 0.5) });
                    }
                }
            }
        }
        self.sparks(dt, &mut pic);
        pic
    }

    fn sparks(&mut self, dt: f32, pic: &mut Picture) {
        for s in &mut self.sparks {
            s.life -= dt;
            s.at = [s.at[0] + s.vel[0] * dt, s.at[1] + s.vel[1] * dt];
            s.vel = s.vel.map(|v| v * (-dt * 2.0).exp());
            push(&mut pic.dots, Dot::glow(s.at, 0.007, [s.color[0], s.color[1], s.color[2], s.life.clamp(0.0, 1.0)]), 1.0);
        }
        self.sparks.retain(|s| s.life > 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visuals::visualizer::tests::music;

    #[test]
    fn a_beat_throws_sparks_that_burn_out() {
        let mut s = Ribbons::new(1);
        s.step(&Input { dt: 1.0 / 60.0, half: [1.6, 1.0], audio: Some(&music(0, 1000)) });
        assert_eq!(s.sparks.len(), SPARKS);
        for i in 1..90 {
            s.step(&Input { dt: 1.0 / 60.0, half: [1.6, 1.0], audio: Some(&music(i, 1000)) });
        }
        assert!(s.sparks.is_empty());
    }

    #[test]
    fn a_beat_kicks_the_feedback_outward() {
        let mut s = Ribbons::new(1);
        let calm = s.step(&Input { dt: 1.0 / 60.0, half: [1.6, 1.0], audio: Some(&music(1, 1000)) }).warp;
        let kick = s.step(&Input { dt: 1.0 / 60.0, half: [1.6, 1.0], audio: Some(&music(0, 1000)) }).warp;
        assert!(kick.from.zoom > calm.from.zoom + 0.005);
    }
}
