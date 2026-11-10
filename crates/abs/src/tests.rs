use super::*;

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use testkit::{Answer, JsonFake, Seen};

/// A client of the fake server.
fn client_of(fake: &JsonFake) -> Client {
    Client::new(fake.base(), device()).unwrap()
}

fn device() -> DeviceInfo {
    DeviceInfo::new("device-1", "0.1.0").with_model("macOS")
}

/// The sign-in and refresh reply, with the legacy token the client must
/// ignore.
fn login_reply(access: &str, refresh: &str) -> String {
    serde_json::json!({
        "user": {
            "id": "u1", "username": "joe", "type": "user", "token": "LEGACY-TOKEN",
            "mediaProgress": [], "bookmarks": [], "librariesAccessible": [],
            "permissions": {"download": true},
            "accessToken": access, "refreshToken": refresh
        },
        "userDefaultLibraryId": "lib1",
        "serverSettings": {"version": "2.37.2", "tokenSecret": null},
        "Source": "docker"
    })
    .to_string()
}

fn bearer(s: &Seen) -> Option<&str> {
    s.header("authorization").and_then(|v| v.strip_prefix("Bearer "))
}

/// Records every auth event the client announces.
fn recorded(client: &Client) -> Arc<Mutex<Vec<AuthEvent>>> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let log = events.clone();
    client.set_auth_hook(move |e| log.lock().unwrap().push(e));
    events
}

