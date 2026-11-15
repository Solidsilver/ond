//! Subsonic's API becomes the rows and answers the app understands here.
//! The app sees capabilities, never a source kind; local folders will answer
//! the same MusicSource without implementing server paging (docs/sources.md).

use super::{Capabilities, MusicSource, Playlist, StarKind};
use crate::bridge::{PlaylistEdit, connect_error};
use crate::convert;
use crate::failure::Failure;
use crate::favorites::{Starred, UserMeta};
use crate::lyrics::{self, Lyrics, SongStats};
use crate::sync::{Fetch, SongPage};
use ::subsonic::{AlbumListType, Client};
use async_trait::async_trait;
use library::{Album, AlbumTracks, Artist, Track};
use std::sync::Arc;

#[derive(Clone)]
pub struct SubsonicSource {
    account: accounts::AccountId,
    client: Client,
    artist_mbids: Arc<std::sync::Mutex<Vec<(String, String)>>>,
    copy_mbids: Arc<std::sync::Mutex<super::mbids::CopyMbids>>,
}

impl SubsonicSource {
    pub fn new(account: accounts::AccountId, client: Client) -> Self {
        Self { account, client, artist_mbids: Arc::default(), copy_mbids: Arc::default() }
    }

    fn remember_albums(&self, albums: &[::subsonic::Album]) {
        let mut ids = self.copy_mbids.lock().unwrap_or_else(|e| e.into_inner());
        for album in albums {
            ids.albums.insert(album.id.clone(), super::artists::clean_mbid(album.music_brainz_id.as_deref()));
        }
    }

    fn remember_songs(&self, songs: &[::subsonic::Song]) {
        let mut ids = self.copy_mbids.lock().unwrap_or_else(|e| e.into_inner());
        for song in songs {
            ids.recordings.insert(song.id.clone(), super::artists::clean_mbid(song.music_brainz_id.as_deref()));
        }
    }

    async fn read_album(&self, id: &str) -> Result<::subsonic::AlbumWithSongs, Failure> {
        let album = self.client.album(id).await?;
        self.remember_albums(std::slice::from_ref(&album.album));
        self.remember_songs(&album.song);
        Ok(album)
    }
}

fn song_meta(songs: &[::subsonic::Song]) -> UserMeta {
    UserMeta {
        starred: songs.iter().map(|s| (StarKind::Song, s.id.clone(), s.starred.is_some())).collect(),
        ratings: songs.iter().map(|s| (s.id.clone(), s.user_rating.unwrap_or(0))).collect(),
    }
}

fn song_track(s: ::subsonic::Song) -> Track {
    let album = s.album_id.clone().unwrap_or_default();
    convert::track(s, &album)
}

fn playlist(p: ::subsonic::Playlist) -> Playlist {
    Playlist { id: p.id, name: p.name, song_count: p.song_count, duration: p.duration, cover_art: p.cover_art, owner: p.owner }
}

#[async_trait]
impl MusicSource for SubsonicSource {
    fn account(&self) -> accounts::AccountId {
        self.account
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::subsonic()
    }

    fn paged(&self) -> Option<Arc<dyn Fetch>> {
        Some(Arc::new(self.clone()))
    }

    async fn ping(&self) -> Result<String, Failure> {
        let info = self.client.ping().await.map_err(|e| {
            let message = connect_error(&e);
            match e {
                ::subsonic::Error::Http(_) => Failure::Network(message),
                _ => Failure::Server(message),
            }
        })?;
        Ok(format!("{} {}", info.server_type.as_deref().unwrap_or("subsonic"), info.server_version.as_deref().unwrap_or(&info.version)))
    }

    async fn album(&self, id: &str) -> Result<(AlbumTracks, UserMeta), Failure> {
        let album = self.read_album(id).await?;
        let mut meta = song_meta(&album.song);
        meta.starred.push((StarKind::Album, id.to_owned(), album.album.starred.is_some()));
        Ok((convert::album_tracks(album), meta))
    }

