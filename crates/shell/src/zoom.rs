//! Zoom: the whole interface larger or smaller, in a browser's steps, with
//! its keys (⌘+ or ⌘=, ⌘−, ⌘0). egui's zoom factor scales every point, so
//! text, icons, spacing and the window's own sizes in points grow together.
//!
//! The app keeps the level as a percentage in its preferences and calls
//! [`install`] with it before the first frame, and [`set`] when it changes.
//! egui's own zoom keys are turned off: they step by tenths, and would step
//! again on top of the app's.
//!
//! The factor is set at once rather than through
//! [`egui::Context::set_zoom_factor`], which waits for the next pass. Window
//! commands sent the same frame (a new minimum size, see
//! [`crate::window::Plan::refit`]) are turned into pixels with the factor egui
//! holds when they're sent, so they need the new one.

use crate::kit::Toasts;
use egui::{Context, InputState, KeyboardShortcut};
use serde::Deserialize;

/// The levels, in percent. Each step is a key press; the stepper in Settings
/// shows the same ones.
pub const STEPS: [u16; 8] = [80, 90, 100, 110, 125, 150, 175, 200];

/// Actual size.
pub const DEFAULT: u16 = 100;

/// A press of a zoom key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    In,
    Out,
    Reset,
}

impl Step {
    /// The level after this step from `percent`. The ends stay where they are.
    pub fn from(self, percent: u16) -> u16 {
        match self {
            Step::In => larger(percent),
            Step::Out => smaller(percent),
            Step::Reset => DEFAULT,
        }
    }
}

/// The step nearest `percent`; between two, the smaller.
pub fn nearest(percent: f64) -> u16 {
    if !percent.is_finite() {
        return DEFAULT;
    }
    STEPS.into_iter().min_by(|a, b| (f64::from(*a) - percent).abs().total_cmp(&(f64::from(*b) - percent).abs())).unwrap_or(DEFAULT)
}

/// The next step up from `percent`, or the largest.
pub fn larger(percent: u16) -> u16 {
    STEPS.into_iter().find(|s| *s > percent).unwrap_or(STEPS[STEPS.len() - 1])
}

/// The next step down from `percent`, or the smallest.
pub fn smaller(percent: u16) -> u16 {
    STEPS.into_iter().rev().find(|s| *s < percent).unwrap_or(STEPS[0])
}

/// egui's factor for `percent`, after rounding it to a step.
pub fn factor(percent: u16) -> f32 {
    f32::from(nearest(f64::from(percent))) / 100.0
}

/// "125%".
pub fn label(percent: u16) -> String {
    format!("{percent}%")
}

/// The toast that confirms a change: "Zoom 125%".
pub fn toast_text(percent: u16) -> String {
    format!("Zoom {percent}%")
}

/// Shows that the zoom is now `percent`, in place of an earlier zoom toast
/// still showing, so pressing ⌘+ three times leaves one toast, not three.
pub fn announce<A>(toasts: &mut Toasts<A>, percent: u16) {
    for step in STEPS {
        toasts.remove(&toast_text(step));
    }
    toasts.push(toast_text(percent), false);
}

/// The keys, as the overlay and menus show them.
pub const KEYS_IN: &str = "⌘+";
pub const KEYS_OUT: &str = "⌘−";
pub const KEYS_RESET: &str = "⌘0";

/// Takes a zoom key pressed this frame, if any: ⌘+ (or ⌘=, the same key
/// without Shift), ⌘− and ⌘0. Shift is ignored, since `+` needs it on some
/// layouts and not others.
pub fn take(input: &mut InputState) -> Option<Step> {
    use egui::gui_zoom::kb_shortcuts::{ZOOM_IN, ZOOM_IN_SECONDARY, ZOOM_OUT, ZOOM_RESET};
    let mut take = |s: KeyboardShortcut| input.consume_shortcut(&s);
    if take(ZOOM_IN) || take(ZOOM_IN_SECONDARY) {
        Some(Step::In)
    } else if take(ZOOM_OUT) {
        Some(Step::Out)
    } else if take(ZOOM_RESET) {
        Some(Step::Reset)
    } else {
        None
    }
}

/// Before the first frame: egui's own zoom keys off, and the saved level on,
/// so the window never shows a frame at 100% first.
pub fn install(ctx: &Context, percent: u16) {
    ctx.options_mut(|o| {
        o.zoom_with_keyboard = false;
        o.zoom_factor = factor(percent);
    });
}

