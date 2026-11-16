//! This app's build identity for the shared update checker. Environment and
//! package version macros belong here so shell never uses its own identity.

pub use shell::updates::*;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn current() -> Version {
    Version::parse(VERSION).expect("the app version is semver")
}

pub fn channel() -> Channel {
    shell::updates::channel(option_env!("OND_DISTRIBUTION"))
}
