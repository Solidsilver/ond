use super::*;

use abs::Tokens;
use std::sync::Arc;

fn account() -> AccountId {
    AccountId::from_uuid(uuid::Uuid::from_u128(0x1234))
}

fn client() -> abs::Client {
    let client = abs::Client::new("https://abs.example.com/", abs::DeviceInfo::new("device-1", "0.1.0")).unwrap();
    client.with_tokens(Tokens::new("access-1", "refresh-1"))
}

/// The minified item of `docs/rede/abs-api.md` §7, as a listing sends it.
fn listed_book() -> abs::LibraryItem {
    serde_json::from_str(
        r#"{
        "id": "li_1", "ino": "12", "libraryId": "lib_books", "folderId": "fol_1",
        "path": "/abs/books/Dune", "relPath": "Dune", "isFile": false,
        "mtimeMs": 1766000000000, "ctimeMs": 1766000000000, "birthtimeMs": 1766000000000,
        "addedAt": 1765000000000, "updatedAt": 1766000000000,
        "isMissing": false, "isInvalid": false, "mediaType": "book",
        "numFiles": 3, "size": 900000000,
        "media": {
            "id": "li_1", "coverPath": "/abs/metadata/li_1/cover.jpg", "tags": ["favourite"],
            "numTracks": 3, "numAudioFiles": 3, "numChapters": 40, "duration": 75600.5, "size": 900000000,
            "metadata": {
                "title": "Dune", "titleIgnorePrefix": "Dune", "subtitle": "Book One",
                "authorName": "Herbert, Frank", "authorNameLF": "Herbert, Frank",
                "narratorName": "Scott Brick, Simon Vance",
                "seriesName": "Dune #1, Dune Chronicles #2.5",
                "genres": ["Science Fiction"], "publishedYear": "1965", "publishedDate": "1965-08-01",
                "publisher": "Chilton", "description": "A desert planet.", "isbn": "9780441013593",
                "asin": "B0001", "language": "English", "explicit": false, "abridged": false
            }
        }
    }"#,
    )
    .unwrap()
}

/// The expanded item of `docs/rede/abs-api.md` §8.
fn expanded_book() -> abs::LibraryItem {
    serde_json::from_str(
        r#"{
        "id": "li_1", "ino": "12", "libraryId": "lib_books", "isFile": false,
        "addedAt": 1765000000000, "updatedAt": 1767000000000, "mediaType": "book",
        "media": {
            "id": "li_1", "coverPath": "/abs/metadata/li_1/cover.jpg", "tags": ["favourite", "re-read"],
            "numTracks": 2, "numAudioFiles": 2, "numChapters": 2, "duration": 75600.5, "size": 900000000,
            "metadata": {
                "title": "Dune", "subtitle": "Book One",
                "authors": [{"id": "aut_1", "name": "Frank Herbert"}],
                "narrators": ["Scott Brick", "Simon Vance"],
                "series": [{"id": "ser_1", "name": "Dune", "sequence": "1"}],
                "genres": ["Science Fiction"], "publishedYear": "1965", "publisher": "Chilton",
                "description": "A desert planet.", "isbn": "9780441013593", "asin": "B0001", "language": "English"
            },
            "audioFiles": [],
            "tracks": [
                {"index": 1, "ino": "101", "startOffset": 0, "duration": 3600.5,
                 "contentUrl": "/api/items/li_1/file/101", "mimeType": "audio/mp4", "codec": "aac",
                 "title": "Dune - 01.m4b",
                 "metadata": {"filename": "Dune - 01.m4b", "ext": ".m4b", "path": "/abs/books/Dune/01.m4b", "relPath": "01.m4b", "size": 500000000, "mtimeMs": 1766000000000}},
                {"index": 2, "ino": "102", "startOffset": 3600.5, "duration": 72000,
                 "contentUrl": "/api/items/li_1/file/102", "mimeType": "audio/mpeg",
                 "title": "Dune - 02.m4b",
                 "metadata": {"filename": "Dune - 02.mp3", "ext": ".mp3", "path": "/abs/books/Dune/02.mp3", "relPath": "02.mp3", "size": 400000000, "mtimeMs": 1766000000000}}
            ],
            "chapters": [
                {"id": 1, "start": 0, "end": 3600, "title": "Chapter 1"},
                {"id": 2, "start": 3600, "end": 7200, "title": "Chapter 2"}
            ]
        }
    }"#,
    )
    .unwrap()
}

fn podcast() -> abs::LibraryItem {
    serde_json::from_str(
        r#"{"id": "pod_1", "libraryId": "lib_pods", "mediaType": "podcast", "addedAt": 1, "updatedAt": 2,
            "media": {"id": "pod_1", "metadata": {"title": "A Podcast"}}}"#,
    )
    .unwrap()
}

fn abs_library(name: &str, media_type: &str) -> abs::Library {
    serde_json::from_value(serde_json::json!({"id": format!("lib_{name}"), "name": name, "mediaType": media_type})).unwrap()
}

