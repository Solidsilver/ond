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
    /// Settings.
    Gear,
    /// Recently added.
    Clock,
    /// Albums.
    Grid,
    /// Songs.
    Note,
    /// Offline.
    Download,
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
        Icon::Expand => {
            painter.add(Shape::line(vec![p(0.15, -0.8), p(0.8, -0.8), p(0.8, -0.15)], line));
            painter.line_segment([p(0.8, -0.8), p(0.2, -0.2)], line);
            painter.add(Shape::line(vec![p(-0.15, 0.8), p(-0.8, 0.8), p(-0.8, 0.15)], line));
            painter.line_segment([p(-0.8, 0.8), p(-0.2, 0.2)], line);
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
            painter.line_segment([p(0.25, 0.45), p(0.25, -0.8)], line);
            painter.add(Shape::line(vec![p(0.25, -0.8), p(0.7, -0.6), p(0.7, -0.3), p(0.25, -0.45)], line));
            painter.circle_filled(p(-0.1, 0.5), r * 0.33, color);
        }
        Icon::Download => {
            painter.line_segment([p(0.0, -0.8), p(0.0, 0.3)], line);
            painter.add(Shape::line(vec![p(-0.45, -0.1), p(0.0, 0.35), p(0.45, -0.1)], line));
            painter.line_segment([p(-0.75, 0.8), p(0.75, 0.8)], line);
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
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, enabled, label));
    resp.on_hover_text(label)
}
