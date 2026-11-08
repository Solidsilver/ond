//! Audiobookshelf, through the `abs` client.
//!
//! Everything server-specific is in this file: the JSON field names (in
//! `abs`), the ways ABS differs from the models in [`crate::model`], and the
//! mapping between the two. The app only sees the models.
//!
//! Three of ABS's habits shape the mapping:
//!
//! - **No "changed since" filter.** `GET /api/libraries/:id/items` has no way
//!   to ask what's new, so [`AbsSource::items`] pages the whole minified
//!   listing and keeps what is newer than `since`. That's cheap: the listing
//!   is paged and carries no tracks.
//! - **Two spellings of the same metadata.** A listing has `authorName` and
//!   `seriesName` as joined strings ("Doe, John, Smith, Jane", "Series #1,
//!   Other"); an expanded item has `authors` and `series` as objects. The
//!   joined forms are split as best they can be, and the expanded item wins
//!   whenever it's fetched.
//! - **Podcasts are items too.** They're left out for now: a
//!   [`LibraryKind::Podcasts`] library is listed
//!   (so the app knows it exists) but its items come back empty. That's one
//!   filter here, in [`summary`], rather than a half-built podcast path
//!   everywhere.

use std::time::Duration;

use abs::MediaType;
use async_trait::async_trait;
use bytes::Bytes;

use crate::error::{Error, Result};
use crate::id::{AccountId, ItemKey};
use crate::model::{
    Capabilities, Changes, Chapter, DeviceInfo, Item, ItemSummary, Library, LibraryId, LibraryKind, PlaySession, Position, Progress, ProgressUpdate, Series,
    SessionTrack, Stamp, TrackFile,
};
use crate::source::Source;

/// Items per listing page. ABS returns everything at once for a `limit` of 0,
/// which for a large library is a lot of JSON; 200 is a few requests for most
/// libraries and still a small reply.
const PAGE_SIZE: u32 = 200;

pub struct AbsSource {
    account: AccountId,
    client: abs::Client,
    /// What the player decodes. Sent with every play session: without it, or
    /// without `forceDirectPlay`, the server starts an HLS transcode instead
    /// of handing over the files.
    mime_types: Vec<String>,
}

impl AbsSource {
    /// Wraps a signed-in client as the app's view of that account.
    /// `supported_mime_types` is what the audio engine can decode.
    pub fn new(account: AccountId, client: abs::Client, supported_mime_types: Vec<String>) -> Self {
        Self { account, client, mime_types: supported_mime_types }
    }

    pub fn account(&self) -> AccountId {
        self.account
    }

    /// The client underneath, for signing out and for rebuilding URLs after a
    /// token refresh.
    pub fn client(&self) -> &abs::Client {
        &self.client
    }
}

