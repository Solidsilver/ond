//! Window modes. Full, Player and Mini are one window that changes size, so
//! playback state never has to be shared between windows. Each mode remembers
//! where it was and how big; Mini and Player stay anchored at the same
//! top-left corner, so "Up next" grows the mini player into the player.
//!
//! The Player's lyrics open as a drawer that widens the window to the right;
//! its remembered size is the player column alone, so the drawer never
//! squeezes it. Mini is square: macOS keeps it so while resizing, elsewhere
//! [`square`] snaps it back.

use egui::{Context, ViewportCommand, WindowLevel};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowMode {
    #[default]
    Full,
    /// Large artwork, transport and the queue; lyrics in a drawer beside it.
    /// Older settings call it "compact".
    #[serde(alias = "compact")]
    Player,
    Mini,
}

/// Width the lyrics drawer adds to the Player window.
pub const DRAWER_W: f32 = 340.0;

impl WindowMode {
    fn default_size(self) -> [f32; 2] {
        match self {
            Self::Full => [1360.0, 860.0],
            Self::Player => [380.0, 880.0],
            Self::Mini => [280.0, 280.0],
        }
    }

    fn min_size(self) -> [f32; 2] {
        match self {
            Self::Full => [900.0, 560.0],
            Self::Player => [320.0, 600.0],
            Self::Mini => [180.0, 180.0],
        }
    }
}

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

/// What the window should become.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan {
    pub mode: WindowMode,
    pub size: [f32; 2],
    pub min_size: [f32; 2],
    pub pos: Option<[f32; 2]>,
    /// Mini has no title bar; it's dragged by its artwork. (On macOS the
    /// window keeps its frame and Mini just hides the traffic lights; see
    /// `chrome`.)
    pub decorations: bool,
    pub on_top: bool,
    /// Keep the window square while resizing (Mini).
    pub square: bool,
    /// Our own window buttons in the bar instead of a system title bar (see `csd`).
    pub controls: bool,
    /// Maximize after resizing (the full window was maximized when last used).
    pub maximized: bool,
}

impl Plan {
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
    /// The system's title bar and frame instead of our own buttons (Linux).
    #[serde(default)]
    pub system_title_bar: bool,
    /// The mode Mini returns to (Full or Player).
    #[serde(default)]
    pub before_mini: WindowMode,
    /// The player last used (Player or Mini): where the artwork in the full
    /// window's now-playing display takes you.
    #[serde(default = "player")]
    pub last_player: WindowMode,
    /// The Player's lyrics drawer is open.
    #[serde(default)]
    pub lyrics_open: bool,
}

