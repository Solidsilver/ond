//! Text: one line cut to fit ([`elide`] and its kin), the quiet [`link`]s
//! that underline on hover, the muted [`section`] label, a key combo drawn
//! as a [`key_cap`], a [`chip`] of tinted text, and the one [`empty_state`].

use crate::icons::Icon;
use crate::theme::{Tokens, semibold};
use egui::text::{LayoutJob, TextWrapping};
use egui::{Color32, FontId, Galley, Response, Sense, Ui, vec2};
use std::sync::Arc;

/// One line of text cut to `width` with an ellipsis. The one way the apps
/// shorten a name to fit: lists, headers, the display, the visualizer's
/// caption (through [`elide_job`]) and [`link_line`].
pub fn elide(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    ui.painter().layout_job(elide_job(text, font, color, width))
}

/// `text` cut in the middle to fit `width`, keeping its start and (more of) its
/// end. For paths, whose last components are the part that tells them apart.
pub fn elide_middle(ui: &Ui, text: &str, font: &FontId, width: f32) -> String {
    let fits = |s: &str| ui.painter().layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x <= width;
    if fits(text) {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let cut = |keep: usize| {
        let head = keep / 3;
        let tail = keep - head;
        format!("{}…{}", chars[..head].iter().collect::<String>(), chars[chars.len() - tail..].iter().collect::<String>())
    };
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(&cut(mid)) { lo = mid } else { hi = mid - 1 }
    }
    cut(lo)
}

/// The layout [`elide`] lays out, for a painter without a `Ui`.
pub fn elide_job(text: &str, font: FontId, color: Color32, width: f32) -> LayoutJob {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = elide_wrap(width);
    job
}

/// One row of at most `width`, cut anywhere, ending in an ellipsis.
fn elide_wrap(width: f32) -> TextWrapping {
    TextWrapping { max_width: width.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') }
}

/// Text that reads as plain until hovered, then underlines: links where they
/// earn it, without turning the page blue.
pub fn link(ui: &mut Ui, text: &str, font: FontId, color: Color32) -> Response {
    // Never wider than the room it's in: a long name wraps (in a wrapping row,
    // across the whole row) instead of widening its column.
    let cap = if ui.layout().main_wrap { ui.max_rect().width() } else { ui.available_width() };
    let wrap = TextWrapping { max_width: cap.max(24.0), ..Default::default() };
    link_with(ui, text, font, color, wrap).0
}

/// A link kept to one line of at most `width`, cut with an ellipsis; the whole
/// text shows on hover when it was cut.
pub fn link_line(ui: &mut Ui, text: &str, font: FontId, color: Color32, width: f32) -> Response {
    let (resp, cut) = link_with(ui, text, font, color, elide_wrap(width));
    if cut { resp.on_hover_text(text) } else { resp }
}

/// The link, and whether its text was cut to fit (the layout says so, so the
/// whole text needn't be laid out again to find out).
fn link_with(ui: &mut Ui, text: &str, font: FontId, color: Color32, wrap: TextWrapping) -> (Response, bool) {
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
        job.wrap = wrap.clone();
        job
    };
    let galley = ui.painter().layout_job(job(false));
    let cut = galley.elided;
    let (rect, resp) = ui.allocate_exact_size(galley.size(), Sense::click());
    if ui.is_rect_visible(rect) {
        let galley = if resp.hovered() { ui.painter().layout_job(job(true)) } else { galley };
        ui.painter().galley(rect.min, galley, color);
    }
    let label = text.to_owned();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, &label));
    (resp.on_hover_cursor(egui::CursorIcon::PointingHand), cut)
}

/// Muted small-caps-ish section label.
pub fn section(ui: &mut Ui, text: &str) {
    let t = Tokens::of(ui);
    ui.label(egui::RichText::new(text).font(FontId::new(11.0, semibold())).color(t.muted));
}

/// A key combo drawn as a small rounded key: `keys` as the platform writes
/// them, on the field's fill with a hairline edge. Named as its keys, so a
/// screen reader reads the combo beside its description.
pub fn key_cap(ui: &mut egui::Ui, keys: &str) {
    let t = Tokens::of(ui);
    let g = ui.painter().layout_no_wrap(keys.to_owned(), FontId::proportional(12.0), t.text);
    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 12.0, 20.0), egui::Sense::hover());
    ui.painter().rect(r, 5.0, t.field, egui::Stroke::new(1.0, t.hairline), egui::StrokeKind::Inside);
    ui.painter().galley(r.center() - g.size() / 2.0, g, t.text);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, keys));
}

/// The one empty-state layout: a muted glyph, a short title, a line or two
/// of how to fill it (none when `body` is empty, as while loading).
/// Centred, with the same spacing everywhere.
pub fn empty_state(ui: &mut Ui, icon: Icon, title: &str, body: &str) {
    let t = Tokens::of(ui);
    ui.add_space(48.0);
    ui.vertical_centered(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
        crate::icons::paint(ui.painter(), r, icon, t.muted);
        ui.add_space(12.0);
        ui.label(egui::RichText::new(title).font(FontId::new(15.0, semibold())).color(t.text));
        if body.is_empty() {
            return;
        }
        ui.add_space(6.0);
        ui.scope(|ui| {
            // Never wider than the column it sits in: a centred layout claims
            // the whole frame, and a panel's scroll area would grow to fit it.
            ui.set_max_width(440.0_f32.min(ui.available_width()));
            ui.add(egui::Label::new(egui::RichText::new(crate::keys::shown(body)).size(13.0).color(t.muted)).wrap());
        });
    });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::themed;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn a_one_line_link_knows_when_its_text_was_cut() {
        let mut h = themed(Vec::new(), |ui, cuts: &mut Vec<bool>| {
            let font = FontId::proportional(13.0);
            cuts.push(link_with(ui, "Short", font.clone(), Color32::WHITE, elide_wrap(400.0)).1);
            cuts.push(link_with(ui, "A title far too long for forty points", font, Color32::WHITE, elide_wrap(40.0)).1);
        });
        h.run();
        assert_eq!(h.state().1.last_chunk::<2>(), Some(&[false, true]));
    }

    #[test]
    fn an_empty_state_shows_its_title_and_its_body_when_it_has_one() {
        // The theme's fonts arrive a frame after they're installed.
        let mut h = egui_kittest::Harness::builder().build_ui_state(
            |ui, installed: &mut bool| {
                if !std::mem::replace(installed, true) {
                    crate::theme::install(ui.ctx(), crate::theme::ThemePref::Light);
                    ui.ctx().request_repaint();
                    return;
                }
                empty_state(ui, Icon::Download, "Nothing downloaded", "");
                empty_state(ui, Icon::Search, "No results", "Try fewer words.");
            },
            false,
        );
        h.run();
        h.get_by_label("Nothing downloaded");
        h.get_by_label("No results");
        h.get_by_label("Try fewer words.");
    }
}
