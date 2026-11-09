use super::*;
use audio::StreamFormat;
use sources::{AccountId, Chapter, ItemSummary, TrackFile};
use std::cell::{Cell, RefCell};

struct Fake {
    sent: RefCell<Vec<Command>>,
    state: Cell<PlayState>,
    position: Cell<f64>,
}
impl Default for Fake {
    fn default() -> Self {
        Self { sent: RefCell::default(), state: Cell::new(PlayState::Loading), position: Cell::new(0.0) }
    }
}
impl Transport for Fake {
    fn send(&self, cmd: Command) {
        self.sent.borrow_mut().push(cmd);
    }
    fn state(&self) -> PlayState {
        self.state.get()
    }
    fn position(&self) -> f64 {
        self.position.get()
    }
}

pub fn opened(single: bool) -> Opened {
    let key = ItemKey::new(AccountId::new(), "book");
    let ranges = if single { vec![(0.0, 100.0)] } else { vec![(0.0, 60.0), (60.0, 40.0)] };
    let tracks: Vec<_> = ranges
        .iter()
        .enumerate()
        .map(|(i, &(start_offset, duration))| TrackFile {
            index: i as u32 + 1,
            start_offset,
            duration: Some(duration),
            content_id: format!("file-{}", i + 1),
            suffix: Some("m4b".into()),
            ..Default::default()
        })
        .collect();
    let item = Item {
        summary: ItemSummary { id: "book".into(), title: "Book".into(), duration: Some(100.0), ..Default::default() },
        tracks: tracks.clone(),
        chapters: vec![Chapter { start: 0.0, end: 40.0, title: "First".into() }, Chapter { start: 40.0, end: 100.0, title: "Second".into() }],
        ..Default::default()
    };
    let session = PlaySession {
        id: "session".into(),
        item: "book".into(),
        start: Position::from_secs(75.0),
        duration: Some(100.0),
        tracks: tracks
            .iter()
            .map(|t| SessionTrack {
                index: t.index,
                start_offset: t.start_offset,
                duration: t.duration,
                url: format!("https://example.com/{}?token=secret", t.content_id),
                ..Default::default()
            })
            .collect(),
    };
    Opened { key, item, session, position: 75.0 }
}

fn started(player: &Player, file: usize, gapless: bool) -> Event {
    let file = &player.book.as_ref().unwrap().files[file];
    Event::TrackStarted {
        track_id: file.id.clone(),
        token: file.token,
        format: StreamFormat { sample_rate: 48_000, channels: 2, bits_per_sample: Some(16) },
        gapless,
        restart: false,
    }
}

fn landed(player: &Player, at: f64) -> Event {
    let book = player.book.as_ref().unwrap();
    Event::Seeked { track_id: book.files[book.file].id.clone(), position_secs: at }
}

#[test]
fn resuming_plays_the_right_file_at_its_source_offset() {
    for (single, file, offset) in [(true, 0, 75.0), (false, 1, 15.0)] {
        let engine = Fake::default();
        let mut player = Player::default();
        let open = opened(single);
        let account = open.key.account.to_string();
        player.load(open, 1.5, &engine, Instant::now()).unwrap();
        let sent = engine.sent.borrow();
        assert!(matches!(sent[0], Command::SetSpeed(s) if s == 1.5));
        assert!(matches!(&sent[1], Command::PlayFrom(src, at) if src.id == format!("{account}/file-{}", file + 1) && at.as_secs_f64() == offset));
        assert!(matches!(sent[2], Command::SetNext(None)));
    }
}

#[test]
fn the_next_file_is_prefetched_and_advanced_when_the_output_reaches_it() {
    let engine = Fake::default();
    let mut player = Player::default();
    let mut open = opened(false);
    open.position = 20.0;
    player.load(open, 1.0, &engine, Instant::now()).unwrap();
    assert!(matches!(&engine.sent.borrow()[2], Command::SetNext(Some(src)) if src.id.ends_with("/file-2")));
    engine.position.set(3.0);
    let mut notes = Vec::new();
    player.event(&started(&player, 1, true), &engine, Instant::now(), &mut notes);
    assert_eq!(player.position, 63.0);
    assert!(matches!(engine.sent.borrow().last(), Some(Command::SetNext(None))));
    assert!(notes.is_empty());
}

