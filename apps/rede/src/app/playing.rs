//! The app supplies the real engine and the scoped session worker. Views only
//! enqueue controls; notes return to the worker in exactly the order produced.

use super::{Action, RedeApp};
use crate::failure::Failure;
use crate::listening::{Event, Request};
use crate::player::{Control, Live, Note};
use std::time::{Duration, Instant, SystemTime};

/// How long a resume waits for the server before playing from the local
/// position. A lossy connection must not hold the play press.
const RESUME_WAIT: Duration = Duration::from_millis(2_500);

impl RedeApp {
    fn ensure_engine(&mut self) -> Result<(), Failure> {
        if self.engine.is_none() {
            let rt = self._rt.0.as_ref().expect("the app runtime is alive");
            let mut config = streamcache::CacheConfig::new(&self.stream_dir);
            // ABS uses HTTP error statuses. The default accepts successful
            // bodies; streamcache itself rejects 401 before caching bytes.
            config.reply_check = streamcache::ReplyCheck::default();
            let cache =
                streamcache::StreamCache::new(config, rt.handle().clone()).map_err(|e| Failure::Local(format!("Couldn't open the stream cache: {e}")))?;
            let engine = audio::spawn(audio::EngineConfig::default(), audio::default_backend(), Some(cache));
            crate::player::apply_speech(self.shown.playback, &engine);
            self.player.set_volume(self.shown.playback.level(), &engine);
            self.bridge.engine_events(&engine, self.ctx.clone(), self.sleep_wake.clone());
            self.engine = Some(engine);
        }
        Ok(())
    }

    pub(super) fn play_book(&mut self, id: String, at: Option<f64>) {
        if self.player.book.as_ref().is_some_and(|b| b.key.item == id) && self.player.active && at.is_none() {
            if self.player.paused {
                self.playback_control(Control::Toggle);
            }
            return;
        }
        let Some(account) = self.session.account.clone() else {
            self.show_failure(Failure::Local("Not connected. Sign in to play this book.".into()));
            return;
        };
        let key = sources::ItemKey::new(account.id, id);
        let Some(listening) = self.start_listening() else {
            self.show_failure(Failure::Local("Not connected. Sign in to play this book.".into()));
            return;
        };
        self.playback_control(Control::Stop);
        self.player.book = None;
        self.playback_generation += 1;
        self.opening = true;
        self.resume_pending = false;
        self.resume_deadline = None;
        listening.send(Request::Open { generation: self.playback_generation, key, at });
    }

    /// The worker that keeps the progress log, the pull and the server session.
    /// Started on first use, so a book can be settled without being opened.
    /// `None` means nothing is signed in; the caller says so in its own words.
    fn start_listening(&mut self) -> Option<crate::listening::Listening> {
        if self.listening.is_none() {
            let (Some(account), Some(source), Some(shelf)) = (&self.session.account, &self.session.source, &self.session.shelf) else { return None };
            self.listening = Some(self.bridge.listening(
                source.clone(),
                shelf.clone(),
                sources::DeviceInfo {
                    device_id: account.device_id.clone(),
                    client_name: "Rede".into(),
                    client_version: env!("CARGO_PKG_VERSION").into(),
                    ..Default::default()
                },
            ));
        }
        self.listening.clone()
    }

    pub(super) fn mark_finished(&mut self, id: String) {
        // Stopping closes the session before the finish is written. A sync
        // after it would send the old position, and ABS un-finishes a finished
        // book whose position moves back before its end threshold.
        if self.player.book.as_ref().is_some_and(|b| b.key.item == id) {
            self.playback_control(Control::Stop);
        }
        // A finished book keeps the place it got to, so marking it unfinished
        // again can go back there.
        let at = self.shown_position(&id).unwrap_or(0.0);
        self.settle(id, at, true);
    }

    pub(super) fn mark_unfinished(&mut self, id: String) {
        // ABS throws the position away in the request that clears a finish, so
        // the known one goes in a second request (`AbsSource::set_progress`
        // sends the reset first). Without a known place the book is at its
        // start, and the server may auto-finish it again if that place is
        // within its end threshold.
        let at = self.shown_position(&id).unwrap_or(0.0);
        self.settle(id, at, false);
    }

