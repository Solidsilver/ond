//! Settings, as both apps show it: a window over the dimmed app
//! ([`settings_window`]) with the tab list down its grey side
//! ([`settings_nav`]), the pane beside it that scrolls under the tab's title
//! ([`settings_pane`]), and an "esc" hint at the top right; Esc or a click
//! outside closes it. About's header ([`about_header`], with the
//! [`app_icon`] it shows) is here too. The tabs and what's on them are the
//! app's.

use crate::theme::{Tokens, semibold};
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};

/// The tab list's width.
pub const SETTINGS_NAV_W: f32 = 176.0;
/// The pane's side margins.
const PANE_PAD: f32 = 28.0;

/// What [`settings_window`] saw this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsWindow {
    /// The tab clicked, which the pane already shows.
    pub clicked: Option<usize>,
    /// Esc, or a click outside the window: close it.
    pub close: bool,
}

/// The Settings window over the dimmed app: `tabs` down its side, the one at
/// `selected` lit, and the pane beside them, which `pane` fills for the tab
/// it's given (the one clicked this frame, or else `selected`). `top` is the
/// height of the bar along the top of the app (Ond's toolbar, Rede's title
/// bar): the window centres in the room below it and keeps clear of it, so
/// what the bar shows stays in view. `id` names the pane's scroll positions,
/// one per tab.
pub fn settings_window(
    ctx: &egui::Context,
    top: f32,
    tabs: &[&str],
    selected: usize,
    id: impl std::hash::Hash + std::fmt::Debug + Copy,
    pane: impl FnOnce(&mut Ui, usize),
) -> SettingsWindow {
    let screen = ctx.content_rect();
    let w = (screen.width() - 64.0).clamp(480.0, 760.0);
    let h = (screen.height() - 96.0).clamp(340.0, 540.0).min((screen.height() - top - 16.0).max(200.0));
    let frame = egui::Frame::popup(&ctx.global_style()).inner_margin(egui::Margin::ZERO).corner_radius(12);
    let mut clicked = None;
    let modal = egui::Modal::new(egui::Id::unique("settings-window"))
        .frame(frame)
        .area(egui::Modal::default_area(egui::Id::unique("settings-window")).anchor(egui::Align2::CENTER_CENTER, vec2(0.0, top / 2.0)))
        .show(ctx, |ui| {
            let t = Tokens::of(ui);
            let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
            let esc = ui.painter().layout_no_wrap("esc".into(), FontId::proportional(11.0), t.muted);
            ui.painter().galley(pos2(rect.right() - 14.0 - esc.size().x, rect.top() + 16.0), esc, t.muted);
            let side = Rect::from_min_size(rect.min, vec2(SETTINGS_NAV_W, h));
            ui.painter().rect_filled(side, egui::CornerRadius { nw: 12, sw: 12, ..Default::default() }, t.sidebar);
            ui.painter().vline(side.right(), side.y_range(), egui::Stroke::new(1.0, t.hairline));
            clicked = settings_nav(ui, side, tabs, selected);
            let shown = clicked.unwrap_or(selected).min(tabs.len().saturating_sub(1));
            let area = Rect::from_min_max(pos2(side.right() + 1.0, rect.top()), rect.max);
            settings_pane(ui, area, (id, shown), tabs.get(shown).copied().unwrap_or_default(), |ui| pane(ui, shown));
        });
    SettingsWindow { clicked, close: modal.should_close() }
}

