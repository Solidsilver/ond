//! What the tests build their tracks from, across the crate. (The UI's
//! fixture library, with its albums and queue, is `app::fixture`; it builds
//! its tracks here too.) `Track` has no `Default`: a track without an id,
//! album and title means nothing, and production code shouldn't be able to
//! make one by accident. Tests start from [`track`] and set what they're
//! about with struct update syntax, so adding a field to `Track` touches
//! this file alone.

use library::Track;

/// A track with an id, an album and a title, and nothing else known.
pub fn track(id: &str, album: &str, title: &str) -> Track {
    Track {
        id: id.into(),
        album_id: album.into(),
        title: title.into(),
        artist: None,
        artist_ref: None,
        track: None,
        disc: None,
        duration_secs: None,
        suffix: None,
        content_type: None,
        size: None,
        bit_rate_kbps: None,
        bit_depth: None,
        sample_rate: None,
        channels: None,
        track_gain: None,
        album_gain: None,
        track_peak: None,
        album_peak: None,
        composer: None,
        contributors: Vec::new(),
        cover_art: None,
        artists: Vec::new(),
    }
}

/// One stable account across the UI and playback fixtures.
pub fn account() -> accounts::AccountId {
    "00000000-0000-4000-8000-000000000001".parse().unwrap()
}

pub fn key(id: &str) -> crate::source::TrackKey {
    crate::source::TrackKey::new(account(), id)
}

/// A 64 px PNG filled with one colour, for a cover a fake source serves or the
/// cache already holds.
pub fn png(rgba: [u8; 4]) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(64, 64, image::Rgba(rgba));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}