#[test]
fn seeking_within_a_file_uses_seek_and_crossing_a_file_uses_play_from() {
    let engine = Fake::default();
    let mut player = Player::default();
    let now = Instant::now();
    player.load(opened(false), 1.0, &engine, now).unwrap();
    let mut notes = Vec::new();
    player.control(Control::Seek(80.0), 5.0, &engine, now, &mut notes);
    assert!(matches!(engine.sent.borrow().last(), Some(Command::Seek(at)) if at.as_secs_f64() == 20.0));
    player.control(Control::Seek(10.0), 5.0, &engine, now, &mut notes);
    assert!(engine.sent.borrow().iter().any(|c| matches!(c, Command::PlayFrom(src, at) if src.id.ends_with("file-1") && at.as_secs_f64() == 10.0)));
    player.control(Control::Seek(500.0), 5.0, &engine, now, &mut notes);
    assert_eq!(player.position, 100.0);
}

#[test]
fn chapter_controls_and_skips_use_book_time() {
    let engine = Fake::default();
    let mut player = Player::default();
    let now = Instant::now();
    player.load(opened(false), 1.0, &engine, now).unwrap();
    let mut notes = Vec::new();
    for (control, expected) in [
        (Control::Skip(-15.0), 60.0),
        (Control::Skip(30.0), 90.0),
        (Control::PreviousChapter, 40.0),
        (Control::PreviousChapter, 0.0),
        (Control::NextChapter, 40.0),
    ] {
        player.control(control, 5.0, &engine, now, &mut notes);
        assert_eq!(player.position, expected);
    }
}

#[test]
fn a_long_pause_rewinds_on_resume_and_short_pauses_do_not() {
    for (pause, rewind, expected) in [(300, 5.0, 75.0), (301, 5.0, 70.0), (600, 0.0, 75.0)] {
        let engine = Fake::default();
        let mut player = Player::default();
        let now = Instant::now();
        player.load(opened(true), 1.0, &engine, now).unwrap();
        let mut notes = Vec::new();
        player.control(Control::Toggle, rewind, &engine, now, &mut notes);
        player.control(Control::Toggle, rewind, &engine, now + Duration::from_secs(pause), &mut notes);
        assert_eq!(player.position, expected);
        assert!(!player.paused);
        assert!(matches!(engine.sent.borrow().last(), Some(Command::Resume)));
    }
}

#[test]
fn reports_count_wall_time_and_exclude_pause_and_buffering() {
    let engine = Fake::default();
    let mut player = Player::default();
    let now = Instant::now();
    player.load(opened(true), 3.0, &engine, now).unwrap();
    let mut notes = Vec::new();
    engine.state.set(PlayState::Playing);
    player.tick(&engine, now, false, &mut notes);
    player.tick(&engine, now + Duration::from_secs(15), true, &mut notes);
    assert!(matches!(&notes[0], Note::Report { update, close: false } if update.listened == Duration::from_secs(15)));
    player.control(Control::Toggle, 0.0, &engine, now + Duration::from_secs(16), &mut notes);
    assert!(matches!(notes.last(), Some(Note::Report { update, .. }) if update.listened == Duration::from_secs(1)));
    engine.state.set(PlayState::Paused);
    player.tick(&engine, now + Duration::from_secs(600), true, &mut notes);
    let n = notes.len();
    player.control(Control::Toggle, 0.0, &engine, now + Duration::from_secs(600), &mut notes);
    engine.state.set(PlayState::Buffering);
    player.tick(&engine, now + Duration::from_secs(610), true, &mut notes);
    assert_eq!(notes.len(), n + 1);
    assert!(matches!(notes.last(), Some(Note::Report { update, .. }) if update.listened.is_zero()));
}

