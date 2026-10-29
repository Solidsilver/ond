//! Thin, safe-ish wrappers over the CoreAudio HAL property API.
//!
//! Everything here runs on the engine thread (never in the render callback).

use objc2_core_audio::{
    AudioObjectAddPropertyListener, AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
    AudioObjectPropertyAddress, AudioObjectPropertyScope, AudioObjectPropertySelector, AudioObjectRemovePropertyListener,
    AudioObjectHasProperty, AudioObjectIsPropertySettable, AudioObjectSetPropertyData, AudioStreamRangedDescription,
    kAudioDevicePropertyAvailableNominalSampleRates, kAudioDevicePropertyMute, kAudioDevicePropertyVolumeDecibels,
    kAudioDevicePropertyVolumeRangeDecibels,
    kAudioDevicePropertyDataSource, kAudioDevicePropertyDeviceUID, kAudioDevicePropertyHogMode,
    kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertyStreams, kAudioDevicePropertyTransportType,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioHardwarePropertyDevices, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject,
    kAudioStreamPropertyAvailablePhysicalFormats, kAudioStreamPropertyPhysicalFormat,
};
use objc2_core_audio_types::{
    AudioStreamBasicDescription, AudioValueRange, kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonMixable,
    kAudioFormatFlagIsSignedInteger, kAudioFormatLinearPCM,
};
use objc2_core_foundation::{CFRetained, CFString};
use std::ffi::c_void;
use std::mem::{MaybeUninit, size_of};
use std::ptr::{NonNull, null};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

pub type Id = AudioObjectID;

pub const SYSTEM: Id = kAudioObjectSystemObject as Id;
const GLOBAL: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal;
const OUTPUT: AudioObjectPropertyScope = kAudioObjectPropertyScopeOutput;
/// Built-in output data sources: 'ispk' (internal speakers) and 'hdpn' (headphones).
pub const SOURCE_SPEAKERS: u32 = u32::from_be_bytes(*b"ispk");
pub const SOURCE_HEADPHONES: u32 = u32::from_be_bytes(*b"hdpn");

pub type Result<T> = std::result::Result<T, String>;

fn addr(selector: AudioObjectPropertySelector, scope: AudioObjectPropertyScope) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress { mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain }
}

fn addr_at(selector: AudioObjectPropertySelector, scope: AudioObjectPropertyScope, element: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress { mSelector: selector, mScope: scope, mElement: element }
}

fn check(status: i32, what: &str) -> Result<()> {
    if status == 0 { Ok(()) } else { Err(format!("{what}: OSStatus {status}")) }
}

/// Reads a fixed-size property.
fn get<T: Copy>(obj: Id, selector: u32, scope: u32) -> Result<T> {
    let a = addr(selector, scope);
    let mut value = MaybeUninit::<T>::zeroed();
    let mut size = size_of::<T>() as u32;
    // SAFETY: the HAL writes at most `size` bytes into `value`, a plain-data T.
    let status = unsafe {
        AudioObjectGetPropertyData(obj, NonNull::from(&a), 0, null(), NonNull::from(&mut size), NonNull::from(&mut value).cast())
    };
    check(status, "get property")?;
    // SAFETY: zero-initialised and (partly) overwritten by the HAL; T is plain data.
    Ok(unsafe { value.assume_init() })
}

/// Reads an array property.
fn get_vec<T: Copy>(obj: Id, selector: u32, scope: u32) -> Result<Vec<T>> {
    let a = addr(selector, scope);
    let mut size = 0u32;
    // SAFETY: valid pointers to stack values.
    let status = unsafe { AudioObjectGetPropertyDataSize(obj, NonNull::from(&a), 0, null(), NonNull::from(&mut size)) };
    check(status, "get property size")?;
    let n = size as usize / size_of::<T>();
    let mut v: Vec<T> = Vec::with_capacity(n);
    if n == 0 {
        return Ok(v);
    }
    // SAFETY: `v` has room for `n` T's (`size` bytes); the HAL writes at most that and
    // reports what it wrote in `size`.
    unsafe {
        let status = AudioObjectGetPropertyData(
            obj,
            NonNull::from(&a),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::new(v.as_mut_ptr()).expect("non-null").cast(),
        );
        check(status, "get property")?;
        v.set_len((size as usize / size_of::<T>()).min(n));
    }
    Ok(v)
}

