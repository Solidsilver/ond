//! Output device picker and the signal-path line under the transport.

use crate::icons::{self, Icon};
use crate::settings::{Paths, Settings};
use audio::{BitPerfectStatus, Command, DeviceInfo, EngineHandle, PlayState, PlaybackMode, SignalPath};
use std::collections::HashMap;

#[derive(Default)]
pub struct Outputs {
    /// Remembered modes per device UID, as saved ("normal" / "bit-perfect").
    saved_modes: HashMap<String, String>,
    devices: Vec<DeviceInfo>,
    current: Option<String>,
    signal: Option<SignalPath>,
    /// A device that went away and came back: (uid, name). Offered, not switched to.
    returned: Option<(String, String)>,
    /// Name of a device that just went away.
    lost: Option<String>,
}

impl Outputs {
    /// Restores the saved device and per-device modes into a fresh engine. The
    /// engine answers with the device's mode (`ModeChanged`) and the device list.
    pub fn restore(engine: &EngineHandle, settings: Option<&Settings>) -> Self {
        let saved_modes = settings.map(|s| s.device_modes.clone()).unwrap_or_default();
        engine.send(Command::DeviceModes(modes(&saved_modes)));
        engine.send(Command::SelectDevice(settings.and_then(|s| s.output_device.clone())));
        engine.send(Command::RefreshDevices);
        Self { saved_modes, ..Self::default() }
    }

    /// Takes the engine events this module cares about. Returns the mode the engine
    /// switched to by itself, if it did.
    pub fn handle(&mut self, ev: &audio::Event) -> Option<PlaybackMode> {
        match ev {
            audio::Event::SignalPath(p) => self.signal = Some(p.clone()),
            audio::Event::Devices { devices, current } => {
                self.devices = devices.clone();
                self.current = current.clone();
            }
            audio::Event::DeviceLost { name } => {
                self.lost = Some(name.clone());
                self.returned = None;
            }
            audio::Event::DeviceReturned { uid, name } => {
                self.returned = Some((uid.clone(), name.clone()));
                self.lost = None;
            }
            audio::Event::ModeChanged { mode, .. } => return Some(*mode),
            _ => {}
        }
        None
    }

    /// Remembers `mode` for the device in use (in settings and in the engine).
    pub fn remember_mode(&mut self, mode: PlaybackMode, engine: &EngineHandle, paths: &Paths) {
        let Some(uid) = self.current.clone() else { return };
        self.saved_modes.insert(uid.clone(), mode_name(mode).into());
        engine.send(Command::DeviceModes(modes(&self.saved_modes)));
        if let Some(mut s) = Settings::load(paths) {
            s.device_modes.insert(uid, mode_name(mode).into());
            if let Err(e) = s.save(paths) {
                tracing::warn!("saving device mode: {e}");
            }
        }
    }

    fn choose(&mut self, uid: Option<String>, engine: &EngineHandle, paths: &Paths) {
        self.returned = None;
        self.lost = None;
        engine.send(Command::SelectDevice(uid.clone()));
        if let Some(mut s) = Settings::load(paths) {
            s.output_device = uid;
            if let Err(e) = s.save(paths) {
                tracing::warn!("saving output device: {e}");
            }
        }
    }

