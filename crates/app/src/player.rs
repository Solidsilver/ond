//! Playback control: the queue and what the engine is told about it. No UI
//! here. The app supplies the engine ([`Transport`]) and a way to turn a track
//! into something playable ([`Sources`]); what the app should do in return
//! (show a notice, analyse a track, check the connection) comes back as
//! [`Note`]s.

use crate::queue::{Queue, Source};
use audio::{Command, EngineHandle, PlayState, TrackSource};
use library::Track;
use std::time::Duration;

/// What a queue action does with a set of tracks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QueueAction {
    /// Play now, starting at this track.
    Play { start: usize },
    Shuffle,
    PlayNext,
    Add,
}

/// The engine, as the player sees it.
pub trait Transport {
    fn send(&self, cmd: Command);
    fn state(&self) -> PlayState;
    fn position_secs(&self) -> f64;
    fn duration_secs(&self) -> Option<f64>;
}

impl Transport for EngineHandle {
    fn send(&self, cmd: Command) {
        EngineHandle::send(self, cmd);
    }
    fn state(&self) -> PlayState {
        self.status.state()
    }
    fn position_secs(&self) -> f64 {
        self.status.position_secs()
    }
    fn duration_secs(&self) -> Option<f64> {
        self.status.duration_secs()
    }
}

/// Where tracks play from, and who hears about them in advance.
pub trait Sources {
    /// What the engine needs to play `track` (`None`: it can't play now, e.g.
    /// offline and not downloaded). `token` is the queue item's id, echoed back
    /// by the engine; `prev` / `next` are its queue neighbours.
    fn source(&self, token: u64, prev: Option<&Track>, track: &Track, next: Option<&Track>) -> Option<TrackSource>;
    /// `track` is about to play (Now Playing learns its details early).
    fn announce(&self, track: &Track);
}

/// Something for the app to act on.
#[derive(Debug, Clone, PartialEq)]
pub enum Note {
    Notice(String),
    Error(String),
    /// Playback failed: the server may have gone away.
    Probe,
    /// A queue item started playing (`restart`: the engine restarted it, e.g.
    /// after a device change; it isn't a new play).
    Started { track: Box<Track>, restart: bool },
}

/// The engine, the sources and where notes go, for one call.
pub struct Ctx<'a> {
    pub engine: &'a dyn Transport,
    pub sources: &'a dyn Sources,
    pub notes: &'a mut Vec<Note>,
}

#[derive(Default)]
pub struct Player {
    pub queue: Queue,
    /// The queue item last sent as `SetNext`, so the engine isn't told twice.
    sent_next: Option<u64>,
    /// A seek that's been sent but not confirmed yet; shown instead of the engine position.
    pub pending_seek: Option<f64>,
    /// The queue item the engine last reported starting.
    started: Option<u64>,
    /// The current item was started because the one before it failed to
    /// open. If it fails too, playback stops instead of skipping again (the
    /// server may be gone, and every track would fail).
    skipped_failure: bool,
}

impl Player {
    /// Where playback is, counting a seek not yet confirmed.
    pub fn position_secs(&self, engine: &dyn Transport) -> f64 {
        self.pending_seek.unwrap_or_else(|| engine.position_secs())
    }

    pub fn duration_secs(&self, engine: &dyn Transport) -> Option<f64> {
        engine.duration_secs().or_else(|| self.queue.current.as_ref().and_then(|c| c.track.duration_secs).map(f64::from))
    }

    /// The queue item last named to the engine as next (tests).
    #[cfg(test)]
    pub fn sent_next(&self) -> Option<u64> {
        self.sent_next
    }

    /// Forgets what the engine was told (it stopped, or the server came back
    /// and what was lined up offline can be lined up again).
    pub fn forget_next(&mut self) {
        self.sent_next = None;
    }

    /// Stops and empties the queue (signing out).
    pub fn reset(&mut self, cx: &mut Ctx<'_>) {
        cx.engine.send(Command::Stop);
        self.queue.clear();
        self.queue.current = None;
        self.sent_next = None;
        self.pending_seek = None;
        self.started = None;
        self.skipped_failure = false;
    }

    /// The queue entry the engine just started: the current one, or the next (gapless).
    fn playing_track(&self, token: u64) -> Option<&Track> {
        [self.queue.current.as_ref(), self.queue.peek_next()].into_iter().flatten().find(|i| i.uid == token).map(|i| &i.track)
    }