#[test]
fn a_failed_token_refresh_is_bounded_and_retries_at_the_book_position() {
    let engine = Fake::default();
    let mut player = Player::default();
    let now = Instant::now();
    player.load(opened(false), 1.0, &engine, now).unwrap();
    let id = player.book.as_ref().unwrap().files[1].id.clone();
    let error = Event::Error { track_id: Some(id), message: "probe: io error: http 401 Unauthorized".into() };
    let mut notes = Vec::new();
    player.event(&error, &engine, now, &mut notes);
    assert!(matches!(notes[0], Note::Refresh));
    let mut tracks = player.book.as_ref().unwrap().session.tracks.clone();
    tracks[1].url = "https://example.com/file?token=rotated".into();
    player.refreshed(tracks, &engine);
    assert!(engine.sent.borrow().iter().any(|c| matches!(c, Command::PlayFrom(src, at) if src.url.ends_with("token=rotated") && at.as_secs_f64() == 15.0)));
    player.event(&error, &engine, now, &mut notes);
    assert!(!player.active);
    assert_eq!(notes.iter().filter(|n| matches!(n, Note::Refresh)).count(), 1);
}

#[test]
fn the_last_file_finishes_the_book_and_closes_the_session() {
    let engine = Fake::default();
    let mut player = Player::default();
    let now = Instant::now();
    player.load(opened(false), 1.0, &engine, now).unwrap();
    let book = player.book.as_ref().unwrap();
    let ended = Event::Ended { track_id: book.files[1].id.clone(), token: book.files[1].token };
    let mut notes = Vec::new();
    player.event(&ended, &engine, now, &mut notes);
    assert!(!player.active);
    assert!(matches!(&notes[0], Note::Report { update, close: true } if update.finished && update.position.secs() == 100.0));
}

#[test]
fn stale_track_events_do_not_clear_a_new_seek_or_finish_a_new_book() {
    let engine = Fake::default();
    let mut player = Player::default();
    let now = Instant::now();
    player.load(opened(false), 1.0, &engine, now).unwrap();
    let old = started(&player, 1, false);
    player.load(opened(false), 1.0, &engine, now).unwrap();
    let mut notes = Vec::new();
    player.event(&old, &engine, now, &mut notes);
    engine.position.set(0.0);
    player.tick(&engine, now, false, &mut notes);
    assert_eq!(player.position, 75.0);
    player.control(Control::Seek(85.0), 0.0, &engine, now, &mut notes);
    player.event(&landed(&player, 1.0), &engine, now, &mut notes);
    player.tick(&engine, now, false, &mut notes);
    assert_eq!(player.position, 85.0);
}

#[tokio::test]
async fn a_real_fetch_rejection_reaches_the_player_as_a_redacted_401() {
    use std::io::{BufRead, Write};
    use std::sync::Arc;
    struct NoOutput;
    impl audio::OutputBackend for NoOutput {
        fn name(&self) -> &str {
            "no output needed"
        }
        fn device_caps(&mut self) -> Option<audio::DeviceCaps> {
            None
        }
        fn open(&mut self, _: StreamFormat, _: audio::RingInput, _: Arc<audio::Status>) -> Result<(), audio::OutputError> {
            Err(audio::OutputError::NoDevice)
        }
        fn drained(&self) -> bool {
            true
        }
        fn close(&mut self) {}
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut reader = std::io::BufReader::new(&stream);
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
        }
        stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let cache = streamcache::StreamCache::new(streamcache::CacheConfig::new(dir.path()), tokio::runtime::Handle::current()).unwrap();
    let engine = audio::spawn(audio::EngineConfig::default(), Box::new(NoOutput), Some(cache));
    let mut input = opened(true);
    input.session.tracks[0].url = format!("http://{address}/file?token=PRIVATE");
    let mut player = Player::default();
    player.load(input, 1.0, &engine, Instant::now()).unwrap();
    let events = engine.events.clone();
    let event = tokio::task::spawn_blocking(move || {
        loop {
            let event = events.recv_timeout(Duration::from_secs(5)).unwrap();
            if matches!(event, Event::Error { .. }) {
                return event;
            }
        }
    })
    .await
    .unwrap();
    engine.shutdown(Duration::from_secs(1));
    server.join().unwrap();
    let Event::Error { message, .. } = &event else { unreachable!() };
    assert!(message.contains("http 401"), "the status survives decoding: {message}");
    assert!(!message.contains("PRIVATE"));
    let mut notes = Vec::new();
    player.event(&event, &engine, Instant::now(), &mut notes);
    player.tick(&engine, Instant::now(), false, &mut notes);
    assert!(matches!(notes.first(), Some(Note::Refresh)));
    assert_eq!(player.position, 75.0, "the stopped engine cannot erase the retry position");
}

