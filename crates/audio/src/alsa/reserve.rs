//! The D-Bus device reservation protocol (`org.freedesktop.ReserveDevice1`):
//! how PipeWire, PulseAudio, JACK and others agree on who opens a sound card
//! directly. Owning the bus name `org.freedesktop.ReserveDevice1.Audio<N>`
//! means owning card N; the owner answers `RequestRelease` from whoever
//! wants it next. The reference is `reserve.c`, shared by PulseAudio and JACK.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use zbus::blocking::Connection;
use zbus::fdo::{RequestNameFlags, RequestNameReply};

const INTERFACE: &str = "org.freedesktop.ReserveDevice1";
/// Above the sound servers' own (PulseAudio and PipeWire hold cards at 0 or
/// below and let go to anyone higher), below pro-audio servers' (JACK asks
/// higher, and wins over us).
pub const PRIORITY: i32 = 10;
/// How long the current owner gets to answer.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Why a card couldn't be reserved.
#[derive(Debug)]
pub enum ReserveError {
    /// No session bus to ask on (headless, or a sandbox without access).
    NoBus(String),
    /// The current owner said no.
    Refused { owner: Option<String> },
    Other(String),
}

/// Card `card`, reserved for us until dropped.
pub struct Reservation {
    conn: Connection,
    name: String,
    /// Playing: requests to release are declined.
    busy: Arc<AtomicBool>,
    /// We agreed to give the card up (idle, someone asked).
    released: Arc<AtomicBool>,
}

/// What we put on the bus for others to ask.
struct Owner {
    device: String,
    busy: Arc<AtomicBool>,
    released: Arc<AtomicBool>,
}

#[zbus::interface(name = "org.freedesktop.ReserveDevice1")]
impl Owner {
    /// Declined while playing; given up while idle, to anyone.
    fn request_release(&self, _priority: i32) -> bool {
        if self.busy.load(Ordering::Acquire) {
            return false;
        }
        self.released.store(true, Ordering::Release);
        true
    }

    #[zbus(property)]
    fn priority(&self) -> i32 {
        PRIORITY
    }

    #[zbus(property)]
    fn application_name(&self) -> String {
        "Ond".into()
    }

    #[zbus(property)]
    fn application_device_name(&self) -> String {
        self.device.clone()
    }
}

fn names(card: u32) -> (String, String) {
    (format!("{INTERFACE}.Audio{card}"), format!("/org/freedesktop/ReserveDevice1/Audio{card}"))
}

impl Reservation {
    /// Reserves card `card` (its index), asking the current owner, if any, to
    /// let go. `device` names it for others asking (e.g. `hw:1`).
    pub fn acquire(card: u32, device: &str) -> Result<Self, ReserveError> {
        let (name, path) = names(card);
        let (busy, released) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let owner = Owner { device: device.to_owned(), busy: busy.clone(), released: released.clone() };
        let no_bus = |e: zbus::Error| ReserveError::NoBus(e.to_string());
        let conn = zbus::blocking::connection::Builder::session()
            .map_err(no_bus)?
            .method_timeout(TIMEOUT)
            .serve_at(path.as_str(), owner)
            .map_err(no_bus)?
            .build()
            .map_err(no_bus)?;
        let other = |e: zbus::Error| ReserveError::Other(e.to_string());
        // Others may take it from us, once we agree (see `request_release`).
        let flags = RequestNameFlags::AllowReplacement | RequestNameFlags::DoNotQueue;
        match conn.request_name_with_flags(name.as_str(), flags).map_err(other)? {
            RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => {}
            RequestNameReply::Exists | RequestNameReply::InQueue => {
                let owner = owner_name(&conn, &name, &path);
                let agreed = conn
                    .call_method(Some(name.as_str()), path.as_str(), Some(INTERFACE), "RequestRelease", &(PRIORITY,))
                    .ok()
                    .and_then(|m| m.body().deserialize::<bool>().ok())
                    .unwrap_or(false);
                if !agreed {
                    return Err(ReserveError::Refused { owner });
                }
                match conn.request_name_with_flags(name.as_str(), flags | RequestNameFlags::ReplaceExisting).map_err(other)? {
                    RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => {}
                    _ => return Err(ReserveError::Refused { owner }),
                }
            }
        }
        Ok(Self { conn, name, busy, released })
    }

    /// Whether requests to release should be declined (we're playing).
    pub fn set_busy(&self, busy: bool) {
        self.busy.store(busy, Ordering::Release);
    }

    /// Still ours: we haven't agreed to give it up, and nobody took it.
    pub fn held(&self) -> bool {
        if self.released.load(Ordering::Acquire) {
            return false;
        }
        let Some(me) = self.conn.unique_name() else { return false };
        let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&self.conn) else { return false };
        let Ok(name) = zbus::names::BusName::try_from(self.name.as_str()) else { return false };
        dbus.get_name_owner(name).is_ok_and(|owner| owner.as_str() == me.as_str())
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        // Dropping the connection would too; this tells the bus right away.
        let _ = self.conn.release_name(self.name.as_str());
    }
}

/// The current owner's `ApplicationName`, to say who won't let go.
fn owner_name(conn: &Connection, name: &str, path: &str) -> Option<String> {
    let reply = conn
        .call_method(Some(name), path, Some("org.freedesktop.DBus.Properties"), "Get", &(INTERFACE, "ApplicationName"))
        .ok()?;
    let value: zbus::zvariant::OwnedValue = reply.body().deserialize().ok()?;
    String::try_from(value).ok().filter(|s| !s.is_empty())
}
