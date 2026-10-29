//! The output: which device plays and in which mode, the playback
//! preferences in use (see `playback` for what they are), the output control
//! (the device's icon, the volume, and a panel with the level, the devices and
//! their modes), and the compact signal-path readout in the now-playing display.

use crate::icons::{self, Icon};
use crate::playback::{NetworkProbe, PlaybackPrefs};
use crate::prefs::Prefs;
use crate::settings::DeviceMode;
use crate::settings::DbReadout;
use audio::{BitPerfectStatus, Command, DeviceInfo, DeviceKind, DeviceVolume, EngineHandle, PlayState, PlaybackMode, SignalPath, Transcode};
use crate::theme::Tokens;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const POPUP_W: f32 = 320.0;
/// The device icon with its ▾.
const ICON_W: f32 = 40.0;
/// Between the slider, the dB readout and the icon.
const GAP: f32 = 6.0;
/// A device in the panel's list.
const ROW_H: f32 = 32.0;
/// The list shows about three and a half devices before it scrolls.
const LIST_H: f32 = ROW_H * 3.5;
/// The group software devices go in, after the rest.
pub const OTHER: &str = "Other";
/// The group hardware goes in, first (it has no heading in the menu).
pub const DEVICES: &str = "Devices";
const BIT_PERFECT_HELP: &str = "Bit-perfect sends samples untouched and keeps other apps off the device while Ond plays. The volume then sets the device's own level, when it has one.";
/// The toolbar slider covers the top this many dB of a device's volume (most
/// of a 100 dB range is too quiet to use); the panel's covers all of it.
const SLIDER_SPAN_DB: f32 = 60.0;
/// The dB field's step: ↑↓, and a scroll notch.
const STEP_DB: f32 = 0.5;
/// With ⇧.
const FINE_STEP_DB: f32 = 0.1;
/// Scrolling this far over the dB field is one step.
const SCROLL_NOTCH: f32 = 24.0;
/// Room beside the slider for "−24.0 dB".
const DB_W: f32 = 52.0;
/// Dragging sends the device volume at most this often (the last value always goes).
const SEND_EVERY: Duration = Duration::from_millis(40);
/// After we set the device volume, reports of other levels are earlier ones
/// still arriving, until the device reports ours or this long has passed.
const SETTLE: Duration = Duration::from_millis(1500);

pub struct Outputs {
    /// ReplayGain, streaming quality, the device and per-device modes live here.
    prefs: Prefs,
    /// Whether the server is on this network (for "transcode only when away").
    network: NetworkProbe,
    devices: Vec<DeviceInfo>,
    current: Option<String>,
    signal: Option<SignalPath>,
    /// "Playback settings…" was clicked in the popup this frame.
    wants_settings: bool,
    /// Something else (the readout) asked for the picker to open.
    open_requested: bool,
    /// The device's own volume, while bit-perfect holds a device that has one.
    device_volume: Option<DeviceVolume>,
    /// The level we last asked for and when: until the device reports it
    /// (see [`SETTLE`]), other reports don't move the slider.
    awaiting: Option<(f32, Instant)>,
    /// A level not yet sent (see [`SEND_EVERY`]), and when the last one was.
    unsent: Option<f32>,
    sent_at: Option<Instant>,
    /// The slider was hovered or dragged last frame (for the dB readout).
    adjusting: bool,
    /// Devices whose settings are open in the panel's list, by UID.
    expanded: std::collections::HashSet<String>,
    /// Names of the devices seen, by UID (to name one that's unplugged).
    names: HashMap<String, String>,
    /// The dB field, while it has the keyboard (its arrows step 0.5 dB, not the drag speed).
    db_field: Option<egui::Id>,
    /// Scrolling over the dB field, not yet a whole step.
    scroll: f32,
}

/// What the output control asks of the app this frame.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct ControlOut {
    /// "Playback settings…" was chosen.
    pub settings: bool,
    /// Ond's own volume (Normal mode), 0..=1.
    pub volume: Option<f32>,
}

/// The few characters of signal path shown in the now-playing display:
/// `24/96 → 16/48` plus a dot for the bit-perfect verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct Readout {
    pub from: String,
    /// What reaches the device, when that differs from the file.
    pub to: Option<String>,
    pub verdict: Verdict,
    /// The full line and verdict, for the tooltip.
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Normal mode: nothing to report.
    None,
    /// Bit-perfect and verified: a green dot.
    Verified,
    /// Bit-perfect asked for but not confirmed: a hollow ring.
    Requested,
    /// Bit-perfect impossible (device limit, transcode, downmix): the warning colour.
    Unavailable,
}

impl Outputs {
    /// Restores the saved device and per-device modes into a fresh engine. The
    /// engine answers with the device's mode (`ModeChanged`) and the device list.
    pub fn restore(engine: &EngineHandle, prefs: Prefs) -> Self {
        let (modes, device, playback, match_rate) =
            prefs.get(|s| (engine_modes(s), s.output_device.clone(), s.playback.clone(), s.output_match_rate));
        engine.send(Command::DeviceModes(modes));
        engine.send(Command::MatchRate(match_rate));
        engine.send(Command::SelectDevice(device));
        engine.send(Command::RefreshDevices);
        engine.send(Command::SetGain(playback.gain_config()));
        Self {
            prefs,
            network: NetworkProbe::default(),
            devices: Vec::new(),
            current: None,
            signal: None,
            wants_settings: false,
            open_requested: false,
            device_volume: None,
            awaiting: None,
            unsent: None,
            sent_at: None,
            adjusting: false,
            expanded: Default::default(),
            names: HashMap::new(),
            db_field: None,
            scroll: 0.0,
        }
    }

