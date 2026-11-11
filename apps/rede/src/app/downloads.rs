//! Download intents and scoped results. No view owns download work.

use super::RedeApp;
use crate::bridge::OfflineMsg;
use crate::failure::Failure;
use offline::Pin;
use sources::ItemKey;

impl RedeApp {
    pub(super) fn download_book(&mut self, key: ItemKey) {
        if self.session.account.as_ref().is_none_or(|a| a.id != key.account) {
            return;
        }
        let (Some(source), Some(shelf), Some(_)) = (self.session.source.clone(), self.session.shelf.clone(), self.downloads.store.get()) else {
            self.show_failure(Failure::Local("Downloads aren't ready. Try again.".into()));
            return;
        };
        self.downloads.remove_after_stop.remove(&key);
        self.downloads.refreshed = false;
        self.downloads.serial += 1;
        let request = self.downloads.serial;
        self.downloads.preparing.insert(key.clone(), request);
        self.downloads.errors.remove(&key);
        self.bridge.prepare_download(source, shelf, key, request);
    }

    pub(super) fn remove_download(&mut self, key: &ItemKey) {
        self.downloads.remove_after_stop.remove(key);
        self.downloads.preparing.remove(key);
        self.downloads.errors.remove(key);
        if let Some(store) = self.downloads.store.get() {
            store.unpin(&Pin::Book(key.to_string()));
        }
        self.downloads.refresh();
    }

    pub(super) fn remove_finished_download(&mut self, key: &ItemKey, finished: bool) {
        let was_finished = self.positions.get(&key.item).is_some_and(|p| p.finished);
        if finished && !was_finished && self.shown.downloads.remove_when_finished {
            self.downloads.remove_after_stop.insert(key.clone());
            self.remove_stopped_downloads();
        } else if !finished {
            self.downloads.remove_after_stop.remove(key);
        }
    }

    pub(super) fn remove_stopped_downloads(&mut self) {
        let ready: Vec<_> = self
            .downloads
            .remove_after_stop
            .iter()
            .filter(|key| !self.player.active || self.player.book.as_ref().is_none_or(|book| book.key != **key))
            .cloned()
            .collect();
        for key in ready {
            self.remove_download(&key);
        }
    }

    pub(super) fn download_connectivity(&mut self) {
        if let Some(listening) = &self.listening {
            listening.send(crate::listening::Request::Reachable(!self.sync.offline));
        }
        if let Some(store) = self.downloads.store.get() {
            if self.sync.offline {
                self.bridge.stop_downloads(&store);
            } else if let (Some(source), Some(shelf)) = (&self.session.source, &self.session.shelf) {
                self.bridge.download_source(source.clone(), shelf.clone(), store);
            }
        }
    }