#[test]
fn a_hidden_book_reports_and_advances_without_any_ui_passes() {
    let engine = Fake::default();
    let mut player = Player::default();
    let now = Instant::now();
    let mut open = opened(false);
    open.position = 0.0;
    player.load(open, 1.0, &engine, now).unwrap();
    let ctx = egui::Context::default();
    let mut input = egui::RawInput::default();
    input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap().minimized = Some(true);
    let mut clock = ReportClock::new(now);
    let mut notes = Vec::new();
    engine.state.set(PlayState::Playing);
    let _ = ctx.run_logic(&input, |_| player.event(&started(&player, 0, false), &engine, now, &mut notes));
    for second in 0..=60 {
        let at = now + Duration::from_secs(second);
        engine.position.set(second as f64);
        if let Some(report) = clock.poll(engine.state(), at) {
            let _ = ctx.run_logic(&input, |_| player.tick(&engine, at, report, &mut notes));
        }
    }
    let reports: Vec<_> = notes
        .iter()
        .filter_map(|n| match n {
            Note::Report { update, .. } => Some((update.position.secs(), update.listened)),
            _ => None,
        })
        .collect();
    assert_eq!(reports, [15.0, 30.0, 45.0, 60.0].map(|at| (at, Duration::from_secs(15))));
    // The event forwarder wakes this same logic path at file boundaries.
    engine.position.set(0.0);
    let _ = ctx.run_logic(&input, |_| player.event(&started(&player, 1, true), &engine, now + Duration::from_secs(60), &mut notes));
    assert_eq!(player.position, 60.0);
    assert!(matches!(engine.sent.borrow().last(), Some(Command::SetNext(None))));
    let file = &player.book.as_ref().unwrap().files[1];
    let ended = Event::Ended { track_id: file.id.clone(), token: file.token };
    engine.state.set(PlayState::Stopped);
    let _ = ctx.run_logic(&input, |_| player.event(&ended, &engine, now + Duration::from_secs(100), &mut notes));
    assert!(matches!(notes.last(), Some(Note::Report { update, close: true }) if update.finished && update.position.secs() == 100.0));
    assert!(!player.active);
    assert_eq!(ctx.cumulative_pass_nr(), 0);
}

#[test]
fn the_report_clock_sleeps_while_paused_and_stopped() {
    let now = Instant::now();
    let mut clock = ReportClock::new(now);
    for state in [PlayState::Paused, PlayState::Stopped] {
        clock.poll(state, now);
        for second in 1..=600 {
            assert!(clock.poll(state, now + Duration::from_secs(second)).is_none());
        }
    }
}

