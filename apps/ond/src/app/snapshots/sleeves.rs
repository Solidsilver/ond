//! The README's screenshots, with made-up record sleeves in place of real
//! covers.

use super::*;

/// The README's screenshots, light and dark, into `target/snapshots`:
///
/// ```sh
/// cargo test -p ond-player --release readme_snapshots -- --ignored --nocapture
/// ```
#[test]
#[ignore = "renders PNGs for review; run with --ignored"]
fn readme_snapshots() {
    shoot_scenes(&[(egui::Theme::Light, "light"), (egui::Theme::Dark, "dark")], None, readme_scenes);
}

fn readme_scenes(h: &mut Harness<'_, Option<OndApp>>, s: &Shooter) {
    let app = app_mut(h);
    for i in 0..ALBUMS.len() {
        app.art.preload(&album_id(i), sleeve(i));
    }
    reset(h, FULL, Route::Recent);
    // Playing bit-perfect on the DAC, verified, with its own volume: what Ond is for.
    {
        let app = app_mut(h);
        app.bit_perfect = true;
        app.prefs.edit(|s| {
            s.device_modes.insert("umc".into(), crate::settings::DeviceMode::BitPerfect);
            s.output_device = Some("umc".into());
        });
        app.output.handle(&signal_with((16, 44_100), (16, 44_100), audio::BitPerfectStatus::Verified, None));
        app.output.handle(&audio::Event::DeviceVolume(Some(audio::DeviceVolume { db: -18.0, min_db: -63.5, max_db: 0.0, muted: Some(false) })));
    }
    s.shoot(h, "readme-library");
    reset(h, FULL, Route::Album(album_id(0)));
    s.shoot(h, "readme-album");
}