    pub(super) fn download_event(&mut self, event: OfflineMsg) {
        match event {
            OfflineMsg::Opened(result) => match result {
                Ok(store) => {
                    store.set_limit(self.shown.downloads.limit);
                    self.downloads.store.install(store);
                    self.downloads.refresh();
                    self.download_connectivity();
                }
                Err(error) => self.show_failure(error),
            },
            OfflineMsg::Changed => {
                self.downloads.refresh();
                let rejected = self.downloads.snapshot.last_errors.values().any(|error| error.http_status == Some(401));
                if rejected
                    && !self.sync.offline
                    && !self.downloads.refreshing
                    && !self.downloads.refreshed
                    && let Some(source) = &self.session.source
                {
                    self.downloads.refreshing = true;
                    self.downloads.refreshed = true;
                    self.bridge.refresh_downloads(source.clone());
                }
            }
            OfflineMsg::Prepared { key, request, result } => {
                if self.downloads.preparing.get(&key) != Some(&request) {
                    return;
                }
                self.downloads.preparing.remove(&key);
                match result {
                    Ok(item) => {
                        if let Some(store) = self.downloads.store.get() {
                            self.bridge.stop_downloads(&store);
                            store.pin(Pin::Book(key.to_string()), item.summary.title.clone(), crate::downloads::tracks(&key, &item));
                            self.books.loaded(&key.item, *item);
                            self.download_connectivity();
                        }
                    }
                    Err(error) => {
                        self.downloads.errors.insert(key, error.message().to_owned());
                        self.show_failure(error);
                    }
                }
                self.downloads.refresh();
            }
            OfflineMsg::Refreshed(result) => {
                self.downloads.refreshing = false;
                match result {
                    Ok(()) => self.download_connectivity(),
                    Err(error) => self.show_failure(error),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Action, PrefsAction, fixture};
    use crate::bridge::Msg;
    use std::sync::Arc;
    use testkit::{Answer, Deadline, JsonFake};

    fn install(app: &mut RedeApp, dir: &std::path::Path) -> offline::OfflineStore {
        let store = offline::OfflineStore::open(
            offline::OfflineConfig { dir: dir.join("downloads"), limit: None, reply_check: Default::default() },
            app._rt.0.as_ref().unwrap().handle().clone(),
            Arc::new(|| {}),
        )
        .unwrap();
        app.downloads.store.install(store.clone());
        store
    }

    #[test]
    fn download_preferences_default_off_and_update_both_the_frame_and_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture::app(&egui::Context::default(), dir.path(), 1);
        let store = install(&mut app, dir.path());
        assert!(!app.shown.downloads.remove_when_finished);
        app.apply(Action::Prefs(PrefsAction::RemoveDownloadsWhenFinished(true)));
        app.apply(Action::Prefs(PrefsAction::DownloadLimit(Some(4096))));
        assert!(app.prefs.get(|s| s.downloads.remove_when_finished));
        assert_eq!(store.snapshot().limit, Some(4096));
        let view = app.view();
        assert_eq!(view.download_settings.limit, Some(4096));
        assert!(view.downloads.snapshot.pins.is_empty());
    }

    #[test]
    fn remove_pause_retry_and_finished_policy_use_the_account_store() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture::app(&egui::Context::default(), dir.path(), 1);
        let store = install(&mut app, dir.path());
        let key = ItemKey::new(fixture::account().id, "book-0");
        let pin = Pin::Book(key.to_string());
        store.pin(pin.clone(), "Book", vec![offline::TrackRef::new("file", None, None)]);
        app.apply(Action::PauseDownloads(true));
        assert!(store.snapshot().paused);
        app.apply(Action::PauseDownloads(false));
        app.apply(Action::RetryDownloads);
        assert!(!store.snapshot().paused);
        app.remove_finished_download(&key, true);
        assert!(store.is_pinned(&pin), "removal is off by default");
        app.apply(Action::Prefs(PrefsAction::RemoveDownloadsWhenFinished(true)));
        app.remove_finished_download(&key, false);
        assert!(store.is_pinned(&pin));
        app.playback_event(crate::listening::Event::Positions(vec![sources::Progress {
            item: key.clone(),
            position: sources::Position::from_secs(100.0),
            finished: true,
            at: Some(std::time::SystemTime::now()),
        }]));
        assert!(!store.is_pinned(&pin));
        store.pin(pin.clone(), "Book", vec![]);
        app.apply(Action::RemoveDownload(key));
        assert!(!store.is_pinned(&pin));
    }

    fn enable_removal(app: &mut RedeApp, store: &offline::OfflineStore, key: &ItemKey) -> Pin {
        app.session.source = Some(Arc::new(Unreachable(Default::default())));
        app.apply(Action::Prefs(PrefsAction::RemoveDownloadsWhenFinished(true)));
        let pin = Pin::Book(key.to_string());
        store.pin(pin.clone(), "Book", vec![offline::TrackRef::new("file", None, None)]);
        pin
    }

    #[test]
    fn pulling_an_already_finished_book_keeps_its_new_download() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture::app(&egui::Context::default(), dir.path(), 1);
        let store = install(&mut app, dir.path());
        let key = ItemKey::new(fixture::account().id, "book-0");
        let progress =
            sources::Progress { item: key.clone(), position: sources::Position::from_secs(100.0), finished: true, at: Some(std::time::SystemTime::now()) };
        app.positions.insert(key.item.clone(), progress.clone());
        let pin = enable_removal(&mut app, &store, &key);
        app.playback_event(crate::listening::Event::Positions(vec![progress]));
        assert!(store.is_pinned(&pin));
    }