/// "Settings" over a row per section, in `side`, the one at `selected` lit.
/// Returns the one clicked.
pub fn settings_nav(ui: &mut Ui, side: Rect, sections: &[&str], selected: usize) -> Option<usize> {
    let t = Tokens::of(ui);
    let mut nav = ui.new_child(egui::UiBuilder::new().max_rect(side.shrink2(vec2(10.0, 16.0))).layout(egui::Layout::top_down(egui::Align::Min)));
    nav.spacing_mut().item_spacing.y = 2.0;
    nav.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Settings").font(FontId::new(15.0, semibold())).color(t.text));
    });
    nav.add_space(12.0);
    let mut clicked = None;
    for (i, label) in sections.iter().enumerate() {
        if super::sidebar_row(&mut nav, label, i == selected, 0.0, label).clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

/// The pane in `pane`: `title` large, then `body`, scrolling when it's taller
/// than the pane. `id` names the scroll position, so each tab keeps its own.
pub fn settings_pane(ui: &mut Ui, pane: Rect, id: impl std::hash::Hash + std::fmt::Debug, title: &str, body: impl FnOnce(&mut Ui)) {
    let t = Tokens::of(ui);
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(pane).layout(egui::Layout::top_down(egui::Align::Min)));
    egui::ScrollArea::vertical().id_salt(id).auto_shrink(false).show(&mut ui, |ui| {
        ui.add_space(20.0);
        ui.horizontal(|ui| {
            ui.add_space(PANE_PAD);
            ui.vertical(|ui| {
                ui.set_width(pane.width() - PANE_PAD * 2.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(egui::RichText::new(title).font(FontId::new(20.0, semibold())).color(t.text));
                ui.add_space(16.0);
                body(ui);
                ui.add_space(24.0);
            });
        });
    });
}

/// The app's icon from its packaged PNG, decoded once and kept for the
/// context's life under `name`.
pub fn app_icon(ctx: &egui::Context, name: &str, png: &[u8]) -> egui::TextureHandle {
    let id = egui::Id::unique("about-app-icon").with(name);
    if let Some(tex) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return tex;
    }
    let image = match image::load_from_memory_with_format(png, image::ImageFormat::Png) {
        Ok(decoded) => {
            let rgba = decoded.to_rgba8();
            egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw())
        }
        Err(e) => {
            tracing::warn!("app icon: {e}");
            egui::ColorImage::example()
        }
    };
    let tex = ctx.load_texture("about-app-icon", image, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

/// About's header: the icon, and beside it the app's name over its version.
pub fn about_header(ui: &mut Ui, icon: &egui::TextureHandle, name: &str, version: &str) {
    let t = Tokens::of(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        ui.add(egui::Image::new((icon.id(), vec2(56.0, 56.0))));
        ui.vertical(|ui| {
            ui.add_space(8.0);
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.label(egui::RichText::new(name).font(FontId::new(17.0, semibold())).color(t.text));
            ui.label(egui::RichText::new(version).size(12.5).color(t.muted));
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::themed;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn the_window_shows_the_tab_clicked_at_once_and_closes_on_esc() {
        let tabs = ["Playback", "Account", "About"];
        let mut h = themed((0usize, false), move |ui, (at, closed): &mut (usize, bool)| {
            let w = settings_window(ui.ctx(), 52.0, &tabs, *at, "test-settings", |ui, shown| {
                ui.label(format!("body of {}", tabs[shown]));
            });
            if let Some(i) = w.clicked {
                *at = i;
            }
            *closed |= w.close;
        });
        h.set_size(vec2(900.0, 640.0));
        h.run_steps(4);
        h.get_by_label("body of Playback");
        h.get_by_label("Account").click();
        h.run();
        assert_eq!(h.state().1.0, 1);
        h.get_by_label("body of Account");
        assert!(!h.state().1.1);
        h.key_press(egui::Key::Escape);
        h.run();
        assert!(h.state().1.1, "Esc closes it");
    }

    #[test]
    fn the_tab_list_lights_the_tab_shown_and_says_which_was_clicked() {
        let mut h = themed(0usize, |ui, at: &mut usize| {
            let side = Rect::from_min_size(ui.cursor().min, vec2(SETTINGS_NAV_W, 300.0));
            if let Some(i) = settings_nav(ui, side, &["Playback", "Account", "About"], *at) {
                *at = i;
            }
            let pane = Rect::from_min_size(side.right_top(), vec2(300.0, 300.0));
            settings_pane(ui, pane, "pane", ["Playback", "Account", "About"][*at], |ui| {
                ui.label("body");
            });
        });
        h.run();
        assert_eq!(h.query_all_by_label("Playback").count(), 2, "the tab, and the pane's title");
        h.get_by_label("About").click();
        h.run();
        assert_eq!(h.state().1, 2);
        // The pane's title follows the tab.
        assert_eq!(h.query_all_by_label("About").count(), 2);
        h.get_by_label("body");
    }
}
