//! Sound cards and their playback devices, from ALSA's control interface,
//! and what a device can do.

use crate::device::DeviceKind;
use crate::signal::{DeviceCaps, RateRange};
use ::alsa::ctl::{Ctl, DeviceIter};
use ::alsa::pcm::{Access, Format, HwParams, PCM};
use ::alsa::{Direction, ValueOr};

/// Rates worth testing a device for.
const RATES: [u32; 13] = [8_000, 11_025, 16_000, 22_050, 32_000, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 352_800, 384_000];
/// More channels than this aren't offered (the engine downmixes past 8).
const MAX_CHANNELS: u32 = 8;

/// One playback device on a card: what `hw:<card>,<device>` opens.
#[derive(Debug, Clone, PartialEq)]
pub struct Pcm {
    /// The card's index: can change when cards come and go.
    pub card: u32,
    pub device: u32,
    /// Stable across reboots and replugging: the card's id string and the device number.
    pub uid: String,
    pub name: String,
    /// The card's own name, for messages about the card as a whole.
    pub card_name: String,
    pub kind: DeviceKind,
}

impl Pcm {
    pub fn hw(&self) -> String {
        format!("hw:{},{}", self.card, self.device)
    }
}

/// Every playback device on every card.
pub fn list() -> Vec<Pcm> {
    let mut out = Vec::new();
    for card in ::alsa::card::Iter::new().filter_map(Result::ok) {
        let Ok(ctl) = Ctl::from_card(&card, false) else { continue };
        let Ok(info) = ctl.card_info() else { continue };
        let id = info.get_id().unwrap_or_default().to_owned();
        let card_name = info.get_name().ok().filter(|n| !n.is_empty()).or_else(|| info.get_longname().ok()).unwrap_or(id.as_str()).to_owned();
        let driver = info.get_driver().unwrap_or_default().to_owned();
        let index = card.get_index() as u32;
        let devices: Vec<(u32, String)> = DeviceIter::new(&ctl)
            .filter_map(|d| {
                let d = u32::try_from(d).ok()?;
                let pcm = ctl.pcm_info(d, 0, Direction::Playback).ok()?;
                Some((d, pcm.get_name().unwrap_or_default().to_owned()))
            })
            .collect();
        let several = devices.len() > 1;
        for (device, pcm_name) in devices {
            let name = if several && !pcm_name.is_empty() { format!("{card_name} · {pcm_name}") } else { card_name.clone() };
            out.push(Pcm { card: index, device, uid: format!("alsa:{id}:{device}"), name, card_name: card_name.clone(), kind: kind_of(&driver, &pcm_name) });
        }
    }
    out
}

fn kind_of(driver: &str, pcm_name: &str) -> DeviceKind {
    let pcm = pcm_name.to_uppercase();
    if driver.contains("USB") {
        DeviceKind::Usb
    } else if pcm.contains("HDMI") || pcm.contains("DISPLAYPORT") {
        DeviceKind::Hdmi
    } else if matches!(driver, "Loopback" | "Dummy" | "Aloop") {
        DeviceKind::Virtual
    } else if driver.contains("HDA") || driver.contains("SOF") {
        DeviceKind::BuiltInOther
    } else {
        DeviceKind::Other
    }
}

/// The integer sample formats a device may take, widest first, with their bits.
pub const FORMATS: [(Format, u32); 4] = [(Format::S32LE, 32), (Format::S24LE, 24), (Format::S243LE, 24), (Format::S16LE, 16)];

/// What an open (but not yet configured) device can do.
pub fn probe(pcm: &PCM, name: &str) -> Option<DeviceCaps> {
    let hwp = HwParams::any(pcm).ok()?;
    hwp.set_access(Access::RWInterleaved).ok()?;
    let (cmin, cmax) = (hwp.get_channels_min().ok()?, hwp.get_channels_max().ok()?.min(MAX_CHANNELS));
    let rates: Vec<u32> = RATES.iter().copied().filter(|r| hwp.test_rate(*r).is_ok()).collect();
    let max_bits = FORMATS.iter().find(|(f, _)| hwp.test_format(*f).is_ok()).map(|(_, b)| *b);
    let mut channels: Vec<u16> = (cmin..=cmax).map(|c| c as u16).collect();
    // A device with more channels plays stereo on its first two.
    if cmin > 2 {
        channels.push(2);
    }
    Some(DeviceCaps { name: name.to_owned(), configs: configs(&channels, &rates), max_bits, ..Default::default() })
}

fn configs(channels: &[u16], rates: &[u32]) -> Vec<RateRange> {
    channels.iter().flat_map(|&c| rates.iter().map(move |&r| RateRange { channels: c, min: r, max: r })).collect()
}

/// What a USB device can do, from the kernel's description of its streams
/// (`/proc/asound/cardN/streamM`), without opening it: it may belong to the
/// sound server right now.
pub fn usb_caps(pcm: &Pcm) -> Option<DeviceCaps> {
    let text = std::fs::read_to_string(format!("/proc/asound/card{}/stream{}", pcm.card, pcm.device)).ok()?;
    parse_usb_stream(&text, &pcm.name)
}

