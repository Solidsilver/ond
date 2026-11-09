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
    Open { generation: u64, key: ItemKey, at: Option<f64> },
    Report { generation: u64, update: ProgressUpdate, close: bool },
    Refresh { generation: u64 },
    Reconcile,
    Finish(oneshot::Sender<()>),
}

pub use crate::player::Opened;

pub enum Event {
    Opened { generation: u64, result: Result<Box<Opened>, Failure> },
    Refreshed { generation: u64, result: Result<Vec<sources::SessionTrack>, Failure> },
    Positions(Vec<Progress>),
    Error(Failure),
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
            let id = if let Request::Report { update, .. } = &request {
                match stored.append_progress(update) {
                    Ok(id) => Some(id),
                    Err(error) => {
                        errors(Event::Error(local_error(error)));
                        continue;
                    }
                }
            } else {
                None
            };
            if ready.send(Envelope { request, id }).is_err() {
                break;
            }
        }
    });
    rt.spawn(async move {
        let mut worker = Worker { source, shelf, device, session: None };
        let mut received = worker.shelf.pending_progress().ok().and_then(|p| p.last().map(|p| p.id)).unwrap_or(0);
        if let Err(e) = worker.replay(None).await && !e.is_network() { notify(Event::Error(e)); }
        match worker.positions().await {
            Ok(positions) => notify(Event::Positions(positions)),
            Err(e) => if !e.is_network() { notify(Event::Error(e)); },
        }
        let period = std::time::Duration::from_secs(30);
        let mut retry = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                request = rx.recv() => {
                    let Some(Envelope { request, id }) = request else { break };
                    match request {
                        Request::Open { generation, key, at } if current() => {
                            let result = worker.open(generation, key, at).await.map(Box::new);
                            notify(Event::Opened { generation, result });
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
                        Request::Reconcile if current() => {
                            match worker.positions().await {
                                Ok(positions) => notify(Event::Positions(positions)),
                                Err(error) if !error.is_network() => notify(Event::Error(error)),
                                Err(_) => {},
                            }
                        }
                        Request::Finish(done) => {
                            let _ = worker.close().await;
                            let _ = done.send(());
                            break;
                        }
                        _ => {}
                    }
                }
                _ = retry.tick() => {
                    if !current() { continue; }
                    if let Err(e) = worker.replay(Some(received.saturating_add(1))).await && !e.is_network() { notify(Event::Error(e)); }
                }
            }
        }
    });
    Listening(tx)
}

impl Worker {
    async fn positions(&self) -> Result<Vec<Progress>, Failure> {
        let remote = self.source.progress().await.map_err(Failure::from)?;
        remote
            .into_iter()
            .map(|p| {
                let local = self.shelf.position(&p.item).map_err(local_error)?;
                Ok(newer(local, Some(p)).expect("a remote position exists"))
            })
            .collect()
    }

    async fn open(&mut self, generation: u64, key: ItemKey, at: Option<f64>) -> Result<Opened, Failure> {
        // A previous book is always closed before another play opens on the
        // same device. A network failure cannot discard its durable report.
        let _ = self.close().await;
        let item = self.source.item(&key.item).await.map_err(Failure::from)?;
        self.shelf.set_details(&key, &item).map_err(local_error)?;
        let remote = self.source.progress().await.map_err(Failure::from)?.into_iter().find(|p| p.item == key);
        let local = self.shelf.position(&key).map_err(local_error)?;
        let local_is_newer = local.as_ref().is_some_and(|local| remote.as_ref().is_none_or(|remote| local.at > remote.at));
        let session = self.source.open(&key.item, &self.device).await.map_err(Failure::from)?;
        let position =
            at.unwrap_or_else(|| if local_is_newer { local.map_or(0.0, |p| if p.finished { 0.0 } else { p.position.secs() }) } else { session.start.secs() });
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
            self.source.set_progress(&updates).await.map_err(Failure::from)?;
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
                    self.source.set_progress(std::slice::from_ref(&update)).await.map_err(Failure::from)?;
                }
            } else {
                self.source.set_progress(std::slice::from_ref(&update)).await.map_err(Failure::from)?;
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
