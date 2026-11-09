use super::*;
use async_trait::async_trait;
use sources::{AccountId, Capabilities, Changes, Item, ItemSummary, Library, LibraryId, SessionTrack, Stamp};
use std::collections::VecDeque;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime};

type Result<T> = sources::Result<T>;

#[derive(Debug, Clone, PartialEq)]
enum Call {
    Open(String),
    Report(f64, Duration),
    Patch(f64, bool),
    Close(f64),
}

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<Call>>,
    report_errors: Mutex<VecDeque<sources::Error>>,
    patch_fails: AtomicBool,
    batches: Mutex<Vec<Vec<f64>>>,
    remote: Mutex<Vec<Progress>>,
    blocked: AtomicBool,
    release: tokio::sync::Notify,
}
impl Fake {
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}
#[async_trait]
impl Source for Fake {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    async fn libraries(&self) -> Result<Vec<Library>> {
        Ok(vec![])
    }
    async fn items(&self, _: &LibraryId, _: Option<Stamp>) -> Result<Changes<ItemSummary>> {
        unreachable!()
    }
    async fn item(&self, id: &str) -> Result<Item> {
        Ok(Item { summary: ItemSummary { id: id.into(), ..Default::default() }, ..Default::default() })
    }
    async fn cover(&self, _: &str, _: Option<u32>) -> Result<bytes::Bytes> {
        unreachable!()
    }
    async fn open(&self, id: &str, _: &DeviceInfo) -> Result<PlaySession> {
        self.calls.lock().unwrap().push(Call::Open(id.into()));
        let start = self.remote.lock().unwrap().iter().find(|p| p.item.item == id).map_or(0.0, |p| p.position.secs());
        Ok(PlaySession {
            id: format!("session-{id}"),
            item: id.into(),
            start: Position::from_secs(start),
            duration: Some(100.0),
            tracks: vec![SessionTrack { index: 1, duration: Some(100.0), ..Default::default() }],
        })
    }
    async fn report(&self, _: &PlaySession, at: Position, listened: Duration) -> Result<()> {
        self.calls.lock().unwrap().push(Call::Report(at.secs(), listened));
        loop {
            let released = self.release.notified();
            if !self.blocked.load(Ordering::Acquire) {
                break;
            }
            released.await;
        }
        match self.report_errors.lock().unwrap().pop_front() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
    async fn close(&self, _: &PlaySession, at: Position) -> Result<()> {
        self.calls.lock().unwrap().push(Call::Close(at.secs()));
        Ok(())
    }
    async fn progress(&self) -> Result<Vec<Progress>> {
        Ok(self.remote.lock().unwrap().clone())
    }
    async fn set_progress(&self, updates: &[ProgressUpdate]) -> Result<()> {
        if self.patch_fails.load(Ordering::Relaxed) {
            return Err(sources::Error::Network("offline".into()));
        }
        self.batches.lock().unwrap().push(updates.iter().map(|u| u.position.secs()).collect());
        for update in updates {
            self.calls.lock().unwrap().push(Call::Patch(update.position.secs(), update.finished));
        }
        Ok(())
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    shelf: Arc<Shelf>,
    fake: Arc<Fake>,
    listening: Listening,
    events: mpsc::UnboundedReceiver<Event>,
    key: ItemKey,
}
impl Rig {
    async fn new(local: Option<ProgressUpdate>, remote: Option<Progress>) -> Self {
        Self::pending(local.into_iter().collect(), remote).await
    }
    async fn pending(local: Vec<ProgressUpdate>, remote: Option<Progress>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let shelf = Arc::new(Shelf::open(dir.path().join("shelf.redb")).unwrap());
        for update in &local {
            shelf.append_progress(update).unwrap();
        }
        let key =
            local.first().map(|p| p.item.clone()).or_else(|| remote.as_ref().map(|p| p.item.clone())).unwrap_or_else(|| ItemKey::new(AccountId::new(), "book"));
        let fake = Arc::new(Fake::default());
        if let Some(remote) = remote {
            fake.remote.lock().unwrap().push(remote);
        }
        let (tx, events) = mpsc::unbounded_channel();
        let listening = spawn(
            &tokio::runtime::Handle::current(),
            fake.clone(),
            shelf.clone(),
            DeviceInfo::default(),
            || true,
            move |event| {
                let _ = tx.send(event);
            },
        );
        let mut rig = Self { _dir: dir, shelf, fake, listening, events, key };
        rig.event(|e| matches!(e, Event::Positions(_))).await;
        rig
    }
    async fn event(&mut self, predicate: impl Fn(&Event) -> bool) -> Event {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let event = self.events.recv().await.unwrap();
                if predicate(&event) {
                    return event;
                }
            }
        })
        .await
        .unwrap()
    }
    async fn open(&mut self, generation: u64, key: ItemKey) -> f64 {
        self.listening.send(Request::Open { generation, key, at: None });
        match self.event(|e| matches!(e, Event::Opened { .. })).await {
            Event::Opened { result: Ok(opened), .. } => opened.position,
            _ => panic!("the fake book opens"),
        }
    }
    async fn report(&mut self, at: f64, finished: bool, close: bool) {
        self.listening.send(Request::Report { generation: 1, update: update(&self.key, at, finished), close });
        self.event(|e| matches!(e, Event::Positions(p) if p.iter().any(|p| p.position.secs() == at))).await;
    }
    async fn finish(self) {
        self.listening.finish().await.unwrap();
    }
}
fn update(key: &ItemKey, at: f64, finished: bool) -> ProgressUpdate {
    ProgressUpdate { item: key.clone(), position: Position::from_secs(at), finished, at: SystemTime::now(), listened: Duration::from_secs(15) }
}

