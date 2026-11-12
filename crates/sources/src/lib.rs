//! Where Rede's books come from.
//!
//! One trait, [`Source`], says what the app may ask an account; one set of
//! models says what a book is. Neither names a server, so the app draws and
//! stores books without knowing whether they came from Audiobookshelf, a
//! folder or a feed. Identity comes from the `accounts` crate so both apps
//! qualify items the same way. [`AbsSource`] is the only implementation so far.
//!
//! Modules:
//!
//! - [`model`]: libraries, items, tracks, chapters, positions
//!   and progress, in Rede's units.
//! - [`source`]: the trait and its capabilities.
//! - `abs_source`: Audiobookshelf, mapped onto the models.

pub mod model;
pub mod source;

mod abs_source;
mod error;

pub use abs_source::AbsSource;
pub use error::{Error, Result};
pub use model::*;
pub use source::Source;
