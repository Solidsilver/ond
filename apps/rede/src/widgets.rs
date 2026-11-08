//! The few pieces Rede draws with that the family's `shell` doesn't have yet:
//! elided text, quiet links, and the segmented pill that picks a sort.
//!
//! Buttons are real egui widgets, so every control is named for the
//! accessibility tree without a view having to say so.

use egui::text::{LayoutJob, TextWrapping};
use egui::{Color32, FontId, Galley, Response, Ui, vec2};
use shell::theme::Tokens;
use std::sync::Arc;

/// One line of text cut to `width` with an ellipsis: the one way a view
/// shortens a name to fit.
pub fn elide(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = TextWrapping { max_width: width.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    ui.painter().layout_job(job)
}

/// Text that reads as plain until hovered, then underlines: a link where it
/// earns one, without turning the page blue.
pub fn link(ui: &mut Ui, text: &str, font: FontId, color: Color32) -> Response {
    let resp = ui.add(egui::Label::new(egui::RichText::new(text).font(font).color(color)).selectable(false).sense(egui::Sense::click()));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        let y = resp.rect.bottom() - 1.0;
        ui.painter().line_segment([egui::pos2(resp.rect.left(), y), egui::pos2(resp.rect.right(), y)], egui::Stroke::new(1.0, color));
    }
    resp
}

/// Mutually exclusive choices in a pill (the library's order). Writes the pick
/// into `current` and says whether it changed. Each choice is a real button, so
/// it's named where the accessibility tree looks.
pub fn choice<T: Copy + PartialEq>(ui: &mut Ui, all: &[T], current: &mut T, label: impl Fn(T) -> String) -> bool {
    let t = Tokens::of(ui);
    let mut changed = false;
    egui::Frame::new().fill(t.field).corner_radius(7.0).inner_margin(egui::Margin::same(2)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
            ui.spacing_mut().button_padding = vec2(9.0, 4.0);
            for &value in all {
                let on = value == *current;
                let text = egui::RichText::new(label(value)).size(12.0).color(if on { t.text } else { t.muted });
                let resp = ui.add(egui::Button::new(text).fill(if on { t.bg } else { Color32::TRANSPARENT }).stroke(egui::Stroke::NONE).corner_radius(5));
                if resp.clicked() && !on {
                    *current = value;
                    changed = true;
                }
            }
        });
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    fn install(ui: &mut egui::Ui) {
        shell::theme::install(ui.ctx(), shell::theme::ThemePref::Light);
        ui.ctx().request_repaint();
    }

    #[test]
    fn the_pill_reports_a_new_choice_and_keeps_the_old_one_on_a_second_click() {
        let mut h = Harness::builder().build_ui_state(
            |ui, state: &mut (usize, bool, Vec<usize>)| {
                if !state.1 {
                    state.1 = true;
                    install(ui);
                    return;
                }
                if choice(ui, &[0, 1, 2], &mut state.0, |v| format!("Choice {v}")) {
                    state.2.push(state.0);
                }
            },
            (0, false, Vec::new()),
        );
        h.run();
        h.get_by_label("Choice 2").click();
        h.run();
        assert_eq!(h.state().0, 2);
        assert_eq!(h.state().2, [2], "the pill says it changed");
        // Choosing the same one again isn't a change.
        h.get_by_label("Choice 2").click();
        h.run();
        assert_eq!(h.state().2, [2]);
    }

    #[test]
    fn elided_text_stops_at_one_line() {
        let mut h = Harness::builder().build_ui_state(
            |ui, done: &mut bool| {
                if !*done {
                    *done = true;
                    install(ui);
                    return;
                }
                let galley =
                    elide(ui, "a book title that is far too long to fit in this narrow column", FontId::proportional(13.0), egui::Color32::WHITE, 80.0);
                assert!(galley.rows.len() == 1, "one row");
                assert!(galley.size().x <= 82.0, "cut to the width, got {}", galley.size().x);
            },
            false,
        );
        h.run();
    }
}