    async fn newest(&self, count: u32) -> Result<Vec<Album>, Failure> {
        let albums = self.client.album_list(AlbumListType::Newest, count, 0).await?;
        self.remember_albums(&albums);
        Ok(albums.into_iter().map(convert::album).collect())
    }

    async fn playlists(&self) -> Result<Vec<Playlist>, Failure> {
        Ok(self.client.playlists().await?.into_iter().map(playlist).collect())
    }

    async fn playlist(&self, id: &str) -> Result<(Vec<Track>, UserMeta), Failure> {
        let p = self.client.playlist(id).await?;
        let meta = song_meta(&p.entry);
        Ok((p.entry.into_iter().map(song_track).collect(), meta))
    }

    async fn starred(&self) -> Result<(Starred, Vec<(String, u8)>), Failure> {
        let s = self.client.starred().await?;
        let mut ratings: Vec<(String, u8)> = s.song.iter().filter_map(|t| t.user_rating.filter(|r| *r > 0).map(|r| (t.id.clone(), r))).collect();
        ratings.extend(s.album.iter().filter_map(|a| a.user_rating.filter(|r| *r > 0).map(|r| (a.id.clone(), r))));
        let starred = Starred {
            songs: s.song.into_iter().map(song_track).collect(),
            albums: s.album.into_iter().map(|a| a.id).collect(),
            artists: s.artist.into_iter().map(|a| (a.id, a.name)).collect(),
        };
        Ok((starred, ratings))
    }

    async fn lyrics(&self, track_id: &str, artist: Option<&str>, title: &str) -> Result<Option<Lyrics>, Failure> {
        // Lyrics are a nicety: a server failure must never raise an error banner.
        let mut lyrics = match self.client.lyrics_by_song(track_id).await {
            Ok(versions) => lyrics::pick(versions.into_iter().map(convert::lyrics).collect()),
            Err(e) => {
                tracing::debug!("getLyricsBySongId {track_id}: {e}");
                None
            }
        };
        if lyrics.is_none()
            && let Some(artist) = artist
        {
            match self.client.lyrics(artist, title).await {
                Ok(text) => lyrics = text.as_deref().and_then(lyrics::from_text),
                Err(e) => tracing::debug!("getLyrics {title}: {e}"),
            }
        }
        Ok(lyrics)
    }

    async fn song_stats(&self, id: &str) -> Result<SongStats, Failure> {
        let s = self.client.song(id).await?;
        Ok(SongStats { plays: s.play_count, last_played: s.played })
    }

    async fn set_starred(&self, kind: StarKind, id: &str, on: bool) -> Result<(), Failure> {
        let kind = match kind {
            StarKind::Song => ::subsonic::StarKind::Song,
            StarKind::Album => ::subsonic::StarKind::Album,
            StarKind::Artist => ::subsonic::StarKind::Artist,
        };
        Ok(self.client.set_starred(kind, id, on).await?)
    }

    async fn set_rating(&self, id: &str, rating: u8) -> Result<(), Failure> {
        Ok(self.client.set_rating(id, rating).await?)
    }

    async fn create_playlist(&self, name: &str, song_ids: &[String]) -> Result<(String, String), Failure> {
        let p = self.client.create_playlist(name, song_ids).await?;
        Ok((p.playlist.id, p.playlist.name))
    }

    async fn edit_playlist(&self, id: &str, edit: &PlaylistEdit) -> Result<(), Failure> {
        Ok(match edit {
            PlaylistEdit::Rename(name) => self.client.update_playlist(id, Some(name), &[], &[]).await,
            PlaylistEdit::Add(songs) => self.client.update_playlist(id, None, songs, &[]).await,
            PlaylistEdit::Remove(at) => self.client.update_playlist(id, None, &[], at).await,
            PlaylistEdit::Replace(songs) => self.client.replace_playlist(id, songs).await,
            PlaylistEdit::Delete => self.client.delete_playlist(id).await,
        }?)
    }