    /// Ond plays wherever the system does (always shared), rather than on a chosen device.
    pub fn following_system(&self) -> bool {
        self.prefs.get(|s| s.output_device.is_none())
    }

    /// The device chosen, while it's unplugged: (its name, the one playing instead).
    fn waiting_for(&self) -> Option<(String, Option<String>)> {
        let uid = self.prefs.get(|s| s.output_device.clone())?;
        if self.current.as_ref() == Some(&uid) {
            return None;
        }
        let name = self.names.get(&uid).cloned().unwrap_or_else(|| "your device".into());
        Some((name, self.in_use_name().map(str::to_owned)))
    }

    /// Checks (in the background) whether `server` is on this network.
    pub fn connected(&mut self, server: &str) {
        self.network = NetworkProbe::start(server);
    }

    /// The signal path of what's playing, as the engine last reported it.
    pub fn signal(&self) -> Option<&SignalPath> {
        self.signal.as_ref()
    }

    /// The transcode new tracks should be streamed with, if any.
    pub fn transcode(&self) -> Option<Transcode> {
        let network = self.network.get();
        self.prefs.get(|s| s.playback.transcode(network))
    }

    /// ReplayGain and streaming preferences.
    pub fn prefs(&self) -> PlaybackPrefs {
        self.prefs.get(|s| s.playback.clone())
    }

    /// Where the server is relative to this computer, as last checked.
    pub fn network(&self) -> crate::playback::Network {
        self.network.get()
    }

    /// Applies and saves new playback preferences (the engine hears about gain
    /// changes at once; streaming changes apply from the next track loaded).
    pub fn set_prefs(&mut self, prefs: PlaybackPrefs, engine: &EngineHandle) {
        let old = self.prefs();
        if prefs == old {
            return;
        }
        if prefs.gain_config() != old.gain_config() {
            engine.send(Command::SetGain(prefs.gain_config()));
        }
        self.prefs.edit(|s| s.playback = prefs);
    }

    /// Whether Normal mode switches the device to each song's sample rate.
    pub fn match_rate(&self) -> bool {
        self.prefs.get(|s| s.output_match_rate)
    }

    /// Saves the choice and tells the engine, which carries on at the new rate.
    pub fn set_match_rate(&mut self, on: bool, engine: &EngineHandle) {
        if self.prefs.managed("output_match_rate") || on == self.match_rate() {
            return;
        }
        self.prefs.edit(|s| s.output_match_rate = on);
        engine.send(Command::MatchRate(on));
    }

    /// The devices macOS reports, and which one is in use (`None`: following the default).
    pub fn devices(&self) -> (&[DeviceInfo], Option<&str>) {
        (&self.devices, self.current.as_deref())
    }

    /// The mode `d` plays in: remembered, else its kind's default. Always
    /// Normal where bit-perfect can't mean anything.
    pub fn device_mode(&self, d: &DeviceInfo) -> PlaybackMode {
        if !d.can_bit_perfect() {
            return PlaybackMode::Normal;
        }
        self.prefs.get(|s| s.device_modes.get(&d.uid).map(|&m| m.into())).unwrap_or_else(|| d.default_mode())
    }

    /// Remembers `mode` for device `uid`; the engine switches now when that's
    /// the device in use. Returns whether it is (the caller updates its flag).
    pub fn set_device_mode(&mut self, uid: &str, mode: PlaybackMode, engine: &EngineHandle) -> bool {
        if self.mode_locked(Some(uid)) {
            return false;
        }
        // Following the system, everything plays shared: it's kept for later.
        let in_use = self.current.as_deref() == Some(uid) && !self.following_system();
        self.remember(uid, mode, engine);
        if in_use {
            engine.send(Command::SetMode(mode));
        }
        in_use
    }

    /// Switches the device in use to `mode` now, remembering it for that device
    /// (following the system default, there's no device to remember it for).
    pub fn set_mode_in_use(&mut self, mode: PlaybackMode, engine: &EngineHandle) {
        if self.mode_locked(self.current.as_deref()) || self.following_system() {
            return;
        }
        engine.send(Command::SetMode(mode));
        if let Some(uid) = self.current.clone() {
            self.remember(&uid, mode, engine);
        }
    }

    fn remember(&mut self, uid: &str, mode: PlaybackMode, engine: &EngineHandle) {
        let modes = {
            let key = uid.to_owned();
            self.prefs.edit(|s| {
                s.device_modes.insert(key, DeviceMode::from(mode));
            });
            self.prefs.get(engine_modes)
        };
        engine.send(Command::DeviceModes(modes));
    }

    /// Takes the engine events this module cares about. Returns the mode the engine
    /// switched to by itself, if it did.
    pub fn handle(&mut self, ev: &audio::Event) -> Option<PlaybackMode> {
        match ev {
            audio::Event::SignalPath(p) => self.signal = Some(p.clone()),
            audio::Event::Devices { devices, current } => {
                self.devices = devices.clone();
                self.current = current.clone();
                for d in devices {
                    self.names.insert(d.uid.clone(), d.name.clone());
                }
            }
            audio::Event::ModeChanged { mode, .. } => return Some(*mode),
            audio::Event::DeviceVolume(v) => {
                // Reports from before our last change are still coming in: keep showing ours.
                let ours = match (v, self.awaiting) {
                    (Some(v), Some((db, at))) if at.elapsed() < SETTLE && (v.db - db).abs() > 0.3 => Some(db),
                    _ => None,
                };
                if ours.is_none() && self.unsent.is_none() {
                    self.awaiting = None;
                }
                self.device_volume = match (v, ours.or(self.unsent)) {
                    (Some(v), Some(db)) => Some(DeviceVolume { db, ..*v }),
                    _ => *v,
                };
            }
            _ => {}
        }
        None
    }

