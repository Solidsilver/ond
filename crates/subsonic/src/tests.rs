use super::*;

#[test]
fn token_matches_spec_example() {
    // Example from the Subsonic API docs: password "sesame", salt "c19b2d".
    let creds = Credentials::new("joe", "sesame");
    let t = creds.token_with_salt("c19b2d");
    assert_eq!(t.token, "26719a1196d2a940705a59634eb18eab");
    assert_eq!(t.salt, "c19b2d");
}

#[test]
fn fresh_salt_per_token() {
    let creds = Credentials::new("joe", "sesame");
    let (a, b) = (creds.token(), creds.token());
    assert_ne!(a.salt, b.salt);
    assert_eq!(a.salt.len(), 12);
    assert_eq!(creds.token_with_salt(&a.salt), a);
}

#[test]
fn debug_hides_password() {
    let creds = Credentials::new("joe", "sesame");
    assert!(!format!("{creds:?}").contains("sesame"));
}

#[test]
fn endpoint_has_auth_params_and_raw_stream() {
    let c = Client::new("https://music.example.com/navidrome", Credentials::new("joe", "pw")).unwrap();
    let url = c.stream_url("abc");
    assert_eq!(url.path(), "/navidrome/rest/stream");
    let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(q["u"], "joe");
    assert_eq!(q["v"], API_VERSION);
    assert_eq!(q["f"], "json");
    assert_eq!(q["format"], "raw");
    assert_eq!(q["id"], "abc");
    assert_eq!(Credentials::new("joe", "pw").token_with_salt(&q["s"]).token, q["t"]);
}

#[test]
fn transcoded_stream_asks_for_format_and_bitrate() {
    let c = Client::new("https://music.example.com", Credentials::new("joe", "pw")).unwrap();
    let q: std::collections::HashMap<_, _> = c.transcoded_stream_url("abc", "aac", 256).query_pairs().into_owned().collect();
    assert_eq!((q["format"].as_str(), q["maxBitRate"].as_str(), q["id"].as_str()), ("aac", "256", "abc"));
}

#[test]
fn parses_ping() {
    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1","type":"navidrome","serverVersion":"0.58.0","openSubsonic":true}}"#;
    let (info, _): (ServerInfo, serde_json::Value) = parse_response(json).unwrap();
    assert_eq!(info.server_type.as_deref(), Some("navidrome"));
    assert!(info.open_subsonic);
}

#[test]
fn parses_structured_lyrics() {
    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1","openSubsonic":true,
      "lyricsList":{"structuredLyrics":[
        {"displayArtist":"Bill Evans Trio","displayTitle":"Solar","lang":"eng","offset":-100,"synced":true,
         "line":[{"start":0,"value":"First"},{"start":2500,"value":"Second"}]},
        {"lang":"xxx","synced":false,"line":[{"value":"Plain one"},{"value":"Plain two"}]}
      ]}}}"#;
    let (_, p): (_, LyricsListPayload) = parse_response(json).unwrap();
    let l = &p.lyrics_list.structured_lyrics;
    assert_eq!(l.len(), 2);
    assert!(l[0].synced);
    assert_eq!(l[0].offset, Some(-100));
    assert_eq!(l[0].line[1].start, Some(2500));
    assert_eq!(l[1].line[0].start, None);
    // No lyrics at all: an empty list, not an error.
    let empty = br#"{"subsonic-response":{"status":"ok","version":"1.16.1","lyricsList":{}}}"#;
    let (_, p): (_, LyricsListPayload) = parse_response(empty).unwrap();
    assert!(p.lyrics_list.structured_lyrics.is_empty());
    let plain = br#"{"subsonic-response":{"status":"ok","version":"1.16.1","lyrics":{"artist":"A","title":"T","value":"la la"}}}"#;
    let (_, p): (_, LyricsPayload) = parse_response(plain).unwrap();
    assert_eq!(p.lyrics.and_then(|l| l.value).as_deref(), Some("la la"));
}

#[test]
fn parses_api_error() {
    let json = br#"{"subsonic-response":{"status":"failed","version":"1.16.1","error":{"code":40,"message":"Wrong username or password"}}}"#;
    match parse_response::<serde_json::Value>(json) {
        Err(Error::Api { code, message }) => {
            assert_eq!(code, 40);
            assert_eq!(message, "Wrong username or password");
        }
        other => panic!("expected api error, got {other:?}"),
    }
}

