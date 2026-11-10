//! What the apps in the Ond family share on screen: the theme, the window
//! and its title bar, the widgets every app draws with, and the machinery
//! under a cover-art cache.
//!
//! The widgets are in [`kit`]: anything either app could draw goes there
//! first, so the two look alike; what's one app's own stays in that app as a
//! thin wrapper. The ranking behind both apps' quick search is in [`search`];
//! how a time of day is said, in [`clock`].
//! Nothing here knows which app it's drawing for.

pub mod art;
pub mod chrome;
pub mod clock;
pub mod colors;
pub mod csd;
pub mod icons;
pub mod keys;
pub mod kit;
pub mod now_playing;
pub mod over_art;
pub mod rich;
pub mod search;
pub mod theme;
pub mod window;
pub mod zoom;