    /// The device's own volume, when bit-perfect holds a device that has one.
    pub fn device_volume(&self) -> Option<DeviceVolume> {
        self.device_volume
    }

    /// Sets the device's own volume (clamped to its range): at once, or
    /// within [`SEND_EVERY`] while it's being dragged.
    pub fn set_device_volume(&mut self, db: f32, engine: &EngineHandle) {
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

    pub fn set_device_mute(&mut self, muted: bool, engine: &EngineHandle) {
        let Some(v) = self.device_volume.as_mut().filter(|v| v.muted.is_some()) else { return };
        v.muted = Some(muted);
        engine.send(Command::SetDeviceMute(muted));
    }

    /// The device in use.
    fn in_use(&self) -> Option<&DeviceInfo> {
        self.devices.iter().find(|d| Some(&d.uid) == self.current.as_ref())
    }

    /// How the output control shows things (see `OutputAction::Show`); a
    /// setting in managed.json stays as it is.
    pub fn set_show(&mut self, green: bool, db: DbReadout, toast: bool) {
        let managed = |k: &str| self.prefs.managed(k);
        let (keep_green, keep_db, keep_toast) = (managed("device_volume_green"), managed("device_volume_db"), managed("device_change_toast"));
        self.prefs.edit(|s| {
            if !keep_green {
                s.device_volume_green = green;
            }
            if !keep_db {
                s.device_volume_db = db;
            }
            if !keep_toast {
                s.device_change_toast = toast;
            }
        });
    }

    /// Leaves device `uid` out of the output menu, or puts it back.
    pub fn set_hidden(&mut self, uid: &str, hidden: bool) {
        if self.prefs.managed("device_hidden") {
            return;
        }
        self.prefs.edit(|s| {
            if hidden {
                s.device_hidden.insert(uid.to_owned());
            } else {
                s.device_hidden.remove(uid);
            }
        });
    }

    /// Puts device `uid` in group `group` in the output menu; blank for its default group.
    pub fn set_group(&mut self, uid: &str, group: &str) {
        if self.prefs.managed(&format!("device_groups.{uid}")) {
            return;
        }
        let group = group.trim().to_owned();
        self.prefs.edit(|s| {
            if group.is_empty() {
                s.device_groups.remove(uid);
            } else {
                s.device_groups.insert(uid.to_owned(), group);
            }
        });
    }

    /// The name of the device in use, for messages.
    pub fn in_use_name(&self) -> Option<&str> {
        self.in_use().map(|d| d.name.as_str())
    }

    /// Whether the device in use can play bit-perfect (see `DeviceInfo::can_bit_perfect`).
    pub fn in_use_can_bit_perfect(&self) -> bool {
        self.in_use().is_some_and(DeviceInfo::can_bit_perfect)
    }

    /// Whether the mode of device `uid` (the one in use) is set in managed.json.
    pub fn mode_locked(&self, uid: Option<&str>) -> bool {
        uid.is_some_and(|uid| self.prefs.managed(&format!("device_modes.{uid}")))
    }

    /// Whether the device to play through is set in managed.json.
    pub fn device_locked(&self) -> bool {
        self.prefs.managed("output_device")
    }

    /// Plays through the device `uid` (the system default for `None`) and remembers it.
    /// Choosing one leaves "System output" for "Choose a device"; `None` goes back.
    pub fn choose(&mut self, uid: Option<String>, engine: &EngineHandle) {
        if self.device_locked() {
            return;
        }
        self.prefs.edit(|s| {
            if uid.is_some() {
                s.last_device = uid.clone();
            }
            s.output_device = uid.clone();
        });
        // The modes first: the engine picks the new device's as it switches.
        engine.send(Command::DeviceModes(self.prefs.get(engine_modes)));
        engine.send(Command::SelectDevice(uid));
    }

    /// "Choose a device": the last one chosen, else the one playing now.
    pub fn choose_a_device(&mut self, engine: &EngineHandle) {
        let uid = self.prefs.get(|s| s.last_device.clone()).or_else(|| self.current.clone());
        if uid.is_some() {
            self.choose(uid, engine);
        }
    }

    /// The room the output control takes (to centre it in the toolbar). The
    /// same in every mode, so nothing moves when the mode does.
    pub fn control_width(&self, slider_w: Option<f32>) -> f32 {
        let db = self.prefs.get(|s| s.device_volume_db) == DbReadout::Always;
        ICON_W + slider_w.map_or(0.0, |w| w + GAP + if db { DB_W } else { 0.0 })
    }

    /// The output control: (given `slider_w`) the volume slider, then the
    /// device's icon with its status dot and a ▾, which opens the panel. The
    /// slider is Ond's volume, or in bit-perfect the device's own when it has
    /// one (unusable when it doesn't).
    pub fn control_ui(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, bit_perfect: &mut bool, volume: f32, slider_w: Option<f32>) -> ControlOut {
        let mut out = ControlOut::default();
        self.flush_device_volume(engine);
        if self.unsent.is_some() {
            ui.ctx().request_repaint_after(SEND_EVERY);
        }
        let hw = self.device_volume.filter(|_| *bit_perfect);
        let readout = self.prefs.get(|s| s.device_volume_db);
        let width = self.control_width(slider_w);
        ui.allocate_ui_with_layout(egui::vec2(width, 28.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            if let Some(w) = slider_w {
                let name = self.in_use_name().unwrap_or("the device").to_owned();
                let (resp, label) = match hw {
                    Some(v) => {
                        let mut pos = slider_pos(&v);
                        let resp = crate::app::thin_slider_marked(ui, &mut pos, w, "Device volume", self.mark(ui))
                            .on_hover_text(device_volume_hover(&name, &v));
                        if resp.changed() {
                            self.set_device_volume(slider_db(&v, pos), engine);
                        }
                        (resp, self.device_volume.map(|v| db_text(&v)))
                    }
                    None => {
                        let mut level = volume;
                        let resp = ui
                            .add_enabled_ui(!*bit_perfect, |ui| crate::app::thin_slider(ui, &mut level, w, "Volume"))
                            .inner
                            .on_disabled_hover_text(format!("Bit-perfect: Ond doesn't change the level. Use {name}'s own volume."));
                        if resp.changed() {
                            out.volume = Some(level);
                        }
                        (resp, (!*bit_perfect).then(|| format!("{:.0}%", level * 100.0)))
                    }
                };
                let adjusting = resp.hovered() || resp.dragged();
                let t = Tokens::of(ui);
                match (readout, label) {
                    // Its own room beside the slider, in every mode.
                    (DbReadout::Always, label) => {
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(DB_W - GAP, 18.0), egui::Sense::hover());
                        if let Some(l) = label.filter(|_| hw.is_some()) {
                            ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, l, egui::FontId::proportional(11.0), t.muted);
                        }
                    }
                    // Floating under the knob while it's adjusted: takes no room.
                    (DbReadout::Adjusting, Some(l)) if adjusting && hw.is_some() => {
                        let layer = egui::LayerId::new(egui::Order::Tooltip, ui.scope_id().with("db-label"));
                        let painter = ui.ctx().layer_painter(layer);
                        let knob = resp.rect.left() + 6.0 + (resp.rect.width() - 12.0) * slider_pos(&hw.expect("matched"));
                        let at = egui::pos2(knob, resp.rect.bottom() + 3.0);
                        let g = painter.layout_no_wrap(l, egui::FontId::proportional(10.5), t.text);
                        let r = egui::Rect::from_center_size(at + egui::vec2(0.0, g.size().y / 2.0 + 2.0), g.size() + egui::vec2(10.0, 4.0));
                        painter.rect(r, 4.0, t.panel, egui::Stroke::new(1.0, t.hairline), egui::StrokeKind::Inside);
                        painter.galley(r.center() - g.size() / 2.0, g, t.text);
                    }
                    _ => {}
                }
                self.adjusting = adjusting;
            }
            let icon = self.icon_button(ui);
            if std::mem::take(&mut self.open_requested) {
                egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&icon));
            }
            egui::Popup::menu(&icon)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .width(POPUP_W)
                .show(|ui| self.popup_ui(ui, engine, bit_perfect, volume, slider_w.is_none(), &mut out));
        });
        out.settings = std::mem::take(&mut self.wants_settings);
        out
    }

    /// The device's icon with the status dot at its corner, and a ▾. Its id
    /// is fixed, so the panel stays open whatever moves beside it.
    fn icon_button(&self, ui: &mut egui::Ui) -> egui::Response {
        let t = Tokens::of(ui);
        let dot = self.dot_color(ui);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ICON_W, 28.0), egui::Sense::hover());
        let resp = ui.interact(rect, egui::Id::unique("output-control-icon"), egui::Sense::click());
        if ui.is_rect_visible(rect) {
            if resp.hovered() {
                ui.painter().rect_filled(rect, 6.0, t.hover);
            }
            let glyph = egui::Rect::from_center_size(egui::pos2(rect.left() + 14.0, rect.center().y), egui::vec2(15.0, 15.0));
            icons::paint(ui.painter(), glyph, kind_icon(self.in_use().map(|d| d.kind)), t.text);
            ui.painter().circle(egui::pos2(glyph.right() + 1.0, glyph.top() + 1.0), 3.5, dot, egui::Stroke::new(1.5, t.bg));
            let chevron = egui::Rect::from_center_size(egui::pos2(rect.right() - 8.0, rect.center().y + 1.0), egui::vec2(7.0, 7.0));
            icons::paint(ui.painter(), chevron, Icon::ChevronDown, t.muted);
        }
        let name = self.in_use_name().unwrap_or("System output").to_owned();
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, true, format!("Output: {name}")));
        let hover = match &self.signal {
            Some(p) => format!("{name}\n{}", hover_text(p)),
            None => name,
        };
        resp.on_hover_text(hover)
    }

    /// The slider knob's mark in bit-perfect mode: a green dot, unless turned off.
    fn mark(&self, ui: &egui::Ui) -> Option<egui::Color32> {
        self.prefs.get(|s| s.device_volume_green).then(|| Tokens::of(ui).ok)
    }

    /// The panel: System output or a chosen device, the device in use (with
    /// its mode and volume), the other devices (grouped, scrolling), and the
    /// way to Settings.
    /// `volume_here`: the toolbar has no slider (narrow), so Ond's volume is here.
    fn popup_ui(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, bit_perfect: &mut bool, volume: f32, volume_here: bool, out: &mut ControlOut) {
        let t = Tokens::of(ui);
        ui.set_width(POPUP_W);
        ui.spacing_mut().item_spacing.y = 0.0;
        let locked = self.device_locked();
        let following = self.following_system();
        ui.add_space(8.0);
        if locked {
            locked_note(ui);
        }
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            ui.add_enabled_ui(!locked, |ui| {
                let mut system = following;
                let labels = |b: bool| if b { "System output".to_owned() } else { "Choose a device".to_owned() };
                if crate::app::choice(ui, &[true, false], &mut system, Some(POPUP_W - 20.0), labels) {
                    if system {
                        self.choose(None, engine);
                    } else {
                        self.choose_a_device(engine);
                    }
                }
            });
        });
        ui.add_space(10.0);
        if following {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.label(egui::RichText::new("Currently using").size(11.0).color(t.muted));
            });
            ui.add_space(4.0);
        }
        self.in_use_card(ui, engine, bit_perfect, volume, volume_here, out);
        ui.add_space(8.0);
        ui.separator();
        // Following the system, the system picks: no list (the tab above leaves it).
        if !following {
            let groups = self.menu_groups();
            egui::ScrollArea::vertical().max_height(LIST_H).auto_shrink([false, true]).show(ui, |ui| {
                for (name, devices) in groups {
                    if let Some(name) = name {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.add_space(12.0);
                            ui.label(egui::RichText::new(name).size(11.0).color(t.muted));
                        });
                        ui.add_space(2.0);
                    }
                    for d in devices {
                        self.device_entry(ui, engine, &d, locked, following, bit_perfect);
                    }
                }
            });
            ui.add_space(4.0);
            ui.separator();
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            if crate::app::link_text(ui, "Playback settings…", egui::FontId::proportional(12.0), t.accent).clicked() {
                self.wants_settings = true;
                ui.close();
            }
        });
        ui.add_space(4.0);
    }

    /// The device in use, at a glance: its icon and name with its mode switch,
    /// what's playing through it, and the volume when it's the device's own
    /// (or the toolbar has none). Details are in the hovers.
    fn in_use_card(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, bit_perfect: &mut bool, volume: f32, volume_here: bool, out: &mut ControlOut) {
        let t = Tokens::of(ui);
        let following = self.following_system();
        if let Some((name, instead)) = self.waiting_for() {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                let text = match instead {
                    Some(i) => format!("Waiting for {name}: playing through {i}"),
                    None => format!("Waiting for {name}"),
                };
                ui.add(egui::Label::new(egui::RichText::new(text).size(11.0).color(t.muted)).wrap());
            });
            ui.add_space(6.0);
        }
        let Some(d) = self.in_use().cloned() else {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.label(egui::RichText::new("No output device").size(13.0).color(t.muted));
            });
            return;
        };
        // Name, and the mode switch at the right.
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::hover());
        let g = egui::Rect::from_center_size(egui::pos2(rect.left() + 20.0, rect.center().y), egui::vec2(16.0, 16.0));
        icons::paint(ui.painter(), g, kind_icon(Some(d.kind)), t.text);
        let switch_w = self.mode_switch(ui, engine, &d, following, bit_perfect, rect);
        let name_w = rect.width() - 36.0 - switch_w - 16.0;
        let name = crate::app::elide_text(ui, &d.name, egui::FontId::new(13.0, crate::theme::semibold()), t.text, name_w);
        ui.painter().galley(egui::pos2(rect.left() + 36.0, rect.center().y - name.size().y / 2.0), name, t.text);
        // What's playing through it.
        if let Some(path) = self.signal.clone() {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(36.0);
                glance_ui(ui, &path).on_hover_text(hover_text(&path));
            });
        }
        if (*bit_perfect && !following) || volume_here {
            ui.add_space(6.0);
            self.volume_row(ui, engine, *bit_perfect, volume, out);
        }
    }

    /// "Bit-perfect" and its switch at the right of `row`, for device `d`:
    /// green when on. None for a device that can only play shared, or while
    /// following the system. Returns their width.
    fn mode_switch(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, d: &DeviceInfo, following: bool, bit_perfect: &mut bool, row: egui::Rect) -> f32 {
        if !d.can_bit_perfect() || following {
            return 0.0;
        }
        let t = Tokens::of(ui);
        let was = self.device_mode(d) == PlaybackMode::BitPerfect;
        let mut on = was;
        let locked = self.mode_locked(Some(&d.uid));
        let g = ui.painter().layout_no_wrap("Bit-perfect".to_owned(), egui::FontId::proportional(12.0), if on { t.text } else { t.muted });
        let w = g.size().x + 8.0 + 30.0;
        let area = egui::Rect::from_min_size(egui::pos2(row.right() - 10.0 - w, row.center().y - 13.0), egui::vec2(w, 26.0));
        // The label switches too.
        let label = ui.interact(egui::Rect::from_min_max(area.min, egui::pos2(area.right() - 30.0, area.bottom())), egui::Id::unique(("mode-label", &d.uid)), if locked { egui::Sense::hover() } else { egui::Sense::click() });
        ui.painter().galley(egui::pos2(area.left(), area.center().y - g.size().y / 2.0), g, if on { t.text } else { t.muted });
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::right_to_left(egui::Align::Center)));
        let help = if locked { crate::managed::LOCKED.to_owned() } else { BIT_PERFECT_HELP.to_owned() };
        let sw = child.add_enabled_ui(!locked, |ui| crate::app::switch_tinted(ui, &mut on, &format!("Bit-perfect on {}", d.name), t.ok)).inner;
        if label.on_hover_text(&help).clicked() {
            on = !on;
        }
        sw.on_hover_text(help);
        if on != was {
            let mode = if on { PlaybackMode::BitPerfect } else { PlaybackMode::Normal };
            if self.set_device_mode(&d.uid, mode, engine) {
                *bit_perfect = on;
            }
        }
        w
    }

    /// One device in the list: its icon and name (what it can do on hover);
    /// a green dot when it's set to bit-perfect; a click plays through it. One
    /// that can play bit-perfect has a ▸ that opens its mode here, without switching.
    fn device_entry(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, d: &DeviceInfo, locked: bool, following: bool, bit_perfect: &mut bool) {
        let t = Tokens::of(ui);
        let opens = d.can_bit_perfect();
        let open = opens && self.expanded.contains(&d.uid);
        let (rect, row) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), if locked { egui::Sense::hover() } else { egui::Sense::click() });
        let chevron = egui::Rect::from_min_size(rect.min, egui::vec2(26.0, ROW_H));
        let toggle = opens.then(|| ui.interact(chevron, egui::Id::unique(("device-open", &d.uid)), egui::Sense::click()));
        if ui.is_rect_visible(rect) {
            if row.hovered() || toggle.as_ref().is_some_and(egui::Response::hovered) {
                ui.painter().rect_filled(rect.shrink2(egui::vec2(4.0, 0.0)), 6.0, t.hover);
            }
            if opens {
                let c = egui::Rect::from_center_size(chevron.center(), egui::vec2(8.0, 8.0));
                icons::paint(ui.painter(), c, if open { Icon::ChevronDown } else { Icon::ChevronRight }, t.muted);
            }
            let g = egui::Rect::from_center_size(egui::pos2(rect.left() + 36.0, rect.center().y), egui::vec2(14.0, 14.0));
            icons::paint(ui.painter(), g, kind_icon(Some(d.kind)), t.muted);
            let x = rect.left() + 52.0;
            let name = crate::app::elide_text(ui, &d.name, egui::FontId::proportional(13.0), t.text, rect.right() - 30.0 - x);
            let name_end = x + name.size().x;
            ui.painter().galley(egui::pos2(x, rect.center().y - name.size().y / 2.0), name, t.text);
            if opens && self.device_mode(d) == PlaybackMode::BitPerfect {
                ui.painter().circle_filled(egui::pos2(name_end + 8.0, rect.center().y), 3.0, t.ok);
            }
        }
        let caps = d.capability_line();
        let hover = if following { format!("Play through {} (leaves System output)\n{caps}", d.name) } else { format!("Play through {}\n{caps}", d.name) };
        let label = d.name.clone();
        let row = row.on_hover_text(hover);
        row.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, !locked, &label));
        if toggle.is_some_and(|r| r.on_hover_text(if open { "Hide its mode" } else { "Its mode" }).clicked()) {
            if open {
                self.expanded.remove(&d.uid);
            } else {
                self.expanded.insert(d.uid.clone());
            }
        } else if row.clicked() {
            self.expanded.remove(&d.uid);
            self.choose(Some(d.uid.clone()), engine);
        }
        if open {
            let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::hover());
            // The same switch as the device in use has; it doesn't switch devices.
            self.mode_switch(ui, engine, d, false, bit_perfect, r);
            ui.add_space(4.0);
        }
    }

    /// The devices for the menu, grouped: hidden ones and the one in use
    /// left out; the user's groups between the unnamed hardware group and
    /// "Other" (software devices); each alphabetical.
    fn menu_groups(&self) -> Vec<(Option<String>, Vec<DeviceInfo>)> {
        let (hidden, named) = self.prefs.get(|s| (s.device_hidden.clone(), s.device_groups.clone()));
        let current = self.current.clone();
        group_devices(self.devices.iter().filter(|d| Some(&d.uid) != current.as_ref() && !hidden.contains(&d.uid)), &named)
    }

    /// The volume line of the card: in bit-perfect the device's own level over
    /// its whole range, with its exact dB (↑↓ step 0.5 dB, ⇧ 0.1; scrolling
    /// too) and mute; Ond's volume when the toolbar has no slider; or a word
    /// on using the device's own control when it has none Ond can set.
    fn volume_row(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, bit_perfect: bool, volume: f32, out: &mut ControlOut) {
        let t = Tokens::of(ui);
        let name = self.in_use_name().unwrap_or("the device").to_owned();
        ui.horizontal(|ui| {
            ui.add_space(36.0);
            ui.spacing_mut().item_spacing.x = 8.0;
            match self.device_volume.filter(|_| bit_perfect) {
                Some(v) => {
                    let mut pos = (v.db - v.min_db) / (v.max_db - v.min_db).max(0.01);
                    // The row's width, less the dB field and the mute button.
                    let w = (ui.available_width() - 112.0).max(100.0);
                    let resp = crate::app::thin_slider_marked(ui, &mut pos, w, "Device volume", self.mark(ui))
                        .on_hover_text(device_volume_hover(&name, &v));
                    if resp.changed() {
                        self.set_device_volume(v.min_db + pos * (v.max_db - v.min_db), engine);
                    }
                    // The field's own arrows step by its drag speed: ours step like the old buttons.
                    let focused = self.db_field.is_some_and(|id| ui.memory(|m| m.has_focus(id)));
                    let step = if focused {
                        ui.input_mut(|i| {
                            let fine = i.count_and_consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowUp) as f32
                                - i.count_and_consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowDown) as f32;
                            let coarse = i.count_and_consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) as f32
                                - i.count_and_consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) as f32;
                            fine * FINE_STEP_DB + coarse * STEP_DB
                        })
                    } else {
                        0.0
                    };
                    let mut db = self.device_volume.map_or(v.db, |v| v.db);
                    let field = ui.add(egui::DragValue::new(&mut db).range(v.min_db..=v.max_db).speed(0.1).fixed_decimals(1).suffix(" dB"));
                    self.db_field = Some(field.id);
                    let mut target = field.changed().then_some(db);
                    if step != 0.0 {
                        target = Some(self.device_volume.map_or(v.db, |v| v.db) + step);
                    }
                    if field.hovered() {
                        self.scroll += ui.input(|i| i.smooth_scroll_delta.y);
                        let notches = (self.scroll / SCROLL_NOTCH).trunc();
                        if notches != 0.0 {
                            self.scroll -= notches * SCROLL_NOTCH;
                            target = Some(self.device_volume.map_or(v.db, |v| v.db) + notches * STEP_DB);
                        }
                    }
                    if let Some(db) = target {
                        self.set_device_volume(db, engine);
                    }
                    field.on_hover_text("↑↓ 0.5 dB · ⇧↑↓ 0.1 dB · or scroll");
                    if let Some(muted) = self.device_volume.and_then(|v| v.muted) {
                        let (r, b) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
                        let g = egui::Rect::from_center_size(r.center(), egui::vec2(13.0, 13.0));
                        icons::paint(ui.painter(), g, if muted { Icon::Muted } else { Icon::Volume }, if muted { t.accent } else { t.muted });
                        if b.on_hover_text(if muted { "Unmute" } else { "Mute" }).clicked() {
                            self.set_device_mute(!muted, engine);
                        }
                    }
                }
                None if bit_perfect => {
                    let why = format!("Ond doesn't change the level in bit-perfect mode. Use {name}'s own volume.");
                    ui.add(egui::Label::new(egui::RichText::new(why).size(11.0).color(t.muted)).wrap());
                }
                None => {
                    let mut level = volume;
                    let w = (ui.available_width() - 56.0).max(100.0);
                    if crate::app::thin_slider(ui, &mut level, w, "Volume").changed() {
                        out.volume = Some(level);
                    }
                    ui.label(egui::RichText::new(format!("{:.0}%", level * 100.0)).size(11.0).color(t.muted));
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

    /// Opens the output popup (from the readout) on the next pass of the picker.
    pub fn request_open(&mut self) {
        self.open_requested = true;
    }

    /// The readout for what's playing, if anything is.
    pub fn readout(&self, status: &audio::Status) -> Option<Readout> {
        let p = self.signal.as_ref()?;
        if status.state() == PlayState::Stopped {
            return None;
        }
        let mut detail = format!("{}\n{}", p.summary(), hover_text(p));
        let underruns = status.underruns();
        if underruns > 0 {
            detail.push_str(&format!("\n{underruns} dropouts"));
        }
        Some(readout_of(p, detail))
    }
}

