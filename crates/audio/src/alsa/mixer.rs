//! A card's own volume control (its ALSA mixer), for bit-perfect mode: the
//! level changes in the device, the samples Ond sends don't. PipeWire drives
//! the same control for the system volume, which is why it carried over.

use crate::DeviceVolume;
use alsa::mixer::{Mixer, MilliBel, Selem, SelemChannelId};

/// Playback controls that usually are the card's main volume, best first.
const NAMES: [&str; 6] = ["PCM", "Master", "Speaker", "Headphone", "Digital", "Playback"];

/// Runs `f` on card `card`'s main playback volume control, if it has one with a dB range.
fn with_volume<R>(card: u32, f: impl FnOnce(&Selem<'_>) -> R) -> Option<R> {
    let mixer = Mixer::new(&format!("hw:{card}"), false).ok()?;
    let usable = |s: &Selem<'_>| {
        let (min, max) = s.get_playback_db_range();
        s.has_playback_volume() && min.0 < max.0
    };
    let selems: Vec<Selem<'_>> = mixer.iter().filter_map(Selem::new).filter(usable).collect();
    let name = |s: &Selem<'_>| s.get_id().get_name().map(str::to_owned).unwrap_or_default();
    let best = NAMES
        .iter()
        .find_map(|n| selems.iter().position(|s| name(s) == *n))
        .or_else(|| (!selems.is_empty()).then_some(0))?;
    Some(f(&selems[best]))
}

/// The card's own volume: the loudest channel's level, its range (capped at
/// 0 dB), and whether it's muted when it has a switch.
pub fn volume(card: u32) -> Option<DeviceVolume> {
    with_volume(card, |s| {
        let (min, max) = s.get_playback_db_range();
        let db = [SelemChannelId::FrontLeft, SelemChannelId::FrontRight, SelemChannelId::mono()]
            .into_iter()
            .filter(|c| s.has_playback_channel(*c))
            .filter_map(|c| s.get_playback_vol_db(c).ok())
            .map(|v| v.0)
            .max()?;
        // The switch is "on" when sound plays.
        let muted = s.has_playback_switch().then(|| {
            [SelemChannelId::FrontLeft, SelemChannelId::mono()]
                .into_iter()
                .find(|c| s.has_playback_channel(*c))
                .and_then(|c| s.get_playback_switch(c).ok())
                .is_some_and(|on| on == 0)
        });
        Some(DeviceVolume { db: mb(db), min_db: mb(min.0), max_db: mb(max.0).min(0.0), muted })
    })
    .flatten()
}

/// Sets every channel to `db` (within the card's range, at most 0 dB).
pub fn set_volume(card: u32, db: f32) -> Result<(), String> {
    with_volume(card, |s| {
        let (min, max) = s.get_playback_db_range();
        let target = ((db * 100.0).round() as i64).clamp(min.0, max.0.min(0));
        s.set_playback_db_all(MilliBel(target), alsa::Round::Floor).map_err(|e| e.to_string())
    })
    .unwrap_or_else(|| Err("no volume control".into()))
}

pub fn set_mute(card: u32, muted: bool) -> Result<(), String> {
    with_volume(card, |s| {
        if !s.has_playback_switch() {
            return Err("no mute switch".into());
        }
        s.set_playback_switch_all(i32::from(!muted)).map_err(|e| e.to_string())
    })
    .unwrap_or_else(|| Err("no volume control".into()))
}

/// Millibels (hundredths of a dB) to dB.
fn mb(v: i64) -> f32 {
    v as f32 / 100.0
}