#[tokio::test]
async fn a_failed_report_is_logged_and_replayed_before_the_next_live_report() {
    let mut rig = Rig::new(None, None).await;
    rig.open(1, rig.key.clone()).await;
    rig.fake.report_errors.lock().unwrap().push_back(sources::Error::Network("offline".into()));
    rig.report(15.0, false, false).await;
    assert_eq!(rig.shelf.pending_progress().unwrap().len(), 1);
    assert_eq!(rig.shelf.position(&rig.key).unwrap().unwrap().position.secs(), 15.0);
    rig.report(30.0, false, false).await;
    assert!(rig.shelf.pending_progress().unwrap().is_empty());
    assert!(rig.fake.calls().ends_with(&[Call::Patch(15.0, false), Call::Report(30.0, Duration::from_secs(15))]));
    rig.finish().await;
}

#[tokio::test]
async fn an_outage_keeps_every_report_and_replays_only_the_latest_position() {
    let mut rig = Rig::new(None, None).await;
    rig.open(1, rig.key.clone()).await;
    rig.fake.report_errors.lock().unwrap().push_back(sources::Error::Network("offline".into()));
    rig.fake.patch_fails.store(true, Ordering::Relaxed);
    rig.report(15.0, false, false).await;
    rig.report(30.0, false, false).await;
    assert_eq!(rig.shelf.pending_progress().unwrap().len(), 2);
    rig.fake.patch_fails.store(false, Ordering::Relaxed);
    rig.report(45.0, false, false).await;
    assert!(rig.fake.calls().ends_with(&[Call::Patch(30.0, false), Call::Report(45.0, Duration::from_secs(15))]));
    assert!(rig.shelf.pending_progress().unwrap().is_empty());
    rig.finish().await;
}

#[tokio::test]
async fn a_lost_server_session_is_reopened_without_rewinding_the_listener() {
    let mut rig = Rig::new(None, None).await;
    rig.open(1, rig.key.clone()).await;
    rig.fake.report_errors.lock().unwrap().push_back(sources::Error::Server { status: 404, message: None });
    rig.report(45.0, false, false).await;
    assert!(rig.fake.calls().ends_with(&[Call::Report(45.0, Duration::from_secs(15)), Call::Open("book".into()), Call::Report(45.0, Duration::from_secs(15))]));
    assert!(rig.shelf.pending_progress().unwrap().is_empty());
    rig.finish().await;
}

#[tokio::test]
async fn switching_books_reports_and_closes_before_opening_the_next_book() {
    let mut rig = Rig::new(None, None).await;
    rig.open(1, rig.key.clone()).await;
    rig.report(24.0, false, true).await;
    rig.open(2, ItemKey::new(rig.key.account, "second")).await;
    assert!(rig.fake.calls().ends_with(&[Call::Report(24.0, Duration::from_secs(15)), Call::Close(24.0), Call::Open("second".into())]));
    rig.finish().await;
}

