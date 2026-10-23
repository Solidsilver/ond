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
