//! The account's tokens, written to the system keyring on one ordered channel.
//!
//! The client's auth hook runs under its refresh lock, so it only enqueues an
//! event. Sign-out closes that login's hook and enqueues deletion under the
//! same lock: every accepted rotation is stored before the deletion.
//!
//! There's one live login, and at most one attempt beside it: an account
//! being added while another is in view (Add source…). The attempt's hook
//! writes without closing the live one's, and cancelling it forgets only
//! the attempt.

use crate::settings::{Account, Settings};
use abs::{AuthEvent, Tokens};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

/// The blocking storage boundary, also used by tests without a keychain.
pub(crate) trait TokenStore: Send + 'static {
    fn store(&self, account: &Account, tokens: &Tokens);
    fn forget(&self, account: &Account);
}

#[cfg(test)]
pub(crate) trait TokenLoad {
    fn load(&self, account: &Account) -> Option<Tokens>;
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
    current: Arc<Mutex<Logins>>,
    tx: Sender<Write>,
}

/// The logins whose hooks may write.
#[derive(Default)]
struct Logins {
    live: Option<Arc<Account>>,
    /// A sign-in beside the live login (see [`Keeper::try_account`]).
    attempt: Option<Arc<Account>>,
}

impl Logins {
    fn accepts(&self, account: &Arc<Account>) -> bool {
        [&self.live, &self.attempt].into_iter().flatten().any(|a| Arc::ptr_eq(a, account))
    }
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
        Self { current: Arc::new(Mutex::new(Logins::default())), tx }
    }

    fn logins(&self) -> std::sync::MutexGuard<'_, Logins> {
        self.current.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Starts a login in place of the live one. Call on the UI thread before
    /// launching its background work.
    pub fn use_account(&self, account: Account) {
        *self.logins() = Logins { live: Some(Arc::new(account)), attempt: None };
    }

    /// Starts a login beside the live one, which keeps writing: adding an
    /// account while another is in view. It replaces an earlier attempt.
    pub fn try_account(&self, account: Account) {
        self.logins().attempt = Some(Arc::new(account));
    }

    /// The attempt signed in: it's the live login from here on. Nothing
    /// changes without one.
    pub fn adopt(&self) {
        let mut logins = self.logins();
        if let Some(attempt) = logins.attempt.take() {
            logins.live = Some(attempt);
        }
    }

    /// Stops accepting this login's events and deletes after every queued write.
    pub fn forget(&self, account: Account) {
        let mut guard = self.logins();
        let logins = &mut *guard;
        for login in [&mut logins.live, &mut logins.attempt] {
            if login.as_ref().is_some_and(|a| a.id == account.id) {
                *login = None;
            }
        }
        let _ = self.tx.send(Write::Forget(account));
    }

    /// A cancelled login must not leave a token pair queued behind its last
    /// network response, or let that client's hook write again.
    pub fn cancel_login(&self) {
        let mut logins = self.logins();
        logins.attempt = None;
        if let Some(account) = logins.live.take() {
            let _ = self.tx.send(Write::Forget((*account).clone()));
        }
    }

    /// [`cancel_login`](Self::cancel_login) for an attempt beside the live
    /// login, which is left alone. The attempt's tokens are forgotten unless
    /// it was the live account signing in again: those are the live login's.
    pub fn cancel_attempt(&self) {
        let mut logins = self.logins();
        if let Some(attempt) = logins.attempt.take()
            && logins.live.as_ref().is_none_or(|live| live.id != attempt.id)
        {
            let _ = self.tx.send(Write::Forget((*attempt).clone()));
        }
    }

    /// The hook for the login just started (the attempt, when there is
    /// one): enqueue without waiting for the keychain.
    pub fn hook(&self) -> impl Fn(AuthEvent) + Send + Sync + 'static {
        let current = self.current.clone();
        let account = {
            let logins = self.logins();
            logins.attempt.clone().or_else(|| logins.live.clone())
        };
        let tx = self.tx.clone();
        move |event| {
            let logins = current.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(account) = &account
                && logins.accepts(account)
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
        assert_eq!(rx.recv_timeout(testkit::slack(testkit::WAIT)).unwrap(), "storing");
        hook(AuthEvent::Tokens(Tokens::new("access-2", "refresh-2")));
        keeper.forget(account);
        hook(AuthEvent::Tokens(Tokens::new("late-access", "late-refresh")));
        resume.send(()).unwrap();
        assert_eq!(rx.recv_timeout(testkit::slack(testkit::WAIT)).unwrap(), "storing");
        resume.send(()).unwrap();
        assert_eq!(rx.recv_timeout(testkit::slack(testkit::WAIT)).unwrap(), "forgotten");
        drop((hook, keeper));
        assert!(matches!(rx.recv_timeout(testkit::slack(testkit::WAIT)), Err(std::sync::mpsc::RecvTimeoutError::Disconnected)));
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
        assert_eq!(rx.recv_timeout(testkit::slack(testkit::WAIT)).unwrap(), "forgotten");
        keeper.use_account(account);
        let new = keeper.hook();
        old(AuthEvent::Tokens(Tokens::new("stale", "stale")));
        old(AuthEvent::SignedOut);
        new(AuthEvent::Tokens(Tokens::new("new-access", "new-refresh")));
        assert_eq!(rx.recv_timeout(testkit::slack(testkit::WAIT)).unwrap(), "storing");
        resume.send(()).unwrap();
        drop((old, new, keeper));
        assert!(matches!(rx.recv_timeout(testkit::slack(testkit::WAIT)), Err(std::sync::mpsc::RecvTimeoutError::Disconnected)));
        assert_eq!(saved.lock().unwrap().as_ref().unwrap().access(), "new-access");
    }

    /// Writes and deletions, each with the username they were for.
    struct Log(Sender<String>);

    impl TokenStore for Log {
        fn store(&self, account: &Account, tokens: &Tokens) {
            self.0.send(format!("{} stored {}", account.username, tokens.access())).unwrap();
        }

        fn forget(&self, account: &Account) {
            self.0.send(format!("{} forgotten", account.username)).unwrap();
        }
    }

    #[test]
    fn an_attempt_beside_the_live_login_leaves_its_writes_alone_and_cancelling_forgets_only_the_attempt() {
        let (tx, rx) = channel();
        let keeper = Keeper::with_store(Log(tx));
        let next = || rx.recv_timeout(testkit::slack(testkit::WAIT)).unwrap();
        keeper.use_account(Account::new("https://books.example.com", "sam"));
        let live = keeper.hook();
        keeper.try_account(Account::new("https://two.example.com", "alex"));
        let attempt = keeper.hook();
        live(AuthEvent::Tokens(Tokens::new("sam-1", "r")));
        assert_eq!(next(), "sam stored sam-1");
        attempt(AuthEvent::Tokens(Tokens::new("alex-1", "r")));
        assert_eq!(next(), "alex stored alex-1");
        keeper.cancel_attempt();
        assert_eq!(next(), "alex forgotten");
        keeper.cancel_attempt();
        attempt(AuthEvent::Tokens(Tokens::new("alex-late", "r")));
        live(AuthEvent::Tokens(Tokens::new("sam-2", "r")));
        assert_eq!(next(), "sam stored sam-2", "the live login still writes, and with no attempt left a cancel touches nothing");
    }

    #[test]
    fn an_adopted_attempt_is_the_live_login() {
        let (tx, rx) = channel();
        let keeper = Keeper::with_store(Log(tx));
        let next = || rx.recv_timeout(testkit::slack(testkit::WAIT)).unwrap();
        keeper.use_account(Account::new("https://books.example.com", "sam"));
        keeper.try_account(Account::new("https://two.example.com", "alex"));
        let attempt = keeper.hook();
        keeper.adopt();
        keeper.cancel_login();
        assert_eq!(next(), "alex forgotten", "with no attempt left, cancelling ends the live login, now alex's");
        attempt(AuthEvent::Tokens(Tokens::new("alex-late", "r")));
        drop((attempt, keeper));
        assert!(rx.recv_timeout(testkit::slack(testkit::WAIT)).is_err());
    }
}
