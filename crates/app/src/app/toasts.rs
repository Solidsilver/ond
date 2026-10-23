//! Transient messages over the bottom of the window. Notices fade after a few
//! seconds (hovering holds them); errors stay until clicked away.

use crate::icons::{self, Icon};
use crate::theme::Tokens;
use egui::{Color32, FontId, Rect, Sense, pos2, vec2};

/// How long a notice stays once the pointer leaves it.
const NOTICE_SECS: f64 = 4.0;
const MAX: usize = 3;
const WIDTH: f32 = 420.0;

#[derive(Debug, Clone)]
struct Toast {
    id: u64,
    text: String,
    error: bool,
    /// When it goes away (notices only), set the first time it's drawn.
    until: Option<f64>,
}

#[derive(Debug, Default)]
pub struct Toasts {
    items: Vec<Toast>,
    next: u64,
}

impl Toasts {
    /// Adds a message; the same text again just refreshes the one showing.
    pub fn push(&mut self, text: String, error: bool) {
        if let Some(t) = self.items.iter_mut().find(|t| t.text == text) {
            t.until = None;
            t.error |= error;
            return;
        }
        self.next += 1;
        self.items.push(Toast { id: self.next, text, error, until: None });
        while self.items.len() > MAX {
            // Notices make way before errors do.
            let at = self.items.iter().position(|t| !t.error).unwrap_or(0);
            self.items.remove(at);
        }
    }

    /// Draws the stack bottom-centre over `within`, oldest on top.
    pub fn ui(&mut self, ctx: &egui::Context, within: Rect) {
        if self.items.is_empty() {
            return;
        }
        let t = Tokens::of_ctx(ctx);
        let now = ctx.input(|i| i.time);
        let width = WIDTH.min(within.width() - 32.0);
        let font = FontId::proportional(12.5);
        let mut bottom = within.bottom() - 16.0;
        let mut gone = Vec::new();
        let mut wake: Option<f64> = None;
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("toasts")));
        for toast in self.items.iter_mut().rev() {
            let text_w = width - 28.0 - if toast.error { 14.0 } else { 0.0 };
            let galley = ctx.fonts_mut(|f| f.layout(toast.text.clone(), font.clone(), t.text, text_w));
            let h = galley.size().y + 18.0;
            let w = (galley.size().x + 28.0 + if toast.error { 14.0 } else { 0.0 }).max(160.0).min(width);
            let rect = Rect::from_min_size(pos2(within.center().x - w / 2.0, bottom - h), vec2(w, h));
            bottom = rect.top() - 8.0;
            let id = egui::Id::unique(("toast", toast.id));
            let resp = egui::Area::new(id)
                .order(egui::Order::Foreground)
                .fixed_pos(rect.min)
                .show(ctx, |ui| {
                    let (_, resp) = ui.allocate_exact_size(rect.size(), Sense::click());
                    resp
                })
                .inner;
            let hovered = resp.hovered();
            if !toast.error {
                match toast.until {
                    _ if hovered => toast.until = Some(now + NOTICE_SECS),
                    None => toast.until = Some(now + NOTICE_SECS),
                    Some(at) if now >= at => gone.push(toast.id),
                    Some(at) => wake = Some(wake.map_or(at, |w: f64| w.min(at))),
                }
            }
            if resp.clicked() {
                gone.push(toast.id);
            }
            // Fade the last second of a notice.
            let alpha = toast.until.map_or(1.0, |at| ((at - now) as f32).clamp(0.0, 1.0));
            let shadow = egui::Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha(if ctx.global_style().visuals.dark_mode { 110 } else { 34 }) };
            painter.add(shadow.as_shape(rect, 10.0));
            painter.rect(rect, 10.0, t.panel.gamma_multiply(alpha.max(0.0)), egui::Stroke::new(1.0, t.hairline.gamma_multiply(alpha)), egui::StrokeKind::Inside);
            let mut x = rect.left() + 14.0;
            if toast.error {
                painter.circle_filled(pos2(x + 3.0, rect.top() + 9.0 + galley.rows.first().map_or(8.0, |r| r.height() / 2.0)), 3.0, t.warn.gamma_multiply(alpha));
                x += 14.0;
            }
            painter.galley(pos2(x, rect.top() + 9.0), galley, t.text.gamma_multiply(alpha));
            if hovered {
                let close = Rect::from_center_size(pos2(rect.right() - 12.0, rect.top() + 12.0), vec2(8.0, 8.0));
                icons::paint(&painter, close, Icon::Close, t.muted);
            }
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Alert, true, &toast.text));
            if hovered {
                resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            }
        }
        self.items.retain(|t| !gone.contains(&t.id));
        if let Some(at) = wake {
            // Wake for the fade, then for the end.
            let left = (at - now).max(0.0);
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(if left <= 1.0 { 1.0 / 30.0 } else { left - 1.0 }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_outlive_notices_when_full() {
        let mut t = Toasts::default();
        t.push("a".into(), true);
        t.push("b".into(), false);
        t.push("c".into(), false);
        t.push("d".into(), false);
        assert_eq!(t.items.len(), MAX);
        assert!(t.items.iter().any(|x| x.text == "a"));
        assert!(!t.items.iter().any(|x| x.text == "b"));
    }

    #[test]
    fn same_text_is_not_stacked() {
        let mut t = Toasts::default();
        t.push("offline".into(), false);
        t.push("offline".into(), false);
        assert_eq!(t.items.len(), 1);
    }
}