/// The icon for a kind of device; the speaker when there's none to go by.
pub fn kind_icon(kind: Option<DeviceKind>) -> Icon {
    match kind {
        Some(DeviceKind::BuiltInHeadphones) => Icon::Headphones,
        Some(DeviceKind::Usb | DeviceKind::Other) => Icon::Dac,
        Some(DeviceKind::Hdmi) => Icon::Display,
        Some(DeviceKind::Bluetooth) => Icon::Bluetooth,
        Some(DeviceKind::AirPlay) => Icon::AirPlay,
        _ => Icon::Volume,
    }
}

/// The toolbar slider's bottom: the top [`SLIDER_SPAN_DB`] of the range.
fn slider_floor(v: &DeviceVolume) -> f32 {
    (v.max_db - SLIDER_SPAN_DB).max(v.min_db)
}

/// Where the device's level sits on the toolbar slider (0 = its minimum).
fn slider_pos(v: &DeviceVolume) -> f32 {
    let floor = slider_floor(v);
    if v.db <= floor { 0.0 } else { ((v.db - floor) / (v.max_db - floor).max(0.01)).clamp(0.0, 1.0) }
}

/// The level for a toolbar slider position: the very bottom is the device's minimum.
fn slider_db(v: &DeviceVolume, pos: f32) -> f32 {
    if pos <= 0.0 { v.min_db } else { slider_floor(v) + pos * (v.max_db - slider_floor(v)) }
}