#[tokio::test]
async fn the_end_of_the_book_sets_finished_and_closes_at_the_end() {
    let mut rig = Rig::new(None, None).await;
    rig.open(1, rig.key.clone()).await;
    rig.report(100.0, true, true).await;
    assert!(rig.fake.calls().ends_with(&[Call::Report(100.0, Duration::from_secs(15)), Call::Patch(100.0, true), Call::Close(100.0)]));
    assert!(rig.shelf.position(&rig.key).unwrap().unwrap().finished);
    rig.finish().await;
}

#[tokio::test]
async fn resuming_takes_the_newer_position_and_old_replays_cannot_rewind_a_phone() {
    for local_newer in [false, true] {
        let key = ItemKey::new(AccountId::new(), "book");
        let mut local = update(&key, 60.0, false);
        local.at = SystemTime::UNIX_EPOCH + Duration::from_secs(if local_newer { 200 } else { 100 });
        let remote =
            Progress { item: key.clone(), position: Position::from_secs(30.0), finished: false, at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(150)) };
        let mut rig = Rig::new(Some(local), Some(remote)).await;
        let at = rig.open(1, key).await;
        assert_eq!(at, if local_newer { 60.0 } else { 30.0 });
        rig.report(75.0, false, false).await;
        let patches: Vec<_> = rig.fake.calls().into_iter().filter(|c| matches!(c, Call::Patch(..))).collect();
        assert_eq!(patches, if local_newer { vec![Call::Patch(60.0, false)] } else { vec![] });
        rig.finish().await;
    }
}

#[tokio::test]
async fn new_positions_are_durable_while_an_earlier_network_report_is_stalled() {
    let mut rig = Rig::new(None, None).await;
    rig.open(1, rig.key.clone()).await;
    rig.fake.blocked.store(true, Ordering::Release);
    for at in [15.0, 30.0, 45.0] {
        rig.listening.send(Request::Report { generation: 1, update: update(&rig.key, at, false), close: false });
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        while rig.shelf.pending_progress().unwrap().len() != 3 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(rig.shelf.position(&rig.key).unwrap().unwrap().position.secs(), 45.0);
    assert!(!rig.fake.calls().contains(&Call::Report(30.0, Duration::from_secs(15))));
    rig.fake.blocked.store(false, Ordering::Release);
    rig.fake.release.notify_waiters();
    rig.event(|e| matches!(e, Event::Positions(p) if p.iter().any(|p| p.position.secs() == 45.0))).await;
    assert!(rig.shelf.pending_progress().unwrap().is_empty());
    rig.finish().await;
}

#[tokio::test]
async fn an_hour_of_pending_progress_is_one_request_and_every_entry_is_synced() {
    let key = ItemKey::new(AccountId::new(), "book");
    let pending = (1..=240).map(|n| update(&key, f64::from(n * 15), false)).collect();
    let rig = Rig::pending(pending, None).await;
    assert_eq!(*rig.fake.batches.lock().unwrap(), vec![vec![3600.0]]);
    assert!(rig.shelf.pending_progress().unwrap().is_empty());
    rig.finish().await;
}

#[tokio::test]
async fn several_items_replay_together_and_a_failed_batch_keeps_every_entry_pending() {
    let mut rig = Rig::new(None, None).await;
    rig.fake.patch_fails.store(true, Ordering::Relaxed);
    for item in ["first", "second"] {
        for at in [15.0, 30.0] {
            rig.shelf.append_progress(&update(&ItemKey::new(rig.key.account, item), at, false)).unwrap();
        }
    }
    // A live report first replays the pending batch; failure must retain it.
    rig.report(45.0, false, false).await;
    assert_eq!(rig.shelf.pending_progress().unwrap().len(), 5);
    rig.fake.patch_fails.store(false, Ordering::Relaxed);
    rig.report(60.0, false, false).await;
    let batches = rig.fake.batches.lock().unwrap().clone();
    let mut replay = batches[0].clone();
    replay.sort_by(f64::total_cmp);
    assert_eq!(replay, vec![30.0, 30.0, 45.0]);
    assert!(rig.shelf.pending_progress().unwrap().is_empty());
    rig.finish().await;
}
