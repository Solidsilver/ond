//! One opt-in, release-only playback round trip. Cleanup runs after every
//! fallible playback step, before any assertion can leave server progress.

use super::*;
use crate::player::{Control, Note, Player};
use std::time::{Duration, Instant};

type Check<T = ()> = std::result::Result<T, &'static str>;

async fn receive(events: &mut mpsc::UnboundedReceiver<Event>) -> Check<Box<Opened>> {
    tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(event) = events.recv().await {
            if let Event::Opened { result, .. } = event {
                return result.map_err(|_| "opening the play session failed");
            }
        }
        Err("the session worker stopped")
    })
    .await
    .map_err(|_| "opening timed out")?
}

fn report(notes: Vec<Note>, listening: &Listening) -> Check {
    for note in notes {
        match note {
            Note::Report { update, close } => listening.send(Request::Report { generation: 1, update, close }),
            Note::Refresh => listening.send(Request::Refresh { generation: 1 }),
            Note::Error(_) => return Err("the audio engine reported an error"),
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "real audio and ABS; release only; creates then deletes progress"]
async fn a_real_book_plays_twenty_seconds_and_leaves_no_progress() {
    if cfg!(debug_assertions) {
        panic!("run this test with --release");
    }
    let required = |name| std::env::var(name).expect("set ABS_TEST_URL, ABS_TEST_USER and ABS_TEST_PASSWORD");
    let client = abs::Client::new(&required("ABS_TEST_URL"), abs::DeviceInfo::new("rede-playback-live-test", env!("CARGO_PKG_VERSION")))
        .unwrap_or_else(|_| panic!("invalid configured ABS address"));
    client.login(&required("ABS_TEST_USER"), &required("ABS_TEST_PASSWORD")).await.unwrap_or_else(|_| panic!("test sign-in failed"));
    let before = client.progress().await.unwrap_or_else(|_| panic!("initial progress read failed"));
    assert!(before.is_empty(), "the live test account must start with no progress");
    let source = Arc::new(sources::AbsSource::new(
        sources::AccountId::new(),
        client.clone(),
        ["audio/mpeg", "audio/mp4", "audio/aac", "audio/flac", "audio/ogg", "audio/webm", "audio/x-matroska"].map(String::from).to_vec(),
    ));
    let libraries = source.libraries().await.unwrap_or_else(|_| panic!("library read failed"));
    let mut chosen = None;
    for library in libraries.iter().filter(|l| l.kind == sources::LibraryKind::Books) {
        let listing = source.items(&library.id, None).await.unwrap_or_else(|_| panic!("book listing failed"));
        if let Some(book) = listing.items.into_iter().find(|b| b.duration.is_some_and(|d| d > 120.0)) {
            chosen = Some(book.id);
            break;
        }
    }
    let id = chosen.expect("a book longer than two minutes");
    let dir = tempfile::tempdir().unwrap();
    let shelf = Arc::new(Shelf::open(dir.path().join("shelf.redb")).unwrap());
    let cache = streamcache::StreamCache::new(streamcache::CacheConfig::new(dir.path().join("streams")), tokio::runtime::Handle::current()).unwrap();
    let engine = audio::spawn(audio::EngineConfig::default(), audio::default_backend(), Some(cache));
    let (tx, mut events) = mpsc::unbounded_channel();
    let listening = spawn(
        &tokio::runtime::Handle::current(),
        source.clone(),
        shelf.clone(),
        DeviceInfo::default(),
        || true,
        move |event| {
            let _ = tx.send(event);
        },
    );
    let key = ItemKey::new(source.account(), &id);
    let outcome: Check = async {
        listening.send(Request::Open { generation: 1, key: key.clone(), at: Some(0.0) });
        let opened = receive(&mut events).await?;
        let mut player = Player::default();
        player.load(*opened, 1.0, &engine, Instant::now())?;
        let deadline = Instant::now() + Duration::from_secs(75);
        let mut heard = Duration::ZERO;
        let mut last = Instant::now();
        while heard < Duration::from_secs(20) {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let now = Instant::now();
            if now > deadline {
                return Err("twenty seconds of playback did not complete");
            }
            let mut notes = Vec::new();
            for event in engine.events.try_iter() {
                player.event(&event, &engine, now, &mut notes);
            }
            if engine.status.state() == audio::PlayState::Playing {
                heard += now - last;
            }
            last = now;
            player.tick(&engine, now, &mut notes);
            report(notes, &listening)?;
            while let Ok(event) = events.try_recv() {
                match event {
                    Event::Error(_) => return Err("session sync failed"),
                    Event::Refreshed { result: Ok(tracks), .. } => player.refreshed(tracks, &engine),
                    Event::Refreshed { result: Err(_), .. } => return Err("file token refresh failed"),
                    _ => {}
                }
            }
        }
        if player.position < 18.0 {
            return Err("source position did not advance");
        }
        let mut notes = Vec::new();
        player.control(Control::Stop, 0.0, &engine, Instant::now(), &mut notes);
        report(notes, &listening)?;
        Ok(())
    }
    .await;
    engine.shutdown(Duration::from_secs(2));
    let ended = listening.finish().await;
    let progress = client.progress().await.unwrap_or_else(|_| panic!("progress read before cleanup failed"));
    let moved = progress.iter().any(|p| p.library_item_id.as_deref() == Some(&id) && p.current_time.is_some_and(|at| at >= 18.0));
    let pending = shelf.pending_progress().unwrap().len();
    let mut cleaned = true;
    for row in &progress {
        if row.library_item_id.as_deref() == Some(&id) {
            cleaned &= client.delete_progress(&row.id).await.is_ok();
        }
    }
    let empty = client.progress().await.is_ok_and(|p| p.is_empty());
    let _ = client.logout().await;
    assert!(cleaned && empty, "server progress cleanup failed");
    println!("Server progress deleted; account has no progress.");
    assert!(ended.is_ok(), "session worker stopped before closing");
    assert!(outcome.is_ok(), "{}", outcome.unwrap_err());
    assert!(moved, "server progress did not move");
    assert_eq!(pending, 0, "all listening reports reached the server");
    println!("Played 20 seconds through the real engine; synced and closed successfully.");
}