/// "−24.0 dB", or "Muted".
fn db_text(v: &DeviceVolume) -> String {
    if v.muted == Some(true) {
        return "Muted".into();
    }
    let db = if v.db.abs() < 0.05 { 0.0 } else { v.db };
    format!("{db:.1} dB").replace('-', "−")
}

fn device_volume_hover(name: &str, v: &DeviceVolume) -> String {
    format!(
        "{name}'s own volume · {}\nOnd's samples reach it untouched; the device sets the level. The top is {:.0} dB, full level.",
        db_text(v),
        v.max_db
    )
}

fn readout_of(p: &SignalPath, detail: String) -> Readout {
    let (from, to) = match &p.transcoded {
        Some(t) => (t.clone(), None),
        None => {
            let from = audio::describe(p.source.bits_per_sample, p.source.sample_rate);
            let changed = p.resampled || matches!((p.device_bits, p.source.bits_per_sample), (Some(d), Some(s)) if d < s);
            (from, changed.then(|| audio::describe(p.device_bits, p.device_rate)))
        }
    };
    let verdict = match &p.bit_perfect {
        BitPerfectStatus::Off => Verdict::None,
        BitPerfectStatus::Verified => Verdict::Verified,
        BitPerfectStatus::Requested { .. } => Verdict::Requested,
        BitPerfectStatus::Unavailable { .. } => Verdict::Unavailable,
    };
    Readout { from, to, verdict, detail }
}

