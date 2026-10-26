//! When a play counts, and what to do with plays the server didn't take.
//!
//! The rule is Last.fm's: a track longer than 30 s counts once it has actually
//! been listened to for half its length or four minutes, whichever comes first.
//! "Listened" means the position advanced while playing: seeking ahead doesn't
//! count, and paused or buffering time doesn't either.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// Tracks this short are never scrobbled.
const MIN_TRACK_SECS: f64 = 30.0;
/// Listening this long always counts, however long the track.
const MAX_THRESHOLD_SECS: f64 = 240.0;
/// How far the position may run ahead of the wall clock between two looks
/// before the jump is taken for a seek. Covers late wake-ups of the poller.
const SLACK_SECS: f64 = 1.0;

/// Seconds of listening a track needs before it's scrobbled; `None` if it never is.
pub fn threshold(duration_secs: Option<f64>) -> Option<f64> {
    match duration_secs {
        Some(d) if d <= MIN_TRACK_SECS => None,
        Some(d) => Some((d / 2.0).min(MAX_THRESHOLD_SECS)),
        // Unknown length: only a long listen is safe to count.
        None => Some(MAX_THRESHOLD_SECS),
    }
}

/// Listening time for one play of one track.
#[derive(Debug, Clone)]
pub struct Listen {
    pub track_id: String,
    /// When playback of this track started, ms since the Unix epoch.
    pub started_ms: u64,
    duration_secs: Option<f64>,
    listened: f64,
    /// The last (position, wall clock) seen while the track was current.
    last: Option<(f64, f64)>,
    was_playing: bool,
    submitted: bool,
}

impl Listen {
    pub fn new(track_id: impl Into<String>, started_ms: u64, duration_secs: Option<f64>) -> Self {
        Self { track_id: track_id.into(), started_ms, duration_secs, listened: 0.0, last: None, was_playing: false, submitted: false }
    }

    /// The length may only become known once the engine has opened the file.
    pub fn set_duration(&mut self, secs: Option<f64>) {
        if self.duration_secs.is_none() {
            self.duration_secs = secs;
        }
    }

    /// One look at the playhead: `pos` in seconds, `now` a monotonic clock in seconds.
    pub fn observe(&mut self, pos: f64, playing: bool, now: f64) {
        if let Some((last_pos, last_now)) = self.last
            && playing
            && self.was_playing
        {
            let moved = pos - last_pos;
            let elapsed = (now - last_now).max(0.0);
            // Backwards or further than time allows: a seek, not listening.
            if moved > 0.0 && moved <= elapsed + SLACK_SECS {
                self.listened += moved;
            }
        }
        self.last = Some((pos, now));
        self.was_playing = playing;
    }

    /// A seek landed at `pos`; the jump to it is not listening time.
    pub fn seeked(&mut self, pos: f64, now: f64) {
        self.last = Some((pos, now));
    }

    #[cfg(test)]
    pub fn listened_secs(&self) -> f64 {
        self.listened
    }

    /// True once, the first time the play counts; the caller submits it then.
    pub fn take_due(&mut self) -> bool {
        if self.submitted {
            return false;
        }
        match threshold(self.duration_secs) {
            Some(need) if self.listened >= need => {
                self.submitted = true;
                true
            }
            _ => false,
        }
    }
}

/// A play the server hasn't confirmed yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingPlay {
    pub id: String,
    pub time_ms: u64,
}

/// Plays waiting to be resubmitted, oldest first, kept in a small JSON file so
/// they survive restarts (offline, or away from the server).
#[derive(Debug)]
pub struct RetryQueue {
    path: Option<PathBuf>,
    items: VecDeque<PendingPlay>,
}

/// More than this and the oldest plays are dropped.
const MAX_PENDING: usize = 5000;

