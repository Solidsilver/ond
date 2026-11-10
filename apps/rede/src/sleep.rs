//! A sleep deadline is wall time, or an anchored chapter end in book time.
//! The latter is converted using the current speed on each engine-clock tick,
//! so a chapter can span files and a speed change never moves its endpoint.

use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Choice {
    Minutes(u32),
    Chapter,
}

pub struct Timer {
    target: Target,
}
enum Target {
    Time(Instant),
    Chapter(f64),
}

impl Timer {
    pub fn new(choice: Choice, chapter_end: f64, now: Instant) -> Self {
        let target = match choice {
            Choice::Minutes(minutes) => Target::Time(now + Duration::from_secs(u64::from(minutes.clamp(5, 120)) * 60)),
            Choice::Chapter => Target::Chapter(chapter_end),
        };
        Self { target }
    }

    pub fn remaining(&self, position: f64, speed: f32, now: Instant) -> f64 {
        match self.target {
            Target::Time(end) => end.saturating_duration_since(now).as_secs_f64(),
            Target::Chapter(end) => ((end - position) / f64::from(crate::timeline::normalize_speed(speed))).max(0.0),
        }
    }

    /// Whether it waits for the chapter to end, rather than a clock.
    pub fn at_chapter_end(&self) -> bool {
        matches!(self.target, Target::Chapter(_))
    }

    /// The book time it waits for, when it waits for a chapter's end.
    pub fn chapter_end(&self) -> Option<f64> {
        match self.target {
            Target::Chapter(end) => Some(end),
            Target::Time(_) => None,
        }
    }

    /// Waits for a later chapter's end, `end` in book time, instead. A timer
    /// on the clock stays as it is.
    pub fn wait_for_chapter(&mut self, end: f64) {
        if let Target::Chapter(at) = &mut self.target {
            *at = end;
        }
    }

    pub fn extend(&mut self, position: f64, speed: f32, now: Instant) {
        self.target = Target::Time(now + Duration::from_secs_f64(self.remaining(position, speed, now) + 600.0));
    }
}
