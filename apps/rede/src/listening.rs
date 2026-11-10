//! One worker owns the server session and serializes opens, reports and
//! closes. A journal stage saves reports even while a server call stalls. A failed
//! report remains in its ordered log, independent of the in-memory session.

use crate::failure::Failure;
use shelf::Shelf;
use sources::{DeviceInfo, ItemKey, PlaySession, Position, Progress, ProgressUpdate, Source};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

pub enum Request {
    Open {
        generation: u64,
        key: ItemKey,
        at: Option<f64>,
    },
    Report {
        generation: u64,
        update: ProgressUpdate,
        close: bool,
    },
    Refresh {
        generation: u64,
    },
    Reconcile,
    Resume {
        generation: u64,
    },
    /// The app sends this before a focus pull and after playback changes.
    Activity {
        item: Option<ItemKey>,
    },
    Bookmark {
        bookmark: sources::Bookmark,
        change: shelf::BookmarkChange,
    },
    Undo(ProgressUpdate),
    Finish(oneshot::Sender<()>),
}

pub use crate::player::Opened;

pub enum Event {
    Opened { generation: u64, result: Result<Box<Opened>, Failure> },
    Refreshed { generation: u64, result: Result<Vec<sources::SessionTrack>, Failure> },
    Positions(Vec<Progress>),
    ResumeReady { generation: u64 },
    Error(Failure),
    Bookmarks(Result<shelf::BookmarkSnapshot, Failure>),
    Reconciled(Reconciled),
}

/// The UI keeps this until dismissed or undone; the old position is carried
/// explicitly so Undo never depends on a later shelf lookup.
#[derive(Debug, Clone, PartialEq)]
pub struct Reconciled {
    pub previous: Progress,
    pub current: Progress,
}

impl Reconciled {
    pub fn message(&self) -> String {
        let minutes = (self.current.position.secs().max(0.0) / 60.0) as u64;
        let at = if minutes >= 60 { format!("{}h {}m", minutes / 60, minutes % 60) } else { format!("{minutes}m") };
        format!("Picked up where you left off on another device, {at} in.")
    }
}

#[derive(Clone)]
pub struct Listening(mpsc::UnboundedSender<Request>);

struct Envelope {
    request: Request,
    id: Option<shelf::ProgressId>,
}
impl Listening {
    pub fn send(&self, request: Request) {
        let _ = self.0.send(request);
    }
    pub fn finish(self) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        self.send(Request::Finish(tx));
        rx
    }
}

struct Session {
    generation: u64,
    key: ItemKey,
    session: PlaySession,
    position: Position,
}

struct Worker {
    source: Arc<dyn Source>,
    shelf: Arc<Shelf>,
    device: DeviceInfo,
    session: Option<Session>,
    active: Option<ItemKey>,
}

