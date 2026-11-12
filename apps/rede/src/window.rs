//! One window changes mode, retaining each geometry and the return from Mini.
//! Plans use the family's frame and always-on-top machinery.

use serde::{Deserialize, Serialize};
use shell::window::{Geometry, Plan};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Full,
    Player,
    Mini,
}

impl Mode {
    pub fn size(self) -> [f32; 2] {
        match self {
            Self::Full => [1100.0, 720.0],
            Self::Player => [480.0, 800.0],
            Self::Mini => [280.0, 280.0],
        }
    }
    pub fn min_size(self) -> [f32; 2] {
        match self {
            Self::Full => crate::MIN_SIZE,
            Self::Player => [380.0, 660.0],
            Self::Mini => [240.0, 240.0],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Windows {
    pub mode: Mode,
    pub geometry: HashMap<Mode, Geometry>,
    before_mini: Mode,
    last_player: Mode,
}
impl Default for Windows {
    fn default() -> Self {
        Self { mode: Mode::Full, geometry: HashMap::new(), before_mini: Mode::Full, last_player: Mode::Player }
    }
}
impl Windows {
    pub fn plan(&self) -> Plan<Mode> {
        let mode = self.mode;
        let saved = self.geometry.get(&mode);
        let min = mode.min_size();
        let mut size = saved.map_or(mode.size(), |g| [g.size[0].max(min[0]), g.size[1].max(min[1])]);
        if mode == Mode::Mini {
            size = [size[0].max(size[1]); 2];
        }
        let mut plan = Plan::framed(mode, size, min);
        plan.pos = saved.and_then(|g| g.pos);
        plan.maximized = mode == Mode::Full && saved.is_some_and(|g| g.maximized);
        if mode == Mode::Mini {
            plan.decorations = false;
            plan.controls = false;
            plan.on_top = true;
            plan.square = true;
        }
        plan
    }
    pub fn remember(&mut self, mut now: Geometry) {
        if now.maximized {
            let saved = self.geometry.entry(self.mode).or_insert(Geometry { pos: None, size: self.mode.size(), maximized: false });
            saved.maximized = true;
        } else {
            let min = self.mode.min_size();
            now.size = [now.size[0].max(min[0]), now.size[1].max(min[1])];
            self.geometry.insert(self.mode, now);
        }
    }
    pub fn switch(&mut self, mode: Mode, seen: Option<Geometry>) -> Option<Plan<Mode>> {
        if mode == self.mode {
            return None;
        }
        let from = self.mode;
        if let Some(g) = seen {
            self.remember(g);
        }
        if mode == Mode::Mini {
            self.before_mini = from;
        }
        if mode != Mode::Full {
            self.last_player = mode;
        }
        self.mode = mode;
        let mut plan = self.plan();
        if from != Mode::Full && mode != Mode::Full {
            plan.pos = seen.and_then(|g| g.pos).or(plan.pos);
        }
        Some(plan)
    }
    pub fn player(&self) -> Mode {
        if self.mode == Mode::Full { self.last_player } else { Mode::Full }
    }
    pub fn mini(&self) -> Mode {
        if self.mode == Mode::Mini { self.before_mini } else { Mode::Mini }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mini_returns_to_its_origin_and_player_returns_to_the_last_player() {
        let mut windows = Windows::default();
        let seen = Geometry { pos: Some([12.0, 20.0]), size: [1200.0, 800.0], maximized: false };
        windows.switch(Mode::Player, Some(seen));
        let mini = windows.switch(windows.mini(), None).unwrap();
        assert!(mini.on_top && mini.square && !mini.decorations);
        assert_eq!(windows.mini(), Mode::Player);
        windows.switch(windows.mini(), None);
        let full = windows.switch(windows.player(), None).unwrap();
        assert_eq!((full.size, full.pos), (seen.size, seen.pos));
        assert_eq!(windows.player(), Mode::Player);
    }
}
