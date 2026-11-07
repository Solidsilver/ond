//! Tokens, and the one refresh in flight.
//!
//! The server rotates the refresh token on every refresh, so two refreshes
//! racing would spend the same token twice, and every refresh counts against
//! the same 40-per-10-minutes budget as a sign-in. [`AuthState`] keeps the
//! tokens with a generation that moves whenever they change. A request
//! remembers the generation it was sent with; when it comes back 401 it asks
//! for a refresh *from that generation*. Under the refresh lock, a caller
//! whose generation is already stale takes the tokens that replaced it, and a
//! caller that waited while an attempt failed takes that attempt's error.
//! Only a caller that finds nothing newer than what it was refused with goes
//! to the server.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::error::{AuthError, Error, Result};

/// An access token and the refresh token that renews it. Both are secrets:
/// `Debug` shows neither, and nothing in this crate logs them.
#[derive(Clone, PartialEq, Eq)]
pub struct Tokens {
    access: String,
    refresh: String,
}

impl Tokens {
    /// Tokens restored from where the app keeps them.
    pub fn new(access: impl Into<String>, refresh: impl Into<String>) -> Self {
        Self { access: access.into(), refresh: refresh.into() }
    }

    pub fn access(&self) -> &str {
        &self.access
    }

    pub fn refresh(&self) -> &str {
        &self.refresh
    }
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens").finish_non_exhaustive()
    }
}

/// A change to the account's tokens the app must persist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthEvent {
    /// New tokens, from a sign-in or a refresh. The previous refresh token
    /// stops working 10 minutes after a refresh, so these replace the stored
    /// pair, atomically. The hook runs before the new access token is used.
    Tokens(Tokens),
    /// The session is gone: the server refused the refresh token, or the
    /// user signed out. The stored pair should be removed.
    SignedOut,
}

/// Called with every [`AuthEvent`], on the task that caused it, while the
/// refresh lock is held. Keep it quick: hand slow work (a keyring that may
/// prompt) to another thread.
pub type AuthHook = Arc<dyn Fn(AuthEvent) + Send + Sync>;

pub(crate) struct AuthState {
    current: Mutex<Current>,
    /// Held for the length of a refresh. Holds the outcome of the latest
    /// attempt for the callers that waited on it.
    refresh: tokio::sync::Mutex<Option<Result<Tokens>>>,
    /// Refresh attempts finished, successful or not.
    attempts: AtomicU64,
    hook: Mutex<Option<AuthHook>>,
}

struct Current {
    tokens: Option<Tokens>,
    generation: u64,
}

/// The tokens a request was sent with, and their generation.
pub(crate) struct Ticket {
    pub tokens: Tokens,
    pub generation: u64,
}

impl AuthState {
    pub fn new(tokens: Option<Tokens>) -> Self {
        Self {
            current: Mutex::new(Current { tokens, generation: 0 }),
            refresh: tokio::sync::Mutex::new(None),
            attempts: AtomicU64::new(0),
            hook: Mutex::new(None),
        }
    }

    pub fn set_hook(&self, hook: Option<AuthHook>) {
        *self.hook.lock().unwrap_or_else(|e| e.into_inner()) = hook;
    }

    fn notify(&self, event: AuthEvent) {
        let hook = self.hook.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(hook) = hook {
            hook(event);
        }
    }

    pub fn ticket(&self) -> Result<Ticket> {
        let c = self.current.lock().unwrap_or_else(|e| e.into_inner());
        match &c.tokens {
            Some(t) => Ok(Ticket { tokens: t.clone(), generation: c.generation }),
            None => Err(AuthError::SignedOut.into()),
        }
    }

    pub fn tokens(&self) -> Option<Tokens> {
        self.current.lock().unwrap_or_else(|e| e.into_inner()).tokens.clone()
    }

    /// Tokens the app kept from an earlier sign-in, before the client is
    /// used.
    pub fn restore(&self, tokens: Tokens) {
        let mut c = self.current.lock().unwrap_or_else(|e| e.into_inner());
        c.tokens = Some(tokens);
        c.generation += 1;
    }

    /// Replaces the tokens after a sign-in or a sign-out, telling the hook
    /// first.
    pub async fn replace(&self, tokens: Option<Tokens>) {
        // Under the refresh lock, so a refresh in flight can't land on top.
        let mut last = self.refresh.lock().await;
        self.notify(match &tokens {
            Some(t) => AuthEvent::Tokens(t.clone()),
            None => AuthEvent::SignedOut,
        });
        let mut c = self.current.lock().unwrap_or_else(|e| e.into_inner());
        c.tokens = tokens;
        c.generation += 1;
        *last = None;
    }

    /// Fresh tokens for a caller that was refused with `generation`, from
    /// whichever refresh got there first. `refresh` goes to the server with a
    /// refresh token and returns the new pair; it runs at most once at a time
    /// per client.
    pub async fn refreshed<F, Fut>(&self, generation: u64, refresh: F) -> Result<Tokens>
    where
        F: FnOnce(Tokens) -> Fut,
        Fut: Future<Output = Result<Tokens>>,
    {
        let seen = self.attempts.load(Ordering::SeqCst);
        let mut last = self.refresh.lock().await;
        let held = {
            let c = self.current.lock().unwrap_or_else(|e| e.into_inner());
            if c.generation != generation {
                return c.tokens.clone().ok_or_else(|| AuthError::SignedOut.into());
            }
            c.tokens.clone()
        };
        if self.attempts.load(Ordering::SeqCst) != seen
            && let Some(outcome) = last.as_ref()
        {
            return outcome.clone();
        }
        let Some(held) = held else { return Err(AuthError::SignedOut.into()) };
        let outcome = refresh(held).await;
        match &outcome {
            Ok(tokens) => {
                self.notify(AuthEvent::Tokens(tokens.clone()));
                let mut c = self.current.lock().unwrap_or_else(|e| e.into_inner());
                c.tokens = Some(tokens.clone());
                c.generation += 1;
            }
            Err(Error::Auth(AuthError::SignedOut)) => {
                self.notify(AuthEvent::SignedOut);
                let mut c = self.current.lock().unwrap_or_else(|e| e.into_inner());
                c.tokens = None;
                c.generation += 1;
            }
            Err(_) => {}
        }
        *last = Some(outcome.clone());
        self.attempts.fetch_add(1, Ordering::SeqCst);
        outcome
    }
}