fn set<T>(obj: Id, selector: u32, scope: u32, value: &T) -> Result<()> {
    let a = addr(selector, scope);
    // SAFETY: `value` is a valid T of the size we pass.
    let status = unsafe {
        AudioObjectSetPropertyData(obj, NonNull::from(&a), 0, null(), size_of::<T>() as u32, NonNull::from(value).cast())
    };
    check(status, "set property")
}

fn get_string(obj: Id, selector: u32) -> Option<String> {
    let a = addr(selector, GLOBAL);
    let mut s: *mut CFString = std::ptr::null_mut();
    let mut size = size_of::<*mut CFString>() as u32;
    // SAFETY: the HAL writes a CFStringRef we own (+1) into `s`.
    let status = unsafe {
        AudioObjectGetPropertyData(obj, NonNull::from(&a), 0, null(), NonNull::from(&mut size), NonNull::from(&mut s).cast())
    };
    if status != 0 {
        return None;
    }
    // SAFETY: a non-null +1 CFString from the call above.
    NonNull::new(s).map(|p| unsafe { CFRetained::from_raw(p) }.to_string())
}

/// All devices with at least one output stream.
pub fn output_devices() -> Vec<Id> {
    get_vec::<Id>(SYSTEM, kAudioHardwarePropertyDevices, GLOBAL)
        .unwrap_or_default()
        .into_iter()
        .filter(|d| !output_streams(*d).is_empty())
        .collect()
}

pub fn default_output() -> Option<Id> {
    get::<Id>(SYSTEM, kAudioHardwarePropertyDefaultOutputDevice, GLOBAL).ok().filter(|d| *d != 0)
}

/// Makes `dev` the system's default output (hardware tests only).
#[cfg(test)]
pub fn set_default_output(dev: Id) -> Result<()> {
    set(SYSTEM, kAudioHardwarePropertyDefaultOutputDevice, GLOBAL, &dev)
}

pub fn uid(dev: Id) -> Option<String> {
    get_string(dev, kAudioDevicePropertyDeviceUID)
}

pub fn name(dev: Id) -> String {
    get_string(dev, kAudioObjectPropertyName).unwrap_or_else(|| format!("device {dev}"))
}

pub fn transport(dev: Id) -> u32 {
    get::<u32>(dev, kAudioDevicePropertyTransportType, GLOBAL).unwrap_or(0)
}

pub fn data_source(dev: Id) -> Option<u32> {
    get::<u32>(dev, kAudioDevicePropertyDataSource, OUTPUT).ok()
}

pub fn output_streams(dev: Id) -> Vec<Id> {
    get_vec::<Id>(dev, kAudioDevicePropertyStreams, OUTPUT).unwrap_or_default()
}

pub fn physical_formats(stream: Id) -> Vec<AudioStreamRangedDescription> {
    get_vec(stream, kAudioStreamPropertyAvailablePhysicalFormats, GLOBAL).unwrap_or_default()
}

pub fn physical_format(stream: Id) -> Option<AudioStreamBasicDescription> {
    get(stream, kAudioStreamPropertyPhysicalFormat, GLOBAL).ok()
}

pub fn available_rates(dev: Id) -> Vec<AudioValueRange> {
    get_vec(dev, kAudioDevicePropertyAvailableNominalSampleRates, GLOBAL).unwrap_or_default()
}

pub fn nominal_rate(dev: Id) -> Option<f64> {
    get::<f64>(dev, kAudioDevicePropertyNominalSampleRate, GLOBAL).ok()
}