    pub(super) fn start_over(&mut self, id: String) {
        if self.player.book.as_ref().is_some_and(|b| b.key.item == id) {
            // The listener asked for this, so moving running audio is right.
            // The seek reports the start through the session and leaves the
            // book as it was, paused or playing; a book loaded without an
            // engine only has its place to move, as a pull's move does.
            if self.player.active && self.engine.is_some() {
                self.playback_control(Control::Seek(0.0));
            } else {
                self.player.position = 0.0;
            }
        }
        self.settle(id, 0.0, false);
    }

    /// Where the app shows a book: the player's place for the book it has
    /// loaded and playing (its reports keep it there), else what was saved.
    fn shown_position(&self, id: &str) -> Option<f64> {
        if self.player.active && self.player.book.as_ref().is_some_and(|b| b.key.item == id) {
            return Some(self.player.position);
        }
        self.positions.get(id).map(|p| p.position.secs())
    }

    /// Writes one intent as a progress entry and shows it at once. The entry's
    /// stamp is after the server's own, as Undo's is, so a pull that races the
    /// write cannot bring the old state back; a write that fails stays in the
    /// log and replays in order with everything else pending.
    fn settle(&mut self, id: String, at: f64, finished: bool) {
        let (Some(account), Some(listening)) = (self.session.account.clone(), self.start_listening()) else {
            self.show_failure(Failure::Local("Not connected. Sign in to change this book's progress.".into()));
            return;
        };
        let key = sources::ItemKey::new(account.id, id);
        let stamp = SystemTime::now().max(
            self.positions
                .get(&key.item)
                .and_then(|p| p.at)
                .unwrap_or(SystemTime::UNIX_EPOCH)
                .checked_add(Duration::from_millis(1))
                .unwrap_or(SystemTime::now()),
        );
        let position = sources::Position::from_secs(at);
        self.positions.insert(key.item.clone(), sources::Progress { item: key.clone(), position, finished, at: Some(stamp) });
        listening.send(Request::Set(sources::ProgressUpdate { item: key, position, finished, at: stamp, listened: Duration::ZERO }));
    }

    pub(super) fn playback_control(&mut self, control: Control) {
        // Resuming asks the server first: the phone may have moved on since this
        // was paused. A second press while that's in flight plays at once.
        if control == Control::Toggle
            && self.player.active
            && self.player.paused
            && !self.sync.offline
            && !self.resume_pending
            && let Some(listening) = &self.listening
        {
            self.resume_pending = true;
            self.resume_deadline = Some(Instant::now() + RESUME_WAIT);
            self.send_activity();
            listening.send(Request::Resume { generation: self.playback_generation });
            return;
        }
        // A seek during the pull is the listener's choice and must not cancel
        // the play press: ResumeReady still starts the book. Any other control
        // (a second play press included) takes over at once.
        if !matches!(control, Control::Seek(_)) {
            self.resume_pending = false;
            self.resume_deadline = None;
        }
        self.apply_playback_control(control);
    }

    fn apply_playback_control(&mut self, control: Control) {
        if !self.player.active
            && let Some(book) = &self.player.book
            && control == Control::Toggle
        {
            let id = book.key.item.clone();
            self.play_book(id, None);
            return;
        }
        let mut notes = Vec::new();
        self.player.sleep_rewind = f64::from(self.shown.playback.sleep_rewind);
        if let Some(engine) = &self.engine {
            self.player.control(control, f64::from(self.shown.playback.rewind), engine, Instant::now(), &mut notes);
        }
        if let Control::Sleep(Some(crate::sleep::Choice::Minutes(minutes))) = control {
            self.prefs.edit(|s| s.playback.sleep_minutes = minutes.clamp(5, 120));
        }
        self.sleep_wake.store(self.player.sleep.is_some(), std::sync::atomic::Ordering::Relaxed);
        if let Control::Speed(speed) = control {
            let speed = crate::timeline::normalize_speed(speed);
            self.prefs.edit(|s| s.playback.speed = speed);
            self.shown.playback.speed = speed;
        }
        self.playback_notes(notes);
        self.send_activity();
    }

