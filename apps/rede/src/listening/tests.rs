use super::*;
use async_trait::async_trait;
use sources::{AccountId, Capabilities, Changes, Item, ItemSummary, Library, LibraryId, SessionTrack, Stamp};
use std::collections::VecDeque;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, SystemTime};
use testkit::{Answer, JsonFake};

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
    bookmarks: Mutex<Vec<sources::Bookmark>>,
    bookmark_lists: AtomicUsize,
    bookmark_calls: Mutex<Vec<String>>,
    bookmark_offline: AtomicBool,
    lose_bookmark_response: AtomicBool,
    /// A bookmark title the server refuses, and the status it answers with.
    refuse_bookmark: Mutex<Option<(String, u16)>>,
    bookmarks_blocked: AtomicBool,
    progress_fails: AtomicBool,
    progress_calls: AtomicUsize,
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
        Capabilities { bookmarks: true, ..Default::default() }
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
        self.progress_calls.fetch_add(1, Ordering::Relaxed);
        if self.progress_fails.load(Ordering::Relaxed) {
            return Err(sources::Error::Network("offline".into()));
        }
        Ok(self.remote.lock().unwrap().clone())
    }
    async fn bookmarks(&self) -> Result<Vec<sources::Bookmark>> {
        self.bookmark_lists.fetch_add(1, Ordering::Relaxed);
        loop {
            let released = self.release.notified();
            if !self.bookmarks_blocked.load(Ordering::Acquire) {
                break;
            }
            released.await;
        }
        if self.bookmark_offline.load(Ordering::Relaxed) {
            return Err(sources::Error::Network("offline".into()));
        }
        Ok(self.bookmarks.lock().unwrap().clone())
    }
    async fn create_bookmark(&self, item: &str, time: Position, title: &str) -> Result<sources::Bookmark> {
        self.bookmark_calls.lock().unwrap().push(format!("create:{title}"));
        if let Some((refused, status)) = self.refuse_bookmark.lock().unwrap().clone()
            && refused == title
        {
            return Err(sources::Error::Server { status, message: Some("no room for bookmarks".into()) });
        }
        let mut bookmarks = self.bookmarks.lock().unwrap();
        let account = bookmarks.first().map_or_else(test_account, |b| b.item.account);
        let value = sources::Bookmark { item: ItemKey::new(account, item), position: time, title: title.into(), created: SystemTime::now() };
        bookmarks.retain(|b| b.item.item != item || b.position != time);
        bookmarks.push(value.clone());
        if self.lose_bookmark_response.swap(false, Ordering::Relaxed) {
            return Err(sources::Error::Network("response lost".into()));
        }
        Ok(value)
    }
    async fn rename_bookmark(&self, item: &str, time: Position, title: &str) -> Result<sources::Bookmark> {
        let mut bookmarks = self.bookmarks.lock().unwrap();
        let b = bookmarks.iter_mut().find(|b| b.item.item == item && b.position == time).ok_or(sources::Error::Server { status: 404, message: None })?;
        b.title = title.into();
        self.bookmark_calls.lock().unwrap().push(format!("rename:{title}"));
        Ok(b.clone())
    }
    async fn delete_bookmark(&self, item: &str, time: Position) -> Result<()> {
        self.bookmarks.lock().unwrap().retain(|b| b.item.item != item || b.position != time);
        self.bookmark_calls.lock().unwrap().push("delete".into());
        Ok(())
    }

    async fn set_progress(&self, updates: &[ProgressUpdate], _remote: &[Progress]) -> Result<()> {
        if self.patch_fails.load(Ordering::Relaxed) {
            return Err(sources::Error::Network("offline".into()));
        }
        self.batches.lock().unwrap().push(updates.iter().map(|u| u.position.secs()).collect());
        for update in updates {
            self.calls.lock().unwrap().push(Call::Patch(update.position.secs(), update.finished));
            let mut remote = self.remote.lock().unwrap();
            remote.retain(|p| p.item != update.item);
            remote.push(Progress { item: update.item.clone(), position: update.position, finished: update.finished, at: Some(update.at) });
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
            local.first().map(|p| p.item.clone()).or_else(|| remote.as_ref().map(|p| p.item.clone())).unwrap_or_else(|| ItemKey::new(test_account(), "book"));
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
        tokio::time::timeout(testkit::slack(testkit::WAIT), async {
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
    tokio::time::timeout(testkit::slack(testkit::WAIT), async {
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
async fn six_minutes_of_pending_progress_is_one_request_and_every_entry_is_synced() {
    let key = ItemKey::new(AccountId::new(), "book");
    let pending = (1..=24).map(|n| update(&key, f64::from(n * 15), false)).collect();
    let rig = Rig::pending(pending, None).await;
    // Every entry, in one batch: 24 × 15 s.
    assert_eq!(*rig.fake.batches.lock().unwrap(), vec![vec![360.0]]);
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

fn test_account() -> AccountId {
    "2f1a5c3e-8a4b-4f6d-9e0a-1c2b3d4e5f60".parse().unwrap()
}
fn bookmark(title: &str) -> sources::Bookmark {
    sources::Bookmark {
        item: ItemKey::new(test_account(), "book"),
        position: Position::from_secs(1_234.000_000_000_000_2),
        title: title.into(),
        created: SystemTime::UNIX_EPOCH,
    }
}

#[tokio::test]
async fn offline_bookmarks_replay_in_order_after_reopening_the_shelf() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shelf.redb");
    let source = Fake::default();
    source.bookmark_offline.store(true, Ordering::Relaxed);
    {
        let shelf = Shelf::open(&path).unwrap();
        shelf.queue_bookmark(&bookmark("First"), shelf::BookmarkChange::Create).unwrap();
        shelf.queue_bookmark(&bookmark("Renamed"), shelf::BookmarkChange::Rename).unwrap();
        assert!(crate::sync::bookmarks(&source, &shelf).await.unwrap_err().is_network());
        assert_eq!(shelf.all_bookmarks().unwrap()[0].title, "Renamed");
    }
    let shelf = Shelf::open(&path).unwrap();
    source.bookmark_offline.store(false, Ordering::Relaxed);
    assert_eq!(crate::sync::bookmarks(&source, &shelf).await.unwrap().snapshot.bookmarks[0].title, "Renamed");
    assert_eq!(*source.bookmark_calls.lock().unwrap(), ["create:First", "rename:Renamed"]);
    assert!(shelf.pending_bookmarks().unwrap().is_empty());
    shelf.queue_bookmark(&bookmark("Renamed"), shelf::BookmarkChange::Delete).unwrap();
    assert!(shelf.all_bookmarks().unwrap().is_empty());
    assert!(crate::sync::bookmarks(&source, &shelf).await.unwrap().snapshot.bookmarks.is_empty());
    assert!(source.bookmarks.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_phone_rename_or_deletion_wins_over_a_queued_bookmark_edit() {
    for change in [shelf::BookmarkChange::Create, shelf::BookmarkChange::Rename, shelf::BookmarkChange::Delete] {
        let dir = tempfile::tempdir().unwrap();
        let shelf = Shelf::open(dir.path().join("shelf.redb")).unwrap();
        let source = Fake::default();
        if change != shelf::BookmarkChange::Create {
            shelf.replace_bookmarks(&[bookmark("Original")]).unwrap();
        }
        shelf.queue_bookmark(&bookmark("Local"), change).unwrap();
        *source.bookmarks.lock().unwrap() = vec![bookmark("Phone")];
        assert_eq!(crate::sync::bookmarks(&source, &shelf).await.unwrap().snapshot.bookmarks, [bookmark("Phone")]);
        assert!(source.bookmark_calls.lock().unwrap().is_empty());
        assert!(shelf.pending_bookmarks().unwrap().is_empty());
    }
    let dir = tempfile::tempdir().unwrap();
    let shelf = Shelf::open(dir.path().join("shelf.redb")).unwrap();
    shelf.replace_bookmarks(&[bookmark("Original")]).unwrap();
    shelf.queue_bookmark(&bookmark("Rename"), shelf::BookmarkChange::Rename).unwrap();
    let source = Fake::default();
    assert!(crate::sync::bookmarks(&source, &shelf).await.unwrap().snapshot.bookmarks.is_empty());
    assert!(source.bookmark_calls.lock().unwrap().is_empty(), "rename must not recreate a deleted phone bookmark");
}

#[tokio::test]
async fn a_lost_bookmark_response_can_be_retried_without_duplicate_writes() {
    let dir = tempfile::tempdir().unwrap();
    let shelf = Shelf::open(dir.path().join("shelf.redb")).unwrap();
    let source = Fake::default();
    source.lose_bookmark_response.store(true, Ordering::Relaxed);
    shelf.queue_bookmark(&bookmark("Local"), shelf::BookmarkChange::Create).unwrap();
    assert!(crate::sync::bookmarks(&source, &shelf).await.unwrap_err().is_network());
    assert_eq!(shelf.pending_bookmarks().unwrap().len(), 1);
    assert_eq!(crate::sync::bookmarks(&source, &shelf).await.unwrap().snapshot.bookmarks.len(), 1);
    assert_eq!(*source.bookmark_calls.lock().unwrap(), ["create:Local"]);
    assert!(shelf.pending_bookmarks().unwrap().is_empty());
}

#[tokio::test]
async fn a_newer_phone_position_moves_a_paused_book_and_undo_corrects_the_server() {
    let mut rig = Rig::new(None, None).await;
    rig.report(50.0, false, false).await;
    rig.open(1, rig.key.clone()).await;
    let remote =
        Progress { item: rig.key.clone(), position: Position::from_secs(4320.0), finished: true, at: Some(SystemTime::now() + Duration::from_secs(10)) };
    *rig.fake.remote.lock().unwrap() = vec![remote.clone()];
    rig.listening.send(Request::Activity { item: None });
    rig.listening.send(Request::Reconcile);
    let Event::Reconciled(reconciled) = rig.event(|e| matches!(e, Event::Reconciled(_))).await else { unreachable!() };
    assert_eq!(reconciled.previous.position.secs(), 50.0);
    assert_eq!(reconciled.message(), "Picked up where you left off on another device, 1h 12m in.");
    let saved = rig.shelf.position(&rig.key).unwrap().unwrap();
    assert_eq!(saved.position, remote.position);
    assert_eq!(saved.finished, remote.finished);
    let undo = ProgressUpdate { at: remote.at.unwrap() + Duration::from_millis(1), listened: Duration::ZERO, ..update(&rig.key, 50.0, false) };
    rig.listening.send(Request::Set(undo));
    rig.event(|e| matches!(e, Event::Positions(p) if p.iter().any(|p| p.position.secs() == 50.0))).await;
    assert!(rig.shelf.pending_progress().unwrap().is_empty());
    assert_eq!(rig.fake.remote.lock().unwrap()[0].position.secs(), 50.0);
    rig.listening.send(Request::Reconcile);
    rig.event(|e| matches!(e, Event::Positions(p) if p.iter().any(|p| p.position.secs() == 50.0))).await;
    rig.finish().await;
}

#[tokio::test]
async fn an_active_listener_keeps_this_devices_position_even_when_the_phone_is_newer() {
    let mut rig = Rig::new(None, None).await;
    rig.open(1, rig.key.clone()).await;
    rig.report(50.0, false, false).await;
    *rig.fake.remote.lock().unwrap() =
        vec![Progress { item: rig.key.clone(), position: Position::from_secs(4000.0), finished: false, at: Some(SystemTime::now() + Duration::from_secs(10)) }];
    rig.listening.send(Request::Activity { item: Some(rig.key.clone()) });
    rig.listening.send(Request::Reconcile);
    let Event::Positions(positions) = rig.event(|e| matches!(e, Event::Positions(_))).await else { unreachable!() };
    assert_eq!(positions[0].position.secs(), 50.0);
    assert_eq!(rig.fake.remote.lock().unwrap()[0].position.secs(), 50.0);
    rig.finish().await;
}

#[tokio::test]
async fn small_phone_changes_are_saved_without_a_reconcile_toast() {
    let mut rig = Rig::new(None, None).await;
    rig.report(50.0, false, false).await;
    *rig.fake.remote.lock().unwrap() =
        vec![Progress { item: rig.key.clone(), position: Position::from_secs(110.0), finished: false, at: Some(SystemTime::now() + Duration::from_secs(10)) }];
    rig.listening.send(Request::Reconcile);
    loop {
        match rig.events.recv().await.unwrap() {
            Event::Reconciled(_) => panic!("exactly one minute does not show a toast"),
            Event::Positions(p) if p.iter().any(|p| p.position.secs() == 110.0) => break,
            _ => {}
        }
    }
    assert_eq!(rig.shelf.position(&rig.key).unwrap().unwrap().position.secs(), 110.0);
    rig.finish().await;
}

#[tokio::test]
async fn a_bookmark_is_visible_and_durable_while_an_earlier_report_is_stalled() {
    let mut rig = Rig::new(None, None).await;
    rig.open(1, rig.key.clone()).await;
    rig.fake.blocked.store(true, Ordering::Release);
    rig.listening.send(Request::Report { generation: 1, update: update(&rig.key, 15.0, false), close: false });
    rig.listening.send(Request::Bookmark { bookmark: bookmark("Saved offline"), change: shelf::BookmarkChange::Create });
    rig.event(|e| matches!(e, Event::Bookmarks(Ok(snapshot)) if snapshot.bookmarks.iter().any(|b| b.title == "Saved offline"))).await;
    assert_eq!(rig.shelf.pending_bookmarks().unwrap().len(), 1);
    assert_eq!(rig.shelf.bookmarks(&rig.key).unwrap()[0].note.as_deref(), Some("Saved offline"));
    rig.fake.blocked.store(false, Ordering::Release);
    rig.fake.release.notify_waiters();
    rig.event(|e| matches!(e, Event::Bookmarks(Ok(_)))).await;
    rig.listening.finish().await.unwrap();
    assert!(rig.shelf.pending_bookmarks().unwrap().is_empty());
}

#[tokio::test]
async fn a_paused_resume_pulls_the_phone_position_before_signalling_playback() {
    let mut rig = Rig::new(None, None).await;
    rig.open(1, rig.key.clone()).await;
    rig.report(50.0, false, false).await;
    *rig.fake.remote.lock().unwrap() =
        vec![Progress { item: rig.key.clone(), position: Position::from_secs(500.0), finished: false, at: Some(SystemTime::now() + Duration::from_secs(10)) }];
    rig.listening.send(Request::Resume { generation: 7 });
    rig.event(|e| matches!(e, Event::Reconciled(_))).await;
    rig.event(|e| matches!(e, Event::ResumeReady { generation: 7 })).await;
    assert_eq!(rig.shelf.position(&rig.key).unwrap().unwrap().position.secs(), 500.0);
    rig.finish().await;
}

#[tokio::test]
async fn a_pull_with_nothing_queued_asks_the_server_for_bookmarks_once() {
    let mut rig = Rig::new(None, None).await;
    // The shelf's own snapshot went first; this is the start-up pull's answer.
    rig.event(|e| matches!(e, Event::Bookmarks(_))).await;
    assert_eq!(rig.fake.bookmark_lists.load(Ordering::Relaxed), 1, "a pull that wrote nothing lists once");
    rig.listening.send(Request::Reconcile);
    rig.event(|e| matches!(e, Event::Bookmarks(_))).await;
    assert_eq!(rig.fake.bookmark_lists.load(Ordering::Relaxed), 2, "and a focus pull is one request");
    rig.finish().await;
}

#[tokio::test]
async fn a_book_first_started_on_the_phone_is_accepted_without_an_undo_toast() {
    let mut rig = Rig::new(None, None).await;
    *rig.fake.remote.lock().unwrap() = vec![Progress {
        item: rig.key.clone(),
        position: Position::from_secs(5_000.0),
        finished: false,
        at: Some(SystemTime::now() + Duration::from_secs(10)),
    }];
    rig.listening.send(Request::Reconcile);
    let mut reconciled = false;
    loop {
        match rig.events.recv().await.unwrap() {
            Event::Reconciled(_) => reconciled = true,
            Event::Positions(positions) if positions.iter().any(|p| p.position.secs() == 5_000.0) => break,
            _ => {}
        }
    }
    assert!(!reconciled, "there is no local place to undo to");
    assert_eq!(rig.shelf.position(&rig.key).unwrap().unwrap().position.secs(), 5_000.0);
    rig.finish().await;
}

#[tokio::test]
async fn a_phone_position_does_not_offer_undo_for_a_book_the_player_has_not_loaded() {
    let mut rig = Rig::new(None, None).await;
    let id = rig.shelf.append_progress(&update(&rig.key, 50.0, false)).unwrap();
    rig.shelf.mark_synced(&[id]).unwrap();
    *rig.fake.remote.lock().unwrap() = vec![Progress {
        item: rig.key.clone(),
        position: Position::from_secs(5_000.0),
        finished: false,
        at: Some(SystemTime::now() + Duration::from_secs(10)),
    }];
    rig.listening.send(Request::Reconcile);
    let mut reconciled = false;
    loop {
        match rig.events.recv().await.unwrap() {
            Event::Reconciled(_) => reconciled = true,
            Event::Positions(positions) if positions.iter().any(|p| p.position.secs() == 5_000.0) => break,
            _ => {}
        }
    }
    assert!(!reconciled, "a book that isn't open has nothing to move or undo");
    rig.finish().await;
}

#[tokio::test]
async fn a_paused_local_position_ahead_of_the_clock_does_not_overwrite_the_phone() {
    let key = ItemKey::new(test_account(), "book");
    let remote =
        Progress { item: key.clone(), position: Position::from_secs(4_000.0), finished: false, at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(100)) };
    let mut rig = Rig::new(None, Some(remote)).await;
    let local = ProgressUpdate {
        item: key.clone(),
        position: Position::from_secs(50.0),
        finished: false,
        at: SystemTime::UNIX_EPOCH + Duration::from_secs(200),
        listened: Duration::ZERO,
    };
    let id = rig.shelf.append_progress(&local).unwrap();
    rig.shelf.mark_synced(&[id]).unwrap();
    rig.listening.send(Request::Reconcile);
    let Event::Positions(positions) = rig.event(|e| matches!(e, Event::Positions(_))).await else { unreachable!() };
    assert_eq!(positions[0].position.secs(), 50.0, "the newest local position still wins here");
    assert_eq!(rig.fake.remote.lock().unwrap()[0].position.secs(), 4_000.0, "but a paused device does not overwrite the phone");
    assert!(!rig.fake.calls().iter().any(|c| matches!(c, Call::Patch(..))));
    rig.finish().await;
}

#[tokio::test]
async fn resume_does_not_wait_for_bookmark_sync() {
    let mut rig = Rig::new(None, None).await;
    rig.fake.bookmarks_blocked.store(true, Ordering::Release);
    rig.listening.send(Request::Resume { generation: 7 });
    rig.event(|e| matches!(e, Event::ResumeReady { generation: 7 })).await;
    rig.fake.bookmarks_blocked.store(false, Ordering::Release);
    rig.fake.release.notify_waiters();
    rig.finish().await;
}

#[tokio::test]
async fn opening_a_book_does_not_wait_for_bookmark_sync() {
    let mut rig = Rig::new(None, None).await;
    rig.fake.bookmarks_blocked.store(true, Ordering::Release);
    rig.open(1, rig.key.clone()).await;
    rig.fake.bookmarks_blocked.store(false, Ordering::Release);
    rig.fake.release.notify_waiters();
    rig.finish().await;
}

#[tokio::test]
async fn a_pull_that_cannot_reach_the_server_skips_bookmark_sync() {
    let mut rig = Rig::new(None, None).await;
    let before = rig.fake.bookmark_lists.load(Ordering::Relaxed);
    rig.fake.progress_fails.store(true, Ordering::Relaxed);
    rig.listening.send(Request::Reconcile);
    rig.event(|e| matches!(e, Event::Error(e) if e.is_network())).await;
    // Give a regression time to make the bookmark request it should skip.
    let _ = tokio::time::timeout(Duration::from_millis(100), async {
        while rig.fake.bookmark_lists.load(Ordering::Relaxed) == before {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert_eq!(rig.fake.bookmark_lists.load(Ordering::Relaxed), before, "the same unreachable server isn't asked again for bookmarks");
    rig.fake.progress_fails.store(false, Ordering::Relaxed);
    rig.finish().await;
}

#[tokio::test]
async fn a_refused_bookmark_edit_is_dropped_reported_once_and_does_not_block_the_next() {
    let mut rig = Rig::new(None, None).await;
    *rig.fake.refuse_bookmark.lock().unwrap() = Some(("Refused".into(), 400));
    rig.shelf.queue_bookmark(&bookmark("Refused"), shelf::BookmarkChange::Create).unwrap();
    let next = sources::Bookmark { position: Position::from_secs(2_000.0), ..bookmark("Next") };
    rig.shelf.queue_bookmark(&next, shelf::BookmarkChange::Create).unwrap();
    rig.listening.send(Request::Reconcile);
    let Event::Error(error) = rig.event(|e| matches!(e, Event::Error(_))).await else { unreachable!() };
    assert!(error.message().contains("400"), "the refusal is shown: {}", error.message());
    assert!(rig.shelf.pending_bookmarks().unwrap().is_empty(), "a permanent refusal is not retried forever");
    assert_eq!(*rig.fake.bookmark_calls.lock().unwrap(), ["create:Refused", "create:Next"], "the rest of the queue still runs");
    assert!(rig.shelf.bookmark_snapshot().unwrap().bookmarks.iter().any(|b| b.title == "Next"));

    // A later pull finds nothing queued and does not create the refused edit again.
    rig.listening.send(Request::Reconcile);
    rig.event(|e| matches!(e, Event::Bookmarks(Ok(snapshot)) if snapshot.bookmarks.iter().any(|b| b.title == "Next"))).await;
    assert_eq!(*rig.fake.bookmark_calls.lock().unwrap(), ["create:Refused", "create:Next"], "a dropped edit is not attempted again");
    rig.finish().await;
}

#[tokio::test]
async fn a_bookmark_edit_refused_for_want_of_sign_in_stays_queued() {
    let mut rig = Rig::new(None, None).await;
    *rig.fake.refuse_bookmark.lock().unwrap() = Some(("Gated".into(), 401));
    rig.shelf.queue_bookmark(&bookmark("Gated"), shelf::BookmarkChange::Create).unwrap();
    rig.listening.send(Request::Reconcile);
    rig.event(|e| matches!(e, Event::Bookmarks(Err(_)))).await;
    assert_eq!(rig.shelf.pending_bookmarks().unwrap().len(), 1, "a 401 from a lapsed gate keeps the edit for the next try");
    *rig.fake.refuse_bookmark.lock().unwrap() = None;
    rig.finish().await;
}

/// A worker over the real Audiobookshelf source and a fake server, so a test
/// checks the request Audiobookshelf reads rather than what a stand-in
/// records.
struct AbsRig {
    _dir: tempfile::TempDir,
    shelf: Arc<Shelf>,
    server: JsonFake,
    listening: Listening,
    events: mpsc::UnboundedReceiver<Event>,
    key: ItemKey,
}

impl AbsRig {
    /// `progress` is the `mediaProgress` list `GET /api/me` answers with: what
    /// the server already holds.
    async fn new(progress: &str) -> Self {
        let progress = progress.to_owned();
        let server = JsonFake::start(move |seen| match seen.path() {
            "/api/me" => Answer::ok(format!(r#"{{"id":"user","mediaProgress":{progress}}}"#)),
            "/api/me/bookmarks" => Answer::ok(r#"{"bookmarks":[]}"#),
            _ => Answer::ok(""),
        });
        let dir = tempfile::tempdir().unwrap();
        let shelf = Arc::new(Shelf::open(dir.path().join("shelf.redb")).unwrap());
        let client = abs::Client::new(server.base(), abs::DeviceInfo::new("device", "1")).unwrap().with_tokens(abs::Tokens::new("access", "refresh"));
        let source: Arc<dyn Source> = Arc::new(sources::AbsSource::new(test_account(), client, Vec::new()));
        let key = ItemKey::new(test_account(), "book");
        let (tx, events) = mpsc::unbounded_channel();
        let listening = spawn(
            &tokio::runtime::Handle::current(),
            source,
            shelf.clone(),
            DeviceInfo::default(),
            || true,
            move |event| {
                let _ = tx.send(event);
            },
        );
        let mut rig = Self { _dir: dir, shelf, server, listening, events, key };
        rig.event(|e| matches!(e, Event::Positions(_))).await;
        rig
    }

    async fn event(&mut self, predicate: impl Fn(&Event) -> bool) -> Event {
        tokio::time::timeout(testkit::slack(testkit::WAIT), async {
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

    /// What the progress endpoints were sent, in order.
    fn progress_bodies(&self) -> Vec<serde_json::Value> {
        self.server.seen().into_iter().filter(|seen| seen.method == "PATCH" && seen.path().starts_with("/api/me/progress")).map(|seen| seen.json()).collect()
    }

    async fn finish(self) {
        self.listening.finish().await.unwrap();
    }
}

#[tokio::test]
async fn finishing_a_book_online_writes_the_finish_abs_stores_and_shows_it() {
    let mut rig = AbsRig::new("[]").await;
    let at = SystemTime::now();
    rig.listening.send(Request::Set(ProgressUpdate {
        item: rig.key.clone(),
        position: Position::from_secs(3_600.0),
        finished: true,
        at,
        listened: Duration::ZERO,
    }));
    let Event::Positions(positions) = rig.event(|e| matches!(e, Event::Positions(p) if p.iter().any(|p| p.finished))).await else { unreachable!() };
    assert_eq!(positions[0].position.secs(), 3_600.0);
    let bodies = rig.progress_bodies();
    assert_eq!(bodies.len(), 1, "a finish needs no reset before it: {bodies:?}");
    assert_eq!(bodies[0], serde_json::json!([{ "libraryItemId": "book", "currentTime": 3_600.0, "isFinished": true, "lastUpdate": Stamp::from(at).millis() }]));
    assert!(rig.shelf.position(&rig.key).unwrap().unwrap().finished);
    assert!(rig.shelf.pending_progress().unwrap().is_empty());
    rig.finish().await;
}

#[tokio::test]
async fn un_finishing_a_book_resets_the_finish_first_and_then_writes_its_place() {
    let remote = r#"[{"id":"p","libraryItemId":"book","currentTime":4200.0,"isFinished":true,"lastUpdate":400000}]"#;
    let mut rig = AbsRig::new(remote).await;
    let at = Stamp::from_millis(400_001).system_time();
    rig.listening.send(Request::Set(ProgressUpdate {
        item: rig.key.clone(),
        position: Position::from_secs(1_234.5),
        finished: false,
        at,
        listened: Duration::ZERO,
    }));
    let Event::Positions(positions) = rig.event(|e| matches!(e, Event::Positions(p) if p.iter().any(|p| p.position.secs() == 1_234.5))).await else {
        unreachable!()
    };
    assert!(!positions[0].finished);
    let bodies = rig.progress_bodies();
    assert_eq!(bodies.len(), 2, "ABS drops a place sent with the reset: {bodies:?}");
    assert_eq!(bodies[0], serde_json::json!({ "isFinished": false, "lastUpdate": 400_000 }), "the reset is stamped a millisecond before the place");
    assert_eq!(bodies[1], serde_json::json!([{ "libraryItemId": "book", "currentTime": 1_234.5, "lastUpdate": 400_001 }]));
    assert!(rig.shelf.pending_progress().unwrap().is_empty());
    rig.finish().await;
}

#[tokio::test]
async fn finishing_the_loaded_book_closes_its_session_before_the_finish_goes_out() {
    let mut rig = Rig::new(None, None).await;
    rig.open(1, rig.key.clone()).await;
    // What the app sends when the listener finishes the book it is playing:
    // stopping reports and closes at the old place, then the finish follows.
    rig.listening.send(Request::Report { generation: 1, update: update(&rig.key, 40.0, false), close: true });
    rig.event(|e| matches!(e, Event::Positions(p) if p.iter().any(|p| p.position.secs() == 40.0))).await;
    rig.listening.send(Request::Set(update(&rig.key, 40.0, true)));
    rig.event(|e| matches!(e, Event::Positions(p) if p.iter().any(|p| p.finished))).await;
    let calls = rig.fake.calls();
    assert!(calls.ends_with(&[Call::Close(40.0), Call::Patch(40.0, true)]), "the close comes first: a sync after the finish un-finishes the book: {calls:?}");
    assert!(rig.fake.remote.lock().unwrap()[0].finished);
    // The session is closed, so nothing of this device's writes the old place
    // again; a pull keeps what the finish said.
    rig.listening.send(Request::Reconcile);
    let Event::Positions(positions) = rig.event(|e| matches!(e, Event::Positions(_))).await else { unreachable!() };
    assert!(positions.iter().all(|p| p.finished));
    assert!(rig.shelf.position(&rig.key).unwrap().unwrap().finished);
    rig.finish().await;
}

#[tokio::test]
async fn settling_a_book_offline_keeps_every_intent_and_the_last_one_wins_on_replay() {
    let mut rig = Rig::new(None, None).await;
    rig.fake.patch_fails.store(true, Ordering::Relaxed);
    for (at, finished) in [(90.0, true), (50.0, false), (0.0, false)] {
        rig.listening.send(Request::Set(update(&rig.key, at, finished)));
        rig.event(|e| matches!(e, Event::Positions(p) if p.iter().any(|p| p.position.secs() == at && p.finished == finished))).await;
    }
    assert_eq!(rig.shelf.pending_progress().unwrap().len(), 3, "every intent waits in the log");
    assert_eq!(rig.shelf.position(&rig.key).unwrap().unwrap().position.secs(), 0.0);
    rig.fake.patch_fails.store(false, Ordering::Relaxed);
    rig.listening.send(Request::Reconcile);
    rig.event(|e| matches!(e, Event::Positions(p) if p.iter().any(|p| p.position.secs() == 0.0 && !p.finished))).await;
    assert!(rig.shelf.pending_progress().unwrap().is_empty(), "the log is sent oldest first and done");
    assert_eq!(*rig.fake.batches.lock().unwrap().last().unwrap(), vec![0.0], "the newest place per item is the one sent");
    assert!(!rig.fake.remote.lock().unwrap()[0].finished);
    rig.finish().await;
}

#[tokio::test]
async fn a_pull_that_races_a_finish_cannot_bring_the_old_place_back() {
    let mut rig = Rig::new(None, None).await;
    let served = Stamp::from_millis(1_800_000_000_000).system_time();
    let old = Progress { item: rig.key.clone(), position: Position::from_secs(40.0), finished: false, at: Some(served) };
    *rig.fake.remote.lock().unwrap() = vec![old.clone()];
    // The app stamps a finish after the row it saw, even when that row's clock
    // is ahead of this one.
    let mut finish = update(&rig.key, 40.0, true);
    finish.at = served + Duration::from_millis(1);
    rig.listening.send(Request::Set(finish));
    rig.event(|e| matches!(e, Event::Positions(p) if p.iter().any(|p| p.finished))).await;
    // A pull answered before the write landed carries the old row back.
    *rig.fake.remote.lock().unwrap() = vec![old];
    rig.listening.send(Request::Reconcile);
    let Event::Positions(positions) = rig.event(|e| matches!(e, Event::Positions(_))).await else { unreachable!() };
    assert!(positions[0].finished, "the newer entry wins");
    assert_eq!(positions[0].at, Some(served + Duration::from_millis(1)));
    assert!(rig.shelf.position(&rig.key).unwrap().unwrap().finished);
    assert!(rig.shelf.pending_progress().unwrap().is_empty());
    rig.finish().await;
}

#[test]
fn a_failed_retry_backs_off_and_a_success_resets_it() {
    let doubled = next_retry_delay(RETRY_DELAY, true);
    assert_eq!(doubled, RETRY_DELAY * 2);
    assert_eq!(next_retry_delay(doubled, true), RETRY_DELAY * 4);
    assert_eq!(next_retry_delay(MAX_RETRY_DELAY, true), MAX_RETRY_DELAY);
    assert_eq!(next_retry_delay(doubled, false), RETRY_DELAY);
}
