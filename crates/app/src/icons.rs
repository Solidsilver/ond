//! Small vector icons painted with egui shapes. egui's bundled fonts have no
//! media glyphs (⏮ ⏯ ⏭ render as boxes), and an icon font is a dependency we
//! don't need for a handful of shapes.

use egui::{Color32, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2, vec2};

#[derive(Debug, Clone, Copy)]
pub enum Icon {
    Previous,
    Play,
    Pause,
    Next,
    /// Separator between stages of the signal path.
    ArrowRight,
    Check,
    Close,
    /// A rotating arc; animates from the frame clock, so the caller keeps repaints coming.
    Spinner,
    ChevronLeft,
    ChevronRight,
    ChevronDown,
    Search,
    /// The queue column toggle: lines with a play mark.
    Queue,
    Volume,
    Shuffle,
    Person,
    Disc,
    List,
    More,
    /// A window with a small player in its corner: switch to the mini player.
    MiniPlayer,
    /// Arrows pointing out to the corners: back to the full window.
    Expand,
    /// A tall window with artwork on top and lines below: the Player.
    Player,
    /// Settings.
    Gear,
    /// Recently added.
    Clock,
    /// Albums.
    Grid,
    /// Songs.
    Note,
    /// Lyrics: a speech bubble with lines of text.
    Lyrics,
    /// Offline.
    Download,
    /// A filled circle with a down arrow: on this computer.
    Downloaded,
    /// A cloud with a slash: the server can't be reached.
    Offline,
    /// Love, not yet given.
    Heart,
    /// Loved.
    HeartFilled,
    /// A rating star, empty.
    Star,
    /// A rating star, given.
    StarFilled,
    /// New playlist.
    Plus,
    /// A question mark in a circle: help, the keyboard shortcuts.
    Help,
    /// Output devices, by kind: a headband with ear cups.
    Headphones,
    /// A small box with a knob and a jack: a DAC or audio interface.
    Dac,
    /// A screen on a stand: HDMI and DisplayPort.
    Display,
    /// The Bluetooth rune.
    Bluetooth,
    /// A triangle under rings: AirPlay.
    AirPlay,
}

/// A five-point star's outline, top point first, in icon units (−1..1):
/// points and inner corners alternate.
fn star_points() -> [(f32, f32); 10] {
    std::array::from_fn(|k| {
        let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * k as f32 / 5.0;
        let r = if k % 2 == 0 { 0.95 } else { 0.4 };
        (r * a.cos(), r * a.sin() + 0.06)
    })
}

fn heart_points(n: usize) -> Vec<(f32, f32)> {
    (0..=n)
        .map(|k| {
            let t = std::f32::consts::TAU * k as f32 / n as f32;
            let x = 16.0 * t.sin().powi(3);
            let y = 13.0 * t.cos() - 5.0 * (2.0 * t).cos() - 2.0 * (3.0 * t).cos() - (4.0 * t).cos();
            (x / 17.0, -(y + 2.5) / 15.5)
        })
        .collect()
}