    pub fn handle_engine(&mut self, ev: &audio::Event, cx: &mut Ctx<'_>) {
        match ev {
            audio::Event::TrackStarted { token, gapless, restart, .. } => {
                // Whatever was being sought, the engine's position is what counts now.
                self.pending_seek = None;
                self.started = Some(*token);
                self.skipped_failure = false;
                if let Some(t) = self.playing_track(*token) {
                    cx.notes.push(Note::Started { track: Box::new(t.clone()), restart: *restart });
                }
                // Matched by queue item (the token), so the same song twice in a row
                // still moves the queue along. With "repeat queue" over a single
                // item, the next item *is* the current one: a gapless start of it
                // is the next pass.
                let is_next = self.queue.peek_next().is_some_and(|n| n.uid == *token);
                let is_current = self.queue.current.as_ref().is_some_and(|c| c.uid == *token);
                if is_next && (*gapless || !is_current) {
                    // The engine moved on gaplessly; line up the track after this one.
                    self.queue.advance();
                    self.sent_next = None;
                    self.sync_next(cx);
                }
            }
            audio::Event::Seeked { .. } => self.pending_seek = None,
            // The engine ran out of queued tracks. If the queue has more (the next one
            // was named too late to play gaplessly), carry on from there.
            audio::Event::Ended { .. } => {
                if self.queue.advance().is_some() {
                    self.start_current(cx);
                } else {
                    self.sent_next = None;
                }
            }
            audio::Event::Error { track_id, message } => {
                self.pending_seek = None;
                // The current track couldn't be opened (e.g. deleted on the
                // server): carry on with the next rather than leave the queue
                // parked on it. A track that failed partway, an error not about
                // a track (the device) or a second failure in a row stops as before.
                let current = self.queue.current.as_ref().filter(|c| track_id.as_ref() == Some(&c.track.id));
                let skip = current
                    .filter(|c| self.started != Some(c.uid) && !self.skipped_failure && self.queue.peek_next().is_some())
                    .map(|c| c.track.title.clone());
                let text = skip.as_ref().map_or_else(|| message.clone(), |title| format!("Skipped “{title}”: {message}"));
                cx.notes.push(Note::Error(text));
                cx.notes.push(Note::Probe);
                if skip.is_some() {
                    self.queue.advance();
                    self.start_current(cx);
                    self.skipped_failure = true;
                }
            }
            audio::Event::SignalPath(_)
            | audio::Event::Devices { .. }
            | audio::Event::ModeChanged { .. }
            | audio::Event::DeviceLost { .. }
            | audio::Event::DeviceReturned { .. } => {}
        }
    }

    /// Starts the queue's current item from the top (or stops when there is
    /// none). Tracks that can't play (offline, not downloaded) are skipped.
    pub fn start_current(&mut self, cx: &mut Ctx<'_>) {
        self.pending_seek = None;
        self.sent_next = None;
        self.started = None;
        self.skipped_failure = false;
        let mut skipped = 0;
        // With "repeat queue" the queue never runs out, so give up after one
        // full pass in which nothing could play.
        let limit = self.queue.pass_len().max(1);
        loop {
            let Some(item) = &self.queue.current else {
                cx.engine.send(Command::Stop);
                break;
            };
            if skipped >= limit {
                cx.engine.send(Command::Stop);
                cx.notes.push(Note::Notice("Nothing in the queue is downloaded".to_owned()));
                return;
            }
            let prev = self.queue.history.last().map(|i| &i.track);
            let next = self.queue.peek_next().map(|i| &i.track);
            if let Some(src) = cx.sources.source(item.uid, prev, &item.track, next) {
                cx.sources.announce(&item.track);
                cx.engine.send(Command::Play(src));
                self.sync_next(cx);
                break;
            }
            skipped += 1;
            if self.queue.advance().is_none() {
                cx.engine.send(Command::Stop);
                break;
            }
        }
        if skipped > 0 {
            let text = if skipped == 1 { "Skipped 1 song that isn't downloaded".to_owned() } else { format!("Skipped {skipped} songs that aren't downloaded") };
            cx.notes.push(Note::Notice(text));
        }
    }

