//! What the apps in the Ond family share on screen: the theme, the window
//! and its title bar, and the widgets every app draws with.
//!
//! Code comes here from an app when a second app needs it, not before, so
//! what's here has been used twice (or is about to be). Nothing in it knows
//! which app it's drawing for.

pub mod chrome;
pub mod colors;
pub mod csd;
pub mod icons;
pub mod keys;
pub mod over_art;
pub mod theme;
pub mod window;