/// The readout in a small pill: the format(s) and, after them, the verdict dot.
/// Right-aligned at `right` (vertically centred on it). Returns the pill's
/// rect; with `paint` false it only measures.
pub fn paint_readout(ui: &egui::Ui, right: egui::Pos2, r: &Readout, size: f32, paint: bool) -> egui::Rect {
    const PAD_X: f32 = 6.0;
    const PAD_Y: f32 = 2.5;
    let t = Tokens::of(ui);
    let painter = ui.painter();
    let font = egui::FontId::proportional(size);
    let from = painter.layout_no_wrap(r.from.clone(), font.clone(), t.muted);
    let to = r.to.as_ref().map(|s| painter.layout_no_wrap(s.clone(), font.clone(), t.muted));
    let arrow_w = size * 0.85;
    let gap = 3.0;
    let dot_w = if r.verdict == Verdict::None { 0.0 } else { 4.0 + 5.0 };
    let text_w = from.size().x + to.as_ref().map_or(0.0, |g| gap + arrow_w + gap + g.size().x);
    let h = from.size().y;
    let rect = egui::Rect::from_min_max(
        egui::pos2(right.x - text_w - dot_w - 2.0 * PAD_X, right.y - h / 2.0 - PAD_Y),
        egui::pos2(right.x, right.y + h / 2.0 + PAD_Y),
    );
    if !paint {
        return rect;
    }
    painter.rect(rect, rect.height() / 2.0, t.field, egui::Stroke::new(1.0, t.hairline), egui::StrokeKind::Inside);
    let mut x = rect.left() + PAD_X;
    let top = right.y - h / 2.0;
    painter.galley(egui::pos2(x, top), from.clone(), t.muted);
    x += from.size().x;
    if let Some(to) = to {
        x += gap;
        let a = egui::Rect::from_center_size(egui::pos2(x + arrow_w / 2.0, right.y), egui::vec2(arrow_w, size * 0.7));
        icons::paint(painter, a, Icon::ArrowRight, t.muted);
        x += arrow_w + gap;
        painter.galley(egui::pos2(x, top), to.clone(), t.muted);
        x += to.size().x;
    }
    let c = egui::pos2(x + 4.0 + 2.5, right.y);
    match r.verdict {
        Verdict::None => {}
        Verdict::Verified => {
            painter.circle_filled(c, 2.6, t.ok);
        }
        Verdict::Unavailable => {
            painter.circle_filled(c, 2.6, t.warn);
        }
        Verdict::Requested => {
            painter.circle_stroke(c, 2.3, egui::Stroke::new(1.0, t.muted));
        }
    }
    rect
}