    /// Tells the engine what follows the current track (prefetch + gapless), if
    /// that changed. A next track that can't play isn't named; when the current
    /// one ends, `start_current` skips past it.
    pub fn sync_next(&mut self, cx: &mut Ctx<'_>) {
        if self.queue.current.is_none() {
            return;
        }
        let next = self.queue.peek_next().map(|i| (i.uid, &i.track));
        let current = self.queue.current.as_ref().map(|i| &i.track);
        let after = self.queue.upcoming.get(1).map(|i| &i.track);
        let source = next.and_then(|(uid, t)| cx.sources.source(uid, current, t, after));
        // By queue item, not track: the same song queued twice is two items.
        let sent = source.as_ref().map(|s| s.token);
        if sent != self.sent_next {
            if let Some((_, t)) = next.filter(|_| source.is_some()) {
                cx.sources.announce(t);
            }
            cx.engine.send(Command::SetNext(source));
            self.sent_next = sent;
        }
    }

    pub fn play_pause(&mut self, cx: &mut Ctx<'_>) {
        match cx.engine.state() {
            PlayState::Playing | PlayState::Buffering | PlayState::Loading => cx.engine.send(Command::Pause),
            PlayState::Paused => cx.engine.send(Command::Resume),
            PlayState::Stopped => {
                if self.queue.current.is_some() || self.queue.advance().is_some() {
                    self.start_current(cx);
                }
            }
        }
    }

    pub fn next(&mut self, cx: &mut Ctx<'_>) {
        if self.queue.advance().is_some() {
            self.start_current(cx);
        } else {
            cx.engine.send(Command::Stop);
            self.sent_next = None;
        }
    }

    /// Restarts the track when it's past the first few seconds, else goes back one.
    pub fn previous(&mut self, cx: &mut Ctx<'_>) {
        if self.position_secs(cx.engine) > 3.0 || self.queue.history.is_empty() {
            self.seek(0.0, cx);
        } else if self.queue.back().is_some() {
            self.start_current(cx);
        }
    }

    pub fn seek(&mut self, to: f64, cx: &mut Ctx<'_>) {
        if self.queue.current.is_none() {
            return;
        }
        // A stopped engine has nothing to seek in and never confirms, so don't
        // show a position it won't reach. (The command still goes: a `Play`
        // sent just before may not have reached the engine yet.)
        if cx.engine.state() != PlayState::Stopped {
            self.pending_seek = Some(to);
        }
        cx.engine.send(Command::Seek(Duration::from_secs_f64(to)));
    }

    /// A media key, Control Center or headphone command.
    pub fn remote(&mut self, r: crate::now_playing::Remote, cx: &mut Ctx<'_>) {
        use crate::now_playing::Remote;
        let state = cx.engine.state();
        match r {
            Remote::Play if state == PlayState::Paused => cx.engine.send(Command::Resume),
            Remote::Play if state == PlayState::Stopped => self.play_pause(cx),
            Remote::Pause if !matches!(state, PlayState::Paused | PlayState::Stopped) => cx.engine.send(Command::Pause),
            Remote::Play | Remote::Pause => {}
            Remote::Toggle => self.play_pause(cx),
            Remote::Next => self.next(cx),
            Remote::Previous => self.previous(cx),
            Remote::Seek(to) => self.seek(to.max(0.0), cx),
            Remote::SkipBy(by) => {
                let end = self.duration_secs(cx.engine).unwrap_or(f64::MAX);
                let to = (self.position_secs(cx.engine) + by).clamp(0.0, end);
                self.seek(to, cx);
            }
            Remote::Raise => {}
        }
    }

    /// Applies a queue action once the tracks are known.
    pub fn queue_tracks(&mut self, tracks: Vec<Track>, source: Source, action: QueueAction, cx: &mut Ctx<'_>) {
        if tracks.is_empty() {
            return;
        }
        match action {
            QueueAction::Play { start } => {
                self.queue.play_now(tracks, start, source);
                self.start_current(cx);
            }
            QueueAction::Shuffle => {
                let mut tracks = tracks;
                shuffle(&mut tracks);
                self.queue.play_now(tracks, 0, source);
                self.start_current(cx);
            }
            QueueAction::PlayNext => {
                self.queue.play_next(tracks);
                self.sync_next(cx);
            }
            QueueAction::Add => {
                self.queue.add(tracks);
                self.sync_next(cx);
            }
        }
    }
}

