//! The play queue: what played, what's playing, what's next, and where each
//! upcoming item came from. Pure data; the app feeds the engine from it.
//!
//! Layout: `history` (oldest first) · `current` · `upcoming` (next first).
//! Items the user queued by hand carry [`Source::Added`]. Where "play next"
//! and "add to queue" put them, and what "play now" does with them, follow
//! [`QueuePrefs`].

use chrono::NaiveTime;
use library::Track;
use serde::{Deserialize, Serialize};

/// Where "play next" puts tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NextAt {
    /// Right after the current track: the latest plays first.
    #[default]
    Top,
    /// After what was played next before, in the order chosen.
    AfterNext,
}

impl NextAt {
    pub const ALL: [NextAt; 2] = [NextAt::Top, NextAt::AfterNext];

    pub fn label(self) -> &'static str {
        match self {
            NextAt::Top => "Top",
            NextAt::AfterNext => "In turn",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            NextAt::Top => "Right after the current track, ahead of anything played next before",
            NextAt::AfterNext => "After anything played next before, in the order you chose them",
        }
    }
}

/// Where "add to queue" puts tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AddAt {
    /// At the very end.
    #[default]
    End,
    /// After the other tracks you added, ahead of the album or playlist.
    BeforeSource,
}

impl AddAt {
    pub const ALL: [AddAt; 2] = [AddAt::End, AddAt::BeforeSource];

    pub fn label(self) -> &'static str {
        match self {
            AddAt::End => "End",
            AddAt::BeforeSource => "Up front",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            AddAt::End => "At the end, after the rest of the album or playlist",
            AddAt::BeforeSource => "After the other tracks you added, ahead of the rest of the album or playlist",
        }
    }
}

/// What playing an album, playlist or song does with the tracks you added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OnPlay {
    /// Kept, to play once the new music ends.
    #[default]
    KeepAfter,
    /// Kept, and played before the rest of the new music.
    KeepBefore,
    /// Cleared along with everything else.
    Clear,
    /// Ask whether to keep them (the app asks; the queue keeps them after).
    Ask,
}

impl OnPlay {
    pub const ALL: [OnPlay; 4] = [OnPlay::KeepAfter, OnPlay::KeepBefore, OnPlay::Clear, OnPlay::Ask];

    pub fn label(self) -> &'static str {
        match self {
            OnPlay::KeepAfter => "Play after",
            OnPlay::KeepBefore => "Play first",
            OnPlay::Clear => "Clear",
            OnPlay::Ask => "Ask",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            OnPlay::KeepAfter => "Tracks you added wait until the new music ends",
            OnPlay::KeepBefore => "Tracks you added play right after the first new track, then the rest of it",
            OnPlay::Clear => "The whole queue is replaced, tracks you added too",
            OnPlay::Ask => "When you've added tracks, Ond asks whether to keep them (for after the new music) or clear them",
        }
    }
}

/// What the queue's Clear removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClearWhat {
    /// Everything up next.
    #[default]
    Everything,
    /// Only the tracks added by hand; the album or playlist stays.
    Added,
}

impl ClearWhat {
    pub const ALL: [ClearWhat; 2] = [ClearWhat::Everything, ClearWhat::Added];

    pub fn label(self) -> &'static str {
        match self {
            ClearWhat::Everything => "Everything",
            ClearWhat::Added => "Yours",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            ClearWhat::Everything => "Everything up next, the rest of the album or playlist too",
            ClearWhat::Added => "Only the tracks you added; the rest of the album or playlist stays",
        }
    }
}

/// How the queue behaves. Saved with the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct QueuePrefs {
    pub play_next: NextAt,
    pub add: AddAt,
    pub on_play: OnPlay,
    pub clear: ClearWhat,
}

/// Named sets of [`QueuePrefs`], for people used to another player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Ond,
    AppleMusic,
    Spotify,
    /// Anything else (only ever shown, never picked).
    Custom,
}

