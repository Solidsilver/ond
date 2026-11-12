//! Where Rede's books come from.
//!
//! One trait, [`Source`], says what the app may ask an account; one set of
//! models says what a book is. Neither names a server, so the app draws and
//! stores books without knowing whether they came from Audiobookshelf, a
//! folder or a feed. [`AbsSource`] is the only implementation so far.
//!
//! Modules:
//!
//! - [`id`]: [`AccountId`] and [`ItemKey`], the identity everything
//!   on disk is keyed by.
//! - [`model`]: libraries, items, tracks, chapters, positions
//!   and progress, in Rede's units.
//! - [`source`]: the trait and its capabilities.
//! - `abs_source`: Audiobookshelf, mapped onto the models.

pub mod id;
pub mod model;
pub mod source;

mod abs_source;
mod error;

pub use abs_source::AbsSource;
pub use error::{Error, Result};
pub use id::{AccountId, ItemKey, KeyError};
pub use model::*;
pub use source::Source;