impl RetryQueue {
    /// Loads the queue from `path` (a missing or unreadable file is an empty queue).
    pub fn load(path: &Path) -> Self {
        let items = std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<Vec<PendingPlay>>(&b).ok()).unwrap_or_default();
        Self { path: Some(path.to_owned()), items: items.into() }
    }

    /// A queue that isn't saved anywhere.
    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self { path: None, items: VecDeque::new() }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn front(&self) -> Option<&PendingPlay> {
        self.items.front()
    }

    pub fn push(&mut self, play: PendingPlay) {
        if self.items.contains(&play) {
            return;
        }
        self.items.push_back(play);
        while self.items.len() > MAX_PENDING {
            self.items.pop_front();
        }
        self.save();
    }

    /// Takes the oldest play off once the server accepted (or permanently refused) it.
    pub fn pop_front(&mut self) -> Option<PendingPlay> {
        let p = self.items.pop_front();
        if p.is_some() {
            self.save();
        }
        p
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        let items: Vec<&PendingPlay> = self.items.iter().collect();
        // On the now-playing worker, so waiting for the disk holds nothing up.
        let result = serde_json::to_vec(&items)
            .map_err(std::io::Error::other)
            .and_then(|bytes| crate::prefs::replace_file(path, &path.with_extension("json.part"), &bytes));
        if let Err(e) = result {
            tracing::warn!("saving pending scrobbles: {e}");
        }
    }
}

/// What to do with a pending play after trying to submit it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Accepted: drop it.
    Done,
    /// The server answered but won't take it (unknown id, ...): drop it too.
    Refused,
    /// Couldn't reach the server: keep it and stop trying for now.
    Unreachable,
}

