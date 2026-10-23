//! Small shared pieces: elided text, quiet links, the accent button, a
//! segmented control, a thin slider, a switch and a menu label.

use crate::theme::{Tokens, semibold};
use egui::text::{LayoutJob, TextWrapping};
use egui::{Align2, Color32, FontId, Galley, Rect, Response, Sense, Ui, vec2};
use std::sync::Arc;

/// One line of text cut to `width` with an ellipsis.
pub fn elide(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = TextWrapping { max_width: width.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    ui.painter().layout_job(job)
}

/// Text that reads as plain until hovered, then underlines: links where they
/// earn it, without turning the page blue.
pub fn link(ui: &mut Ui, text: &str, font: FontId, color: Color32) -> Response {
    // Never wider than the room it's in: a long name wraps (in a wrapping row,
    // across the whole row) instead of widening its column.
    let cap = if ui.layout().main_wrap { ui.max_rect().width() } else { ui.available_width() };
    let job = |underline: bool| {
        let mut job = LayoutJob::single_section(
            text.to_owned(),
            egui::TextFormat {
                font_id: font.clone(),
                color,
                underline: if underline { egui::Stroke::new(1.0, color) } else { egui::Stroke::NONE },
                ..Default::default()
            },
        );
        job.wrap.max_width = cap.max(24.0);
        job
    };
    let galley = ui.painter().layout_job(job(false));
    let (rect, resp) = ui.allocate_exact_size(galley.size(), Sense::click());
    if ui.is_rect_visible(rect) {
        let galley = if resp.hovered() { ui.painter().layout_job(job(true)) } else { galley };
        ui.painter().galley(rect.min, galley, color);
    }
    let label = text.to_owned();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, &label));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The filled accent button (Play).
pub fn primary(ui: &mut Ui, icon: Option<crate::icons::Icon>, text: &str) -> Response {
    let t = Tokens::of(ui);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(13.0, semibold()), t.on_accent);
    let icon_w = if icon.is_some() { 16.0 } else { 0.0 };
    let size = vec2(galley.size().x + icon_w + 32.0, 30.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if resp.hovered() { t.accent.gamma_multiply(0.88) } else { t.accent };
        ui.painter().rect_filled(rect, 6.0, fill);
        let mut x = rect.left() + 16.0;
        if let Some(icon) = icon {
            crate::icons::paint(ui.painter(), Rect::from_center_size(egui::pos2(x + 5.0, rect.center().y), vec2(10.0, 10.0)), icon, t.on_accent);
            x += icon_w;
        }
        ui.painter().galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, t.on_accent);
    }
    let label = text.to_owned();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    resp
}

/// A filled button for a destructive choice (Delete in a confirmation).
pub fn danger(ui: &mut Ui, text: &str) -> Response {
    let t = Tokens::of(ui);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(13.0, semibold()), Color32::WHITE);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 32.0, 30.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if resp.hovered() { t.danger.gamma_multiply(0.88) } else { t.danger };
        ui.painter().rect_filled(rect, 6.0, fill);
        ui.painter().galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
    }
    let label = text.to_owned();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    resp
}

/// A heart that loves / unloves: filled accent when on, a quiet outline when off.
pub fn heart_button(ui: &mut Ui, on: bool, size: f32, glyph: f32) -> Response {
    let t = Tokens::of(ui);
    let (icon, color, label) = if on { (crate::icons::Icon::HeartFilled, t.accent, "Unlove") } else { (crate::icons::Icon::Heart, t.muted, "Love") };
    crate::icons::sized_button(ui, icon, label, vec2(size, size), glyph, Some(color))
}

/// A quiet button with a tinted fill (Shuffle next to Play).
pub fn secondary(ui: &mut Ui, text: &str) -> Response {
    let t = Tokens::of(ui);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(13.0, semibold()), t.accent);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 28.0, 30.0), Sense::click());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, 6.0, if resp.hovered() { t.selected } else { t.field });
        ui.painter().galley(rect.center() - galley.size() / 2.0, galley, t.accent);
    }
    let label = text.to_owned();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    resp
}

/// Mutually exclusive choices in a pill (Up Next / History, sort order).
/// Returns the index clicked, if any.
pub fn segmented(ui: &mut Ui, options: &[&str], selected: usize) -> Option<usize> {
    let font = FontId::proportional(12.0);
    let natural: Vec<f32> = options.iter().map(|o| ui.painter().layout_no_wrap((*o).to_owned(), font.clone(), Color32::WHITE).size().x).collect();
    segments(ui, options, &fit_segments(&natural, ui.available_width()), selected)
}

