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