#[test]
fn parses_album_list2() {
    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1","type":"navidrome","serverVersion":"0.58.0","openSubsonic":true,
      "albumList2":{"album":[
        {"id":"al-1","name":"Kind of Blue","artist":"Miles Davis","artistId":"ar-1","coverArt":"al-1_0","songCount":5,"duration":2756,"year":1959,"genre":"Jazz","created":"2024-01-02T10:00:00Z","sortName":"kind of blue"},
        {"id":"al-2","name":"In Rainbows","songCount":10,"duration":2552}
      ]}}}"#;
    let (_, p): (_, AlbumList2Payload) = parse_response(json).unwrap();
    let albums = p.album_list2.album;
    assert_eq!(albums.len(), 2);
    assert_eq!(albums[0].year, Some(1959));
    assert_eq!(albums[0].artist_id.as_deref(), Some("ar-1"));
    assert_eq!(albums[1].artist, None);
}

#[test]
fn parses_empty_album_list2() {
    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1","albumList2":{}}}"#;
    let (_, p): (_, AlbumList2Payload) = parse_response(json).unwrap();
    assert!(p.album_list2.album.is_empty());
}

#[test]
fn parses_album_with_hires_songs() {
    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1","openSubsonic":true,
      "album":{"id":"al-9","name":"Random Access Memories","artist":"Daft Punk","songCount":2,"duration":800,
        "song":[
          {"id":"s-1","title":"Give Life Back to Music","albumId":"al-9","track":1,"discNumber":1,"duration":274,
           "suffix":"flac","contentType":"audio/flac","bitRate":2900,"bitDepth":24,"samplingRate":88200,"channelCount":2,
           "replayGain":{"trackGain":-8.1,"albumGain":-8.4,"trackPeak":0.98,"albumPeak":0.99}},
          {"id":"s-2","title":"The Game of Love","track":2,"duration":321}
        ]}}}"#;
    let (_, p): (_, AlbumPayload) = parse_response(json).unwrap();
    let a = p.album;
    assert_eq!(a.album.name, "Random Access Memories");
    assert_eq!(a.song.len(), 2);
    let s = &a.song[0];
    assert_eq!((s.bit_depth, s.sampling_rate), (Some(24), Some(88200)));
    assert_eq!(s.replay_gain.as_ref().and_then(|r| r.album_gain), Some(-8.4));
    assert_eq!(a.song[1].replay_gain, None);
}

#[test]
fn parses_artists_index() {
    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1",
      "artists":{"ignoredArticles":"The El La Los Las Le Les","index":[
        {"name":"A","artist":[{"id":"ar-1","name":"Air","albumCount":3}]},
        {"name":"M","artist":[{"id":"ar-2","name":"Miles Davis","albumCount":12,"coverArt":"ar-2"}]}
      ]}}}"#;
    let (_, p): (_, ArtistsPayload) = parse_response(json).unwrap();
    let names: Vec<_> = p.artists.index.iter().flat_map(|i| &i.artist).map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Air", "Miles Davis"]);
}

#[test]
fn parses_navidrome_participants_and_labels() {
    // Shape as Navidrome 0.55+ returns it from getAlbum (OpenSubsonic).
    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1","type":"navidrome","serverVersion":"0.64.2","openSubsonic":true,
      "album":{"id":"al-7","name":"Sunday at the Village Vanguard","artist":"Bill Evans Trio","songCount":1,"duration":370,
        "year":1961,"genre":"Jazz","recordLabels":[{"name":"Riverside"}],"genres":[{"name":"Jazz"},{"name":"Cool Jazz"}],
        "song":[{"id":"s-1","title":"Gloria's Step","track":1,"duration":370,"displayComposer":"Scott LaFaro",
          "contributors":[
            {"role":"composer","artist":{"id":"ar-9","name":"Scott LaFaro"}},
            {"role":"performer","subRole":"piano","artist":{"id":"ar-1","name":"Bill Evans"}},
            {"role":"producer","artist":{"name":"Orrin Keepnews"}}
          ]}]}}}"#;
    let (_, p): (_, AlbumPayload) = parse_response(json).unwrap();
    let a = p.album;
    assert_eq!(a.album.record_labels, [Named { name: "Riverside".into() }]);
    assert_eq!(a.album.genres.len(), 2);
    let s = &a.song[0];
    assert_eq!(s.display_composer.as_deref(), Some("Scott LaFaro"));
    assert_eq!(s.contributors.len(), 3);
    assert_eq!(s.contributors[1].sub_role.as_deref(), Some("piano"));
    assert_eq!(s.contributors[2].artist.id, None);
}