/// A change to the queue itself, from the queue column or its keys.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QueueEdit {
    JumpTo(u64),
    Replay(u64),
    Remove(u64),
    Move(u64, usize),
    /// One step earlier (-1) or later (+1).
    MoveBy(u64, isize),
    Clear,
    SetEnd(crate::queue::EndAction),
    /// Brings what the user added, split into groups, together.
    GatherAdded,
}

impl Player {
    /// Applies `edit` and tells the engine what changed. `selected` is the
    /// queue column's selection: when the selected item is removed it moves to
    /// a neighbour, so ⌫ can be pressed again.
    pub fn edit(&mut self, edit: QueueEdit, selected: &mut Option<u64>, cx: &mut Ctx<'_>) {
        let q = &mut self.queue;
        match edit {
            QueueEdit::JumpTo(uid) => {
                if q.jump_to(uid) {
                    self.start_current(cx);
                }
                return;
            }
            QueueEdit::Replay(uid) => {
                if q.replay(uid) {
                    self.start_current(cx);
                }
                return;
            }
            QueueEdit::Remove(uid) if *selected == Some(uid) => {
                let at = q.position(uid).unwrap_or(0);
                q.remove(uid);
                *selected = q.upcoming.get(at).or(q.upcoming.last()).map(|i| i.uid);
            }
            QueueEdit::Remove(uid) => {
                q.remove(uid);
            }
            QueueEdit::Move(uid, to) => {
                q.move_to(uid, to);
            }
            QueueEdit::MoveBy(uid, delta) => {
                q.move_by(uid, delta);
            }
            QueueEdit::Clear => q.clear(),
            QueueEdit::SetEnd(end) => q.end = end,
            QueueEdit::GatherAdded => q.gather_added(),
        }
        self.sync_next(cx);
    }
}

