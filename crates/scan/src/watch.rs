//! Native folder events, with shallow watches above each root so a removed
//! drive or directory can be noticed when it returns. Access events are
//! ignored: scanning and playback must never trigger their own rescans.
//!
//! The caller coalesces notifications and calls [`FolderWatch::refresh`]
//! before scanning to move watches onto roots that have reappeared. There
//! are no timers or polling threads while the folders are unchanged.

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Recursive watches for music roots and shallow watches for their parents.
pub struct FolderWatch {
    watcher: RecommendedWatcher,
    roots: Vec<PathBuf>,
    watched: HashMap<PathBuf, RecursiveMode>,
    invalidated: Arc<AtomicBool>,
}

impl FolderWatch {
    /// Starts watching, including the nearest existing ancestor of a missing
    /// root. Errors from a backend (including overflow) also request a scan:
    /// an incremental scan is how the caller recovers a complete listing.
    pub fn new(roots: Vec<PathBuf>, on_change: impl Fn() + Send + 'static) -> notify::Result<Self> {
        let roots: Vec<PathBuf> = roots.into_iter().map(|p| event_path(&p)).collect::<std::io::Result<_>>()?;
        let relevant = roots.clone();
        let invalidated = Arc::new(AtomicBool::new(false));
        let changed_root = invalidated.clone();
        let watcher = notify::recommended_watcher(move |result: notify::Result<Event>| match result {
            Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                if event.need_rescan() || event.paths.iter().any(|p| relevant.iter().any(|r| r.starts_with(p))) {
                    changed_root.store(true, Ordering::Release);
                }
                if event.need_rescan() || event.paths.iter().any(|p| relevant.iter().any(|r| p.starts_with(r) || r.starts_with(p))) {
                    on_change();
                }
            }
            Err(e) => {
                changed_root.store(true, Ordering::Release);
                tracing::warn!("watching music folders: {e}");
                on_change();
            }
            _ => {}
        })?;
        let mut this = Self { watcher, roots, watched: HashMap::new(), invalidated };
        // Keep successful watches when one root cannot be attached. The
        // scan reports missing roots, and later events or Rescan retry them.
        if let Err(e) = this.refresh() {
            tracing::warn!("attaching music folder watches: {e}");
        }
        Ok(this)
    }

    /// Reattaches recursive watches after a root returns, retaining an
    /// ancestor watch while it is absent. A failed attachment is retried on
    /// the next event or explicit rescan.
    pub fn refresh(&mut self) -> notify::Result<()> {
        let mut wanted = HashMap::new();
        for root in &self.roots {
            if root.is_dir() {
                wanted.insert(root.clone(), RecursiveMode::Recursive);
            }
            if let Some(parent) = root.parent().and_then(existing_ancestor) {
                wanted.entry(parent.to_owned()).or_insert(RecursiveMode::NonRecursive);
            }
        }
        // A backend can discard a watch when a root is removed, even if the
        // same path has been recreated before the debounce window ends.
        let invalidated = self.invalidated.swap(false, Ordering::AcqRel);
        self.watched.retain(|path, mode| {
            if !invalidated && wanted.get(path) == Some(mode) {
                true
            } else {
                // The backend may already have removed a watch for a vanished root.
                let _ = self.watcher.unwatch(path);
                false
            }
        });
        let mut error = None;
        for (path, mode) in wanted {
            if !self.watched.contains_key(&path) {
                match self.watcher.watch(&path, mode) {
                    Ok(()) => {
                        self.watched.insert(path, mode);
                    }
                    Err(e) => error = Some(e),
                }
            }
        }
        error.map_or(Ok(()), Err)
    }
}

fn existing_ancestor(path: &Path) -> Option<&Path> {
    path.ancestors().find(|p| p.is_dir())
}

/// FSEvents reports canonical paths (for example `/private/var` on macOS).
/// Resolve the part that exists even when the root itself is absent.
fn event_path(path: &Path) -> std::io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    match absolute.ancestors().find(|p| p.exists()) {
        Some(ancestor) => Ok(ancestor.canonicalize()?.join(absolute.strip_prefix(ancestor).expect("ancestor prefix"))),
        None => Ok(absolute),
    }
}