#[async_trait]
impl Source for AbsSource {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            // The app's search runs over the local shelf; the client has no
            // call for the server's.
            server_search: false,
            // ABS keeps bookmarks, but the trait has no calls for them yet, so
            // claiming them would offer the user something with no wiring.
            bookmarks: false,
            // socket.io isn't spoken yet; progress is pulled on start, on
            // focus and before playing.
            live_updates: false,
            // Podcast libraries are listed but their items are skipped.
            subscriptions: false,
            // The server can transcode, though Rede always asks for direct
            // play: a book's files play as they are.
            transcoding: true,
        }
    }

    async fn logout(&self) -> Result<()> {
        Ok(self.client.logout().await?)
    }

    async fn libraries(&self) -> Result<Vec<Library>> {
        Ok(self.client.libraries().await?.into_iter().filter_map(library).collect())
    }

    async fn items(&self, library: &LibraryId, since: Option<Stamp>) -> Result<Changes<ItemSummary>> {
        let mut changes = Changes::new(self.account);
        let mut listed: u64 = 0;
        let mut page: u32 = 0;
        loop {
            let reply = self.client.library_items(library, page, PAGE_SIZE).await?;
            let on_page = reply.results.len() as u64;
            listed += on_page;
            let more = match reply.total {
                Some(total) => listed < total,
                // Without a total, a short page is the last one.
                None => on_page == u64::from(PAGE_SIZE),
            };
            for item in reply.results {
                let updated = item.updated_at.map(Stamp::from_millis);
                // The stamp covers everything on the listing, including the
                // items this build leaves out: a podcast episode moving on
                // mustn't make the next sync look for books again.
                changes.stamp = changes.stamp.max(updated);
                let Some(summary) = summary(&item, library) else { continue };
                let keep = match (since, summary.updated) {
                    (None, _) => true,
                    (Some(since), Some(updated)) => updated > since,
                    // No stamp: it can't be proven unchanged, so it's kept.
                    (Some(_), None) => true,
                };
                if keep {
                    changes.items.push(summary);
                }
            }
            if !more || on_page == 0 {
                break;
            }
            page += 1;
        }
        Ok(changes)
    }

    async fn item(&self, id: &str) -> Result<Item> {
        let item = self.client.item(id).await?;
        details(&item).ok_or_else(|| Error::NotSupported(format!("item {id} is a podcast, which this version of Rede doesn't play")))
    }

    async fn cover(&self, id: &str, size: Option<u32>) -> Result<Bytes> {
        Ok(Bytes::from(self.client.cover(id, size).await?))
    }

    async fn open(&self, id: &str, _device: &DeviceInfo) -> Result<PlaySession> {
        // The session's device is the one the client was built with
        // (`abs::Client::device`); the app makes that client, so the argument
        // is only for sources whose sessions take the device per call.
        let played = self.client.play(id, None, &self.mime_types).await?;
        session(id, played, &self.client)
    }

    async fn report(&self, session: &PlaySession, at: Position, listened: Duration) -> Result<()> {
        let sync = abs::SessionSync { current_time: at.secs(), time_listened: listened.as_secs_f64(), duration: session.duration };
        Ok(self.client.sync_session(&session.id, &sync).await?)
    }

    async fn close(&self, session: &PlaySession, at: Position) -> Result<()> {
        // The listening time is not reported again: it was reported as it
        // happened, and closing only writes the last position.
        let sync = abs::SessionSync { current_time: at.secs(), time_listened: 0.0, duration: session.duration };
        Ok(self.client.close_session(&session.id, Some(&sync)).await?)
    }

    async fn progress(&self) -> Result<Vec<Progress>> {
        Ok(self.client.progress().await?.iter().filter_map(|p| progress(&self.account, p)).collect())
    }

    async fn set_progress(&self, batch: &[ProgressUpdate]) -> Result<()> {
        let updates: Vec<abs::ItemProgressUpdate> = batch.iter().map(progress_update).collect();
        Ok(self.client.update_progress_batch(&updates).await?)
    }
}

/// A library in Rede's terms. A media type this build doesn't know has no side
/// of the app to show on, so it's left out rather than guessed at.
fn library(library: abs::Library) -> Option<Library> {
    let kind = match library.media_type? {
        MediaType::Book => LibraryKind::Books,
        MediaType::Podcast => LibraryKind::Podcasts,
        MediaType::Other => {
            tracing::debug!("library {} is of a media type this build doesn't know; leaving it out", library.id);
            return None;
        }
    };
    Some(Library { id: library.id, name: library.name.unwrap_or_default(), kind })
}

