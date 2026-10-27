//! Native ALSA output (Linux).
//!
//! Normal mode plays through the sound server (PipeWire, PulseAudio, or
//! ALSA's default device) with [`CpalBackend`], on the output the server has
//! for the chosen card. Bit-perfect mode on a chosen card:
//! - reserves the card with the D-Bus device reservation protocol, so the
//!   sound server lets go of it (see [`reserve`]);
//! - opens its `hw:` device at the ring's rate, in the widest integer format
//!   the device takes, and writes the samples to it as exact integers;
//! - keeps the card for a few seconds after playback stops (so going from one
//!   track to the next doesn't hand it back and forth), and gives it back
//!   then, on switching to Normal, on choosing another device, and on drop;
//! - plays through the sound server instead when the card can't be had (the
//!   server or another app won't let go, it's in use, it can't do the rate),
//!   and the signal path says why.
//!
//! Without a session bus there's nobody to ask: the card is opened directly,
//! which works when no sound server holds it.

mod cards;
mod hw;
mod reserve;

use crate::device::{DeviceEvent, DeviceInfo};
use crate::output::{CpalBackend, OutputBackend, OutputError, RingInput, StreamFormat, alsa_card_of};
use crate::signal::DeviceCaps;
use crate::{PlaybackMode, Status};
use cards::Pcm;
use reserve::{ReserveError, Reservation};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long a card stays reserved after playback stops.
const IDLE_RELEASE: Duration = Duration::from_secs(5);
/// How often the card list is checked for devices coming and going.
const LIST_EVERY: Duration = Duration::from_secs(1);

/// A card we've reserved (or, without a session bus, simply mean to open).
struct Claim {
    card: u32,
    reservation: Option<Reservation>,
}

/// What's playing.
enum Out {
    Exclusive { stream: hw::Stream, uid: String },
    Shared,
}

pub struct AlsaBackend {
    shared: CpalBackend,
    name: String,
    /// The user's choice, by UID. `None` follows the system default.
    selected: Option<String>,
    mode: PlaybackMode,
    pcms: Vec<Pcm>,
    listed: Instant,
    /// What each device could do when we last had it open, by UID.
    caps: HashMap<String, DeviceCaps>,
    claim: Option<Claim>,
    out: Option<Out>,
    /// Why bit-perfect mode is playing through the sound server.
    why_shared: Option<String>,
    /// Nothing has played since then: the card goes back once it's long enough ago.
    idle_since: Option<Instant>,
    drained: Arc<AtomicBool>,
}

