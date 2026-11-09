//! Playback policy, independent of egui and the network. The transport reads
//! source time; sources build account-qualified files; notes ask the app to
//! persist and report listening. Fakes exercise the same control path.

use crate::timeline::{Timeline, normalize_speed};
use audio::{Command, EngineHandle, Event, PlayState, TrackSource};
use sources::{Item, ItemKey, PlaySession, Position, ProgressUpdate, SessionTrack};
use std::time::{Duration, Instant, SystemTime};

pub trait Transport {
    fn send(&self, cmd: Command);
    fn state(&self) -> PlayState;
    fn position(&self) -> f64;
}
impl Transport for EngineHandle {
    fn send(&self, cmd: Command) {
        self.send(cmd);
    }
    fn state(&self) -> PlayState {
        self.status.state()
    }
    fn position(&self) -> f64 {
        self.status.source_position_secs()
    }
}

pub trait Sources {
    fn track(&self, key: &ItemKey, item: &Item, track: &SessionTrack, token: u64) -> TrackSource;
}

pub struct SessionSources;
impl Sources for SessionSources {
    fn track(&self, key: &ItemKey, item: &Item, track: &SessionTrack, token: u64) -> TrackSource {
        let file = item.tracks.iter().find(|f| f.index == track.index);
        let id = file.filter(|f| !f.content_id.is_empty()).map_or_else(|| format!("{}/file/{}", key.item, track.index), |f| f.content_id.clone());
        TrackSource {
            id: ItemKey::new(key.account, id).to_string(),
            url: track.url.clone(),
            suffix: file.and_then(|f| f.suffix.clone()),
            token,
            ..Default::default()
        }
    }
}

pub enum Note {
    Report { update: ProgressUpdate, close: bool },
    Refresh,
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Control {
    Toggle,
    Seek(f64),
    Skip(f64),
    PreviousChapter,
    NextChapter,
    Speed(f32),
    Stop,
}

pub struct Opened {
    pub key: ItemKey,
    pub item: Item,
    pub session: PlaySession,
    pub position: f64,
}

pub struct Book {
    pub key: ItemKey,
    pub item: Item,
    pub session: PlaySession,
    pub timeline: Timeline,
    files: Vec<TrackSource>,
    file: usize,
}

pub struct Player {
    pub book: Option<Book>,
    pub position: f64,
    pub speed: f32,
    pub paused: bool,
    pub active: bool,
    pending: bool,
    refreshed: bool,
    serial: u64,
    paused_at: Option<Instant>,
    last_tick: Instant,
    listened: Duration,
    audible: bool,
}
impl Default for Player {
    fn default() -> Self {
        Self {
            book: None,
            position: 0.0,
            speed: 1.0,
            paused: false,
            active: false,
            pending: false,
            refreshed: false,
            serial: 0,
            paused_at: None,
            last_tick: Instant::now(),
            listened: Duration::ZERO,
            audible: false,
        }
    }
}

impl Player {
    pub fn load(&mut self, opened: Opened, speed: f32, engine: &dyn Transport, now: Instant) -> Result<(), &'static str> {
        let Opened { key, item, session, position: at } = opened;
        let timeline = Timeline::new(
            &session.tracks.iter().map(|t| (t.start_offset, t.duration)).collect::<Vec<_>>(),
            item.chapters.clone(),
            session.duration.or(item.summary.duration),
        );
        let (file, _) = timeline.locate(at).ok_or("This book has no audio files.")?;
        let files = session
            .tracks
            .iter()
            .map(|t| {
                self.serial += 1;
                SessionSources.track(&key, &item, t, self.serial)
            })
            .collect();
        self.book = Some(Book { key, item, session, timeline, files, file });
        self.position = at;
        self.speed = normalize_speed(speed);
        self.active = true;
        self.paused = false;
        self.refreshed = false;
        self.paused_at = None;
        self.listened = Duration::ZERO;
        self.audible = false;
        self.last_tick = now;
        engine.send(Command::SetSpeed(self.speed));
        self.play_from(at, engine);
        Ok(())
    }

