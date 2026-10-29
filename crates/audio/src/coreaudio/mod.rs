//! Native CoreAudio output (macOS).
//!
//! On top of what cpal does, this backend:
//! - lets the user pick a device (by UID) instead of always following the default;
//! - in bit-perfect mode (and in Normal mode when asked to match the song's
//!   rate), switches the device's nominal rate to the ring's rate and waits for
//!   the HAL to confirm before starting, and picks the widest integer physical
//!   format at that rate; otherwise leaves the device exactly as macOS has it
//!   and reports only its current rate, so the decoder resamples to it;
//! - takes hog (exclusive) mode in bit-perfect mode, and releases it on mode
//!   change, close and drop;
//! - puts every device it changed back the way it found it: the format at
//!   once when letting go of exclusive access (a non-mixable format left behind
//!   locks every other app out of the device), the rate and format once the
//!   player has been idle a few seconds, on a device switch and on drop;
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
use objc2_core_audio_types::AudioStreamBasicDescription;
use objc2_core_audio::{
    kAudioDeviceTransportTypeAggregate, kAudioDeviceTransportTypeAirPlay, kAudioDeviceTransportTypeBluetooth,
    kAudioDeviceTransportTypeBluetoothLE, kAudioDeviceTransportTypeBuiltIn, kAudioDeviceTransportTypeDisplayPort,
    kAudioDeviceTransportTypeHDMI, kAudioDeviceTransportTypeUSB, kAudioDeviceTransportTypeVirtual,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// `kAudioOutputUnitProperty_CurrentDevice`.
const CURRENT_DEVICE: u32 = 2000;

/// How long the player stays idle before devices get their own rate and
/// format back. Starting the next track within it doesn't flip the device
/// back and forth.
const IDLE_RESTORE: Duration = Duration::from_secs(5);

/// A device as it was before we first changed it.
#[derive(Clone, Copy)]
struct Original {
    rate: Option<f64>,
    stream: Option<hal::Id>,
    format: Option<AudioStreamBasicDescription>,
}

/// The stream that's playing.
struct Open {
    dev: hal::Id,
    unit: AudioUnit,
    format: StreamFormat,
    /// Integer bits of the physical format in effect (None: float).
    active_bits: Option<u32>,
    /// The same, shared with the render callback (0: float), which dithers to it.
    /// A mode switch can change the physical format under a running stream.
    device_bits: Arc<AtomicU32>,
}

impl Open {
    fn set_bits(&mut self, bits: Option<u32>) {
        self.active_bits = bits;
        self.device_bits.store(bits.unwrap_or(0), Ordering::Relaxed);
    }
}

pub struct CoreAudioBackend {
    name: String,
    /// The user's choice, by UID. `None` follows the system default.
    selected: Option<String>,
    mode: PlaybackMode,
    open: Option<Open>,
    /// Device we hold in hog mode.
    hogged: Option<hal::Id>,
    /// Following the default, the device that was the default when we took it
    /// exclusively. macOS moves the default off a device one app holds; that
    /// move is ours, not the user's, so playback stays here until the user
    /// picks another default.
    pinned_default: Option<hal::Id>,
    /// Normal mode switches the device to the song's rate (true) or leaves it be.
    match_rate: bool,
    /// Devices we've changed, as they were before.
    originals: HashMap<hal::Id, Original>,
    /// Nothing has played since then: devices go back once it's long enough ago.
    idle_since: Option<Instant>,
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
        // An earlier run that crashed (or anything else) may have left a device
        // in a format no one can share: nobody holds it now, so fix it.
        for dev in hal::output_devices().into_iter().filter(|d| hal::hog_pid(*d) == -1) {
            match hal::make_mixable(dev) {
                Ok(true) => tracing::info!("{}: was left in an exclusive-only format; made it shareable", hal::name(dev)),
                Ok(false) => {}
                Err(e) => tracing::warn!("{}: exclusive-only format: {e}", hal::name(dev)),
            }
        }
        Ok(Self {
            name: "CoreAudio".into(),
            selected: None,
            mode: PlaybackMode::Normal,
            open: None,
            hogged: None,
            pinned_default: None,
            match_rate: false,
            originals: HashMap::new(),
            idle_since: None,
            drained: Arc::new(AtomicBool::new(false)),
            devices_changed,
            default_changed,
            _listeners: listeners,
        })
    }

    /// The device to play on: the selected one if it's present, else the default.
    fn resolve(&self) -> Option<hal::Id> {
        let devices = hal::output_devices();
        let pinned = || self.pinned_default.filter(|d| self.selected.is_none() && devices.contains(d));
        self.selected
            .as_deref()
            .and_then(|uid| devices.iter().copied().find(|d| hal::uid(*d).as_deref() == Some(uid)))
            .or_else(pinned)
            .or_else(hal::default_output)
            .or_else(|| devices.first().copied())
    }

    fn release_hog(&mut self) {
        let Some(dev) = self.hogged.take() else { return };
        // While we still hold it: a format only we could use would lock everyone else out.
        if let Err(e) = hal::make_mixable(dev) {
            tracing::warn!("making {} shareable: {e}", hal::name(dev));
        }
        if let Err(e) = hal::set_hog(dev, false) {
            tracing::warn!("releasing exclusive access: {e}");
        }
    }

    /// Notes how `dev` is set up, before the first change we make to it.
    fn remember(&mut self, dev: hal::Id) {
        self.originals.entry(dev).or_insert_with(|| {
            let stream = hal::output_streams(dev).first().copied();
            Original { rate: hal::nominal_rate(dev), stream, format: stream.and_then(hal::physical_format) }
        });
    }

    /// Puts `dev`'s format and rate back the way we found them. Not while we
    /// hold it exclusively, and never into a format nobody can share.
    fn restore(&mut self, dev: hal::Id) {
        let Some(o) = self.originals.remove(&dev) else { return };
        if let (Some(stream), Some(format)) = (o.stream, o.format)
            && hal::is_mixable(&format)
            && let Err(e) = hal::set_physical_format(stream, &format)
        {
            tracing::warn!("restoring {}'s format: {e}", hal::name(dev));
        }
        if let Some(rate) = o.rate
            && let Err(e) = hal::set_nominal_rate(dev, rate.round() as u32)
        {
            tracing::warn!("restoring {}'s rate: {e}", hal::name(dev));
        }
    }

    /// Restores every device we changed, except the one `keep`.
    fn restore_all(&mut self, keep: Option<hal::Id>) {
        let devs: Vec<hal::Id> = self.originals.keys().copied().filter(|d| Some(*d) != keep).collect();
        for dev in devs {
            self.restore(dev);
        }
    }

    /// Normal mode, leaving the device at its own rate and format.
    fn leaves_device_alone(&self) -> bool {
        self.mode == PlaybackMode::Normal && !self.match_rate
    }

    /// Who, other than us, holds `dev` exclusively.
    fn held_by_other(dev: hal::Id) -> Option<String> {
        let pid = hal::hog_pid(dev);
        (pid != -1 && pid != hal::own_pid()).then(|| hal::process_name(pid).unwrap_or_else(|| format!("another app ({pid})")))
    }

    fn take_hog(&mut self, dev: hal::Id) -> bool {
        if self.hogged == Some(dev) {
            return true;
        }
        self.release_hog();
        match hal::set_hog(dev, true) {
            Ok(true) => {
                self.hogged = Some(dev);
                if self.selected.is_none() {
                    self.pinned_default = Some(dev);
                }
                true
            }
            Ok(false) => false,
            Err(e) => {
                tracing::warn!("exclusive access: {e}");
                false
            }
        }
    }

    /// Picks and sets the widest mixable integer physical format at `rate`.
    /// Returns the integer bits in effect, or None when the device stays on a
    /// float format.
    ///
    /// Never a non-mixable format, even holding the device: the AUHAL accepts
    /// one and then never calls the render callback (a UMC22 sat silent at
    /// "Verified"), and one left behind locks every other app out. A mixable
    /// integer format is as exact: the samples arrive on its grid, so the HAL's
    /// float → integer step changes nothing.
    fn set_integer_format(&self, dev: hal::Id, rate: u32, channels: u16) -> Option<u32> {
        let stream = *hal::output_streams(dev).first()?;
        let best = hal::physical_formats(stream)
            .into_iter()
            .filter(|f| {
                let r = rate as f64;
                hal::is_int(&f.mFormat)
                    && f.mFormat.mChannelsPerFrame >= channels as u32
                    && hal::is_mixable(&f.mFormat)
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
        if let Some(holder) = Self::held_by_other(dev) {
            return Err(format!("{holder} is using {} exclusively", hal::name(dev)));
        }
        let active_bits = if self.leaves_device_alone() {
            // The unit converts to whatever rate the device is at.
            hal::output_streams(dev).first().copied().and_then(hal::physical_format).filter(hal::is_int).map(|f| f.mBitsPerChannel)
        } else {
            self.remember(dev);
            hal::set_nominal_rate(dev, format.sample_rate)?;
            self.set_integer_format(dev, format.sample_rate, format.channels)
        };

        let mut unit = AudioUnit::new_uninitialized(IOType::HalOutput).map_err(err("audio unit"))?;
        if let Err(e) = unit.set_property(CURRENT_DEVICE, Scope::Global, Element::Output, Some(&dev)) {
            // A device stuck in a format only one app may use refuses the unit: unstick it and try once more.
            if self.hogged == Some(dev) || !hal::make_mixable(dev).unwrap_or(false) {
                return Err(format!("select device: {e}"));
            }
            unit.set_property(CURRENT_DEVICE, Scope::Global, Element::Output, Some(&dev)).map_err(err("select device"))?;
        }
        let client = coreaudio::audio_unit::StreamFormat {
            sample_rate: format.sample_rate as f64,
            sample_format: SampleFormat::F32,
            flags: LinearPcmFlags::IS_FLOAT | LinearPcmFlags::IS_PACKED,
            channels: format.channels as u32,
        };
        unit.set_stream_format(client, Scope::Input, Element::Output).map_err(err("stream format"))?;

        let drained = self.drained.clone();
        let rate = format.sample_rate;
        let device_bits = Arc::new(AtomicU32::new(active_bits.unwrap_or(0)));
        let mut render = Render { input, status, device_bits: device_bits.clone(), dither: Tpdf::default() };
        unit.set_render_callback(move |args: render_callback::Args<data::Interleaved<f32>>| {
            if render.fill(args.data.buffer, args.data.channels, rate) {
                drained.store(true, Ordering::Release);
            }
            Ok(())
        })
        .map_err(err("render callback"))?;
        unit.initialize().map_err(err("initialize"))?;
        unit.start().map_err(err("start"))?;
        Ok(Open { dev, unit, format, active_bits, device_bits })
    }

    /// Stops the stream, keeping any exclusive access.
    fn stop_unit(&mut self) {
        if let Some(mut open) = self.open.take() {
            // Stopping is synchronous: no render callback runs after it returns.
            let _ = open.unit.stop();
        }
    }

    fn info(&self, dev: hal::Id, default: Option<hal::Id>) -> Option<DeviceInfo> {
        let uid = hal::uid(dev)?;
        let kind = kind_of(dev);
        let mut caps = caps_of(dev, kind);
        caps.exclusive = self.hogged == Some(dev) && hal::hog_pid(dev) == hal::own_pid();
        caps.shared_reason = (!caps.exclusive)
            .then(|| Self::held_by_other(dev).map_or_else(|| "shared mode".into(), |holder| format!("{holder} has the device")));
        if let Some(open) = self.open.as_ref().filter(|o| o.dev == dev) {
            caps.active_bits = open.active_bits;
        }
        Some(DeviceInfo { uid, name: caps.name.clone(), kind, caps, is_default: default == Some(dev) })
    }
}

impl Drop for CoreAudioBackend {
    fn drop(&mut self) {
        self.close();
        self.restore_all(None);
    }
}

/// The realtime half: everything the render callback touches. No allocation, no locks.
struct Render {
    input: RingInput,
    status: Arc<Status>,
    /// Integer bits of the device's physical format, 0 for float. Read on every
    /// callback: a mode switch can change it while the stream runs.
    device_bits: Arc<AtomicU32>,
    dither: Tpdf,
}

impl Render {
    fn fill(&mut self, out: &mut [f32], channels: usize, rate: u32) -> bool {
        let bits = self.device_bits.load(Ordering::Relaxed);
        let bits = (bits != 0).then_some(bits);
        self.input.fill_for_device(out, channels, rate, &self.status, bits, &mut self.dither)
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
        integer: int_bits.is_some(),
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

    fn target_device(&mut self) -> Option<DeviceInfo> {
        let dev = self.resolve()?;
        self.info(dev, hal::default_output())
    }

    /// A UID that isn't plugged in is kept: playback uses the default until it appears.
    fn select_device(&mut self, uid: Option<&str>) -> Result<(), OutputError> {
        self.close();
        self.restore_all(None);
        self.pinned_default = None;
        self.selected = uid.map(str::to_owned);
        Ok(())
    }

    fn poll_device_events(&mut self) -> Vec<DeviceEvent> {
        if self.open.is_none() && self.idle_since.is_some_and(|t| t.elapsed() >= IDLE_RESTORE) {
            self.idle_since = None;
            self.restore_all(None);
        }
        let mut events = Vec::new();
        if self.devices_changed.swap(false, Ordering::AcqRel) {
            events.push(DeviceEvent::ListChanged);
        }
        if self.default_changed.swap(false, Ordering::AcqRel) {
            // While we hold a device, a default change is macOS moving off it: ours.
            // Otherwise it's the user's, unless it went back to where we are.
            if self.hogged.is_none() && self.pinned_default.is_some_and(|d| hal::default_output() != Some(d)) {
                self.pinned_default = None;
            }
            if self.pinned_default.is_none() {
                events.push(DeviceEvent::DefaultChanged);
            }
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
                    o.set_bits(bits);
                    let _ = o.unit.start();
                }
            }
            PlaybackMode::Normal => {
                self.release_hog();
                if !self.match_rate {
                    // The unit converts until the engine restarts the track at the device's own rate.
                    self.restore(dev);
                }
                // The format we had exclusively may not survive sharing: dither to what's in effect.
                let stream = hal::output_streams(dev).first().copied();
                let bits = stream.and_then(hal::physical_format).filter(hal::is_int).map(|f| f.mBitsPerChannel);
                if let Some(o) = self.open.as_mut() {
                    o.set_bits(bits);
                }
            }
            _ => {}
        }
    }

    /// Leaving the device alone, its current rate is the only one on offer.
    fn device_caps(&mut self) -> Option<DeviceCaps> {
        let mut caps = self.current_device()?.caps;
        if self.leaves_device_alone()
            && let Some(rate) = caps.nominal_rate
        {
            let at: Vec<RateRange> =
                caps.configs.iter().filter(|c| c.min <= rate && rate <= c.max).map(|c| RateRange { min: rate, max: rate, ..*c }).collect();
            if !at.is_empty() {
                caps.configs = at;
            }
        }
        Some(caps)
    }

    /// The device's own volume, while we hold it in bit-perfect mode.
    fn device_volume(&mut self) -> Option<crate::DeviceVolume> {
        let dev = self.hogged.filter(|d| self.mode == PlaybackMode::BitPerfect && hal::hog_pid(*d) == hal::own_pid())?;
        let (db, min_db, max_db) = hal::volume_db(dev)?;
        Some(crate::DeviceVolume { db, min_db, max_db: max_db.min(0.0), muted: hal::muted(dev) })
    }

    fn set_device_volume(&mut self, db: f32) {
        let Some(v) = self.device_volume() else { return };
        let dev = self.hogged.expect("held: there's a device volume");
        if let Err(e) = hal::set_volume_db(dev, db.clamp(v.min_db, v.max_db)) {
            tracing::warn!("device volume: {e}");
        }
    }

    fn set_device_mute(&mut self, muted: bool) {
        if self.device_volume().is_none() {
            return;
        }
        let dev = self.hogged.expect("held: there's a device volume");
        if let Err(e) = hal::set_muted(dev, muted) {
            tracing::warn!("device mute: {e}");
        }
    }

    fn set_match_rate(&mut self, on: bool) {
        self.match_rate = on;
        if self.leaves_device_alone() {
            // As in a mode change: the engine restarts the track at the device's own rate.
            let keep = self.hogged;
            self.restore_all(keep);
        }
    }

    fn open(&mut self, format: StreamFormat, input: RingInput, status: Arc<Status>) -> Result<(), OutputError> {
        // Exclusive access carries over when it's the same device in the same mode.
        self.stop_unit();
        self.idle_since = None;
        self.drained = Arc::new(AtomicBool::new(false));
        let dev = self.resolve().ok_or(OutputError::NoDevice)?;
        // Playback moved on from any other device we changed.
        self.restore_all(Some(dev));
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
        self.stop_unit();
        self.release_hog();
        self.idle_since = Some(Instant::now());
    }

    /// Keeps exclusive access for the `open` that follows: releasing and
    /// retaking it would only add HAL notifications (and possibly pops).
    fn close_for_reopen(&mut self) {
        self.stop_unit();
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
        let mut render = Render { input, status, device_bits: Arc::new(AtomicU32::new(bits)), dither: Tpdf::default() };
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

    #[test]
    fn the_callback_dithers_to_the_format_in_effect_now() {
        // 24-bit audio, bit-perfect: a 16-bit device needs it requantized, a 24-bit one doesn't.
        let samples: Vec<f32> =
            (0..1_024i64).map(|i| ((i * 2_654_435) % (1 << 24) - (1 << 23)) as f32 / (1 << 23) as f32).collect();
        let (mut p, c) = rtrb::RingBuffer::new(samples.len());
        p.push_entire_slice(&samples).unwrap();
        let (_m, mc) = rtrb::RingBuffer::new(4);
        let input = RingInput::new(c, mc, Arc::new(AtomicBool::new(true)), Arc::default()).with_source(Some(24), false);
        let status = Arc::new(Status::default());
        status.bit_perfect.store(true, Ordering::Relaxed);
        let bits = Arc::new(AtomicU32::new(16));
        let mut render = Render { input, status, device_bits: bits.clone(), dither: Tpdf::default() };
        let on_grid = |s: &[f32], bits: u32| s.iter().all(|v| (*v * (1u32 << (bits - 1)) as f32).fract() == 0.0);
        let mut out = vec![0.0f32; 512];
        render.fill(&mut out, 2, 96_000);
        assert!(on_grid(&out, 16), "16-bit device: requantized");
        // Taking the device exclusively switched it to 24 bits mid-stream.
        bits.store(24, Ordering::Relaxed);
        render.fill(&mut out, 2, 96_000);
        let exact = out.iter().zip(&samples[512..]).all(|(a, b)| a.to_bits() == b.to_bits());
        assert!(exact, "24-bit device: untouched");
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

    /// Needs real hardware: the whole engine on the device named in
    /// `OND_TEST_DEVICE` (by UID) in bit-perfect mode, as the app drives it,
    /// playing near-silence. Prints every event and the position as it goes.
    #[test]
    #[ignore = "needs audio hardware"]
    fn engine_plays_bit_perfect_on_a_real_device() {
        let uid = std::env::var("OND_TEST_DEVICE").expect("OND_TEST_DEVICE");
        let dir = tempfile::tempdir().unwrap();
        let quiet = vec![1i16; 44_100 * 2 * 6];
        let src = crate::decode::tests::file_source(&dir, "quiet", &quiet);
        let engine = crate::spawn(crate::EngineConfig::default(), Box::new(CoreAudioBackend::new().unwrap()), None);
        // OND_TEST_SWITCH: start in Normal and switch to bit-perfect mid-track, as the picker does.
        let switch = std::env::var("OND_TEST_SWITCH").is_ok();
        let first = if switch { PlaybackMode::Normal } else { PlaybackMode::BitPerfect };
        // OND_TEST_FOLLOW: make the device the system default and follow the default.
        let follow = std::env::var("OND_TEST_FOLLOW").is_ok();
        let old_default = hal::default_output();
        if follow {
            let dev = hal::output_devices().into_iter().find(|d| hal::uid(*d).as_deref() == Some(&uid)).expect("device");
            hal::set_default_output(dev).unwrap();
        }
        engine.send(crate::Command::DeviceModes([(uid.clone(), first)].into()));
        engine.send(crate::Command::SelectDevice((!follow).then_some(uid)));
        engine.send(crate::Command::Play(src));
        let start = std::time::Instant::now();
        let mut last = -1.0;
        let mut switched = !switch;
        while start.elapsed() < std::time::Duration::from_secs(5) {
            if !switched && start.elapsed() > std::time::Duration::from_millis(1500) {
                println!("{:>5} ms  -- SetMode(BitPerfect)", start.elapsed().as_millis());
                engine.send(crate::Command::SetMode(PlaybackMode::BitPerfect));
                switched = true;
            }
            while let Ok(ev) = engine.events.try_recv() {
                println!("{:>5} ms  {ev:?}", start.elapsed().as_millis());
            }
            let pos = engine.status.position_secs();
            if (pos - last).abs() > 0.4 {
                println!("{:>5} ms  position {pos:.2} state {:?}", start.elapsed().as_millis(), engine.status.state());
                last = pos;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        println!("final position {:.2}", engine.status.position_secs());
        engine.shutdown(std::time::Duration::from_secs(2));
        if follow && let Some(d) = old_default {
            let _ = hal::set_default_output(d);
        }
    }

    /// Needs real hardware: plays 1 s of silence bit-perfectly on the default
    /// device (or the one named in `OND_TEST_DEVICE`, by UID), prints the
    /// verdict, and checks the device ends up as it was. `OND_TEST_RATE` picks
    /// the rate (44100 by default). Run with `--ignored --nocapture`.
    #[test]
    #[ignore = "needs audio hardware"]
    fn bit_perfect_on_the_default_device() {
        let mut b = CoreAudioBackend::new().unwrap();
        if let Ok(uid) = std::env::var("OND_TEST_DEVICE") {
            b.select_device(Some(&uid)).unwrap();
        }
        let rate: u32 = std::env::var("OND_TEST_RATE").ok().and_then(|r| r.parse().ok()).unwrap_or(44_100);
        let dev = b.resolve().unwrap();
        let stream = hal::output_streams(dev)[0];
        let before = (hal::nominal_rate(dev), hal::physical_format(stream).map(|f| (f.mSampleRate, f.mFormatFlags, f.mBitsPerChannel)));
        b.set_mode(PlaybackMode::BitPerfect);
        let format = StreamFormat { sample_rate: rate, channels: 2, bits_per_sample: Some(16) };
        let (mut p, c) = rtrb::RingBuffer::new(rate as usize * 2);
        p.push_entire_slice(&vec![0.0f32; rate as usize * 2]).unwrap();
        let (_m, mc) = rtrb::RingBuffer::new(4);
        let input = RingInput::new(c, mc, Arc::new(AtomicBool::new(true)), Arc::default()).with_source(Some(16), false);
        let status = Arc::new(Status::default());
        status.bit_perfect.store(true, Ordering::Relaxed);
        b.open(format, input, status).unwrap();
        let caps = b.device_caps().unwrap();
        let during = hal::physical_format(stream).map(|f| (f.mSampleRate, f.mFormatFlags, f.mBitsPerChannel));
        let volume = b.device_volume();
        println!("device volume while held: {volume:?}");
        if let Some(v) = volume {
            // Nudge it and put it back: the device's own control answers.
            b.set_device_volume(v.db - 1.0);
            println!("after −1 dB: {:?}", b.device_volume());
            b.set_device_volume(v.db);
        }
        println!("{}: {:?}\nduring {during:?}", b.name(), crate::assess(PlaybackMode::BitPerfect, &format, rate, Some(&caps)));
        std::thread::sleep(std::time::Duration::from_secs(1));
        b.close();
        assert!(b.hogged.is_none(), "closing (playback stopped) releases exclusive access");
        assert!(hal::physical_format(stream).is_some_and(|f| hal::is_mixable(&f)), "nothing exclusive-only left behind");
        b.set_mode(PlaybackMode::Normal);
        drop(b);
        let after = (hal::nominal_rate(dev), hal::physical_format(stream).map(|f| (f.mSampleRate, f.mFormatFlags, f.mBitsPerChannel)));
        println!("before {before:?}\nafter  {after:?}");
        assert_eq!(after, before, "the device is back the way it was");
    }
}