impl AlsaBackend {
    pub fn new() -> Self {
        Self {
            shared: CpalBackend::default(),
            name: "ALSA".into(),
            selected: None,
            mode: PlaybackMode::Normal,
            pcms: cards::list(),
            listed: Instant::now(),
            caps: HashMap::new(),
            claim: None,
            out: None,
            why_shared: None,
            idle_since: None,
            drained: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The card the sound server plays to by default, by index.
    fn default_card() -> Option<u32> {
        use cpal::traits::{DeviceTrait, HostTrait};
        let device = cpal::default_host().default_output_device()?;
        device.description().ok()?.address().and_then(alsa_card_of)
    }

    /// The device to play on: the selected one if it's present, else the
    /// default card's first, else the first there is.
    fn resolve(&self) -> Option<Pcm> {
        let selected = self.selected.as_deref().and_then(|uid| self.pcms.iter().find(|p| p.uid == uid));
        selected
            .or_else(|| Self::default_card().and_then(|c| self.pcms.iter().find(|p| p.card == c)))
            .or_else(|| self.pcms.first())
            .cloned()
    }

    /// The card the sound server should play to in Normal mode: the chosen
    /// one, else whatever it plays to by default.
    fn shared_card(&self) -> Option<u32> {
        let uid = self.selected.as_deref()?;
        self.pcms.iter().find(|p| p.uid == uid).map(|p| p.card)
    }

    /// Reserves `pcm`'s card (keeping a reservation we still hold).
    fn claim(&mut self, pcm: &Pcm) -> Result<(), String> {
        if let Some(c) = &self.claim {
            let still = c.card == pcm.card && c.reservation.as_ref().is_none_or(Reservation::held);
            if still {
                return Ok(());
            }
            self.release_claim();
        }
        match Reservation::acquire(pcm.card, &format!("hw:{}", pcm.card)) {
            Ok(r) => {
                self.claim = Some(Claim { card: pcm.card, reservation: Some(r) });
                Ok(())
            }
            // Nobody to ask: open it directly, which works when nothing else holds it.
            Err(ReserveError::NoBus(e)) => {
                tracing::info!("no session bus to reserve {} ({e}); opening it directly", pcm.card_name);
                self.claim = Some(Claim { card: pcm.card, reservation: None });
                Ok(())
            }
            Err(ReserveError::Refused { owner }) => {
                Err(format!("{} won't release {}", owner.unwrap_or_else(|| "the sound server".into()), pcm.card_name))
            }
            Err(ReserveError::Other(e)) => Err(format!("reserving {}: {e}", pcm.card_name)),
        }
    }

    fn release_claim(&mut self) {
        self.idle_since = None;
        // Dropping the reservation hands the card back to the sound server.
        self.claim = None;
    }

    /// What `pcm` can do, read from the device itself (we hold its card).
    fn probe(&mut self, pcm: &Pcm) -> Result<DeviceCaps, String> {
        let device = cards::open_hw(pcm, Duration::from_millis(1_500)).map_err(|e| cards::open_error(pcm, &e))?;
        let caps = cards::probe(&device, &pcm.name).ok_or_else(|| format!("couldn't read what {} can do", pcm.name))?;
        self.caps.insert(pcm.uid.clone(), caps.clone());
        Ok(caps)
    }

    /// What `pcm` can do, without opening it: from the last time we did, or
    /// from the kernel for USB devices.
    fn known_caps(&self, pcm: &Pcm) -> DeviceCaps {
        self.caps
            .get(&pcm.uid)
            .cloned()
            .or_else(|| cards::usb_caps(pcm))
            .unwrap_or_else(|| DeviceCaps { name: pcm.name.clone(), ..Default::default() })
    }

    fn info(&self, pcm: &Pcm, default_card: Option<u32>) -> DeviceInfo {
        let mut caps = self.known_caps(pcm);
        // A hardware device always takes integer samples.
        caps.integer = true;
        match &self.out {
            Some(Out::Exclusive { stream, uid }) if *uid == pcm.uid => {
                caps.exclusive = true;
                caps.nominal_rate = Some(stream.rate);
                caps.active_bits = Some(stream.bits);
            }
            _ => caps.shared_reason = Some(self.why_shared.clone().unwrap_or_else(|| "shared mode".into())),
        }
        // A card's first device stands for it as the default.
        let first_of_card = self.pcms.iter().find(|p| p.card == pcm.card).is_some_and(|p| p.uid == pcm.uid);
        DeviceInfo {
            uid: pcm.uid.clone(),
            name: pcm.name.clone(),
            kind: pcm.kind,
            caps,
            is_default: first_of_card && default_card == Some(pcm.card),
        }
    }

    /// The sound server's view, for Normal mode and bit-perfect's fallback.
    fn shared_caps(&mut self) -> Option<DeviceCaps> {
        self.shared.route_to_card(self.shared_card());
        let mut caps = self.shared.device_caps()?;
        if let Some(why) = &self.why_shared {
            caps.shared_reason = Some(why.clone());
        }
        Some(caps)
    }

    /// Stops whatever is playing, keeping the card.
    fn stop_out(&mut self) {
        match self.out.take() {
            Some(Out::Exclusive { stream, .. }) => stream.stop(),
            Some(Out::Shared) => self.shared.close(),
            None => {}
        }
        if let Some(r) = self.claim.as_ref().and_then(|c| c.reservation.as_ref()) {
            r.set_busy(false);
        }
    }

    /// Takes `pcm`'s card and sets the device up for `format`.
    fn prepare_exclusive(&mut self, pcm: &Pcm, format: StreamFormat) -> Result<hw::Prepared, String> {
        self.claim(pcm)?;
        hw::prepare(pcm, format)
    }
}

impl Default for AlsaBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for AlsaBackend {
    fn drop(&mut self) {
        self.stop_out();
        self.release_claim();
    }
}

impl OutputBackend for AlsaBackend {
    fn name(&self) -> &str {
        match self.out {
            Some(Out::Shared) => self.shared.name(),
            _ => &self.name,
        }
    }

    fn devices(&mut self) -> Vec<DeviceInfo> {
        self.pcms = cards::list();
        self.listed = Instant::now();
        let default = Self::default_card();
        self.pcms.iter().map(|p| self.info(p, default)).collect()
    }

    fn current_device(&mut self) -> Option<DeviceInfo> {
        let open = match &self.out {
            Some(Out::Exclusive { uid, .. }) => self.pcms.iter().find(|p| &p.uid == uid).cloned(),
            _ => None,
        };
        let pcm = open.or_else(|| self.resolve())?;
        Some(self.info(&pcm, Self::default_card()))
    }

    fn target_device(&mut self) -> Option<DeviceInfo> {
        let pcm = self.resolve()?;
        Some(self.info(&pcm, Self::default_card()))
    }

    /// A UID that isn't plugged in is kept: playback uses the default until it appears.
    fn select_device(&mut self, uid: Option<&str>) -> Result<(), OutputError> {
        self.close();
        self.release_claim();
        self.why_shared = None;
        self.selected = uid.map(str::to_owned);
        Ok(())
    }

    fn poll_device_events(&mut self) -> Vec<DeviceEvent> {
        if self.out.is_none() && self.idle_since.is_some_and(|t| t.elapsed() >= IDLE_RELEASE) {
            self.release_claim();
        }
        if self.listed.elapsed() < LIST_EVERY {
            return Vec::new();
        }
        self.listed = Instant::now();
        let now = cards::list();
        let uids = |l: &[Pcm]| l.iter().map(|p| p.uid.clone()).collect::<Vec<_>>();
        let changed = uids(&now) != uids(&self.pcms);
        self.pcms = now;
        if changed { vec![DeviceEvent::ListChanged] } else { Vec::new() }
    }

    fn set_mode(&mut self, mode: PlaybackMode) {
        self.mode = mode;
        self.why_shared = None;
        // An exclusive stream ends when the engine reopens (see `wants_reopen`);
        // with nothing playing, the card goes back now.
        if mode == PlaybackMode::Normal && self.out.is_none() {
            self.release_claim();
        }
    }

    /// The stream in use doesn't fit the mode: exclusive in Normal mode, or
    /// shared in bit-perfect mode before it was tried.
    fn wants_reopen(&mut self) -> bool {
        match self.out {
            Some(Out::Exclusive { .. }) => self.mode == PlaybackMode::Normal,
            Some(Out::Shared) => self.mode == PlaybackMode::BitPerfect && self.why_shared.is_none() && self.resolve().is_some(),
            None => false,
        }
    }

    fn device_caps(&mut self) -> Option<DeviceCaps> {
        match &self.out {
            Some(Out::Exclusive { uid, .. }) => {
                let pcm = self.pcms.iter().find(|p| &p.uid == uid)?;
                return Some(self.info(pcm, None).caps);
            }
            Some(Out::Shared) => return self.shared_caps(),
            None => {}
        }
        if self.mode == PlaybackMode::BitPerfect
            && let Some(pcm) = self.resolve()
        {
            // About to play: take the card now, so the decoder picks a rate it can run at.
            match self.claim(&pcm).and_then(|()| self.probe(&pcm)) {
                Ok(caps) => {
                    self.why_shared = None;
                    return Some(caps);
                }
                Err(why) => {
                    tracing::info!("bit-perfect unavailable: {why}");
                    self.release_claim();
                    self.why_shared = Some(why);
                }
            }
        }
        self.shared_caps()
    }

    fn open(&mut self, format: StreamFormat, input: RingInput, status: Arc<Status>) -> Result<(), OutputError> {
        self.stop_out();
        self.idle_since = None;
        self.drained = Arc::new(AtomicBool::new(false));
        let bit_perfect = status.bit_perfect.load(Ordering::Relaxed);
        self.mode = if bit_perfect { PlaybackMode::BitPerfect } else { PlaybackMode::Normal };
        if bit_perfect
            && self.why_shared.is_none()
            && let Some(pcm) = self.resolve()
        {
            match self.prepare_exclusive(&pcm, format) {
                Ok(prepared) => {
                    let stream = hw::Stream::start(prepared, format, input, status, self.drained.clone());
                    if let Some(r) = self.claim.as_ref().and_then(|c| c.reservation.as_ref()) {
                        r.set_busy(true);
                    }
                    self.name = format!("ALSA · {} (exclusive)", pcm.name);
                    self.out = Some(Out::Exclusive { stream, uid: pcm.uid });
                    return Ok(());
                }
                Err(why) => {
                    tracing::info!("bit-perfect unavailable: {why}");
                    self.why_shared = Some(why);
                    self.release_claim();
                }
            }
        } else if !bit_perfect {
            self.release_claim();
        }
        self.shared.route_to_card(self.shared_card());
        self.shared.open(format, input, status)?;
        self.out = Some(Out::Shared);
        Ok(())
    }

    fn drained(&self) -> bool {
        match self.out {
            Some(Out::Shared) => self.shared.drained(),
            _ => self.drained.load(Ordering::Acquire),
        }
    }

    /// Stops playing. In bit-perfect mode the card stays reserved a little
    /// while, for the next track; in Normal mode it goes back now.
    fn close(&mut self) {
        self.stop_out();
        if self.mode == PlaybackMode::Normal {
            self.release_claim();
        } else if self.claim.is_some() {
            self.idle_since = Some(Instant::now());
        }
    }

    /// Keeps the card for the `open` that follows.
    fn close_for_reopen(&mut self) {
        self.stop_out();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs real hardware: lists the playback devices and what we'd report for them.
    #[test]
    #[ignore = "needs audio hardware"]
    fn list_devices() {
        let mut b = AlsaBackend::new();
        for d in b.devices() {
            println!(
                "{} [{}] {:?} default={} · {} · configs {:?} max bits {:?}",
                d.name,
                d.uid,
                d.kind,
                d.is_default,
                d.capability_line(),
                d.caps.configs,
                d.caps.max_bits
            );
        }
    }

    /// Needs real hardware: plays 1 s of silence bit-perfectly on the default
    /// card (or the device named by UID in `OND_TEST_DEVICE`, from
    /// `list_devices`) at `OND_TEST_RATE` (44100 by default), prints the
    /// verdict, and checks the card is given back. Run with `--ignored --nocapture`.
    #[test]
    #[ignore = "needs audio hardware"]
    fn bit_perfect_on_the_default_device() {
        let mut b = AlsaBackend::new();
        if let Ok(uid) = std::env::var("OND_TEST_DEVICE") {
            b.select_device(Some(&uid)).unwrap();
        }
        let rate: u32 = std::env::var("OND_TEST_RATE").ok().and_then(|r| r.parse().ok()).unwrap_or(44_100);
        b.set_mode(PlaybackMode::BitPerfect);
        let caps = b.device_caps().unwrap();
        println!("before opening: {caps:?}");
        let format = StreamFormat { sample_rate: rate, channels: 2, bits_per_sample: Some(16) };
        let (mut p, c) = rtrb::RingBuffer::new(rate as usize * 2);
        p.push_entire_slice(&vec![0.0f32; rate as usize * 2]).unwrap();
        let (_m, mc) = rtrb::RingBuffer::new(4);
        let input = RingInput::new(c, mc, Arc::new(AtomicBool::new(true)), Arc::default()).with_source(Some(16), false);
        let status = Arc::new(Status::default());
        status.bit_perfect.store(true, Ordering::Relaxed);
        b.open(format, input, status).unwrap();
        let caps = b.device_caps().unwrap();
        println!("{}: {:?}", b.name(), crate::assess(PlaybackMode::BitPerfect, &format, rate, Some(&caps)));
        assert!(matches!(b.out, Some(Out::Exclusive { .. })), "playing on the hardware: {:?}", b.why_shared);
        std::thread::sleep(Duration::from_secs(1));
        b.close();
        assert!(b.claim.is_some(), "kept a moment for the next track");
        b.set_mode(PlaybackMode::Normal);
        assert!(b.claim.is_none(), "Normal mode gives the card back");
    }
}
