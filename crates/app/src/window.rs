//! Window modes. Full, Compact and Mini are one window that changes size, so
//! playback state never has to be shared between windows. Each mode remembers
//! where it was and how big; Mini and Compact stay anchored at the same
//! top-left corner, so "Up next" grows the mini player downward.

use egui::{Context, ViewportCommand, WindowLevel};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowMode {
    #[default]
    Full,
    Compact,
    Mini,
}

impl WindowMode {
    fn default_size(self) -> [f32; 2] {
        match self {
            Self::Full => [1360.0, 860.0],
            Self::Compact => [340.0, 660.0],
            Self::Mini => [280.0, 280.0],
        }
    }

    fn min_size(self) -> [f32; 2] {
        match self {
            Self::Full => [900.0, 560.0],
            Self::Compact => [300.0, 420.0],
            Self::Mini => [200.0, 200.0],
        }
    }
}

/// Where a window was: outer position (when the platform reports it) and inner size, in points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Geometry {
    pub pos: Option<[f32; 2]>,
    pub size: [f32; 2],
}

impl Geometry {
    /// The window as egui last saw it.
    pub fn read(ctx: &Context) -> Option<Self> {
        ctx.input(|i| {
            let v = i.viewport();
            let size = v.inner_rect?.size();
            Some(Self { pos: v.outer_rect.map(|r| [r.min.x, r.min.y]), size: [size.x, size.y] })
        })
    }
}

/// What the window should become.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan {
    pub mode: WindowMode,
    pub size: [f32; 2],
    pub min_size: [f32; 2],
    pub pos: Option<[f32; 2]>,
    /// Mini has no title bar; it's dragged by its artwork.
    pub decorations: bool,
    pub on_top: bool,
}

impl Plan {
    /// The window this plan describes, for startup.
    pub fn builder(&self, b: egui::ViewportBuilder) -> egui::ViewportBuilder {
        let b = b.with_inner_size(self.size).with_min_inner_size(self.min_size).with_decorations(self.decorations).with_window_level(self.level());
        match self.pos {
            Some(p) => b.with_position(p),
            None => b,
        }
    }

    /// Turns the running window into this one.
    pub fn apply(&self, ctx: &Context) {
        // Lower the minimum first, or shrinking below the old one is refused.
        ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(self.min_size.into()));
        ctx.send_viewport_cmd(ViewportCommand::Decorations(self.decorations));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(self.size.into()));
        if let Some(p) = self.pos {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(p.into()));
        }
        ctx.send_viewport_cmd(ViewportCommand::WindowLevel(self.level()));
    }

    fn level(&self) -> WindowLevel {
        if self.on_top { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal }
    }
}

fn yes() -> bool {
    true
}

/// The current mode and each mode's last geometry; saved with the settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Windows {
    #[serde(default)]
    pub mode: WindowMode,
    #[serde(default)]
    pub geometry: HashMap<WindowMode, Geometry>,
    /// Keep the mini player above other windows.
    #[serde(default = "yes")]
    pub mini_on_top: bool,
    /// The mode Mini returns to (Full or Compact).
    #[serde(default)]
    pub before_mini: WindowMode,
}

impl Default for Windows {
    fn default() -> Self {
        Self { mode: WindowMode::Full, geometry: HashMap::new(), mini_on_top: true, before_mini: WindowMode::Full }
    }
}

impl Windows {
    /// The window for the current mode.
    pub fn plan(&self) -> Plan {
        let mode = self.mode;
        let saved = self.geometry.get(&mode).copied();
        let min = mode.min_size();
        let size = saved.map_or(mode.default_size(), |g| [g.size[0].max(min[0]), g.size[1].max(min[1])]);
        Plan {
            mode,
            size,
            min_size: min,
            pos: saved.and_then(|g| g.pos),
            decorations: mode != WindowMode::Mini,
            on_top: mode == WindowMode::Mini && self.mini_on_top,
        }
    }

    /// Records where the window is now, for the current mode.
    pub fn remember(&mut self, now: Geometry) {
        self.geometry.insert(self.mode, now);
    }