/// Parses the playback half of a `/proc/asound/cardN/streamM` file.
pub(crate) fn parse_usb_stream(text: &str, name: &str) -> Option<DeviceCaps> {
    let playback = text.split("Capture:").next()?;
    let (mut configs, mut max_bits) = (Vec::new(), None::<u32>);
    // Each "Altset" block lists a format, a channel count and its rates.
    for block in playback.split("Altset").skip(1) {
        let field = |key: &str| block.lines().find_map(|l| l.trim().strip_prefix(key).map(str::trim));
        let bits = field("Format:").and_then(|f| FORMATS.iter().find(|(fmt, _)| fmt.to_string() == f).map(|(_, b)| *b));
        let Some(channels) = field("Channels:").and_then(|c| c.parse::<u16>().ok()) else { continue };
        let rates = field("Rates:").map(parse_rates).unwrap_or_default();
        if bits.is_none() || rates.is_empty() {
            continue;
        }
        max_bits = max_bits.max(bits);
        for r in configs_for(channels, &rates) {
            if !configs.contains(&r) {
                configs.push(r);
            }
        }
    }
    (!configs.is_empty()).then(|| DeviceCaps { name: name.to_owned(), configs, max_bits, ..Default::default() })
}

/// A list (`44100, 48000, 96000`) or a range (`44100 - 192000 (continuous)`,
/// read as the common rates within it).
fn parse_rates(text: &str) -> Vec<u32> {
    let text = text.split('(').next().unwrap_or(text);
    if let Some((lo, hi)) = text.split_once('-')
        && let (Ok(lo), Ok(hi)) = (lo.trim().parse::<u32>(), hi.trim().parse::<u32>())
    {
        return RATES.iter().copied().filter(|r| (lo..=hi).contains(r)).collect();
    }
    text.split(',').filter_map(|x| x.trim().parse().ok()).collect()
}

fn configs_for(channels: u16, rates: &[u32]) -> Vec<RateRange> {
    let mut counts = vec![channels];
    if channels > 2 {
        counts.push(2);
    }
    configs(&counts, rates)
}

/// The app that has `pcm` open, from the kernel's status file, if any.
pub fn holder(pcm: &Pcm) -> Option<String> {
    let status = std::fs::read_to_string(format!("/proc/asound/card{}/pcm{}p/sub0/status", pcm.card, pcm.device)).ok()?;
    let pid: u32 = status.lines().find_map(|l| l.strip_prefix("owner_pid")?.trim().trim_start_matches(':').trim().parse().ok())?;
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(comm.trim().to_owned()).filter(|c| !c.is_empty())
}

/// Opens `hw:` playback, retrying briefly while it's busy: a sound server
/// that just agreed to let go may still be closing it.
pub fn open_hw(pcm: &Pcm, patience: std::time::Duration) -> Result<PCM, ::alsa::Error> {
    let deadline = std::time::Instant::now() + patience;
    loop {
        match PCM::new(&pcm.hw(), Direction::Playback, false) {
            Err(e) if e.errno() == libc::EBUSY && std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            other => return other,
        }
    }
}

/// What to say when `pcm` won't open.
pub fn open_error(pcm: &Pcm, e: &::alsa::Error) -> String {
    if e.errno() == libc::EBUSY {
        format!("{} is in use by {}", pcm.name, holder(pcm).unwrap_or_else(|| "another app".into()))
    } else {
        format!("{}: {e}", pcm.name)
    }
}

/// Picks `rate` exactly, else fails.
pub fn set_rate_exactly(hwp: &HwParams, rate: u32) -> bool {
    hwp.set_rate_resample(false).ok();
    hwp.set_rate(rate, ValueOr::Nearest).is_ok() && hwp.get_rate().ok() == Some(rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_usb_stream_file_gives_its_playback_formats() {
        let text = "\
Topping DX3 Pro at usb-0000:00:14.0-2, high speed : USB Audio

Playback:
  Status: Stop
  Interface 1
    Altset 1
    Format: S16_LE
    Channels: 2
    Endpoint: 0x01 (1 OUT) (ASYNC)
    Rates: 44100, 48000, 96000
  Interface 1
    Altset 2
    Format: S32_LE
    Channels: 2
    Rates: 44100, 48000, 88200, 96000, 176400, 192000

Capture:
  Interface 2
    Altset 1
    Format: S24_3LE
    Channels: 2
    Rates: 384000
";
        let caps = parse_usb_stream(text, "DX3").unwrap();
        assert_eq!(caps.max_bits, Some(32));
        assert!(caps.supports(192_000, 2) && caps.supports(44_100, 2));
        assert!(!caps.supports(384_000, 2), "capture rates aren't playback rates");
        assert_eq!(caps.max_rate(), Some(192_000));
    }

    #[test]
    fn rates_as_a_list_or_a_range() {
        assert_eq!(parse_rates("44100, 48000, 96000"), vec![44_100, 48_000, 96_000]);
        assert_eq!(parse_rates("44100 - 96000 (continuous)"), vec![44_100, 48_000, 88_200, 96_000]);
    }

    #[test]
    fn device_kinds() {
        assert_eq!(kind_of("USB-Audio", "USB Audio"), DeviceKind::Usb);
        assert_eq!(kind_of("HDA-Intel", "HDMI 0"), DeviceKind::Hdmi);
        assert_eq!(kind_of("HDA-Intel", "ALC892 Analog"), DeviceKind::BuiltInOther);
        assert_eq!(kind_of("Loopback", "Loopback PCM"), DeviceKind::Virtual);
    }
}
