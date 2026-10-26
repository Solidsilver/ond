//! The window itself: switching modes (Full, Player, Mini) and following
//! the window until it reaches the size asked for, keeping Mini square, and
//! the column widths and window state kept in the preferences.

use super::{OndApp, layout, now_playing_view, toolbar};
use crate::window::{Geometry, WindowMode, Windows};
use std::time::Duration;

/// How long a mode switch waits for the window to leave maximized.
const UNMAXIMIZE_WAIT: Duration = Duration::from_secs(1);

/// Waiting for the window to reach a size we asked for.
#[derive(Debug, Clone, Copy)]
pub struct Settle {
    pub want: [f32; 2],
    /// Give up (and take the window as it is) after this.
    until: std::time::Instant,
    /// When the size was last asked for.
    pub asked: std::time::Instant,
}

impl Settle {
    const WAIT: Duration = Duration::from_millis(1500);
    const REASK: Duration = Duration::from_millis(200);

    fn new(want: [f32; 2]) -> Self {
        let now = std::time::Instant::now();
        Self { want, until: now + Self::WAIT, asked: now }
    }

    /// Still short of the size, and still willing to wait for it.
    fn waiting(&self, size: [f32; 2]) -> bool {
        let off = (size[0] - self.want[0]).abs() > 2.0 || (size[1] - self.want[1]).abs() > 2.0;
        off && std::time::Instant::now() < self.until
    }
}

/// A change to the window: its mode, the Player's lyrics drawer, its title
/// bar, the queue column.
pub enum WindowAction {
    SetMode(WindowMode),
    /// Into Mini, or back to where Mini was entered from.
    ToggleMini,
    /// The full window ↔ the player last used (Player or Mini).
    TogglePlayer,
    /// Open or close the Player's lyrics drawer.
    SetLyricsDrawer(bool),
    /// The system's title bar instead of our own window buttons (Linux).
    SetSystemTitleBar(bool),
    /// Show or hide the queue: the column, or in narrow windows the sheet.
    ToggleQueue,
}

/// The window as last seen, and mode switches still settling.
#[derive(Default)]
pub struct WindowState {
    /// Mini is held square: what was last asked of the platform, and whether it
    /// could (if not, the size is snapped back each frame instead).
    pub square_wanted: Option<bool>,
    pub square_native: bool,
    /// The window as last seen, saved on quit.
    pub seen: Option<Geometry>,
    /// A mode switch waiting for the window to leave maximized first: a
    /// compositor answers an un-maximize with the window's old size, which
    /// would land after (and over) the new one.
    pub unmaximizing: Option<(crate::window::Plan, std::time::Instant)>,
    /// After a mode switch, until the window reaches the size asked for. It
    /// reports its old size for a frame or two meanwhile.
    pub settling: Option<Settle>,
    /// When the current mode switch was asked for (for the debug log).
    pub switch_started: Option<std::time::Instant>,
}

impl OndApp {
    pub(super) fn apply_window(&mut self, action: WindowAction) {
        match action {
            WindowAction::SetMode(mode) => self.set_mode(Some(mode)),
            WindowAction::ToggleMini => self.set_mode(None),
            WindowAction::TogglePlayer => {
                let seen = self.ui.window.seen;
                let plan = self.edit_windows(|w| w.toggle_player(seen));
                self.follow_plan(plan);
            }
            WindowAction::SetLyricsDrawer(open) => {
                let seen = self.ui.window.seen;
                let plan = self.edit_windows(|w| w.set_lyrics(open, seen));
                self.follow_plan(plan);
            }
            WindowAction::SetSystemTitleBar(on) => {
                let plan = self.edit_windows(|w| {
                    w.system_title_bar = on;
                    w.plan()
                });
                plan.apply_frame(&self.ctx);
            }
            WindowAction::ToggleQueue if self.ui.queue_narrow => self.ui.queue_sheet = !self.ui.queue_sheet,
            WindowAction::ToggleQueue => self.prefs.edit(|s| s.show_queue = !s.show_queue),
        }
    }

    /// Saves new column widths; defaults are stored as "not set".
    pub(super) fn set_columns(&self, c: layout::Columns) {
        self.prefs.edit(|s| {
            s.sidebar_width = (c.left != layout::LEFT_DEFAULT).then_some(c.left);
            s.queue_width = (c.right != layout::RIGHT_DEFAULT).then_some(c.right);
        });
    }