#[test]
fn buffering_and_loading_do_not_accrue_listening_across_recovery() {
    let engine = Fake::default();
    let mut player = Player::default();
    let now = Instant::now();
    player.load(opened(true), 1.0, &engine, now).unwrap();
    let mut notes = Vec::new();
    for (second, state) in [
        (0, PlayState::Playing),
        (10, PlayState::Playing),
        (11, PlayState::Buffering),
        (40, PlayState::Buffering),
        (41, PlayState::Loading),
        (70, PlayState::Loading),
        (71, PlayState::Playing),
        (81, PlayState::Playing),
    ] {
        engine.state.set(state);
        player.tick(&engine, now + Duration::from_secs(second), true, &mut notes);
    }
    player.control(Control::Stop, 0.0, &engine, now + Duration::from_secs(81), &mut notes);
    let listened: Duration = notes
        .iter()
        .filter_map(|n| match n {
            Note::Report { update, .. } => Some(update.listened),
            _ => None,
        })
        .sum();
    assert_eq!(listened, Duration::from_secs(20));
}

#[tokio::test]
async fn refreshed_urls_reuse_the_account_qualified_cache_file_without_tokens() {
    let engine = Fake::default();
    let mut player = Player::default();
    let input = opened(true);
    let mut other = opened(true);
    other.key.account = AccountId::new();
    for fallback in [false, true] {
        let mut input = Opened { key: input.key.clone(), item: input.item.clone(), session: input.session.clone(), position: input.position };
        if fallback {
            input.item.tracks[0].content_id.clear();
        }
        let before = SessionSources.track(&input.key, &input.item, &input.session.tracks[0], 1);
        let other = SessionSources.track(&other.key, &input.item, &input.session.tracks[0], 1);
        let key = streamcache::CacheKey::original(before.id.clone(), before.suffix.clone());
        let other_key = streamcache::CacheKey::original(other.id, other.suffix);
        player.load(input, 1.0, &engine, Instant::now()).unwrap();
        let mut refreshed = player.book.as_ref().unwrap().session.tracks.clone();
        refreshed[0].url = "https://example.com/file?token=rotated-private-token".into();
        player.refreshed(refreshed, &engine);
        let sent = engine.sent.borrow();
        let after = sent
            .iter()
            .rev()
            .find_map(|cmd| match cmd {
                Command::PlayFrom(source, _) => Some(source),
                _ => None,
            })
            .unwrap();
        let after_key = streamcache::CacheKey::original(after.id.clone(), after.suffix.clone());
        assert_eq!(key, after_key);
        assert_eq!(key.file_name(), after_key.file_name());
        assert_ne!(key.file_name(), other_key.file_name());
        assert!(!key.file_name().contains("secret"));
        assert!(!after_key.file_name().contains("rotated-private-token"));
        assert_ne!(before.url, after.url);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(key.file_name()), b"cached book audio").unwrap();
        let cache = streamcache::StreamCache::new(streamcache::CacheConfig::new(dir.path()), tokio::runtime::Handle::current()).unwrap();
        for url in [&before.url, &after.url] {
            let mut cached = cache.open(&after_key, url, std::sync::Arc::new(|| true)).unwrap();
            assert!(matches!(cached, streamcache::CachedSource::File { .. }));
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut cached, &mut bytes).unwrap();
            assert_eq!(bytes, b"cached book audio");
        }
    }
}

#[test]
fn a_hidden_file_end_starts_the_next_file_when_gapless_playback_was_unavailable() {
    let engine = Fake::default();
    let mut player = Player::default();
    let now = Instant::now();
    let mut open = opened(false);
    open.position = 0.0;
    player.load(open, 1.0, &engine, now).unwrap();
    let file = &player.book.as_ref().unwrap().files[0];
    let ended = Event::Ended { track_id: file.id.clone(), token: file.token };
    let ctx = egui::Context::default();
    let mut notes = Vec::new();
    let _ = ctx.run_logic(&egui::RawInput::default(), |_| player.event(&ended, &engine, now, &mut notes));
    assert!(engine.sent.borrow().iter().any(|cmd| matches!(cmd,
        Command::PlayFrom(src, at) if src.id.ends_with("file-2") && at.is_zero()
    )));
    assert!(player.active);
    assert!(notes.is_empty());
    assert_eq!(ctx.cumulative_pass_nr(), 0);
}
