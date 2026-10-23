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
fn parses_ping() {
    let json = br#"{"subsonic-response":{"status":"ok","version":"1.16.1","type":"navidrome","serverVersion":"0.58.0","openSubsonic":true}}"#;
    let (info, _): (ServerInfo, serde_json::Value) = parse_response(json).unwrap();
    assert_eq!(info.server_type.as_deref(), Some("navidrome"));
    assert!(info.open_subsonic);
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