    fn playback_notes(&mut self, notes: Vec<Note>) {
        #[cfg(target_os = "macos")]
        if self.now_playing.update(&self.player, self.engine.as_ref().map_or(audio::PlayState::Stopped, |e| e.status.state()), Instant::now())
            && let (Some(book), Some(source)) = (&self.player.book, &self.session.source)
            && let Some(cover) = &book.item.summary.cover
        {
            self.bridge.artwork(source.clone(), book.key.to_string(), cover.clone());
        }

        self.sleep_wake.store(self.player.sleep.is_some(), std::sync::atomic::Ordering::Relaxed);
        for note in notes {
            match note {
                Note::Report { update, close } => {
                    if let Some(listening) = &self.listening {
                        self.positions.insert(
                            update.item.item.clone(),
                            sources::Progress { item: update.item.clone(), position: update.position, finished: update.finished, at: Some(update.at) },
                        );
                        listening.send(Request::Report { generation: self.playback_generation, update, close });
                    }
                }
                Note::Refresh => {
                    if let Some(listening) = &self.listening {
                        listening.send(Request::Refresh { generation: self.playback_generation });
                    }
                }
                Note::Error(message) => self.show_error(message),
            }
        }
    }

    pub(super) fn playback_event(&mut self, event: Event) {
        match event {
            Event::Opened { generation, result } if generation == self.playback_generation => {
                self.opening = false;
                match result {
                    Ok(opened) => {
                        if let Err(error) = self.ensure_engine() {
                            self.show_failure(error);
                            return;
                        }
                        let result = self.player.load(*opened, self.shown.playback.speed, self.engine.as_ref().unwrap(), Instant::now());
                        if let Err(error) = result {
                            self.show_error(error);
                        }
                        self.send_activity();
                    }
                    Err(error) => self.show_failure(error),
                }
            }
            Event::Refreshed { generation, result } if generation == self.playback_generation => match result {
                Ok(tracks) => {
                    if self.player.active
                        && let Some(engine) = &self.engine
                    {
                        self.player.refreshed(tracks, engine);
                    }
                }
                Err(error) => {
                    self.playback_control(Control::Stop);
                    self.show_failure(error);
                }
            },
            Event::Positions(positions) => {
                let any = !positions.is_empty();
                for position in positions {
                    let old = self.positions.remove(&position.item.item);
                    let latest = crate::listening::newer(old, Some(position)).unwrap();
                    // A paused book is moved only for a real change. Comparing
                    // exact f64s made an unchanged position look different on
                    // every pull and re-seeked the engine.
                    if self.player.paused
                        && self.player.book.as_ref().is_some_and(|b| b.key == latest.item)
                        && (self.player.position - latest.position.secs()).abs() > 0.5
                    {
                        self.move_to_reconciled(latest.position.secs());
                    }
                    self.positions.insert(latest.item.item.clone(), latest);
                }
                self.land(any);
            }
            Event::ResumeReady { generation } if generation == self.playback_generation && self.resume_pending => {
                self.resume_pending = false;
                self.resume_deadline = None;
                if self.player.paused {
                    self.apply_playback_control(Control::Toggle);
                }
            }
            Event::Reconciled(reconciled) => self.reconciled(reconciled),
            Event::Bookmarks(result) => match result {
                Ok(rows) => self.bookmark_event(crate::bridge::BookmarkMsg::Updated(rows)),
                Err(e) => self.show_failure(e),
            },
            Event::Error(error) => self.show_failure(error),
            _ => {}
        }
    }

    pub(super) fn engine_event(&mut self, event: audio::Event) {
        let mut notes = Vec::new();
        if let Some(engine) = &self.engine {
            self.player.event(&event, engine, Instant::now(), &mut notes);
        }
        self.playback_notes(notes);
        self.send_activity();
    }

    pub(super) fn report_playback(&mut self, report: bool) {
        let mut notes = Vec::new();
        if let Some(engine) = &self.engine {
            self.player.tick(engine, Instant::now(), report, &mut notes);
        }
        self.playback_notes(notes);
    }