pub fn is_lpcm(f: &AudioStreamBasicDescription) -> bool {
    f.mFormatID == kAudioFormatLinearPCM
}

pub fn is_int(f: &AudioStreamBasicDescription) -> bool {
    is_lpcm(f) && f.mFormatFlags & kAudioFormatFlagIsSignedInteger != 0 && f.mFormatFlags & kAudioFormatFlagIsFloat == 0
}

pub fn is_mixable(f: &AudioStreamBasicDescription) -> bool {
    f.mFormatFlags & kAudioFormatFlagIsNonMixable == 0
}

/// Sets the device's nominal rate and waits (up to 2 s) for the HAL's
/// rate-change notification confirming it.
pub fn set_nominal_rate(dev: Id, rate: u32) -> Result<()> {
    let target = rate as f64;
    if nominal_rate(dev).is_some_and(|r| (r - target).abs() < 0.5) {
        return Ok(());
    }
    let (tx, rx) = mpsc::channel();
    let _listener = Listener::new(dev, kAudioDevicePropertyNominalSampleRate, GLOBAL, move || {
        let _ = tx.send(());
    })?;
    set(dev, kAudioDevicePropertyNominalSampleRate, GLOBAL, &target)?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let _ = rx.recv_timeout(Duration::from_millis(100));
        if nominal_rate(dev).is_some_and(|r| (r - target).abs() < 0.5) {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(format!("device didn't switch to {rate} Hz"));
        }
    }
}