/// What a listing said about one item, or `None` for an item this build
/// doesn't show (a podcast, or a media type it has no place for).
/// `fallback_library` is the library being listed, for a server that doesn't
/// name it on every item.
fn summary(item: &abs::LibraryItem, fallback_library: &str) -> Option<ItemSummary> {
    match item.media_type {
        Some(MediaType::Podcast) => return None,
        Some(MediaType::Other) | None => {
            // A library item with no type is a serialisation this build
            // doesn't know; showing a titleless grid entry is worse.
            tracing::debug!("item {} has no usable media type; leaving it out", item.id);
            return None;
        }
        Some(MediaType::Book) => {}
    }
    let media = item.media.as_ref();
    let metadata = media.and_then(|m| m.metadata.as_ref());
    Some(ItemSummary {
        id: item.id.clone(),
        library: item.library_id.clone().unwrap_or_else(|| fallback_library.to_owned()),
        title: metadata.and_then(|m| m.title.clone()).unwrap_or_default(),
        subtitle: metadata.and_then(|m| m.subtitle.clone()),
        authors: match metadata {
            Some(m) if !m.authors.is_empty() => m.authors.iter().filter_map(|p| p.name.clone()).collect(),
            Some(m) => split_joined(m.author_name.as_deref()),
            None => Vec::new(),
        },
        narrators: match metadata {
            Some(m) if !m.narrators.is_empty() => m.narrators.clone(),
            Some(m) => split_joined(m.narrator_name.as_deref()),
            None => Vec::new(),
        },
        series: series(metadata),
        duration: media.and_then(|m| m.duration),
        // ABS serves an item's cover under the item's own id.
        cover: Some(item.id.clone()),
        added: item.added_at.map(Stamp::from_millis),
        updated: item.updated_at.map(Stamp::from_millis),
    })
}

/// An expanded item: the summary plus everything that takes a second request.
fn details(item: &abs::LibraryItem) -> Option<Item> {
    // The expanded item names its library, so there's no fallback to give.
    let summary = summary(item, "")?;
    let media = item.media.as_ref();
    let metadata = media.and_then(|m| m.metadata.as_ref());
    Some(Item {
        description: metadata.and_then(|m| m.description.clone()),
        genres: metadata.map(|m| m.genres.clone()).unwrap_or_default(),
        tags: media.map(|m| m.tags.clone()).unwrap_or_default(),
        published_year: metadata.and_then(|m| m.published_year.clone()),
        publisher: metadata.and_then(|m| m.publisher.clone()),
        language: metadata.and_then(|m| m.language.clone()),
        asin: metadata.and_then(|m| m.asin.clone()),
        isbn: metadata.and_then(|m| m.isbn.clone()),
        tracks: media.map(tracks).unwrap_or_default(),
        chapters: media.map(chapters).unwrap_or_default(),
        summary,
    })
}

/// The files that play, in the order the server gives them (its stored order,
/// `exclude`d files already dropped).
fn tracks(media: &abs::BookMedia) -> Vec<TrackFile> {
    media
        .tracks
        .iter()
        .enumerate()
        .map(|(i, track)| TrackFile {
            // ABS numbers a book's files from 1; the listing keeps its own
            // echo so a missing index doesn't read as track 0.
            index: track.index.unwrap_or(i as u32 + 1),
            start_offset: track.start_offset.unwrap_or(0.0),
            duration: track.duration,
            suffix: track.metadata.as_ref().and_then(|m| m.ext.as_deref()).and_then(extension),
            size: track.size(),
            mime: track.mime_type.clone(),
            // The file's `ino` is what `contentUrl` ends in, and what names
            // the file in the stream cache and the offline store.
            content_id: track.ino.clone().or_else(|| track.content_url.clone()).unwrap_or_default(),
        })
        .collect()
}

fn chapters(media: &abs::BookMedia) -> Vec<Chapter> {
    media
        .chapters
        .iter()
        .filter_map(|c| {
            // A chapter without a start or an end has no place on the
            // timeline; the server's own list has both.
            Some(Chapter { start: c.start?, end: c.end?, title: c.title.clone().unwrap_or_default() })
        })
        .collect()
}

