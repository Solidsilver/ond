//! Colors and spacing as named tokens (from the "Quiet" design), for light and
//! dark. Follows the system appearance unless the user picks one.
//!
//! Styling here is deliberately light-touch: egui's widgets with quieter
//! fills, hairline separators and the design's palette.

use egui::{Color32, Context, FontFamily, Theme};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemePref {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy)]
pub struct Tokens {
    pub bg: Color32,
    pub sidebar: Color32,
    /// The queue column and other secondary surfaces.
    pub panel: Color32,
    /// Fields and the now-playing display.
    pub field: Color32,
    pub hairline: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub hover: Color32,
    pub selected: Color32,
    pub placeholder: Color32,
    pub warn: Color32,
    pub ok: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

impl Tokens {
    pub const LIGHT: Tokens = Tokens {
        bg: hex(0xFFFFFF),
        sidebar: hex(0xF4F4F5),
        panel: hex(0xFBFBFC),
        field: hex(0xEDEDF0),
        hairline: hex(0xE6E6E8),
        text: hex(0x1C1C1E),
        // 5.3:1 on white, 4.9:1 on the sidebar.
        muted: hex(0x6B6B70),
        accent: hex(0x2F5D86),
        on_accent: hex(0xFFFFFF),
        hover: hex(0xEFEFF2),
        selected: hex(0xE2E2E6),
        placeholder: hex(0xE4E4E7),
        warn: hex(0xB25E09),
        ok: hex(0x2E7D5B),
    };

    pub const DARK: Tokens = Tokens {
        bg: hex(0x1B1B1D),
        sidebar: hex(0x232326),
        panel: hex(0x202023),
        field: hex(0x2C2C30),
        hairline: hex(0x323236),
        text: hex(0xE9E9EC),
        // 6.4:1 on bg, 5.8:1 on the sidebar.
        muted: hex(0x9D9DA4),
        accent: hex(0x8DB4E2),
        on_accent: hex(0x10213A),
        hover: hex(0x2A2A2E),
        selected: hex(0x36363B),
        placeholder: hex(0x2E2E32),
        warn: hex(0xE8A54B),
        ok: hex(0x6FCF97),
    };

    pub fn of(ui: &egui::Ui) -> Tokens {
        if ui.visuals().dark_mode { Self::DARK } else { Self::LIGHT }
    }
}

/// Semibold text in the UI font (the system font's weight axis when loaded).
pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

pub fn install(ctx: &Context, pref: ThemePref) {
    install_fonts(ctx);
    for (theme, t) in [(Theme::Light, Tokens::LIGHT), (Theme::Dark, Tokens::DARK)] {
        ctx.set_visuals_of(theme, visuals(theme, &t));
        ctx.style_mut_of(theme, |s| {
            s.spacing.item_spacing = egui::vec2(8.0, 6.0);
            s.spacing.button_padding = egui::vec2(8.0, 4.0);
            s.spacing.interact_size.y = 24.0;
            s.spacing.menu_margin = egui::Margin::same(6);
            use egui::{FontId, TextStyle};
            s.text_styles.insert(TextStyle::Body, FontId::proportional(13.0));
            s.text_styles.insert(TextStyle::Button, FontId::proportional(13.0));
            s.text_styles.insert(TextStyle::Small, FontId::proportional(11.0));
            s.text_styles.insert(TextStyle::Heading, FontId::new(22.0, semibold()));
            s.text_styles.insert(TextStyle::Monospace, FontId::monospace(12.0));
            // Overlay scroll bars, invisible until scrolling (see `scroll_bars`).
            s.spacing.scroll = egui::style::ScrollStyle {
                active_handle_opacity: 0.0,
                active_background_opacity: 0.0,
                interact_background_opacity: 0.0,
                ..egui::style::ScrollStyle::floating()
            };
        });
    }
    set_pref(ctx, pref);
}

/// macOS-style scroll bars: shown while a scroll is in progress, hidden
/// otherwise. Hovering the bar itself still shows it, so it can be dragged.
pub fn scroll_bars(ctx: &Context, visible: bool) {
    for theme in [Theme::Light, Theme::Dark] {
        ctx.style_mut_of(theme, |s| s.spacing.scroll.active_handle_opacity = if visible { 0.6 } else { 0.0 });
    }
}

pub fn set_pref(ctx: &Context, pref: ThemePref) {
    ctx.set_theme(match pref {
        ThemePref::System => egui::ThemePreference::System,
        ThemePref::Light => egui::ThemePreference::Light,
        ThemePref::Dark => egui::ThemePreference::Dark,
    });
}

fn visuals(theme: Theme, t: &Tokens) -> egui::Visuals {
    let mut v = match theme {
        Theme::Light => egui::Visuals::light(),
        Theme::Dark => egui::Visuals::dark(),
    };
    v.panel_fill = t.bg;
    v.window_fill = t.bg;
    v.extreme_bg_color = t.field;
    v.faint_bg_color = t.panel;
    v.hyperlink_color = t.accent;
    v.warn_fg_color = t.warn;
    v.selection.bg_fill = t.accent.gamma_multiply(if theme == Theme::Dark { 0.45 } else { 0.22 });
    v.selection.stroke = egui::Stroke::new(1.0, t.accent);
    v.window_stroke = egui::Stroke::new(1.0, t.hairline);
    v.window_corner_radius = egui::CornerRadius::same(10);
    v.menu_corner_radius = egui::CornerRadius::same(8);
    let radius = egui::CornerRadius::same(6);
    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = egui::Stroke::new(1.0, t.hairline);
    w.noninteractive.fg_stroke.color = t.text;
    w.noninteractive.corner_radius = radius;
    for (state, fill) in [(&mut w.inactive, Color32::TRANSPARENT), (&mut w.hovered, t.hover), (&mut w.active, t.selected), (&mut w.open, t.hover)] {
        state.weak_bg_fill = fill;
        state.bg_stroke = egui::Stroke::NONE;
        state.corner_radius = radius;
        state.fg_stroke.color = t.text;
        state.expansion = 0.0;
    }
    // Sliders, checkboxes and scrollbars draw with `bg_fill`.
    w.inactive.bg_fill = t.field;
    w.hovered.bg_fill = t.selected;
    w.active.bg_fill = t.muted;
    v
}

/// Uses the macOS system font (SF) when it's there: a variable font, so one
/// file gives both the regular and semibold faces via the weight axis. Falls
/// back to egui's bundled fonts everywhere else.
fn install_fonts(ctx: &Context) {
    let mut fonts = egui::FontDefinitions::default();
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut semibold_family = fallback.clone();
    if let Ok(bytes) = std::fs::read("/System/Library/Fonts/SFNS.ttf") {
        let regular = egui::FontData::from_owned(bytes.clone())
            .tweak(egui::FontTweak { coords: egui::epaint::text::VariationCoords::new([(b"wght", 400.0)]), ..Default::default() });
        let semi = egui::FontData::from_owned(bytes)
            .tweak(egui::FontTweak { coords: egui::epaint::text::VariationCoords::new([(b"wght", 590.0)]), ..Default::default() });
        fonts.font_data.insert("sf".into(), regular.into());
        fonts.font_data.insert("sf-semibold".into(), semi.into());
        if let Some(p) = fonts.families.get_mut(&FontFamily::Proportional) {
            p.insert(0, "sf".into());
        }
        semibold_family.insert(0, "sf-semibold".into());
    }
    fonts.families.insert(semibold(), semibold_family);
    ctx.set_fonts(fonts);
}
