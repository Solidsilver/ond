//! The device's own volume, while bit-perfect holds a device that has one:
//! the toolbar slider's span and maths, the dB text, and sending a level to
//! the engine (throttled while dragging) and settling on what it reports back.

use super::Outputs;
use audio::{Command, DeviceVolume, EngineHandle};
use std::time::{Duration, Instant};

/// The toolbar slider covers the top this many dB of a device's volume (most
/// of a 100 dB range is too quiet to use); the panel's covers all of it.
const SLIDER_SPAN_DB: f32 = 60.0;
/// Dragging sends the device volume at most this often (the last value always goes).
const SEND_EVERY: Duration = Duration::from_millis(40);
/// After we set the device volume, reports of other levels are earlier ones
/// still arriving, until the device reports ours or this long has passed.
const SETTLE: Duration = Duration::from_millis(1500);
/// A report this close to the level we set counts as ours (devices round).
const SETTLE_DB: f32 = 0.3;

impl Outputs {
    /// The device's own volume, when bit-perfect holds a device that has one.
    pub fn device_volume(&self) -> Option<DeviceVolume> {
        self.device_volume
    }

    /// Sets the device's own volume (clamped to its range): at once, or
    /// within [`SEND_EVERY`] while it's being dragged.
    pub fn set_device_volume(&mut self, db: f32, engine: &EngineHandle) {
        // Turning it up or down while muted means "let me hear it", as in macOS.
        if self.device_volume.is_some_and(|v| v.muted == Some(true)) {
            self.set_device_mute(false, engine);
        }
        let Some(v) = self.device_volume.as_mut() else { return };
        v.db = db.clamp(v.min_db, v.max_db);
        self.unsent = Some(v.db);
        self.flush_device_volume(engine);
    }

    /// Sends the level waiting to go, when it's time.
    fn flush_device_volume(&mut self, engine: &EngineHandle) {
        let Some(db) = self.unsent else { return };
        if self.sent_at.is_some_and(|t| t.elapsed() < SEND_EVERY) {
            return;
        }
        engine.send(Command::SetDeviceVolume(db));
        self.unsent = None;
        self.sent_at = Some(Instant::now());
        self.awaiting = Some((db, Instant::now()));
    }

    /// Once a frame, whether or not the control is drawn (the visualizer or a
    /// window mode may hide it): sends a level still waiting, and keeps the
    /// frames coming until it's gone.
    pub fn tick(&mut self, ctx: &egui::Context, engine: &EngineHandle) {
        self.flush_device_volume(engine);
        if self.unsent.is_some() {
            ctx.request_repaint_after(SEND_EVERY);
        }
    }

    pub fn set_device_mute(&mut self, muted: bool, engine: &EngineHandle) {
        let Some(v) = self.device_volume.as_mut().filter(|v| v.muted.is_some()) else { return };
        v.muted = Some(muted);
        engine.send(Command::SetDeviceMute(muted));
    }

    /// The engine's word on the device's own volume (`None`: no device held,
    /// or it has no volume to set). Reports from before our last change are
    /// still coming in: until the device reports ours (within [`SETTLE_DB`])
    /// or [`SETTLE`] has passed, other levels don't move the slider.
    pub(super) fn device_volume_reported(&mut self, v: Option<DeviceVolume>) {
        let ours = match (v, self.awaiting) {
            (Some(v), Some((db, at))) if at.elapsed() < SETTLE && (v.db - db).abs() > SETTLE_DB => Some(db),
            _ => None,
        };
        if ours.is_none() && self.unsent.is_none() {
            self.awaiting = None;
        }
        self.device_volume = match (v, ours.or(self.unsent)) {
            (Some(v), Some(db)) => Some(DeviceVolume { db, ..v }),
            _ => v,
        };
    }
}

/// The toolbar slider's bottom: the top [`SLIDER_SPAN_DB`] of the range.
fn slider_floor(v: &DeviceVolume) -> f32 {
    (v.max_db - SLIDER_SPAN_DB).max(v.min_db)
}

/// Where the device's level sits on the toolbar slider (0 = its minimum).
pub(super) fn slider_pos(v: &DeviceVolume) -> f32 {
    let floor = slider_floor(v);
    if v.db <= floor { 0.0 } else { ((v.db - floor) / (v.max_db - floor).max(0.01)).clamp(0.0, 1.0) }
}

/// The level for a toolbar slider position: the very bottom is the device's minimum.
pub(super) fn slider_db(v: &DeviceVolume, pos: f32) -> f32 {
    if pos <= 0.0 { v.min_db } else { slider_floor(v) + pos * (v.max_db - slider_floor(v)) }
}

/// "−24.0 dB", or "Muted".
pub(super) fn db_text(v: &DeviceVolume) -> String {
    if v.muted == Some(true) {
        return "Muted".into();
    }
    let db = if v.db.abs() < 0.05 { 0.0 } else { v.db };
    format!("{db:.1} dB").replace('-', "−")
}

pub(super) fn device_volume_hover(name: &str, v: &DeviceVolume) -> String {
    format!(
        "{name}'s own volume · {}\nOnd's samples reach it untouched; the device sets the level. The top is {:.0} dB, full level.",
        db_text(v),
        v.max_db
    )
}
