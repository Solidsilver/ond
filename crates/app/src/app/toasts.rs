//! Transient messages over the bottom of the window. Notices fade after a few
//! seconds (hovering holds them); errors and offers (a message with a button,
//! like "Switch back") stay until clicked away.

use super::Action;
use crate::icons::{self, Icon};
use crate::theme::Tokens;
use egui::{Color32, FontId, Rect, Sense, pos2, vec2};

/// How long a notice stays once the pointer leaves it.
const NOTICE_SECS: f64 = 4.0;
const MAX: usize = 3;
const WIDTH: f32 = 420.0;

struct Toast {
    id: u64,
    text: String,
    error: bool,
    /// When it goes away (notices only), set the first time it's drawn.
    until: Option<f64>,
    /// A button's label and what it does. A toast with one stays until clicked away.
    action: Option<(&'static str, Action)>,
}

impl Toast {
    fn fades(&self) -> bool {
        !self.error && self.action.is_none()
    }
}

#[derive(Default)]
pub struct Toasts {
    items: Vec<Toast>,
    next: u64,
}

impl Toasts {
    /// Starts with every startup notice showing.
    pub fn with_notices(notices: Vec<String>) -> Self {
        let mut t = Self::default();
        for n in notices {
            t.push(n, false);
        }
        t
    }

    /// The messages showing, oldest first.
    #[cfg(test)]
    pub fn texts(&self) -> Vec<&str> {
        self.items.iter().map(|t| t.text.as_str()).collect()
    }

    /// Adds a message; the same text again just refreshes the one showing.
    pub fn push(&mut self, text: String, error: bool) {
        if let Some(t) = self.items.iter_mut().find(|t| t.text == text) {
            t.until = None;
            t.error |= error;
            return;
        }
        self.add(Toast { id: 0, text, error, until: None, action: None });
    }

    /// Adds a message with a button that does `action`; the same text again
    /// replaces the one showing.
    pub fn offer(&mut self, text: String, label: &'static str, action: Action) {
        self.remove(&text);
        self.add(Toast { id: 0, text, error: false, until: None, action: Some((label, action)) });
    }

    /// Takes away the message `text`, if it's showing.
    pub fn remove(&mut self, text: &str) {
        self.items.retain(|t| t.text != text);
    }

    fn add(&mut self, mut toast: Toast) {
        self.next += 1;
        toast.id = self.next;
        self.items.push(toast);
        while self.items.len() > MAX {
            // Notices make way before errors and offers do.
            let at = self.items.iter().position(Toast::fades).unwrap_or(0);
            self.items.remove(at);
        }
    }

    /// Puts `text` in place of the message `old` if that's still showing (a
    /// progress notice moving on), else adds it.
    pub fn replace(&mut self, old: Option<&str>, text: String, error: bool) {
        match self.items.iter_mut().find(|t| Some(t.text.as_str()) == old) {
            Some(t) => *t = Toast { id: t.id, text, error, until: None, action: None },
            None => self.push(text, error),
        }
    }

    /// Draws the stack bottom-centre over `within`, oldest on top. A clicked
    /// button's action goes into `actions`.
    pub fn ui(&mut self, ctx: &egui::Context, within: Rect, actions: &mut Vec<Action>) {
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
            // An offer's button, then a close mark that always shows.
            let button = toast.action.as_ref().map(|(label, _)| ctx.fonts_mut(|f| f.layout_no_wrap((*label).to_owned(), font.clone(), t.accent)));
            let right = button.as_ref().map_or(0.0, |b| b.size().x + 16.0 + 18.0);
            let left = if toast.error { 14.0 } else { 0.0 };
            let text_w = width - 28.0 - left - right;
            let galley = ctx.fonts_mut(|f| f.layout(toast.text.clone(), font.clone(), t.text, text_w));
            let h = galley.size().y + 18.0;
            let w = (galley.size().x + 28.0 + left + right).max(160.0).min(width);
            let rect = Rect::from_min_size(pos2(within.center().x - w / 2.0, bottom - h), vec2(w, h));
            bottom = rect.top() - 8.0;
            let close = Rect::from_center_size(pos2(rect.right() - 12.0, if button.is_some() { rect.center().y } else { rect.top() + 12.0 }), vec2(8.0, 8.0));
            let button_rect = button.as_ref().map(|b| Rect::from_min_size(pos2(close.left() - 10.0 - b.size().x - 12.0, rect.center().y - 11.0), vec2(b.size().x + 12.0, 22.0)));
            let id = egui::Id::unique(("toast", toast.id));
            let (resp, pressed) = egui::Area::new(id)
                .order(egui::Order::Foreground)
                .fixed_pos(rect.min)
                .show(ctx, |ui| {
                    let (_, resp) = ui.allocate_exact_size(rect.size(), Sense::click());
                    // Added after the body, so it takes the click over it.
                    let pressed = button_rect.zip(toast.action.as_ref()).map(|(r, (label, _))| {
                        let b = ui.interact(r, id.with("action"), Sense::click());
                        b.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, *label));
                        b
                    });
                    (resp, pressed)
                })
                .inner;
            let hovered = resp.hovered() || pressed.as_ref().is_some_and(|b| b.hovered());
            if toast.fades() {
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
            if pressed.as_ref().is_some_and(|b| b.clicked())
                && let Some((_, action)) = toast.action.take()
            {
                actions.push(action);
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
            if let (Some(b), Some(r), Some(p)) = (button, button_rect, pressed.as_ref()) {
                if p.hovered() {
                    painter.rect_filled(r, 6.0, t.hover);
                }
                painter.galley(r.center() - b.size() / 2.0, b, t.accent);
            }
            if hovered || button_rect.is_some() {
                icons::paint(&painter, close, Icon::Close, t.muted);
            }
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Alert, true, &toast.text));
            if hovered {
                resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                if let Some(p) = pressed {
                    p.on_hover_cursor(egui::CursorIcon::PointingHand);
                }
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
    fn offers_stay_and_outlive_notices() {
        let mut t = Toasts::default();
        t.offer("DAC is back".into(), "Switch back", Action::Back);
        t.push("b".into(), false);
        t.push("c".into(), false);
        t.push("d".into(), false);
        assert!(t.texts().contains(&"DAC is back"));
        assert!(!t.items[0].fades());
        t.remove("DAC is back");
        assert!(!t.texts().contains(&"DAC is back"));
    }

    #[test]
    fn same_text_is_not_stacked() {
        let mut t = Toasts::default();
        t.push("offline".into(), false);
        t.push("offline".into(), false);
        assert_eq!(t.items.len(), 1);
    }
}