    pub(super) fn playback_tick(&mut self, ctx: &egui::Context) {
        // A resume pull that never answers falls back to the local position,
        // rather than leaving a play press waiting on a lossy network.
        if self.resume_pending {
            let deadline = *self.resume_deadline.get_or_insert_with(|| Instant::now() + RESUME_WAIT);
            let now = Instant::now();
            if now >= deadline {
                self.resume_pending = false;
                self.resume_deadline = None;
                if self.player.paused {
                    self.apply_playback_control(Control::Toggle);
                }
            } else {
                ctx.request_repaint_after(deadline - now);
            }
        }
        self.report_playback(false);
        if let Some(engine) = &self.engine {
            // Hidden windows run logic without a UI pass. Their reporting clock
            // and engine events suffice; only a visible bar needs animation.
            let visible = ctx.input(|i| i.viewport().visible().unwrap_or(true));
            Live(visible && self.player.active && !self.player.paused && engine.status.state() == audio::PlayState::Playing).request(ctx);
        }
    }

    pub(super) fn playback_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() || self.player.book.is_none() {
            return;
        }
        let prefs = self.shown.playback;
        let control = ctx.input_mut(|i| {
            use egui::{Key, Modifiers};
            if i.consume_key(Modifiers::NONE, Key::Space) {
                Some(Control::Toggle)
            } else if i.consume_key(Modifiers::SHIFT, Key::ArrowLeft) {
                Some(Control::PreviousChapter)
            } else if i.consume_key(Modifiers::SHIFT, Key::ArrowRight) {
                Some(Control::NextChapter)
            } else if i.consume_key(Modifiers::NONE, Key::ArrowLeft) {
                Some(Control::Skip(-f64::from(prefs.skip_back)))
            } else if i.consume_key(Modifiers::NONE, Key::ArrowRight) {
                Some(Control::Skip(f64::from(prefs.skip_forward)))
            } else if i.consume_key(Modifiers::NONE, Key::OpenBracket) {
                Some(Control::Speed(self.player.speed - 0.05))
            } else if i.consume_key(Modifiers::NONE, Key::CloseBracket) {
                Some(Control::Speed(self.player.speed + 0.05))
            } else {
                None
            }
        });
        if let Some(control) = control {
            self.actions.push(Action::Playback(control));
        }
    }

    pub(super) fn finish_playback(&mut self) {
        self.playback_control(Control::Stop);
        if let Some(engine) = self.engine.take() {
            engine.shutdown(Duration::from_secs(2));
        }
        if let Some(listening) = self.listening.take()
            && let Some(rt) = &self._rt.0
        {
            let done = listening.finish();
            let _ = rt.block_on(async { tokio::time::timeout(Duration::from_secs(2), done).await });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paused_book_ignores_a_position_change_under_half_a_second() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = super::super::fixture::app(&ctx, dir.path(), 1);
        super::super::fixture::playing(&mut app, true);
        let key = app.player.book.as_ref().unwrap().key.clone();
        app.player.position = 100.0;
        let positions = |secs: f64| {
            vec![sources::Progress { item: key.clone(), position: sources::Position::from_secs(secs), finished: false, at: Some(std::time::SystemTime::now()) }]
        };
        app.playback_event(Event::Positions(positions(100.4)));
        assert_eq!(app.player.position, 100.0, "a fraction of a second is the same place");
        app.playback_event(Event::Positions(positions(102.0)));
        assert_eq!(app.player.position, 102.0, "a real move still moves the book");
    }

    #[test]
    fn a_seek_during_a_resume_pull_keeps_the_play_press_and_other_controls_take_over() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = super::super::fixture::app(&ctx, dir.path(), 1);
        super::super::fixture::playing(&mut app, true);
        app.resume_pending = true;
        app.playback_control(Control::Seek(60.0));
        assert!(app.resume_pending, "the seek must not drop the play that is waiting on the pull");
        app.playback_control(Control::Stop);
        assert!(!app.resume_pending, "stopping cancels it");
    }

    #[test]
    fn a_resume_pull_that_outlasts_its_deadline_plays_from_the_local_position() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = super::super::fixture::app(&ctx, dir.path(), 1);
        super::super::fixture::playing(&mut app, true);
        app.resume_pending = true;
        app.resume_deadline = Some(Instant::now() - Duration::from_millis(1));
        app.playback_tick(&ctx);
        assert!(!app.resume_pending, "the wait is over and playback may start");
        assert_eq!(app.resume_deadline, None);
    }
}