    /// A small button with the device name and a status dot; the popup lists devices.
    pub fn picker_ui(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, paths: &Paths) {
        if let Some((uid, name)) = self.returned.clone() {
            if ui.small_button(format!("Switch back to {name}")).clicked() {
                self.choose(Some(uid), engine, paths);
            }
        } else if let Some(name) = &self.lost {
            ui.weak(format!("{name} disconnected"));
        }
        let current = self.devices.iter().find(|d| Some(&d.uid) == self.current.as_ref());
        let label = current.map_or("Output", |d| d.name.as_str());
        let dot = self.dot_color(ui);
        let button = ui.add(egui::Button::new(label).small());
        // The status dot sits just inside the button's left edge (painted: no glyph needed).
        let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 3.5, dot);
        let button = match &self.signal {
            Some(p) => button.on_hover_text(hover_text(p)),
            None => button,
        };
        egui::Popup::menu(&button).show(|ui| {
            ui.set_min_width(260.0);
            let follows_default = self.devices.iter().find(|d| d.is_default).map(|d| d.name.as_str()).unwrap_or("default");
            if ui.button(format!("System default ({follows_default})")).clicked() {
                self.choose(None, engine, paths);
            }
            ui.separator();
            for d in self.devices.clone() {
                let mode = self.saved_modes.get(&d.uid).map(String::as_str).unwrap_or_else(|| mode_name(d.default_mode()));
                let selected = Some(&d.uid) == self.current.as_ref();
                let resp = ui.selectable_label(selected, &d.name);
                ui.weak(format!("{} · {mode}", d.capability_line()));
                if resp.clicked() {
                    self.choose(Some(d.uid.clone()), engine, paths);
                    ui.close();
                }
            }
        });
    }

    fn dot_color(&self, ui: &egui::Ui) -> egui::Color32 {
        let v = ui.visuals();
        match self.signal.as_ref().map(|p| &p.bit_perfect) {
            Some(BitPerfectStatus::Verified) => egui::Color32::from_rgb(0x4c, 0xaf, 0x50),
            Some(BitPerfectStatus::Unavailable { .. }) => v.warn_fg_color,
            Some(BitPerfectStatus::Requested { .. }) => v.text_color(),
            _ => v.weak_text_color(),
        }
    }

    /// `FLAC · 24/96 → 16/48 · resampled, dithered · device limit`.
    pub fn signal_ui(&self, ui: &mut egui::Ui, status: &audio::Status, codec: Option<&str>) {
        let Some(path) = &self.signal else { return };
        if status.state() == PlayState::Stopped {
            return;
        }
        ui.spacing_mut().item_spacing.x = 6.0;
        if let Some(codec) = codec {
            ui.weak(codec);
        }
        let color = match &path.bit_perfect {
            BitPerfectStatus::Verified => {
                icons::inline(ui, Icon::Check, ui.visuals().strong_text_color());
                ui.visuals().text_color()
            }
            BitPerfectStatus::Unavailable { .. } => ui.visuals().warn_fg_color,
            _ => ui.visuals().weak_text_color(),
        };
        // The default font has no arrow glyph: draw it between the parts instead.
        let summary = path.summary();
        for (i, part) in summary.split(" → ").enumerate() {
            if i > 0 {
                icons::inline(ui, Icon::ArrowRight, color);
            }
            ui.colored_label(color, part).on_hover_text(hover_text(path));
        }
        let underruns = status.underruns.load(std::sync::atomic::Ordering::Relaxed);
        if underruns > 0 {
            ui.weak(format!("· {underruns} underruns"));
        }
    }
}

fn hover_text(p: &SignalPath) -> String {
    let verdict = match &p.bit_perfect {
        BitPerfectStatus::Off => "Normal mode: volume and resampling allowed".to_owned(),
        BitPerfectStatus::Verified => "Bit-perfect: exclusive access, matching rate, untouched samples".to_owned(),
        BitPerfectStatus::Requested { reason } => format!("Bit-perfect requested, not verified: {reason}"),
        BitPerfectStatus::Unavailable { reason } => format!("Not bit-perfect: {reason}"),
    };
    format!("{}\n{verdict}", p.output)
}

fn mode_name(mode: PlaybackMode) -> &'static str {
    match mode {
        PlaybackMode::Normal => "normal",
        PlaybackMode::BitPerfect => "bit-perfect",
    }
}

fn modes(saved: &HashMap<String, String>) -> HashMap<String, PlaybackMode> {
    saved
        .iter()
        .map(|(uid, m)| (uid.clone(), if m == "bit-perfect" { PlaybackMode::BitPerfect } else { PlaybackMode::Normal }))
        .collect()
}
