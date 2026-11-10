//! Transient messages over the bottom of the window. Notices fade after a few
//! seconds (hovering holds them); errors and offers (a message with a button,
//! like "Switch back") stay until clicked away. A button hands back the app's
//! own action, `A`, for the app to run after the frame.
//!
//! Three variations on those: a [`Toasts::note`] is a notice with a button
//! that still fades (a confirmation with a way back, like "Cleared 12 songs ·
//! Undo"); an [`Toasts::error`] is an error with a button (Retry); and an
//! offer made with [`Toasts::offer_or`] hands back a second action when it's
//! clicked away (or pushed out), for an app whose state says the offer is
//! open and has to hear that it closed.
//!
//! An error is told from a notice by shape as well as colour: it leads with
//! [`icons::Status::Failed`], and its × always shows. The stack is one
//! column of one width (the widest message's, capped), newest at the foot,
//! each message wrapping to two lines and then cut short. At most [`MAX`]
//! show; when another comes, notices make way first, then offers, and an
//! error only for a newer one when nothing else is left to go.

use crate::icons::{self, Icon, Status};
use crate::theme::Tokens;
use egui::text::{LayoutJob, TextWrapping};
use egui::{Color32, FontId, Rect, Sense, pos2, vec2};

/// How long a notice stays once the pointer leaves it.
const NOTICE_SECS: f64 = 4.0;
/// The most toasts that show at once.
pub const MAX: usize = 3;
/// The stack's widest, and its narrowest.
const WIDTH: f32 = 420.0;
const MIN_WIDTH: f32 = 200.0;
/// From the toast's left edge to its text, and the room an error's mark takes.
const PAD: f32 = 14.0;
const MARK: f32 = 20.0;
/// The × at the right edge, its hit area this wide.
const CLOSE: f32 = 26.0;
const FONT: f32 = 12.5;

struct Toast<A> {
    id: u64,
    text: String,
    error: bool,
    /// When it goes away (notices only), set the first time it's drawn.
    until: Option<f64>,
    /// A button's label and what it does. A toast with one stays until
    /// clicked away, unless it's `brief`.
    action: Option<(&'static str, A)>,
    /// Fades as a notice does, button or not.
    brief: bool,
    /// What clicking it away does, besides taking it away.
    dismissed: Option<A>,
}

impl<A> Toast<A> {
    fn new(text: String, error: bool, action: Option<(&'static str, A)>, brief: bool, dismissed: Option<A>) -> Self {
        Self { id: 0, text, error, until: None, action, brief, dismissed }
    }

    fn fades(&self) -> bool {
        !self.error && (self.action.is_none() || self.brief)
    }

    /// Whether its × shows without the pointer on it.
    fn closable(&self) -> bool {
        self.error || (self.action.is_some() && !self.brief)
    }
}

/// The toasts showing, and what each one's button does (`A`).
pub struct Toasts<A> {
    items: Vec<Toast<A>>,
    next: u64,
    /// Dismissals owed for offers pushed out by newer toasts; handed back
    /// with the next frame's actions.
    dropped: Vec<A>,
}

impl<A> Default for Toasts<A> {
    fn default() -> Self {
        Self { items: Vec::new(), next: 0, dropped: Vec::new() }
    }
}

impl<A> Toasts<A> {
    /// Starts with every startup notice showing.
    pub fn with_notices(notices: Vec<String>) -> Self {
        let mut t = Self::default();
        for n in notices {
            t.push(n, false);
        }
        t
    }

    /// The messages showing, oldest first.
    pub fn texts(&self) -> Vec<&str> {
        self.items.iter().map(|t| t.text.as_str()).collect()
    }

    /// The label of the button on the message `text`, if it has one.
    pub fn button(&self, text: &str) -> Option<&'static str> {
        self.items.iter().find(|t| t.text == text).and_then(|t| t.action.as_ref()).map(|(label, _)| *label)
    }

    /// Whether the message `text` is showing as an error.
    pub fn is_error(&self, text: &str) -> bool {
        self.items.iter().any(|t| t.text == text && t.error)
    }

    /// Adds a message; the same text again just refreshes the one showing.
    /// An error stays until clicked away; a notice fades.
    pub fn push(&mut self, text: String, error: bool) {
        if let Some(t) = self.items.iter_mut().find(|t| t.text == text) {
            t.until = None;
            t.error |= error;
            return;
        }
        self.add(Toast::new(text, error, None, false, None));
    }

