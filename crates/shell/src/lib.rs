//! What the apps in the Ond family share on screen: the theme, the window
//! and its title bar, the widgets every app draws with, and the machinery
//! under a cover-art cache.
//!
//! Code comes here from an app when a second app needs it, not before, so
//! what's here has been used twice (or is about to be). Nothing in it knows
//! which app it's drawing for.

pub mod art;
pub mod chrome;
pub mod colors;
pub mod csd;
pub mod icons;
pub mod keys;
pub mod kit;
pub mod now_playing;
pub mod over_art;
pub mod rich;
pub mod theme;
pub mod widgets;
pub mod window;
