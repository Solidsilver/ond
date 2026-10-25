//! Kinetic trackpad scrolling where the system doesn't provide it.
//!
//! macOS keeps a two-finger scroll going after the fingers lift (its momentum
//! phase) and egui scrolls along with it. Wayland only says when the fingers
//! lift (`axis_stop`) and leaves the coasting to each app, as GTK and Firefox
//! do. This adds it: the finger speed at lift-off carries on and decays
//! exponentially, sent to egui as momentum-phase wheel events, so scroll areas
//! treat it exactly like macOS's (including the single bounce at an edge).
//!
//! It only touches precise (point) wheel deltas from an unknown source. On
//! macOS egui-winit labels every wheel event, so nothing changes there; mouse
//! wheels send lines and are left alone everywhere.

use egui::{Event, Modifiers, MouseWheelSource, MouseWheelUnit, TouchPhase, Vec2};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How fast a coast slows: velocity × e^(−FRICTION·t), as GTK does.
const FRICTION: f32 = 4.0;
/// Below this speed (points per second) a coast stops.
const MIN_SPEED: f32 = 40.0;
/// A flick faster than this is capped, so one stray sample can't fling a long list.
const MAX_SPEED: f32 = 6_000.0;
/// The lift-off speed is the average over this much of the gesture's end.
const WINDOW: Duration = Duration::from_millis(80);
/// Fingers that rested this long before lifting start no coast.
const REST: Duration = Duration::from_millis(60);
/// Roughly one frame: the time the oldest sample in the window stands for.
const FRAME: Duration = Duration::from_millis(16);

#[derive(Default)]
pub struct Momentum {
    /// Finger deltas while the fingers are down, newest last.
    samples: VecDeque<(Instant, Vec2)>,
    coast: Option<Coast>,
}

struct Coast {
    velocity: Vec2,
    last: Instant,
    started: bool,
    modifiers: Modifiers,
}

impl Momentum {
    /// Labels trackpad scrolls in `events` and appends this frame's share of a
    /// coast. Returns whether a coast is still running (keep repainting).
    pub fn process(&mut self, events: &mut Vec<Event>, now: Instant) -> bool {
        let mut out = Vec::with_capacity(events.len() + 1);
        for event in events.drain(..) {
            match event {
                Event::MouseWheel { unit: MouseWheelUnit::Point, delta, phase, source: MouseWheelSource::Unknown, modifiers } => {
                    match phase {
                        TouchPhase::Start => {
                            self.stop(&mut out);
                            self.samples.clear();
                            self.sample(now, delta);
                        }
                        TouchPhase::Move => self.sample(now, delta),
                        TouchPhase::End => {
                            self.sample(now, delta);
                            self.lift(now, modifiers);
                        }
                        TouchPhase::Cancel => self.samples.clear(),
                    }
                    out.push(Event::MouseWheel { unit: MouseWheelUnit::Point, delta, phase, source: MouseWheelSource::Trackpad, modifiers });
                }
                event => {
                    // Any other scroll, a click or a touch catches the content.
                    if matches!(event, Event::MouseWheel { .. } | Event::PointerButton { pressed: true, .. } | Event::Touch { phase: TouchPhase::Start, .. }) {
                        self.stop(&mut out);
                    }
                    out.push(event);
                }
            }
        }
        self.step(now, &mut out);
        *events = out;
        self.coast.is_some()
    }

    fn sample(&mut self, now: Instant, delta: Vec2) {
        if delta != Vec2::ZERO {
            self.samples.push_back((now, delta));
        }
        while self.samples.front().is_some_and(|(t, _)| now.duration_since(*t) > WINDOW) {
            self.samples.pop_front();
        }
    }

    /// The fingers lifted: coast at their recent speed, if they were moving.
    fn lift(&mut self, now: Instant, modifiers: Modifiers) {
        let samples = std::mem::take(&mut self.samples);
        let (Some((first, _)), Some((newest, _))) = (samples.front(), samples.back()) else { return };
        if now.duration_since(*newest) > REST {
            return;
        }
        let span = (newest.duration_since(*first) + FRAME).as_secs_f32();
        let moved = samples.iter().fold(Vec2::ZERO, |sum, (_, d)| sum + *d);
        let mut velocity = moved / span;
        if velocity.length() > MAX_SPEED {
            velocity = velocity.normalized() * MAX_SPEED;
        }
        if velocity.length() >= MIN_SPEED {
            self.coast = Some(Coast { velocity, last: now, started: false, modifiers });
        }
    }

    /// Advances a running coast to `now`.
    fn step(&mut self, now: Instant, out: &mut Vec<Event>) {
        let Some(c) = &mut self.coast else { return };
        // A long gap (the app was hidden, a slow frame) shouldn't jump the content.
        let dt = now.duration_since(c.last).as_secs_f32().min(0.1);
        c.last = now;
        let decay = (-FRICTION * dt).exp();
        // The distance covered while the speed decays over `dt`.
        let delta = c.velocity * (1.0 - decay) / FRICTION;
        c.velocity *= decay;
        let phase = if c.started { TouchPhase::Move } else { TouchPhase::Start };
        c.started = true;
        out.push(momentum(delta, phase, c.modifiers));
        if c.velocity.length() < MIN_SPEED {
            self.stop(out);
        }
    }

    /// Ends a running coast (egui expects each momentum phase to end).
    fn stop(&mut self, out: &mut Vec<Event>) {
        if let Some(c) = self.coast.take()
            && c.started
        {
            out.push(momentum(Vec2::ZERO, TouchPhase::End, c.modifiers));
        }
    }
}

