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
const OTHER: &str = "Other";
const BIT_PERFECT_HELP: &str = "Bit-perfect sends samples untouched and keeps other apps off the device while Ond plays. The volume then sets the device's own level, when it has one.";
/// The toolbar slider covers the top this many dB of a device's volume (most
/// of a 100 dB range is too quiet to use); the panel's covers all of it.
const SLIDER_SPAN_DB: f32 = 60.0;
/// The panel's step buttons, in dB.
const STEP_DB: f32 = 0.5;
/// Room beside the slider for "−24.0 dB".
const DB_W: f32 = 52.0;
/// After we set the device volume, reports older than this don't move the slider back.
const SETTLE: Duration = Duration::from_millis(600);

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
    /// When we last set it ourselves (see [`SETTLE`]).
    set_at: Option<Instant>,
    /// The slider was hovered or dragged last frame (for the dB readout).
    adjusting: bool,
    /// Devices whose settings are open in the panel's list, by UID.
    expanded: std::collections::HashSet<String>,
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
            prefs.get(|s| (engine_modes(&s.device_modes), s.output_device.clone(), s.playback.clone(), s.output_match_rate));
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
            set_at: None,
            adjusting: false,
            expanded: Default::default(),
        }
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
        let in_use = self.current.as_deref() == Some(uid);
        self.remember(uid, mode, engine);
        if in_use {
            engine.send(Command::SetMode(mode));
        }
        in_use
    }

    /// Switches the device in use to `mode` now, remembering it for that device
    /// (following the system default, there's no device to remember it for).
    pub fn set_mode_in_use(&mut self, mode: PlaybackMode, engine: &EngineHandle) {
        if self.mode_locked(self.current.as_deref()) {
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
            self.prefs.get(|s| engine_modes(&s.device_modes))
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
            }
            audio::Event::ModeChanged { mode, .. } => return Some(*mode),
            audio::Event::DeviceVolume(v) => {
                // Our own change may not have reached the device yet: keep showing it.
                let settling = self.set_at.is_some_and(|t| t.elapsed() < SETTLE);
                self.device_volume = match (v, self.device_volume) {
                    (Some(v), Some(mine)) if settling => Some(DeviceVolume { db: mine.db, ..*v }),
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

    /// Sets the device's own volume (clamped to its range).
    pub fn set_device_volume(&mut self, db: f32, engine: &EngineHandle) {
        let Some(v) = self.device_volume.as_mut() else { return };
        v.db = db.clamp(v.min_db, v.max_db);
        engine.send(Command::SetDeviceVolume(v.db));
        self.set_at = Some(Instant::now());
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

    /// Whether device `uid` is left out of the output menu, and its group (blank: default).
    pub fn menu_place(&self, uid: &str) -> (bool, String) {
        self.prefs.get(|s| (s.device_hidden.contains(uid), s.device_groups.get(uid).cloned().unwrap_or_default()))
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
    pub fn choose(&mut self, uid: Option<String>, engine: &EngineHandle) {
        if self.device_locked() {
            return;
        }
        engine.send(Command::SelectDevice(uid.clone()));
        self.prefs.edit(|s| s.output_device = uid);
    }

    /// The room the output control takes (to centre it in the toolbar).
    pub fn control_width(&self, bit_perfect: bool, slider_w: Option<f32>) -> f32 {
        ICON_W + slider_w.map_or(0.0, |w| w + GAP + self.db_width(bit_perfect))
    }

    /// Room for the dB readout beside the slider, when it can show.
    fn db_width(&self, bit_perfect: bool) -> f32 {
        let hw = bit_perfect && self.device_volume.is_some();
        if hw && self.prefs.get(|s| s.device_volume_db) != DbReadout::Never { DB_W } else { 0.0 }
    }

    /// The output control: (given `slider_w`) the volume slider, then the
    /// device's icon with its status dot and a ▾, which opens the panel. The
    /// slider is Ond's volume in Normal mode, and the device's own in
    /// bit-perfect mode when it has one (unusable when it doesn't).
    pub fn control_ui(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, bit_perfect: &mut bool, volume: f32, slider_w: Option<f32>) -> ControlOut {
        let mut out = ControlOut::default();
        let hw = self.device_volume.filter(|_| *bit_perfect);
        let readout = self.prefs.get(|s| s.device_volume_db);
        let db_w = self.db_width(*bit_perfect);
        let width = self.control_width(*bit_perfect, slider_w);
        ui.allocate_ui_with_layout(egui::vec2(width, 28.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            if let Some(w) = slider_w {
                let name = self.in_use_name().unwrap_or("the device").to_owned();
                match hw {
                    Some(v) => {
                        let mut pos = slider_pos(&v);
                        let resp = crate::app::thin_slider_marked(ui, &mut pos, w, "Device volume", self.mark(ui))
                            .on_hover_text(device_volume_hover(&name, &v));
                        if resp.changed() {
                            self.set_device_volume(slider_db(&v, pos), engine);
                        }
                        self.adjusting = resp.hovered() || resp.dragged();
                        if db_w > 0.0 {
                            let shown = readout == DbReadout::Always || self.adjusting;
                            let (rect, _) = ui.allocate_exact_size(egui::vec2(db_w - GAP, 18.0), egui::Sense::hover());
                            if shown && let Some(v) = self.device_volume {
                                let t = Tokens::of(ui);
                                ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, db_text(&v), egui::FontId::proportional(11.0), t.muted);
                            }
                        }
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
                    }
                }
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

    /// The device's icon with the status dot at its corner, and a ▾.
    fn icon_button(&self, ui: &mut egui::Ui) -> egui::Response {
        let t = Tokens::of(ui);
        let dot = self.dot_color(ui);
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ICON_W, 28.0), egui::Sense::click());
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
        let name = self.in_use_name().unwrap_or("System default").to_owned();
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

    /// The panel: the device in use (pinned, with its settings), following
    /// the system default, the other devices (grouped, scrolling), and the
    /// way to Settings.
    /// `volume_here`: the toolbar has no slider (narrow), so Ond's volume is here.
    fn popup_ui(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, bit_perfect: &mut bool, volume: f32, volume_here: bool, out: &mut ControlOut) {
        let t = Tokens::of(ui);
        ui.set_width(POPUP_W);
        ui.spacing_mut().item_spacing.y = 0.0;
        let locked = self.device_locked();
        ui.add_space(6.0);
        self.in_use_card(ui, engine, bit_perfect, volume, volume_here, out);
        ui.add_space(8.0);
        ui.separator();
        if locked {
            ui.add_space(4.0);
            locked_note(ui);
        }
        let following = self.prefs.get(|s| s.output_device.is_none());
        let mut follow = following;
        ui.add_enabled_ui(!locked, |ui| setting_line(ui, "Follow system default", None, &mut follow));
        if follow != following {
            let uid = if follow { None } else { self.current.clone() };
            self.choose(uid, engine);
        }
        let groups = self.menu_groups();
        egui::ScrollArea::vertical().max_height(LIST_H).auto_shrink([false, true]).show(ui, |ui| {
            for (name, devices) in groups {
                if let Some(name) = name {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new(name).size(11.0).color(t.muted));
                    });
                    ui.add_space(2.0);
                }
                for d in devices {
                    self.device_entry(ui, engine, &d, locked, bit_perfect);
                }
            }
        });
        ui.add_space(4.0);
        ui.separator();
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            if crate::app::link_text(ui, "Playback settings…", egui::FontId::proportional(12.0), t.accent).clicked() {
                self.wants_settings = true;
                ui.close();
            }
        });
        ui.add_space(4.0);
    }

    /// The device in use: its name and what it can do, what's playing
    /// through it, its bit-perfect switch, and in bit-perfect its own volume.
    fn in_use_card(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, bit_perfect: &mut bool, volume: f32, volume_here: bool, out: &mut ControlOut) {
        let t = Tokens::of(ui);
        let Some(d) = self.in_use().cloned() else {
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.label(egui::RichText::new("No output device").size(13.0).color(t.muted));
            });
            return;
        };
        // Following the default, and this is it.
        let following = self.prefs.get(|s| s.output_device.is_none()) && d.is_default;
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            let (g, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
            icons::paint(ui.painter(), g, kind_icon(Some(d.kind)), t.text);
            ui.add_space(4.0);
            ui.label(egui::RichText::new(&d.name).size(13.0).font(egui::FontId::new(13.0, crate::theme::semibold())).color(t.text));
        });
        ui.add_space(2.0);
        let caps = d.capability_line();
        let sub = match (following, caps.is_empty()) {
            (true, true) => "System default".to_owned(),
            (true, false) => format!("System default · {caps}"),
            (false, _) => caps,
        };
        if !sub.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(34.0);
                ui.label(egui::RichText::new(sub).size(11.0).color(t.muted));
            });
        }
        if let Some(path) = self.signal.clone() {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(34.0);
                summary_ui(ui, &path, 12.0, true);
            });
        }
        if d.can_bit_perfect() {
            ui.add_space(4.0);
            self.bit_perfect_line(ui, engine, &d, bit_perfect);
        }
        if *bit_perfect || volume_here {
            ui.add_space(2.0);
            self.volume_row(ui, engine, *bit_perfect, volume, out);
        }
    }

    /// "Bit-perfect" and its switch, for device `d`: sets its mode (the one in
    /// use switches at once).
    fn bit_perfect_line(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, d: &DeviceInfo, bit_perfect: &mut bool) {
        let was = self.device_mode(d) == PlaybackMode::BitPerfect;
        let mut on = was;
        let locked = self.mode_locked(Some(&d.uid));
        ui.add_enabled_ui(!locked, |ui| setting_line(ui, "Bit-perfect", Some(BIT_PERFECT_HELP), &mut on));
        if on != was {
            let mode = if on { PlaybackMode::BitPerfect } else { PlaybackMode::Normal };
            if self.set_device_mode(&d.uid, mode, engine) {
                *bit_perfect = on;
            }
        }
    }

    /// One device in the list: a click plays through it. One that has
    /// settings has a ▸ that opens them here, without switching.
    fn device_entry(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, d: &DeviceInfo, locked: bool, bit_perfect: &mut bool) {
        let t = Tokens::of(ui);
        let opens = d.can_bit_perfect();
        let open = opens && self.expanded.contains(&d.uid);
        let (rect, row) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), if locked { egui::Sense::hover() } else { egui::Sense::click() });
        let chevron = egui::Rect::from_min_size(rect.min, egui::vec2(26.0, ROW_H));
        let toggle = opens.then(|| ui.interact(chevron, ui.scope_id().with(("open", &d.uid)), egui::Sense::click()));
        if ui.is_rect_visible(rect) {
            if row.hovered() || toggle.as_ref().is_some_and(egui::Response::hovered) {
                ui.painter().rect_filled(rect, 6.0, t.hover);
            }
            if opens {
                let c = egui::Rect::from_center_size(chevron.center(), egui::vec2(8.0, 8.0));
                icons::paint(ui.painter(), c, if open { Icon::ChevronDown } else { Icon::ChevronRight }, t.muted);
            }
            let g = egui::Rect::from_center_size(egui::pos2(rect.left() + 36.0, rect.center().y), egui::vec2(14.0, 14.0));
            icons::paint(ui.painter(), g, kind_icon(Some(d.kind)), t.muted);
            let caps = d.capability_line();
            let caps_g = ui.painter().layout_no_wrap(caps, egui::FontId::proportional(11.0), t.muted);
            let x = rect.left() + 50.0;
            let name_w = (rect.right() - 10.0 - caps_g.size().x - 8.0 - x).max(40.0);
            let name = crate::app::elide_text(ui, &d.name, egui::FontId::proportional(13.0), t.text, name_w);
            ui.painter().galley(egui::pos2(x, rect.center().y - name.size().y / 2.0), name, t.text);
            ui.painter().galley(egui::pos2(rect.right() - 10.0 - caps_g.size().x, rect.center().y - caps_g.size().y / 2.0), caps_g, t.muted);
        }
        let label = d.name.clone();
        let row = row.on_hover_text(format!("Play through {label}"));
        row.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, !locked, &label));
        if toggle.is_some_and(|r| r.on_hover_text(if open { "Hide settings" } else { "Settings" }).clicked()) {
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
            ui.indent(("settings", &d.uid), |ui| self.bit_perfect_line(ui, engine, d, bit_perfect));
            ui.add_space(4.0);
        }
    }

    /// The devices for the menu, grouped: hidden ones and the one in use
    /// left out; the user's groups between the unnamed hardware group and
    /// "Other" (software devices); each alphabetical.
    fn menu_groups(&self) -> Vec<(Option<String>, Vec<DeviceInfo>)> {
        let (hidden, named) = self.prefs.get(|s| (s.device_hidden.clone(), s.device_groups.clone()));
        let mut groups: Vec<(Option<String>, Vec<DeviceInfo>)> = Vec::new();
        for d in self.devices.iter().filter(|d| Some(&d.uid) != self.current.as_ref() && !hidden.contains(&d.uid)) {
            let group = named.get(&d.uid).filter(|g| !g.trim().is_empty()).map(|g| g.trim().to_owned()).or_else(|| (d.kind == DeviceKind::Virtual).then(|| OTHER.to_owned()));
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

    /// The volume line of the card: the device's own level over its whole
    /// range, to the step, with mute (bit-perfect); or a word on using the
    /// device's own control when it has none Ond can set.
    fn volume_row(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, bit_perfect: bool, volume: f32, out: &mut ControlOut) {
        let t = Tokens::of(ui);
        let name = self.in_use_name().unwrap_or("the device").to_owned();
        ui.horizontal(|ui| {
            ui.add_space(34.0);
            ui.spacing_mut().item_spacing.x = 6.0;
            match self.device_volume.filter(|_| bit_perfect) {
                Some(v) => {
                    let mut pos = (v.db - v.min_db) / (v.max_db - v.min_db).max(0.01);
                    let resp = crate::app::thin_slider_marked(ui, &mut pos, 110.0, "Device volume", self.mark(ui))
                        .on_hover_text(device_volume_hover(&name, &v));
                    if resp.changed() {
                        self.set_device_volume(v.min_db + pos * (v.max_db - v.min_db), engine);
                    }
                    let mut db = self.device_volume.map_or(v.db, |v| v.db);
                    let field = ui.add(egui::DragValue::new(&mut db).range(v.min_db..=v.max_db).speed(0.1).fixed_decimals(1).suffix(" dB"));
                    if field.changed() {
                        self.set_device_volume(db, engine);
                    }
                    for (text, step, label) in [("−", -STEP_DB, "Down 0.5 dB"), ("+", STEP_DB, "Up 0.5 dB")] {
                        let b = ui.add(egui::Button::new(egui::RichText::new(text).size(13.0).color(t.muted)).frame(false).min_size(egui::vec2(14.0, 18.0)));
                        if b.on_hover_text(label).clicked() {
                            let now = self.device_volume.map_or(v.db, |v| v.db);
                            self.set_device_volume(now + step, engine);
                        }
                    }
                    if let Some(muted) = self.device_volume.and_then(|v| v.muted) {
                        let text = egui::RichText::new(if muted { "Muted" } else { "Mute" }).size(11.0).color(if muted { t.accent } else { t.muted });
                        if ui.add(egui::Button::new(text).frame(false)).on_hover_text(if muted { "Unmute" } else { "Mute" }).clicked() {
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
                    if crate::app::thin_slider(ui, &mut level, 160.0, "Volume").changed() {
                        out.volume = Some(level);
                    }
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
fn kind_icon(kind: Option<DeviceKind>) -> Icon {
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

/// A label with a switch at the right of the panel, `help` on hover.
fn setting_line(ui: &mut egui::Ui, label: &str, help: Option<&str>, on: &mut bool) {
    let t = Tokens::of(ui);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::hover());
    ui.painter().text(egui::pos2(rect.left() + 34.0, rect.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(12.0), if ui.is_enabled() { t.text } else { t.muted });
    let area = egui::Rect::from_min_max(egui::pos2(rect.right() - 50.0, rect.top()), egui::pos2(rect.right() - 8.0, rect.bottom()));
    let mut sw = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::right_to_left(egui::Align::Center)));
    let s = crate::app::switch(&mut sw, on, label);
    if let Some(help) = help {
        resp.union(s).on_hover_text(help);
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

fn engine_modes(saved: &HashMap<String, DeviceMode>) -> HashMap<String, PlaybackMode> {
    saved.iter().map(|(uid, &m)| (uid.clone(), m.into())).collect()
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
