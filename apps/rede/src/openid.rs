//! A pending browser sign-in lives in the keyring, including the ABS session
//! cookies. A redirect can launch a fresh process, which must recover the
//! original verifier rather than starting a new authorization request.

use crate::settings::Account;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime};

#[cfg(target_os = "macos")]
pub mod macos;

pub const TIMEOUT: Duration = Duration::from_secs(5 * 60);
pub const FALLBACK: &str = "This server uses audiobookshelf://oauth. Your admin can add rede://oauth to the server's allowed OpenID redirect URIs.";

#[derive(Clone, Serialize, Deserialize)]
pub struct Pending {
    pub account: Account,
    pub flow: abs::OpenId,
    pub started: SystemTime,
    pub warning: Option<String>,
}

impl Pending {
    pub fn remaining(&self, now: SystemTime) -> Duration {
        TIMEOUT.saturating_sub(now.duration_since(self.started).unwrap_or(TIMEOUT))
    }
}

/// The blocking credential boundary. Debug is deliberately absent, including
/// on errors: keyring and JSON diagnostics can include credential values.
pub trait Store: Send + Sync {
    fn save(&self, pending: &Pending) -> Result<(), &'static str>;
    fn load(&self) -> Option<Pending>;
    fn clear(&self);
}

pub struct Keyring;

fn entry() -> Result<keyring::Entry, keyring::Error> {
    keyring::Entry::new("dev.solidsilver.rede", "pending-openid")
}

impl Store for Keyring {
    fn save(&self, pending: &Pending) -> Result<(), &'static str> {
        let value = serde_json::to_string(pending).map_err(|_| "Couldn't keep the browser sign-in safely. Try again.")?;
        entry().and_then(|e| e.set_password(&value)).map_err(|_| "Couldn't keep the browser sign-in in the keyring. Unlock it and try again.")
    }

    fn load(&self) -> Option<Pending> {
        let value = entry().ok()?.get_password().ok()?;
        serde_json::from_str(&value).ok()
    }

    fn clear(&self) {
        if let Ok(entry) = entry() {
            let _ = entry.delete_credential();
        }
    }
}

/// Early launch URLs wait until the keyring read finishes.
#[derive(Default)]
pub struct State {
    pub pending: Option<Pending>,
    pub restored: bool,
    pub exchanging: bool,
    pub early_redirect: Option<String>,
}

#[cfg(test)]
#[derive(Clone, Default)]
pub struct TestStore(pub std::sync::Arc<std::sync::Mutex<Option<Pending>>>);

#[cfg(test)]
impl Store for TestStore {
    fn save(&self, pending: &Pending) -> Result<(), &'static str> {
        *self.0.lock().unwrap() = Some(pending.clone());
        Ok(())
    }
    fn load(&self) -> Option<Pending> {
        self.0.lock().unwrap().clone()
    }
    fn clear(&self) {
        *self.0.lock().unwrap() = None;
    }
}

#[cfg(test)]
pub fn test_pending() -> crate::openid::Pending {
    crate::openid::Pending {
        account: Account::new("https://books.example.com", ""),
        flow: serde_json::from_value(serde_json::json!({
            "verifier": "fixture-verifier", "state": "fixture-state", "redirect": "rede://oauth",
            "cookies": "connect.sid=fixture; auth_method=openid-mobile", "browser": "https://id.example/authorize"
        }))
        .unwrap(),
        started: std::time::SystemTime::now(),
        warning: None,
    }
}