pub fn spawn(
    rt: &tokio::runtime::Handle,
    source: Arc<dyn Source>,
    shelf: Arc<Shelf>,
    device: DeviceInfo,
    current: impl Fn() -> bool + Send + 'static,
    notify: impl Fn(Event) + Send + Sync + 'static,
) -> Listening {
    let (tx, mut journal) = mpsc::unbounded_channel();
    let (ready, mut rx) = mpsc::unbounded_channel();
    let notify = Arc::new(notify);
    let stored = shelf.clone();
    let errors = notify.clone();
    rt.spawn(async move {
        while let Some(request) = journal.recv().await {
            let saved = match &request {
                Request::Report { update, .. } | Request::Undo(update) => stored.append_progress(update).map(Some),
                Request::Bookmark { bookmark, change } => stored.queue_bookmark(bookmark, *change).map(|_| None),
                _ => Ok(None),
            };
            let id = match saved {
                Ok(id) => id,
                Err(error) => {
                    errors(Event::Error(local_error(error)));
                    continue;
                }
            };
            if matches!(request, Request::Bookmark { .. }) {
                errors(Event::Bookmarks(stored.bookmark_snapshot().map_err(local_error)));
            }
            if ready.send(Envelope { request, id }).is_err() {
                break;
            }
        }
    });
    rt.spawn(async move {
        let mut worker = Worker { source, shelf, device, session: None, active: None };
        notify(Event::Bookmarks(worker.shelf.bookmark_snapshot().map_err(local_error)));
        let mut received = worker.shelf.pending_progress().ok().and_then(|p| p.last().map(|p| p.id)).unwrap_or(0);
        if current() { worker.pull(None, &*notify).await; }
        let mut retry_delay = RETRY_DELAY;
        let mut retry_at = tokio::time::Instant::now() + retry_delay;
        loop {
            tokio::select! {
                biased;
                request = rx.recv() => {
                    let Some(Envelope { request, id }) = request else { break };
                    match request {
                        Request::Open { generation, key, at } if current() => {
                            // Opening must not wait for bookmark sync: the book
                            // starts playing as soon as its position is known.
                            let failed = worker.pull_positions(Some(received.saturating_add(1)), &*notify).await;
                            let result = worker.open(generation, key, at).await.map(Box::new);
                            notify(Event::Opened { generation, result });
                            if !failed {
                                let _ = sync_bookmarks(&*worker.source, &worker.shelf, &*notify).await;
                            }
                        }
                        Request::Refresh { generation } if current() => {
                            if let Some(session) = &mut worker.session && session.generation == generation {
                                let result = worker.source.refresh_tracks(&session.session).await.map_err(Failure::from);
                                if let Ok(tracks) = &result { session.session.tracks.clone_from(tracks); }
                                notify(Event::Refreshed { generation, result });
                            }
                        }
                        Request::Report { generation, update, close } => {
                            let id = id.expect("the journal saved every report");
                            received = id;
                            if let Err(error) = worker.report(id, generation, update.clone(), close).await && !error.is_network() {
                                notify(Event::Error(error));
                            }
                            notify(Event::Positions(vec![Progress { item: update.item, position: update.position, finished: update.finished, at: Some(update.at) }]));
                        }
                        Request::Activity { item } if current() => { worker.active = item; }
                        Request::Bookmark { .. } if current() => {
                            let _ = sync_bookmarks(&*worker.source, &worker.shelf, &*notify).await;
                        }
                        Request::Undo(update) if current() => {
                            let id = id.expect("Undo is durable");
                            received = id;
                            // Undo may restore an unfinished position over a
                            // finished row, so it needs the server's current
                            // list to decide whether a reset PATCH comes first.
                            let result = match worker.source.progress().await.map_err(Failure::from) {
                                Ok(remote) => worker.source.set_progress(std::slice::from_ref(&update), &remote).await.map_err(Failure::from),
                                Err(e) => Err(e),
                            };
                            match result {
                                Ok(()) => { if let Err(e) = worker.shelf.mark_synced(&[id]) { notify(Event::Error(local_error(e))); } }
                                Err(e) => notify(Event::Error(e)),
                            }
                            if let Some(session) = &mut worker.session && session.key == update.item { session.position = update.position; }
                            notify(Event::Positions(vec![Progress { item: update.item, position: update.position, finished: update.finished, at: Some(update.at) }]));
                        }
                        Request::Resume { generation } if current() => {
                            // ResumeReady comes straight after the positions:
                            // the app waits on it, and bookmark sync must not
                            // hold playback up.
                            let failed = worker.pull_positions(Some(received.saturating_add(1)), &*notify).await;
                            notify(Event::ResumeReady { generation });
                            if !failed {
                                let _ = sync_bookmarks(&*worker.source, &worker.shelf, &*notify).await;
                            }
                        }
                        Request::Reconcile if current() => {
                            worker.pull(Some(received.saturating_add(1)), &*notify).await;
                        }
                        Request::Finish(done) => {
                            let _ = worker.close().await;
                            let _ = done.send(());
                            break;
                        }
                        _ => {}
                    }
                }
                _ = tokio::time::sleep_until(retry_at) => {
                    let mut failed = false;
                    if current() {
                        if let Err(e) = worker.replay(Some(received.saturating_add(1))).await {
                            failed = true;
                            notify(Event::Error(e));
                        }
                        // Only queued bookmark edits are retried here: a pull already
                        // asks when the window is focused, so an idle app asks nothing.
                        match worker.shelf.pending_bookmarks() {
                            Ok(pending) if !pending.is_empty() => {
                                failed |= sync_bookmarks(&*worker.source, &worker.shelf, &*notify).await;
                            }
                            Ok(_) => {}
                            Err(e) => notify(Event::Error(local_error(e))),
                        }
                    }
                    retry_delay = next_retry_delay(retry_delay, failed);
                    retry_at = tokio::time::Instant::now() + retry_delay;
                }
            }
        }
    });
    Listening(tx)
}

/// The first pause after a failed retry, and the longest it grows to. A
/// permanent refusal is dropped by `sync::bookmarks`, so only a network or 5xx
/// failure keeps backing off.
const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(30);
const MAX_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(10 * 60);