    #[test]
    fn marking_a_book_finished_removes_its_download_only_on_the_transition() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture::app(&egui::Context::default(), dir.path(), 1);
        let store = install(&mut app, dir.path());
        let key = ItemKey::new(fixture::account().id, "book-0");
        let pin = enable_removal(&mut app, &store, &key);
        app.apply(Action::MarkFinished(key.item.clone()));
        assert!(!store.is_pinned(&pin));
        store.pin(pin.clone(), "Book", vec![]);
        app.apply(Action::MarkFinished(key.item));
        assert!(store.is_pinned(&pin));
    }

    #[test]
    fn a_cancelled_prepare_and_an_older_accounts_result_cannot_pin_a_book() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture::app(&egui::Context::default(), dir.path(), 1);
        let store = install(&mut app, dir.path());
        let key = ItemKey::new(fixture::account().id, "book-0");
        app.downloads.preparing.insert(key.clone(), 1);
        app.apply(Action::RemoveDownload(key.clone()));
        app.download_event(OfflineMsg::Prepared { key: key.clone(), request: 1, result: Ok(Box::new(fixture::details(0))) });
        assert!(!store.is_pinned(&Pin::Book(key.to_string())));
        let old = app.bridge.session();
        app.bridge.end_session();
        app.downloads.preparing.insert(key.clone(), 2);
        app.handle(Msg::Offline { session: old, event: OfflineMsg::Prepared { key: key.clone(), request: 2, result: Ok(Box::new(fixture::details(0))) } });
        assert!(!store.is_pinned(&Pin::Book(key.to_string())));
    }

    #[test]
    fn downloading_fetches_details_before_pinning_and_hard_links_the_streamed_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture::app(&egui::Context::default(), dir.path(), 0);
        let account = fixture::account();
        let key = ItemKey::new(account.id, "new-book");
        let api = JsonFake::start(|seen| {
            assert_eq!(seen.path(), "/api/items/new-book");
            Answer::ok(
                r#"{"id":"new-book","libraryId":"lib-books","mediaType":"book","media":{"duration":100,"metadata":{"title":"New book"},"tracks":[{"index":1,"ino":"123","duration":100,"size":4096,"metadata":{"ext":".m4b"}}],"chapters":[{"start":0,"end":100,"title":"Chapter"}]}}"#,
            )
        });
        let client = abs::Client::new(api.base(), account.device()).unwrap().with_tokens(abs::Tokens::new("secret", "refresh"));
        app.session.source = Some(Arc::new(sources::AbsSource::new(account.id, client, vec![])));
        app.bridge.open_downloads(account.id, None, app.stream_dir.clone());
        let deadline = Deadline::new();
        while app.downloads.store.get().is_none() {
            assert!(!deadline.passed());
            app.handle(app.inbox.recv_timeout(testkit::slack(testkit::WAIT)).unwrap());
        }
        let id = ItemKey::new(account.id, "123").to_string();
        std::fs::create_dir_all(&app.stream_dir).unwrap();
        let cached = app.stream_dir.join(streamcache::file_name(&id, Some("m4b")));
        std::fs::write(&cached, vec![42; 4096]).unwrap();
        app.apply(Action::DownloadBook(key.clone()));
        while !app.downloads.book(&key).is_some_and(offline::PinSummary::complete) {
            assert!(!deadline.passed());
            let msg = app.inbox.recv_timeout(testkit::slack(testkit::WAIT)).unwrap();
            app.handle(msg);
        }
        let item = app.session.shelf.as_ref().unwrap().item(&key).unwrap().unwrap();
        assert_eq!(item.chapters.len(), 1);
        assert_eq!(item.tracks[0].content_id, "123");
        assert_eq!(api.seen().len(), 1, "the original file came from the stream cache");
        let downloaded = app.downloads.store.get().unwrap().local_path(&id).unwrap();
        assert_eq!(std::fs::read(&downloaded).unwrap(), std::fs::read(&cached).unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(std::fs::metadata(downloaded).unwrap().ino(), std::fs::metadata(cached).unwrap().ino());
        }
    }
    struct Unreachable(std::sync::atomic::AtomicUsize);
    fn network<T>() -> sources::Result<T> {
        Err(sources::Error::Network("offline".into()))
    }
    #[async_trait::async_trait]
    impl sources::Source for Unreachable {
        fn capabilities(&self) -> sources::Capabilities {
            Default::default()
        }
        async fn libraries(&self) -> sources::Result<Vec<sources::Library>> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            network()
        }
        async fn items(&self, _: &sources::LibraryId, _: Option<sources::Stamp>) -> sources::Result<sources::Changes<sources::ItemSummary>> {
            network()
        }
        async fn item(&self, _: &str) -> sources::Result<sources::Item> {
            network()
        }
        async fn cover(&self, _: &str, _: Option<u32>) -> sources::Result<bytes::Bytes> {
            network()
        }
        async fn open(&self, _: &str, _: &sources::DeviceInfo) -> sources::Result<sources::PlaySession> {
            network()
        }
        async fn report(&self, _: &sources::PlaySession, _: sources::Position, _: std::time::Duration) -> sources::Result<()> {
            network()
        }
        async fn close(&self, _: &sources::PlaySession, _: sources::Position) -> sources::Result<()> {
            network()
        }
        async fn progress(&self) -> sources::Result<Vec<sources::Progress>> {
            network()
        }
        async fn set_progress(&self, _: &[sources::ProgressUpdate], _: &[sources::Progress]) -> sources::Result<()> {
            network()
        }
    }

    #[test]
    fn an_unreachable_start_keeps_the_shelf_and_pages_and_schedules_another_try() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = fixture::app(&ctx, dir.path(), 0);
        let shelf = app.session.shelf.clone().unwrap();
        let account = fixture::account();
        let key = ItemKey::new(account.id, "book-0");
        shelf.set_details(&key, &fixture::details(0)).unwrap();
        let source = Arc::new(Unreachable(Default::default()));
        app.signed_in(account, source.clone(), shelf.clone(), None);
        let deadline = Deadline::new();
        while app.sync.running || app.books.len() == 0 || !app.sync.offline {
            assert!(!deadline.passed());
            let msg = app.inbox.recv_timeout(testkit::slack(testkit::WAIT)).unwrap();
            app.handle(msg);
        }
        assert!(app.session.signed_in());
        assert!(app.login.is_none());
        assert!(app.books.by_id("book-0").is_some());
        assert!(app.sync.retry_at.is_some());
        app.apply(Action::Go(super::super::Route::Home));
        app.apply(Action::Go(super::super::Route::Book("book-0".into())));
        while !matches!(app.books.details.get("book-0"), Some(super::super::Details::Loaded(_))) {
            assert!(!deadline.passed());
            let msg = app.inbox.recv_timeout(testkit::slack(testkit::WAIT)).unwrap();
            app.handle(msg);
        }
        assert_eq!(shelf.item(&key).unwrap().unwrap().chapters, fixture::details(0).chapters);
        app.sync.retry_at = Some(std::time::Instant::now());
        app.logic(&ctx);
        while source.0.load(std::sync::atomic::Ordering::Relaxed) < 2 || app.sync.running {
            assert!(!deadline.passed());
            let msg = app.inbox.recv_timeout(testkit::slack(testkit::WAIT)).unwrap();
            app.handle(msg);
        }
        assert!(app.session.signed_in() && app.sync.offline);
        assert!(app.sync.retry_at.is_some());
    }

    #[test]
    fn marking_a_book_finished_unpins_it_only_when_the_preference_is_on() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture::app(&egui::Context::default(), dir.path(), 1);
        let store = install(&mut app, dir.path());
        let source = Arc::new(Unreachable(Default::default()));
        app.session.source = Some(source);
        let key = ItemKey::new(fixture::account().id, "book-0");
        let pin = Pin::Book(key.to_string());
        store.pin(pin.clone(), "Book", vec![]);
        app.apply(Action::MarkFinished("book-0".into()));
        assert!(store.is_pinned(&pin));
        app.apply(Action::Prefs(PrefsAction::RemoveDownloadsWhenFinished(true)));
        app.apply(Action::MarkUnfinished("book-0".into()));
        app.apply(Action::MarkFinished("book-0".into()));
        assert!(!store.is_pinned(&pin));
    }
    #[test]
    fn rejected_downloads_refresh_credentials_and_later_books_can_refresh_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture::app(&egui::Context::default(), dir.path(), 0);
        let account = fixture::account();
        let refreshes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = refreshes.clone();
        let api = JsonFake::start(move |seen| {
            if seen.path() == "/auth/refresh" {
                let n = count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                Answer::ok(serde_json::json!({"user":{"id":"user","accessToken":format!("access-{n}"),"refreshToken":format!("refresh-{n}")}}).to_string())
            } else if seen.path().contains("/file/") {
                let required = if seen.path().contains("book-1") { 1 } else { 2 };
                if seen.target.ends_with(&format!("token=access-{required}")) {
                    Answer::new("200 OK", "data").with_header("Content-Type", "audio/mp4")
                } else {
                    Answer::new("401 Unauthorized", "")
                }
            } else {
                let id = seen.path().rsplit('/').next().unwrap();
                Answer::ok(
                    serde_json::json!({"id":id,"libraryId":"lib-books","mediaType":"book","media":{
                        "metadata":{"title":id},"duration":100,"tracks":[{"ino":id,"index":1,"duration":100,"size":4,"metadata":{"ext":".m4b"}}]
                    }})
                    .to_string(),
                )
            }
        });
        let client = abs::Client::new(api.base(), account.device()).unwrap().with_tokens(abs::Tokens::new("expired", "refresh-0"));
        app.session.source = Some(Arc::new(sources::AbsSource::new(account.id, client, vec![])));
        app.bridge.open_downloads(account.id, None, app.stream_dir.clone());
        let deadline = Deadline::new();
        while app.downloads.store.get().is_none() {
            assert!(!deadline.passed());
            let msg = app.inbox.recv_timeout(testkit::slack(testkit::WAIT)).unwrap();
            app.handle(msg);
        }
        for n in 1..=2 {
            let mut saw_rejection = false;
            let key = ItemKey::new(account.id, format!("book-{n}"));
            app.apply(Action::DownloadBook(key.clone()));
            while !app.downloads.book(&key).is_some_and(offline::PinSummary::complete) {
                assert!(!deadline.passed());
                let msg = app.inbox.recv_timeout(testkit::slack(testkit::WAIT)).unwrap();
                app.handle(msg);
                saw_rejection |= app.downloads.snapshot.last_errors.values().any(|error| error.http_status == Some(401));
            }
            assert!(saw_rejection, "each book exposes the typed rejection before credentials are refreshed");
        }
        assert_eq!(refreshes.load(std::sync::atomic::Ordering::Relaxed), 2);
    }
}
