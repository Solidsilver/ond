//! Output device picker and the signal-path line under the transport.

use crate::icons::{self, Icon};
use crate::playback::{BITRATES, NetworkProbe, PlaybackPrefs, QualityMode, RgMode, TranscodeFormat};
use crate::settings::{Paths, Settings};
use audio::{BitPerfectStatus, Command, DeviceInfo, EngineHandle, PlayState, PlaybackMode, SignalPath, Transcode};
use std::collections::HashMap;

#[derive(Default)]
pub struct Outputs {
    /// ReplayGain and streaming quality.
    prefs: PlaybackPrefs,
    /// Whether the server is on this network (for "transcode only when away").
    network: NetworkProbe,
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
        let prefs = settings.map(|s| s.playback.clone()).unwrap_or_default();
        engine.send(Command::DeviceModes(modes(&saved_modes)));
        engine.send(Command::SelectDevice(settings.and_then(|s| s.output_device.clone())));
        engine.send(Command::RefreshDevices);
        engine.send(Command::SetGain(prefs.gain_config()));
        Self { saved_modes, prefs, ..Self::default() }
    }

    /// Checks (in the background) whether `server` is on this network.
    pub fn connected(&mut self, server: &str) {
        self.network = NetworkProbe::start(server);
    }

    /// The transcode new tracks should be streamed with, if any.
    pub fn transcode(&self) -> Option<Transcode> {
        self.prefs.transcode(self.network.get())
    }

    fn save_prefs(&self, paths: &Paths) {
        let prefs = self.prefs.clone();
        Settings::update(paths, |s| s.playback = prefs);
    }

    /// ReplayGain and streaming quality, inside the output popup.
    fn playback_ui(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, paths: &Paths, bit_perfect: bool) {
        let before = self.prefs.clone();
        ui.strong("ReplayGain");
        ui.horizontal(|ui| {
            for m in RgMode::ALL {
                ui.selectable_value(&mut self.prefs.replay_gain, m, m.label()).on_hover_text(m.hint());
            }
        });
        if self.prefs.replay_gain != RgMode::Off {
            ui.add(egui::Slider::new(&mut self.prefs.preamp_db, -12.0..=12.0).step_by(0.5).suffix(" dB").text("Pre-amp"));
            ui.add(egui::Slider::new(&mut self.prefs.untagged_db, -12.0..=0.0).step_by(0.5).suffix(" dB").text("Untagged"))
                .on_hover_text("Gain for tracks without ReplayGain tags");
            if bit_perfect {
                ui.weak("Bypassed in bit-perfect mode");
            }
        }
        ui.separator();
        ui.strong("Streaming");
        for q in QualityMode::ALL {
            ui.selectable_value(&mut self.prefs.quality, q, q.label());
        }
        if self.prefs.quality != QualityMode::Original {
            ui.horizontal(|ui| {
                for f in TranscodeFormat::ALL {
                    ui.selectable_value(&mut self.prefs.format, f, f.id().to_ascii_uppercase());
                }
                ui.separator();
                for k in BITRATES {
                    ui.selectable_value(&mut self.prefs.kbps, k, k.to_string());
                }
                ui.weak("kbps");
            });
            let now = match self.transcode() {
                Some(t) => format!("{} · new tracks stream as {}", self.network.get().label(), t.label()),
                None => format!("{} · new tracks stream as original files", self.network.get().label()),
            };
            ui.weak(now);
        }
        if self.prefs != before {
            if self.prefs.gain_config() != before.gain_config() {
                engine.send(Command::SetGain(self.prefs.gain_config()));
            }
            self.save_prefs(paths);
        }
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

    /// The output chip: device name with a status dot. Its popup lists devices
    /// and holds the bit-perfect switch for the device in use.
    pub fn picker_ui(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, paths: &Paths, bit_perfect: &mut bool) {
        if let Some((uid, name)) = self.returned.clone() {
            if ui.small_button(format!("Switch back to {name}")).clicked() {
                self.choose(Some(uid), engine, paths);
            }
        } else if let Some(name) = &self.lost {
            ui.weak(format!("{name} disconnected"));
        }
        let current = self.devices.iter().find(|d| Some(&d.uid) == self.current.as_ref());
        let label = current.map_or("Output", |d| d.name.as_str()).to_owned();
        let dot = self.dot_color(ui);
        let t = crate::theme::Tokens::of(ui);
        let galley = ui.painter().layout_no_wrap(label.clone(), egui::FontId::proportional(12.0), t.text);
        let size = egui::vec2(galley.size().x + 30.0, 24.0);
        let (rect, chip) = ui.allocate_exact_size(size, egui::Sense::click());
        if ui.is_rect_visible(rect) {
            let fill = if chip.hovered() { t.hover } else { t.bg };
            ui.painter().rect(rect, 12.0, fill, egui::Stroke::new(1.0, t.hairline), egui::StrokeKind::Inside);
            ui.painter().galley(egui::pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0), galley, t.text);
            ui.painter().circle_filled(egui::pos2(rect.right() - 12.0, rect.center().y), 3.5, dot);
        }
        chip.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, true, format!("Output: {label}")));
        let chip = match &self.signal {
            Some(p) => chip.on_hover_text(hover_text(p)),
            None => chip,
        };
        egui::Popup::menu(&chip).show(|ui| {
            ui.set_min_width(280.0);
            if ui.checkbox(bit_perfect, "Bit-perfect").changed() {
                let mode = if *bit_perfect { PlaybackMode::BitPerfect } else { PlaybackMode::Normal };
                engine.send(Command::SetMode(mode));
                self.remember_mode(mode, engine, paths);
            }
            ui.weak(if *bit_perfect { "Volume is your DAC's or the system's in this mode" } else { "Volume, resampling and dither allowed" });
            ui.separator();
            self.playback_ui(ui, engine, paths, *bit_perfect);
            ui.separator();
            let follows_default = self.devices.iter().find(|d| d.is_default).map(|d| d.name.as_str()).unwrap_or("default");
            if ui.selectable_label(self.current.is_none(), format!("System default ({follows_default})")).clicked() {
                self.choose(None, engine, paths);
                ui.close();
            }
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
            Some(BitPerfectStatus::Verified) => crate::theme::Tokens::of(ui).ok,
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
        // A transcode names its own codec in the summary; the file's would be wrong.
        if let Some(codec) = codec.filter(|_| path.transcoded.is_none()) {
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
        BitPerfectStatus::Off => "Normal mode: volume, ReplayGain and resampling allowed".to_owned(),
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
