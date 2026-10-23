//! Renders the app icon: `cargo run -p listener --example make_icon`.
//!
//! Writes `packaging/macos/icon-1024.png`, `icon-256.png` (the window icon)
//! and a `Listener.iconset` that `iconutil -c icns` turns into
//! `Listener.icns`. The outputs are committed; rerun only to change the icon.
//!
//! The shape follows the macOS icon grid: an 824 pt rounded square centred on
//! a 1024 canvas with a soft shadow, a deep blue gradient, and a short
//! waveform of rounded bars.

use image::{Rgba, RgbaImage, imageops};
use std::path::Path;

const SIZE: u32 = 1024;
/// Samples per pixel edge for anti-aliasing.
const SS: u32 = 4;

fn main() {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/macos");
    std::fs::create_dir_all(&out).expect("create packaging/macos");
    let icon = render();
    icon.save(out.join("icon-1024.png")).expect("write icon-1024.png");
    imageops::resize(&icon, 256, 256, imageops::FilterType::Lanczos3).save(out.join("icon-256.png")).expect("write icon-256.png");

    let set = out.join("Listener.iconset");
    std::fs::create_dir_all(&set).expect("create iconset");
    for base in [16u32, 32, 128, 256, 512] {
        for scale in [1u32, 2] {
            let px = base * scale;
            let name = if scale == 1 { format!("icon_{base}x{base}.png") } else { format!("icon_{base}x{base}@2x.png") };
            let img = if px == SIZE { icon.clone() } else { imageops::resize(&icon, px, px, imageops::FilterType::Lanczos3) };
            img.save(set.join(name)).expect("write iconset image");
        }
    }
    println!("wrote {}", out.display());
}

/// Signed distance to a rounded rectangle centred at `c` with half-size `h`.
fn rounded_rect(p: (f32, f32), c: (f32, f32), h: (f32, f32), r: f32) -> f32 {
    let qx = (p.0 - c.0).abs() - (h.0 - r);
    let qy = (p.1 - c.1).abs() - (h.1 - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
}

fn hex(c: u32) -> [f32; 3] {
    [((c >> 16) & 0xff) as f32 / 255.0, ((c >> 8) & 0xff) as f32 / 255.0, (c & 0xff) as f32 / 255.0]
}

/// The waveform: (centre x, half height) of each bar, in canvas units.
fn bars() -> Vec<(f32, f32)> {
    let heights = [0.16, 0.30, 0.52, 0.80, 1.0, 0.68, 0.42, 0.24, 0.12];
    let gap = 66.0;
    let x0 = 512.0 - gap * (heights.len() as f32 - 1.0) / 2.0;
    heights.iter().enumerate().map(|(i, h)| (x0 + gap * i as f32, 30.0 + 190.0 * h)).collect()
}

fn render() -> RgbaImage {
    let top = hex(0x2B3A55);
    let bottom = hex(0x16202F);
    let accent = hex(0x8FB3E0);
    let white = [0.97, 0.97, 0.98];
    let tile_c = (512.0, 512.0);
    let tile_h = (412.0, 412.0);
    let tile_r = 185.0;
    let bars = bars();
    let bar_w = 19.0;

    let mut img = RgbaImage::new(SIZE, SIZE);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let mut acc = [0.0f32; 4];
            for sy in 0..SS {
                for sx in 0..SS {
                    let p = (x as f32 + (sx as f32 + 0.5) / SS as f32, y as f32 + (sy as f32 + 0.5) / SS as f32);
                    // Shadow under the tile, slightly lower and blurred by distance.
                    let shadow_d = rounded_rect(p, (tile_c.0, tile_c.1 + 12.0), tile_h, tile_r);
                    let shadow_a = (1.0 - (shadow_d / 28.0).clamp(0.0, 1.0)) * 0.32;
                    let d = rounded_rect(p, tile_c, tile_h, tile_r);
                    let (rgb, a) = if d <= 0.0 {
                        let t = ((p.1 - (tile_c.1 - tile_h.1)) / (2.0 * tile_h.1)).clamp(0.0, 1.0);
                        let mut c = mix(top, bottom, t);
                        // A faint top highlight on the tile.
                        let glow = (1.0 - t * 2.5).max(0.0) * 0.05;
                        c = mix(c, white, glow);
                        for (i, (bx, bh)) in bars.iter().enumerate() {
                            let bd = rounded_rect(p, (*bx, 512.0), (bar_w, *bh), bar_w);
                            if bd <= 0.0 {
                                // The middle bars carry the accent, fading to white at the ends.
                                let edge = (i as f32 - 4.0).abs() / 4.0;
                                c = mix(accent, white, edge);
                            }
                        }
                        (c, 1.0)
                    } else {
                        ([0.0, 0.0, 0.0], shadow_a)
                    };
                    acc[0] += rgb[0] * a;
                    acc[1] += rgb[1] * a;
                    acc[2] += rgb[2] * a;
                    acc[3] += a;
                }
            }
            let n = (SS * SS) as f32;
            let a = acc[3] / n;
            let px = if a > 0.0 {
                let un = |v: f32| ((v / acc[3]).clamp(0.0, 1.0) * 255.0).round() as u8;
                Rgba([un(acc[0]), un(acc[1]), un(acc[2]), (a * 255.0).round() as u8])
            } else {
                Rgba([0, 0, 0, 0])
            };
            img.put_pixel(x, y, px);
        }
    }
    img
}