/// Puts devices in their groups for the menu: the unnamed hardware group,
/// the user's groups, then "Other" (software devices); each alphabetical.
pub fn group_devices<'a>(devices: impl Iterator<Item = &'a DeviceInfo>, named: &HashMap<String, String>) -> Vec<(Option<String>, Vec<DeviceInfo>)> {
    let mut groups: Vec<(Option<String>, Vec<DeviceInfo>)> = Vec::new();
    for d in devices {
        let group = Some(group_of(d, named)).filter(|g| g != DEVICES);
        match groups.iter_mut().find(|(g, _)| *g == group) {
            Some((_, list)) => list.push(d.clone()),
            None => groups.push((group, vec![d.clone()])),
        }
    }
    let rank = |g: &Option<String>| match g.as_deref() {
        None => 0,
        Some(OTHER) => 2,
        Some(_) => 1,
    };
    groups.sort_by(|a, b| rank(&a.0).cmp(&rank(&b.0)).then_with(|| a.0.cmp(&b.0)));
    for (_, list) in &mut groups {
        list.sort_by_key(|d| d.name.to_lowercase());
    }
    groups
}

/// The group device `d` is in: the one given it, else hardware in
/// [`DEVICES`] and software in [`OTHER`].
pub fn group_of(d: &DeviceInfo, named: &HashMap<String, String>) -> String {
    named.get(&d.uid).map(|g| g.trim()).filter(|g| !g.is_empty()).map(str::to_owned).unwrap_or_else(|| auto_group(d).to_owned())
}

