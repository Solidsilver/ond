//! The window: where it was, and what it should become. An app keeps its own
//! modes (Ond's Full, Player and Mini) and turns the current one into a
//! [`Plan`]; the plan opens the window at startup and resizes it later, with
//! the title bar each platform gets (folded on macOS, see `chrome`; our own
//! buttons on Linux, see `csd`).
//!
//! A window's size and place are kept in screen units: egui's points at 100%
//! zoom, the system's own points. Zooming (see `zoom`) leaves the window as it
//! is and lays the content out in fewer points or more, as a browser does, so
//! a saved size means the same window at every zoom. A plan's minimum is the
//! content's, in points, so it grows with the zoom: what fits at 100% still
//! fits at 200%. Nothing is asked of the window larger than its screen,
//! though; there the content has to make do.

use egui::{Context, ViewportCommand, WindowLevel};
use serde::{Deserialize, Serialize};

/// Where a window was: outer position (when the platform reports it) and
/// inner size, in screen units (see the module's header).
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
        let zoom = ctx.zoom_factor();
        ctx.input(|i| {
            let v = i.viewport();
            let size = v.inner_rect?.size();
            // macOS reports a zoomed window as maximized; zoom is just a size
            // there, remembered like any other.
            let maximized = !cfg!(target_os = "macos") && v.maximized.unwrap_or(false);
            Some(Self { pos: v.outer_rect.map(|r| [r.min.x * zoom, r.min.y * zoom]), size: [size.x * zoom, size.y * zoom], maximized })
        })
    }
}

/// What the window should become, in the app's window mode `M`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan<M> {
    pub mode: M,
    /// In screen units; raised to fit the minimum at the zoom it opens at.
    pub size: [f32; 2],
    /// The content's minimum, in points: larger on screen as the zoom grows.
    pub min_size: [f32; 2],
    /// In screen units.
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

    /// The window this plan describes, for startup, at the `zoom` factor the
    /// app will draw at. egui's own factor is still 1 when the window is made,
    /// so sizes given here are screen units.
    pub fn builder(&self, b: egui::ViewportBuilder, zoom: f32) -> egui::ViewportBuilder {
        let min = self.min_on_screen(zoom, None);
        let b = b.with_inner_size(self.fit(self.size, min)).with_min_inner_size(min).with_window_level(self.level()).with_maximized(self.maximized);
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
        let zoom = ctx.zoom_factor();
        ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(egui::vec2(1.0, 1.0)));
        self.apply_frame(ctx);
        resize(ctx, self.target(ctx));
        ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(points(self.min_on_screen(zoom, monitor(ctx)), zoom)));
        if let Some(p) = self.pos {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(points(p, zoom).to_pos2()));
        }
        ctx.send_viewport_cmd(ViewportCommand::WindowLevel(self.level()));
        if self.maximized {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(true));
        }
    }

    /// The size, in screen units, the window gets from [`apply`](Self::apply)
    /// at the zoom `ctx` draws at: the plan's, raised to the minimum.
    pub fn target(&self, ctx: &Context) -> [f32; 2] {
        self.fit(self.size, self.min_on_screen(ctx.zoom_factor(), monitor(ctx)))
    }

    /// After the zoom changed: the minimum follows it, and a window `now`
    /// smaller than that grows to it (a square one staying square). A larger
    /// one keeps its size; the content reflows in it.
    pub fn refit(&self, ctx: &Context, now: Option<Geometry>) {
        let zoom = ctx.zoom_factor();
        let min = self.min_on_screen(zoom, monitor(ctx));
        ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(points(min, zoom)));
        if let Some(g) = now.filter(|g| !g.maximized) {
            let want = self.fit(g.size, min);
            if (want[0] - g.size[0]).abs() > 0.5 || (want[1] - g.size[1]).abs() > 0.5 {
                resize(ctx, want);
            }
        }
    }

    /// The minimum in screen units at `zoom`, kept within `monitor` (its size
    /// in screen units) when that's known.
    fn min_on_screen(&self, zoom: f32, monitor: Option<[f32; 2]>) -> [f32; 2] {
        let min = [self.min_size[0] * zoom, self.min_size[1] * zoom];
        match monitor {
            // Room for the menu bar and the dock (or a panel).
            Some(m) => [min[0].min(m[0] * 0.9), min[1].min(m[1] * 0.9)],
            None => min,
        }
    }

    /// `size` raised to `min`, and square if the plan is.
    fn fit(&self, size: [f32; 2], min: [f32; 2]) -> [f32; 2] {
        let s = [size[0].max(min[0]), size[1].max(min[1])];
        if self.square { [s[0].max(s[1]); 2] } else { s }
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

/// Asks for the window's inner size to become `size`, in screen units.
pub fn resize(ctx: &Context, size: [f32; 2]) {
    ctx.send_viewport_cmd(ViewportCommand::InnerSize(points(size, ctx.zoom_factor())));
}

/// Screen units as egui's points at `zoom`, which window commands take.
fn points(screen: [f32; 2], zoom: f32) -> egui::Vec2 {
    egui::vec2(screen[0], screen[1]) / zoom
}

/// The screen the window is on, in screen units, when egui knows it.
fn monitor(ctx: &Context) -> Option<[f32; 2]> {
    let zoom = ctx.zoom_factor();
    ctx.input(|i| i.viewport().monitor_size).map(|m| [m.x * zoom, m.y * zoom])
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

    #[test]
    fn the_minimum_grows_with_the_zoom_and_the_size_with_it_only_when_it_must() {
        let plan = Plan::framed((), [960.0, 640.0], [640.0, 420.0]);
        assert_eq!(plan.fit(plan.size, plan.min_on_screen(1.0, None)), [960.0, 640.0]);
        assert_eq!(plan.min_on_screen(2.0, None), [1280.0, 840.0]);
        assert_eq!(plan.fit(plan.size, plan.min_on_screen(2.0, None)), [1280.0, 840.0], "too small for the content at 200%");
        assert_eq!(plan.fit(plan.size, plan.min_on_screen(1.25, None)), [960.0, 640.0], "room enough at 125%: the window keeps its size");
        // Never more than the screen holds.
        assert_eq!(plan.min_on_screen(2.0, Some([1440.0, 900.0])), [1280.0, 810.0]);
        let mut mini = Plan::framed((), [280.0, 280.0], [240.0, 240.0]);
        mini.square = true;
        assert_eq!(mini.fit([300.0, 280.0], mini.min_on_screen(1.5, None)), [360.0, 360.0], "a square window grows square");
    }

    #[test]
    fn a_window_is_read_in_screen_units_whatever_the_zoom() {
        let ctx = Context::default();
        crate::zoom::install(&ctx, 200);
        let mut raw = egui::RawInput::default();
        let v = raw.viewports.entry(egui::ViewportId::ROOT).or_default();
        // As egui-winit reports a 1000×800 window at 200%: in points.
        v.inner_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(500.0, 400.0)));
        v.outer_rect = Some(egui::Rect::from_min_size(egui::pos2(50.0, 20.0), egui::vec2(500.0, 414.0)));
        let mut seen = None;
        let mut full = ctx.run_ui(raw, |ui| seen = Geometry::read(ui.ctx()));
        full.textures_delta.clear();
        let seen = seen.unwrap();
        assert_eq!((seen.size, seen.pos), ([1000.0, 800.0], Some([100.0, 40.0])));
        assert_eq!(points(seen.size, ctx.zoom_factor()), egui::vec2(500.0, 400.0), "and asked for in points again");
    }
}
