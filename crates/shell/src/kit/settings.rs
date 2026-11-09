//! A Settings page's parts: the tab list down its side ([`settings_nav`]),
//! the pane beside it that scrolls under the tab's title
//! ([`settings_pane`]), and About's header ([`about_header`], with the
//! [`app_icon`] it shows). Ond draws them in a window over the library, Rede
//! in the page's place; what frames them (a fill, a hairline, the "esc"
//! hint) is the app's.

use crate::theme::{Tokens, semibold};
use egui::{FontId, Rect, Ui, vec2};

/// The tab list's width.
pub const SETTINGS_NAV_W: f32 = 176.0;
/// The pane's side margins.
const PANE_PAD: f32 = 28.0;

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
