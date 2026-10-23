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

/// A button showing `icon`; `label` is its tooltip and accessible name.
pub fn button(ui: &mut Ui, icon: Icon, label: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 24.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let v = ui.style().interact(&resp);
        ui.painter().rect_filled(rect.expand(v.expansion), v.corner_radius, v.weak_bg_fill);
        paint(ui.painter(), rect.shrink2(vec2(8.0, 6.0)), icon, v.fg_stroke.color);
    }
    let enabled = ui.is_enabled();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, enabled, label));
    resp.on_hover_text(label)
}

/// An icon inline with text, at text height.
pub fn inline(ui: &mut Ui, icon: Icon, color: Color32) {
    let h = ui.text_style_height(&egui::TextStyle::Body) * 0.7;
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(h), Sense::hover());
    paint(ui.painter(), rect, icon, color);
}