#[test]
fn a_listed_book_becomes_a_summary_from_the_minified_fields() {
    let summary = summary(&listed_book(), "lib_fallback").unwrap();
    assert_eq!(summary.id, "li_1");
    assert_eq!(summary.library, "lib_books");
    assert_eq!(summary.title, "Dune");
    assert_eq!(summary.subtitle.as_deref(), Some("Book One"));
    // The joined forms are split as far as they can be.
    assert_eq!(summary.authors, ["Herbert", "Frank"]);
    assert_eq!(summary.narrators, ["Scott Brick", "Simon Vance"]);
    assert_eq!(summary.duration, Some(75600.5));
    // ABS serves a cover under the item's id.
    assert_eq!(summary.cover.as_deref(), Some("li_1"));
    assert_eq!(summary.added, Some(Stamp::from_millis(1765000000000)));
    assert_eq!(summary.updated, Some(Stamp::from_millis(1766000000000)));
}

#[test]
fn a_minified_series_string_becomes_one_entry_per_series() {
    let metadata = listed_book().media.unwrap().metadata.unwrap();
    assert_eq!(
        series(Some(&metadata)),
        [Series { name: "Dune".into(), sequence: Some("1".into()) }, Series { name: "Dune Chronicles".into(), sequence: Some("2.5".into()) },]
    );
    // Expanded metadata carries them apart and wins.
    let metadata = expanded_book().media.unwrap().metadata.unwrap();
    assert_eq!(series(Some(&metadata)), [Series { name: "Dune".into(), sequence: Some("1".into()) }]);
    // A series without a sequence, and none at all.
    assert_eq!(split_joined(Some("The Expanse")), ["The Expanse"]);
    assert!(split_joined(None).is_empty());
}

#[test]
fn an_expanded_item_becomes_tracks_and_chapters_on_the_book_timeline() {
    let item = details(&expanded_book()).unwrap();
    assert_eq!(item.summary.title, "Dune");
    assert_eq!(item.summary.authors, ["Frank Herbert"], "the expanded names, not the joined one");
    assert_eq!(item.description.as_deref(), Some("A desert planet."));
    assert_eq!(item.tags, ["favourite", "re-read"]);
    assert_eq!(item.published_year.as_deref(), Some("1965"));
    assert_eq!(item.isbn.as_deref(), Some("9780441013593"));
    assert_eq!(item.asin.as_deref(), Some("B0001"));
    assert_eq!(item.language.as_deref(), Some("English"));

    assert_eq!(
        item.tracks,
        [
            TrackFile {
                index: 1,
                start_offset: 0.0,
                duration: Some(3600.5),
                suffix: Some("m4b".into()),
                size: Some(500000000),
                mime: Some("audio/mp4".into()),
                content_id: "101".into(),
            },
            TrackFile {
                index: 2,
                start_offset: 3600.5,
                duration: Some(72000.0),
                suffix: Some("mp3".into()),
                size: Some(400000000),
                mime: Some("audio/mpeg".into()),
                content_id: "102".into(),
            },
        ]
    );
    // Book time, not file time: the second chapter starts at 3600 and the
    // second file at 3600.5.
    assert_eq!(
        item.chapters,
        [Chapter { start: 0.0, end: 3600.0, title: "Chapter 1".into() }, Chapter { start: 3600.0, end: 7200.0, title: "Chapter 2".into() }]
    );
}

#[test]
fn podcast_items_are_left_out() {
    assert!(summary(&podcast(), "lib_pods").is_none());
    assert!(details(&podcast()).is_none());
}

#[test]
fn libraries_are_typed_and_a_kind_this_build_doesnt_know_is_left_out() {
    assert_eq!(library(abs_library("Books", "book")), Some(Library { id: "lib_Books".into(), name: "Books".into(), kind: LibraryKind::Books }));
    assert_eq!(library(abs_library("Pods", "podcast")), Some(Library { id: "lib_Pods".into(), name: "Pods".into(), kind: LibraryKind::Podcasts }));
    // A media type newer than this build ("ebook") reads as Other.
    assert_eq!(library(abs_library("Papers", "ebook")), None);
}

#[test]
fn a_play_session_maps_its_tracks_to_urls_with_the_token() {
    let played: abs::PlaybackSession = serde_json::from_str(
        r#"{
        "id": "sess_1", "libraryItemId": "li_1", "mediaType": "book", "playMethod": 0,
        "duration": 75600.5, "currentTime": 3725.5, "timeListening": 0,
        "startedAt": 1767000000000, "updatedAt": 1767000000000,
        "audioTracks": [
            {"index": 1, "ino": "101", "startOffset": 0, "duration": 3600.5,
             "contentUrl": "/api/items/li_1/file/101", "mimeType": "audio/mp4",
             "metadata": {"size": 500000000}},
            {"index": 2, "ino": "102", "startOffset": 3600.5, "duration": 72000,
             "contentUrl": "/api/items/li_1/file/102", "mimeType": "audio/mpeg"}
        ]
    }"#,
    )
    .unwrap();
    let client = client();
    let session = session("li_1", played, &client).unwrap();
    assert_eq!(session.id, "sess_1");
    assert_eq!(session.item, "li_1");
    assert_eq!(session.start, Position::from_secs(3725.5));
    assert_eq!(session.duration, Some(75600.5));
    assert_eq!(session.tracks.len(), 2);
    assert_eq!(session.tracks[1].start_offset, 3600.5);
    assert_eq!(session.tracks[0].size, Some(500000000));
    assert_eq!(session.tracks[0].url, "https://abs.example.com/api/items/li_1/file/101?token=access-1");
}

