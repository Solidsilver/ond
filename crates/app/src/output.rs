//! Output device picker and the signal-path line under the transport.

use crate::icons::{self, Icon};
use crate::playback::{BITRATES, NetworkProbe, PlaybackPrefs, QualityMode, RgMode, TranscodeFormat};
use crate::settings::{Paths, Settings};
use audio::{BitPerfectStatus, Command, DeviceInfo, EngineHandle, PlayState, PlaybackMode, SignalPath, Transcode};
use crate::theme::Tokens;
use std::collections::HashMap;

const POPUP_W: f32 = 300.0;

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
        let t = Tokens::of(ui);
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
        egui::Popup::menu(&chip)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .width(POPUP_W)
            .show(|ui| self.popup_ui(ui, engine, paths, bit_perfect));
    }

    /// What's playing through what, the devices, and the mode for the one in use.
    fn popup_ui(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, paths: &Paths, bit_perfect: &mut bool) {
        let t = Tokens::of(ui);
        ui.set_width(POPUP_W);
        ui.spacing_mut().item_spacing.y = 0.0;
        if let Some(path) = self.signal.clone() {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                summary_ui(ui, &path, 12.0, true);
            });
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(&path.output).size(11.0).color(t.muted));
            });
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
        }
        let follows_default = self.devices.iter().find(|d| d.is_default).map(|d| d.name.clone()).unwrap_or_else(|| "default".into());
        if device_row(ui, &format!("System default ({follows_default})"), None, self.current.is_none()).clicked() {
            self.choose(None, engine, paths);
            ui.close();
        }
        for d in self.devices.clone() {
            let mode = self.saved_modes.get(&d.uid).map(String::as_str).unwrap_or_else(|| mode_name(d.default_mode()));
            let selected = Some(&d.uid) == self.current.as_ref();
            let caps = d.capability_line();
            let sub = if caps.is_empty() { mode.to_owned() } else { format!("{caps} · {mode}") };
            if device_row(ui, &d.name, Some(&sub), selected).clicked() {
                self.choose(Some(d.uid.clone()), engine, paths);
                ui.close();
            }
        }
        ui.add_space(4.0);
        ui.separator();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            if let Some(i) = crate::app::segmented_wide(ui, &["Normal", "Bit-perfect"], usize::from(*bit_perfect), POPUP_W - 16.0) {
                let want = i == 1;
                if want != *bit_perfect {
                    *bit_perfect = want;
                    let mode = if want { PlaybackMode::BitPerfect } else { PlaybackMode::Normal };
                    engine.send(Command::SetMode(mode));
                    self.remember_mode(mode, engine, paths);
                }
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            let why = if *bit_perfect {
                "Samples reach the device untouched. Use your DAC's or the system's volume."
            } else {
                "Volume, ReplayGain and resampling allowed."
            };
            ui.add(egui::Label::new(egui::RichText::new(why).size(11.0).color(t.muted)).wrap());
        });
        ui.add_space(8.0);
        ui.separator();
        let bp = *bit_perfect;
        egui::CollapsingHeader::new(egui::RichText::new("ReplayGain & streaming").size(12.0))
            .id_salt("playback-prefs")
            .default_open(false)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                self.playback_ui(ui, engine, paths, bp);
            });
        ui.add_space(4.0);
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

    /// `FLAC  24/96 → 16/48 · resampled, dithered · device limit`, quiet: muted
    /// text with only the verdict coloured.
    pub fn signal_ui(&self, ui: &mut egui::Ui, status: &audio::Status, codec: Option<&str>) {
        let Some(path) = &self.signal else { return };
        if status.state() == PlayState::Stopped {
            return;
        }
        ui.spacing_mut().item_spacing.x = 5.0;
        // A transcode names its own codec in the summary; the file's would be wrong.
        if let Some(codec) = codec.filter(|_| path.transcoded.is_none()) {
            ui.weak(codec);
        }
        summary_ui(ui, path, 11.0, false).on_hover_text(hover_text(path));
        let underruns = status.underruns.load(std::sync::atomic::Ordering::Relaxed);
        if underruns > 0 {
            ui.weak(format!("· {underruns} underruns"));
        }
    }
}

/// The signal path summary: formats and processing in muted text, the verdict
/// (the last part) coloured: green with a check when verified, the warning
/// colour when bit-perfect isn't possible. `strong` uses the text colour for
/// the formats (the picker's header).
fn summary_ui(ui: &mut egui::Ui, path: &SignalPath, size: f32, strong: bool) -> egui::Response {
    let t = Tokens::of(ui);
    let base = if strong { t.text } else { t.muted };
    let summary = path.summary();
    let mut parts: Vec<&str> = summary.split(" · ").collect();
    let verdict = match &path.bit_perfect {
        BitPerfectStatus::Off => None,
        _ if parts.len() > 1 => parts.pop(),
        _ => None,
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (k, part) in parts.iter().enumerate() {
            if k > 0 {
                ui.label(egui::RichText::new("·").size(size).color(t.muted));
            }
            // The default font has no arrow glyph: draw it between the formats.
            for (i, f) in part.split(" → ").enumerate() {
                if i > 0 {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(size * 0.9, size), egui::Sense::hover());
                    icons::paint(ui.painter(), r, Icon::ArrowRight, t.muted);
                }
                ui.label(egui::RichText::new(f).size(size).color(if k == 0 { base } else { t.muted }));
            }
        }
        if let Some(v) = verdict {
            ui.label(egui::RichText::new("·").size(size).color(t.muted));
            let color = match &path.bit_perfect {
                BitPerfectStatus::Verified => {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(size * 0.9, size), egui::Sense::hover());
                    icons::paint(ui.painter(), r, Icon::Check, t.ok);
                    t.ok
                }
                BitPerfectStatus::Unavailable { .. } => t.warn,
                _ => t.muted,
            };
            ui.label(egui::RichText::new(v).size(size).color(color));
        }
    })
    .response
}

/// One row of the device list: a check for the device in use, its name, and
/// a muted capability line under it. The whole row is the target.
fn device_row(ui: &mut egui::Ui, name: &str, sub: Option<&str>, selected: bool) -> egui::Response {
    let t = Tokens::of(ui);
    let h = if sub.is_some() { 40.0 } else { 30.0 };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        if resp.hovered() {
            ui.painter().rect_filled(rect, 6.0, t.hover);
        }
        if selected {
            let c = egui::Rect::from_center_size(egui::pos2(rect.left() + 15.0, rect.top() + 15.0), egui::vec2(10.0, 10.0));
            icons::paint(ui.painter(), c, Icon::Check, t.accent);
        }
        let x = rect.left() + 30.0;
        let w = rect.right() - x - 8.0;
        let font = if selected { egui::FontId::new(13.0, crate::theme::semibold()) } else { egui::FontId::proportional(13.0) };
        let g = crate::app::elide_text(ui, name, font, t.text, w);
        ui.painter().galley(egui::pos2(x, rect.top() + 15.0 - g.size().y / 2.0), g, t.text);
        if let Some(sub) = sub {
            let g = crate::app::elide_text(ui, sub, egui::FontId::proportional(11.0), t.muted, w);
            ui.painter().galley(egui::pos2(x, rect.top() + 23.0), g, t.muted);
        }
    }
    let label = name.to_owned();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, selected, &label));
    resp
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