    /// Leaves the current mode for `to`, remembering where the window was.
    /// Between Mini and Compact the window keeps its top-left corner, so it
    /// grows downward and shrinks back up. `None` when already there.
    pub fn switch(&mut self, to: WindowMode, now: Option<Geometry>) -> Option<Plan> {
        let from = self.mode;
        if to == from {
            return None;
        }
        if let Some(g) = now {
            self.remember(g);
        }
        if to == WindowMode::Mini {
            self.before_mini = from;
        }
        self.mode = to;
        let mut plan = self.plan();
        let small = |m| matches!(m, WindowMode::Mini | WindowMode::Compact);
        if small(from) && small(to)
            && let Some(p) = now.and_then(|g| g.pos)
        {
            plan.pos = Some(p);
        }
        Some(plan)
    }

    /// Mini, or back from it to where it was entered from.
    pub fn toggle_mini(&mut self, now: Option<Geometry>) -> Option<Plan> {
        let to = if self.mode == WindowMode::Mini { self.before_mini } else { WindowMode::Mini };
        self.switch(to, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32, y: f32, w: f32, h: f32) -> Option<Geometry> {
        Some(Geometry { pos: Some([x, y]), size: [w, h] })
    }

    #[test]
    fn defaults_per_mode() {
        let mut w = Windows::default();
        let full = w.plan();
        assert_eq!((full.size, full.decorations, full.on_top, full.pos), ([1360.0, 860.0], true, false, None));
        let mini = w.switch(WindowMode::Mini, at(100.0, 80.0, 1200.0, 800.0)).unwrap();
        assert_eq!((mini.size, mini.decorations, mini.on_top), ([280.0, 280.0], false, true));
        // No mini position yet: the OS places it.
        assert_eq!(mini.pos, None);
    }

    #[test]
    fn full_geometry_comes_back() {
        let mut w = Windows::default();
        w.switch(WindowMode::Mini, at(100.0, 80.0, 1200.0, 800.0));
        let back = w.switch(WindowMode::Full, at(1500.0, 40.0, 280.0, 280.0)).unwrap();
        assert_eq!((back.size, back.pos), ([1200.0, 800.0], Some([100.0, 80.0])));
        // And the mini player's spot is remembered for next time.
        let mini = w.switch(WindowMode::Mini, at(100.0, 80.0, 1200.0, 800.0)).unwrap();
        assert_eq!((mini.size, mini.pos), ([280.0, 280.0], Some([1500.0, 40.0])));
    }

    #[test]
    fn mini_grows_down_into_compact_and_back_up() {
        let mut w = Windows::default();
        w.switch(WindowMode::Mini, at(0.0, 0.0, 1360.0, 860.0));
        // Compact was last somewhere else, but it opens where the mini player is.
        w.geometry.insert(WindowMode::Compact, Geometry { pos: Some([5.0, 5.0]), size: [360.0, 700.0] });
        let compact = w.switch(WindowMode::Compact, at(1500.0, 40.0, 280.0, 280.0)).unwrap();
        assert_eq!((compact.pos, compact.size, compact.decorations), (Some([1500.0, 40.0]), [360.0, 700.0], true));
        let mini = w.switch(WindowMode::Mini, at(1490.0, 30.0, 360.0, 700.0)).unwrap();
        assert_eq!(mini.pos, Some([1490.0, 30.0]));
    }

    #[test]
    fn toggle_returns_to_where_mini_came_from() {
        let mut w = Windows::default();
        w.switch(WindowMode::Compact, None);
        assert_eq!(w.toggle_mini(None).unwrap().mode, WindowMode::Mini);
        assert_eq!(w.toggle_mini(None).unwrap().mode, WindowMode::Compact);
        assert_eq!(w.switch(WindowMode::Compact, None), None);
    }

    #[test]
    fn sizes_never_below_the_minimum_and_on_top_is_optional() {
        let mut w = Windows { mini_on_top: false, ..Default::default() };
        w.geometry.insert(WindowMode::Mini, Geometry { pos: None, size: [50.0, 900.0] });
        let mini = w.switch(WindowMode::Mini, None).unwrap();
        assert_eq!((mini.size, mini.on_top), ([200.0, 900.0], false));
    }

    #[test]
    fn saves_as_json() {
        let mut w = Windows::default();
        w.switch(WindowMode::Mini, at(1.0, 2.0, 1000.0, 700.0));
        let json = serde_json::to_string(&w).unwrap();
        assert!(json.contains("\"mini\""));
        assert_eq!(serde_json::from_str::<Windows>(&json).unwrap(), w);
        // Older settings files have no window section.
        assert_eq!(serde_json::from_str::<Windows>("{}").unwrap(), Windows::default());
    }
}
