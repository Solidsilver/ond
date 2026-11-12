//! The library knows nothing about servers; Subsonic's shapes become rows here.
//! The Subsonic source uses these conversions because the orphan rule forbids
//! From between two foreign crates. They stay here until a separate module move.

use library::{Album, AlbumTracks, Artist, ArtistLink, Contributor, Track};

pub fn artist(a: subsonic::Artist) -> Artist {
    Artist { id: a.id, name: a.name, sort: a.sort_name, album_count: a.album_count, cover_art: a.cover_art }
}

fn artist_link(a: subsonic::ArtistRef) -> ArtistLink {
    ArtistLink { id: a.id, name: a.name }
}

pub fn album(a: subsonic::Album) -> Album {
    Album {
        id: a.id,
        name: a.name,
        sort: a.sort_name,
        artist: a.artist,
        artist_ref: a.artist_id,
        year: a.year,
        genre: a.genre,
        song_count: a.song_count,
        duration_secs: a.duration,
        cover_art: a.cover_art,
        created: a.created,
        labels: a.record_labels.into_iter().map(|l| l.name).collect(),
        genres: a.genres.into_iter().map(|g| g.name).collect(),
        artists: a.artists.into_iter().map(artist_link).collect(),
    }
}

pub fn track(s: subsonic::Song, album_id: &str) -> Track {
    let rg = s.replay_gain.unwrap_or_default();
    Track {
        id: s.id,
        album_id: s.album_id.unwrap_or_else(|| album_id.to_owned()),
        title: s.title,
        artist: s.artist,
        artist_ref: s.artist_id,
        track: s.track,
        disc: s.disc_number,
        duration_secs: s.duration,
        suffix: s.suffix,
        content_type: s.content_type,
        size: s.size,
        bit_rate_kbps: s.bit_rate,
        bit_depth: s.bit_depth,
        sample_rate: s.sampling_rate,
        channels: s.channel_count,
        track_gain: rg.track_gain,
        album_gain: rg.album_gain,
        track_peak: rg.track_peak,
        album_peak: rg.album_peak,
        composer: s.display_composer.filter(|c| !c.is_empty()),
        contributors: s
            .contributors
            .into_iter()
            .map(|c| Contributor { role: c.role, sub_role: c.sub_role, name: c.artist.name, artist_id: c.artist.id })
            .collect(),
        cover_art: s.cover_art,
        artists: s.artists.into_iter().map(artist_link).collect(),
    }
}

pub fn album_tracks(a: subsonic::AlbumWithSongs) -> AlbumTracks {
    let tracks = a.song.into_iter().map(|s| track(s, &a.album.id)).collect();
    AlbumTracks { album: album(a.album), tracks }
}

pub fn bulk_tracks(songs: Vec<subsonic::Song>) -> Vec<Track> {
    songs
        .into_iter()
        .filter_map(|s| {
            let album_id = s.album_id.clone()?;
            Some(track(s, &album_id))
        })
        .collect()
}