#[tokio::test]
async fn login_returns_the_tokens_and_keeps_them() {
    let fake = JsonFake::start(|s| match s.path() {
        "/login" if s.header("x-return-tokens") == Some("true") => Answer::ok(login_reply("access-1", "refresh-1")),
        "/login" => Answer::ok(login_reply("access-1", "")),
        "/api/me" if bearer(s) == Some("access-1") => Answer::ok(r#"{"id":"u1","username":"joe"}"#),
        _ => Answer::new("401 Unauthorized", "Unauthorized"),
    });
    let client = client_of(&fake);
    let events = recorded(&client);
    let login = client.login("joe", "sesame").await.unwrap();
    assert_eq!((login.tokens.access(), login.tokens.refresh()), ("access-1", "refresh-1"));
    assert_eq!(login.user.username.as_deref(), Some("joe"));
    assert_eq!(login.default_library_id.as_deref(), Some("lib1"));
    assert_eq!(login.server_version.as_deref(), Some("2.37.2"));
    assert_eq!(*events.lock().unwrap(), vec![AuthEvent::Tokens(Tokens::new("access-1", "refresh-1"))]);
    let sent = &fake.seen()[0];
    assert_eq!((sent.method.as_str(), sent.json()["username"].as_str(), sent.json()["password"].as_str()), ("POST", Some("joe"), Some("sesame")));
    // The tokens are used from then on.
    assert_eq!(client.me().await.unwrap().id, "u1");
}

#[tokio::test]
async fn sign_in_failures_are_told_apart() {
    let fake = JsonFake::start(|s| match s.json()["username"].as_str() {
        Some("wrong") => Answer::new("401 Unauthorized", "Unauthorized"),
        Some("busy") => Answer {
            status: "429 Too Many Requests",
            headers: vec![("Retry-After", "60".into())],
            body: r#"{"error":"Too many authentication requests"}"#.into(),
        },
        _ => Answer::new("500 Internal Server Error", r#"{"error":"Database is locked"}"#),
    });
    let client = client_of(&fake);
    assert!(matches!(client.login("wrong", "pw").await, Err(Error::Auth(AuthError::BadCredentials))));
    match client.login("busy", "pw").await {
        Err(Error::Auth(AuthError::RateLimited { retry_after })) => assert_eq!(retry_after, Some(Duration::from_secs(60))),
        other => panic!("expected rate limited, got {other:?}"),
    }
    match client.login("joe", "pw").await {
        Err(Error::Server { status: 500, message }) => assert_eq!(message.as_deref(), Some("Database is locked")),
        other => panic!("expected a server error, got {other:?}"),
    }
    assert!(!client.is_signed_in());
}

/// A server that rotates: refresh token `refresh-n` buys `access-(n+1)` and
/// `refresh-(n+1)`, and only the current access token is accepted.
fn rotating_server(delay: Duration) -> JsonFake {
    let current = Arc::new(AtomicUsize::new(1));
    JsonFake::start(move |s| {
        let n = current.load(Ordering::SeqCst);
        match s.path() {
            "/auth/refresh" => {
                std::thread::sleep(delay);
                if s.header("x-refresh-token") != Some(&format!("refresh-{n}")) {
                    return Answer::new("401 Unauthorized", r#"{"error":"Invalid refresh token"}"#);
                }
                current.store(n + 1, Ordering::SeqCst);
                Answer::ok(login_reply(&format!("access-{}", n + 1), &format!("refresh-{}", n + 1)))
            }
            "/api/libraries" if bearer(s) == Some(&format!("access-{n}")) => Answer::ok(r#"{"libraries":[{"id":"lib1","name":"Books","mediaType":"book"}]}"#),
            _ => Answer::new("401 Unauthorized", "Unauthorized"),
        }
    })
}

#[tokio::test]
async fn refresh_rotates_the_refresh_token_every_time() {
    let fake = rotating_server(Duration::ZERO);
    let client = client_of(&fake).with_tokens(Tokens::new("access-1", "refresh-1"));
    let events = recorded(&client);
    let first = client.refresh().await.unwrap();
    assert_eq!((first.access(), first.refresh()), ("access-2", "refresh-2"));
    let second = client.refresh().await.unwrap();
    assert_eq!((second.access(), second.refresh()), ("access-3", "refresh-3"));
    let sent: Vec<_> = fake.seen().iter().map(|s| s.header("x-refresh-token").unwrap_or_default().to_owned()).collect();
    assert_eq!(sent, ["refresh-1", "refresh-2"]);
    assert_eq!(*events.lock().unwrap(), vec![AuthEvent::Tokens(first), AuthEvent::Tokens(second.clone())]);
    assert_eq!(client.tokens(), Some(second));
}

#[tokio::test]
async fn a_refused_access_token_refreshes_and_retries_once() {
    let fake = rotating_server(Duration::ZERO);
    // The stored access token is already stale.
    let client = client_of(&fake).with_tokens(Tokens::new("access-0", "refresh-1"));
    let events = recorded(&client);
    let libs = client.libraries().await.unwrap();
    assert_eq!((libs[0].id.as_str(), libs[0].media_type), ("lib1", Some(MediaType::Book)));
    let paths: Vec<_> = fake.seen().iter().map(|s| s.path().to_owned()).collect();
    assert_eq!(paths, ["/api/libraries", "/auth/refresh", "/api/libraries"]);
    assert_eq!(*events.lock().unwrap(), vec![AuthEvent::Tokens(Tokens::new("access-2", "refresh-2"))]);
}

#[tokio::test]
async fn a_second_401_after_a_refresh_is_not_retried_again() {
    let fake = JsonFake::start(|s| match s.path() {
        "/auth/refresh" => Answer::ok(login_reply("access-2", "refresh-2")),
        _ => Answer::new("401 Unauthorized", "Unauthorized"),
    });
    let client = client_of(&fake).with_tokens(Tokens::new("access-1", "refresh-1"));
    assert!(matches!(client.libraries().await, Err(Error::Server { status: 401, .. })));
    assert_eq!((fake.count("/api/libraries"), fake.count("/auth/refresh")), (2, 1));
    // The refreshed tokens are kept: the server took the refresh token.
    assert_eq!(client.tokens(), Some(Tokens::new("access-2", "refresh-2")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_401s_share_one_refresh() {
    // The refresh is slow, so every request is refused while it's in flight.
    let fake = rotating_server(Duration::from_millis(300));
    let client = client_of(&fake).with_tokens(Tokens::new("access-0", "refresh-1"));
    let events = recorded(&client);
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let c = client.clone();
        tasks.spawn(async move { c.libraries().await });
    }
    while let Some(done) = tasks.join_next().await {
        assert_eq!(done.unwrap().unwrap().len(), 1);
    }
    assert_eq!(fake.count("/auth/refresh"), 1);
    assert_eq!(fake.count("/api/libraries"), 16);
    assert_eq!(events.lock().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expired_refresh_token_signs_out_every_waiting_request() {
    let fake = JsonFake::start(|s| match s.path() {
        "/auth/refresh" => {
            std::thread::sleep(Duration::from_millis(300));
            Answer::new("401 Unauthorized", r#"{"error":"Refresh token expired"}"#)
        }
        _ => Answer::new("401 Unauthorized", "Unauthorized"),
    });
    let client = client_of(&fake).with_tokens(Tokens::new("access-1", "refresh-1"));
    let events = recorded(&client);
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let c = client.clone();
        tasks.spawn(async move { c.libraries().await });
    }
    while let Some(done) = tasks.join_next().await {
        let e = done.unwrap().unwrap_err();
        assert!(e.is_signed_out(), "{e:?}");
        assert_eq!(e.to_string(), "signed out; sign in again");
    }
    assert_eq!(fake.count("/auth/refresh"), 1);
    assert_eq!(*events.lock().unwrap(), vec![AuthEvent::SignedOut]);
    assert!(!client.is_signed_in());
    // Signed out: nothing more goes to the server.
    let before = fake.seen().len();
    assert!(client.libraries().await.unwrap_err().is_signed_out());
    assert!(client.file_url("/api/items/x/file/1").unwrap_err().is_signed_out());
    assert_eq!(fake.seen().len(), before);
}

#[tokio::test]
async fn a_refresh_that_cant_reach_the_server_keeps_the_tokens() {
    let client = Client::new("http://127.0.0.1:1/", device()).unwrap().with_tokens(Tokens::new("access-1", "refresh-1"));
    assert!(client.refresh().await.unwrap_err().is_network());
    assert_eq!(client.tokens(), Some(Tokens::new("access-1", "refresh-1")));
}

#[tokio::test]
async fn logout_forgets_the_tokens_and_tells_the_server() {
    let fake = JsonFake::start(|_| Answer::ok(r#"{"redirect_url":null}"#));
    let client = client_of(&fake).with_tokens(Tokens::new("access-1", "refresh-1"));
    let events = recorded(&client);
    client.logout().await.unwrap();
    assert!(!client.is_signed_in());
    assert_eq!(*events.lock().unwrap(), vec![AuthEvent::SignedOut]);
    let sent = &fake.seen()[0];
    assert_eq!((sent.path(), sent.header("x-refresh-token")), ("/logout", Some("refresh-1")));
}

#[tokio::test]
async fn paging_reads_limit_and_page_sent_back_as_strings() {
    let fake = JsonFake::start(|_| {
        Answer::ok(
            r#"{
          "results": [
            {"id":"li1","ino":"649644248522215260","libraryId":"lib1","mediaType":"book","addedAt":1730000000000,"updatedAt":1730000000001,
             "numFiles":3,"size":1000,
             "media":{"id":"b1","metadata":{"title":"Dune","authorName":"Frank Herbert","authorNameLF":"Herbert, Frank",
                      "series":{"id":"s1","name":"Dune","sequence":"1"},"publishedYear":1965,"genres":null},
                      "coverPath":null,"tags":[],"numTracks":3,"numAudioFiles":3,"numChapters":40,"duration":75600.5,"size":1000}},
            {"ino":"no id, so skipped"},
            {"id":"li2","ino":42,"media":{"metadata":{"title":null}}}
          ],
          "total": 2, "limit": "50", "page": "2", "sortBy": "addedAt", "sortDesc": false,
          "mediaType": "book", "minified": false, "collapseseries": false, "include": "", "offset": 100
        }"#,
        )
    });
    let client = client_of(&fake).with_tokens(Tokens::new("access-1", "refresh-1"));
    let page = client.library_items("lib 1", 2, 50).await.unwrap();
    assert_eq!((page.limit, page.page, page.total, page.offset), (Some(50), Some(2), Some(2), Some(100)));
    assert_eq!(page.results.len(), 2);
    let book = &page.results[0];
    let media = book.media.as_ref().unwrap();
    let meta = media.metadata.as_ref().unwrap();
    assert_eq!(meta.author_name_lf.as_deref(), Some("Herbert, Frank"));
    assert_eq!(meta.published_year.as_deref(), Some("1965"));
    assert_eq!(meta.series[0].sequence.as_deref(), Some("1"));
    assert_eq!((media.num_chapters, media.duration), (Some(40), Some(75600.5)));
    assert_eq!(page.results[1].ino.as_deref(), Some("42"));
    let sent = &fake.seen()[0];
    assert_eq!(sent.target, "/api/libraries/lib%201/items?sort=addedAt&limit=50&page=2");
    // Sent with no paging, the server echoes numbers instead.
    let unpaged: ItemsPage = serde_json::from_str(r#"{"results":[],"total":0,"limit":0,"page":0}"#).unwrap();
    assert_eq!((unpaged.limit, unpaged.page), (Some(0), Some(0)));
}

#[test]
fn file_urls_are_absolute_with_the_token_and_the_path_prefix() {
    let client = Client::new("https://example.com/abs", device()).unwrap().with_tokens(Tokens::new("access-1", "refresh-1"));
    let want = "https://example.com/abs/api/items/li1/file/123?token=access-1";
    assert_eq!(client.file_url("/api/items/li1/file/123").unwrap().as_str(), want);
    assert_eq!(client.file_url("api/items/li1/file/123").unwrap().as_str(), want);
    // A server whose base path matches the proxy's may send it already.
    assert_eq!(client.file_url("/abs/api/items/li1/file/123").unwrap().as_str(), want);
    // Only relative paths: the token mustn't go to another host.
    assert!(client.file_url("https://elsewhere.example/api/items/li1/file/123").is_err());
    // At the root, too.
    let root = Client::new("http://10.0.0.2:13378", device()).unwrap().with_tokens(Tokens::new("a b", "r"));
    assert_eq!(root.file_url("/api/items/li1/file/9").unwrap().as_str(), "http://10.0.0.2:13378/api/items/li1/file/9?token=a+b");
    // Covers need no token.
    assert_eq!(client.cover_url("li 1", Some(800)).as_str(), "https://example.com/abs/api/items/li%201/cover?width=800");
    assert_eq!(client.cover_url("li1", None).as_str(), "https://example.com/abs/api/items/li1/cover");
}

#[tokio::test]
async fn a_cover_is_fetched_without_a_token() {
    let fake = JsonFake::start(|s| match s.path() {
        "/api/items/li1/cover" if s.header("authorization").is_none() => Answer::ok("PNGDATA"),
        _ => Answer::new("404 Not Found", ""),
    });
    // Not signed in: a cover needs no token.
    let client = client_of(&fake);
    assert_eq!(client.cover("li1", Some(200)).await.unwrap(), b"PNGDATA");
    assert_eq!(fake.seen()[0].target, "/api/items/li1/cover?width=200");
}

#[test]
fn server_addresses() {
    let c = Client::new(" https://example.com/abs/?x=1#y ", device()).unwrap();
    assert_eq!(c.base().as_str(), "https://example.com/abs/");
    for bad in ["example.com", "example.com:13378", "ftp://example.com", "mailto:joe@example.com"] {
        assert!(matches!(Client::new(bad, device()), Err(Error::Url(_))), "{bad} accepted");
    }
}

#[tokio::test]
async fn credentials_never_show_in_debug_or_errors() {
    let tokens = Tokens::new("SECRET-ACCESS", "SECRET-REFRESH");
    let fake = JsonFake::start(|s| match s.path() {
        // A sign-in reply that doesn't decode, quoting the token where a
        // user should be: serde would put it in its message.
        "/login" => Answer::ok(r#"{"user":"SECRET-ACCESS"}"#),
        "/auth/refresh" => Answer::ok(r#"{"user":{"id":7,"accessToken":"SECRET-ACCESS"}}"#),
        _ => Answer::new("401 Unauthorized", "Unauthorized"),
    });
    let client = client_of(&fake).with_tokens(tokens.clone());
    let flow = client_of(&JsonFake::start(openid_authorized)).begin_openid().await.unwrap();
    let mut shown = vec![format!("{tokens:?}"), format!("{client:?}"), format!("{flow:?}")];
    let mismatch = flow.validate_callback("rede://oauth?state=SECRET-STATE&code=SECRET-CODE&error=SECRET").unwrap_err();
    shown.push(format!("{mismatch} / {mismatch:?}"));
    let e = client.login("joe", "SECRET-PASSWORD").await.unwrap_err();
    assert!(matches!(e, Error::Decode(_)), "{e:?}");
    shown.push(format!("{e} / {e:?}"));
    let e = client.refresh().await.unwrap_err();
    shown.push(format!("{e} / {e:?}"));
    // reqwest names the URL in its errors, and a file URL carries the token.
    let unreachable = Client::new("http://127.0.0.1:1/", device()).unwrap().with_tokens(tokens.clone());
    let url = unreachable.file_url("/api/items/li1/file/1").unwrap();
    let e = Error::from(reqwest::get(url).await.unwrap_err());
    shown.push(format!("{e} / {e:?}"));
    let e = unreachable.libraries().await.unwrap_err();
    assert!(e.is_network());
    shown.push(format!("{e} / {e:?}"));
    let login = Login { tokens, user: serde_json::from_str(r#"{"id":"u1","token":"SECRET-LEGACY"}"#).unwrap(), default_library_id: None, server_version: None };
    shown.push(format!("{login:?}"));
    for s in shown {
        for leak in ["SECRET", "token="] {
            assert!(!s.contains(leak), "{leak} leaked in {s}");
        }
    }
}

#[tokio::test]
async fn server_errors_keep_what_the_server_said() {
    let fake = JsonFake::start(|s| match s.path() {
        "/api/me/progress/li1" => Answer::new("404 Not Found", "Not Found"),
        "/api/items/li1" => Answer::new("403 Forbidden", "<html><head><title>Forbidden\n by policy</title></head></html>"),
        "/api/me" => Answer::ok("<!DOCTYPE html><html><head><title>Sign in to Authelia</title></head></html>"),
        _ => Answer::new("500 Internal Server Error", ""),
    });
    let client = client_of(&fake).with_tokens(Tokens::new("a", "r"));
    assert_eq!(client.item_progress("li1", None).await.unwrap(), None);
    match client.item("li1").await {
        Err(e @ Error::Server { status: 403, .. }) => assert_eq!(e.to_string(), "server error 403: Forbidden by policy"),
        other => panic!("expected a 403, got {other:?}"),
    }
    assert!(matches!(client.me().await, Err(Error::NotApi(Some(s))) if s == "Sign in to Authelia"));
    assert!(matches!(client.libraries().await, Err(Error::Server { status: 500, message: None })));
}

#[tokio::test]
async fn play_asks_for_direct_play_and_reads_the_session() {
    let fake = JsonFake::start(|_| {
        Answer::ok(
            r#"{
          "id":"ses1","libraryItemId":"li1","mediaType":"book","displayTitle":"Dune","duration":7200.25,"playMethod":0,
          "currentTime":120.5,"startedAt":1730000000000,"updatedAt":1730000000000,
          "chapters":[{"id":0,"start":0,"end":3600.1,"title":"One"},{"id":1,"start":3600.1,"end":7200.25,"title":"Two"}],
          "audioTracks":[
            {"index":1,"ino":"111","title":"01.mp3","startOffset":0,"duration":3600.1,"contentUrl":"/api/items/li1/file/111",
             "mimeType":"audio/mpeg","codec":"mp3","metadata":{"filename":"01.mp3","size":5000,"path":"/books/01.mp3"}},
            {"index":2,"ino":"222","title":"02.mp3","startOffset":3600.1,"duration":3600.15,"contentUrl":"/api/items/li1/file/222","mimeType":"audio/mpeg"}
          ],
          "deviceInfo":{"clientName":"Rede"}
        }"#,
        )
    });
    let client = client_of(&fake).with_tokens(Tokens::new("a", "r"));
    let mimes = vec!["audio/mpeg".to_owned(), "audio/mp4".to_owned()];
    let s = client.play("li1", None, &mimes).await.unwrap();
    assert_eq!((s.play_method(), s.current_time, s.chapters.len()), (Some(PlayMethod::DirectPlay), Some(120.5), 2));
    assert_eq!((s.audio_tracks[1].start_offset, s.audio_tracks[0].size()), (Some(3600.1), Some(5000)));
    let sent = fake.seen()[0].clone();
    assert_eq!(sent.path(), "/api/items/li1/play");
    let body = sent.json();
    assert_eq!(body["forceDirectPlay"], true);
    assert_eq!(body["supportedMimeTypes"], serde_json::json!(mimes));
    assert_eq!(body["deviceInfo"], serde_json::json!({"deviceId":"device-1","clientName":"Rede","clientVersion":"0.1.0","model":"macOS"}));
}

#[tokio::test]
async fn sessions_progress_and_bookmarks_send_what_the_server_reads() {
    let fake = JsonFake::start(|s| match s.path() {
        p if p.ends_with("/bookmark") => Answer::ok(format!(r#"{{"libraryItemId":"li1","title":"Here","createdAt":1,"time":{}}}"#, s.json()["time"])),
        "/api/session/local-all" => {
            Answer::ok(r#"{"results":[{"id":"loc1","success":true,"progressSynced":false},{"id":"loc2","success":false,"error":"Media item not found"}]}"#)
        }
        _ => Answer::ok(""),
    });
    let client = client_of(&fake).with_tokens(Tokens::new("a", "r"));
    client.sync_session("ses1", &SessionSync { current_time: 130.0, time_listened: 9.5, duration: None }).await.unwrap();
    client.close_session("ses1", None).await.unwrap();
    let local = LocalSession {
        id: "loc1".into(),
        library_item_id: "li1".into(),
        episode_id: None,
        library_id: None,
        display_title: None,
        display_author: None,
        duration: Some(7200.0),
        start_time: 0.0,
        current_time: 60.0,
        time_listening: 60.0,
        started_at: 1_730_000_000_000,
        updated_at: 1_730_000_060_000,
    };
    client.sync_local_session(&local).await.unwrap();
    let results = client.sync_local_sessions(std::slice::from_ref(&local)).await.unwrap();
    assert_eq!((results[0].progress_synced, results[1].error.as_deref()), (Some(false), Some("Media item not found")));
    client.update_progress("li1", None, &ProgressUpdate { current_time: Some(61.0), ..Default::default() }).await.unwrap();
    client
        .update_progress_batch(&[ItemProgressUpdate {
            library_item_id: "li1".into(),
            episode_id: None,
            update: ProgressUpdate { is_finished: Some(true), ..Default::default() },
        }])
        .await
        .unwrap();
    client.update_progress_batch(&[]).await.unwrap();
    let b = client.create_bookmark("li1", 12.5, "Here").await.unwrap();
    assert_eq!(b.time, 12.5);
    client.rename_bookmark("li1", 100.0, "There").await.unwrap();
    client.delete_bookmark("li1", 12.5).await.unwrap();
    client.delete_bookmark("li1", 100.0).await.unwrap();
    client.delete_progress("mp1").await.unwrap();

    let seen = fake.seen();
    let sent: Vec<_> = seen.iter().map(|s| (s.method.as_str(), s.path(), s.body.as_str())).collect();
    assert_eq!(sent[0], ("POST", "/api/session/ses1/sync", r#"{"currentTime":130.0,"timeListened":9.5}"#));
    assert_eq!(sent[1], ("POST", "/api/session/ses1/close", "{}"));
    let one = seen[2].json();
    assert_eq!((one["updatedAt"].as_i64(), one["playMethod"].as_i64(), one["mediaType"].as_str()), (Some(1_730_000_060_000), Some(3), Some("book")));
    assert_eq!(one["deviceInfo"]["clientName"], "Rede");
    let all = seen[3].json();
    assert_eq!((all["sessions"][0]["updatedAt"].as_i64(), all["deviceInfo"]["deviceId"].as_str()), (Some(1_730_000_060_000), Some("device-1")));
    assert_eq!(sent[4], ("PATCH", "/api/me/progress/li1", r#"{"currentTime":61.0}"#));
    assert_eq!(sent[5], ("PATCH", "/api/me/progress/batch/update", r#"[{"libraryItemId":"li1","isFinished":true}]"#));
    // The empty batch sent nothing.
    assert_eq!(sent[6], ("POST", "/api/me/item/li1/bookmark", r#"{"time":12.5,"title":"Here"}"#));
    assert_eq!(sent[7], ("PATCH", "/api/me/item/li1/bookmark", r#"{"time":100.0,"title":"There"}"#));
    // The time in a delete path is the number as JavaScript writes it.
    assert_eq!((sent[8].0, sent[8].1), ("DELETE", "/api/me/item/li1/bookmark/12.5"));
    assert_eq!(sent[9].1, "/api/me/item/li1/bookmark/100");
    assert_eq!((sent[10].0, sent[10].1), ("DELETE", "/api/me/progress/mp1"));
    assert!(seen.iter().all(|s| s.header("authorization") == Some("Bearer a")));
}

#[tokio::test]
async fn status_needs_no_sign_in() {
    let fake = JsonFake::start(|_| {
        Answer::ok(r#"{"app":"audiobookshelf","serverVersion":"2.37.2","isInit":true,"language":"en-us","authMethods":["local"],"authFormData":{}}"#)
    });
    let status = client_of(&fake).status().await.unwrap();
    assert_eq!(status.version(), Some(Version(2, 37, 2)));
    assert!(status.version().unwrap() >= Version::REFRESH_FIX);
    assert_eq!(fake.seen()[0].header("authorization"), None);
}

#[test]
fn versions_parse_and_order() {
    assert_eq!(Version::parse("v2.26.0"), Some(Version::TOKENS));
    assert_eq!(Version::parse("2.36"), Some(Version::REFRESH_FIX));
    assert_eq!(Version::parse("2.37.2-beta.1"), Some(Version(2, 37, 2)));
    assert_eq!(Version::parse("nightly"), None);
    assert!(Version(2, 30, 9) < Version::REFRESH_FIX && Version(2, 30, 9) > Version::TOKENS);
    assert_eq!(Version(2, 37, 2).to_string(), "2.37.2");
}

fn openid_authorized(s: &Seen) -> Answer {
    let params: HashMap<_, _> =
        Url::parse(&format!("http://localhost{}", s.target)).unwrap().query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    assert_eq!(params["response_type"], "code");
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(params["code_challenge"].len(), 43);
    assert_eq!(params["state"].len(), 43);
    Answer {
        status: "302 Found",
        headers: vec![
            ("Location", format!("https://id.example/authorize?state={}", params["state"])),
            ("Set-Cookie", "connect.sid=SECRET-SESSION; Path=/; HttpOnly".into()),
            ("Set-Cookie", "auth_method=openid-mobile; Path=/; HttpOnly".into()),
        ],
        body: String::new(),
    }
}

fn openid_callback(pending: &OpenId, scheme: &str) -> String {
    let state = Url::parse(pending.browser_url()).unwrap().query_pairs().find(|(k, _)| k == "state").unwrap().1.into_owned();
    format!("{scheme}://oauth?code=SECRET-CODE&state={state}")
}

#[tokio::test]
async fn openid_falls_back_only_for_the_exact_invalid_redirect_response() {
    let fake =
        JsonFake::start(
            |s| if s.target.contains("rede%3A%2F%2Foauth") { Answer::new("400 Bad Request", "Invalid redirect_uri") } else { openid_authorized(s) },
        );
    let pending = client_of(&fake).begin_openid().await.unwrap();
    assert!(pending.used_fallback());
    assert_eq!(fake.count("/auth/openid"), 2);
    let other = JsonFake::start(|_| Answer::new("400 Bad Request", "code_challenge required for mobile flow (PKCE)"));
    assert!(matches!(client_of(&other).begin_openid().await, Err(Error::OpenId(OpenIdError::Server { status: 400 }))));
    assert_eq!(other.count("/auth/openid"), 1);
}

#[tokio::test]
async fn openid_exchanges_the_code_with_its_original_session_and_stores_tokens() {
    let fake = JsonFake::start(|s| match s.path() {
        "/auth/openid" => openid_authorized(s),
        "/auth/openid/callback" => {
            assert_eq!(s.method, "GET");
            assert_eq!(s.header("cookie"), Some("connect.sid=SECRET-SESSION; auth_method=openid-mobile"));
            assert_eq!(s.header("x-return-tokens"), None);
            let url = Url::parse(&format!("http://localhost{}", s.target)).unwrap();
            let params: HashMap<_, _> = url.query_pairs().collect();
            assert_eq!(params["code"], "SECRET-CODE");
            assert_eq!(params["code_verifier"].len(), 43);
            Answer::ok(login_reply("SECRET-ACCESS", "SECRET-REFRESH"))
        }
        _ => panic!("unexpected request"),
    });
    let client = client_of(&fake);
    let saved = Arc::new(Mutex::new(None));
    let stored = saved.clone();
    client.set_auth_hook(move |e| {
        if let AuthEvent::Tokens(t) = e {
            *stored.lock().unwrap() = Some(t);
        }
    });
    let pending = client.begin_openid().await.unwrap();
    assert!(!pending.used_fallback());
    let callback = openid_callback(&pending, "rede");
    let secrets = serde_json::to_value(&pending).unwrap();
    let request = fake.seen().into_iter().find(|r| r.path() == "/auth/openid").unwrap();
    let url = Url::parse(&format!("http://localhost{}", request.target)).unwrap();
    let params: HashMap<_, _> = url.query_pairs().collect();
    use base64::Engine;
    use sha2::Digest;
    let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(secrets["verifier"].as_str().unwrap().as_bytes()));
    assert_eq!(params["code_challenge"], expected);
    assert_eq!(params["redirect_uri"], "rede://oauth");
    assert_ne!(secrets["state"], secrets["verifier"]);
    // A round-trip through secure storage preserves what a cold launch needs.
    let pending: OpenId = serde_json::from_str(&serde_json::to_string(&pending).unwrap()).unwrap();
    let login = client.complete_openid(pending, &callback).await.unwrap();
    assert_eq!(login.user.username.as_deref(), Some("joe"));
    assert_eq!(saved.lock().unwrap().as_ref().unwrap().refresh(), "SECRET-REFRESH");
    assert_eq!(client.tokens().unwrap().access(), "SECRET-ACCESS");
}

#[tokio::test]
async fn an_unrelated_redirect_is_ignored_before_an_exchange_or_cancellation() {
    let fake = JsonFake::start(openid_authorized);
    let client = client_of(&fake);
    let pending = client.begin_openid().await.unwrap();
    for callback in ["rede://oauth?state=wrong&code=SECRET", "rede://oauth?state=wrong&error=access_denied", "https://example.org/?state=wrong"] {
        assert!(matches!(pending.validate_callback(callback), Err(Error::OpenId(OpenIdError::StateMismatch))));
    }
    let callback = openid_callback(&pending, "rede").replace("code=SECRET-CODE", "error=access_denied");
    assert!(matches!(pending.validate_callback(&callback), Err(Error::OpenId(OpenIdError::Cancelled))));
    let undefined = openid_callback(&pending, "rede").replace("SECRET-CODE", "undefined");
    assert!(matches!(pending.validate_callback(&undefined), Err(Error::OpenId(OpenIdError::Cancelled))));
    let duplicate = format!("{}&state=other", openid_callback(&pending, "rede"));
    assert!(matches!(pending.validate_callback(&duplicate), Err(Error::OpenId(OpenIdError::StateMismatch))));
    assert_eq!(fake.count("/auth/openid/callback"), 0);
}

#[tokio::test]
async fn openid_secrets_never_appear_in_debug_or_errors() {
    let fake = JsonFake::start(|s| {
        if s.path() == "/auth/openid" { openid_authorized(s) } else { Answer::new("500 Internal Server Error", "SECRET-CODE SECRET-VERIFIER SECRET-STATE") }
    });
    let client = client_of(&fake);
    let pending = client.begin_openid().await.unwrap();
    let serialized = serde_json::to_value(&pending).unwrap();
    let callback = openid_callback(&pending, "rede");
    let shown = format!("{pending:?} {:?} {}", client.complete_openid(pending.clone(), &callback).await.unwrap_err(), OpenIdError::Cancelled);
    for key in ["verifier", "state", "cookies", "browser"] {
        assert!(!shown.contains(serialized[key].as_str().unwrap()));
    }
    assert!(!shown.contains("SECRET"));
}

#[tokio::test]
async fn a_mobile_callback_error_redirect_is_reported_without_following_it() {
    let fake = JsonFake::start(|s| match s.path() {
        "/auth/openid" => openid_authorized(s),
        "/auth/openid/callback" => {
            Answer { status: "302 Found", headers: vec![("Location", "/login?error=SECRET-PROVIDER-ERROR".into())], body: String::new() }
        }
        _ => panic!("the callback's failure redirect must not be followed"),
    });
    let client = client_of(&fake);
    let pending = client.begin_openid().await.unwrap();
    let callback = openid_callback(&pending, "rede");
    let error = client.complete_openid(pending, &callback).await.unwrap_err();
    assert!(matches!(error, Error::OpenId(OpenIdError::Server { status: 302 })));
    assert!(!format!("{error:?} / {error}").contains("SECRET"));
    assert_eq!(fake.count("/login"), 0);
}

#[tokio::test]
async fn bookmark_listing_falls_back_only_when_the_route_is_missing() {
    for status in ["404 Not Found", "403 Forbidden", "500 Internal Server Error"] {
        let fake = JsonFake::start(move |s| match s.path() {
            "/api/me/bookmarks" => Answer::new(status, r#"{"error":"no route"}"#),
            "/api/me" => Answer::ok(r#"{"id":"u","bookmarks":[{"libraryItemId":"book","time":0.84551240822557006,"title":"Phone","createdAt":10}]}"#),
            _ => panic!("unexpected request"),
        });
        let result = client_of(&fake).with_tokens(Tokens::new("a", "r")).bookmarks().await;
        if status.starts_with("404") {
            let bookmarks = result.unwrap();
            assert_eq!(bookmarks[0].time.to_bits(), 0.845_512_408_225_570_1_f64.to_bits());
            assert_eq!(fake.count("/api/me"), 1);
        } else {
            assert_eq!(result.unwrap_err().status(), Some(if status.starts_with("403") { 403 } else { 500 }));
            assert_eq!(fake.count("/api/me"), 0);
        }
    }
}

#[tokio::test]
async fn deleting_a_listed_bookmark_preserves_every_bit_of_its_time() {
    let fake = JsonFake::start(|s| match s.method.as_str() {
        "GET" => Answer::ok(r#"{"bookmarks":[{"libraryItemId":"book","time":1234.0000000000002,"title":"Phone","createdAt":10}]}"#),
        "DELETE" => {
            let time: f64 = s.path().rsplit('/').next().unwrap().parse().unwrap();
            assert_eq!(time.to_bits(), 1_234.000_000_000_000_2_f64.to_bits());
            Answer::ok("")
        }
        _ => Answer::new("404 Not Found", r#"{"error":"No bookmark"}"#),
    });
    let client = client_of(&fake).with_tokens(Tokens::new("a", "r"));
    let listed = client.bookmarks().await.unwrap().remove(0);
    assert_eq!(fake.count("/api/me/bookmarks"), 1, "the 2.36 route answers, so `/api/me` isn't read");
    client.delete_bookmark(&listed.library_item_id, listed.time).await.unwrap();
    assert!(client.rename_bookmark("book", listed.time, "Renamed").await.unwrap_err().is_not_found());
}

#[tokio::test]
async fn bookmark_writes_keep_server_errors_typed() {
    for (status, code) in [("400 Bad Request", 400), ("403 Forbidden", 403), ("404 Not Found", 404)] {
        let fake = JsonFake::start(move |_| Answer::new(status, r#"{"error":"Invalid bookmark"}"#));
        let client = client_of(&fake).with_tokens(Tokens::new("a", "r"));
        assert_eq!(client.create_bookmark("book", 12.5, "Title").await.unwrap_err().status(), Some(code));
        assert_eq!(client.rename_bookmark("book", 12.5, "Title").await.unwrap_err().status(), Some(code));
        assert_eq!(client.delete_bookmark("book", 12.5).await.unwrap_err().status(), Some(code));
    }
}
