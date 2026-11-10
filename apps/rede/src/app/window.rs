//! Bounded mode-switch settling avoids saving the old size under a new mode.

use super::{Action, RedeApp, view_ctx::ViewCtx};
use crate::window::Mode;
use shell::window::{Geometry, Plan};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct State {
    pub seen: Option<Geometry>,
    pending: Option<(Plan<Mode>, Instant)>,
    square: Option<bool>,
    native_square: bool,
}

impl RedeApp {
    pub(super) fn set_mode(&mut self, mode: Mode) {
        let mut windows = self.prefs.get(|s| s.window.clone());
        if let Some(plan) = windows.switch(mode, self.window.seen) {
            self.prefs.edit(|s| s.window = windows.clone());
            self.mode = mode;
            self.window.seen = None;
            // The palette searches the full window's library.
            if mode != Mode::Full {
                self.ui.palette = None;
            }
            self.window.pending = Some((plan, Instant::now()));
            if self.ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
            } else {
                plan.apply(&self.ctx);
            }
            self.ctx.request_repaint();
        }
    }
    pub(super) fn track_window(&mut self, ctx: &egui::Context) {
        let square = self.mode == Mode::Mini;
        if self.window.square != Some(square) {
            self.window.square = Some(square);
            self.window.native_square = shell::window::lock_square(square);
        }
        let Some(g) = Geometry::read(ctx) else { return };
        if let Some((plan, since)) = self.window.pending {
            let reached = (g.size[0] - plan.size[0]).abs() < 2.0 && (g.size[1] - plan.size[1]).abs() < 2.0;
            if !reached && since.elapsed() < Duration::from_millis(1500) {
                if !g.maximized {
                    plan.apply(ctx);
                }
                ctx.request_repaint_after(Duration::from_millis(100));
                return;
            }
            self.window.pending = None;
        }
        if square
            && !self.window.native_square
            && let Some(size) = shell::window::square(self.window.seen.map_or(g.size, |g| g.size), g.size)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size.into()));
        }
        self.window.seen = Some(g);
    }
}

impl ViewCtx<'_> {
    pub(super) fn mode_buttons(&mut self, ui: &mut egui::Ui) {
        self.mode_buttons_sized(ui, 28.0, 14.0, None);
    }

    /// The other modes, and the shortcuts where there's no sidebar to hold
    /// them (the Player and Mini), right to left, as `side` square buttons
    /// with `glyph` icons in `color` (the theme's when `None`).
    pub(super) fn mode_buttons_sized(&mut self, ui: &mut egui::Ui, side: f32, glyph: f32, color: Option<egui::Color32>) {
        use shell::icons::{self, Icon};
        let size = egui::vec2(side, side);
        for (mode, icon, label) in [
            (Mode::Full, Icon::Expand, "Full window (⌥⌘1)"),
            (Mode::Player, Icon::Player, "Player window (⌥⌘2)"),
            (Mode::Mini, Icon::MiniPlayer, "Mini player (⇧⌘M)"),
        ] {
            if mode != self.mode && icons::sized_button(ui, icon, label, size, glyph, color).clicked() {
                self.actions.push(Action::Mode(mode));
            }
        }
        if self.mode != Mode::Full && icons::sized_button(ui, Icon::More, "Keyboard shortcuts (⌘/)", size, glyph, color).clicked() {
            self.actions.push(Action::Shortcuts);
        }
    }
}