    fn next(&self, engine: &dyn Transport) {
        if let Some(book) = &self.book {
            engine.send(Command::SetNext(book.files.get(book.file + 1).cloned()));
        }
    }

    fn play_from(&mut self, at: f64, engine: &dyn Transport) {
        let Some(book) = &mut self.book else { return };
        let Some((file, offset)) = book.timeline.locate(at) else { return };
        book.file = file;
        self.position = book.timeline.clamp(at);
        self.pending = true;
        engine.send(Command::PlayFrom(book.files[file].clone(), Duration::from_secs_f64(offset)));
        self.next(engine);
        if self.paused {
            engine.send(Command::Pause);
        }
    }

    fn seek(&mut self, at: f64, engine: &dyn Transport) {
        let Some(book) = &self.book else { return };
        let Some((file, offset)) = book.timeline.locate(at) else { return };
        if file == book.file && self.active {
            self.position = book.timeline.clamp(at);
            self.pending = true;
            engine.send(Command::Seek(Duration::from_secs_f64(offset)));
        } else {
            self.play_from(at, engine);
        }
    }

    fn account_time(&mut self, engine: &dyn Transport, now: Instant) {
        let state = engine.state();
        if self.active && !self.paused && self.audible && state == PlayState::Playing {
            self.listened += now.saturating_duration_since(self.last_tick);
        }
        self.audible = self.active && !self.paused && state == PlayState::Playing;
        self.last_tick = now;
        if self.active
            && state != PlayState::Stopped
            && !self.pending
            && let Some(book) = &self.book
        {
            self.position = book.timeline.book_position(book.file, engine.position());
        }
    }

    fn report(&mut self, close: bool, finished: bool, notes: &mut Vec<Note>) {
        let Some(book) = &self.book else { return };
        notes.push(Note::Report {
            update: ProgressUpdate {
                item: book.key.clone(),
                position: Position::from_secs(self.position),
                finished,
                at: SystemTime::now(),
                listened: std::mem::take(&mut self.listened),
            },
            close,
        });
    }

    pub fn tick(&mut self, engine: &dyn Transport, now: Instant, report: bool, notes: &mut Vec<Note>) {
        self.account_time(engine, now);
        if report && self.active && !self.paused {
            self.report(false, false, notes);
        }
    }

    pub fn control(&mut self, control: Control, rewind: f64, engine: &dyn Transport, now: Instant, notes: &mut Vec<Note>) {
        self.account_time(engine, now);
        if self.book.is_none() {
            return;
        }
        match control {
            Control::Toggle if self.paused || !self.active => {
                let rewind = if self.paused_at.is_some_and(|at| now.saturating_duration_since(at) > Duration::from_secs(300)) { rewind } else { 0.0 };
                if !self.active {
                    self.position = 0.0;
                    self.play_from(0.0, engine);
                } else if rewind > 0.0 {
                    self.seek(self.position - rewind, engine);
                    self.report(false, false, notes);
                }
                self.paused = false;
                self.active = true;
                self.last_tick = now;
                engine.send(Command::Resume);
            }
            Control::Toggle => {
                self.paused = true;
                self.paused_at = Some(now);
                engine.send(Command::Pause);
                self.report(false, false, notes);
            }
            Control::Stop => {
                if self.active {
                    self.report(true, false, notes);
                }
                self.active = false;
                self.paused = true;
                engine.send(Command::Stop);
            }
            Control::Speed(speed) => {
                self.speed = normalize_speed(speed);
                engine.send(Command::SetSpeed(self.speed));
            }
            other => {
                let timeline = &self.book.as_ref().unwrap().timeline;
                let at = match other {
                    Control::Seek(at) => at,
                    Control::Skip(by) => self.position + by,
                    Control::PreviousChapter => timeline.previous_chapter(self.position),
                    Control::NextChapter => timeline.next_chapter(self.position),
                    _ => unreachable!(),
                };
                self.seek(at, engine);
                self.report(false, false, notes);
            }
        }
    }

