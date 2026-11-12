//! Against a real Audiobookshelf server, when one is configured:
//!
//! ```sh
//! ABS_TEST_URL=... ABS_TEST_USER=... ABS_TEST_PASSWORD=... cargo test -p abs -- --ignored
//! ```
//!
//! Sign-ins and refreshes share a budget of 40 per 10 minutes per address, so
//! this is one test that signs in once and refreshes once. It opens a play
//! session and writes progress and a bookmark on the first book the account
//! has no progress in, and puts back what it changed, whether or not the
//! checks pass. Nothing here prints the address, the account or a token.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use abs::{AuthEvent, Client, DeviceInfo, ItemProgressUpdate, LibraryItem, MediaType, ProgressUpdate, SessionSync, Version};

type Outcome<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn env() -> Option<(String, String, String)> {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    Some((get("ABS_TEST_URL")?, get("ABS_TEST_USER")?, get("ABS_TEST_PASSWORD")?))
}

fn mimes() -> Vec<String> {
    ["audio/mpeg", "audio/mp4", "audio/aac", "audio/flac", "audio/ogg", "audio/webm", "audio/x-matroska"].map(String::from).to_vec()
}

#[tokio::test]
#[ignore = "needs a real server: set ABS_TEST_URL, ABS_TEST_USER and ABS_TEST_PASSWORD"]
async fn a_real_server_round_trip() {
    let Some((url, user, password)) = env() else {
        eprintln!("skipping: ABS_TEST_URL, ABS_TEST_USER and ABS_TEST_PASSWORD aren't all set");
        return;
    };
    let client = Client::new(&url, DeviceInfo::new("rede-abs-integration-test", env!("CARGO_PKG_VERSION")).with_model("abs tests")).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let log = events.clone();
    client.set_auth_hook(move |e| log.lock().unwrap().push(e));

    let status = client.status().await.unwrap();
    let version = status.version().expect("the server says its version");
    println!("server {version}, sign-in methods {:?}", status.auth_methods);
    assert!(version >= Version::TOKENS);

    let login = client.login(&user, &password).await.unwrap();
    assert!(!login.tokens.access().is_empty() && !login.tokens.refresh().is_empty());
    assert_eq!(events.lock().unwrap().len(), 1);

    let me = client.me().await.unwrap();
    let progress_before: BTreeSet<_> = me.media_progress.iter().map(|p| (p.id.clone(), p.last_update)).collect();
    let bookmarks_before: BTreeSet<_> = me.bookmarks.iter().map(|b| (b.library_item_id.clone(), b.time.to_bits())).collect();
    assert!(progress_before.is_empty() && bookmarks_before.is_empty(), "the dedicated test account must start empty");

    let books = survey(&client).await.unwrap();

    // Writes, then putting things back however they went.
    let started: BTreeSet<_> = me.media_progress.iter().filter_map(|p| p.library_item_id.clone()).collect();
    let target = books.iter().find(|b| b.user_media_progress.is_none() && !started.contains(&b.id));
    let outcome = match target {
        Some(book) => {
            // Join even an assertion panic before cleaning up the account.
            let client = client.clone();
            let book = book.clone();
            match tokio::spawn(async move { exercise(&client, &book).await }).await {
                Ok(outcome) => outcome,
                Err(_) => Err("a live assertion failed; cleanup still runs".into()),
            }
        }
        None => {
            println!("no book without progress; skipping the writes");
            Ok(())
        }
    };
    let restored = restore(&client, &progress_before, &bookmarks_before).await;

    // One refresh: rotated, announced, and the new access token works.
    let before = client.tokens().unwrap();
    let after = client.refresh().await.unwrap();
    assert_ne!(before.refresh(), after.refresh());
    assert_ne!(before.access(), after.access());
    assert_eq!(events.lock().unwrap().last(), Some(&AuthEvent::Tokens(after)));
    client.libraries().await.unwrap();

    client.logout().await.unwrap();
    assert!(!client.is_signed_in());
    assert_eq!(events.lock().unwrap().last(), Some(&AuthEvent::SignedOut));

    outcome.unwrap();
    restored.unwrap();
}

