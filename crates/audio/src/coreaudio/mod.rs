//! Native CoreAudio output (macOS).
//!
//! On top of what cpal does, this backend:
//! - lets the user pick a device (by UID) instead of always following the default;
//! - switches the device's nominal rate to the ring's rate and waits for the
//!   HAL to confirm before starting;
//! - picks the widest integer physical format at that rate;
//! - takes hog (exclusive) mode in bit-perfect mode, and releases it on mode
//!   change, close and drop;
//! - reports enough state for [`assess`](crate::assess) to call a path `Verified`.
//!
//! The render callback asks the AUHAL for 32-bit float, interleaved, at the
//! device's own rate, so the unit does no rate conversion. Float holds any
//! 16- or 24-bit integer exactly, and when the signal isn't already on the
//! device's integer grid we requantize it with TPDF dither first, so the HAL's
//! float → integer step is always exact.
//!
//! Crate choice: `coreaudio-rs` (RustAudio, what cpal uses) for the AudioUnit
//! and its render callback, and its own bindings, `objc2-core-audio`, for HAL
//! properties it doesn't wrap (stream physical formats, hog mode, UIDs,
//! listeners). Same versions as cpal's, so nothing is duplicated.

mod hal;

use crate::device::{DeviceEvent, DeviceInfo, DeviceKind};
use crate::dither::Tpdf;
use crate::output::{OutputBackend, OutputError, RingInput, StreamFormat};
use crate::signal::{DeviceCaps, RateRange};
use crate::{PlaybackMode, Status};
use coreaudio::audio_unit::audio_format::LinearPcmFlags;
use coreaudio::audio_unit::render_callback::{self, data};
use coreaudio::audio_unit::{AudioUnit, Element, IOType, SampleFormat, Scope};
use objc2_core_audio::{
    kAudioDeviceTransportTypeAggregate, kAudioDeviceTransportTypeAirPlay, kAudioDeviceTransportTypeBluetooth,
    kAudioDeviceTransportTypeBluetoothLE, kAudioDeviceTransportTypeBuiltIn, kAudioDeviceTransportTypeDisplayPort,
    kAudioDeviceTransportTypeHDMI, kAudioDeviceTransportTypeUSB, kAudioDeviceTransportTypeVirtual,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// `kAudioOutputUnitProperty_CurrentDevice`.
const CURRENT_DEVICE: u32 = 2000;

/// The stream that's playing.
struct Open {
    dev: hal::Id,
    unit: AudioUnit,
    format: StreamFormat,
    /// Integer bits of the physical format in effect (None: float).
    active_bits: Option<u32>,
}

pub struct CoreAudioBackend {
    name: String,
    /// The user's choice, by UID. `None` follows the system default.
    selected: Option<String>,
    mode: PlaybackMode,
    open: Option<Open>,
    /// Device we hold in hog mode.
    hogged: Option<hal::Id>,
    drained: Arc<AtomicBool>,
    devices_changed: Arc<AtomicBool>,
    default_changed: Arc<AtomicBool>,
    _listeners: Vec<hal::Listener>,
}

impl CoreAudioBackend {
    /// Fails if the system has no output device at all.
    pub fn new() -> Result<Self, OutputError> {
        if hal::output_devices().is_empty() {
            return Err(OutputError::NoDevice);
        }
        let devices_changed = Arc::new(AtomicBool::new(false));
        let default_changed = Arc::new(AtomicBool::new(false));
        let (d, f) = (devices_changed.clone(), default_changed.clone());
        let listeners =
            hal::watch_system(move || d.store(true, Ordering::Release), move || f.store(true, Ordering::Release));
        Ok(Self {
            name: "CoreAudio".into(),
            selected: None,
            mode: PlaybackMode::Normal,
            open: None,
            hogged: None,
            drained: Arc::new(AtomicBool::new(false)),
            devices_changed,
            default_changed,
            _listeners: listeners,
        })
    }

    /// The device to play on: the selected one if it's present, else the default.
    fn resolve(&self) -> Option<hal::Id> {
        let devices = hal::output_devices();
        self.selected
            .as_deref()
            .and_then(|uid| devices.iter().copied().find(|d| hal::uid(*d).as_deref() == Some(uid)))
            .or_else(hal::default_output)
            .or_else(|| devices.first().copied())
    }

    fn release_hog(&mut self) {
        if let Some(dev) = self.hogged.take()
            && let Err(e) = hal::set_hog(dev, false)
        {
            tracing::warn!("releasing exclusive access: {e}");
        }
    }

    fn take_hog(&mut self, dev: hal::Id) -> bool {
        if self.hogged == Some(dev) {
            return true;
        }
        self.release_hog();
        match hal::set_hog(dev, true) {
            Ok(true) => {
                self.hogged = Some(dev);
                true
            }
            Ok(false) => false,
            Err(e) => {
                tracing::warn!("exclusive access: {e}");
                false
            }
        }
    }

    /// Picks and sets the widest integer physical format at `rate` (mixable only,
    /// unless we hold the device). Returns the integer bits in effect, or None
    /// when the device stays on a float format.
    fn set_integer_format(&self, dev: hal::Id, rate: u32, channels: u16) -> Option<u32> {
        let exclusive = self.hogged == Some(dev);
        let stream = *hal::output_streams(dev).first()?;
        let best = hal::physical_formats(stream)
            .into_iter()
            .filter(|f| {
                let r = rate as f64;
                hal::is_int(&f.mFormat)
                    && f.mFormat.mChannelsPerFrame >= channels as u32
                    && (exclusive || hal::is_mixable(&f.mFormat))
                    && (f.mSampleRateRange.mMinimum <= r && r <= f.mSampleRateRange.mMaximum
                        || f.mFormat.mSampleRate as u32 == rate)
            })
            .max_by_key(|f| f.mFormat.mBitsPerChannel);
        if let Some(f) = best {
            let mut format = f.mFormat;
            format.mSampleRate = rate as f64;
            if let Err(e) = hal::set_physical_format(stream, &format) {
                tracing::warn!("integer format: {e}");
            }
        }
        hal::physical_format(stream).filter(hal::is_int).map(|f| f.mBitsPerChannel)
    }

    fn start_unit(&mut self, dev: hal::Id, format: StreamFormat, input: RingInput, status: Arc<Status>) -> Result<Open, String> {
        fn err(what: &'static str) -> impl Fn(coreaudio::Error) -> String {
            move |e| format!("{what}: {e}")
        }
        hal::set_nominal_rate(dev, format.sample_rate)?;
        let active_bits = self.set_integer_format(dev, format.sample_rate, format.channels);

        let mut unit = AudioUnit::new_uninitialized(IOType::HalOutput).map_err(err("audio unit"))?;
        unit.set_property(CURRENT_DEVICE, Scope::Global, Element::Output, Some(&dev)).map_err(err("select device"))?;
        let client = coreaudio::audio_unit::StreamFormat {
            sample_rate: format.sample_rate as f64,
            sample_format: SampleFormat::F32,
            flags: LinearPcmFlags::IS_FLOAT | LinearPcmFlags::IS_PACKED,
            channels: format.channels as u32,
        };
        unit.set_stream_format(client, Scope::Input, Element::Output).map_err(err("stream format"))?;

        let drained = self.drained.clone();
        let rate = format.sample_rate;
        let mut render = Render { input, status, device_bits: active_bits, dither: Tpdf::default() };
        unit.set_render_callback(move |args: render_callback::Args<data::Interleaved<f32>>| {
            if render.fill(args.data.buffer, args.data.channels, rate) {
                drained.store(true, Ordering::Release);
            }
            Ok(())
        })
        .map_err(err("render callback"))?;
        unit.initialize().map_err(err("initialize"))?;
        unit.start().map_err(err("start"))?;
        Ok(Open { dev, unit, format, active_bits })
    }

    fn info(&self, dev: hal::Id, default: Option<hal::Id>) -> Option<DeviceInfo> {
        let uid = hal::uid(dev)?;
        let kind = kind_of(dev);
        let mut caps = caps_of(dev, kind);
        caps.exclusive = self.hogged == Some(dev) && hal::hog_pid(dev) == hal::own_pid();
        caps.shared_reason = (!caps.exclusive).then(|| "shared mode".into());
        if let Some(open) = self.open.as_ref().filter(|o| o.dev == dev) {
            caps.active_bits = open.active_bits;
        }
        Some(DeviceInfo { uid, name: caps.name.clone(), kind, caps, is_default: default == Some(dev) })
    }
}

impl Drop for CoreAudioBackend {
    fn drop(&mut self) {
        self.close();
        self.release_hog();
    }
}

/// The realtime half: everything the render callback touches. No allocation, no locks.
struct Render {
    input: RingInput,
    status: Arc<Status>,
    device_bits: Option<u32>,
    dither: Tpdf,
}

impl Render {
    fn fill(&mut self, out: &mut [f32], channels: usize, rate: u32) -> bool {
        self.input.fill_for_device(out, channels, rate, &self.status, self.device_bits, &mut self.dither)
    }
}

fn kind_of(dev: hal::Id) -> DeviceKind {
    #[allow(non_upper_case_globals)]
    match hal::transport(dev) {
        kAudioDeviceTransportTypeBuiltIn => {
            let name = hal::name(dev).to_lowercase();
            match hal::data_source(dev) {
                Some(hal::SOURCE_SPEAKERS) => DeviceKind::BuiltInSpeakers,
                Some(hal::SOURCE_HEADPHONES) => DeviceKind::BuiltInHeadphones,
                _ if name.contains("speaker") => DeviceKind::BuiltInSpeakers,
                _ if name.contains("headphone") => DeviceKind::BuiltInHeadphones,
                _ => DeviceKind::BuiltInOther,
            }
        }
        kAudioDeviceTransportTypeUSB => DeviceKind::Usb,
        kAudioDeviceTransportTypeBluetooth | kAudioDeviceTransportTypeBluetoothLE => DeviceKind::Bluetooth,
        kAudioDeviceTransportTypeAirPlay => DeviceKind::AirPlay,
        kAudioDeviceTransportTypeHDMI | kAudioDeviceTransportTypeDisplayPort => DeviceKind::Hdmi,
        kAudioDeviceTransportTypeVirtual | kAudioDeviceTransportTypeAggregate => DeviceKind::Virtual,
        _ => DeviceKind::Other,
    }
}

/// What the device can do, from its first output stream's physical formats.
fn caps_of(dev: hal::Id, kind: DeviceKind) -> DeviceCaps {
    let stream = hal::output_streams(dev).first().copied();
    let formats = stream.map(hal::physical_formats).unwrap_or_default();
    let mut configs: Vec<RateRange> = Vec::new();
    let (mut int_bits, mut float) = (None::<u32>, false);
    for f in formats.iter().filter(|f| hal::is_lpcm(&f.mFormat)) {
        let range = RateRange {
            channels: f.mFormat.mChannelsPerFrame as u16,
            min: f.mSampleRateRange.mMinimum as u32,
            max: f.mSampleRateRange.mMaximum as u32,
        };
        // A stream with more channels plays stereo on its first two.
        for channels in [range.channels, 2, 1].into_iter().filter(|c| *c <= range.channels) {
            let r = RateRange { channels, ..range };
            if !configs.contains(&r) {
                configs.push(r);
            }
        }
        if hal::is_int(&f.mFormat) {
            int_bits = int_bits.max(Some(f.mFormat.mBitsPerChannel));
        } else {
            float = true;
        }
    }
    if configs.is_empty() {
        // No physical formats reported: fall back to the device's nominal rates.
        configs = hal::available_rates(dev)
            .iter()
            .map(|r| RateRange { channels: 2, min: r.mMinimum as u32, max: r.mMaximum as u32 })
            .collect();
    }
    let active = stream.and_then(hal::physical_format);
    DeviceCaps {
        name: hal::name(dev),
        configs,
        // Float-only devices carry 24 bits exactly.
        max_bits: int_bits.or(float.then_some(24)),
        exclusive: false,
        shared_reason: None,
        nominal_rate: hal::nominal_rate(dev).map(|r| r.round() as u32),
        active_bits: active.filter(hal::is_int).map(|f| f.mBitsPerChannel),
        post_processed: (kind == DeviceKind::BuiltInSpeakers).then(|| "macOS speaker processing".into()),
    }
}

impl OutputBackend for CoreAudioBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn devices(&mut self) -> Vec<DeviceInfo> {
        let default = hal::default_output();
        hal::output_devices().into_iter().filter_map(|d| self.info(d, default)).collect()
    }

    fn current_device(&mut self) -> Option<DeviceInfo> {
        let dev = self.open.as_ref().map(|o| o.dev).or_else(|| self.resolve())?;
        self.info(dev, hal::default_output())
    }

    /// A UID that isn't plugged in is kept: playback uses the default until it appears.
    fn select_device(&mut self, uid: Option<&str>) -> Result<(), OutputError> {
        self.close();
        self.release_hog();
        self.selected = uid.map(str::to_owned);
        Ok(())
    }

    fn poll_device_events(&mut self) -> Vec<DeviceEvent> {
        let mut events = Vec::new();
        if self.devices_changed.swap(false, Ordering::AcqRel) {
            events.push(DeviceEvent::ListChanged);
        }
        if self.default_changed.swap(false, Ordering::AcqRel) {
            events.push(DeviceEvent::DefaultChanged);
        }
        events
    }

    fn set_mode(&mut self, mode: PlaybackMode) {
        self.mode = mode;
        let Some(open) = self.open.as_ref() else {
            if mode == PlaybackMode::Normal {
                self.release_hog();
            }
            return;
        };
        let (dev, format) = (open.dev, open.format);
        match mode {
            PlaybackMode::BitPerfect if self.hogged != Some(dev) => {
                // Pause the unit around taking the device, then settle the integer format.
                if let Some(o) = self.open.as_mut() {
                    let _ = o.unit.stop();
                }
                self.take_hog(dev);
                let bits = self.set_integer_format(dev, format.sample_rate, format.channels);
                if let Some(o) = self.open.as_mut() {
                    o.active_bits = bits;
                    let _ = o.unit.start();
                }
            }
            PlaybackMode::Normal => self.release_hog(),
            _ => {}
        }
    }

    fn device_caps(&mut self) -> Option<DeviceCaps> {
        self.current_device().map(|d| d.caps)
    }

    fn open(&mut self, format: StreamFormat, input: RingInput, status: Arc<Status>) -> Result<(), OutputError> {
        self.close();
        self.drained = Arc::new(AtomicBool::new(false));
        let dev = self.resolve().ok_or(OutputError::NoDevice)?;
        // The engine's mode flag is the source of truth when a stream opens.
        if status.bit_perfect.load(Ordering::Relaxed) {
            self.mode = PlaybackMode::BitPerfect;
            self.take_hog(dev);
        } else {
            self.mode = PlaybackMode::Normal;
            self.release_hog();
        }
        let open = match self.start_unit(dev, format, input, status) {
            Ok(o) => o,
            Err(e) => {
                // Nothing is playing: don't keep other apps off the device.
                self.release_hog();
                return Err(OutputError::Device(e));
            }
        };
        self.name = format!("CoreAudio · {}", hal::name(dev));
        self.open = Some(open);
        Ok(())
    }

    fn drained(&self) -> bool {
        self.drained.load(Ordering::Acquire)
    }

    /// Stops the stream and lets go of the device: an idle player never keeps
    /// other apps (or system sounds) off it. The next `open` takes it again.
    fn close(&mut self) {
        if let Some(mut open) = self.open.take() {
            // Stopping is synchronous: no render callback runs after it returns.
            let _ = open.unit.stop();
        }
        self.release_hog();
    }
}