    pub fn event(&mut self, event: &Event, engine: &dyn Transport, now: Instant, notes: &mut Vec<Note>) {
        if !self.active {
            return;
        }
        let Some(book) = &mut self.book else { return };
        match event {
            Event::TrackStarted { token, .. } => {
                if *token == book.files[book.file].token {
                    self.pending = false;
                    self.refreshed = false;
                    self.last_tick = now;
                } else if book.files.get(book.file + 1).is_some_and(|f| f.token == *token) {
                    book.file += 1;
                    self.position = book.timeline.book_position(book.file, engine.position());
                    self.pending = false;
                    self.next(engine);
                }
            }
            Event::Seeked { track_id, position_secs } if *track_id == book.files[book.file].id => {
                let landed = book.timeline.book_position(book.file, *position_secs);
                if (landed - self.position).abs() < 0.5 {
                    self.pending = false;
                }
            }
            Event::Ended { token, .. } if *token == book.files[book.file].token => {
                if book.file + 1 < book.files.len() {
                    let at = book.session.tracks[book.file + 1].start_offset;
                    self.play_from(at, engine);
                } else {
                    self.account_time(engine, now);
                    self.position = self.book.as_ref().unwrap().timeline.duration;
                    self.report(true, true, notes);
                    self.active = false;
                    self.paused = true;
                }
            }
            Event::Error { track_id: Some(id), message } if book.files.iter().any(|f| f.id == *id) => {
                if message.contains("http 401") && !self.refreshed {
                    self.refreshed = true;
                    notes.push(Note::Refresh);
                } else {
                    self.control(Control::Stop, 0.0, engine, now, notes);
                    notes.push(Note::Error("Couldn't play this book. Try again.".into()));
                }
            }
            Event::Error { track_id: None, .. } => {
                self.control(Control::Stop, 0.0, engine, now, notes);
                notes.push(Note::Error("Couldn't open the audio output.".into()));
            }
            _ => {}
        }
    }

    pub fn refreshed(&mut self, tracks: Vec<SessionTrack>, engine: &dyn Transport) {
        let Some(book) = &mut self.book else { return };
        if tracks.len() != book.files.len() {
            return;
        }
        for (file, track) in book.files.iter_mut().zip(&tracks) {
            file.url.clone_from(&track.url);
        }
        book.session.tracks = tracks;
        self.play_from(self.position, engine);
    }
}

/// The engine event forwarder owns this clock so reporting does not depend
/// on painted frames. State changes also wake accounting across buffer holds.
pub struct ReportClock {
    state: PlayState,
    last: Instant,
}
impl ReportClock {
    pub fn new(now: Instant) -> Self {
        Self { state: PlayState::Stopped, last: now }
    }

    pub fn poll(&mut self, state: PlayState, now: Instant) -> Option<bool> {
        let changed = state != self.state;
        let running = matches!(state, PlayState::Playing | PlayState::Buffering);
        let was_running = matches!(self.state, PlayState::Playing | PlayState::Buffering);
        if running && !was_running {
            self.last = now;
        }
        // An intermittent stream still needs reports while it alternates
        // between audible output and buffer holds.
        let due = running && now.saturating_duration_since(self.last) >= Duration::from_secs(15);
        self.state = state;
        if due {
            self.last = now;
        }
        (changed || due).then_some(due)
    }
}

/// Repaints only while source time can move. Paused and stopped players have
/// no timer; engine and bridge events wake the app themselves.
pub struct Live(pub bool);
impl Live {
    pub fn request(&self, ctx: &egui::Context) {
        if self.0 {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests;
