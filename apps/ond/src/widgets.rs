//! Small shared pieces: elided text, quiet links, the accent button, a
//! segmented control, a thin slider, a switch and a menu label, and the rows
//! Settings is built from (a [`group`] of [`setting`]s).

use crate::icons::Icon;
use crate::theme::Tokens;
use egui::{Color32, Rect, Response, Sense, Ui, vec2};
pub use shell::kit::{
    Head, KV_VALUE, ROW_PAD, chip, choice, divider, elide, elide_job, group, heart_button, hover_heart, icon_choice, kv_row, kv_section, kv_text, link,
    link_line, managed, menu_label, row_fill, section, switch, switch_tinted, thin_slider, thin_slider_marked, tile_button,
};
pub use shell::widgets::{danger, empty_state, focus_ring, menu_row, primary, secondary, secondary_danger, sidebar_row};

/// One choice in a rating menu: `rating` stars drawn as icons, filled up to
/// it, with a check when it's the current rating.
pub fn rating_choice(ui: &mut Ui, rating: u8, current: bool) -> Response {
    let t = Tokens::of(ui);
    let star = 12.0;
    // A fixed width: a menu sizes itself to its widest row, so taking the room
    // offered would stretch the menu across the window.
    let width = 26.0 + 5.0 * (star + 3.0) + 8.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 22.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, t.selected);
    }
    if current {
        crate::icons::paint(
            ui.painter(),
            Rect::from_center_size(egui::pos2(rect.left() + 10.0, rect.center().y), vec2(10.0, 10.0)),
            crate::icons::Icon::Check,
            t.text,
        );
    }
    for k in 0..5u8 {
        let c = egui::pos2(rect.left() + 26.0 + f32::from(k) * (star + 3.0), rect.center().y);
        let (icon, color) = if k < rating { (crate::icons::Icon::StarFilled, t.text) } else { (crate::icons::Icon::Star, t.muted) };
        crate::icons::paint(ui.painter(), Rect::from_center_size(c, vec2(star, star)), icon, color);
    }
    let label = crate::fmt::stars(rating);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::MenuItem, true, current, &label));
    resp
}

/// Grid or list: an icon for each in the kit's [`icon_choice`]. Writes the
/// pick into `current` and says whether it changed.
pub fn view_picker(ui: &mut Ui, current: &mut crate::settings::ViewMode) -> bool {
    use crate::settings::ViewMode;
    let all = [ViewMode::Grid, ViewMode::List];
    let at = all.iter().position(|m| m == current).unwrap_or(0);
    match icon_choice(ui, &[(Icon::Grid, "Grid"), (Icon::List, "List")], at) {
        Some(i) => {
            *current = all[i];
            true
        }
        None => false,
    }
}

/// A track title's colour in a list: the accent while it plays, dimmed when it
/// can't play right now (offline and not downloaded).
pub fn track_color(t: &Tokens, playing: bool, playable: bool) -> Color32 {
    if playing {
        t.accent
    } else if playable {
        t.text
    } else {
        t.muted.gamma_multiply(0.75)
    }
}

/// One setting: its name and a short hint on the left, its control on the
/// right (the kit's [`shell::kit::setting`]), tagged as set in managed.json
/// when a [`managed`] row is locked.
pub fn setting<R>(ui: &mut Ui, label: &str, hint: &str, control: impl FnOnce(&mut Ui) -> R) -> R {
    row_with(ui, Head { label, hint, ..Default::default() }, control)
}

/// [`setting`] with the whole [`Head`].
pub fn row_with<R>(ui: &mut Ui, mut head: Head<'_>, control: impl FnOnce(&mut Ui) -> R) -> R {
    // Rows are only ever disabled whole by `managed`.
    if !ui.is_enabled() && head.tag.is_none() {
        head.tag = Some(crate::managed::LOCKED);
    }
    shell::kit::row_with(ui, head, control)
}

/// Bit-perfect's switch for the device `name`, green when on: the one the
/// output panel and Settings › Output both show. `mode` comes back as chosen;
/// each caller acts on the change its own way.
pub fn bit_perfect_switch(ui: &mut Ui, name: &str, mode: &mut audio::PlaybackMode) -> Response {
    let mut on = *mode == audio::PlaybackMode::BitPerfect;
    let resp = switch_tinted(ui, &mut on, &format!("Bit-perfect on {name}"), Tokens::of(ui).ok);
    *mode = if on { audio::PlaybackMode::BitPerfect } else { audio::PlaybackMode::Normal };
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use shell::kit::segmented_wide;

    #[test]
    fn tab_shows_which_painted_control_has_focus() {
        // One of each control that paints itself, and whether its ring is drawn.
        let mut h = egui_kittest::Harness::builder().build_ui_state(
            |ui, on: &mut bool| {
                segmented_wide(ui, &["Off", "On"], 0, 120.0);
                switch(ui, on, "Shuffle");
                hover_heart(ui, Rect::from_min_size(ui.cursor().min, vec2(24.0, 24.0)), ui.scope_id().with("heart"), false, false, 14.0, "Love");
            },
            false,
        );
        let rings = |h: &egui_kittest::Harness<'_, bool>| {
            let accent = Tokens::of_ctx(&h.ctx).accent;
            h.output().shapes.iter().filter(|c| matches!(&c.shape, egui::Shape::Rect(r) if r.stroke.color == accent && r.stroke.width == 1.5)).count()
        };
        h.run();
        assert_eq!(rings(&h), 0, "nothing focused, no ring");
        for _ in 0..4 {
            h.key_press(egui::Key::Tab);
            h.run();
            assert_eq!(rings(&h), 1, "one control focused, one ring");
        }
    }

    #[test]
    fn the_bit_perfect_switch_hands_back_the_mode_chosen() {
        use egui_kittest::kittest::Queryable;
        let mut h = egui_kittest::Harness::builder()
            .build_ui_state(|ui, mode: &mut audio::PlaybackMode| _ = bit_perfect_switch(ui, "DAC", mode), audio::PlaybackMode::Normal);
        h.run();
        h.get_by_label("Bit-perfect on DAC").click();
        h.run();
        assert_eq!(*h.state(), audio::PlaybackMode::BitPerfect);
        h.get_by_label("Bit-perfect on DAC").click();
        h.run();
        assert_eq!(*h.state(), audio::PlaybackMode::Normal);
    }
}