    /// An error with a button that does `action` (Retry). It stays until
    /// clicked away or its button is taken; the same text again replaces the
    /// one showing.
    pub fn error(&mut self, text: String, label: &'static str, action: A) {
        self.remove(&text);
        self.add(Toast::new(text, true, Some((label, action)), false, None));
    }

    /// Adds a message with a button that does `action`; the same text again
    /// replaces the one showing.
    pub fn offer(&mut self, text: String, label: &'static str, action: A) {
        self.remove(&text);
        self.add(Toast::new(text, false, Some((label, action)), false, None));
    }

    /// [`offer`](Self::offer), and `dismissed` when it's clicked away (or
    /// pushed out by newer toasts) rather than its button taken.
    pub fn offer_or(&mut self, text: String, label: &'static str, action: A, dismissed: A) {
        self.remove(&text);
        self.add(Toast::new(text, false, Some((label, action)), false, Some(dismissed)));
    }

    /// A notice with a button: it fades as a notice does (hovering holds it),
    /// and the button does `action` while it shows. The same text again
    /// replaces the one showing.
    pub fn note(&mut self, text: String, label: &'static str, action: A) {
        self.remove(&text);
        self.add(Toast::new(text, false, Some((label, action)), true, None));
    }

    /// Takes away the message `text`, if it's showing.
    pub fn remove(&mut self, text: &str) {
        self.items.retain(|t| t.text != text);
    }

    fn add(&mut self, mut toast: Toast<A>) {
        self.next += 1;
        toast.id = self.next;
        self.items.push(toast);
        while self.items.len() > MAX {
            // The oldest notice goes first, then the oldest offer; errors
            // only make way for newer errors.
            let at = (self.items.iter().position(Toast::fades)).or_else(|| self.items.iter().position(|t| !t.error)).unwrap_or(0);
            if let Some(dismissed) = self.items.remove(at).dismissed {
                self.dropped.push(dismissed);
            }
        }
    }

    /// Puts `text` in place of the message `old` if that's still showing (a
    /// progress notice moving on), else adds it.
    pub fn replace(&mut self, old: Option<&str>, text: String, error: bool) {
        match self.items.iter_mut().find(|t| Some(t.text.as_str()) == old) {
            Some(t) => *t = Toast { id: t.id, ..Toast::new(text, error, None, false, None) },
            None => self.push(text, error),
        }
    }