/// A made-up record sleeve, in one of eight styles by `i` (rings, color
/// fields, a grid, ridgelines, a horizon, halftone, overlapping circles,
/// stripes) and colors from `i`, for screenshots that look like a library
/// without showing anyone's real artwork.
fn sleeve(i: usize) -> ColorImage {
    const N: usize = 512;
    type Rgb = [f32; 3];
    let base = (i as f32 * 0.137 + 0.05).fract();
    let hsv = |dh: f32, s: f32, v: f32| -> Rgb {
        let c = Color32::from(egui::ecolor::Hsva::new((base + dh).rem_euclid(1.0), s, v, 1.0));
        [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0]
    };
    let mix = |a: Rgb, b: Rgb, t: f32| -> Rgb {
        let t = t.clamp(0.0, 1.0);
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
    };
    // An edge `w` pixels soft: 1 inside (`d` < 0), 0 outside.
    let inside = |d: f32, w: f32| (0.5 - d / w).clamp(0.0, 1.0);
    let hash = |x: usize, y: usize, k: usize| -> f32 {
        let mut n =
            (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ (k as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
        n ^= n >> 29;
        n = n.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        n ^= n >> 32;
        (n % 10_000) as f32 / 10_000.0
    };
    let n = N as f32;
    // Neighbours in a row of four, and the album under each, differ.
    let style = (i * 3 + i / 8) % 8;
    let paper = hsv(0.08, 0.12, 0.93);
    let ink = hsv(0.0, 0.7, 0.75);
    let dark = hsv(0.55, 0.45, 0.16);
    let pixel = |x: usize, y: usize| -> Rgb {
        let (fx, fy) = (x as f32, y as f32);
        let (u, v) = (fx / n, fy / n);
        match style {
            // Concentric rings off centre.
            0 => {
                let d = ((fx - n * 0.62).powi(2) + (fy - n * 0.44).powi(2)).sqrt();
                const BAND: f32 = 34.0;
                let band = (d / BAND).floor() as usize;
                let ring = if band.is_multiple_of(2) { hsv(0.0, 0.55, 0.88) } else { hsv(0.1, 0.45, 0.55) };
                mix(dark, ring, (1.15 - d / (n * 0.8)).clamp(0.0, 1.0))
            }
            // Soft color fields, stacked.
            1 => {
                let bg = hsv(0.02, 0.55, 0.42);
                let field = |c: Rgb, top: f32, bottom: f32, px: Rgb| {
                    let d = (u - 0.5).abs().max(((v - (top + bottom) / 2.0).abs()) - (bottom - top) / 2.0 + 0.4) - 0.4;
                    mix(px, c, inside(d * n, 14.0))
                };
                let px = field(hsv(0.06, 0.5, 0.82), 0.1, 0.45, bg);
                field(hsv(-0.04, 0.5, 0.28), 0.55, 0.9, px)
            }
            // A grid of tinted squares.
            2 => {
                let cells = 4.0;
                let (cx, cy) = ((u * cells).floor() as usize, (v * cells).floor() as usize);
                let (lx, ly) = ((u * cells).fract(), (v * cells).fract());
                let pick = hash(cx, cy, i);
                let c = if pick < 0.3 {
                    ink
                } else if pick < 0.55 {
                    hsv(0.5, 0.35, 0.55)
                } else if pick < 0.8 {
                    hsv(0.12, 0.45, 0.88)
                } else {
                    dark
                };
                let gap = 0.025;
                if lx < gap || ly < gap || lx > 1.0 - gap || ly > 1.0 - gap { paper } else { c }
            }
            // Ridgelines, each hiding the ones behind it.
            3 => {
                let lines = 36;
                let ridge = |k: usize| {
                    let y0 = n * 0.18 + k as f32 * n * 0.019;
                    let bump = (-((u - 0.5) / 0.16).powi(2)).exp();
                    let wobble = ((u * 23.0 + k as f32 * 1.7).sin() + (u * 41.0 + k as f32 * 0.9).sin() * 0.6) * 0.5 + 0.5;
                    y0 - bump * wobble * n * (0.06 + 0.05 * hash(k, 0, i))
                };
                let black = [0.04, 0.04, 0.05];
                for k in (0..lines).rev() {
                    let ry = ridge(k);
                    if ry - 1.5 <= fy {
                        return mix(black, [0.92, 0.92, 0.9], inside((fy - ry).abs() - 1.0, 1.2));
                    }
                }
                black
            }
            // A low sun over water.
            4 => {
                let horizon = 0.62;
                let sky = mix(hsv(0.62, 0.45, 0.35), hsv(0.02, 0.55, 0.95), (v / horizon).powf(1.6));
                let sun = ((fx - n * 0.5).powi(2) + (fy - n * (horizon - 0.17)).powi(2)).sqrt() - n * 0.09;
                if v < horizon {
                    mix(sky, hsv(0.1, 0.3, 1.0), inside(sun, 2.0))
                } else {
                    let depth = (v - horizon) / (1.0 - horizon);
                    let sea = mix(hsv(0.6, 0.45, 0.32), hsv(0.62, 0.5, 0.12), depth);
                    // Broken streaks under the sun, wider further out.
                    let row = (fy / 5.0).floor() as usize;
                    let half = 0.03 + 0.1 * depth * hash(row, 1, i);
                    let shift = (hash(row, 2, i) - 0.5) * 0.06;
                    let streak = ((fy % 5.0) < 2.0 && (u - 0.5 - shift).abs() < half) as u8 as f32;
                    mix(sea, hsv(0.08, 0.35, 0.95), streak * (0.8 - depth * 0.5))
                }
            }
            // Halftone dots growing along a diagonal.
            5 => {
                let cell = 16.0;
                let (cx, cy) = ((fx / cell).floor() * cell + cell / 2.0, (fy / cell).floor() * cell + cell / 2.0);
                let r = cell * 0.48 * ((cx + cy) / (2.0 * n)).powf(1.3);
                let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt() - r;
                mix(paper, ink, inside(d, 1.2))
            }
            // Overlapping circles, multiplied.
            6 => {
                let mut px = hsv(0.1, 0.1, 0.95);
                for (k, (cx, cy, dh)) in [(0.38, 0.4, 0.0), (0.62, 0.42, 0.33), (0.5, 0.63, 0.66)].into_iter().enumerate() {
                    let d = ((u - cx).powi(2) + (v - cy).powi(2)).sqrt() - 0.24;
                    let c = hsv(dh + k as f32 * 0.01, 0.55, 0.95);
                    let t = inside(d * n, 1.5);
                    px = [px[0] * (1.0 - t + t * c[0]), px[1] * (1.0 - t + t * c[1]), px[2] * (1.0 - t + t * c[2])];
                }
                px
            }
            // Diagonal stripes behind a solid disc.
            _ => {
                let stripe = ((u + v) * 9.0).fract() < 0.5;
                let bg = if stripe { hsv(0.0, 0.55, 0.8) } else { hsv(0.0, 0.5, 0.6) };
                let d = ((u - 0.5).powi(2) + (v - 0.5).powi(2)).sqrt() - 0.27;
                mix(bg, dark, inside(d * n, 1.5))
            }
        }
    };
    let mut pixels = Vec::with_capacity(N * N);
    for y in 0..N {
        for x in 0..N {
            let [r, g, b] = pixel(x, y);
            // A little grain, as printed sleeves have.
            let grain = (hash(x, y, 99) - 0.5) * 0.035;
            let to = |c: f32| ((c + grain).clamp(0.0, 1.0) * 255.0).round() as u8;
            pixels.push(Color32::from_rgb(to(r), to(g), to(b)));
        }
    }
    ColorImage::new([N, N], pixels)
}
