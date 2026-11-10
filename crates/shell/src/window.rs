//! The window: where it was, and what it should become. An app keeps its own
//! modes (Ond's Full, Player and Mini) and turns the current one into a
//! [`Plan`]; the plan opens the window at startup and resizes it later, with
//! the title bar each platform gets (folded on macOS, see `chrome`; our own
//! buttons on Linux, see `csd`).

use egui::{Context, ViewportCommand, WindowLevel};
use serde::{Deserialize, Serialize};

/// Where a window was: outer position (when the platform reports it) and inner size, in points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Geometry {
    pub pos: Option<[f32; 2]>,
    pub size: [f32; 2],
    /// Maximized. The size and position are then the window's normal ones,
    /// from before it was maximized.
    #[serde(default)]
    pub maximized: bool,
}

impl Geometry {
    /// The window as egui last saw it.
    pub fn read(ctx: &Context) -> Option<Self> {
        ctx.input(|i| {
            let v = i.viewport();
            let size = v.inner_rect?.size();
            // macOS reports a zoomed window as maximized; zoom is just a size
            // there, remembered like any other.
            let maximized = !cfg!(target_os = "macos") && v.maximized.unwrap_or(false);
            Some(Self { pos: v.outer_rect.map(|r| [r.min.x, r.min.y]), size: [size.x, size.y], maximized })
        })
    }
}

/// What the window should become, in the app's window mode `M`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan<M> {
    pub mode: M,
    pub size: [f32; 2],
    pub min_size: [f32; 2],
    pub pos: Option<[f32; 2]>,
    /// Ond's Mini has no title bar; it's dragged by its artwork. (On macOS
    /// the window keeps its frame and just hides the traffic lights; see
    /// `chrome`.)
    pub decorations: bool,
    pub on_top: bool,
    /// Keep the window square while resizing (Mini).
    pub square: bool,
    /// Our own window buttons in the bar instead of a system title bar (see `csd`).
    pub controls: bool,
    /// Maximize after resizing (the window was maximized when last used).
    pub maximized: bool,
}

impl<M> Plan<M> {
    /// An ordinary window in `mode`: framed, with our own buttons where the
    /// platform draws them in the bar (see `csd`), at the system's choice of place.
    pub fn framed(mode: M, size: [f32; 2], min_size: [f32; 2]) -> Self {
        let controls = crate::csd::SUPPORTED;
        Self { mode, size, min_size, pos: None, decorations: !controls, on_top: false, square: false, controls, maximized: false }
    }

    /// The window this plan describes, for startup.
    pub fn builder(&self, b: egui::ViewportBuilder) -> egui::ViewportBuilder {
        let b = b.with_inner_size(self.size).with_min_inner_size(self.min_size).with_window_level(self.level()).with_maximized(self.maximized);
        // macOS: the title bar folds into our toolbar (see `chrome`).
        let b = if cfg!(target_os = "macos") {
            b.with_decorations(true).with_fullsize_content_view(true).with_titlebar_shown(false).with_title_shown(false)
        } else {
            b.with_decorations(self.decorations)
        };
        match self.pos {
            Some(p) => b.with_position(p),
            None => b,
        }
    }

    /// Turns the running window into this one.
    pub fn apply(&self, ctx: &Context) {
        // Drop the minimum out of the way, resize, then set the new one: a
        // larger minimum set first would resize the window once on its own,
        // and a smaller one set after would refuse the shrink.
        ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(egui::vec2(1.0, 1.0)));
        self.apply_frame(ctx);
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(self.size.into()));
        ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(self.min_size.into()));
        if let Some(p) = self.pos {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(p.into()));
        }
        ctx.send_viewport_cmd(ViewportCommand::WindowLevel(self.level()));
        if self.maximized {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(true));
        }
    }

    /// The system frame or our own controls (see `csd`), as this plan has it.
    /// macOS keeps its frame and folds the title bar instead (see `chrome`).
    pub fn apply_frame(&self, ctx: &Context) {
        crate::csd::set_window(ctx, self.controls, !self.decorations);
        if !cfg!(target_os = "macos") {
            ctx.send_viewport_cmd(ViewportCommand::Decorations(self.decorations));
        }
    }

    fn level(&self) -> WindowLevel {
        if self.on_top { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal }
    }
}

/// Asks the platform to keep the window's content square while it's resized
/// (or to stop). Returns whether it could; where it can't, the app snaps the
/// size back with [`square`] instead.
#[cfg(target_os = "macos")]
pub fn lock_square(on: bool) -> bool {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_core_foundation::CGSize;

    let Some(mtm) = MainThreadMarker::new() else { return false };
    let app = NSApplication::sharedApplication(mtm);
    let Some(window) = app.mainWindow().or_else(|| app.keyWindow()).or_else(|| app.windows().firstObject()) else { return false };
    let one = CGSize { width: 1.0, height: 1.0 };
    if on {
        window.setContentAspectRatio(one);
    } else {
        // Setting resize increments is how AppKit drops an aspect ratio.
        window.setContentResizeIncrements(one);
    }
    true
}

#[cfg(not(target_os = "macos"))]
pub fn lock_square(_: bool) -> bool {
    false
}

/// The square nearest to `now` for a window that was `prev`: the side the
/// user is dragging wins. `None` when it's already square (within a point).
pub fn square(prev: [f32; 2], now: [f32; 2]) -> Option<[f32; 2]> {
    if (now[0] - now[1]).abs() <= 1.0 {
        return None;
    }
    let side = if (now[0] - prev[0]).abs() >= (now[1] - prev[1]).abs() { now[0] } else { now[1] };
    Some([side, side])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mini_window_stays_square_following_the_dragged_edge() {
        assert_eq!(square([280.0, 280.0], [280.0, 280.5]), None);
        // Dragging the right edge: the width wins.
        assert_eq!(square([280.0, 280.0], [340.0, 280.0]), Some([340.0, 340.0]));
        // Dragging the bottom edge: the height wins.
        assert_eq!(square([280.0, 280.0], [280.0, 250.0]), Some([250.0, 250.0]));
    }

    #[test]
    fn a_framed_window_draws_its_own_buttons_only_where_the_platform_wants_them() {
        let plan = Plan::framed((), [960.0, 640.0], [640.0, 420.0]);
        assert_eq!((plan.controls, plan.decorations), (crate::csd::SUPPORTED, !crate::csd::SUPPORTED));
        assert_eq!((plan.pos, plan.on_top, plan.square, plan.maximized), (None, false, false, false));
    }
}