    /// Draws the stack bottom-centre over `within`, newest at the foot. A
    /// clicked button's action goes into `actions`. Only a fading notice asks
    /// for frames of its own; toasts that stay ask for none.
    pub fn ui(&mut self, ctx: &egui::Context, within: Rect, actions: &mut Vec<A>) {
        actions.append(&mut self.dropped);
        if self.items.is_empty() {
            return;
        }
        let t = Tokens::of_ctx(ctx);
        let now = ctx.input(|i| i.time);
        let font = FontId::proportional(FONT);
        let buttons: Vec<Option<std::sync::Arc<egui::Galley>>> = (self.items.iter())
            .map(|toast| toast.action.as_ref().map(|(label, _)| ctx.fonts_mut(|f| f.layout_no_wrap((*label).to_owned(), font.clone(), t.accent))))
            .collect();
        // One width for the stack: the widest toast's, capped, so its edges line up.
        let cap = WIDTH.min(within.width() - 32.0).max(MIN_WIDTH.min(within.width() - 16.0));
        let chrome = |toast: &Toast<A>, button: Option<&std::sync::Arc<egui::Galley>>| {
            PAD + if toast.error { MARK } else { 0.0 } + button.map_or(6.0, |b| 10.0 + b.size().x + 16.0 + 2.0) + CLOSE
        };
        let natural = (self.items.iter().zip(&buttons))
            .map(|(toast, b)| chrome(toast, b.as_ref()) + ctx.fonts_mut(|f| f.layout_no_wrap(toast.text.clone(), font.clone(), t.text)).size().x)
            .fold(0.0, f32::max);
        let width = natural.clamp(MIN_WIDTH, cap.max(MIN_WIDTH)).min(cap);
        let left = within.center().x - width / 2.0;
        let mut bottom = within.bottom() - 16.0;
        let mut gone = Vec::new();
        let mut wake: Option<f64> = None;
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("toasts")));
        for (toast, button) in self.items.iter_mut().zip(buttons).rev() {
            let text_w = width - chrome(toast, button.as_ref());
            let mut job = LayoutJob::simple(toast.text.clone(), font.clone(), t.text, text_w);
            job.wrap = TextWrapping { max_width: text_w.max(1.0), max_rows: 2, break_anywhere: false, overflow_character: Some('…') };
            let galley = ctx.fonts_mut(|f| f.layout_job(job));
            let h = (galley.size().y + 18.0).max(if button.is_some() { 36.0 } else { 0.0 });
            let rect = Rect::from_min_size(pos2(left, bottom - h), vec2(width, h));
            bottom = rect.top() - 8.0;
            let close_rect = Rect::from_center_size(pos2(rect.right() - CLOSE / 2.0 - 2.0, rect.center().y), vec2(22.0, 22.0));
            let button_rect = button
                .as_ref()
                .map(|b| Rect::from_min_size(pos2(close_rect.left() - 2.0 - b.size().x - 16.0, rect.center().y - 12.0), vec2(b.size().x + 16.0, 24.0)));
            let id = egui::Id::unique(("toast", toast.id));
            let (resp, pressed, close) = egui::Area::new(id)
                .order(egui::Order::Foreground)
                .fixed_pos(rect.min)
                .show(ctx, |ui| {
                    let (_, resp) = ui.allocate_exact_size(rect.size(), Sense::click());
                    // Added after the body, so they take the click over it.
                    let pressed = button_rect.zip(toast.action.as_ref()).map(|(r, (label, _))| {
                        let b = ui.interact(r, id.with("action"), Sense::click());
                        b.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, *label));
                        b
                    });
                    let close = ui.interact(close_rect, id.with("close"), Sense::click());
                    close.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Dismiss"));
                    (resp, pressed, close)
                })
                .inner;
            let hovered = resp.hovered() || close.hovered() || pressed.as_ref().is_some_and(|b| b.hovered());
            if toast.fades() {
                match toast.until {
                    _ if hovered => toast.until = Some(now + NOTICE_SECS),
                    None => toast.until = Some(now + NOTICE_SECS),
                    Some(at) if now >= at => gone.push(toast.id),
                    Some(at) => wake = Some(wake.map_or(at, |w: f64| w.min(at))),
                }
            }
            if resp.clicked() || close.clicked() {
                gone.push(toast.id);
                if let Some(dismissed) = toast.dismissed.take() {
                    actions.push(dismissed);
                }
            }
            if pressed.as_ref().is_some_and(|b| b.clicked())
                && let Some((_, action)) = toast.action.take()
            {
                actions.push(action);
                gone.push(toast.id);
            }
            // Fade the last second of a notice.
            let alpha = toast.until.map_or(1.0, |at| ((at - now) as f32).clamp(0.0, 1.0));
            let shadow = egui::Shadow {
                offset: [0, 6],
                blur: 20,
                spread: 0,
                color: Color32::from_black_alpha(if ctx.global_style().visuals.dark_mode { 110 } else { 34 }).gamma_multiply(alpha),
            };
            painter.add(shadow.as_shape(rect, 10.0));
            painter.rect(rect, 10.0, t.panel.gamma_multiply(alpha), egui::Stroke::new(1.0, t.hairline.gamma_multiply(alpha)), egui::StrokeKind::Inside);
            let mut x = rect.left() + PAD;
            let first_row = rect.top() + 9.0 + galley.rows.first().map_or(8.0, |r| r.height() / 2.0);
            if toast.error {
                let mut fade = painter.clone();
                fade.multiply_opacity(alpha);
                icons::status_mark(&fade, pos2(x + 6.0, first_row), 6.0, Status::Failed, None);
                x += MARK;
            }
            painter.galley(pos2(x, rect.top() + 9.0), galley, t.text.gamma_multiply(alpha));
            if let (Some(b), Some(r), Some(p)) = (button, button_rect, pressed.as_ref()) {
                if p.hovered() {
                    painter.rect_filled(r, 6.0, t.hover);
                }
                painter.galley(r.center() - b.size() / 2.0, b, t.accent.gamma_multiply(alpha));
            }
            if hovered || toast.closable() {
                if close.hovered() {
                    painter.rect_filled(close_rect, 6.0, t.hover);
                }
                icons::paint(&painter, Rect::from_center_size(close_rect.center(), vec2(8.0, 8.0)), Icon::Close, t.muted.gamma_multiply(alpha));
            }
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Alert, true, &toast.text));
            if hovered {
                resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                close.on_hover_cursor(egui::CursorIcon::PointingHand);
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
    use egui_kittest::kittest::Queryable;

    type Stack = (Toasts<&'static str>, Vec<&'static str>);

    /// The stack drawn over a window `size` wide and tall.
    fn harness(toasts: Toasts<&'static str>, size: egui::Vec2) -> egui_kittest::Harness<'static, (bool, Stack)> {
        let mut h = egui_kittest::Harness::builder().with_size(size).build_ui_state(
            |ui, (installed, (toasts, actions)): &mut (bool, Stack)| {
                if !std::mem::replace(installed, true) {
                    crate::theme::install(ui.ctx(), crate::theme::ThemePref::Light);
                    ui.ctx().request_repaint();
                    return;
                }
                toasts.ui(&ui.ctx().clone(), ui.ctx().content_rect(), actions);
            },
            (false, (toasts, Vec::new())),
        );
        h.run_steps(2);
        h
    }

    fn rect_of(h: &egui_kittest::Harness<'static, (bool, Stack)>, text: &str) -> Rect {
        h.get_by_label(text).rect()
    }

    #[test]
    fn errors_outlive_notices_when_full() {
        let mut t = Toasts::<&str>::default();
        t.push("a".into(), true);
        t.push("b".into(), false);
        t.push("c".into(), false);
        t.push("d".into(), false);
        assert_eq!(t.items.len(), MAX);
        assert!(t.items.iter().any(|x| x.text == "a"));
        assert!(!t.items.iter().any(|x| x.text == "b"));
    }

    #[test]
    fn a_full_stack_drops_the_oldest_notice_then_offers_and_keeps_errors() {
        let mut t = Toasts::<&str>::default();
        t.error("Couldn't load the album".into(), "Retry", "retry");
        t.offer("DAC is back".into(), "Switch back", "back");
        t.push("Couldn't sync".into(), true);
        t.push("Added 6 songs".into(), false);
        assert_eq!(t.texts(), ["Couldn't load the album", "DAC is back", "Couldn't sync"], "a notice makes way first, even its own");
        t.push("Couldn't save the heart".into(), true);
        assert_eq!(t.texts(), ["Couldn't load the album", "Couldn't sync", "Couldn't save the heart"], "then an offer");
        t.push("Couldn't change the playlist".into(), true);
        assert_eq!(t.texts(), ["Couldn't sync", "Couldn't save the heart", "Couldn't change the playlist"], "only then the oldest error");
        assert!(t.is_error("Couldn't sync"));
    }

    #[test]
    fn an_offer_pushed_out_hands_back_its_dismissal() {
        let mut toasts = Toasts::<&str>::default();
        toasts.offer_or("Picked up".into(), "Undo", "undo", "dismissed");
        for e in ["one", "two", "three"] {
            toasts.push(e.into(), true);
        }
        assert!(!toasts.texts().contains(&"Picked up"));
        let mut h = harness(toasts, vec2(800.0, 600.0));
        h.run_steps(1);
        assert_eq!(h.state().1.1, ["dismissed"], "its app hears that it closed");
    }

    #[test]
    fn offers_stay_and_outlive_notices() {
        let mut t = Toasts::<&str>::default();
        t.offer("DAC is back".into(), "Switch back", "back");
        t.push("b".into(), false);
        t.push("c".into(), false);
        t.push("d".into(), false);
        assert!(t.texts().contains(&"DAC is back"));
        assert!(!t.items[0].fades());
        t.remove("DAC is back");
        assert!(!t.texts().contains(&"DAC is back"));
    }

    #[test]
    fn a_note_fades_with_its_button_and_an_offer_can_hear_it_was_dismissed() {
        let mut t = Toasts::<&str>::default();
        t.note("Bookmarked".into(), "Show", "show");
        assert!(t.items[0].fades(), "a confirmation goes on its own, button and all");
        t.offer_or("Picked up".into(), "Undo", "undo", "dismissed");
        assert!(!t.items[1].fades());
        assert_eq!(t.items[1].dismissed, Some("dismissed"));
        t.error("Couldn't load".into(), "Retry", "retry");
        assert!(!t.items[2].fades(), "an error with a button stays");
        assert_eq!(t.button("Couldn't load"), Some("Retry"));
    }

    #[test]
    fn clicking_an_offer_away_hands_back_its_dismissal_and_its_button_its_action() {
        let mut toasts = Toasts::<&str>::default();
        toasts.offer_or("Picked up".into(), "Undo", "undo", "dismissed");
        let mut h = super::super::themed((toasts, Vec::new()), |ui, (toasts, actions): &mut (Toasts<&str>, Vec<&str>)| {
            toasts.ui(&ui.ctx().clone(), ui.ctx().content_rect(), actions);
        });
        h.run_steps(2);
        h.get_by_label("Picked up").click();
        h.run_steps(2);
        assert_eq!(h.state().1.1, ["dismissed"]);
        assert!(h.state().1.0.texts().is_empty());
        h.state_mut().1.0.offer_or("Picked up".into(), "Undo", "undo", "dismissed");
        h.run_steps(2);
        h.get_by_label("Undo").click();
        h.run_steps(2);
        assert_eq!(h.state().1.1, ["dismissed", "undo"], "the button's action, and not the dismissal too");
    }

    #[test]
    fn an_error_has_a_dismiss_button_and_its_retry_hands_back_the_action() {
        let mut toasts = Toasts::<&str>::default();
        toasts.error("Couldn't load the album".into(), "Retry", "retry");
        toasts.push("Couldn't sync".into(), true);
        let mut h = harness(toasts, vec2(800.0, 600.0));
        h.get_by_label("Retry").click();
        h.run_steps(2);
        assert_eq!(h.state().1.1, ["retry"]);
        assert_eq!(h.state().1.0.texts(), ["Couldn't sync"]);
        h.get_by_label("Dismiss").click();
        h.run_steps(2);
        assert!(h.state().1.0.texts().is_empty(), "the × takes an error away");
    }

    #[test]
    fn the_stack_is_one_width_newest_at_the_foot() {
        let mut toasts = Toasts::<&str>::default();
        toasts.push("Short".into(), false);
        toasts.error("Couldn't load Sunday at the Village Vanguard".into(), "Retry", "retry");
        toasts.note("Cleared 12 songs".into(), "Undo", "undo");
        let h = harness(toasts, vec2(1000.0, 700.0));
        let rects = ["Short", "Couldn't load Sunday at the Village Vanguard", "Cleared 12 songs"].map(|t| rect_of(&h, t));
        for r in &rects[1..] {
            assert_eq!((r.left(), r.right()), (rects[0].left(), rects[0].right()), "straight edges");
        }
        assert!(rects[0].bottom() < rects[1].top() && rects[1].bottom() < rects[2].top(), "oldest on top: {rects:?}");
        assert!(rects[0].width() <= WIDTH);
    }

    #[test]
    fn a_long_message_wraps_to_two_lines_and_then_is_cut_short() {
        let long = "Couldn't load the songs of 14 albums: the server said the request was malformed, and then it said a great deal more besides that nobody needs to read in a toast.";
        let mut toasts = Toasts::<&str>::default();
        toasts.push("One line".into(), true);
        toasts.push(long.into(), true);
        let h = harness(toasts, vec2(1000.0, 700.0));
        let (one, two) = (rect_of(&h, "One line"), rect_of(&h, long));
        assert_eq!(one.width(), WIDTH, "the widest sets the width, capped");
        assert!(two.height() > one.height() && two.height() < one.height() * 2.2, "two lines and no more: {one:?} {two:?}");
        // In a narrow window, the stack keeps a margin either side.
        let mut toasts = Toasts::<&str>::default();
        toasts.push(long.into(), true);
        let h = harness(toasts, vec2(320.0, 600.0));
        let r = rect_of(&h, long);
        assert!(r.left() >= 16.0 && r.right() <= 304.0, "{r:?}");
    }

    #[test]
    fn a_notice_fades_and_asks_for_frames_until_it_goes_and_staying_toasts_ask_for_none() {
        let mut toasts = Toasts::<&str>::default();
        toasts.push("Added 6 songs".into(), false);
        let mut h = harness(toasts, vec2(800.0, 600.0));
        h.step();
        assert!(h.ctx.has_requested_repaint(), "a fading notice wakes itself");
        for _ in 0..600 {
            h.step();
            if h.state().1.0.texts().is_empty() {
                break;
            }
        }
        assert!(h.state().1.0.texts().is_empty(), "it went on its own");
        h.state_mut().1.0.error("Couldn't load".into(), "Retry", "retry");
        h.state_mut().1.0.offer("DAC is back".into(), "Switch back", "back");
        h.state_mut().1.0.push("Couldn't sync".into(), true);
        h.run_steps(3);
        for _ in 0..20 {
            h.step();
            assert!(!h.ctx.has_requested_repaint(), "toasts that stay ask for no frames");
        }
        assert_eq!(h.state().1.0.texts().len(), 3, "and they stay");
    }

    #[test]
    fn same_text_is_not_stacked() {
        let mut t = Toasts::<&str>::default();
        t.push("offline".into(), false);
        t.push("offline".into(), false);
        assert_eq!(t.items.len(), 1);
    }
}