impl Outcome {
    pub fn of(result: &subsonic::Result<()>) -> Self {
        match result {
            Ok(()) => Outcome::Done,
            Err(subsonic::Error::Http(_)) => Outcome::Unreachable,
            Err(_) => Outcome::Refused,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plays `secs` of continuous listening from `from`, looking every half second.
    fn play(l: &mut Listen, from: f64, secs: f64, clock: &mut f64) -> f64 {
        let mut pos = from;
        let end = from + secs;
        l.observe(pos, true, *clock);
        while pos < end {
            pos = (pos + 0.5).min(end);
            *clock += 0.5;
            l.observe(pos, true, *clock);
        }
        pos
    }

    #[test]
    fn threshold_follows_lastfm() {
        assert_eq!(threshold(Some(20.0)), None);
        assert_eq!(threshold(Some(30.0)), None);
        assert_eq!(threshold(Some(200.0)), Some(100.0));
        assert_eq!(threshold(Some(600.0)), Some(240.0));
        assert_eq!(threshold(None), Some(240.0));
    }

    #[test]
    fn continuous_listening_counts_once() {
        let mut l = Listen::new("a", 0, Some(200.0));
        let mut clock = 0.0;
        play(&mut l, 0.0, 99.0, &mut clock);
        assert!(!l.take_due());
        play(&mut l, 99.0, 2.0, &mut clock);
        assert!(l.take_due());
        play(&mut l, 101.0, 50.0, &mut clock);
        assert!(!l.take_due(), "a play is submitted once");
    }

    #[test]
    fn seeking_ahead_is_not_listening() {
        let mut l = Listen::new("a", 0, Some(200.0));
        let mut clock = 0.0;
        play(&mut l, 0.0, 10.0, &mut clock);
        // Jump to near the end without the engine telling us (seen as a jump)...
        clock += 0.5;
        l.observe(180.0, true, clock);
        play(&mut l, 180.0, 19.0, &mut clock);
        assert!((l.listened_secs() - 29.0).abs() < 0.01, "listened {}", l.listened_secs());
        assert!(!l.take_due());
        // ...and with it telling us.
        let mut l = Listen::new("b", 0, Some(200.0));
        let mut clock = 0.0;
        play(&mut l, 0.0, 10.0, &mut clock);
        clock += 0.1;
        l.seeked(150.0, clock);
        play(&mut l, 150.0, 20.0, &mut clock);
        assert!((l.listened_secs() - 30.0).abs() < 0.01);
    }

    #[test]
    fn seeking_back_and_relistening_counts() {
        // Listening is time spent, not distinct parts of the track.
        let mut l = Listen::new("a", 0, Some(100.0));
        let mut clock = 0.0;
        play(&mut l, 0.0, 30.0, &mut clock);
        l.seeked(0.0, clock);
        play(&mut l, 0.0, 25.0, &mut clock);
        assert!(l.take_due());
    }

    #[test]
    fn paused_and_buffering_time_does_not_count() {
        let mut l = Listen::new("a", 0, Some(100.0));
        let mut clock = 0.0;
        play(&mut l, 0.0, 20.0, &mut clock);
        // Paused for a long time; the position doesn't move.
        for _ in 0..600 {
            clock += 0.5;
            l.observe(20.0, false, clock);
        }
        assert!((l.listened_secs() - 20.0).abs() < 0.01);
        // Resuming picks up from where it was, without a jump.
        play(&mut l, 20.0, 31.0, &mut clock);
        assert!(l.take_due());
    }

    #[test]
    fn a_late_look_still_counts_real_progress() {
        // The poller woke up late: 3 s of wall time, 3 s of position.
        let mut l = Listen::new("a", 0, Some(100.0));
        l.observe(0.0, true, 0.0);
        l.observe(3.0, true, 3.0);
        assert!((l.listened_secs() - 3.0).abs() < 0.01);
    }

    #[test]
    fn gapless_boundary_starts_a_fresh_listen() {
        // The old listen may see the next track's position before the start event;
        // that's a backwards jump and is ignored.
        let mut old = Listen::new("a", 0, Some(100.0));
        let mut clock = 0.0;
        play(&mut old, 0.0, 99.5, &mut clock);
        assert!(old.take_due());
        clock += 0.5;
        old.observe(0.3, true, clock);
        assert!((old.listened_secs() - 99.5).abs() < 0.01);
        let mut new = Listen::new("b", 1, Some(100.0));
        play(&mut new, 0.3, 49.0, &mut clock);
        assert!(!new.take_due());
        play(&mut new, 49.3, 1.0, &mut clock);
        assert!(new.take_due());
    }

    #[test]
    fn short_tracks_never_count() {
        let mut l = Listen::new("a", 0, Some(25.0));
        let mut clock = 0.0;
        play(&mut l, 0.0, 25.0, &mut clock);
        assert!(!l.take_due());
    }

    #[test]
    fn duration_learned_later_applies() {
        let mut l = Listen::new("a", 0, None);
        let mut clock = 0.0;
        play(&mut l, 0.0, 60.0, &mut clock);
        assert!(!l.take_due(), "unknown length needs four minutes");
        l.set_duration(Some(100.0));
        assert!(l.take_due());
    }

    #[test]
    fn retry_queue_persists_dedups_and_caps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scrobbles.json");
        let mut q = RetryQueue::load(&path);
        assert!(q.is_empty());
        q.push(PendingPlay { id: "a".into(), time_ms: 1 });
        q.push(PendingPlay { id: "b".into(), time_ms: 2 });
        q.push(PendingPlay { id: "a".into(), time_ms: 1 });
        assert_eq!(q.len(), 2);

        let mut again = RetryQueue::load(&path);
        assert_eq!(again.len(), 2);
        assert_eq!(again.pop_front().unwrap().id, "a");
        assert_eq!(RetryQueue::load(&path).front().unwrap().id, "b");

        let mut big = RetryQueue::in_memory();
        for i in 0..(MAX_PENDING as u64 + 10) {
            big.push(PendingPlay { id: format!("t{i}"), time_ms: i });
        }
        assert_eq!(big.len(), MAX_PENDING);
        assert_eq!(big.front().unwrap().time_ms, 10, "the oldest go first");
    }

    #[test]
    fn unreadable_queue_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scrobbles.json");
        std::fs::write(&path, b"not json").unwrap();
        assert!(RetryQueue::load(&path).is_empty());
    }

    #[test]
    fn outcomes() {
        assert_eq!(Outcome::of(&Ok(())), Outcome::Done);
        assert_eq!(Outcome::of(&Err(subsonic::Error::Api { code: 70, message: "not found".into() })), Outcome::Refused);
    }
}