/// Reads every library, pages through each, and expands some books. Returns
/// the expanded books.
async fn survey(client: &Client) -> Outcome<Vec<LibraryItem>> {
    let libraries = client.libraries().await?;
    println!("{} libraries", libraries.len());
    let mut expanded = Vec::new();
    for lib in &libraries {
        let kind = lib.media_type.unwrap_or(MediaType::Other);
        let first = client.library_items(&lib.id, 0, 20).await?;
        assert_eq!((first.limit, first.page), (Some(20), Some(0)));
        let total = first.total.unwrap_or(0);
        // Page through the rest, to see the paging hold together.
        let mut ids: BTreeSet<String> = first.results.iter().map(|i| i.id.clone()).collect();
        let several = |items: &[LibraryItem]| items.iter().filter(|i| i.media.as_ref().and_then(|m| m.num_tracks).is_some_and(|n| n > 1)).count();
        let mut listed_multi_file = several(&first.results);
        let mut page = 1;
        while (ids.len() as u64) < total && page < 50 {
            let next = client.library_items(&lib.id, page, 20).await?;
            assert_eq!(next.page, Some(page));
            if next.results.is_empty() {
                break;
            }
            listed_multi_file += several(&next.results);
            ids.extend(next.results.into_iter().map(|i| i.id));
            page += 1;
        }
        assert_eq!(ids.len() as u64, total, "paging by addedAt saw every item once");
        let (mut multi_file, mut with_chapters, mut tracks, mut chapters, mut mime) = (0, 0, 0, 0, BTreeSet::new());
        let sample: Vec<_> = ids.iter().take(25).cloned().collect();
        for id in &sample {
            let item = client.item(id).await?;
            if let Some(m) = &item.media {
                multi_file += usize::from(m.tracks.len() > 1);
                with_chapters += usize::from(!m.chapters.is_empty());
                tracks += m.tracks.len();
                chapters += m.chapters.len();
                mime.extend(m.tracks.iter().filter_map(|t| t.mime_type.clone()));
                for pair in m.tracks.windows(2) {
                    assert!(pair[0].start_offset < pair[1].start_offset, "tracks start in order");
                }
                assert!(m.tracks.iter().all(|t| t.content_url.as_deref().is_some_and(|u| u.starts_with("/api/items/"))));
            }
            if kind == MediaType::Book {
                expanded.push(item);
            }
        }
        println!(
            "library ({kind:?}): {total} items, {listed_multi_file} with several tracks; of {} expanded, {multi_file} have several files, {with_chapters} have chapters; {tracks} tracks, {chapters} chapters; types {mime:?}",
            sample.len()
        );
    }
    Ok(expanded)
}

/// Plays, syncs and closes a session, writes progress three ways and a
/// bookmark, on a book the account has no progress in.
async fn exercise(client: &Client, book: &LibraryItem) -> Outcome {
    let session = client.play(&book.id, None, &mimes()).await?;
    assert_eq!(session.play_method(), Some(abs::PlayMethod::DirectPlay));
    assert!(!session.audio_tracks.is_empty());
    let track = &session.audio_tracks[0];

    // The file, through the URL the stream cache would get.
    let url = client.file_url(track.content_url.as_deref().unwrap_or_default())?;
    let resp = reqwest::Client::new().get(url).header("range", "bytes=0-15").send().await.map_err(abs::Error::from)?;
    let ct = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
    println!("file: {} {ct}", resp.status().as_u16());
    assert!(resp.status().is_success() && ct.starts_with("audio/"), "a file fetch with ?token= gets audio");
    let cover = reqwest::get(client.cover_url(&book.id, Some(200))).await.map_err(abs::Error::from)?.status();
    println!("cover: {}", cover.as_u16());
    assert!(cover.is_success() || cover.as_u16() == 404);

    client.sync_session(&session.id, &SessionSync { current_time: 30.0, time_listened: 0.0, duration: None }).await?;
    client.close_session(&session.id, None).await?;
    let synced = client.item_progress(&book.id, None).await?.ok_or("a session sync writes progress")?;
    assert_eq!(synced.current_time, Some(30.0));
    assert!(client.sync_session(&session.id, &SessionSync { current_time: 31.0, time_listened: 0.0, duration: None }).await.unwrap_err().is_not_found());

    client.update_progress(&book.id, None, &ProgressUpdate { current_time: Some(42.0), ..Default::default() }).await?;
    assert_eq!(client.item_progress(&book.id, None).await?.and_then(|p| p.current_time), Some(42.0));
    let batch =
        ItemProgressUpdate { library_item_id: book.id.clone(), episode_id: None, update: ProgressUpdate { current_time: Some(50.0), ..Default::default() } };
    client.update_progress_batch(&[batch]).await?;
    assert_eq!(client.item_progress(&book.id, None).await?.and_then(|p| p.current_time), Some(50.0));
    settle(client, book, finish_stamp()).await?;
    assert!(client.items_in_progress(25).await?.iter().any(|i| i.id == book.id));
    assert!(client.progress().await?.iter().any(|p| p.id == synced.id));
    client.delete_progress(&synced.id).await?;
    assert_eq!(client.item_progress(&book.id, None).await?, None);

    let time = 12.345_678_901_234_567;
    let made = client.create_bookmark(&book.id, time, "abs test").await?;
    assert_eq!(made.time, time);
    let updated = client.create_bookmark(&book.id, made.time, "abs test, same time").await?;
    assert_eq!(updated.title.as_deref(), Some("abs test, same time"));
    assert_eq!(client.bookmarks().await?.iter().filter(|b| b.library_item_id == book.id && b.time == made.time).count(), 1);
    let renamed = client.rename_bookmark(&book.id, made.time, "abs test, renamed").await?;
    assert_eq!(renamed.title.as_deref(), Some("abs test, renamed"));
    assert!(client.bookmarks().await?.iter().any(|b| b.library_item_id == book.id && b.time == time));
    client.delete_bookmark(&book.id, made.time).await?;
    assert!(!client.bookmarks().await?.iter().any(|b| b.library_item_id == book.id && b.time == time));
    Ok(())
}