    async fn scrobble(&self, id: &str, time_ms: u64, submission: bool) -> Result<(), Failure> {
        Ok(self.client.scrobble(id, time_ms, submission).await?)
    }

    async fn cover(&self, cover_id: &str, size: u32) -> Result<Vec<u8>, Failure> {
        Ok(self.client.cover_art(cover_id, Some(size)).await?)
    }

    fn stream(&self, track: &Track, transcode: Option<&audio::Transcode>) -> (String, Option<String>) {
        match transcode {
            None => (self.client.stream_url(&track.id).to_string(), track.suffix.clone()),
            Some(t) => (self.client.transcoded_stream_url(&track.id, &t.format, t.kbps).to_string(), Some(t.format.clone())),
        }
    }

    fn download_url(&self, id: &str) -> Option<String> {
        Some(self.client.stream_url(id).to_string())
    }
}

#[async_trait]
impl Fetch for SubsonicSource {
    fn artist_mbids(&self) -> Vec<(String, String)> {
        self.artist_mbids.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    /// Takes what's been seen since the last call: each save merges into
    /// the stored table, so one album's fetch writes only that album's ids.
    fn copy_mbids(&self) -> super::mbids::CopyMbids {
        std::mem::take(&mut *self.copy_mbids.lock().unwrap_or_else(|e| e.into_inner()))
    }
    async fn album_page(&self, size: u32, offset: u32) -> Result<Vec<Album>, Failure> {
        let albums = self.client.album_list(AlbumListType::AlphabeticalByArtist, size, offset).await?;
        self.remember_albums(&albums);
        Ok(albums.into_iter().map(convert::album).collect())
    }

    async fn artists(&self) -> Result<Vec<Artist>, Failure> {
        let artists = self.client.artists().await?;
        *self.artist_mbids.lock().unwrap_or_else(|e| e.into_inner()) =
            artists.iter().filter_map(|a| Some((a.id.clone(), crate::source::artists::clean_mbid(a.music_brainz_id.as_deref())?))).collect();
        Ok(artists.into_iter().map(convert::artist).collect())
    }

    async fn album(&self, id: String) -> Result<AlbumTracks, Failure> {
        Ok(convert::album_tracks(self.read_album(&id).await?))
    }

    async fn all_songs(&self, query: &'static str, count: u32, offset: u32) -> Result<SongPage, Failure> {
        let songs = self.client.search_songs(query, count, offset).await?;
        self.remember_songs(&songs);
        Ok(SongPage { scanned: songs.len() as u32, tracks: convert::bulk_tracks(songs) })
    }
}

#[cfg(test)]
pub(crate) fn test_source() -> super::Src {
    Arc::new(SubsonicSource::new(crate::test_tracks::account(), Client::new("http://127.0.0.1:9", ::subsonic::Credentials::new("u", "p")).unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use testkit::{Answer, JsonFake};

    fn ok(fields: Value) -> Answer {
        let mut response = json!({"status": "ok", "version": "1.16.1"});
        response.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
        Answer::ok(json!({"subsonic-response": response}).to_string())
    }

    fn failed() -> Answer {
        Answer::ok(json!({"subsonic-response": {"status": "failed", "version": "1.16.1", "error": {"code": 70, "message": "unsupported"}}}).to_string())
    }

    fn source(fake: &JsonFake) -> SubsonicSource {
        SubsonicSource::new(crate::test_tracks::account(), Client::new(fake.base(), ::subsonic::Credentials::new("u", "p")).unwrap())
    }

    #[tokio::test]
    async fn lyrics_by_song_win_without_asking_by_artist_and_title() {
        let fake = JsonFake::start(|r| {
            assert_eq!(r.path(), "/rest/getLyricsBySongId");
            ok(json!({"lyricsList": {"structuredLyrics": [{"synced": true, "line": [{"start": 1000, "value": "The song's words"}]}]}}))
        });
        let got = source(&fake).lyrics("song", Some("artist"), "title").await.unwrap();
        assert_eq!(got, Some(Lyrics::Synced(vec![lyrics::Line { start_ms: 1000, text: "The song's words".into() }])));
        assert_eq!(fake.seen().len(), 1);
    }

    #[tokio::test]
    async fn missing_empty_or_failed_song_lyrics_fall_back_to_artist_and_title() {
        for structured in [Some(json!({"lyricsList": {}})), Some(json!({"lyricsList": {"structuredLyrics": [{"line": [{"value": "Embed"}]}]}})), None] {
            let fake = JsonFake::start(move |r| match r.path() {
                "/rest/getLyricsBySongId" => structured.clone().map_or_else(failed, ok),
                "/rest/getLyrics" => ok(json!({"lyrics": {"value": "Fallback words"}})),
                _ => panic!("unexpected request"),
            });
            assert_eq!(source(&fake).lyrics("song", Some("The Artist"), "The Title").await.unwrap(), Some(Lyrics::Plain(vec!["Fallback words".into()])));
            let seen = fake.seen();
            assert_eq!(seen.iter().map(|r| r.path()).collect::<Vec<_>>(), ["/rest/getLyricsBySongId", "/rest/getLyrics"]);
            let first = reqwest::Url::parse(&format!("{}{}", fake.base().trim_end_matches('/'), seen[0].target)).unwrap();
            assert!(first.query_pairs().any(|(k, v)| k == "id" && v == "song"));
            let fallback = reqwest::Url::parse(&format!("{}{}", fake.base().trim_end_matches('/'), seen[1].target)).unwrap();
            assert!(fallback.query_pairs().any(|(k, v)| k == "artist" && v == "The Artist"));
            assert!(fallback.query_pairs().any(|(k, v)| k == "title" && v == "The Title"));
        }
    }

    #[tokio::test]
    async fn lyrics_failures_are_none_and_no_artist_means_no_fallback() {
        let fake = JsonFake::start(|_| failed());
        let source = source(&fake);
        assert_eq!(source.lyrics("song", Some("artist"), "title").await.unwrap(), None);
        assert_eq!(fake.seen().len(), 2);
        assert_eq!(source.lyrics("song", None, "title").await.unwrap(), None);
        assert_eq!(fake.count("/rest/getLyricsBySongId"), 2);
        assert_eq!(fake.count("/rest/getLyrics"), 1);
    }

    #[tokio::test]
    async fn album_and_playlist_answers_keep_stars_and_zero_ratings() {
        let song = json!({"id": "song", "title": "Title", "starred": "today"});
        let fake = JsonFake::start(move |r| match r.path() {
            "/rest/getAlbum" => ok(json!({"album": {"id": "album", "name": "Album", "starred": "today", "song": [song]}})),
            "/rest/getPlaylist" => ok(json!({"playlist": {"id": "list", "name": "List", "entry": [song]}})),
            _ => panic!("unexpected request"),
        });
        let source = source(&fake);
        let (album, meta) = MusicSource::album(&source, "album").await.unwrap();
        assert_eq!(album.tracks[0].album_id, "album");
        assert_eq!(meta.starred, [(StarKind::Song, "song".into(), true), (StarKind::Album, "album".into(), true)]);
        assert_eq!(meta.ratings, [("song".into(), 0)]);
        let (tracks, meta) = source.playlist("list").await.unwrap();
        assert_eq!(tracks[0].album_id, "");
        assert_eq!(meta.starred, [(StarKind::Song, "song".into(), true)]);
        assert_eq!(meta.ratings, [("song".into(), 0)]);
    }

    #[tokio::test]
    async fn starred_answers_keep_only_positive_ratings_from_songs_and_albums() {
        let fake = JsonFake::start(|_| {
            ok(json!({"starred2": {
                "song": [{"id": "s", "title": "Song", "userRating": 4}, {"id": "zero", "title": "Zero", "userRating": 0}],
                "album": [{"id": "a", "name": "Album", "userRating": 3}], "artist": [{"id": "ar", "name": "Artist"}]
            }}))
        });
        let (starred, ratings) = source(&fake).starred().await.unwrap();
        assert_eq!(starred.songs.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s", "zero"]);
        assert_eq!(starred.albums, ["a"]);
        assert_eq!(starred.artists, [("ar".into(), "Artist".into())]);
        assert_eq!(ratings, [("s".into(), 4), ("a".into(), 3)]);
    }
    #[tokio::test]
    async fn songs_without_albums_do_not_shorten_a_bulk_page_or_change_its_offset() {
        let fake = JsonFake::start(|r| {
            assert_eq!(r.path(), "/rest/search3");
            let url = reqwest::Url::parse(&format!("http://localhost{}", r.target)).unwrap();
            let offset: u32 = url.query_pairs().find(|(k, _)| k == "songOffset").unwrap().1.parse().unwrap();
            let songs: Vec<_> = match offset {
                0 => (0..500)
                    .map(|n| {
                        if n == 0 { json!({"id": "orphan", "title": "No album"}) } else { json!({"id": format!("s{n}"), "title": "Song", "albumId": "album"}) }
                    })
                    .collect(),
                500 => vec![json!({"id": "last", "title": "Last", "albumId": "album"})],
                _ => panic!("unexpected offset"),
            };
            ok(json!({"searchResult3": {"song": songs}}))
        });
        let lib = Arc::new(library::Library::in_memory().unwrap());
        lib.upsert_albums([crate::sync::tests::album("album", 500)]).unwrap();
        lib.set_meta("tracks_bulk_offset", Some(0)).unwrap();
        let outcome = crate::sync::run(lib.clone(), source(&fake).paged().unwrap(), Arc::default(), || true, |_| {}).await;
        assert_eq!(outcome, crate::sync::Outcome::Done { fetched: 0, bulk: true });
        assert_eq!(fake.seen().len(), 2);
        assert_eq!(lib.album_tracks("album").unwrap().len(), 500);
        assert!(lib.album_tracks("album").unwrap().iter().any(|t| t.id == "last"));
    }
    #[tokio::test]
    async fn album_fetches_and_bulk_songs_collect_clean_copy_ids_separately_from_rows() {
        let fake = JsonFake::start(|r| match r.path() {
            "/rest/getAlbumList2" => ok(json!({"albumList2":{"album":[{"id":"a", "name":"Record", "musicBrainzId":" release "}]}})),
            "/rest/getAlbum" => {
                ok(json!({"album":{"id":"a", "name":"Record", "musicBrainzId":"release", "song":[{"id":"s", "title":"Song", "musicBrainzId":" recording "}]}}))
            }
            "/rest/search3" => ok(
                json!({"searchResult3":{"song":[{"id":"bulk", "albumId":"a", "title":"Bulk song", "musicBrainzId":" bulk-recording "}, {"id":"blank", "albumId":"a", "title":"Blank", "musicBrainzId":"  "}]}}),
            ),
            _ => panic!("unexpected request"),
        });
        let source = source(&fake);
        assert_eq!(source.album_page(500, 0).await.unwrap().len(), 1);
        assert_eq!(Fetch::album(&source, "a".into()).await.unwrap().tracks.len(), 1);
        assert_eq!(source.all_songs("", 500, 0).await.unwrap().tracks.len(), 2);
        let library = library::Library::in_memory().unwrap();
        source.copy_mbids().save(&library).unwrap();
        let albums = library.saved::<std::collections::BTreeMap<String, String>>(super::super::mbids::ALBUMS).unwrap().unwrap();
        let songs = library.saved::<std::collections::BTreeMap<String, String>>(super::super::mbids::RECORDINGS).unwrap().unwrap();
        assert_eq!(albums["a"], "release");
        assert_eq!(songs["s"], "recording");
        assert_eq!(songs["bulk"], "bulk-recording");
        assert!(!songs.contains_key("blank"));
        assert!(library.albums().unwrap().is_empty());
        assert!(library.all_tracks().unwrap().is_empty());
    }
}
