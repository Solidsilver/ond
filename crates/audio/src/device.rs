//! Output devices as the UI sees them.

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

/// A device's own volume control, in the device: changing it doesn't touch
/// the samples Ond sends. Reported while Ond holds the device in bit-perfect
/// mode, when the device has one Ond may set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeviceVolume {
    /// The level now (the loudest channel's, when they differ).
    pub db: f32,
    pub min_db: f32,
    /// The top of the range, at most 0 dB: above it a device can clip.
    pub max_db: f32,
    /// Muted, when the device has a mute control.
    pub muted: Option<bool>,
}

impl DeviceInfo {
    /// Whether bit-perfect can mean anything here: a device that takes integer
    /// samples, not one macOS processes (speakers), compresses (Bluetooth,
    /// AirPlay) or that's software (virtual devices, meeting apps' drivers).
    pub fn can_bit_perfect(&self) -> bool {
        self.caps.integer
            && !matches!(self.kind, DeviceKind::BuiltInSpeakers | DeviceKind::Bluetooth | DeviceKind::AirPlay | DeviceKind::Virtual)
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
            caps: DeviceCaps { configs: vec![RateRange { channels: 2, min: 44_100, max }], max_bits: bits, integer: true, ..Default::default() },
            is_default: false,
        }
    }

    #[test]
    fn bit_perfect_is_offered_where_it_can_mean_something() {
        assert!(info(DeviceKind::Usb, 192_000, Some(24)).can_bit_perfect());
        assert!(info(DeviceKind::Hdmi, 96_000, Some(24)).can_bit_perfect());
        for kind in [DeviceKind::BuiltInSpeakers, DeviceKind::Bluetooth, DeviceKind::AirPlay, DeviceKind::Virtual] {
            assert!(!info(kind, 96_000, Some(24)).can_bit_perfect(), "{kind:?}");
        }
        let mut jack = info(DeviceKind::BuiltInHeadphones, 96_000, Some(24));
        jack.caps.integer = false;
        assert!(!jack.can_bit_perfect(), "a float-only headphone jack can't be verified");
    }

    #[test]
    fn capability_lines() {
        let speakers = info(DeviceKind::BuiltInSpeakers, 96_000, Some(24));
        assert_eq!(speakers.capability_line(), "up to 24/96 · macOS processing");
        let umc = info(DeviceKind::Usb, 48_000, Some(16));
        assert_eq!(umc.capability_line(), "up to 16/48");
    }
}