    /// Changes the window modes and geometry kept in the preferences.
    pub(super) fn edit_windows<R>(&self, change: impl FnOnce(&mut Windows) -> R) -> R {
        let mut out = None;
        self.prefs.edit(|s| out = Some(change(&mut s.window)));
        out.expect("edit runs its closure")
    }

    /// Switches window mode (`None` toggles Mini) and saves the choice.
    pub(super) fn set_mode(&mut self, to: Option<WindowMode>) {
        let seen = self.ui.window.seen;
        let plan = self.edit_windows(|w| match to {
            Some(m) => w.switch(m, seen),
            None => w.toggle_mini(seen),
        });
        self.follow_plan(plan);
    }

    /// Resizes the window to `plan` (if any). The window state itself is in the
    /// preferences already (they save in the background).
    pub(super) fn follow_plan(&mut self, plan: Option<crate::window::Plan>) {
        let Some(plan) = plan else { return };
        let ctx = self.ctx.clone();
        // Until the window reports its new size, don't mistake the old one for it.
        self.ui.window.seen = None;
        self.ui.window.switch_started = Some(std::time::Instant::now());
        tracing::debug!(target: "ond::window", mode = ?plan.mode, size = ?plan.size, "window mode switch");
        // A maximized window ignores sizes (Linux compositors maximize a large
        // window on their own), and answers an un-maximize with its old size,
        // which would land over ours. So un-maximize first; `track_window`
        // resizes once the window has let go. (macOS zoom is just a size.)
        if !cfg!(target_os = "macos") && !plan.maximized && ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
            self.ui.window.unmaximizing = Some((plan, std::time::Instant::now()));
            ctx.request_repaint();
            return;
        }
        self.resize_to(plan);
    }

    /// Follows the window each frame: finishes a mode switch (after an
    /// un-maximize, until the new size lands), keeps Mini square, and notes
    /// where the window is for next time.
    pub(super) fn track_window(&mut self, ctx: &egui::Context) {
        let square = self.ui.prefs.mode == WindowMode::Mini;
        // Until the window exists the platform can't lock it; keep asking.
        if self.ui.window.square_wanted != Some(square) || (square && !self.ui.window.square_native) {
            self.ui.window.square_wanted = Some(square);
            self.ui.window.square_native = crate::window::lock_square(square) && square;
        }
        if let Some((plan, since)) = self.ui.window.unmaximizing {
            let still = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            if !still || since.elapsed() > UNMAXIMIZE_WAIT {
                self.ui.window.unmaximizing = None;
                self.resize_to(plan);
            } else {
                ctx.request_repaint();
            }
        }
        if self.ui.window.unmaximizing.is_none()
            && let Some(g) = Geometry::read(ctx)
        {
            match &mut self.ui.window.settling {
                Some(s) if s.waiting(g.size) => {
                    // A compositor can still answer with an older size after
                    // ours; ask again now and then.
                    if !g.maximized && s.asked.elapsed() > Settle::REASK {
                        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(s.want.into()));
                        s.asked = std::time::Instant::now();
                    }
                    ctx.request_repaint();
                }
                _ => {
                    if self.ui.window.settling.take().is_some() {
                        // Our own resize has landed: put presentation back to normal
                        // (see `chrome::end_programmatic_resize`).
                        crate::chrome::end_programmatic_resize();
                        if let Some(t) = self.ui.window.switch_started.take() {
                            tracing::debug!(target: "ond::window", elapsed_ms = t.elapsed().as_secs_f64() * 1000.0, size = ?g.size, "window mode switch settled");
                        }
                    }
                    // Mini is square; where the platform doesn't hold it so, snap back.
                    if square
                        && !self.ui.window.square_native
                        && let Some(side) = crate::window::square(self.ui.window.seen.map_or(g.size, |s| s.size), g.size)
                    {
                        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(side.into()));
                    }
                    self.ui.window.seen = Some(g);
                }
            }
        }
    }

    pub(super) fn resize_to(&mut self, plan: crate::window::Plan) {
        plan.apply(&self.ctx);
        // A maximized window takes the screen's size, not the plan's.
        self.ui.window.settling = (!plan.maximized).then(|| Settle::new(plan.size));
    }

}

/// The height of the bar along the top in `mode`, which the traffic lights
/// centre in and our own window buttons sit in; `None`: no bar (Mini).
pub(super) fn title_bar_h(mode: WindowMode) -> Option<f32> {
    match mode {
        WindowMode::Full => Some(toolbar::TOOLBAR_H),
        WindowMode::Player => Some(now_playing_view::PLAYER_BAR_H),
        WindowMode::Mini => None,
    }
}