/// Fisher–Yates with a clock-seeded xorshift (no need for a crypto RNG here).
pub fn shuffle<T>(v: &mut [T]) {
    let mut s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0x9E37_79B9, |d| d.as_nanos() as u64) | 1;
    for i in (1..v.len()).rev() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        v.swap(i, (s % (i as u64 + 1)) as usize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashSet;

    /// Records what the engine was told; its state is whatever the test sets.
    #[derive(Default)]
    struct FakeEngine {
        sent: RefCell<Vec<Command>>,
        state: std::cell::Cell<Option<PlayState>>,
        position: std::cell::Cell<f64>,
    }

    impl Transport for FakeEngine {
        fn send(&self, cmd: Command) {
            self.sent.borrow_mut().push(cmd);
        }
        fn state(&self) -> PlayState {
            self.state.get().unwrap_or(PlayState::Stopped)
        }
        fn position_secs(&self) -> f64 {
            self.position.get()
        }
        fn duration_secs(&self) -> Option<f64> {
            None
        }
    }

    impl FakeEngine {
        /// The track ids of every `Play` sent, in order.
        fn played(&self) -> Vec<String> {
            self.sent.borrow().iter().filter_map(|c| if let Command::Play(s) = c { Some(s.id.clone()) } else { None }).collect()
        }
        fn last_next(&self) -> Option<Option<String>> {
            self.sent.borrow().iter().rev().find_map(|c| if let Command::SetNext(s) = c { Some(s.as_ref().map(|s| s.id.clone())) } else { None })
        }
        fn stopped(&self) -> bool {
            matches!(self.sent.borrow().last(), Some(Command::Stop))
        }
    }

    /// Every track plays except those listed (as if offline, not downloaded).
    #[derive(Default)]
    struct FakeSources {
        unplayable: HashSet<String>,
    }

    impl Sources for FakeSources {
        fn source(&self, token: u64, _: Option<&Track>, track: &Track, _: Option<&Track>) -> Option<TrackSource> {
            (!self.unplayable.contains(&track.id)).then(|| TrackSource {
                id: track.id.clone(),
                url: format!("test://{}", track.id),
                suffix: None,
                replay_gain: Default::default(),
                album_context: false,
                transcode: None,
                token,
            })
        }
        fn announce(&self, _: &Track) {}
    }

    fn track(id: &str) -> Track {
        Track {
            id: id.into(),
            album_id: "al".into(),
            title: id.into(),
            artist: None,
            artist_ref: None,
            track: None,
            disc: None,
            duration_secs: Some(200),
            suffix: Some("flac".into()),
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

    fn tracks(ids: &[&str]) -> Vec<Track> {
        ids.iter().map(|i| track(i)).collect()
    }

    fn source() -> Source {
        Source::Album { id: "al".into(), name: "Album".into() }
    }

    struct Rig {
        player: Player,
        engine: FakeEngine,
        sources: FakeSources,
        notes: Vec<Note>,
    }

    impl Rig {
        fn new() -> Self {
            Self { player: Player::default(), engine: FakeEngine::default(), sources: FakeSources::default(), notes: Vec::new() }
        }
        fn run<R>(&mut self, f: impl FnOnce(&mut Player, &mut Ctx<'_>) -> R) -> R {
            let mut cx = Ctx { engine: &self.engine, sources: &self.sources, notes: &mut self.notes };
            f(&mut self.player, &mut cx)
        }
        fn started(&mut self, token: u64, gapless: bool) {
            let format = audio::StreamFormat { sample_rate: 44_100, channels: 2, bits_per_sample: Some(16) };
            let ev = audio::Event::TrackStarted { track_id: String::new(), token, format, gapless, restart: false };
            self.run(|p, cx| p.handle_engine(&ev, cx));
        }
        fn uid_of(&self, id: &str) -> u64 {
            let q = &self.player.queue;
            q.current.iter().chain(q.upcoming.iter()).find(|i| i.track.id == id).map(|i| i.uid).unwrap()
        }
    }

    #[test]
    fn play_now_starts_the_first_and_lines_up_the_second() {
        let mut r = Rig::new();
        r.run(|p, cx| p.queue_tracks(tracks(&["a", "b", "c"]), source(), QueueAction::Play { start: 0 }, cx));
        assert_eq!(r.engine.played(), ["a"]);
        assert_eq!(r.engine.last_next(), Some(Some("b".into())));
    }

    #[test]
    fn a_gapless_start_advances_the_queue_and_lines_up_the_next() {
        let mut r = Rig::new();
        r.run(|p, cx| p.queue_tracks(tracks(&["a", "b", "c"]), source(), QueueAction::Play { start: 0 }, cx));
        let b = r.uid_of("b");
        r.started(b, true);
        assert_eq!(r.player.queue.current.as_ref().unwrap().track.id, "b");
        assert_eq!(r.engine.last_next(), Some(Some("c".into())));
        assert!(matches!(r.notes.last(), Some(Note::Started { track, restart: false }) if track.id == "b"));
    }

    #[test]
    fn the_same_song_twice_still_moves_along() {
        let mut r = Rig::new();
        r.run(|p, cx| p.queue_tracks(tracks(&["a", "a", "b"]), source(), QueueAction::Play { start: 0 }, cx));
        let second = r.player.queue.upcoming[0].uid;
        r.started(second, true);
        assert_eq!(r.player.queue.current.as_ref().unwrap().uid, second);
        assert_eq!(r.engine.last_next(), Some(Some("b".into())));
    }

    #[test]
    fn songs_that_cannot_play_are_skipped_with_a_notice() {
        let mut r = Rig::new();
        r.sources.unplayable = ["a".to_owned(), "b".to_owned()].into();
        r.run(|p, cx| p.queue_tracks(tracks(&["a", "b", "c"]), source(), QueueAction::Play { start: 0 }, cx));
        assert_eq!(r.engine.played(), ["c"]);
        assert_eq!(r.notes, [Note::Notice("Skipped 2 songs that aren't downloaded".into())]);
    }

    #[test]
    fn a_next_that_cannot_play_is_not_named() {
        let mut r = Rig::new();
        r.sources.unplayable = ["b".to_owned()].into();
        r.run(|p, cx| p.queue_tracks(tracks(&["a", "b"]), source(), QueueAction::Play { start: 0 }, cx));
        assert!(matches!(r.engine.last_next(), None | Some(None)), "the engine never hears of b");
        assert_eq!(r.player.sent_next(), None);
    }

    #[test]
    fn repeat_with_nothing_playable_stops_after_one_pass() {
        let mut r = Rig::new();
        r.player.queue.end = crate::queue::EndAction::RepeatQueue;
        r.sources.unplayable = ["a".to_owned(), "b".to_owned()].into();
        r.run(|p, cx| p.queue_tracks(tracks(&["a", "b"]), source(), QueueAction::Play { start: 0 }, cx));
        assert!(r.engine.played().is_empty());
        assert!(r.engine.stopped());
        assert!(r.notes.contains(&Note::Notice("Nothing in the queue is downloaded".into())));
    }

    #[test]
    fn ended_carries_on_with_the_queue_or_stops() {
        let mut r = Rig::new();
        r.run(|p, cx| p.queue_tracks(tracks(&["a", "b"]), source(), QueueAction::Play { start: 0 }, cx));
        let ended = audio::Event::Ended { track_id: "a".into(), token: 0 };
        r.run(|p, cx| p.handle_engine(&ended, cx));
        assert_eq!(r.engine.played(), ["a", "b"]);
        r.run(|p, cx| p.handle_engine(&ended, cx));
        assert_eq!(r.engine.played(), ["a", "b"], "nothing after the last");
    }

    #[test]
    fn previous_restarts_after_three_seconds_else_goes_back() {
        let mut r = Rig::new();
        r.run(|p, cx| p.queue_tracks(tracks(&["a", "b"]), source(), QueueAction::Play { start: 0 }, cx));
        r.run(|p, cx| p.next(cx));
        r.engine.state.set(Some(PlayState::Playing));
        r.engine.position.set(10.0);
        r.run(|p, cx| p.previous(cx));
        assert!(matches!(r.engine.sent.borrow().last(), Some(Command::Seek(d)) if d.is_zero()));
        assert_eq!(r.player.pending_seek, Some(0.0));
        r.engine.position.set(1.0);
        r.player.pending_seek = None;
        r.run(|p, cx| p.previous(cx));
        assert_eq!(r.engine.played(), ["a", "b", "a"]);
    }

    #[test]
    fn play_pause_follows_the_engine_state() {
        let mut r = Rig::new();
        r.run(|p, cx| p.queue_tracks(tracks(&["a"]), source(), QueueAction::Add, cx));
        r.run(|p, cx| p.play_pause(cx));
        assert_eq!(r.engine.played(), ["a"], "stopped with a queue: starts it");
        r.engine.state.set(Some(PlayState::Playing));
        r.run(|p, cx| p.play_pause(cx));
        assert!(matches!(r.engine.sent.borrow().last(), Some(Command::Pause)));
        r.engine.state.set(Some(PlayState::Paused));
        r.run(|p, cx| p.play_pause(cx));
        assert!(matches!(r.engine.sent.borrow().last(), Some(Command::Resume)));
    }

    #[test]
    fn engine_errors_become_an_error_and_a_probe() {
        let mut r = Rig::new();
        r.player.pending_seek = Some(3.0);
        let ev = audio::Event::Error { track_id: None, message: "boom".into() };
        r.run(|p, cx| p.handle_engine(&ev, cx));
        assert_eq!(r.notes, [Note::Error("boom".into()), Note::Probe]);
        assert_eq!(r.player.pending_seek, None);
    }

    #[test]
    fn a_seek_while_stopped_holds_no_position() {
        let mut r = Rig::new();
        r.run(|p, cx| p.queue_tracks(tracks(&["a", "b"]), source(), QueueAction::Add, cx));
        r.player.queue.advance();
        assert!(r.player.queue.current.is_some(), "a current item, but nothing playing (offline, or after an error)");
        r.run(|p, cx| p.seek(30.0, cx));
        assert_eq!(r.player.pending_seek, None, "the engine won't confirm it, so it would show for good");
        assert_eq!(r.player.position_secs(&r.engine), 0.0);
        r.run(|p, cx| p.remote(crate::now_playing::Remote::SkipBy(15.0), cx));
        assert_eq!(r.player.pending_seek, None);
    }

    #[test]
    fn a_seek_shows_until_the_engine_confirms_it_or_starts_a_track() {
        let mut r = Rig::new();
        r.run(|p, cx| p.queue_tracks(tracks(&["a", "b"]), source(), QueueAction::Play { start: 0 }, cx));
        r.engine.state.set(Some(PlayState::Playing));
        r.run(|p, cx| p.seek(42.0, cx));
        assert_eq!(r.player.pending_seek, Some(42.0));
        assert_eq!(r.player.position_secs(&r.engine), 42.0);
        let seeked = audio::Event::Seeked { track_id: "a".into(), position_secs: 42.0 };
        r.run(|p, cx| p.handle_engine(&seeked, cx));
        assert_eq!(r.player.pending_seek, None);
        // The engine started the track over instead (a seek it couldn't serve in place).
        r.run(|p, cx| p.seek(50.0, cx));
        let a = r.uid_of("a");
        let ev = audio::Event::TrackStarted {
            track_id: "a".into(),
            token: a,
            format: audio::StreamFormat { sample_rate: 44_100, channels: 2, bits_per_sample: Some(16) },
            gapless: false,
            restart: true,
        };
        r.run(|p, cx| p.handle_engine(&ev, cx));
        assert_eq!(r.player.pending_seek, None);
    }

    #[test]
    fn queue_edits_keep_the_engine_s_next_up_to_date() {
        let mut r = Rig::new();
        r.run(|p, cx| p.queue_tracks(tracks(&["a", "b", "c", "d"]), source(), QueueAction::Play { start: 0 }, cx));
        let (b, c, d) = (r.uid_of("b"), r.uid_of("c"), r.uid_of("d"));
        let mut sel = None;
        r.run(|p, cx| p.edit(QueueEdit::Remove(b), &mut sel, cx));
        assert_eq!(r.engine.last_next(), Some(Some("c".into())));
        assert_eq!(r.player.sent_next(), Some(c));
        r.run(|p, cx| p.edit(QueueEdit::Move(d, 0), &mut sel, cx));
        assert_eq!(r.engine.last_next(), Some(Some("d".into())));
        r.run(|p, cx| p.edit(QueueEdit::MoveBy(d, 1), &mut sel, cx));
        assert_eq!(r.engine.last_next(), Some(Some("c".into())));
        assert_eq!(r.player.sent_next(), Some(c));
        // Repeat with one item left: the next after the last is the first again.
        r.run(|p, cx| p.edit(QueueEdit::Clear, &mut sel, cx));
        assert_eq!(r.engine.last_next(), Some(None));
        assert_eq!(r.player.sent_next(), None);
        r.run(|p, cx| p.edit(QueueEdit::SetEnd(crate::queue::EndAction::RepeatQueue), &mut sel, cx));
        assert_eq!(r.engine.last_next(), Some(Some("a".into())));
        assert!(r.player.sent_next().is_some());
    }

    #[test]
    fn jump_and_replay_start_over_and_line_up_afresh() {
        let mut r = Rig::new();
        r.run(|p, cx| p.queue_tracks(tracks(&["a", "b", "c", "d"]), source(), QueueAction::Play { start: 0 }, cx));
        let (a, c, d) = (r.uid_of("a"), r.uid_of("c"), r.uid_of("d"));
        let nexts = |r: &Rig| r.engine.sent.borrow().iter().filter(|c| matches!(c, Command::SetNext(Some(s)) if s.id == "d")).count();
        let mut sel = None;
        r.run(|p, cx| p.edit(QueueEdit::JumpTo(c), &mut sel, cx));
        assert_eq!(r.engine.played(), ["a", "c"]);
        assert_eq!(r.player.sent_next(), Some(d));
        assert_eq!(nexts(&r), 1);
        // A new Play: the engine is told d again, though it was the last next.
        r.run(|p, cx| p.edit(QueueEdit::Replay(a), &mut sel, cx));
        assert_eq!(r.engine.played(), ["a", "c", "a"]);
        assert_eq!(r.player.sent_next(), Some(d));
        assert_eq!(nexts(&r), 2);
    }

    #[test]
    fn remote_commands_map_onto_the_player() {
        use crate::now_playing::Remote;
        let mut r = Rig::new();
        r.run(|p, cx| p.queue_tracks(tracks(&["a", "b"]), source(), QueueAction::Add, cx));
        r.run(|p, cx| p.remote(Remote::Pause, cx));
        assert!(r.engine.sent.borrow().iter().all(|c| !matches!(c, Command::Pause)), "stopped: nothing to pause");
        r.run(|p, cx| p.remote(Remote::Play, cx));
        assert_eq!(r.engine.played(), ["a"], "stopped: Play starts the queue");
        r.engine.state.set(Some(PlayState::Playing));
        r.run(|p, cx| p.remote(Remote::Play, cx));
        assert_eq!(r.engine.played(), ["a"], "already playing");
        r.run(|p, cx| p.remote(Remote::Pause, cx));
        assert!(matches!(r.engine.sent.borrow().last(), Some(Command::Pause)));
        r.engine.state.set(Some(PlayState::Paused));
        r.run(|p, cx| p.remote(Remote::Play, cx));
        assert!(matches!(r.engine.sent.borrow().last(), Some(Command::Resume)));
        r.run(|p, cx| p.remote(Remote::Toggle, cx));
        assert!(matches!(r.engine.sent.borrow().last(), Some(Command::Resume)));
        r.engine.state.set(Some(PlayState::Playing));
        r.engine.position.set(100.0);
        r.run(|p, cx| p.remote(Remote::SkipBy(-150.0), cx));
        assert!(matches!(r.engine.sent.borrow().last(), Some(Command::Seek(d)) if d.is_zero()), "clamped at the start");
        r.player.pending_seek = None;
        r.run(|p, cx| p.remote(Remote::SkipBy(500.0), cx));
        assert_eq!(r.player.pending_seek, Some(200.0), "clamped at the track's length");
        r.run(|p, cx| p.remote(Remote::Seek(-5.0), cx));
        assert_eq!(r.player.pending_seek, Some(0.0));
        r.run(|p, cx| p.remote(Remote::Next, cx));
        assert_eq!(r.engine.played(), ["a", "b"]);
        r.engine.position.set(1.0);
        r.player.pending_seek = None;
        r.run(|p, cx| p.remote(Remote::Previous, cx));
        assert_eq!(r.engine.played(), ["a", "b", "a"]);
    }

    fn error(id: Option<&str>) -> audio::Event {
        audio::Event::Error { track_id: id.map(Into::into), message: "not found".into() }
    }

    #[test]
    fn a_track_that_cannot_be_opened_is_skipped() {
        let mut r = Rig::new();
        r.run(|p, cx| p.queue_tracks(tracks(&["a", "b", "c"]), source(), QueueAction::Play { start: 0 }, cx));
        r.run(|p, cx| p.handle_engine(&error(Some("a")), cx));
        assert_eq!(r.engine.played(), ["a", "b"]);
        assert_eq!(r.player.queue.current.as_ref().unwrap().track.id, "b");
        assert_eq!(r.notes, [Note::Error("Skipped “a”: not found".into()), Note::Probe]);
        // b starts and ends; then c, the last, fails to open: nothing to skip to, so it stays current.
        let b = r.uid_of("b");
        r.started(b, false);
        r.run(|p, cx| p.handle_engine(&ended_of("b"), cx));
        r.run(|p, cx| p.handle_engine(&error(Some("c")), cx));
        assert_eq!(r.engine.played(), ["a", "b", "c"]);
        assert_eq!(r.player.queue.current.as_ref().unwrap().track.id, "c", "the last one stays current");
    }

    fn ended_of(id: &str) -> audio::Event {
        audio::Event::Ended { track_id: id.into(), token: 0 }
    }

    #[test]
    fn a_second_failure_in_a_row_stops() {
        let mut r = Rig::new();
        r.run(|p, cx| p.queue_tracks(tracks(&["a", "b", "c"]), source(), QueueAction::Play { start: 0 }, cx));
        r.run(|p, cx| p.handle_engine(&error(Some("a")), cx));
        r.run(|p, cx| p.handle_engine(&error(Some("b")), cx));
        assert_eq!(r.engine.played(), ["a", "b"], "the server may be gone: no run through the whole queue");
        assert_eq!(r.player.queue.current.as_ref().unwrap().track.id, "b");
        assert!(r.notes.ends_with(&[Note::Error("not found".into()), Note::Probe]));
    }

    #[test]
    fn errors_that_are_not_a_track_failing_to_open_stop_as_before() {
        // Not about a track (the device).
        let mut r = Rig::new();
        r.run(|p, cx| p.queue_tracks(tracks(&["a", "b"]), source(), QueueAction::Play { start: 0 }, cx));
        r.run(|p, cx| p.handle_engine(&error(None), cx));
        assert_eq!(r.engine.played(), ["a"]);
        // Partway through a track that had started (e.g. the connection dropped).
        let a = r.uid_of("a");
        r.started(a, false);
        r.run(|p, cx| p.handle_engine(&error(Some("a")), cx));
        assert_eq!(r.engine.played(), ["a"]);
        // About a later track: the current one plays out.
        r.run(|p, cx| p.handle_engine(&error(Some("b")), cx));
        assert_eq!(r.engine.played(), ["a"]);
        assert_eq!(r.player.queue.current.as_ref().unwrap().track.id, "a");
    }
}