impl Preset {
    pub const ALL: [Preset; 4] = [Preset::Ond, Preset::AppleMusic, Preset::Spotify, Preset::Custom];

    pub fn label(self) -> &'static str {
        match self {
            Preset::Ond => "Ond",
            Preset::AppleMusic => "Apple Music",
            Preset::Spotify => "Spotify",
            Preset::Custom => "Custom",
        }
    }

    pub fn prefs(self) -> Option<QueuePrefs> {
        match self {
            Preset::Ond => Some(QueuePrefs::default()),
            Preset::AppleMusic => Some(QueuePrefs { play_next: NextAt::Top, add: AddAt::End, on_play: OnPlay::Ask, clear: ClearWhat::Everything }),
            Preset::Spotify => {
                Some(QueuePrefs { play_next: NextAt::AfterNext, add: AddAt::BeforeSource, on_play: OnPlay::KeepBefore, clear: ClearWhat::Added })
            }
            Preset::Custom => None,
        }
    }

    pub fn of(prefs: QueuePrefs) -> Preset {
        Preset::ALL.into_iter().find(|p| p.prefs() == Some(prefs)).unwrap_or(Preset::Custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Queued by hand: "play next" or "add to queue".
    Added,
    Album {
        id: String,
        name: String,
    },
    Playlist {
        id: String,
        name: String,
    },
    ArtistShuffle {
        name: String,
    },
}

impl Source {
    pub fn label(&self) -> &str {
        match self {
            Source::Added => "You added",
            Source::Album { name, .. } | Source::Playlist { name, .. } | Source::ArtistShuffle { name } => name,
        }
    }

    /// Stable key for remembering per-group UI state (collapsed or not).
    pub fn key(&self) -> String {
        match self {
            Source::Added => "added".into(),
            Source::Album { id, .. } => format!("album:{id}"),
            Source::Playlist { id, .. } => format!("playlist:{id}"),
            Source::ArtistShuffle { name } => format!("artist:{name}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Item {
    /// Unique within the queue, so the same track can be queued twice.
    pub uid: u64,
    pub track: Track,
    pub source: Source,
    /// Queued with "play next" (for [`NextAt::AfterNext`]).
    pub next: bool,
}

/// What happens when the last upcoming item finishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EndAction {
    #[default]
    Stop,
    /// Start again from the first item played since the last "play now".
    RepeatQueue,
}

/// A run of consecutive upcoming items with the same source.
#[derive(Debug, Clone, PartialEq)]
pub struct Group<'a> {
    pub source: &'a Source,
    /// Index into `upcoming` of the first item.
    pub start: usize,
    pub len: usize,
}

#[derive(Debug, Default)]
pub struct Queue {
    pub history: Vec<Item>,
    pub current: Option<Item>,
    pub upcoming: Vec<Item>,
    pub end: EndAction,
    pub prefs: QueuePrefs,
    /// Where in `history` the current pass started; what "repeat queue" replays.
    cycle_start: usize,
    next_uid: u64,
}

impl Queue {
    fn item(&mut self, track: Track, source: Source) -> Item {
        self.next_uid += 1;
        Item { uid: self.next_uid, track, source, next: false }
    }

    fn items(&mut self, tracks: Vec<Track>, source: &Source) -> Vec<Item> {
        tracks.into_iter().map(|t| self.item(t, source.clone())).collect()
    }

    fn retire_current(&mut self) {
        if let Some(c) = self.current.take() {
            self.history.push(c);
        }
    }

    /// Plays `tracks[start]` now and queues the rest of `tracks` after it. What
    /// was upcoming is replaced, except items the user added by hand, which
    /// stay before or after the new tracks or go too, as [`OnPlay`] says.
    pub fn play_now(&mut self, tracks: Vec<Track>, start: usize, source: Source) {
        if start >= tracks.len() {
            return;
        }
        self.retire_current();
        self.cycle_start = self.history.len();
        let mut items = self.items(tracks, &source);
        let rest = items.split_off(start + 1);
        self.current = items.pop();
        self.upcoming.retain(|i| i.source == Source::Added);
        match self.prefs.on_play {
            OnPlay::KeepAfter | OnPlay::Ask => drop(self.upcoming.splice(0..0, rest)),
            OnPlay::KeepBefore => self.upcoming.extend(rest),
            OnPlay::Clear => self.upcoming = rest,
        }
    }

    /// Queues `tracks` right after the current track, or after what was
    /// played next before them ([`NextAt`]).
    pub fn play_next(&mut self, tracks: Vec<Track>) {
        let at = match self.prefs.play_next {
            NextAt::Top => 0,
            NextAt::AfterNext => self.upcoming.iter().take_while(|i| i.next).count(),
        };
        let mut items = self.items(tracks, &Source::Added);
        items.iter_mut().for_each(|i| i.next = true);
        self.upcoming.splice(at..at, items);
    }

    /// Queues `tracks` at the end, or after the other tracks added by hand at
    /// the front ([`AddAt`]).
    pub fn add(&mut self, tracks: Vec<Track>) {
        let at = match self.prefs.add {
            AddAt::End => self.upcoming.len(),
            AddAt::BeforeSource => self.upcoming.iter().take_while(|i| i.source == Source::Added).count(),
        };
        let items = self.items(tracks, &Source::Added);
        self.upcoming.splice(at..at, items);
    }

    /// Whether what the user added is split into more than one group.
    pub fn added_is_split(&self) -> bool {
        self.groups().filter(|g| *g.source == Source::Added).count() > 1
    }

    /// Brings everything the user added together, in order, where the first
    /// of it is.
    pub fn gather_added(&mut self) {
        let Some(at) = self.upcoming.iter().position(|i| i.source == Source::Added) else { return };
        let (added, rest): (Vec<Item>, Vec<Item>) = std::mem::take(&mut self.upcoming).into_iter().partition(|i| i.source == Source::Added);
        self.upcoming = rest;
        self.upcoming.splice(at..at, added);
    }

    pub fn remove(&mut self, uid: u64) -> bool {
        let before = self.upcoming.len();
        self.upcoming.retain(|i| i.uid != uid);
        self.upcoming.len() != before
    }

    /// Clears what's up next, or only what was added by hand ([`ClearWhat`]).
    pub fn clear(&mut self) {
        match self.prefs.clear {
            ClearWhat::Everything => self.upcoming.clear(),
            ClearWhat::Added => self.upcoming.retain(|i| i.source != Source::Added),
        }
    }

    /// Whether Clear would remove anything.
    pub fn can_clear(&self) -> bool {
        match self.prefs.clear {
            ClearWhat::Everything => !self.upcoming.is_empty(),
            ClearWhat::Added => self.added_count() > 0,
        }
    }

    /// How many upcoming items the user added by hand.
    pub fn added_count(&self) -> usize {
        self.upcoming.iter().filter(|i| i.source == Source::Added).count()
    }

    pub fn position(&self, uid: u64) -> Option<usize> {
        self.upcoming.iter().position(|i| i.uid == uid)
    }

    /// Moves an upcoming item so it ends up at `to` (clamped). A moved item keeps
    /// its source, so dragging an album track into the "You added" block doesn't
    /// relabel it.
    pub fn move_to(&mut self, uid: u64, to: usize) -> bool {
        let Some(from) = self.position(uid) else { return false };
        let item = self.upcoming.remove(from);
        let to = to.min(self.upcoming.len());
        self.upcoming.insert(to, item);
        from != to
    }

    /// ⌥↑ / ⌥↓: one step earlier (`-1`) or later (`+1`).
    pub fn move_by(&mut self, uid: u64, delta: isize) -> bool {
        let Some(from) = self.position(uid) else { return false };
        let to = from.saturating_add_signed(delta).min(self.upcoming.len().saturating_sub(1));
        self.move_to(uid, to)
    }

    /// How many items one trip through the queue visits: the current item,
    /// what's upcoming and, with "repeat queue", the replayed part of history.
    pub fn pass_len(&self) -> usize {
        let replay = if self.end == EndAction::RepeatQueue { self.history.len().saturating_sub(self.cycle_start) } else { 0 };
        usize::from(self.current.is_some()) + self.upcoming.len() + replay
    }

    /// The track the engine should line up after the current one.
    pub fn peek_next(&self) -> Option<&Item> {
        match self.upcoming.first() {
            Some(i) => Some(i),
            None if self.end == EndAction::RepeatQueue => self.history.get(self.cycle_start).or(self.current.as_ref()),
            None => None,
        }
    }

    /// Moves on to the next item (the current one finished, or the user skipped).
    /// Returns the new current item; `None` means playback should stop.
    pub fn advance(&mut self) -> Option<&Item> {
        if self.upcoming.is_empty() && self.end == EndAction::RepeatQueue {
            let mut pass: Vec<Track> = self.history[self.cycle_start.min(self.history.len())..].iter().map(|i| i.track.clone()).collect();
            if let Some(c) = &self.current {
                pass.push(c.track.clone());
            }
            let sources: Vec<Source> =
                self.history[self.cycle_start.min(self.history.len())..].iter().chain(self.current.as_ref()).map(|i| i.source.clone()).collect();
            self.retire_current();
            self.cycle_start = self.history.len();
            let items: Vec<Item> = pass.into_iter().zip(sources).map(|(t, s)| self.item(t, s)).collect();
            self.upcoming = items;
        } else {
            self.retire_current();
        }
        if self.upcoming.is_empty() {
            return None;
        }
        self.current = Some(self.upcoming.remove(0));
        self.current.as_ref()
    }

    /// Back to the previous item; the current one returns to the front of upcoming.
    pub fn back(&mut self) -> Option<&Item> {
        let prev = self.history.pop()?;
        if let Some(mut c) = self.current.take() {
            c.next = false;
            self.upcoming.insert(0, c);
        }
        self.cycle_start = self.cycle_start.min(self.history.len());
        self.current = Some(prev);
        self.current.as_ref()
    }

    /// Plays an upcoming item now; the ones before it count as skipped (history).
    pub fn jump_to(&mut self, uid: u64) -> bool {
        let Some(at) = self.position(uid) else { return false };
        self.retire_current();
        let skipped: Vec<Item> = self.upcoming.drain(..at).collect();
        self.history.extend(skipped);
        self.current = Some(self.upcoming.remove(0));
        true
    }

    /// Plays a track from history again, right now, without disturbing what's next.
    pub fn replay(&mut self, uid: u64) -> bool {
        let Some(track) = self.history.iter().find(|i| i.uid == uid).map(|i| i.track.clone()) else { return false };
        self.retire_current();
        let item = self.item(track, Source::Added);
        self.current = Some(item);
        true
    }

    /// Consecutive runs of upcoming items by source (borrowed: the queue
    /// column walks them every frame).
    pub fn groups(&self) -> impl Iterator<Item = Group<'_>> {
        let mut start = 0;
        std::iter::from_fn(move || {
            let source = &self.upcoming.get(start)?.source;
            let len = self.upcoming[start..].iter().take_while(|i| i.source == *source).count();
            let group = Group { source, start, len };
            start += len;
            Some(group)
        })
    }
}

/// Seconds from now until each upcoming item starts, given how much of the
/// current track is left. Unknown durations count as zero.
pub fn start_offsets(current_remaining: f64, upcoming: &[Item]) -> Vec<f64> {
    let mut at = current_remaining.max(0.0);
    upcoming
        .iter()
        .map(|i| {
            let start = at;
            at += f64::from(i.track.duration_secs.unwrap_or(0));
            start
        })
        .collect()
}

/// Seconds from now until the queue runs out.
pub fn end_offset(current_remaining: f64, upcoming: &[Item]) -> f64 {
    current_remaining.max(0.0) + upcoming.iter().map(|i| f64::from(i.track.duration_secs.unwrap_or(0))).sum::<f64>()
}

/// Wall-clock label for `offset` seconds after `now`, e.g. `10:56 PM`.
pub fn clock_label(now: NaiveTime, offset: f64) -> String {
    let at = now + chrono::TimeDelta::seconds(offset.round() as i64);
    at.format("%-I:%M %p").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str, secs: u32) -> Track {
        Track {
            id: id.into(),
            album_id: "al".into(),
            title: id.into(),
            artist: None,
            artist_ref: None,
            track: None,
            disc: None,
            duration_secs: Some(secs),
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

    fn album(ids: &[&str]) -> Vec<Track> {
        ids.iter().map(|id| t(id, 100)).collect()
    }

    fn src(name: &str) -> Source {
        Source::Album { id: name.into(), name: name.into() }
    }

    fn ids(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.track.id.as_str()).collect()
    }

    fn cur(q: &Queue) -> Option<&str> {
        q.current.as_ref().map(|i| i.track.id.as_str())
    }

    #[test]
    fn play_now_starts_mid_album() {
        let mut q = Queue::default();
        q.play_now(album(&["a1", "a2", "a3", "a4"]), 1, src("A"));
        assert_eq!(cur(&q), Some("a2"));
        assert_eq!(ids(&q.upcoming), ["a3", "a4"]);
        assert!(q.history.is_empty());
        // Out-of-range start is ignored.
        q.play_now(album(&["x"]), 5, src("X"));
        assert_eq!(cur(&q), Some("a2"));
    }

    #[test]
    fn play_now_keeps_hand_added_items_and_replaces_the_rest() {
        let mut q = Queue::default();
        q.play_now(album(&["a1", "a2", "a3"]), 0, src("A"));
        q.play_next(album(&["n1"]));
        q.play_now(album(&["b1", "b2"]), 0, src("B"));
        assert_eq!(cur(&q), Some("b1"));
        assert_eq!(ids(&q.upcoming), ["b2", "n1"]);
        assert_eq!(ids(&q.history), ["a1"]);
        q.prefs.on_play = OnPlay::KeepBefore;
        q.play_now(album(&["c1", "c2"]), 0, src("C"));
        assert_eq!(ids(&q.upcoming), ["n1", "c2"]);
        q.prefs.on_play = OnPlay::Clear;
        q.play_now(album(&["d1", "d2"]), 0, src("D"));
        assert_eq!(ids(&q.upcoming), ["d2"]);
    }

    #[test]
    fn clear_can_leave_the_album() {
        let mut q = Queue::default();
        q.play_now(album(&["a1", "a2", "a3"]), 0, src("A"));
        q.play_next(album(&["n1"]));
        q.add(album(&["z1"]));
        q.prefs.clear = ClearWhat::Added;
        assert!(q.can_clear());
        q.clear();
        assert_eq!(ids(&q.upcoming), ["a2", "a3"]);
        assert!(!q.can_clear());
        q.prefs.clear = ClearWhat::Everything;
        assert!(q.can_clear());
        q.clear();
        assert!(q.upcoming.is_empty());
    }

    #[test]
    fn spotify_style_queue() {
        let mut q = Queue { prefs: Preset::Spotify.prefs().unwrap(), ..Default::default() };
        q.play_now(album(&["a1", "a2"]), 0, src("A"));
        q.add(album(&["z1"]));
        q.play_next(album(&["n1"]));
        q.play_next(album(&["n2"]));
        q.add(album(&["z2"]));
        assert_eq!(ids(&q.upcoming), ["n1", "n2", "z1", "z2", "a2"]);
    }

    #[test]
    fn gathering_what_was_added() {
        let mut q = Queue::default();
        q.play_now(album(&["a1", "a2", "a3"]), 0, src("A"));
        q.play_next(album(&["n1"]));
        q.add(album(&["z1", "z2"]));
        assert!(q.added_is_split());
        q.gather_added();
        assert_eq!(ids(&q.upcoming), ["n1", "z1", "z2", "a2", "a3"]);
        assert!(!q.added_is_split());
        // Nothing added at the front: they gather where the first one is.
        let mut q = Queue::default();
        q.play_now(album(&["a1", "a2", "a3"]), 0, src("A"));
        q.add(album(&["z1"]));
        q.upcoming.swap(1, 2);
        q.add(album(&["z2"]));
        q.gather_added();
        assert_eq!(ids(&q.upcoming), ["a2", "z1", "z2", "a3"]);
    }

    #[test]
    fn presets_round_trip() {
        for p in [Preset::Ond, Preset::AppleMusic, Preset::Spotify] {
            assert_eq!(Preset::of(p.prefs().unwrap()), p);
        }
        assert_eq!(Preset::of(QueuePrefs { on_play: OnPlay::KeepBefore, ..Default::default() }), Preset::Custom);
    }

    #[test]
    fn play_next_goes_to_the_top() {
        let mut q = Queue::default();
        q.play_now(album(&["a1", "a2"]), 0, src("A"));
        q.play_next(album(&["n1"]));
        q.play_next(album(&["n2", "n3"]));
        assert_eq!(ids(&q.upcoming), ["n2", "n3", "n1", "a2"]);
        q.add(album(&["z"]));
        assert_eq!(ids(&q.upcoming), ["n2", "n3", "n1", "a2", "z"]);
        assert_eq!(q.upcoming[4].source, Source::Added);
        // Ahead of what was added to the queue, too.
        q.play_next(album(&["n4"]));
        assert_eq!(ids(&q.upcoming)[0], "n4");
    }

    #[test]
    fn groups_are_consecutive_runs() {
        let mut q = Queue::default();
        q.play_now(album(&["a1", "a2", "a3"]), 0, src("A"));
        q.play_next(album(&["n1", "n2"]));
        q.add(album(&["z"]));
        let g: Vec<Group> = q.groups().collect();
        assert_eq!(g.len(), 3);
        assert_eq!((g[0].source.label(), g[0].start, g[0].len), ("You added", 0, 2));
        assert_eq!((g[1].source.label(), g[1].start, g[1].len), ("A", 2, 2));
        assert_eq!((g[2].source.label(), g[2].start, g[2].len), ("You added", 4, 1));
    }

    #[test]
    fn remove_reorder_clear() {
        let mut q = Queue::default();
        q.play_now(album(&["a1", "a2", "a3", "a4", "a5"]), 0, src("A"));
        let uid = |q: &Queue, id: &str| q.upcoming.iter().find(|i| i.track.id == id).unwrap().uid;
        assert!(q.remove(uid(&q, "a3")));
        assert!(!q.remove(9999));
        assert_eq!(ids(&q.upcoming), ["a2", "a4", "a5"]);
        assert!(q.move_to(uid(&q, "a5"), 0));
        assert_eq!(ids(&q.upcoming), ["a5", "a2", "a4"]);
        assert!(q.move_by(uid(&q, "a5"), 1));
        assert_eq!(ids(&q.upcoming), ["a2", "a5", "a4"]);
        // Clamped at both ends.
        assert!(!q.move_by(uid(&q, "a2"), -1));
        assert!(!q.move_by(uid(&q, "a4"), 1));
        assert!(q.move_to(uid(&q, "a2"), 99));
        assert_eq!(ids(&q.upcoming), ["a5", "a4", "a2"]);
        q.clear();
        assert!(q.upcoming.is_empty());
        assert_eq!(cur(&q), Some("a1"));
    }

    #[test]
    fn advance_and_stop_at_end() {
        let mut q = Queue::default();
        q.play_now(album(&["a1", "a2"]), 0, src("A"));
        assert_eq!(q.peek_next().map(|i| i.track.id.as_str()), Some("a2"));
        assert_eq!(q.advance().map(|i| i.track.id.clone()), Some("a2".into()));
        assert_eq!(q.peek_next().map(|i| i.track.id.as_str()), None);
        assert!(q.advance().is_none());
        assert_eq!(cur(&q), None);
        assert_eq!(ids(&q.history), ["a1", "a2"]);
    }

    #[test]
    fn repeat_queue_replays_the_current_pass() {
        let mut q = Queue::default();
        q.play_now(album(&["old"]), 0, src("O"));
        q.play_now(album(&["a1", "a2"]), 0, src("A"));
        q.end = EndAction::RepeatQueue;
        q.advance();
        assert_eq!(cur(&q), Some("a2"));
        // Nothing upcoming: the engine should line up the first track of this pass.
        assert_eq!(q.peek_next().map(|i| i.track.id.as_str()), Some("a1"));
        assert_eq!(q.advance().map(|i| i.track.id.clone()), Some("a1".into()));
        assert_eq!(ids(&q.upcoming), ["a2"]);
        // And again: still just this pass, never the track from before "play now".
        q.advance();
        assert_eq!(q.advance().map(|i| i.track.id.clone()), Some("a1".into()));
        assert!(!ids(&q.upcoming).contains(&"old"));
    }

    #[test]
    fn back_jump_and_replay() {
        let mut q = Queue::default();
        q.play_now(album(&["a1", "a2", "a3", "a4"]), 0, src("A"));
        q.advance();
        assert_eq!(q.back().map(|i| i.track.id.clone()), Some("a1".into()));
        assert_eq!(ids(&q.upcoming), ["a2", "a3", "a4"]);
        let a3 = q.upcoming[1].uid;
        assert!(q.jump_to(a3));
        assert_eq!(cur(&q), Some("a3"));
        assert_eq!(ids(&q.history), ["a1", "a2"]);
        assert_eq!(ids(&q.upcoming), ["a4"]);
        let a1 = q.history[0].uid;
        assert!(q.replay(a1));
        assert_eq!(cur(&q), Some("a1"));
        assert_eq!(ids(&q.upcoming), ["a4"]);
        assert_eq!(ids(&q.history), ["a1", "a2", "a3"]);
        // Nothing to go back to on a fresh queue.
        assert!(Queue::default().back().is_none());
    }

    #[test]
    fn same_track_twice_has_distinct_uids() {
        let mut q = Queue::default();
        q.add(album(&["x"]));
        q.add(album(&["x"]));
        assert_ne!(q.upcoming[0].uid, q.upcoming[1].uid);
        assert!(q.remove(q.upcoming[1].uid));
        assert_eq!(q.upcoming.len(), 1);
    }

    #[test]
    fn timing() {
        let mut q = Queue::default();
        q.play_now(vec![t("a", 300), t("b", 200), t("c", 60), t("d", 0)], 0, src("A"));
        assert_eq!(start_offsets(120.0, &q.upcoming), [120.0, 320.0, 380.0]);
        assert_eq!(end_offset(120.0, &q.upcoming), 380.0);
        assert_eq!(end_offset(-5.0, &[]), 0.0);
        let now = NaiveTime::from_hms_opt(22, 7, 40).unwrap();
        assert_eq!(clock_label(now, 0.0), "10:07 PM");
        assert_eq!(clock_label(now, 380.0), "10:14 PM");
        // Wraps past midnight.
        assert_eq!(clock_label(NaiveTime::from_hms_opt(23, 59, 0).unwrap(), 120.0), "12:01 AM");
    }
}
