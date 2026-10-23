//! ReplayGain: what gain a track gets, and how it's kept from clipping.
//!
//! The gain is applied in the output callback, folded into the volume multiply
//! (see [`crate::output::RingInput::fill`]). Each track's linear gain sits in a
//! small table on [`crate::Status`] indexed by session sequence number; the
//! callback switches to the next track's entry on the exact sample where that
//! track starts, so gapless albums with different track gains stay seamless.
//! Bit-perfect mode never applies it.

use std::sync::atomic::{AtomicU32, Ordering};

/// Gains and peaks as tagged (or as the server reports them). Gains are in dB
/// relative to the ReplayGain reference; peaks are linear (1.0 = full scale).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ReplayGain {
    pub track_gain: Option<f32>,
    pub album_gain: Option<f32>,
    pub track_peak: Option<f32>,
    pub album_peak: Option<f32>,
}

impl ReplayGain {
    pub fn is_empty(&self) -> bool {
        self.track_gain.is_none() && self.album_gain.is_none()
    }

    /// Fills whatever `self` lacks from `other` (server data first, file tags second).
    pub fn or(self, other: ReplayGain) -> ReplayGain {
        ReplayGain {
            track_gain: self.track_gain.or(other.track_gain),
            album_gain: self.album_gain.or(other.album_gain),
            track_peak: self.track_peak.or(other.track_peak),
            album_peak: self.album_peak.or(other.album_peak),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GainMode {
    #[default]
    Off,
    Track,
    Album,
    /// Album gain while the queue plays an album in order, else track gain.
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GainConfig {
    pub mode: GainMode,
    /// Added to the tagged gain.
    pub preamp_db: f32,
    /// Used instead of a tagged gain when a track has none. Tagged tracks land
    /// near −18 LUFS; untagged modern masters are often around −9 LUFS, older or
    /// classical ones quieter. −6 dB keeps untagged tracks from jumping out
    /// without burying the quiet ones. The pre-amp isn't added to it.
    pub fallback_db: f32,
}

impl Default for GainConfig {
    fn default() -> Self {
        Self { mode: GainMode::Off, preamp_db: 0.0, fallback_db: -6.0 }
    }
}

/// Highest sample level a gain may push a tagged peak to: −1 dBFS.
const PEAK_CEILING: f32 = 0.891_250_9;

/// Which gain a track got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GainSource {
    Track,
    Album,
    /// The track has no ReplayGain info: [`GainConfig::fallback_db`].
    Untagged,
}

/// The gain applied to one track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppliedGain {
    /// Linear factor applied to samples.
    pub linear: f32,
    pub source: GainSource,
    /// Lowered to keep the tagged peak under −1 dBFS.
    pub limited: bool,
}

impl AppliedGain {
    pub fn db(&self) -> f32 {
        20.0 * self.linear.max(1e-9).log10()
    }
}

/// The gain for a track with `rg` tags. `album_context` says whether it plays
/// as part of its album (for [`GainMode::Auto`]). `None` when ReplayGain is off.
///
/// Clipping: with a peak known, the gain is capped so the peak stays at or
/// below −1 dBFS. Without one, a positive gain is capped at 0 dB, since the
/// source may already reach full scale. No limiter is needed beyond that: the
/// cap is exact for tagged peaks, and the rare mis-tagged file only clips if
/// its real peak exceeds what it claims.
pub fn applied_gain(cfg: &GainConfig, rg: &ReplayGain, album_context: bool) -> Option<AppliedGain> {
    let use_album = match cfg.mode {
        GainMode::Off => return None,
        GainMode::Track => false,
        GainMode::Album => true,
        GainMode::Auto => album_context,
    };
    let album = rg.album_gain.map(|g| (g, rg.album_peak.or(rg.track_peak), GainSource::Album));
    let track = rg.track_gain.map(|g| (g, rg.track_peak, GainSource::Track));
    let chosen = if use_album { album.or(track) } else { track.or(album) };
    let Some((gain_db, peak, source)) = chosen else {
        return Some(AppliedGain { linear: db_to_linear(cfg.fallback_db.min(0.0)), source: GainSource::Untagged, limited: false });
    };
    let wanted = db_to_linear(gain_db + cfg.preamp_db);
    let ceiling = match peak {
        Some(p) if p > 0.0 => PEAK_CEILING / p,
        _ => 1.0,
    };
    let linear = wanted.min(ceiling);
    Some(AppliedGain { linear, source, limited: linear < wanted })
}

pub fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Where a track sits in its album, for [`album_context`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlbumPos<'a> {
    pub album_id: &'a str,
    pub disc: u32,
    pub track: u32,
}

impl AlbumPos<'_> {
    fn before(&self, other: &AlbumPos<'_>) -> bool {
        self.album_id == other.album_id && (self.disc, self.track) < (other.disc, other.track)
    }
}

/// Whether `cur` plays as part of its album: the queue item before it is an
/// earlier track of the same album, or the one after it a later one. A lone
/// album track, or an album shuffled, gets track gain under [`GainMode::Auto`].
pub fn album_context(prev: Option<AlbumPos<'_>>, cur: AlbumPos<'_>, next: Option<AlbumPos<'_>>) -> bool {
    prev.is_some_and(|p| p.before(&cur)) || next.is_some_and(|n| cur.before(&n))
}

/// Parses tag values like `-7.23 dB` or `+1.5dB` (gain) and `0.988` (peak).
pub fn parse_tag_value(s: &str) -> Option<f32> {
    let s = s.trim();
    let end = s.find(|c: char| !(c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | 'e' | 'E'))).unwrap_or(s.len());
    s[..end].parse::<f32>().ok().filter(|v| v.is_finite())
}

