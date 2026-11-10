//! Waiting on something another thread (or a device, or a socket) will make
//! true. Time budgets in tests are tight on purpose: they guard real
//! performance, and a machine that is merely slow should not fail them.

use std::time::{Duration, Instant};

/// The usual wait for something a test can't hurry along.
pub const WAIT: Duration = Duration::from_secs(10);

/// How often a wait looks again: fine enough for a callback or a connect.
const POLL: Duration = Duration::from_millis(1);

/// Time budgets in these tests are tight on purpose (they guard real
/// performance), but shared CI runners are slower and noisier: allow 10× there.
pub fn slack(d: Duration) -> Duration {
    if std::env::var_os("CI").is_some() { d * 10 } else { d }
}

/// The end of a wait, for a loop that does more than look (drives a fake
/// device, collects messages) or that wants to say more than [`wait_until`]
/// can when it gives up: `assert!(!deadline.passed(), "stuck at {pos}")`.
#[derive(Clone, Copy, Debug)]
pub struct Deadline(Instant);

impl Deadline {
    /// [`WAIT`] from now, stretched by [`slack`].
    pub fn new() -> Self {
        Self::within(WAIT)
    }

    /// `timeout` from now, stretched by [`slack`].
    pub fn within(timeout: Duration) -> Self {
        Self(Instant::now() + slack(timeout))
    }

    pub fn passed(&self) -> bool {
        Instant::now() >= self.0
    }

    /// What's left, for a blocking call's own timeout (`recv_timeout`).
    pub fn left(&self) -> Duration {
        self.0.saturating_duration_since(Instant::now())
    }
}

impl Default for Deadline {
    fn default() -> Self {
        Self::new()
    }
}

/// Waits until `cond` holds, panicking with `what` after [`WAIT`].
#[track_caller]
pub fn wait_until(what: &str, cond: impl FnMut() -> bool) {
    wait_until_within(what, WAIT, cond);
}

/// [`wait_until`] with a budget of your own (stretched by [`slack`]).
#[track_caller]
pub fn wait_until_within(what: &str, timeout: Duration, mut cond: impl FnMut() -> bool) {
    let deadline = Deadline::within(timeout);
    while !cond() {
        assert!(!deadline.passed(), "timed out waiting for {what}");
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_condition_that_never_holds_times_out_naming_what_was_waited_for() {
        let r = std::panic::catch_unwind(|| wait_until_within("the thing", Duration::from_millis(20), || false));
        let err = r.expect_err("must panic");
        let text = err.downcast_ref::<String>().map(String::as_str).unwrap_or_default();
        assert_eq!(text, "timed out waiting for the thing");
    }

    #[test]
    fn a_condition_that_holds_at_once_doesnt_sleep() {
        let t = Instant::now();
        wait_until("nothing", || true);
        assert!(t.elapsed() < POLL);
    }

    #[test]
    fn slack_stretches_a_budget_only_on_ci() {
        let d = Duration::from_millis(50);
        // The variable is read per call, so this works in either environment.
        match std::env::var_os("CI") {
            Some(_) => assert_eq!(slack(d), d * 10),
            None => assert_eq!(slack(d), d),
        }
    }
}
