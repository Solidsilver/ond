//! Helpers the workspace's tests share. Dev-only: no app code is built
//! against it, and it depends on no other workspace crate, so every crate can
//! take it as a dev-dependency without a cycle.
//!
//! - [`wait_until`], [`Deadline`] and [`slack`]: waiting on something a test
//!   can't hurry along, with one budget ([`WAIT`]) the shared runners in CI
//!   stretch.
//! - [`RangeServer`]: a local file server for download caches, with Range
//!   support, a trickle, a gate that holds the body part-way, and the wrong
//!   answers a broken server or proxy gives.
//! - [`JsonFake`]: a local server that answers each request from a closure and
//!   keeps what it saw, for API clients.

mod http;
mod json;
mod wait;

pub use http::{RangeServer, Serve};
pub use json::{Answer, JsonFake, Seen};
pub use wait::{Deadline, WAIT, slack, wait_until, wait_until_within};