#[test]
fn parses_playlists() {
    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1",
      "playlists":{"playlist":[{"id":"pl-1","name":"Late night","songCount":14,"duration":3600,"owner":"me"}]}}}"#;
    let (_, p): (_, PlaylistsPayload) = parse_response(json).unwrap();
    assert_eq!(p.playlists.playlist[0].name, "Late night");
    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1",
      "playlist":{"id":"pl-1","name":"Late night","songCount":1,"duration":200,"entry":[{"id":"s-1","title":"Peace Piece","albumId":"al-3","duration":402}]}}}"#;
    let (_, p): (_, PlaylistPayload) = parse_response(json).unwrap();
    assert_eq!(p.playlist.entry[0].album_id.as_deref(), Some("al-3"));
}

#[test]
fn parses_search3_songs_and_split_artists() {
    // Navidrome 0.64: a joined display string plus each artist as an entity.
    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1","openSubsonic":true,
      "searchResult3":{"song":[
        {"id":"s-1","title":"Ondes","albumId":"al-1","artist":"Johannes Motschmann, David Panzl, Boris Bolles",
         "artists":[{"id":"ar-1","name":"Johannes Motschmann"},{"id":"ar-2","name":"David Panzl"},{"id":"ar-3","name":"Boris Bolles"}]},
        {"id":"s-2","title":"No artists field","albumId":"al-2"}
      ]}}}"#;
    let (_, p): (_, SearchResult3Payload) = parse_response(json).unwrap();
    let songs = p.search_result3.song;
    assert_eq!(songs.len(), 2);
    assert_eq!(songs[0].artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Johannes Motschmann", "David Panzl", "Boris Bolles"]);
    assert!(songs[1].artists.is_empty());
    // An empty result omits the key entirely.
    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1","searchResult3":{}}}"#;
    let (_, p): (_, SearchResult3Payload) = parse_response(json).unwrap();
    assert!(p.search_result3.song.is_empty());
}

#[test]
fn parses_album_artist_entities() {
    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1",
      "albumList2":{"album":[{"id":"al-1","name":"Duo","artist":"A & B","artists":[{"id":"ar-a","name":"A"},{"id":"ar-b","name":"B"}]}]}}}"#;
    let (_, p): (_, AlbumList2Payload) = parse_response(json).unwrap();
    assert_eq!(p.album_list2.album[0].artists.len(), 2);
    assert_eq!(p.album_list2.album[0].artists[1].id.as_deref(), Some("ar-b"));
}

#[test]
fn parses_starred2_with_ratings() {
    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1","starred2":{
        "artist":[{"id":"ar1","name":"Bill Evans","albumCount":14,"starred":"2026-01-02T03:04:05Z"}],
        "album":[{"id":"al1","name":"Portrait in Jazz","songCount":12,"duration":2700,"starred":"2026-01-02T03:04:05Z","userRating":4}],
        "song":[{"id":"s1","title":"Peace Piece","albumId":"al2","starred":"2026-01-02T03:04:05Z","userRating":5},
                {"id":"s2","title":"Autumn Leaves","albumId":"al1"}]}}}"#;
    let (_, p): (ServerInfo, Starred2Payload) = parse_response(json).unwrap();
    let s = p.starred2;
    assert_eq!(s.artist[0].name, "Bill Evans");
    assert_eq!(s.album[0].user_rating, Some(4));
    assert_eq!(s.song[0].user_rating, Some(5));
    assert!(s.song[0].starred.is_some());
    assert_eq!(s.song[1].user_rating, None);
    // An empty result omits the lists entirely.
    let empty = br#"{"subsonic-response":{"status":"ok","version":"1.16.1","starred2":{}}}"#;
    let (_, p): (ServerInfo, Starred2Payload) = parse_response(empty).unwrap();
    assert!(p.starred2.song.is_empty());
}

#[test]
fn playlist_edits_repeat_their_params() {
    let c = Client::new("https://music.example.com", Credentials::new("joe", "pw")).unwrap();
    let url = c.endpoint("updatePlaylist", &[("playlistId", "p1"), ("songIdToAdd", "a"), ("songIdToAdd", "b"), ("songIndexToRemove", "3")]);
    let adds: Vec<String> = url.query_pairs().filter(|(k, _)| k == "songIdToAdd").map(|(_, v)| v.into_owned()).collect();
    assert_eq!(adds, ["a", "b"]);
    assert_eq!(StarKind::Album.param(), "albumId");
}

#[tokio::test]
async fn http_errors_never_carry_the_auth_token() {
    // Nothing listens on port 1: the request fails, and reqwest would name the
    // URL (with u, t and s) in its message.
    let c = Client::new("http://127.0.0.1:1/", Credentials::new("joe", "sesame")).unwrap();
    let e = c.ping().await.unwrap_err();
    let shown = format!("{e} / {e:?}");
    for leak in ["t=", "s=", "u=joe", "127.0.0.1:1/rest"] {
        assert!(!shown.contains(leak), "{leak} leaked in {shown}");
    }
}