/// Sets a stream's physical format and waits (up to 2 s) until it reads back.
pub fn set_physical_format(stream: Id, format: &AudioStreamBasicDescription) -> Result<()> {
    let same = |f: &AudioStreamBasicDescription| {
        f.mSampleRate as u32 == format.mSampleRate as u32
            && f.mFormatID == format.mFormatID
            && f.mFormatFlags == format.mFormatFlags
            && f.mBitsPerChannel == format.mBitsPerChannel
            && f.mChannelsPerFrame == format.mChannelsPerFrame
            && f.mBytesPerFrame == format.mBytesPerFrame
    };
    if physical_format(stream).is_some_and(|f| same(&f)) {
        return Ok(());
    }
    set(stream, kAudioStreamPropertyPhysicalFormat, GLOBAL, format)?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while !physical_format(stream).is_some_and(|f| same(&f)) {
        if Instant::now() > deadline {
            return Err("physical format didn't change".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

/// The pid holding the device exclusively, or -1.
pub fn hog_pid(dev: Id) -> i32 {
    get::<libc::pid_t>(dev, kAudioDevicePropertyHogMode, GLOBAL).unwrap_or(-1)
}

pub fn own_pid() -> i32 {
    // SAFETY: getpid has no preconditions.
    unsafe { libc::getpid() }
}

/// Takes (`true`) or releases exclusive access. Returns whether this process
/// holds it afterwards.
pub fn set_hog(dev: Id, take: bool) -> Result<bool> {
    let me = own_pid();
    let holder = hog_pid(dev);
    if (holder == me) == take {
        return Ok(take);
    }
    if take && holder != -1 {
        return Err(format!("another process ({holder}) has exclusive access"));
    }
    let value: libc::pid_t = if take { me } else { -1 };
    set(dev, kAudioDevicePropertyHogMode, GLOBAL, &value)?;
    Ok(hog_pid(dev) == me)
}

/// The name of process `pid`, to say who holds a device.
pub fn process_name(pid: i32) -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: proc_name writes at most `buf.len()` bytes and returns how many.
    let n = unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

/// A non-mixable format is only usable by the process holding the device:
/// left on a device nobody holds, macOS can't play anything through it. This
/// switches the device's first output stream to the closest mixable integer
/// (else float) format at the rate it's at. Returns whether it changed anything.
/// Call it while holding the device, or when nobody does.
pub fn make_mixable(dev: Id) -> Result<bool> {
    let Some(stream) = output_streams(dev).first().copied() else { return Ok(false) };
    let Some(current) = physical_format(stream) else { return Ok(false) };
    if !is_lpcm(&current) || is_mixable(&current) {
        return Ok(false);
    }
    let rate = current.mSampleRate;
    let best = physical_formats(stream)
        .into_iter()
        .filter(|f| {
            let r = &f.mSampleRateRange;
            r.mMinimum - 0.5 <= rate && rate <= r.mMaximum + 0.5 || (f.mFormat.mSampleRate - rate).abs() < 0.5
        })
        .map(|f| f.mFormat)
        .filter(|f| is_lpcm(f) && is_mixable(f) && f.mChannelsPerFrame == current.mChannelsPerFrame)
        // Same bit depth first, then integer over float, then the widest.
        .max_by_key(|f| (f.mBitsPerChannel == current.mBitsPerChannel, is_int(f), f.mBitsPerChannel));
    let Some(mut format) = best else { return Err("no mixable format to switch to".into()) };
    format.mSampleRate = rate;
    set_physical_format(stream, &format)?;
    Ok(true)
}

/// Output-scope elements carrying `selector` that we may set: the main one,
/// else each channel's (devices put their volume and mute on either).
fn settable_elements(dev: Id, selector: u32) -> Vec<u32> {
    let settable = |element: u32| {
        let a = addr_at(selector, OUTPUT, element);
        let mut ok: u8 = 0;
        // SAFETY: valid pointers to stack values.
        unsafe {
            AudioObjectHasProperty(dev, NonNull::from(&a))
                && AudioObjectIsPropertySettable(dev, NonNull::from(&a), NonNull::from(&mut ok).cast()) == 0
                && ok != 0
        }
    };
    if settable(kAudioObjectPropertyElementMain) {
        return vec![kAudioObjectPropertyElementMain];
    }
    (1..=2).filter(|e| settable(*e)).collect()
}

fn get_at<T: Copy>(obj: Id, selector: u32, element: u32) -> Option<T> {
    let a = addr_at(selector, OUTPUT, element);
    let mut value = MaybeUninit::<T>::zeroed();
    let mut size = size_of::<T>() as u32;
    // SAFETY: the HAL writes at most `size` bytes into `value`, a plain-data T.
    let status = unsafe {
        AudioObjectGetPropertyData(obj, NonNull::from(&a), 0, null(), NonNull::from(&mut size), NonNull::from(&mut value).cast())
    };
    // SAFETY: zero-initialised and (partly) overwritten by the HAL; T is plain data.
    (status == 0).then(|| unsafe { value.assume_init() })
}

fn set_at<T>(obj: Id, selector: u32, element: u32, value: &T) -> Result<()> {
    let a = addr_at(selector, OUTPUT, element);
    // SAFETY: `value` is a valid T of the size we pass.
    let status = unsafe {
        AudioObjectSetPropertyData(obj, NonNull::from(&a), 0, null(), size_of::<T>() as u32, NonNull::from(value).cast())
    };
    check(status, "set property")
}

/// The device's own output volume: (level, min, max) in dB, the loudest
/// channel's level when they differ. None when it has none we may set.
pub fn volume_db(dev: Id) -> Option<(f32, f32, f32)> {
    let elements = settable_elements(dev, kAudioDevicePropertyVolumeDecibels);
    let range: AudioValueRange = get_at(dev, kAudioDevicePropertyVolumeRangeDecibels, *elements.first()?)?;
    let db = elements.iter().filter_map(|e| get_at::<f32>(dev, kAudioDevicePropertyVolumeDecibels, *e)).reduce(f32::max)?;
    Some((db, range.mMinimum as f32, range.mMaximum as f32))
}

/// Sets every channel of the device's own volume to `db`.
pub fn set_volume_db(dev: Id, db: f32) -> Result<()> {
    for e in settable_elements(dev, kAudioDevicePropertyVolumeDecibels) {
        set_at(dev, kAudioDevicePropertyVolumeDecibels, e, &db)?;
    }
    Ok(())
}

/// Whether the device is muted, when it has a mute we may set.
pub fn muted(dev: Id) -> Option<bool> {
    let e = *settable_elements(dev, kAudioDevicePropertyMute).first()?;
    get_at::<u32>(dev, kAudioDevicePropertyMute, e).map(|m| m != 0)
}

pub fn set_muted(dev: Id, muted: bool) -> Result<()> {
    for e in settable_elements(dev, kAudioDevicePropertyMute) {
        set_at(dev, kAudioDevicePropertyMute, e, &u32::from(muted))?;
    }
    Ok(())
}

type Callback = Box<dyn FnMut() + Send>;

/// What a listener registration points the HAL at. Never freed once
/// registered: Apple doesn't promise that a callback already under way has
/// returned when `AudioObjectRemovePropertyListener` does, so removal only
/// empties it (dropping the callback and what it holds) and leaks the slot
/// itself, a few bytes per listener.
struct Slot(Mutex<Option<Callback>>);

/// A property listener; removed on drop. The callback runs on a HAL thread.
pub struct Listener {
    slot: &'static Slot,
    obj: Id,
    addr: AudioObjectPropertyAddress,
}

// SAFETY: the slot is Sync (a Mutex around a Send callback); the rest is plain
// data identifying the registration.
unsafe impl Send for Listener {}

impl Listener {
    pub fn new(obj: Id, selector: u32, scope: u32, callback: impl FnMut() + Send + 'static) -> Result<Self> {
        let slot = Box::into_raw(Box::new(Slot(Mutex::new(Some(Box::new(callback))))));
        let a = addr(selector, scope);
        // SAFETY: `slot` is a live heap allocation; once registered it's never freed.
        let status = unsafe { AudioObjectAddPropertyListener(obj, NonNull::from(&a), Some(shim), slot.cast()) };
        if let Err(e) = check(status, "add listener") {
            // SAFETY: never registered, so nothing else can see it: ours to free.
            drop(unsafe { Box::from_raw(slot) });
            return Err(e);
        }
        // SAFETY: from `Box::into_raw` above and never freed from here on.
        Ok(Self { slot: unsafe { &*slot }, obj, addr: a })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let data = self.slot as *const Slot as *mut c_void;
        // SAFETY: same registration as in `new`.
        unsafe {
            AudioObjectRemovePropertyListener(self.obj, NonNull::from(&self.addr), Some(shim), data);
        }
        // A call still under way finishes first (it holds the lock); a later one finds nothing.
        let callback = self.slot.0.lock().unwrap_or_else(|e| e.into_inner()).take();
        drop(callback);
    }
}

unsafe extern "C-unwind" fn shim(_: Id, _: u32, _: NonNull<AudioObjectPropertyAddress>, data: *mut c_void) -> i32 {
    // SAFETY: `data` is the `Slot` registered in `Listener::new`, which is never freed.
    // Only shared references are made to it; the callback is reached through its lock.
    let slot = unsafe { &*(data as *const Slot) };
    if let Some(callback) = slot.0.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        callback();
    }
    0
}

/// Listens for device list and default-output changes.
pub fn watch_system(on_devices: impl FnMut() + Send + 'static, on_default: impl FnMut() + Send + 'static) -> Vec<Listener> {
    [
        Listener::new(SYSTEM, kAudioHardwarePropertyDevices, GLOBAL, on_devices),
        Listener::new(SYSTEM, kAudioHardwarePropertyDefaultOutputDevice, GLOBAL, on_default),
    ]
    .into_iter()
    .filter_map(|l| l.map_err(|e| tracing::warn!("device listener: {e}")).ok())
    .collect()
}
