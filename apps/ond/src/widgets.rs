//! Small shared pieces: elided text, quiet links, the accent button, a
//! segmented control, a thin slider, a switch and a menu label, and the rows
//! Settings is built from (a [`group`] of [`setting`]s).

use crate::icons::{self, Icon};
use crate::theme::Tokens;
use egui::{Color32, FontId, Rect, Response, Sense, Ui, vec2};
pub use shell::kit::{
    Head, ROW_PAD, choice, divider, elide, elide_job, group, link, link_line, managed, section, switch, switch_tinted, thin_slider, thin_slider_marked,
};
pub use shell::widgets::{danger, empty_state, focus_ring, menu_row, primary, secondary, secondary_danger, sidebar_row};

/// A heart that loves / unloves: filled accent when on, a quiet outline when off.
pub fn heart_button(ui: &mut Ui, on: bool, size: f32, glyph: f32) -> Response {
    let t = Tokens::of(ui);
    let (icon, color, label) = if on { (crate::icons::Icon::HeartFilled, t.accent, "Unlove") } else { (crate::icons::Icon::Heart, t.muted, "Love") };
    crate::icons::sized_button(ui, icon, label, vec2(size, size), glyph, Some(color))
}

/// The heart over a row or display: drawn filled when loved, as an outline when
/// `reveal` (its row is hovered) or when it's hovered itself, and always
/// clickable. `label` names it for accessibility ("Love", "Unlove (⇧⌘L)").
pub fn hover_heart(ui: &mut Ui, rect: Rect, id: egui::Id, loved: bool, reveal: bool, glyph: f32, label: &str) -> Response {
    let t = Tokens::of(ui);
    let resp = ui.interact(rect, id, Sense::click());
    // Focused, it shows (as when hovered), or Tab would land on nothing.
    let hot = resp.hovered() || resp.has_focus();
    if loved || reveal || hot {
        let (icon, color) = if loved { (crate::icons::Icon::HeartFilled, t.accent) } else { (crate::icons::Icon::Heart, if hot { t.text } else { t.muted }) };
        crate::icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(glyph, glyph)), icon, color);
    }
    let side = glyph + 6.0;
    focus_ring(ui, &resp, Rect::from_center_size(rect.center(), vec2(side, side)), side / 2.0);
    let label = crate::keys::shown(label).into_owned();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    resp
}

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

/// Grid or list: an icon for each in a segmented pill. Writes the pick into
/// `current` and says whether it changed.
pub fn view_picker(ui: &mut Ui, current: &mut crate::settings::ViewMode) -> bool {
    use crate::settings::ViewMode;
    let t = Tokens::of(ui);
    let seg = vec2(30.0, 22.0);
    let (rect, pill) = ui.allocate_exact_size(vec2(seg.x * 2.0 + 4.0, 26.0), Sense::hover());
    ui.painter().rect_filled(rect, 7.0, t.field);
    let mut changed = false;
    for (i, (mode, icon, label)) in [(ViewMode::Grid, Icon::Grid, "Grid"), (ViewMode::List, Icon::List, "List")].into_iter().enumerate() {
        let r = Rect::from_min_size(egui::pos2(rect.left() + 2.0 + seg.x * i as f32, rect.top() + 2.0), seg);
        let resp = ui.interact(r, pill.id.with(("view", i)), Sense::click());
        let on = *current == mode;
        if on {
            ui.painter().rect_filled(r, 5.0, t.bg);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 5.0, t.hover);
        }
        icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(13.0, 13.0)), icon, if on { t.text } else { t.muted });
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, on, label));
        if resp.on_hover_text(label).clicked() && !on {
            *current = mode;
            changed = true;
        }
    }
    changed
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

/// A list row's background: the selection, else the hover tint, else none.
pub fn row_fill(t: &Tokens, selected: bool, hovered: bool) -> Color32 {
    if selected {
        t.selected
    } else if hovered {
        t.hover
    } else {
        Color32::TRANSPARENT
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

/// The label column of [`kv_row`]s, so values line up down a section.
const KV_LABEL_W: f32 = 96.0;
/// Value text size in [`kv_row`]s.
pub const KV_VALUE: f32 = 12.5;

/// A [`section`] label with room above it, heading a run of [`kv_row`]s.
pub fn kv_section(ui: &mut Ui, title: &str) {
    ui.add_space(18.0);
    section(ui, title);
    ui.add_space(6.0);
}

/// A muted label in a fixed column, then the value laid out by `value`
/// (Info tab, album details, the Player's details).
pub fn kv_row(ui: &mut Ui, label: &str, value: impl FnOnce(&mut Ui)) {
    let t = Tokens::of(ui);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (r, _) = ui.allocate_exact_size(vec2(KV_LABEL_W, KV_VALUE + 4.0), Sense::hover());
        let g = elide(ui, label, FontId::proportional(12.0), t.muted, KV_LABEL_W - 4.0);
        ui.painter().galley(r.min + vec2(0.0, 1.0), g, t.muted);
        let rest = ui.available_width();
        ui.allocate_ui_with_layout(vec2(rest, 0.0), egui::Layout::top_down(egui::Align::Min), value);
    });
    ui.add_space(6.0);
}

/// A [`kv_row`] whose value is plain, wrapping text.
pub fn kv_text(ui: &mut Ui, label: &str, value: &str) {
    let t = Tokens::of(ui);
    kv_row(ui, label, |ui| {
        ui.add(egui::Label::new(egui::RichText::new(value).size(KV_VALUE).color(t.text)).wrap());
    });
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

/// The round button on a tile's art (play on an album, shuffle on an
/// artist), 32 points across at `at`. It's there while the tile or the
/// button itself is hovered (on the button, the tile doesn't read as
/// hovered), filled with the accent under the pointer. True when clicked.
pub fn tile_button(ui: &Ui, t: &Tokens, at: Rect, id: egui::Id, tile_hovered: bool, icon: Icon, label: &str) -> bool {
    let resp = ui.interact(at, id, Sense::click());
    if !(tile_hovered || resp.hovered()) {
        return false;
    }
    let fill = if resp.hovered() { t.accent } else { t.bg.gamma_multiply(0.92) };
    let fg = if resp.hovered() { t.on_accent } else { t.text };
    ui.painter().circle_filled(at.center(), 16.0, fill);
    // The play triangle sits a point right of centre so it looks centred.
    let glyph = if matches!(icon, Icon::Play) {
        Rect::from_center_size(at.center() + vec2(1.0, 0.0), vec2(12.0, 12.0))
    } else {
        Rect::from_center_size(at.center(), vec2(13.0, 13.0))
    };
    icons::paint(ui.painter(), glyph, icon, fg);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    resp.clicked()
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
