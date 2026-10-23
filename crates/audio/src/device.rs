//! Output devices as the UI sees them.

use crate::PlaybackMode;
use crate::signal::{DeviceCaps, describe};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    /// The laptop's own speakers: macOS applies its speaker processing after us.
    BuiltInSpeakers,
    /// The built-in headphone jack (a separate device on Apple silicon).
    BuiltInHeadphones,
    BuiltInOther,
    Usb,
    Bluetooth,
    AirPlay,
    Hdmi,
    Virtual,
    Other,
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// Stable across reboots and replugging (CoreAudio device UID).
    pub uid: String,
    pub name: String,
    pub kind: DeviceKind,
    pub caps: DeviceCaps,
    pub is_default: bool,
}

impl DeviceInfo {
    /// The mode a device starts in until the user picks one: Normal where
    /// bit-perfect can't mean anything (speaker DSP, lossy wireless links),
    /// bit-perfect for DACs, headphone jacks and other wired outputs.
    pub fn default_mode(&self) -> PlaybackMode {
        match self.kind {
            DeviceKind::BuiltInSpeakers | DeviceKind::Bluetooth | DeviceKind::AirPlay | DeviceKind::Virtual => {
                PlaybackMode::Normal
            }
            _ => PlaybackMode::BitPerfect,
        }
    }

    /// A terse capability line for the picker: "up to 24/96", "AirPlay".
    pub fn capability_line(&self) -> String {
        let limit = self.caps.max_rate().map(|r| format!("up to {}", describe(self.caps.max_bits, r)));
        let note = match self.kind {
            DeviceKind::BuiltInSpeakers => Some("macOS processing"),
            DeviceKind::Bluetooth => Some("Bluetooth"),
            DeviceKind::AirPlay => Some("AirPlay"),
            _ => None,
        };
        match (limit, note) {
            (Some(l), Some(n)) => format!("{l} · {n}"),
            (Some(l), None) => l,
            (None, Some(n)) => n.into(),
            (None, None) => String::new(),
        }
    }
}

/// Something changed in the system's device list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceEvent {
    /// Devices were added or removed.
    ListChanged,
    /// The system default output changed (e.g. headphones plugged in).
    DefaultChanged,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::RateRange;

    fn info(kind: DeviceKind, max: u32, bits: Option<u32>) -> DeviceInfo {
        DeviceInfo {
            uid: "x".into(),
            name: "x".into(),
            kind,
            caps: DeviceCaps { configs: vec![RateRange { channels: 2, min: 44_100, max }], max_bits: bits, ..Default::default() },
            is_default: false,
        }
    }

    #[test]
    fn defaults_and_capability_lines() {
        let speakers = info(DeviceKind::BuiltInSpeakers, 96_000, Some(24));
        assert_eq!(speakers.default_mode(), PlaybackMode::Normal);
        assert_eq!(speakers.capability_line(), "up to 24/96 · macOS processing");
        let umc = info(DeviceKind::Usb, 48_000, Some(16));
        assert_eq!(umc.default_mode(), PlaybackMode::BitPerfect);
        assert_eq!(umc.capability_line(), "up to 16/48");
        assert_eq!(info(DeviceKind::BuiltInHeadphones, 96_000, Some(24)).default_mode(), PlaybackMode::BitPerfect);
    }
}