fn next_retry_delay(current: std::time::Duration, failed: bool) -> std::time::Duration {
    if failed { (current * 2).min(MAX_RETRY_DELAY) } else { RETRY_DELAY }
}

/// Reports a bookmark sync's snapshot and, when an edit was permanently
/// refused and dropped, that refusal exactly once. Returns whether the failure
/// is worth retrying, so only a network or 5xx answer backs the retry off.
async fn sync_bookmarks(source: &dyn Source, shelf: &Shelf, notify: &(dyn Fn(Event) + Sync)) -> bool {
    match crate::sync::bookmarks(source, shelf).await {
        Ok(sync) => {
            notify(Event::Bookmarks(Ok(sync.snapshot)));
            if let Some(error) = sync.refused {
                notify(Event::Error(error));
            }
            false
        }
        Err(error) => {
            notify(Event::Bookmarks(Err(error)));
            true
        }
    }
}

impl Worker {
    async fn pull(&mut self, before: Option<shelf::ProgressId>, notify: &(dyn Fn(Event) + Sync)) {
        let network_failed = self.pull_positions(before, notify).await;
        // A pull that couldn't reach the server skips the bookmark step: the
        // next answer is likely the same, and resume must not wait for it.
        if !network_failed {
            sync_bookmarks(&*self.source, &self.shelf, notify).await;
        }
    }

    /// Replays the log and takes the server's positions. Returns whether a
    /// network failure happened on the way, which makes the bookmark step
    /// pointless for this pull.
    async fn pull_positions(&mut self, before: Option<shelf::ProgressId>, notify: &(dyn Fn(Event) + Sync)) -> bool {
        let mut network_failed = false;
        match self.replay(before).await {
            Ok(()) => match self.positions(notify).await {
                Ok(positions) => notify(Event::Positions(positions)),
                Err(e) => {
                    network_failed = e.is_network();
                    notify(Event::Error(e));
                }
            },
            Err(e) => {
                network_failed = e.is_network();
                notify(Event::Error(e));
            }
        }
        network_failed
    }

    async fn positions(&mut self, notify: &(dyn Fn(Event) + Sync)) -> Result<Vec<Progress>, Failure> {
        let remote_list = self.source.progress().await.map_err(Failure::from)?;
        let mut positions = Vec::new();
        for remote in remote_list.clone() {
            let local = self.shelf.position(&remote.item).map_err(local_error)?;
            // Moving audio under an active listener would interrupt this device.
            // It reports its own position; paused books may pick up the phone.
            let active = self.active.as_ref() == Some(&remote.item);
            let local_wins = local.as_ref().is_some_and(|p| active || p.at > remote.at);
            if local_wins {
                let local = local.unwrap();
                // Only the device actually playing may overwrite the server.
                // A paused local position with a fast clock would otherwise win
                // every pull and erase what the phone listened to; replay has
                // already sent anything genuinely unsent.
                if active && (local.at > remote.at || local.position != remote.position || local.finished != remote.finished) {
                    self.source
                        .set_progress(
                            &[ProgressUpdate {
                                item: local.item.clone(),
                                position: local.position,
                                finished: local.finished,
                                at: local.at.unwrap_or(std::time::SystemTime::now()),
                                listened: std::time::Duration::ZERO,
                            }],
                            &remote_list,
                        )
                        .await
                        .map_err(Failure::from)?;
                }
                positions.push(local);
            } else if !active && self.shelf.accept_position(&remote).map_err(local_error)? {
                // A toast and Undo only make sense for the book the player has
                // loaded. A book started on the phone but never opened here is
                // accepted silently: there is no local position to offer back.
                let loaded = self.session.as_ref().is_some_and(|session| session.key == remote.item);
                if loaded
                    && let Some(previous) = &local
                    && remote.at > previous.at
                    && (remote.position.secs() - previous.position.secs()).abs() > 60.0
                {
                    notify(Event::Reconciled(Reconciled { previous: previous.clone(), current: remote.clone() }));
                }
                if let Some(session) = &mut self.session
                    && session.key == remote.item
                {
                    session.position = remote.position;
                }
                positions.push(remote);
            } else if let Some(local) = self.shelf.position(&remote.item).map_err(local_error)? {
                positions.push(local);
            }
        }
        Ok(positions)
    }