#[test]
fn error_replies_are_recognised_as_json_or_xml() {
    let json = br#"{"subsonic-response":{"status":"failed","version":"1.16.1","error":{"code":70,"message":"Not found"}}}"#;
    let xml = br#"<?xml version="1.0" encoding="UTF-8"?><subsonic-response status="failed"><error code="70" message="Not found"/></subsonic-response>"#;
    assert!(crate::is_error_reply(Some("application/json"), b""));
    assert!(crate::is_error_reply(Some("text/xml; charset=utf-8"), b""));
    // No or a wrong content type: sniffed from the body.
    assert!(crate::is_error_reply(None, json));
    assert!(crate::is_error_reply(Some("audio/flac"), xml));
    assert!(!crate::is_error_reply(Some("audio/flac"), b"fLaC\0\0\0\x22"));
    assert!(!crate::is_error_reply(None, b"{ not subsonic }"));
    assert_eq!(crate::error_text(json), "Not found");
    assert_eq!(crate::error_text(xml), "Not found");
}

#[test]
fn one_malformed_song_is_skipped_not_the_page() {
    // s-2 has no title; s-3's numbers come as strings and floats, as some
    // servers send them; s-4's are out of range or junk.
    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1",
      "searchResult3":{"song":[
        {"id":"s-1","title":"Peace Piece","track":1},
        {"id":"s-2","track":2},
        {"id":"s-3","title":"Strings","track":"3","discNumber":"1","year":"1961","duration":402.6,"bitRate":"1411","size":"12345"},
        {"id":"s-4","title":"Junk","track":-1,"year":"unknown","userRating":300,"replayGain":"loud","contributors":[{"role":"composer"}]}
      ]}}}"#;
    let (_, p): (_, SearchResult3Payload) = parse_response(json).unwrap();
    let songs = p.search_result3.song;
    assert_eq!(songs.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s-1", "s-3", "s-4"]);
    let s = &songs[1];
    assert_eq!((s.track, s.disc_number, s.year, s.duration, s.bit_rate, s.size), (Some(3), Some(1), Some(1961), Some(403), Some(1411), Some(12345)));
    let s = &songs[2];
    assert_eq!((s.track, s.year, s.user_rating), (None, None, None));
    assert_eq!(s.replay_gain, None);
    assert!(s.contributors.is_empty(), "a contributor without an artist is dropped, not the song");
}

#[test]
fn malformed_items_are_skipped_in_every_list() {
    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1",
      "albumList2":{"album":[{"id":"al-1","name":"Fine","songCount":"12","duration":2700.0},{"name":"No id"},{"id":"al-3","name":"Also fine"}]}}}"#;
    let (_, p): (_, AlbumList2Payload) = parse_response(json).unwrap();
    let albums = p.album_list2.album;
    assert_eq!(albums.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["al-1", "al-3"]);
    assert_eq!((albums[0].song_count, albums[0].duration), (12, 2700));

    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1",
      "album":{"id":"al-9","name":"Album","song":[{"id":"s-1","title":"One"},{"id":7,"title":"Numeric id"},{"id":"s-3","title":"Three"}]}}}"#;
    let (_, p): (_, AlbumPayload) = parse_response(json).unwrap();
    assert_eq!(p.album.song.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s-1", "s-3"]);

    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1",
      "playlist":{"id":"pl-1","name":"Mix","entry":[{"title":"No id"},{"id":"s-2","title":"Two"}]}}}"#;
    let (_, p): (_, PlaylistPayload) = parse_response(json).unwrap();
    assert_eq!(p.playlist.entry.len(), 1);

    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1",
      "artists":{"index":[{"name":"A","artist":[{"id":"ar-1","name":"Air"},{"id":"ar-2"}]},{"name":"B","artist":"oops"}]}}}"#;
    let (_, p): (_, ArtistsPayload) = parse_response(json).unwrap();
    assert_eq!(p.artists.index.iter().flat_map(|i| &i.artist).count(), 1);
}

#[test]
fn lists_take_null_and_a_lone_object() {
    // Servers that convert from XML send a one-item list as the item itself.
    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1",
      "album":{"id":"al-9","name":"Single","song":{"id":"s-1","title":"Only"},"genres":null}}}"#;
    let (_, p): (_, AlbumPayload) = parse_response(json).unwrap();
    assert_eq!(p.album.song.len(), 1);
    assert!(p.album.album.genres.is_empty());
}

