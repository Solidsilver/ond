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

impl ThemePref {
    pub const ALL: [ThemePref; 3] = [ThemePref::System, ThemePref::Light, ThemePref::Dark];

    pub fn label(self) -> &'static str {
        match self {
            ThemePref::System => "System",
            ThemePref::Light => "Light",
            ThemePref::Dark => "Dark",
        }
    }
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
    /// Errors (failed login): a softer red than egui's default.
    pub danger: Color32,
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
        danger: hex(0xC0392B),
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
        danger: hex(0xF0857A),
        ok: hex(0x6FCF97),
    };

    pub fn of(ui: &egui::Ui) -> Tokens {
        Self::themed(ui.ctx(), ui.visuals().dark_mode)
    }

    /// For code that has a context but no `Ui` (overlays).
    pub fn of_ctx(ctx: &Context) -> Tokens {
        Self::themed(ctx, ctx.global_style().visuals.dark_mode)
    }

    /// The theme's tokens, with the accent from the album art when that
    /// experiment is on (see [`crate::visuals::colors`]).
    fn themed(ctx: &Context, dark: bool) -> Tokens {
        let base = if dark { Self::DARK } else { Self::LIGHT };
        match tint(ctx, dark) {
            Some(t) => t.apply(base),
            None => base,
        }
    }
}

/// The album-art tint in use, if that experiment is on and has one.
pub fn tint(ctx: &Context, dark: bool) -> Option<crate::visuals::colors::Tint> {
    ctx.data(|d| d.get_temp::<crate::visuals::colors::Tints>(crate::visuals::colors::id())).map(|t| t.of(dark))
}

/// [`tint`] for the theme `ui` is drawn in.
pub fn tint_of(ui: &egui::Ui) -> Option<crate::visuals::colors::Tint> {
    tint(ui.ctx(), ui.visuals().dark_mode)
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
    v.error_fg_color = t.danger;
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

/// Uses the system UI font (SF, Segoe UI, or whatever fontconfig calls
/// sans-serif) and a CJK fallback when they're there, ahead of egui's bundled
/// fonts.
fn install_fonts(ctx: &Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut semibold_family = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let system = system_fonts();
    // The UI font first, then CJK, then egui's bundled fallbacks.
    for (name, faces) in [("cjk", system.cjk), ("ui", system.ui)] {
        let Some(Faces { regular, semibold }) = faces else { continue };
        fonts.font_data.insert(name.into(), regular.into());
        let semi_name = match semibold {
            Some(face) => {
                let n = format!("{name}-semibold");
                fonts.font_data.insert(n.clone(), face.into());
                n
            }
            None => name.to_owned(),
        };
        if let Some(p) = fonts.families.get_mut(&FontFamily::Proportional) {
            p.insert(0, name.into());
        }
        semibold_family.insert(0, semi_name);
    }
    fonts.families.insert(semibold(), semibold_family);
    ctx.set_fonts(fonts);
}

/// The UI font and a CJK font.
#[derive(Default)]
struct SystemFonts {
    ui: Option<Faces>,
    cjk: Option<Faces>,
}

/// A regular face and, when there's a separate one, a semibold.
struct Faces {
    regular: egui::FontData,
    /// `None`: semibold text uses the regular face too.
    semibold: Option<egui::FontData>,
}

/// One file per weight. Without a separate semibold file (or one that won't
/// read) semibold text uses the regular face.
fn pair(regular: &std::path::Path, semibold: &std::path::Path) -> Option<Faces> {
    let face = |p: &std::path::Path| std::fs::read(p).ok().map(egui::FontData::from_owned);
    let semibold = if semibold == regular { None } else { face(semibold) };
    Some(Faces { regular: face(regular)?, semibold })
}

/// A variable font: one file gives both weights through its weight axis.
#[cfg_attr(windows, allow(dead_code))]
fn variable(bytes: Vec<u8>, semibold_weight: f32) -> Faces {
    use egui::epaint::text::VariationCoords;
    let weight = |w: f32| egui::FontTweak { coords: VariationCoords::new([(b"wght", w)]), ..Default::default() };
    Faces {
        regular: egui::FontData::from_owned(bytes.clone()).tweak(weight(400.0)),
        semibold: Some(egui::FontData::from_owned(bytes).tweak(weight(semibold_weight))),
    }
}

#[cfg(target_os = "macos")]
fn system_fonts() -> SystemFonts {
    // SF is a variable font; its semibold sits at 590 on the weight axis.
    let ui = std::fs::read("/System/Library/Fonts/SFNS.ttf").ok().map(|bytes| variable(bytes, 590.0));
    // SF has no CJK. Hiragino Sans covers kanji, kana and most common hanzi in
    // ~8 MB a weight; the Chinese and Korean system fonts are 20–55 MB each,
    // too much to hold in memory for the odd title. Colour emoji is a bitmap
    // font egui can't draw, so it's left out.
    let dir = std::path::Path::new("/System/Library/Fonts");
    let cjk = pair(&dir.join("ヒラギノ角ゴシック W3.ttc"), &dir.join("ヒラギノ角ゴシック W6.ttc"));
    SystemFonts { ui, cjk }
}

#[cfg(target_os = "windows")]
fn system_fonts() -> SystemFonts {
    let dir = std::env::var_os("WINDIR").map_or_else(|| "C:\\Windows".into(), std::path::PathBuf::from).join("Fonts");
    // Yu Gothic plays Hiragino's part: Japanese plus the common hanzi.
    SystemFonts { ui: pair(&dir.join("segoeui.ttf"), &dir.join("seguisb.ttf")), cjk: pair(&dir.join("YuGothR.ttc"), &dir.join("YuGothB.ttc")) }
}

/// Asks fontconfig, which knows the desktop's chosen sans-serif.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn system_fonts() -> SystemFonts {
    let find = |pattern: &str| -> Option<std::path::PathBuf> {
        let out = std::process::Command::new("fc-match").args(["--format=%{file}", pattern]).output().ok()?;
        let path = String::from_utf8(out.stdout).ok()?;
        // Only TrueType/OpenType: egui can't read Type 1 or bitmap fonts.
        let ok = out.status.success() && [".ttf", ".otf", ".ttc"].iter().any(|e| path.to_ascii_lowercase().ends_with(e));
        ok.then(|| path.into())
    };
    let weights = |family: &str| {
        let regular = find(&format!("{family}:weight=regular"))?;
        let semi = find(&format!("{family}:weight=semibold")).unwrap_or_else(|| regular.clone());
        Some((regular, semi))
    };
    // GNOME's UI fonts (Adwaita Sans, Cantarell) are variable: one file for
    // every weight, set through the weight axis.
    let ui_paths = weights("sans-serif");
    let ui = ui_paths.as_ref().and_then(|(regular, semi)| if regular == semi { std::fs::read(regular).ok().map(|b| variable(b, 600.0)) } else { pair(regular, semi) });
    // A CJK font is big (Noto Sans CJK is ~20 MB): one face, used for both
    // weights, and none at all when fontconfig only has the UI font to offer.
    let cjk = weights("sans-serif:lang=ja")
        .filter(|(regular, _)| ui_paths.as_ref().is_none_or(|(ui, _)| ui != regular))
        .and_then(|(regular, _)| pair(&regular, &regular));
    SystemFonts { ui, cjk }
}
