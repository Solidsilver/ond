//! The output: which device plays and in which mode, the playback
//! preferences in use (see `playback` for what they are), and the engine's
//! word on devices and the signal. The control and its panel are in `panel`,
//! the device's own volume in `volume`, the signal-path readout in `readout`.

mod panel;
mod readout;
mod volume;

pub use readout::{Readout, paint_readout};

use crate::icons::Icon;
use crate::playback::{NetworkProbe, PlaybackPrefs};
use crate::prefs::Prefs;
use crate::settings::DeviceMode;
use crate::settings::DbReadout;
use audio::{Command, DeviceInfo, DeviceKind, DeviceVolume, EngineHandle, PlaybackMode, SignalPath, Transcode};
use std::collections::HashMap;
use std::time::Instant;

/// The volume slider in the toolbar and the Player's status bar.
pub const SLIDER_W: f32 = 90.0;
/// The group software devices go in, after the rest.
pub const OTHER: &str = "Other";
/// The group hardware goes in, first (it has no heading in the menu).
pub const DEVICES: &str = "Devices";

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
    /// Names of every device seen this session, by UID: kept after a device
    /// is unplugged, to say which one we're waiting for.
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

    /// The devices the system reports, and which one is in use (`None`: following the default).
    pub fn devices(&self) -> (&[DeviceInfo], Option<&str>) {
        (&self.devices, self.current.as_deref())
    }

    /// The mode `d` plays in: the one remembered for it, else Normal (bit-perfect
    /// is opt-in). Always Normal where bit-perfect can't mean anything.
    pub fn device_mode(&self, d: &DeviceInfo) -> PlaybackMode {
        if !d.can_bit_perfect() {
            return PlaybackMode::Normal;
        }
        self.prefs.get(|s| s.device_modes.get(&d.uid).map(|&m| m.into())).unwrap_or(PlaybackMode::Normal)
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

    /// Switches the device in use to `mode` now, remembering it for that
    /// device. Returns whether it did: not when the mode is locked, when the
    /// device can't play bit-perfect, or following the system default (there's
    /// no device to remember it for).
    pub fn set_mode_in_use(&mut self, mode: PlaybackMode, engine: &EngineHandle) -> bool {
        if self.mode_locked(self.current.as_deref()) || !self.in_use_can_bit_perfect() || self.following_system() {
            return false;
        }
        engine.send(Command::SetMode(mode));
        if let Some(uid) = self.current.clone() {
            self.remember(&uid, mode, engine);
        }
        true
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
                // A device that's gone has no settings to show.
                self.expanded.retain(|uid| devices.iter().any(|d| &d.uid == uid));
            }
            audio::Event::ModeChanged { mode, .. } => return Some(*mode),
            audio::Event::DeviceVolume(v) => self.device_volume_reported(*v),
            _ => {}
        }
        None
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

/// The modes the engine plays devices in: the remembered ones on a chosen
/// device; following the system, none (every device plays Normal: taking the
/// system's device for Ond alone would only send the system elsewhere).
fn engine_modes(s: &crate::settings::Settings) -> HashMap<String, PlaybackMode> {
    if s.output_device.is_none() {
        return HashMap::new();
    }
    s.device_modes.iter().map(|(uid, &m)| (uid.clone(), m.into())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(uid: &str, name: &str, kind: DeviceKind) -> DeviceInfo {
        DeviceInfo { uid: uid.into(), name: name.into(), kind, caps: Default::default(), is_default: false }
    }

    #[test]
    fn the_menu_groups_hardware_first_then_the_users_groups_then_other() {
        let devices = [dev("tv", "LG TV", DeviceKind::Hdmi), dev("loop", "BlackHole", DeviceKind::Virtual), dev("dac", "DAC", DeviceKind::Usb), dev("amp", "Amp", DeviceKind::Usb), dev("mic", "Zoom", DeviceKind::Virtual)];
        let named = HashMap::from([("tv".to_owned(), "Living room".to_owned()), ("amp".to_owned(), "  Bedroom ".to_owned()), ("mic".to_owned(), "   ".to_owned())]);
        let groups = group_devices(devices.iter(), &named);
        let names: Vec<(Option<String>, Vec<String>)> = groups.into_iter().map(|(g, l)| (g, l.into_iter().map(|d| d.name).collect())).collect();
        assert_eq!(
            names,
            [
                (None, vec!["DAC".to_owned()]),
                (Some("Bedroom".to_owned()), vec!["Amp".to_owned()]),
                (Some("Living room".to_owned()), vec!["LG TV".to_owned()]),
                (Some(OTHER.to_owned()), vec!["BlackHole".to_owned(), "Zoom".to_owned()]),
            ]
        );
        assert_eq!(group_of(&devices[3], &named), "Bedroom", "trimmed");
        assert_eq!(group_of(&devices[4], &named), OTHER, "a blank name is no group: back to its own");
        assert_eq!(group_of(&devices[2], &named), DEVICES);
    }

    #[test]
    fn the_engine_hears_no_modes_while_following_the_system() {
        let mut s = crate::settings::Settings::default();
        s.device_modes.insert("dac".into(), DeviceMode::BitPerfect);
        assert!(engine_modes(&s).is_empty());
        s.output_device = Some("dac".into());
        assert_eq!(engine_modes(&s).get("dac"), Some(&PlaybackMode::BitPerfect));
    }
}