/// Paints `icon` centred in `rect`.
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let r = rect.width().min(rect.height()) / 2.0;
    let p = |x: f32, y: f32| -> Pos2 { c + vec2(x * r, y * r) };
    let tri = |pts: Vec<Pos2>| Shape::convex_polygon(pts, color, Stroke::NONE);
    let bar = |x0: f32, x1: f32| Rect::from_min_max(p(x0, -0.75), p(x1, 0.75));
    let line = Stroke::new((r * 0.22).max(1.2), color);
    match icon {
        Icon::Play => {
            painter.add(tri(vec![p(-0.55, -0.8), p(0.8, 0.0), p(-0.55, 0.8)]));
        }
        Icon::Pause => {
            painter.rect_filled(bar(-0.6, -0.15), 1.0, color);
            painter.rect_filled(bar(0.15, 0.6), 1.0, color);
        }
        Icon::Previous => {
            painter.rect_filled(bar(-0.8, -0.55), 0.5, color);
            painter.add(tri(vec![p(0.75, -0.75), p(0.75, 0.75), p(-0.5, 0.0)]));
        }
        Icon::Next => {
            painter.rect_filled(bar(0.55, 0.8), 0.5, color);
            painter.add(tri(vec![p(-0.75, -0.75), p(0.5, 0.0), p(-0.75, 0.75)]));
        }
        Icon::ArrowRight => {
            painter.line_segment([p(-0.7, 0.0), p(0.7, 0.0)], line);
            painter.line_segment([p(0.25, -0.45), p(0.7, 0.0)], line);
            painter.line_segment([p(0.25, 0.45), p(0.7, 0.0)], line);
        }
        Icon::Check => {
            painter.line_segment([p(-0.7, 0.05), p(-0.2, 0.55)], line);
            painter.line_segment([p(-0.2, 0.55), p(0.75, -0.5)], line);
        }
        Icon::Close => {
            painter.line_segment([p(-0.6, -0.6), p(0.6, 0.6)], line);
            painter.line_segment([p(-0.6, 0.6), p(0.6, -0.6)], line);
        }
        Icon::ChevronLeft => {
            painter.add(Shape::line(vec![p(0.3, -0.7), p(-0.35, 0.0), p(0.3, 0.7)], line));
        }
        Icon::ChevronRight => {
            painter.add(Shape::line(vec![p(-0.3, -0.7), p(0.35, 0.0), p(-0.3, 0.7)], line));
        }
        Icon::ChevronDown => {
            painter.add(Shape::line(vec![p(-0.7, -0.3), p(0.0, 0.35), p(0.7, -0.3)], line));
        }
        Icon::Search => {
            painter.circle_stroke(p(-0.15, -0.15), r * 0.55, line);
            painter.line_segment([p(0.25, 0.25), p(0.75, 0.75)], line);
        }
        Icon::Queue => {
            for y in [-0.6, 0.0] {
                painter.line_segment([p(-0.8, y), p(0.8, y)], line);
            }
            painter.line_segment([p(-0.8, 0.6), p(0.0, 0.6)], line);
            painter.add(tri(vec![p(0.3, 0.3), p(0.85, 0.62), p(0.3, 0.95)]));
        }
        Icon::Volume => {
            painter.add(tri(vec![p(-0.8, -0.3), p(-0.35, -0.3), p(0.15, -0.75), p(0.15, 0.75), p(-0.35, 0.3), p(-0.8, 0.3)]));
            let arc = (0..=8).map(|k| {
                let a = -0.9 + 1.8 * k as f32 / 8.0;
                p(0.2 + 0.55 * a.cos(), 0.55 * a.sin())
            });
            painter.add(Shape::line(arc.collect(), line));
        }
        Icon::Shuffle => {
            painter.add(Shape::line(vec![p(-0.85, -0.55), p(-0.35, -0.55), p(0.35, 0.55), p(0.85, 0.55)], line));
            painter.add(Shape::line(vec![p(-0.85, 0.55), p(-0.35, 0.55), p(0.35, -0.55), p(0.85, -0.55)], line));
            painter.add(Shape::line(vec![p(0.55, -0.85), p(0.85, -0.55), p(0.55, -0.25)], line));
            painter.add(Shape::line(vec![p(0.55, 0.25), p(0.85, 0.55), p(0.55, 0.85)], line));
        }
        Icon::Person => {
            painter.circle_stroke(p(0.0, -0.4), r * 0.35, line);
            let arc = (0..=10).map(|k| {
                let a = std::f32::consts::PI * (1.0 + k as f32 / 10.0);
                p(0.65 * a.cos(), 0.85 + 0.6 * a.sin())
            });
            painter.add(Shape::line(arc.collect(), line));
        }
        Icon::Disc => {
            painter.circle_stroke(c, r * 0.8, line);
            painter.circle_filled(c, r * 0.18, color);
        }
        Icon::List => {
            for y in [-0.55, 0.0, 0.55] {
                painter.line_segment([p(-0.8, y), p(0.8, y)], line);
            }
        }
        Icon::More => {
            for x in [-0.6, 0.0, 0.6] {
                painter.circle_filled(p(x, 0.0), r * 0.13, color);
            }
        }
        Icon::MiniPlayer => {
            painter.rect_stroke(Rect::from_min_max(p(-0.85, -0.7), p(0.85, 0.7)), r * 0.15, line, egui::StrokeKind::Middle);
            painter.rect_filled(Rect::from_min_max(p(0.05, 0.05), p(0.6, 0.45)), r * 0.08, color);
        }
        Icon::Player => {
            painter.rect_stroke(Rect::from_min_max(p(-0.6, -0.9), p(0.6, 0.9)), r * 0.15, line, egui::StrokeKind::Middle);
            painter.rect_filled(Rect::from_min_max(p(-0.35, -0.65), p(0.35, 0.05)), r * 0.06, color);
            painter.line_segment([p(-0.35, 0.35), p(0.35, 0.35)], line);
            painter.line_segment([p(-0.35, 0.62), p(0.15, 0.62)], line);
        }
        Icon::Expand => {
            painter.add(Shape::line(vec![p(0.15, -0.8), p(0.8, -0.8), p(0.8, -0.15)], line));
            painter.line_segment([p(0.8, -0.8), p(0.2, -0.2)], line);
            painter.add(Shape::line(vec![p(-0.15, 0.8), p(-0.8, 0.8), p(-0.8, 0.15)], line));
            painter.line_segment([p(-0.8, 0.8), p(-0.2, 0.2)], line);
        }
        Icon::Help => {
            let thin = Stroke::new((r * 0.16).max(1.1), color);
            painter.circle_stroke(c, r * 0.85, thin);
            // The hook: an arc over the top, then down to the stem.
            let hook: Vec<Pos2> = (0..=10)
                .map(|k| {
                    let a = std::f32::consts::PI * (1.05 + 1.1 * k as f32 / 10.0);
                    p(0.28 * a.cos(), -0.2 + 0.26 * a.sin())
                })
                .chain([p(0.0, 0.1), p(0.0, 0.22)])
                .collect();
            painter.add(Shape::line(hook, thin));
            painter.circle_filled(p(0.0, 0.45), r * 0.09, color);
        }
        Icon::Headphones => {
            let band = (0..=12).map(|k| {
                let a = std::f32::consts::PI * (1.0 + k as f32 / 12.0);
                p(0.68 * a.cos(), 0.15 + 0.75 * a.sin())
            });
            painter.add(Shape::line(band.collect(), line));
            for x in [-0.68, 0.68] {
                let cup = Rect::from_center_size(p(x, 0.42), vec2(r * 0.42, r * 0.75));
                painter.rect_filled(cup, r * 0.14, color);
            }
        }
        Icon::Dac => {
            let body = Rect::from_min_max(p(-0.85, -0.5), p(0.85, 0.5));
            painter.rect_stroke(body, r * 0.15, line, egui::StrokeKind::Middle);
            painter.circle_stroke(p(-0.3, 0.0), r * 0.24, line);
            painter.circle_filled(p(0.42, 0.0), r * 0.12, color);
        }
        Icon::Display => {
            let screen = Rect::from_min_max(p(-0.85, -0.7), p(0.85, 0.4));
            painter.rect_stroke(screen, r * 0.1, line, egui::StrokeKind::Middle);
            painter.line_segment([p(0.0, 0.4), p(0.0, 0.75)], line);
            painter.line_segment([p(-0.4, 0.78), p(0.4, 0.78)], line);
        }
        Icon::Bluetooth => {
            painter.add(Shape::line(vec![p(-0.45, -0.4), p(0.4, 0.4), p(0.0, 0.8), p(0.0, -0.8), p(0.4, -0.4), p(-0.45, 0.4)], line));
        }
        Icon::AirPlay => {
            for rad in [0.85f32, 0.5] {
                let arc = (0..=10).map(|i| {
                    let a = std::f32::consts::PI * (1.15 + 0.7 * i as f32 / 10.0);
                    p(rad * a.cos(), 0.2 + rad * a.sin())
                });
                painter.add(Shape::line(arc.collect(), line));
            }
            painter.add(tri(vec![p(0.0, 0.05), p(0.55, 0.85), p(-0.55, 0.85)]));
        }
        Icon::Gear => {
            for k in 0..8 {
                let a = std::f32::consts::TAU * k as f32 / 8.0;
                painter.line_segment([p(0.5 * a.cos(), 0.5 * a.sin()), p(0.85 * a.cos(), 0.85 * a.sin())], Stroke::new(line.width * 1.5, color));
            }
            painter.circle_stroke(c, r * 0.55, line);
            painter.circle_stroke(c, r * 0.2, line);
        }
        Icon::Clock => {
            painter.circle_stroke(c, r * 0.8, line);
            painter.add(Shape::line(vec![p(0.0, -0.45), p(0.0, 0.0), p(0.35, 0.2)], line));
        }
        Icon::Grid => {
            for (x, y) in [(-0.8, -0.8), (0.1, -0.8), (-0.8, 0.1), (0.1, 0.1)] {
                painter.rect_stroke(Rect::from_min_max(p(x, y), p(x + 0.7, y + 0.7)), r * 0.1, line, egui::StrokeKind::Middle);
            }
        }
        Icon::Note => {
            // Filled, so it holds up large (an empty playlist's cover): a round
            // head, the stem rising from its right edge, a flag sloping down.
            let (head, top, right) = (0.42, -0.95, 0.66);
            let stem = (r * 0.2).max(1.2) / r;
            let x = -0.21 + head;
            painter.circle_filled(p(-0.21, 0.53), r * head, color);
            painter.rect_filled(Rect::from_min_max(p(x - stem, top), p(x, 0.53)), 0.0, color);
            painter.add(tri(vec![p(x - stem, top), p(right, top + 0.22), p(right, top + 0.56), p(x - stem, top + 0.34)]));
        }
        Icon::Lyrics => {
            painter.add(Shape::line(
                vec![p(-0.85, -0.7), p(0.85, -0.7), p(0.85, 0.4), p(-0.2, 0.4), p(-0.55, 0.8), p(-0.55, 0.4), p(-0.85, 0.4), p(-0.85, -0.7)],
                line,
            ));
            painter.line_segment([p(-0.5, -0.3), p(0.5, -0.3)], line);
            painter.line_segment([p(-0.5, 0.05), p(0.2, 0.05)], line);
        }
        Icon::Download => {
            painter.line_segment([p(0.0, -0.85), p(0.0, 0.3)], line);
            painter.add(Shape::line(vec![p(-0.45, -0.15), p(0.0, 0.3), p(0.45, -0.15)], line));
            painter.add(Shape::line(vec![p(-0.8, 0.35), p(-0.8, 0.8), p(0.8, 0.8), p(0.8, 0.35)], line));
        }
        Icon::Downloaded => {
            let thin = Stroke::new((r * 0.16).max(1.0), color);
            painter.circle_stroke(c, r * 0.88, thin);
            painter.line_segment([p(0.0, -0.45), p(0.0, 0.38)], thin);
            painter.add(Shape::line(vec![p(-0.32, 0.08), p(0.0, 0.4), p(0.32, 0.08)], thin));
        }
        Icon::Offline => {
            let arc = (0..=12).map(|k| {
                let a = std::f32::consts::PI * (1.0 + k as f32 / 12.0);
                p(0.1 + 0.45 * a.cos(), 0.1 + 0.45 * a.sin())
            });
            painter.add(Shape::line(arc.collect(), line));
            painter.add(Shape::line(vec![p(-0.35, 0.1), p(-0.75, 0.1), p(-0.85, 0.35), p(-0.6, 0.55), p(0.65, 0.55), p(0.85, 0.3), p(0.55, 0.1)], line));
            painter.line_segment([p(-0.8, -0.8), p(0.8, 0.8)], line);
        }
        Icon::Heart => {
            // Two open halves, notch to tip, so neither sharp point gets a
            // mitred spike from a joined line.
            let pts: Vec<Pos2> = heart_points(48).into_iter().map(|(x, y)| p(x * 0.92, y * 0.92)).collect();
            let stroke = Stroke::new((r * 0.16).max(1.1), color);
            painter.add(Shape::line(pts[..=24].to_vec(), stroke));
            painter.add(Shape::line(pts[24..].to_vec(), stroke));
        }
        Icon::HeartFilled => {
            // Each half is convex (the notch is on the split), which egui fills cleanly.
            let pts = heart_points(48);
            let half = |right: bool| -> Vec<Pos2> {
                pts.iter().filter(|(x, _)| if right { *x >= -1e-4 } else { *x <= 1e-4 }).map(|(x, y)| p(x * 0.92, y * 0.92)).collect()
            };
            painter.add(Shape::convex_polygon(half(true), color, Stroke::NONE));
            painter.add(Shape::convex_polygon(half(false), color, Stroke::NONE));
        }
        Icon::Star => {
            let pts: Vec<Pos2> = star_points().iter().map(|&(x, y)| p(x, y)).collect();
            painter.add(Shape::closed_line(pts, Stroke::new((r * 0.12).max(1.0), color)));
        }
        Icon::StarFilled => {
            // The star isn't convex: fill the inner pentagon and each point's
            // triangle, which are.
            let pts: Vec<Pos2> = star_points().iter().map(|&(x, y)| p(x, y)).collect();
            let inner: Vec<Pos2> = pts.iter().skip(1).step_by(2).copied().collect();
            painter.add(Shape::convex_polygon(inner, color, Stroke::NONE));
            for k in (0..10).step_by(2) {
                let tri = vec![pts[(k + 9) % 10], pts[k], pts[k + 1]];
                painter.add(Shape::convex_polygon(tri, color, Stroke::NONE));
            }
        }
        Icon::Plus => {
            painter.line_segment([p(0.0, -0.75), p(0.0, 0.75)], line);
            painter.line_segment([p(-0.75, 0.0), p(0.75, 0.0)], line);
        }
        Icon::Spinner => {
            let t = painter.ctx().input(|i| i.time) as f32;
            let start = t * std::f32::consts::TAU * 1.2;
            let sweep = std::f32::consts::TAU * 0.7;
            let pts = (0..=24)
                .map(|k| {
                    let a = start + sweep * k as f32 / 24.0;
                    p(0.75 * a.cos(), 0.75 * a.sin())
                })
                .collect();
            painter.add(Shape::line(pts, line));
        }
    }
}