/// How an open session becomes tracks the player can fetch.
fn session(item: &str, played: abs::PlaybackSession, client: &abs::Client) -> Result<PlaySession> {
    let mut tracks = Vec::with_capacity(played.audio_tracks.len());
    for (i, track) in played.audio_tracks.iter().enumerate() {
        let content_url = track.content_url.as_deref().ok_or_else(|| Error::Decode(format!("a session for item {item} has a track with no contentUrl")))?;
        tracks.push(SessionTrack {
            index: track.index.unwrap_or(i as u32 + 1),
            start_offset: track.start_offset.unwrap_or(0.0),
            duration: track.duration,
            size: track.size(),
            mime: track.mime_type.clone(),
            // The absolute URL with the access token in it.
            url: client.file_url(content_url)?.to_string(),
        });
    }
    Ok(PlaySession {
        id: played.id,
        item: item.to_owned(),
        // Where the server says to resume. A finished book starts at 0.
        start: Position::from_secs(played.current_time.unwrap_or(0.0)),
        duration: played.duration,
        tracks,
    })
}

/// The server's progress rows, for books. An episode's row is skipped with the
/// rest of the podcast side.
fn progress(account: &AccountId, row: &abs::MediaProgress) -> Option<Progress> {
    if row.episode_id.is_some() {
        return None;
    }
    let item = row.library_item_id.as_deref()?;
    Some(Progress {
        item: ItemKey::new(*account, item),
        position: Position::from_secs(row.current_time.unwrap_or(0.0)),
        finished: row.is_finished.unwrap_or(false),
        at: row.last_update.map(|ms| Stamp::from_millis(ms).system_time()),
    })
}

fn progress_update(update: &ProgressUpdate) -> abs::ItemProgressUpdate {
    abs::ItemProgressUpdate {
        // Podcasts aren't shown, so no update names an episode.
        episode_id: None,
        library_item_id: update.item.item.clone(),
        update: abs::ProgressUpdate {
            // Without a duration the server can't apply its "within 10
            // seconds of the end" rule; the log's own `finished` is the one
            // that decides.
            duration: None,
            progress: None,
            current_time: Some(update.position.secs()),
            // Only a finish is ever said. `isFinished: false` on a finished
            // item resets its position to 0 and drops the `currentTime` sent
            // with it, so sending it would undo the position being reported;
            // clearing a finish takes two requests, which the app makes when
            // the user asks for it.
            is_finished: update.finished.then_some(true),
            hide_from_continue_listening: None,
            finished_at: None,
            // The server stores this as the progress's own stamp instead of
            // reading its clock, so the log's time is what another device
            // compares against.
            last_update: Some(Stamp::from(update.at).millis()),
        },
    }
}

/// The series a listing names in one string ("Series #1, Other"), or the
/// expanded item's list when it has one.
fn series(metadata: Option<&abs::BookMetadata>) -> Vec<Series> {
    let Some(metadata) = metadata else { return Vec::new() };
    if !metadata.series.is_empty() {
        return metadata.series.iter().filter_map(|s| s.name.clone().map(|name| Series { name, sequence: s.sequence.clone() })).collect();
    }
    split_joined(metadata.series_name.as_deref())
        .into_iter()
        .map(|part| match part.rsplit_once(" #") {
            Some((name, sequence)) if !name.is_empty() => Series { name: name.to_owned(), sequence: Some(sequence.to_owned()) },
            _ => Series { name: part, sequence: None },
        })
        .collect()
}

/// ABS joins names into one string with ", ". Splitting it back is a guess
/// (a surname with a comma reads as two names), and it is the only form the
/// minified listing has; an expanded item carries the names apart.
fn split_joined(joined: Option<&str>) -> Vec<String> {
    joined.unwrap_or_default().split(", ").map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

/// "m4b" from ".m4b". The stream cache and the offline store put it after the
/// id and keep it to letters and digits.
fn extension(ext: &str) -> Option<String> {
    let ext: String = ext.trim_start_matches('.').chars().filter(char::is_ascii_alphanumeric).collect();
    (!ext.is_empty()).then(|| ext.to_ascii_lowercase())
}

#[cfg(test)]
mod tests;