/// The group a device goes in unless it's given another.
pub fn auto_group(d: &DeviceInfo) -> &'static str {
    if d.kind == DeviceKind::Virtual { OTHER } else { DEVICES }
}

/// What's playing through the device, at a glance: the format pill (the
/// display's), and a word only when something's wrong.
fn glance_ui(ui: &mut egui::Ui, path: &SignalPath) -> egui::Response {
    let t = Tokens::of(ui);
    let r = readout_of(path, String::new());
    let size = paint_readout(ui, egui::Pos2::ZERO, &r, 11.0, false).size();
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    paint_readout(ui, rect.right_center(), &r, 11.0, true);
    // Verified needs no word (the switch says what was asked; the green dot, that it is).
    let word = match &path.bit_perfect {
        BitPerfectStatus::Verified => None,
        BitPerfectStatus::Unavailable { .. } if path.summary().ends_with("device limit") => Some(("device limit".to_owned(), t.warn)),
        BitPerfectStatus::Unavailable { .. } => Some(("not bit-perfect".to_owned(), t.warn)),
        BitPerfectStatus::Requested { reason } => Some((format!("not verified: {reason}"), t.muted)),
        BitPerfectStatus::Off => None,
    };
    match word {
        Some((w, c)) => resp.union(ui.label(egui::RichText::new(w).size(11.0).color(c))),
        None => resp,
    }
}

fn hover_text(p: &SignalPath) -> String {
    let verdict = match &p.bit_perfect {
        BitPerfectStatus::Off => "Normal mode, with volume, ReplayGain and resampling".to_owned(),
        BitPerfectStatus::Verified => "Bit-perfect, with exclusive access, the file's sample rate and untouched samples".to_owned(),
        BitPerfectStatus::Requested { reason } => format!("Bit-perfect requested, not verified: {reason}"),
        BitPerfectStatus::Unavailable { reason } => format!("Not bit-perfect: {reason}"),
    };
    format!("{}\n{verdict}", p.output)
}

/// The modes the engine plays devices in: the remembered ones on a chosen
/// device; following the system, none (every device plays Normal: taking the
/// system's device for Ond alone would only send the system elsewhere).
fn engine_modes(s: &crate::settings::Settings) -> HashMap<String, PlaybackMode> {
    if s.output_device.is_none() {
        return HashMap::new();
    }
    s.device_modes.iter().map(|(uid, &m)| (uid.clone(), m.into())).collect()
}

/// Above a part of the popup set in managed.json, which can't be changed here.
fn locked_note(ui: &mut egui::Ui) {
    let t = Tokens::of(ui);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(crate::managed::LOCKED).size(11.0).color(t.muted));
    });
    ui.add_space(4.0);
}