/// Segment widths for labels of `natural` width: comfortable padding when it
/// fits in `room`, tighter padding next, and finally the room shared out (the
/// labels then elide) — the control never grows past its column.
pub fn fit_segments(natural: &[f32], room: f32) -> Vec<f32> {
    let room = (room - 4.0).max(0.0);
    for pad in [22.0, 14.0, 8.0] {
        let w: Vec<f32> = natural.iter().map(|n| n + pad).collect();
        if w.iter().sum::<f32>() <= room {
            return w;
        }
    }
    vec![room / natural.len().max(1) as f32; natural.len()]
}

/// [`segmented`] with equal segments filling `width`.
pub fn segmented_wide(ui: &mut Ui, options: &[&str], selected: usize, width: f32) -> Option<usize> {
    let each = (width - 4.0) / options.len().max(1) as f32;
    segments(ui, options, &vec![each; options.len()], selected)
}

fn segments(ui: &mut Ui, options: &[&str], widths: &[f32], selected: usize) -> Option<usize> {
    let t = Tokens::of(ui);
    let font = FontId::proportional(12.0);
    let widths = widths.to_vec();
    let size = vec2(widths.iter().sum::<f32>() + 4.0, 26.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, 7.0, t.field);
    let mut x = rect.left() + 2.0;
    let mut clicked = None;
    for (i, (opt, w)) in options.iter().zip(widths).enumerate() {
        let r = Rect::from_min_size(egui::pos2(x, rect.top() + 2.0), vec2(w, rect.height() - 4.0));
        let resp = ui.interact(r, ui.scope_id().with(("seg", i, *opt)), Sense::click());
        if i == selected {
            ui.painter().rect_filled(r, 5.0, t.bg);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 5.0, t.hover);
        }
        let color = if i == selected { t.text } else { t.muted };
        let g = elide(ui, opt, font.clone(), color, w - 6.0);
        ui.painter().galley(r.center() - g.size() / 2.0, g, color);
        let label = (*opt).to_owned();
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, i == selected, &label));
        if resp.clicked() {
            clicked = Some(i);
        }
        x += w;
    }
    clicked
}

/// How far along something is on its way to this computer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mark {
    Done,
    /// 0..=1; 0 while waiting its turn.
    Progress(f32),
    Failed,
}

impl Mark {
    pub fn of_track(state: Option<&offline::TrackState>) -> Option<Mark> {
        Some(match state? {
            offline::TrackState::Done { .. } => Mark::Done,
            offline::TrackState::Queued => Mark::Progress(0.0),
            offline::TrackState::Downloading { done, total } => Mark::Progress(total.map_or(0.0, |t| *done as f32 / t.max(1) as f32)),
            offline::TrackState::Failed { .. } => Mark::Failed,
        })
    }

    pub fn of_pin(sum: Option<&offline::PinSummary>) -> Option<Mark> {
        let sum = sum?;
        Some(if sum.complete() {
            Mark::Done
        } else if sum.failed > 0 && !sum.downloading && sum.done + sum.failed == sum.total {
            Mark::Failed
        } else {
            Mark::Progress(sum.fraction())
        })
    }

    pub fn hint(self) -> &'static str {
        match self {
            Mark::Done => "Downloaded",
            Mark::Progress(f) if f > 0.0 => "Downloading",
            Mark::Progress(_) => "Waiting to download",
            Mark::Failed => "Download failed",
        }
    }
}

/// The small glyph for a [`Mark`], centred at `center`, about `size` across.
pub fn mark(ui: &Ui, center: egui::Pos2, size: f32, m: Mark) {
    let t = Tokens::of(ui);
    let r = size / 2.0;
    match m {
        Mark::Done => crate::icons::paint(ui.painter(), Rect::from_center_size(center, vec2(size, size)), crate::icons::Icon::Downloaded, t.muted),
        Mark::Progress(f) => crate::icons::progress_ring(ui.painter(), center, r * 0.8, f, t.accent, t.hairline),
        Mark::Failed => {
            ui.painter().circle_stroke(center, r * 0.8, egui::Stroke::new(1.3, t.warn));
            ui.painter().text(center, Align2::CENTER_CENTER, "!", FontId::new(size * 0.7, semibold()), t.warn);
        }
    }
}

/// A small pill of tinted text (a finding worth a glance, not an alarm).
pub fn chip(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(11.0), color);
    let size = galley.size() + vec2(14.0, 4.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, rect.height() / 2.0, color.gamma_multiply(0.13));
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text));
    resp
}

/// Muted small-caps-ish section label.
pub fn section(ui: &mut Ui, text: &str) {
    let t = Tokens::of(ui);
    ui.label(egui::RichText::new(text).font(FontId::new(11.0, semibold())).color(t.muted));
}

