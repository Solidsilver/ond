//! The kit's text and choice pieces under the names Rede's views use:
//! elided text, the quiet link, and the segmented pill (the library's order,
//! the chapters column's tabs). They're Ond's, from `shell::kit`, so the two
//! apps draw them alike.

use egui::Ui;
pub use shell::kit::{elide, link};

/// Mutually exclusive choices in Ond's segmented pill, each segment fitting
/// its label. Writes the pick into `current` and says whether it changed.
pub fn choice<T: Copy + PartialEq>(ui: &mut Ui, all: &[T], current: &mut T, label: impl Fn(T) -> String) -> bool {
    shell::kit::choice(ui, all, current, None, label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::FontId;
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
