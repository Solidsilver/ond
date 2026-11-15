//! One local source's watch and scan queue. Native events carry their last
//! arrival time, so a whole album copy settles before scanning. Requests
//! received during a scan collapse into one follow-up pass; nothing polls
//! or wakes the UI while the source is idle.

use super::{Bridge, Msg};
use crate::failure::Failure;
use crate::source::Src;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// How long a folder must be quiet before it's rescanned. Tests use less:
/// at two seconds a test that unplugs and replugs a folder spends most of
/// its wait budget here, and times out when the suite runs in parallel.
/// Their bursts are a tight loop of copies, well inside the shorter window.
const SETTLE: Duration = if cfg!(test) { Duration::from_millis(500) } else { Duration::from_secs(2) };

pub(super) struct LocalWatch {
    source: Src,
    request: watch::Sender<()>,
    task: tokio::task::AbortHandle,
}

impl LocalWatch {
    pub(super) fn start(bridge: Bridge, source: Src) -> Self {
        let (request, requests) = watch::channel(());
        let task = bridge.rt.spawn(run(bridge.clone(), source.clone(), requests)).abort_handle();
        Self { source, request, task }
    }

    pub(super) fn rescan(&self, source: &Src) -> bool {
        if !Arc::ptr_eq(&self.source, source) {
            return false;
        }
        self.request.send_replace(());
        true
    }
}

impl Drop for LocalWatch {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn run(bridge: Bridge, source: Src, mut requests: watch::Receiver<()>) {
    let roots = source.local().expect("local source").folders().to_vec();
    let (events, mut changes) = watch::channel(None);
    let watcher = bridge
        .rt
        .spawn_blocking(move || {
            scan::watch::FolderWatch::new(roots, move || {
                events.send_replace(Some(Instant::now()));
            })
        })
        .await;
    let mut watcher = match watcher {
        Ok(Ok(w)) => Some(w),
        result => {
            let error = match result {
                Ok(Err(e)) => e.to_string(),
                Err(e) => e.to_string(),
                Ok(Ok(_)) => unreachable!(),
            };
            bridge.send(Msg::Error(Failure::Local(format!("Couldn't watch music folders: {error}"))));
            None
        }
    };
    loop {
        if !bridge.current() {
            return;
        }
        // Watch setup precedes the startup scan so changes during it are kept.
        if let Some(mut w) = watcher.take() {
            watcher = bridge
                .rt
                .spawn_blocking(move || {
                    if let Err(e) = w.refresh() {
                        tracing::warn!("refreshing music folder watches: {e}");
                    }
                    w
                })
                .await
                .ok();
        }
        bridge.scan_local(source.clone()).await;
        let mut settle: Option<Instant> = None;
        loop {
            tokio::select! {
                changed = requests.changed() => {
                    if changed.is_err() { return; }
                    // An explicit Rescan also covers events already received.
                    changes.borrow_and_update();
                    break;
                }
                changed = changes.changed(), if watcher.is_some() => {
                    if changed.is_err() { return; }
                    settle = *changes.borrow_and_update();
                }
                () = async {
                    match settle {
                        Some(last) => tokio::time::sleep_until((last + SETTLE).into()).await,
                        None => std::future::pending().await,
                    }
                } => {
                    // A timeout and a fresh event can become ready together.
                    // Always check the latest timestamp before starting a scan.
                    let latest = *changes.borrow_and_update();
                    if latest.is_some_and(|last| last + SETTLE > Instant::now()) {
                        settle = latest;
                        continue;
                    }
                    break;
                },
            }
        }
    }
}