pub fn lyrics(v: subsonic::StructuredLyrics) -> crate::lyrics::StructuredLyrics {
    crate::lyrics::StructuredLyrics {
        synced: v.synced,
        offset: v.offset,
        line: v.line.into_iter().map(|l| crate::lyrics::LyricLine { start: l.start, value: l.value }).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn song(id: &str, album_id: Option<&str>) -> subsonic::Song {
        serde_json::from_value(json!({ "id": id, "title": "Song", "albumId": album_id })).unwrap()
    }

    #[test]
    fn artists_and_albums_keep_their_metadata_and_split_credits() {
        let a = subsonic::Artist {
            id: "ar-1".into(),
            name: "Bill Evans".into(),
            sort_name: Some("Evans, Bill".into()),
            album_count: 3,
            cover_art: Some("cover".into()),
        };
        assert_eq!(
            artist(a),
            Artist { id: "ar-1".into(), name: "Bill Evans".into(), sort: Some("Evans, Bill".into()), album_count: 3, cover_art: Some("cover".into()) }
        );
        let a = serde_json::from_value(json!({
            "id": "al-1", "name": "Sunday", "sortName": "Sunday at", "artist": "Bill Evans Trio", "artistId": "ar-2",
            "year": 1961, "genre": "Jazz", "songCount": 5, "duration": 1800, "coverArt": "cover", "created": "2024-01-02",
            "recordLabels": [{"name": "Riverside"}, {"name": "Fantasy"}], "genres": [{"name": "Bebop"}, {"name": "Modal"}],
            "artists": [{"id": "ar-2", "name": "Bill Evans Trio"}, {"name": "Guest"}]
        }))
        .unwrap();
        assert_eq!(
            album(a),
            Album {
                id: "al-1".into(),
                name: "Sunday".into(),
                sort: Some("Sunday at".into()),
                artist: Some("Bill Evans Trio".into()),
                artist_ref: Some("ar-2".into()),
                year: Some(1961),
                genre: Some("Jazz".into()),
                song_count: 5,
                duration_secs: 1800,
                cover_art: Some("cover".into()),
                created: Some("2024-01-02".into()),
                labels: vec!["Riverside".into(), "Fantasy".into()],
                genres: vec!["Bebop".into(), "Modal".into()],
                artists: vec![ArtistLink { id: Some("ar-2".into()), name: "Bill Evans Trio".into() }, ArtistLink { id: None, name: "Guest".into() }],
            }
        );
    }

    #[test]
    fn tracks_keep_their_audio_metadata_gains_and_credits() {
        let s = serde_json::from_value(json!({
            "id": "s-1", "albumId": "al-1", "title": "Gloria's Step", "artist": "Bill Evans Trio", "artistId": "ar-2",
            "track": 2, "discNumber": 1, "duration": 370, "suffix": "flac", "contentType": "audio/flac", "size": 123456,
            "bitRate": 900, "bitDepth": 24, "samplingRate": 88200, "channelCount": 2,
            "replayGain": {"trackGain": -3.5, "albumGain": -2.5, "trackPeak": 0.75, "albumPeak": 1.0},
            "displayComposer": "Scott LaFaro", "coverArt": "cover",
            "contributors": [{"role": "performer", "subRole": "bass", "artist": {"id": "ar-3", "name": "Scott LaFaro"}}],
            "artists": [{"name": "Bill Evans"}]
        }))
        .unwrap();
        assert_eq!(
            track(s, "fallback"),
            Track {
                id: "s-1".into(),
                album_id: "al-1".into(),
                title: "Gloria's Step".into(),
                artist: Some("Bill Evans Trio".into()),
                artist_ref: Some("ar-2".into()),
                track: Some(2),
                disc: Some(1),
                duration_secs: Some(370),
                suffix: Some("flac".into()),
                content_type: Some("audio/flac".into()),
                size: Some(123456),
                bit_rate_kbps: Some(900),
                bit_depth: Some(24),
                sample_rate: Some(88200),
                channels: Some(2),
                track_gain: Some(-3.5),
                album_gain: Some(-2.5),
                track_peak: Some(0.75),
                album_peak: Some(1.0),
                composer: Some("Scott LaFaro".into()),
                contributors: vec![Contributor {
                    role: "performer".into(),
                    sub_role: Some("bass".into()),
                    name: "Scott LaFaro".into(),
                    artist_id: Some("ar-3".into())
                }],
                cover_art: Some("cover".into()),
                artists: vec![ArtistLink { id: None, name: "Bill Evans".into() }],
            }
        );
    }

    #[test]
    fn missing_album_ids_fall_back_and_only_empty_composers_are_filtered() {
        for (composer, expected) in [(None, None), (Some(""), None), (Some(" "), Some(" "))] {
            let mut s = song("s-1", None);
            s.display_composer = composer.map(str::to_owned);
            let t = track(s, "fallback");
            assert_eq!(t.album_id, "fallback");
            assert_eq!(t.composer.as_deref(), expected);
            assert_eq!((t.track_gain, t.album_gain, t.track_peak, t.album_peak), (None, None, None, None));
        }
        let a = serde_json::from_value(json!({"id": "al-1", "name": "Album", "songCount": 2, "duration": 0})).unwrap();
        let rows = album_tracks(subsonic::AlbumWithSongs { album: a, song: vec![song("missing", None), song("explicit", Some("other"))] });
        assert_eq!(rows.album.id, "al-1");
        assert_eq!(rows.tracks.iter().map(|t| t.album_id.as_str()).collect::<Vec<_>>(), ["al-1", "other"]);
    }

    #[test]
    fn bulk_tracks_drop_only_songs_without_an_album_id() {
        let tracks = bulk_tracks(vec![song("a", Some("one")), song("x", None), song("b", Some("")), song("c", Some("two"))]);
        assert_eq!(tracks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(tracks.iter().map(|t| t.album_id.as_str()).collect::<Vec<_>>(), ["one", "", "two"]);
    }
}