fn player() -> WindowMode {
    WindowMode::Player
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

impl Default for Windows {
    fn default() -> Self {
        Self {
            mode: WindowMode::Full,
            geometry: HashMap::new(),
            mini_on_top: true,
            system_title_bar: false,
            before_mini: WindowMode::Full,
            last_player: WindowMode::Player,
            lyrics_open: false,
        }
    }
}

impl Windows {
    /// The window for the current mode.
    pub fn plan(&self) -> Plan {
        let mode = self.mode;
        let saved = self.geometry.get(&mode).copied();
        let mut min = mode.min_size();
        let mut size = saved.map_or(mode.default_size(), |g| [g.size[0].max(min[0]), g.size[1].max(min[1])]);
        if mode == WindowMode::Mini {
            let side = size[0].max(size[1]);
            size = [side, side];
        }
        let drawer = self.drawer_w();
        size[0] += drawer;
        min[0] += drawer;
        // Linux draws its own buttons in the bar unless asked for the system's.
        let controls = crate::csd::SUPPORTED && mode != WindowMode::Mini && !self.system_title_bar;
        Plan {
            mode,
            size,
            min_size: min,
            pos: saved.and_then(|g| g.pos),
            decorations: mode != WindowMode::Mini && !controls,
            on_top: mode == WindowMode::Mini && self.mini_on_top,
            square: mode == WindowMode::Mini,
            controls,
            // Only the full window goes back to maximized.
            maximized: mode == WindowMode::Full && saved.is_some_and(|g| g.maximized),
        }
    }

    /// Width the drawer adds in the current mode (0 unless the Player has lyrics open).
    pub fn drawer_w(&self) -> f32 {
        if self.mode == WindowMode::Player && self.lyrics_open { DRAWER_W } else { 0.0 }
    }

    /// Records where the window is now, for the current mode. The Player is
    /// remembered without its drawer.
    ///
    /// A maximized window keeps its normal size and place, and only notes
    /// that it was maximized; nothing smaller than the mode allows is kept.
    pub fn remember(&mut self, mut now: Geometry) {
        let min = self.mode.min_size();
        if now.maximized {
            if let Some(g) = self.geometry.get_mut(&self.mode) {
                g.maximized = true;
                return;
            }
            now.size = self.mode.default_size();
            now.pos = None;
        }
        now.size[0] = (now.size[0] - self.drawer_w()).max(min[0]);
        now.size[1] = now.size[1].max(min[1]);
        self.geometry.insert(self.mode, now);
    }

    /// Leaves the current mode for `to`, remembering where the window was.
    /// Between Mini and Player the window keeps its top-left corner, so it
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
        if to != WindowMode::Full {
            self.last_player = to;
        }
        self.mode = to;
        let mut plan = self.plan();
        let small = |m| matches!(m, WindowMode::Mini | WindowMode::Player);
        if small(from)
            && small(to)
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

    /// From the full window to the player last used; from a player, back to full.
    pub fn toggle_player(&mut self, now: Option<Geometry>) -> Option<Plan> {
        let to = if self.mode == WindowMode::Full { self.last_player } else { WindowMode::Full };
        self.switch(to, now)
    }

    /// Opens or closes the Player's lyrics drawer: the window widens (or
    /// narrows) to the right, keeping its place. `None` outside the Player or
    /// when nothing changes.
    pub fn set_lyrics(&mut self, open: bool, now: Option<Geometry>) -> Option<Plan> {
        if self.mode != WindowMode::Player || self.lyrics_open == open {
            self.lyrics_open = open;
            return None;
        }
        if let Some(g) = now {
            self.remember(g);
        }
        self.lyrics_open = open;
        let mut plan = self.plan();
        if let Some(p) = now.and_then(|g| g.pos) {
            plan.pos = Some(p);
        }
        Some(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32, y: f32, w: f32, h: f32) -> Option<Geometry> {
        Some(Geometry { pos: Some([x, y]), size: [w, h], maximized: false })
    }

    #[test]
    fn defaults_per_mode() {
        let mut w = Windows::default();
        let full = w.plan();
        // Linux draws its own title bar buttons; elsewhere the system's frame stays.
        let framed = !crate::csd::SUPPORTED;
        assert_eq!((full.size, full.decorations, full.on_top, full.pos), ([1360.0, 860.0], framed, false, None));
        assert_eq!(full.controls, crate::csd::SUPPORTED);
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
    fn a_maximized_full_window_comes_back_maximized_at_its_normal_size() {
        let mut w = Windows::default();
        // Used at 1200×800, then maximized (the desktop did it, or the user).
        w.remember(Geometry { pos: Some([100.0, 80.0]), size: [1200.0, 800.0], maximized: false });
        let maxed = Geometry { pos: Some([0.0, 0.0]), size: [1368.0, 880.0], maximized: true };
        let mini = w.switch(WindowMode::Mini, Some(maxed)).unwrap();
        assert!(!mini.maximized, "the mini player is never maximized");
        let back = w.switch(WindowMode::Full, at(1500.0, 40.0, 280.0, 280.0)).unwrap();
        assert!(back.maximized);
        assert_eq!((back.size, back.pos), ([1200.0, 800.0], Some([100.0, 80.0])), "and un-maximizes to where it was");
    }

    #[test]
    fn a_window_too_small_for_its_mode_is_not_remembered_that_small() {
        let mut w = Windows::default();
        // The window never reached the full size (a compositor kept it small).
        w.remember(Geometry { pos: None, size: [280.0, 280.0], maximized: false });
        let size = w.geometry[&WindowMode::Full].size;
        assert_eq!(size, WindowMode::Full.min_size());
    }

    #[test]
    fn mini_grows_down_into_the_player_and_back_up() {
        let mut w = Windows::default();
        w.switch(WindowMode::Mini, at(0.0, 0.0, 1360.0, 860.0));
        // The player was last somewhere else, but it opens where the mini player is.
        w.geometry.insert(WindowMode::Player, Geometry { pos: Some([5.0, 5.0]), size: [360.0, 700.0], maximized: false });
        let player = w.switch(WindowMode::Player, at(1500.0, 40.0, 280.0, 280.0)).unwrap();
        assert_eq!((player.pos, player.size, player.decorations), (Some([1500.0, 40.0]), [360.0, 700.0], !crate::csd::SUPPORTED));
        let mini = w.switch(WindowMode::Mini, at(1490.0, 30.0, 360.0, 700.0)).unwrap();
        assert_eq!(mini.pos, Some([1490.0, 30.0]));
    }

    #[test]
    fn toggle_returns_to_where_mini_came_from() {
        let mut w = Windows::default();
        w.switch(WindowMode::Player, None);
        assert_eq!(w.toggle_mini(None).unwrap().mode, WindowMode::Mini);
        assert_eq!(w.toggle_mini(None).unwrap().mode, WindowMode::Player);
        assert_eq!(w.switch(WindowMode::Player, None), None);
    }

    #[test]
    fn sizes_never_below_the_minimum_and_on_top_is_optional() {
        let mut w = Windows { mini_on_top: false, ..Default::default() };
        w.geometry.insert(WindowMode::Mini, Geometry { pos: None, size: [50.0, 90.0], maximized: false });
        let mini = w.switch(WindowMode::Mini, None).unwrap();
        // Raised to the minimum, then made square.
        assert_eq!((mini.size, mini.on_top, mini.square), ([180.0, 180.0], false, true));
    }

    #[test]
    fn the_art_toggles_to_the_player_last_used() {
        let mut w = Windows::default();
        // Nothing used yet: the Player.
        assert_eq!(w.toggle_player(None).unwrap().mode, WindowMode::Player);
        assert_eq!(w.toggle_player(None).unwrap().mode, WindowMode::Full);
        w.switch(WindowMode::Mini, None);
        w.switch(WindowMode::Full, None);
        assert_eq!(w.toggle_player(None).unwrap().mode, WindowMode::Mini);
        assert_eq!(w.toggle_player(None).unwrap().mode, WindowMode::Full);
    }

    #[test]
    fn the_lyrics_drawer_widens_the_player_without_squeezing_it() {
        let mut w = Windows::default();
        w.switch(WindowMode::Player, at(0.0, 0.0, 1360.0, 860.0));
        let open = w.set_lyrics(true, at(200.0, 100.0, 400.0, 780.0)).unwrap();
        assert_eq!((open.size, open.pos), ([400.0 + DRAWER_W, 780.0], Some([200.0, 100.0])));
        assert_eq!(open.min_size[0], WindowMode::Player.min_size()[0] + DRAWER_W);
        // The player column is what's remembered, whatever the drawer.
        w.remember(Geometry { pos: Some([200.0, 100.0]), size: [420.0 + DRAWER_W, 800.0], maximized: false });
        assert_eq!(w.geometry[&WindowMode::Player].size, [420.0, 800.0]);
        let closed = w.set_lyrics(false, at(200.0, 100.0, 420.0 + DRAWER_W, 800.0)).unwrap();
        assert_eq!(closed.size, [420.0, 800.0]);
        assert_eq!(w.set_lyrics(false, None), None);
        // Reopening the player brings the drawer back as it was left.
        w.set_lyrics(true, None);
        w.switch(WindowMode::Full, None);
        assert_eq!(w.switch(WindowMode::Player, None).unwrap().size, [420.0 + DRAWER_W, 800.0]);
    }

    #[test]
    fn mini_stays_square() {
        assert_eq!(square([280.0, 280.0], [280.0, 280.5]), None);
        // Dragging the right edge: the width wins.
        assert_eq!(square([280.0, 280.0], [340.0, 280.0]), Some([340.0, 340.0]));
        // Dragging the bottom edge: the height wins.
        assert_eq!(square([280.0, 280.0], [280.0, 250.0]), Some([250.0, 250.0]));
    }

    #[test]
    fn compact_from_older_settings_becomes_the_player() {
        let old = r#"{"mode":"compact","geometry":{"compact":{"pos":[1.0,2.0],"size":[340.0,660.0]},"full":{"pos":null,"size":[1200.0,800.0]}},"before_mini":"compact"}"#;
        let w: Windows = serde_json::from_str(old).unwrap();
        assert_eq!((w.mode, w.before_mini, w.last_player), (WindowMode::Player, WindowMode::Player, WindowMode::Player));
        assert_eq!(w.geometry[&WindowMode::Player].size, [340.0, 660.0]);
        assert!(serde_json::to_string(&w).unwrap().contains("\"player\""));
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