#[test]
fn a_reply_missing_what_was_asked_for_still_fails() {
    // Leniency is per list item: a reply without the payload, or a single
    // song without an id, is wrong.
    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1"}}"#;
    assert!(matches!(parse_response::<AlbumPayload>(json), Err(Error::Decode(_))));
    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1","song":{"title":"No id"}}}"#;
    assert!(matches!(parse_response::<SongPayload>(json), Err(Error::Decode(_))));
}

#[test]
fn failed_status_without_an_error_object() {
    let json = br#"{"subsonic-response":{"status":"failed","version":"1.16.1"}}"#;
    match parse_response::<serde_json::Value>(json) {
        Err(Error::Api { code, message }) => assert_eq!((code, message.as_str()), (-1, "unknown error")),
        other => panic!("expected api error, got {other:?}"),
    }
}

#[test]
fn server_addresses() {
    let creds = || Credentials::new("joe", "pw");
    // A query string or fragment on the address is dropped, not sent along.
    let c = Client::new("https://music.example.com/nd?theme=dark#x", creds()).unwrap();
    let url = c.endpoint("ping", &[]);
    assert_eq!(url.path(), "/nd/rest/ping");
    assert!(!url.query().unwrap().contains("theme"));
    assert_eq!(url.fragment(), None);
    // No scheme: "host:port" would parse with the host as its scheme.
    for bad in ["music.example.com", "music.example.com:4533", "localhost:4533/nd", "ftp://music.example.com", "mailto:joe@example.com"] {
        assert!(matches!(Client::new(bad, creds()), Err(Error::Url(_))), "{bad} accepted");
    }
}

/// Serves one connection on a local port: reads the request, then hands the
/// stream to `respond`. Returns the base URL.
fn serve_once(respond: impl FnOnce(std::net::TcpStream) + Send + 'static) -> String {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut req = Vec::new();
        let mut buf = [0u8; 1024];
        while !req.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => req.extend_from_slice(&buf[..n]),
            }
        }
        respond(stream);
    });
    format!("http://{addr}/")
}

fn reply(status: &str, content_type: &str, body: &str) -> impl FnOnce(std::net::TcpStream) + Send + 'static {
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    move |mut s| {
        use std::io::Write;
        let _ = s.write_all(response.as_bytes());
    }
}

#[tokio::test]
async fn http_errors_keep_the_servers_message() {
    let cases = [
        ("503 Service Unavailable", "text/html", "<html><head><title>503 Service\n Unavailable</title></head><body>nginx</body></html>", "503 Service Unavailable"),
        ("401 Unauthorized", "application/json", r#"{"subsonic-response":{"status":"failed","version":"1.16.1","error":{"code":40,"message":"Wrong username or password"}}}"#, "Wrong username or password"),
        ("500 Internal Server Error", "text/plain", "database is locked\nstack trace...", "database is locked"),
    ];
    for (status, ct, body, said) in cases {
        let base = serve_once(reply(status, ct, body));
        let c = Client::new(&base, Credentials::new("joe", "sesame")).unwrap();
        match c.ping().await {
            Err(Error::Http(h)) => {
                assert_eq!(h.message(), Some(said));
                assert_eq!(h.status().map(|s| s.as_u16()), status[..3].parse().ok());
                let shown = Error::Http(h).to_string();
                assert!(shown.contains(said) && !shown.contains("t="), "{shown}");
            }
            other => panic!("expected an http error, got {other:?}"),
        }
    }
    // An empty body: nothing to add.
    let base = serve_once(reply("502 Bad Gateway", "text/html", ""));
    let c = Client::new(&base, Credentials::new("joe", "sesame")).unwrap();
    assert!(matches!(c.ping().await, Err(Error::Http(h)) if h.message().is_none()));
}

#[tokio::test]
async fn a_call_that_trickles_times_out() {
    // Headers, then a byte at a time, forever: the read timeout never fires,
    // so only the call's total timeout can end it.
    let base = serve_once(|mut s| {
        use std::io::Write;
        if s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100000\r\n\r\n").is_err() {
            return;
        }
        while s.write_all(b" ").and_then(|()| s.flush()).is_ok() {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    });
    let c = Client::new(&base, Credentials::new("joe", "sesame")).unwrap().with_call_timeout(std::time::Duration::from_millis(300));
    match c.ping().await {
        Err(Error::Http(h)) => assert!(h.is_timeout(), "{h:?}"),
        other => panic!("expected a timeout, got {other:?}"),
    }
}