/// Milliseconds since the epoch, as the server stores them.
fn finish_stamp() -> i64 {
    i64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()).unwrap()
}

/// The three ways Rede settles a book by hand, and the server rules they rest
/// on (abs-api.md §12): a finish is stored with the client's stamp, clearing
/// one resets the place and cannot carry one with it, and the place written
/// after the reset is the one the book keeps. A place within ten seconds of
/// the end finishes the book again.
async fn settle(client: &Client, book: &LibraryItem, stamp: i64) -> Outcome {
    let finish = ItemProgressUpdate {
        library_item_id: book.id.clone(),
        episode_id: None,
        update: ProgressUpdate { current_time: Some(50.0), is_finished: Some(true), last_update: Some(stamp), ..Default::default() },
    };
    client.update_progress_batch(&[finish]).await?;
    let finished = client.item_progress(&book.id, None).await?.ok_or("a finish leaves the row")?;
    assert_eq!(finished.is_finished, Some(true), "the finish is stored");
    assert_eq!(finished.last_update, Some(stamp), "the client's stamp is kept");

    let reset = ProgressUpdate { is_finished: Some(false), last_update: Some(stamp - 1), ..Default::default() };
    client.update_progress(&book.id, None, &reset).await?;
    let cleared = client.item_progress(&book.id, None).await?.ok_or("clearing a finish leaves the row")?;
    assert_eq!(cleared.is_finished, Some(false));
    assert_eq!(cleared.current_time, Some(0.0), "clearing a finish resets the place, so the place goes in a second write");

    let place = ProgressUpdate { current_time: Some(50.0), last_update: Some(stamp), ..Default::default() };
    client.update_progress(&book.id, None, &place).await?;
    let restored = client.item_progress(&book.id, None).await?.ok_or("the row is still there")?;
    assert_eq!(restored.current_time, Some(50.0), "the place written after the reset is kept");
    assert_eq!(restored.is_finished, Some(false));

    if let Some(duration) = book.media.as_ref().and_then(|m| m.duration).filter(|d| *d > 15.0) {
        let near_end = ProgressUpdate {
            current_time: Some(duration - 5.0),
            duration: Some(duration),
            is_finished: Some(false),
            last_update: Some(stamp + 1),
            ..Default::default()
        };
        client.update_progress(&book.id, None, &near_end).await?;
        let row = client.item_progress(&book.id, None).await?.ok_or("the row is still there")?;
        println!("a place within ten seconds of the end reads finished: {:?}", row.is_finished);
        assert_eq!(row.is_finished, Some(true), "ABS finishes a place inside its end threshold");
    }
    Ok(())
}

/// Deletes any progress row and bookmark that wasn't there at the start.
async fn restore(client: &Client, progress: &BTreeSet<(String, Option<i64>)>, bookmarks: &BTreeSet<(String, u64)>) -> Outcome {
    let me = client.me().await?;
    let known: BTreeSet<_> = progress.iter().map(|(id, _)| id.clone()).collect();
    for p in me.media_progress.iter().filter(|p| !known.contains(&p.id)) {
        client.delete_progress(&p.id).await?;
    }
    for b in me.bookmarks.iter().filter(|b| !bookmarks.contains(&(b.library_item_id.clone(), b.time.to_bits()))) {
        client.delete_bookmark(&b.library_item_id, b.time).await?;
    }
    let me = client.me().await?;
    let now: BTreeSet<_> = me.media_progress.iter().map(|p| (p.id.clone(), p.last_update)).collect();
    assert_eq!(&now, progress, "progress is as it was");
    assert_eq!(me.bookmarks.len(), bookmarks.len(), "bookmarks are as they were");
    Ok(())
}
