//! The one thing the app knows about a book's origin.
//!
//! A source is a signed-in account: a server, and the credentials that reach
//! it. The app holds `Arc<dyn Source>` and never checks which implementation
//! it has; what it can't do is hidden by
//! [`Capabilities`], and the failures it can have are
//! [the error type](crate::Error)'s variants. Adding Jellyfin later is a new
//! implementation behind this trait, not a change to the app.
//!
//! The trait is written with [`async_trait`](mod@async_trait) rather than as native
//! `async fn`s in a trait: native ones aren't dyn-compatible without boxing
//! every call by hand, and the app needs one trait object to hold a source of
//! any kind.
//!
//! **Sessions.** Playing a book goes through [`open`](Source::open), which
//! asks the source to remember this device's session, then
//! [`report`](Source::report) while it plays and [`close`](Source::close) at
//! the end. A source with nothing to remember returns a session it can ignore,
//! which is what local folders and feeds will do.
//!
//! **Progress.** [`progress`](Source::progress) is the source's own view, for
//! reconciling with the local log; [`set_progress`](Source::set_progress)
//! writes outstanding log entries back. Deciding which of the two wins is the
//! app's job, because it's the only part that knows the local log.

use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;

use crate::error::Result;
use crate::model::{Capabilities, Changes, DeviceInfo, Item, ItemSummary, Library, LibraryId, PlaySession, Position, Progress, ProgressUpdate, Stamp};

#[async_trait]
pub trait Source: Send + Sync {
    fn capabilities(&self) -> Capabilities;

    /// Ends the account's server login, if it has one. The app must forget
    /// its local credentials even when this fails.
    async fn logout(&self) -> Result<()> {
        Ok(())
    }

    /// Every library this account may read.
    async fn libraries(&self) -> Result<Vec<Library>>;

    /// What changed in `library` since `since`, or everything when it's
    /// `None`.
    ///
    /// A source that can't filter on its side (Audiobookshelf has no "changed
    /// since") lists everything and answers from that, so a change set from a
    /// full listing is the only one that can also report removals.
    async fn items(&self, library: &LibraryId, since: Option<Stamp>) -> Result<Changes<ItemSummary>>;

    /// One item with its tracks, chapters and full metadata.
    async fn item(&self, id: &str) -> Result<Item>;

    /// The cover image, `size` pixels wide when the source can resize.
    async fn cover(&self, id: &str, size: Option<u32>) -> Result<Bytes>;

    /// Starts a play session for `id`, which some sources remember in order
    /// to count listening and to close it again.
    async fn open(&self, id: &str, device: &DeviceInfo) -> Result<PlaySession>;

    /// Rebuilds file URLs after an expired credential was rejected by a
    /// downloader. Sources without expiring URLs need not implement this.
    async fn refresh_tracks(&self, _session: &PlaySession) -> Result<Vec<crate::SessionTrack>> {
        Err(crate::Error::NotSupported("This source cannot refresh file URLs".into()))
    }

    /// The position now, and the time listened since the last report.
    async fn report(&self, session: &PlaySession, at: Position, listened: Duration) -> Result<()>;

    /// Ends the session at `at`.
    async fn close(&self, session: &PlaySession, at: Position) -> Result<()>;

    /// Everything the source remembers, for reconciling with the local log.
    async fn progress(&self) -> Result<Vec<Progress>>;

    /// All bookmarks for this account, when supported.
    async fn bookmarks(&self) -> Result<Vec<crate::Bookmark>> {
        Err(crate::Error::NotSupported("This source cannot sync bookmarks".into()))
    }
    async fn create_bookmark(&self, _item: &str, _time: Position, _title: &str) -> Result<crate::Bookmark> {
        Err(crate::Error::NotSupported("This source cannot create bookmarks".into()))
    }
    /// Changes only the title; changing time requires delete followed by create.
    async fn rename_bookmark(&self, _item: &str, _time: Position, _title: &str) -> Result<crate::Bookmark> {
        Err(crate::Error::NotSupported("This source cannot rename bookmarks".into()))
    }
    async fn delete_bookmark(&self, _item: &str, _time: Position) -> Result<()> {
        Err(crate::Error::NotSupported("This source cannot delete bookmarks".into()))
    }

    /// Writes log entries back to the source, oldest first.
    async fn set_progress(&self, batch: &[ProgressUpdate]) -> Result<()>;
}