fn momentum(delta: Vec2, phase: TouchPhase, modifiers: Modifiers) -> Event {
    Event::MouseWheel { unit: MouseWheelUnit::Point, delta, phase, source: MouseWheelSource::Momentum, modifiers }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel(delta: f32, phase: TouchPhase, source: MouseWheelSource, unit: MouseWheelUnit) -> Event {
        Event::MouseWheel { unit, delta: Vec2::new(0.0, delta), phase, source, modifiers: Modifiers::NONE }
    }

    fn finger(delta: f32, phase: TouchPhase) -> Event {
        wheel(delta, phase, MouseWheelSource::Unknown, MouseWheelUnit::Point)
    }

    /// (delta, phase, source) of each wheel event.
    fn wheels(events: &[Event]) -> Vec<(f32, TouchPhase, MouseWheelSource)> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::MouseWheel { delta, phase, source, .. } => Some((delta.y, *phase, *source)),
                _ => None,
            })
            .collect()
    }

    /// A two-finger swipe of `per_frame` points a frame for `frames` frames,
    /// then lift-off. Returns the time and the lift-off frame's events.
    fn swipe_events(m: &mut Momentum, t0: Instant, per_frame: f32, frames: u32) -> (Instant, Vec<Event>) {
        let mut t = t0;
        let mut first = vec![finger(per_frame, TouchPhase::Start)];
        m.process(&mut first, t);
        for _ in 1..frames {
            t += FRAME;
            m.process(&mut vec![finger(per_frame, TouchPhase::Move)], t);
        }
        t += FRAME;
        let mut end = vec![finger(0.0, TouchPhase::End)];
        m.process(&mut end, t);
        (t, end)
    }

    fn swipe(m: &mut Momentum, t0: Instant, per_frame: f32, frames: u32) -> Instant {
        swipe_events(m, t0, per_frame, frames).0
    }

    #[test]
    fn a_flick_coasts_and_slows_to_a_stop() {
        let mut m = Momentum::default();
        let (mut t, lift) = swipe_events(&mut m, Instant::now(), 20.0, 6);
        // The coast starts on the lift-off frame.
        let mut coasted: Vec<_> = wheels(&lift).into_iter().filter(|w| w.2 == MouseWheelSource::Momentum).collect();
        for _ in 0..200 {
            t += FRAME;
            let mut events = Vec::new();
            let running = m.process(&mut events, t);
            coasted.extend(wheels(&events));
            if !running {
                break;
            }
        }
        assert_eq!(coasted.first().map(|w| (w.1, w.2)), Some((TouchPhase::Start, MouseWheelSource::Momentum)));
        assert_eq!(coasted.last().map(|w| (w.0, w.1)), Some((0.0, TouchPhase::End)), "the phase ends");
        let moves: Vec<f32> = coasted.iter().filter(|w| w.1 == TouchPhase::Move).map(|w| w.0).collect();
        assert!(moves.len() > 10, "keeps going for a while: {}", moves.len());
        assert!(moves.windows(2).all(|w| w[1] < w[0] && w[1] > 0.0), "same direction, slowing: {moves:?}");
        // About a speed of 20 pt per 16 ms, carried for 1/FRICTION seconds.
        let total: f32 = coasted.iter().map(|w| w.0).sum();
        assert!((200.0..400.0).contains(&total), "coasted {total}");
    }

    #[test]
    fn finger_scrolls_pass_through_labelled_as_trackpad() {
        let mut m = Momentum::default();
        let mut events = vec![finger(5.0, TouchPhase::Start)];
        m.process(&mut events, Instant::now());
        assert_eq!(wheels(&events), [(5.0, TouchPhase::Start, MouseWheelSource::Trackpad)]);
    }

    #[test]
    fn resting_fingers_or_a_slow_drag_start_no_coast() {
        let mut m = Momentum::default();
        let t = Instant::now();
        let mut start = vec![finger(20.0, TouchPhase::Start)];
        m.process(&mut start, t);
        assert!(!m.process(&mut vec![finger(0.0, TouchPhase::End)], t + REST + FRAME), "rested before lifting");
        assert!(!{
            let t = swipe(&mut m, t + Duration::from_secs(1), 0.2, 10);
            m.process(&mut Vec::new(), t + FRAME)
        });
    }

    #[test]
    fn touching_down_again_catches_the_content() {
        let mut m = Momentum::default();
        let mut t = swipe(&mut m, Instant::now(), 30.0, 5);
        t += FRAME;
        assert!(m.process(&mut Vec::new(), t));
        t += FRAME;
        let mut events = vec![finger(1.0, TouchPhase::Start)];
        assert!(!m.process(&mut events, t));
        let w = wheels(&events);
        assert_eq!((w[0].1, w[0].2), (TouchPhase::End, MouseWheelSource::Momentum), "the coast ends first");
        assert_eq!(w[1].2, MouseWheelSource::Trackpad);
    }

    #[test]
    fn mouse_wheels_and_labelled_events_are_left_alone() {
        let mut m = Momentum::default();
        let line = wheel(1.0, TouchPhase::Move, MouseWheelSource::Unknown, MouseWheelUnit::Line);
        let mac = wheel(3.0, TouchPhase::End, MouseWheelSource::Trackpad, MouseWheelUnit::Point);
        let mut events = vec![line.clone(), mac.clone()];
        assert!(!m.process(&mut events, Instant::now()));
        assert_eq!(events, [line, mac]);
    }
}