/// An icon button of any size; `glyph` is the icon's drawn size, `color`
/// overrides the text color (e.g. muted secondary controls).
pub fn sized_button(ui: &mut Ui, icon: Icon, label: &str, size: Vec2, glyph: f32, color: Option<Color32>) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let v = ui.style().interact(&resp);
        ui.painter().rect_filled(rect.expand(v.expansion), v.corner_radius, v.weak_bg_fill);
        let fg = if ui.is_enabled() { color.unwrap_or(v.fg_stroke.color) } else { ui.visuals().weak_text_color() };
        paint(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(glyph)), icon, fg);
    }
    let enabled = ui.is_enabled();
    let label = crate::keys::shown(label);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, enabled, &*label));
    resp.on_hover_text(&*label)
}

/// A thin progress ring (0..=1) around `center`, on a faint full circle.
pub fn progress_ring(painter: &Painter, center: Pos2, radius: f32, fraction: f32, color: Color32, track: Color32) {
    let width = (radius * 0.28).max(1.3);
    painter.circle_stroke(center, radius, Stroke::new(width, track));
    let f = fraction.clamp(0.0, 1.0);
    if f <= 0.0 {
        return;
    }
    let start = -std::f32::consts::FRAC_PI_2;
    let steps = ((40.0 * f).ceil() as usize).max(2);
    let pts = (0..=steps)
        .map(|k| {
            let a = start + std::f32::consts::TAU * f * k as f32 / steps as f32;
            center + vec2(radius * a.cos(), radius * a.sin())
        })
        .collect();
    painter.add(Shape::line(pts, Stroke::new(width, color)));
}

/// An icon inline with text, at text height.
pub fn inline(ui: &mut Ui, icon: Icon, color: Color32) {
    let h = ui.text_style_height(&egui::TextStyle::Body) * 0.7;
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(h), Sense::hover());
    paint(ui.painter(), rect, icon, color);
}
