//! The account's tokens, written to the system keyring on one ordered channel.
//!
//! The client's auth hook runs under its refresh lock, so it only enqueues an
//! event. Sign-out closes that login's hook and enqueues deletion under the
//! same lock: every accepted rotation is stored before the deletion.

use crate::settings::{Account, Settings};
use abs::{AuthEvent, Tokens};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

/// The blocking storage boundary, also used by tests without a keychain.
pub(crate) trait TokenStore: Send + 'static {
    fn store(&self, account: &Account, tokens: &Tokens);
    fn forget(&self, account: &Account);
}

impl TokenStore for Settings {
    fn store(&self, account: &Account, tokens: &Tokens) {
        if let Err(e) = self.store_tokens(account, tokens) {
            tracing::warn!("keeping the account's tokens: {e}");
        }
    }

    fn forget(&self, account: &Account) {
        self.forget_tokens(account);
    }
}

enum Write {
    Event(Arc<Account>, AuthEvent),
    Forget(Account),
}

/// A handle to the thread that writes tokens. Cheap to clone.
#[derive(Clone)]
pub struct Keeper {
    /// Each login owns a distinct Arc, even when it reuses an account's id.
    /// An old client's hook must not write into a later login.
    current: Arc<Mutex<Option<Arc<Account>>>>,
    tx: Sender<Write>,
}

impl Keeper {
    /// Starts the writer thread, which lives as long as the app.
    pub fn start() -> Self {
        Self::with_store(Settings::default())
    }

    pub(crate) fn with_store(store: impl TokenStore) -> Self {
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("token-keeper".into())
            .spawn(move || {
                for write in rx {
                    match write {
                        Write::Event(account, AuthEvent::Tokens(tokens)) => store.store(&account, &tokens),
                        Write::Event(account, AuthEvent::SignedOut) => store.forget(&account),
                        Write::Forget(account) => store.forget(&account),
                    }
                }
            })
            .expect("spawn the token keeper");
        Self { current: Arc::new(Mutex::new(None)), tx }
    }

    /// Starts a login. Call on the UI thread before launching its background work.
    pub fn use_account(&self, account: Account) {
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(account));
    }

    /// Stops accepting this login's events and deletes after every queued write.
    pub fn forget(&self, account: Account) {
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if current.as_ref().is_some_and(|a| a.id == account.id) {
            *current = None;
        }
        let _ = self.tx.send(Write::Forget(account));
    }

    /// The hook for this login: enqueue without waiting for the keychain.
    pub fn hook(&self) -> impl Fn(AuthEvent) + Send + Sync + 'static {
        let current = self.current.clone();
        let account = current.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let tx = self.tx.clone();
        move |event| {
            let active = current.lock().unwrap_or_else(|e| e.into_inner());
            if let (Some(account), Some(active)) = (&account, active.as_ref())
                && Arc::ptr_eq(account, active)
            {
                // Keep the lock through enqueueing so forget cannot overtake it.
                let _ = tx.send(Write::Event(account.clone(), event));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Receiver;
    use std::time::Duration;

    struct Store {
        saved: Arc<Mutex<Option<Tokens>>>,
        events: Sender<&'static str>,
        resume: Receiver<()>,
    }

    impl TokenStore for Store {
        fn store(&self, _: &Account, tokens: &Tokens) {
            self.events.send("storing").unwrap();
            self.resume.recv().unwrap();
            *self.saved.lock().unwrap() = Some(tokens.clone());
        }

        fn forget(&self, _: &Account) {
            *self.saved.lock().unwrap() = None;
            self.events.send("forgotten").unwrap();
        }
    }

    #[test]
    fn a_queued_rotation_followed_by_sign_out_leaves_no_stored_tokens() {
        let (events, rx) = channel();
        let (resume, paused) = channel();
        let saved = Arc::new(Mutex::new(None));
        let keeper = Keeper::with_store(Store { saved: saved.clone(), events, resume: paused });
        let account = Account::new("https://books.example.com", "sam");
        keeper.use_account(account.clone());
        let hook = keeper.hook();
        hook(AuthEvent::Tokens(Tokens::new("access-1", "refresh-1")));
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), "storing");
        hook(AuthEvent::Tokens(Tokens::new("access-2", "refresh-2")));
        keeper.forget(account);
        hook(AuthEvent::Tokens(Tokens::new("late-access", "late-refresh")));
        resume.send(()).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), "storing");
        resume.send(()).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), "forgotten");
        drop((hook, keeper));
        assert!(matches!(rx.recv_timeout(Duration::from_secs(3)), Err(std::sync::mpsc::RecvTimeoutError::Disconnected)));
        assert!(saved.lock().unwrap().is_none());
    }

    #[test]
    fn an_old_clients_hook_cannot_write_into_a_new_login() {
        let (events, rx) = channel();
        let (resume, paused) = channel();
        let saved = Arc::new(Mutex::new(None));
        let keeper = Keeper::with_store(Store { saved: saved.clone(), events, resume: paused });
        let account = Account::new("https://books.example.com", "sam");
        keeper.use_account(account.clone());
        let old = keeper.hook();
        keeper.forget(account.clone());
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), "forgotten");
        keeper.use_account(account);
        let new = keeper.hook();
        old(AuthEvent::Tokens(Tokens::new("stale", "stale")));
        old(AuthEvent::SignedOut);
        new(AuthEvent::Tokens(Tokens::new("new-access", "new-refresh")));
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), "storing");
        resume.send(()).unwrap();
        drop((old, new, keeper));
        assert!(matches!(rx.recv_timeout(Duration::from_secs(3)), Err(std::sync::mpsc::RecvTimeoutError::Disconnected)));
        assert_eq!(saved.lock().unwrap().as_ref().unwrap().access(), "new-access");
    }
}
