//! Where the account's tokens are kept between runs: the system keyring, keyed
//! by the account's id.
//!
//! The `abs` client rotates the refresh token on every refresh and hands each
//! new pair to its [`AuthEvent`] hook while it holds the refresh lock, so the
//! hook mustn't wait on anything. This module is the other side of that: the
//! hook only hands the event to a channel, and one thread of its own does the
//! keychain write.

use crate::settings::{Account, Settings};
use abs::{AuthEvent, Tokens};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

/// A handle to the thread that writes tokens. Cheap to clone.
#[derive(Clone)]
pub struct Keeper {
    /// The account the tokens belong to, or `None` once signed out: an event
    /// from a client that outlived the sign-out would otherwise store tokens
    /// for an account nobody is using.
    current: Arc<Mutex<Option<Account>>>,
    tx: Sender<(Account, AuthEvent)>,
}

impl Keeper {
    /// Starts the writer thread, which lives as long as the app.
    pub fn start() -> Self {
        let (tx, rx) = channel::<(Account, AuthEvent)>();
        std::thread::Builder::new()
            .name("token-keeper".into())
            .spawn(move || {
                let settings = Settings::default();
                for (account, event) in rx {
                    match event {
                        AuthEvent::Tokens(tokens) => store(&settings, &account, &tokens),
                        // The server refused the refresh token: the saved pair
                        // is worthless, and the app is on its way to the
                        // sign-in form.
                        AuthEvent::SignedOut => settings.forget_tokens(&account),
                    }
                }
            })
            .expect("spawn the token keeper");
        Self { current: Arc::new(Mutex::new(None)), tx }
    }

    /// Sets (or, with `None`, clears) the account whose tokens are kept.
    pub fn use_account(&self, account: Option<Account>) {
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) = account;
    }

    /// The hook to give a client: it never blocks, and does nothing once no
    /// account is in use.
    pub fn hook(&self) -> impl Fn(AuthEvent) + Send + Sync + 'static {
        let current = self.current.clone();
        let tx = self.tx.clone();
        move |event| {
            let account = current.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(account) = account {
                let _ = tx.send((account, event));
            }
        }
    }
}

/// Blocking: may show a keychain prompt. The keyring is the only copy of the
/// tokens, so a failure is logged and the next start simply asks to sign in.
fn store(settings: &Settings, account: &Account, tokens: &Tokens) {
    if let Err(e) = settings.store_tokens(account, tokens) {
        tracing::warn!("keeping the account's tokens: {e}");
    }
}