#[test]
fn a_session_track_without_a_content_url_is_a_malformed_reply() {
    let played: abs::PlaybackSession = serde_json::from_str(r#"{"id": "s", "audioTracks": [{"index": 1, "startOffset": 0}]}"#).unwrap();
    assert!(matches!(session("li_1", played, &client()), Err(Error::Decode(_))));
}

#[test]
fn progress_rows_are_books_keyed_by_the_account() {
    let row: abs::MediaProgress = serde_json::from_str(
        r#"{"id": "prog_1", "libraryItemId": "li_1", "mediaItemId": "li_1", "mediaItemType": "book",
            "duration": 75600.5, "progress": 0.05, "currentTime": 3725.5, "isFinished": false,
            "lastUpdate": 1767000000000, "startedAt": 1766000000000}"#,
    )
    .unwrap();
    let found = progress(&account(), &row).unwrap();
    assert_eq!(found.item, ItemKey::new(account(), "li_1"));
    assert_eq!(found.position, Position::from_secs(3725.5));
    assert!(!found.finished);
    assert_eq!(found.at, Some(Stamp::from_millis(1767000000000).system_time()));

    // A podcast episode's row belongs to a podcast, which this build doesn't
    // show.
    let episode: abs::MediaProgress = serde_json::from_str(
        r#"{"id": "prog_2", "libraryItemId": "pod_1", "episodeId": "ep_1", "mediaItemType": "podcastEpisode",
            "currentTime": 5}"#,
    )
    .unwrap();
    assert!(progress(&account(), &episode).is_none());
}

#[test]
fn a_progress_update_reports_a_position_and_a_finish_but_never_an_unfinish() {
    let update = ProgressUpdate {
        item: ItemKey::new(account(), "li_1"),
        position: Position::from_secs(3725.5),
        finished: false,
        at: Stamp::from_millis(1767000000000).system_time(),
        listened: Duration::from_secs(300),
    };
    let sent = serde_json::to_value(progress_update(&update)).unwrap();
    assert_eq!(sent["libraryItemId"], "li_1");
    assert_eq!(sent["currentTime"], 3725.5);
    assert_eq!(sent["lastUpdate"], 1_767_000_000_000i64);
    // Not sent at all: the server resets a finished item's position when it
    // reads `isFinished: false`.
    assert!(sent.get("isFinished").is_none());

    let finished = progress_update(&ProgressUpdate { finished: true, ..update });
    let sent = serde_json::to_value(finished).unwrap();
    assert_eq!(sent["isFinished"], true);
    assert_eq!(sent["libraryItemId"], "li_1");
}

#[test]
fn the_source_is_one_trait_object_of_any_kind() {
    let source = AbsSource::new(account(), client(), vec!["audio/mp4".into(), "audio/mpeg".into()]);
    let capabilities = source.capabilities();
    assert!(capabilities.transcoding);
    assert!(!capabilities.live_updates);
    let source: Arc<dyn Source> = Arc::new(source);
    assert_eq!(source.capabilities(), capabilities);
}

#[tokio::test]
async fn an_expired_file_token_is_rotated_and_every_url_is_rebuilt() {
    use std::io::{BufRead, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/books/", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut reader = std::io::BufReader::new(&stream);
        let mut first = String::new();
        reader.read_line(&mut first).unwrap();
        assert!(first.starts_with("POST /books/auth/refresh "));
        let mut length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some((_, value)) = line.to_ascii_lowercase().split_once("content-length:") {
                length = value.trim().parse::<usize>().unwrap();
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let body = r#"{"user":{"id":"user","accessToken":"access-2","refreshToken":"refresh-2"}}"#;
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    let client = abs::Client::new(&base, abs::DeviceInfo::new("device", "1")).unwrap().with_tokens(Tokens::new("access-1", "refresh-1"));
    let source = AbsSource::new(account(), client, vec![]);
    let session = PlaySession {
        id: "s".into(),
        item: "book".into(),
        start: Position::default(),
        duration: Some(100.0),
        tracks: (1..=2)
            .map(|index| SessionTrack {
                index,
                url: source.client.file_url(&format!("/api/items/book/file/{index}")).unwrap().to_string(),
                ..Default::default()
            })
            .collect(),
    };
    let tracks = source.refresh_tracks(&session).await.unwrap();
    server.join().unwrap();
    for (i, track) in tracks.iter().enumerate() {
        assert_eq!(track.url, format!("{base}api/items/book/file/{}?token=access-2", i + 1));
    }
    // A report can refresh before the decoder's rejection arrives. Reusing
    // that rotation must not spend another login-limiter request.
    let again = source.refresh_tracks(&session).await.unwrap();
    assert_eq!(again, tracks);
}