    async fn open(&mut self, generation: u64, key: ItemKey, at: Option<f64>) -> Result<Opened, Failure> {
        // A previous book is always closed before another play opens on the
        // same device. A network failure cannot discard its durable report.
        let _ = self.close().await;
        let item = self.source.item(&key.item).await.map_err(Failure::from)?;
        self.shelf.set_details(&key, &item).map_err(local_error)?;
        let local = self.shelf.position(&key).map_err(local_error)?;
        let session = self.source.open(&key.item, &self.device).await.map_err(Failure::from)?;
        let position = at.unwrap_or_else(|| local.map_or(session.start.secs(), |p| if p.finished { 0.0 } else { p.position.secs() }));
        self.session = Some(Session { generation, key: key.clone(), session: session.clone(), position: Position::from_secs(position) });
        Ok(Opened { key, item, session, position })
    }

    /// ABS blindly overwrites progress, so only the newest local position for
    /// each item may follow the timestamp comparison onto the server.
    async fn replay(&self, before: Option<shelf::ProgressId>) -> Result<(), Failure> {
        let pending = self.shelf.pending_progress().map_err(local_error)?;
        let pending: Vec<_> = pending.into_iter().take_while(|p| before.is_none_or(|id| p.id < id)).collect();
        if pending.is_empty() {
            return Ok(());
        }
        let remote = self.source.progress().await.map_err(Failure::from)?;
        let mut newest = HashMap::new();
        for entry in &pending {
            let retained = newest.entry(&entry.update.item).or_insert(entry);
            if (entry.update.at, entry.id) > (retained.update.at, retained.id) {
                *retained = entry;
            }
        }
        let updates: Vec<_> = newest
            .values()
            .filter(|entry| !remote.iter().any(|p| p.item == entry.update.item && p.at.is_some_and(|at| at > entry.update.at)))
            .map(|entry| entry.update.clone())
            .collect();
        // M4 will send offline listening as ABS local sessions for statistics;
        // progress PATCH carries only the latest position, not listening time.
        if !updates.is_empty() {
            self.source.set_progress(&updates, &remote).await.map_err(Failure::from)?;
        }
        self.shelf.mark_synced(&pending.iter().map(|entry| entry.id).collect::<Vec<_>>()).map_err(local_error)
    }

    async fn report(&mut self, id: shelf::ProgressId, generation: u64, update: ProgressUpdate, close: bool) -> Result<(), Failure> {
        let sent = async {
            self.replay(Some(id)).await?;
            if let Some(session) = &mut self.session
                && session.generation == generation
                && session.key == update.item
            {
                session.position = update.position;
                let result = self.source.report(&session.session, update.position, update.listened).await;
                if result.as_ref().is_err_and(|e| e.status() == Some(404)) {
                    session.session = self.source.open(&session.key.item, &self.device).await.map_err(Failure::from)?;
                    self.source.report(&session.session, update.position, update.listened).await.map_err(Failure::from)?;
                } else {
                    result.map_err(Failure::from)?;
                }
                if update.finished {
                    // A finished update never needs a reset, so it has no use
                    // for the server's progress list.
                    self.source.set_progress(std::slice::from_ref(&update), &[]).await.map_err(Failure::from)?;
                }
            } else if update.finished {
                self.source.set_progress(std::slice::from_ref(&update), &[]).await.map_err(Failure::from)?;
            } else {
                // No session to report through: the update goes straight to the
                // progress endpoint, which may need the server's list to clear
                // a finish first.
                let remote = self.source.progress().await.map_err(Failure::from)?;
                self.source.set_progress(std::slice::from_ref(&update), &remote).await.map_err(Failure::from)?;
            }
            self.shelf.mark_synced(&[id]).map_err(local_error)
        }
        .await;
        if let Some(session) = &mut self.session
            && session.generation == generation
        {
            session.position = update.position;
        }
        if close && self.session.as_ref().is_some_and(|s| s.generation == generation) {
            let closed = self.close().await;
            sent.and(closed)
        } else {
            sent
        }
    }

    async fn close(&mut self) -> Result<(), Failure> {
        let Some(session) = self.session.take() else { return Ok(()) };
        match self.source.close(&session.session, session.position).await {
            Err(e) if e.status() == Some(404) => Ok(()),
            result => result.map_err(Failure::from),
        }
    }
}

fn local_error(e: shelf::Error) -> Failure {
    Failure::Local(format!("Couldn't save listening progress: {e}"))
}

pub fn newer(local: Option<Progress>, remote: Option<Progress>) -> Option<Progress> {
    match (local, remote) {
        (Some(local), Some(remote)) => Some(if local.at > remote.at { local } else { remote }),
        (local, remote) => local.or(remote),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod live;
