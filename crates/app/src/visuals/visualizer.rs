//! The visualizer's scene, in homage to the old iTunes one: ribbons of light
//! that wind around the middle of the screen, swelling with their part of the
//! spectrum and kicking on the beat, while the previous frame fades, turns and
//! drifts outward behind them (the feedback is done on the GPU; see `gpu`).
//!
//! This part is plain arithmetic, run on the UI thread each frame: where every
//! light is, how big and in what color, and how the feedback moves. It's
//! deterministic for a given time step and input, so a snapshot can show a
//! fixed frame.

use super::spectrum::{BANDS, Frame};
use egui::Color32;

/// Lights per ribbon, spaced along its path.
const PER_RIBBON: usize = 28;
/// Low, middle and high: one ribbon each.
const RIBBONS: usize = 3;

/// One light: centre (x right, y up, the window's shorter side spanning -1..1),
/// radius in the same units, and color (alpha is brightness).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Light {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub color: [f32; 4],
}

/// How last frame's picture carries into this one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Feedback {
    /// Brightness kept per frame (at 60 frames a second).
    pub fade: f32,
    /// Scale toward the viewer per frame: above 1, trails flow outward.
    pub zoom: f32,
    /// Turn per frame, radians.
    pub turn: f32,
    /// Shift per frame, in texture units.
    pub drift: [f32; 2],
}

#[derive(Debug, Clone)]
pub struct Scene {
    /// Scene time, seconds of animation (not wall time: it runs with the music).
    t: f32,
    /// Each ribbon's progress along its path, so speed changes don't jump.
    phase: [f32; RIBBONS],
    /// Smoothed loudness per ribbon.
    energy: [f32; RIBBONS],
    beat: f32,
    colors: Vec<[f32; 3]>,
}

/// Colors used when the cover has none: quiet blue, violet and teal.
const DEFAULT_COLORS: [Color32; 3] = [Color32::from_rgb(0x6F, 0xA8, 0xDC), Color32::from_rgb(0x9B, 0x8A, 0xE6), Color32::from_rgb(0x5C, 0xC8, 0xB8)];

impl Default for Scene {
    fn default() -> Self {
        Self { t: 0.0, phase: [0.0, 2.1, 4.2], energy: [0.0; RIBBONS], beat: 0.0, colors: glow_colors(&DEFAULT_COLORS) }
    }
}

/// Palette colors made to glow on black: bright, never neon.
fn glow_colors(colors: &[Color32]) -> Vec<[f32; 3]> {
    colors
        .iter()
        .map(|c| {
            let [r, g, b] = [c.r(), c.g(), c.b()].map(|v| f32::from(v) / 255.0);
            let max = r.max(g).max(b).max(1e-3);
            let min = r.min(g).min(b);
            // Lift to full brightness, then ease saturation back toward 0.65 at most.
            let (r, g, b) = (r / max, g / max, b / max);
            let sat = 1.0 - min / max;
            let keep = if sat > 0.65 { 0.65 / sat } else { 1.0 };
            [1.0 - (1.0 - r) * keep, 1.0 - (1.0 - g) * keep, 1.0 - (1.0 - b) * keep]
        })
        .collect()
}

impl Scene {
    /// Takes the album's colors (strongest first); none: the defaults.
    pub fn set_colors(&mut self, colors: &[Color32]) {
        self.colors = glow_colors(if colors.is_empty() { &DEFAULT_COLORS } else { colors });
    }

    /// Advances by `dt` seconds with this frame's `audio` (`None`: nothing
    /// playing, the scene idles slowly) and returns the lights and feedback.
    pub fn step(&mut self, dt: f32, audio: Option<&Frame>) -> (Vec<Light>, Feedback) {
        let dt = dt.clamp(0.0, 0.05);
        self.t += dt;
        let thirds = |i: usize| -> f32 {
            let Some(f) = audio else { return 0.0 };
            let (a, b) = (i * BANDS / RIBBONS, (i + 1) * BANDS / RIBBONS);
            f.bands[a..b].iter().sum::<f32>() / (b - a) as f32
        };
        let beat = audio.map_or(0.0, |f| f.beat);
        self.beat = beat.max(self.beat - dt * 3.0);
        let mut lights = Vec::with_capacity(RIBBONS * PER_RIBBON);
        for r in 0..RIBBONS {
            let target = thirds(r);
            self.energy[r] += (target - self.energy[r]) * (dt * 8.0).min(1.0);
            let e = self.energy[r];
            // Louder moves faster; quiet still drifts.
            self.phase[r] += dt * (0.25 + 1.1 * e + 0.6 * self.beat);
            // A cover with fewer colors than ribbons: the others lighter shades of them.
            let base = self.colors[r % self.colors.len()];
            let lift = if r < self.colors.len() { 0.0 } else { 0.25 * (r / self.colors.len()) as f32 };
            let color = base.map(|c| c + (1.0 - c) * lift);
            // A Lissajous path per ribbon, its size breathing with its band.
            let (fx, fy) = [(1.0, 2.0), (3.0, 2.0), (2.0, 5.0)][r];
            let size = 0.35 + 0.45 * e + 0.15 * self.beat;
            for k in 0..PER_RIBBON {
                let s = self.phase[r] - k as f32 * 0.03;
                let x = (fx * s + r as f32).sin() * size * 1.25;
                let y = (fy * s + 0.7 * r as f32).cos() * size;
                // The head of the ribbon is biggest and brightest.
                let head = 1.0 - k as f32 / PER_RIBBON as f32;
                let radius = (0.014 + 0.032 * e + 0.02 * self.beat) * (0.4 + 0.6 * head);
                let bright = (0.06 + 0.36 * e + 0.22 * self.beat) * head;
                lights.push(Light { x, y, radius, color: [color[0], color[1], color[2], bright.min(1.0)] });
            }
        }
        let level = audio.map_or(0.0, |f| f.level);
        let feedback = Feedback {
            fade: 0.93 + 0.03 * (1.0 - level),
            zoom: 1.004 + 0.012 * self.beat,
            turn: 0.0015 + 0.004 * self.energy[1],
            drift: [0.0008 * (self.t * 0.13).sin(), 0.0008 * (self.t * 0.11).cos()],
        };
        (lights, feedback)
    }
}