/// Changes the zoom to `percent` now (see the module's header for why not
/// next pass), and draws again at it.
pub fn set(ctx: &Context, percent: u16) {
    let f = factor(percent);
    if ctx.zoom_factor() != f {
        ctx.options_mut(|o| o.zoom_factor = f);
        ctx.request_repaint();
    }
}

/// For `#[serde(deserialize_with)]`: a saved level, rounded to the nearest
/// step; anything that isn't a number reads as [`DEFAULT`], so a hand-edited
/// file costs only this setting.
pub fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u16, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Saved {
        Number(f64),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Saved::deserialize(d)? {
        Saved::Number(n) => nearest(n),
        Saved::Other(_) => DEFAULT,
    })
}

/// For `#[serde(default)]`.
pub fn default_percent() -> u16 {
    DEFAULT
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Key, Modifiers};

    #[test]
    fn steps_go_up_and_down_and_stop_at_the_ends() {
        assert_eq!(larger(100), 110);
        assert_eq!(larger(110), 125);
        assert_eq!(smaller(100), 90);
        assert_eq!(smaller(125), 110);
        assert_eq!(larger(200), 200, "the largest stays");
        assert_eq!(smaller(80), 80, "the smallest stays");
        // From a level between steps, to the neighbour on that side.
        assert_eq!(larger(130), 150);
        assert_eq!(smaller(130), 125);
        assert_eq!(Step::Reset.from(175), 100);
        assert_eq!(Step::In.from(90), 100);
    }

    #[test]
    fn any_number_rounds_to_a_step() {
        assert_eq!(nearest(100.0), 100);
        assert_eq!(nearest(133.0), 125);
        assert_eq!(nearest(140.0), 150);
        assert_eq!(nearest(5.0), 80);
        assert_eq!(nearest(1e9), 200);
        assert_eq!(nearest(f64::NAN), DEFAULT);
        assert_eq!(factor(150), 1.5);
        assert_eq!(factor(7), 0.8);
    }

    #[test]
    fn a_saved_level_is_read_leniently() {
        #[derive(Deserialize)]
        struct S {
            #[serde(default = "default_percent", deserialize_with = "lenient")]
            zoom: u16,
        }
        let read = |json: &str| serde_json::from_str::<S>(json).unwrap().zoom;
        assert_eq!(read("{}"), 100);
        assert_eq!(read(r#"{"zoom": 125}"#), 125);
        assert_eq!(read(r#"{"zoom": 1000}"#), 200);
        assert_eq!(read(r#"{"zoom": -3}"#), 80);
        assert_eq!(read(r#"{"zoom": 118.5}"#), 125);
        assert_eq!(read(r#"{"zoom": "big"}"#), 100);
        assert_eq!(read(r#"{"zoom": null}"#), 100);
    }

    #[test]
    fn a_new_zoom_toast_replaces_the_last_one() {
        let mut toasts: Toasts<()> = Toasts::default();
        toasts.push("Downloaded".into(), false);
        announce(&mut toasts, 110);
        announce(&mut toasts, 125);
        assert_eq!(toasts.texts(), ["Downloaded", "Zoom 125%"]);
    }

    fn press(key: Key, modifiers: Modifiers) -> Option<Step> {
        let ctx = Context::default();
        let ev = egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        let mut out = None;
        let mut full = ctx.run_ui(egui::RawInput { events: vec![ev], ..Default::default() }, |ui| out = ui.ctx().input_mut(take));
        full.textures_delta.clear();
        out
    }

    #[test]
    fn the_zoom_keys_are_taken_with_or_without_shift() {
        assert_eq!(press(Key::Plus, Modifiers::COMMAND | Modifiers::SHIFT), Some(Step::In));
        assert_eq!(press(Key::Plus, Modifiers::COMMAND), Some(Step::In));
        assert_eq!(press(Key::Equals, Modifiers::COMMAND), Some(Step::In));
        assert_eq!(press(Key::Minus, Modifiers::COMMAND), Some(Step::Out));
        assert_eq!(press(Key::Num0, Modifiers::COMMAND), Some(Step::Reset));
        assert_eq!(press(Key::Num0, Modifiers::NONE), None, "a plain 0 is typing");
    }

    #[test]
    fn installing_turns_off_eguis_own_keys_and_sets_the_factor_at_once() {
        let ctx = Context::default();
        install(&ctx, 150);
        assert_eq!(ctx.zoom_factor(), 1.5);
        assert!(!ctx.options(|o| o.zoom_with_keyboard));
        set(&ctx, 90);
        assert_eq!(ctx.zoom_factor(), 0.9, "no wait for the next pass");
    }
}