/// A thin horizontal slider for `value` in 0..=1 (volume): a 3 px track, the
/// filled part in the accent, a small round knob that grows on hover.
/// Dragging, clicking, the scroll wheel and ←/→ (when focused) all adjust it.
pub fn thin_slider(ui: &mut Ui, value: &mut f32, width: f32, label: &str) -> Response {
    let t = Tokens::of(ui);
    let enabled = ui.is_enabled();
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, 18.0), Sense::click_and_drag());
    let knob_r = 6.0;
    let track = Rect::from_min_max(egui::pos2(rect.left() + knob_r, rect.center().y - 1.5), egui::pos2(rect.right() - knob_r, rect.center().y + 1.5));
    let before = *value;
    if enabled {
        if let Some(p) = resp.interact_pointer_pos()
            && (resp.dragged() || resp.clicked() || resp.drag_started())
        {
            *value = ((p.x - track.left()) / track.width()).clamp(0.0, 1.0);
        }
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta);
            let d = if scroll.x.abs() > scroll.y.abs() { scroll.x } else { scroll.y };
            if d != 0.0 {
                *value = (*value + d / 400.0).clamp(0.0, 1.0);
            }
        }
        if resp.has_focus() {
            let step = ui.input(|i| {
                let mut s = 0.0;
                if i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::ArrowUp) {
                    s += 0.05;
                }
                if i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowDown) {
                    s -= 0.05;
                }
                s
            });
            *value = (*value + step).clamp(0.0, 1.0);
        }
    }
    if *value != before {
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let fill = if enabled { t.accent } else { t.muted.gamma_multiply(0.5) };
        ui.painter().rect_filled(track, 1.5, t.hairline);
        let x = track.left() + track.width() * value.clamp(0.0, 1.0);
        ui.painter().rect_filled(Rect::from_min_max(track.min, egui::pos2(x, track.bottom())), 1.5, fill);
        if enabled {
            let hot = resp.hovered() || resp.dragged() || resp.has_focus();
            let r = if hot { knob_r + 1.0 } else { knob_r };
            let c = egui::pos2(x, track.center().y);
            let knob = if ui.visuals().dark_mode { t.text } else { Color32::WHITE };
            ui.painter().circle_filled(c + vec2(0.0, 0.5), r + 0.5, Color32::from_black_alpha(40));
            ui.painter().circle(c, r, knob, egui::Stroke::new(1.0, t.hairline));
        }
    }
    let v = f64::from(*value);
    let label = label.to_owned();
    resp.widget_info(|| egui::WidgetInfo::slider(enabled, v, &label));
    resp
}

/// An on/off switch with its label on the left, the row filling the width.
pub fn toggle(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let t = Tokens::of(ui);
    let (rect, mut resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        ui.painter().text(egui::pos2(rect.left(), rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(13.0), t.text);
        let sw = Rect::from_min_size(egui::pos2(rect.right() - 30.0, rect.center().y - 9.0), vec2(30.0, 18.0));
        let how_on = ui.ctx().animate_bool_responsive(resp.id, *on);
        let track = if *on { t.accent } else { t.selected };
        ui.painter().rect_filled(sw, 9.0, track);
        let x = egui::lerp(sw.left() + 9.0..=sw.right() - 9.0, how_on);
        ui.painter().circle_filled(egui::pos2(x, sw.center().y), 7.0, Color32::WHITE);
    }
    let (on, label) = (*on, label.to_owned());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Switch, true, on, &label));
    resp
}

/// A quiet button that opens a menu: its text and a small chevron, hugging
/// its content.
pub fn menu_label(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let t = Tokens::of(ui);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(12.0), color);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 26.0, 24.0), Sense::click());
    if ui.is_rect_visible(rect) {
        if resp.hovered() {
            ui.painter().rect_filled(rect, 6.0, t.hover);
        }
        ui.painter().galley(egui::pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0), galley, color);
        let chev = Rect::from_center_size(egui::pos2(rect.right() - 10.0, rect.center().y + 1.0), vec2(8.0, 8.0));
        crate::icons::paint(ui.painter(), chev, crate::icons::Icon::ChevronDown, t.muted);
    }
    let label = text.to_owned();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    resp
}

/// The one empty-state layout: a muted glyph, a short title, a line or two
/// of how to fill it. Centred, with the same spacing everywhere.
pub fn empty_state(ui: &mut Ui, icon: crate::icons::Icon, title: &str, body: &str) {
    let t = Tokens::of(ui);
    ui.add_space(48.0);
    ui.vertical_centered(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
        crate::icons::paint(ui.painter(), r, icon, t.muted);
        ui.add_space(12.0);
        ui.label(egui::RichText::new(title).font(FontId::new(15.0, semibold())).color(t.text));
        ui.add_space(6.0);
        ui.scope(|ui| {
            ui.set_max_width(440.0);
            ui.add(egui::Label::new(egui::RichText::new(body).size(13.0).color(t.muted)).wrap());
        });
    });
}