/// Title, then artist, shown when a track starts: how visible (0..=1) at
/// `since` seconds after it started.
pub fn caption_alpha(since: f64) -> f32 {
    const IN: f64 = 0.6;
    const HOLD: f64 = 3.5;
    const OUT: f64 = 1.2;
    let a = if since < 0.0 {
        0.0
    } else if since < IN {
        since / IN
    } else if since < IN + HOLD {
        1.0
    } else {
        1.0 - (since - IN - HOLD) / OUT
    };
    a.clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loud(beat: f32) -> Frame {
        Frame { bands: [0.8; BANDS], raw: [0.8; BANDS], beat, level: 0.7, fresh: true }
    }

    #[test]
    fn the_same_input_gives_the_same_frame() {
        let (mut a, mut b) = (Scene::default(), Scene::default());
        for _ in 0..30 {
            assert_eq!(a.step(1.0 / 60.0, Some(&loud(0.2))), b.step(1.0 / 60.0, Some(&loud(0.2))));
        }
    }

    #[test]
    fn music_makes_it_bigger_brighter_and_faster_than_silence() {
        let (mut quiet, mut music) = (Scene::default(), Scene::default());
        let mut q = (Vec::new(), quiet.step(0.0, None).1);
        let mut m = q.clone();
        for _ in 0..60 {
            q = quiet.step(1.0 / 60.0, None);
            m = music.step(1.0 / 60.0, Some(&loud(0.0)));
        }
        let bright = |l: &[Light]| l.iter().map(|l| l.color[3]).sum::<f32>();
        let reach = |l: &[Light]| l.iter().map(|l| l.x.hypot(l.y)).fold(0.0, f32::max);
        assert!(bright(&m.0) > 3.0 * bright(&q.0));
        assert!(reach(&m.0) > reach(&q.0));
        assert!(music.phase[0] > quiet.phase[0]);
        // Everything stays on screen (the shorter side spans -1..1).
        assert!(m.0.iter().all(|l| l.x.abs() <= 1.6 && l.y.abs() <= 1.0));
        // Silence keeps the trails longer; music fades them faster.
        assert!(q.1.fade > m.1.fade && m.1.fade < 1.0);
    }

    #[test]
    fn a_beat_kicks_the_feedback_outward() {
        let mut s = Scene::default();
        let calm = s.step(1.0 / 60.0, Some(&loud(0.0))).1;
        let kick = s.step(1.0 / 60.0, Some(&loud(1.0))).1;
        assert!(kick.zoom > calm.zoom + 0.005);
        // And it settles again.
        let mut after = kick;
        for _ in 0..60 {
            after = s.step(1.0 / 60.0, Some(&loud(0.0))).1;
        }
        assert!((after.zoom - calm.zoom).abs() < 1e-3);
    }

    #[test]
    fn album_colors_glow_without_turning_neon() {
        let mut s = Scene::default();
        s.set_colors(&[Color32::from_rgb(120, 10, 10), Color32::from_rgb(10, 10, 10)]);
        let [r, g, b] = s.colors[0];
        assert!((r - 1.0).abs() < 1e-4, "lifted to full brightness");
        assert!(g > 0.3 && b > 0.3, "and calmed: {:?}", s.colors[0]);
        assert_eq!(s.colors[1], [1.0, 1.0, 1.0], "a near-black becomes white light");
        s.set_colors(&[]);
        assert_eq!(s.colors.len(), DEFAULT_COLORS.len());
    }

    #[test]
    fn the_caption_fades_in_holds_and_fades_out() {
        assert_eq!(caption_alpha(-1.0), 0.0);
        assert!(caption_alpha(0.3) > 0.4 && caption_alpha(0.3) < 0.6);
        assert_eq!(caption_alpha(2.0), 1.0);
        assert_eq!(caption_alpha(10.0), 0.0);
    }
}