/// Feeds a known 24-bit pattern through the exact render path bit-perfect mode
/// uses (ring → callback fill → dither decision) into a buffer instead of the
/// hardware, then converts it the way the HAL does for an integer device and
/// compares bit for bit. Also checks that a resampled ring *is* requantized.
#[cfg(test)]
fn bit_perfect_self_check() -> Result<(), String> {
    let frames = 4_096;
    // Spread over the whole 24-bit range, both signs, odd values included.
    let pattern: Vec<i32> = (0..frames as i64 * 2).map(|i| ((i * 2_654_435) % (1 << 24) - (1 << 23)) as i32).collect();
    let samples: Vec<f32> = pattern.iter().map(|v| *v as f32 / (1 << 23) as f32).collect();

    let run = |bits: u32, resampled: bool| -> Vec<f32> {
        let (mut p, c) = rtrb::RingBuffer::new(samples.len());
        p.push_entire_slice(&samples).expect("ring fits the pattern");
        let (_m, mc) = rtrb::RingBuffer::new(4);
        let input = RingInput::new(c, mc, Arc::new(AtomicBool::new(true)), Arc::default()).with_source(Some(24), resampled);
        let status = Arc::new(Status::default());
        status.bit_perfect.store(true, Ordering::Relaxed);
        let mut render = Render { input, status, device_bits: Some(bits), dither: Tpdf::default() };
        let mut out = vec![0.0f32; samples.len()];
        for chunk in out.chunks_mut(512) {
            render.fill(chunk, 2, 96_000);
        }
        out
    };

    let out = run(24, false);
    let back: Vec<i32> = out.iter().map(|s| (*s as f64 * (1 << 23) as f64) as i32).collect();
    if let Some(i) = back.iter().zip(&pattern).position(|(a, b)| a != b) {
        return Err(format!("sample {i}: wrote {}, expected {}", back[i], pattern[i]));
    }
    if run(24, true) == out {
        return Err("a resampled ring wasn't requantized".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_check_passes() {
        bit_perfect_self_check().unwrap();
    }

    /// Needs real hardware: lists output devices and what we'd report for them.
    #[test]
    #[ignore = "needs audio hardware"]
    fn list_devices() {
        let mut b = CoreAudioBackend::new().unwrap();
        for d in b.devices() {
            println!(
                "{} [{}] {:?} default={} mode={:?} · {} · rate {:?} active {:?} bits",
                d.name,
                d.uid,
                d.kind,
                d.is_default,
                d.default_mode(),
                d.capability_line(),
                d.caps.nominal_rate,
                d.caps.active_bits
            );
        }
    }

    /// Needs real hardware: plays 1 s of silence bit-perfectly on the default
    /// device and prints the verdict. Run with `--ignored --nocapture`.
    #[test]
    #[ignore = "needs audio hardware"]
    fn bit_perfect_on_the_default_device() {
        let mut b = CoreAudioBackend::new().unwrap();
        b.set_mode(PlaybackMode::BitPerfect);
        let format = StreamFormat { sample_rate: 44_100, channels: 2, bits_per_sample: Some(16) };
        let (mut p, c) = rtrb::RingBuffer::new(44_100 * 2);
        p.push_entire_slice(&vec![0.0f32; 44_100 * 2]).unwrap();
        let (_m, mc) = rtrb::RingBuffer::new(4);
        let input = RingInput::new(c, mc, Arc::new(AtomicBool::new(true)), Arc::default()).with_source(Some(16), false);
        let status = Arc::new(Status::default());
        status.bit_perfect.store(true, Ordering::Relaxed);
        b.open(format, input, status).unwrap();
        let caps = b.device_caps().unwrap();
        println!("{}: {:?}", b.name(), crate::assess(PlaybackMode::BitPerfect, &format, 44_100, Some(&caps)));
        std::thread::sleep(std::time::Duration::from_secs(1));
        b.close();
        assert!(b.hogged.is_none(), "closing (playback stopped) releases exclusive access");
        b.set_mode(PlaybackMode::Normal);
    }
}