/// Slots in the per-track gain table. A decoder more than this many tracks
/// ahead of the output would overwrite a live entry; with a few seconds of
/// buffering that needs dozens of sub-second tracks.
pub(crate) const GAIN_SLOTS: usize = 64;

/// Linear gain per session track (by sequence number, wrapping), read by the
/// realtime callback. 1.0 until set.
#[derive(Debug)]
pub(crate) struct TrackGains([AtomicU32; GAIN_SLOTS]);

impl Default for TrackGains {
    fn default() -> Self {
        Self(std::array::from_fn(|_| AtomicU32::new(1f32.to_bits())))
    }
}

impl TrackGains {
    pub fn get(&self, seq: u64) -> f32 {
        f32::from_bits(self.0[seq as usize % GAIN_SLOTS].load(Ordering::Acquire))
    }

    pub fn set(&self, seq: u64, linear: f32) {
        self.0[seq as usize % GAIN_SLOTS].store(linear.to_bits(), Ordering::Release);
    }

    pub fn reset(&self) {
        for g in &self.0 {
            g.store(1f32.to_bits(), Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rg(tg: f32, tp: f32, ag: f32, ap: f32) -> ReplayGain {
        ReplayGain { track_gain: Some(tg), album_gain: Some(ag), track_peak: Some(tp), album_peak: Some(ap) }
    }

    fn cfg(mode: GainMode) -> GainConfig {
        GainConfig { mode, ..Default::default() }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn off_applies_nothing() {
        assert_eq!(applied_gain(&cfg(GainMode::Off), &rg(-7.0, 0.9, -8.0, 1.0), true), None);
    }

    #[test]
    fn track_and_album_pick_their_gain() {
        let r = rg(-7.0, 0.5, -9.0, 0.6);
        let t = applied_gain(&cfg(GainMode::Track), &r, true).unwrap();
        assert_eq!(t.source, GainSource::Track);
        assert!(close(t.db(), -7.0) && !t.limited);
        let a = applied_gain(&cfg(GainMode::Album), &r, false).unwrap();
        assert_eq!(a.source, GainSource::Album);
        assert!(close(a.db(), -9.0));
        // Auto follows the album context.
        assert_eq!(applied_gain(&cfg(GainMode::Auto), &r, true).unwrap().source, GainSource::Album);
        assert_eq!(applied_gain(&cfg(GainMode::Auto), &r, false).unwrap().source, GainSource::Track);
    }

    #[test]
    fn missing_album_gain_falls_back_to_track_and_vice_versa() {
        let only_track = ReplayGain { track_gain: Some(-5.0), track_peak: Some(0.5), ..Default::default() };
        let g = applied_gain(&cfg(GainMode::Album), &only_track, true).unwrap();
        assert_eq!(g.source, GainSource::Track);
        let only_album = ReplayGain { album_gain: Some(-4.0), album_peak: Some(0.5), ..Default::default() };
        assert_eq!(applied_gain(&cfg(GainMode::Track), &only_album, false).unwrap().source, GainSource::Album);
    }

    #[test]
    fn untagged_tracks_get_the_fallback_without_preamp() {
        let c = GainConfig { mode: GainMode::Track, preamp_db: 6.0, fallback_db: -6.0 };
        let g = applied_gain(&c, &ReplayGain::default(), false).unwrap();
        assert_eq!(g.source, GainSource::Untagged);
        assert!(close(g.db(), -6.0));
        // A positive fallback is never applied (nothing says the track has headroom).
        let up = GainConfig { fallback_db: 3.0, ..c };
        assert!(close(applied_gain(&up, &ReplayGain::default(), false).unwrap().db(), 0.0));
    }

    #[test]
    fn peaks_keep_gain_under_minus_one_dbfs() {
        // +6 dB on a track peaking at 0.8 would reach 1.6: capped so the peak lands at −1 dBFS.
        let r = ReplayGain { track_gain: Some(6.0), track_peak: Some(0.8), ..Default::default() };
        let g = applied_gain(&cfg(GainMode::Track), &r, false).unwrap();
        assert!(g.limited);
        assert!(close(0.8 * g.linear, PEAK_CEILING));
        // Pre-amp pushing a negative gain over the ceiling is capped the same way.
        let r = ReplayGain { track_gain: Some(-2.0), track_peak: Some(1.0), ..Default::default() };
        let c = GainConfig { preamp_db: 6.0, ..cfg(GainMode::Track) };
        let g = applied_gain(&c, &r, false).unwrap();
        assert!(g.limited && close(g.linear, PEAK_CEILING));
        // Headroom left: untouched.
        let r = ReplayGain { track_gain: Some(-8.0), track_peak: Some(1.0), ..Default::default() };
        assert!(!applied_gain(&cfg(GainMode::Track), &r, false).unwrap().limited);
    }

    #[test]
    fn positive_gain_without_peak_is_capped_at_unity() {
        let r = ReplayGain { track_gain: Some(4.0), ..Default::default() };
        let g = applied_gain(&cfg(GainMode::Track), &r, false).unwrap();
        assert!(g.limited && close(g.linear, 1.0));
        let r = ReplayGain { track_gain: Some(-4.0), ..Default::default() };
        let g = applied_gain(&cfg(GainMode::Track), &r, false).unwrap();
        assert!(!g.limited && close(g.db(), -4.0));
    }

    #[test]
    fn album_peak_falls_back_to_track_peak() {
        let r = ReplayGain { album_gain: Some(5.0), track_peak: Some(0.9), ..Default::default() };
        let g = applied_gain(&cfg(GainMode::Album), &r, true).unwrap();
        assert!(g.limited && close(0.9 * g.linear, PEAK_CEILING));
    }

    fn pos(album: &str, disc: u32, track: u32) -> AlbumPos<'_> {
        AlbumPos { album_id: album, disc, track }
    }

    #[test]
    fn auto_detects_albums_played_in_order() {
        let (a1, a2, a3) = (pos("a", 1, 1), pos("a", 1, 2), pos("a", 1, 3));
        // Middle of an album played in order.
        assert!(album_context(Some(a1), a2, Some(a3)));
        // First and last tracks of the run count too.
        assert!(album_context(None, a1, Some(a2)));
        assert!(album_context(Some(a2), a3, None));
        // Across discs.
        assert!(album_context(Some(pos("a", 1, 12)), pos("a", 2, 1), None));
        // A lone album track between other things.
        assert!(!album_context(Some(pos("b", 1, 4)), a2, Some(pos("c", 1, 1))));
        assert!(!album_context(None, a2, None));
        // The same album shuffled: neighbours out of order.
        assert!(!album_context(Some(a3), a1, Some(pos("a", 1, 1))));
        assert!(!album_context(Some(a3), a2, Some(a1)));
    }

    #[test]
    fn tag_values_parse() {
        assert_eq!(parse_tag_value("-7.23 dB"), Some(-7.23));
        assert_eq!(parse_tag_value(" +1.50dB"), Some(1.5));
        assert_eq!(parse_tag_value("0.988525"), Some(0.988_525));
        assert_eq!(parse_tag_value("dB"), None);
        assert_eq!(parse_tag_value(""), None);
    }

    #[test]
    fn gain_table_defaults_to_unity_and_wraps() {
        let t = TrackGains::default();
        assert_eq!(t.get(5), 1.0);
        t.set(3, 0.5);
        assert_eq!(t.get(3), 0.5);
        assert_eq!(t.get(3 + GAIN_SLOTS as u64), 0.5);
        t.reset();
        assert_eq!(t.get(3), 1.0);
    }
}
