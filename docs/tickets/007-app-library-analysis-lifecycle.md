# 007 · App: library analysis silently cancelled at startup; UI-thread cost of enqueueing

- Area: `crates/app` (analyze.rs, app.rs) · Kind: bug + perf · Priority: P1
- Status: open · Claimed by: —
- Source: app-core review (A1, B5, D1, D2). Line numbers at commit 669bb00.

## Problem

1. **Resume never happens.** `logic()` (`crates/app/src/app.rs:2306-2308`) runs `finish_library_analysis()` when `analysis_resume && loaded && !tracks.all.is_empty() && !runner.progress().active()`. But `resume_library_analysis()` (`app.rs:1603-1611`) only enqueues when `session.client.is_some()`. Normal startup: `AllTracks`/`AnalysesLoaded` land in ~100 ms, `Connected` after a network RTT (or never, offline). In between nothing is queued, `active()` is false, and `logic()` clears `analysis_resume`. Verified.
2. `enqueue_analysis` runs over the whole library on the UI thread (`app.rs:1584-1606`), called from `AllTracks`, `AnalysesLoaded`, `Connected`, `AnalyzeLibrary`: per track a `stamp_of` `format!`, an `OfflineStore` lock + lookup, and a `stream_url(..).to_string()`. 20k tracks ≈ 50–150 ms stall per call.
3. `runner.progress()` every frame (`app.rs:2306`) sums `secs` over the whole queue under the shared lock (`analyze.rs:87-101`): O(queue) per frame during a library run, contending with the worker's `stop()` polls.
4. `Runner::clear()` then an immediate `enqueue` of the in-flight track loses it (`analyze.rs:158-162, 281, 296`).
5. `analyze.rs:364` polls with `std::thread::sleep(100 ms)` in the watcher thread; a `Condvar` would avoid it (minor).

## Fix

- Gate the `logic()` check on the same conditions as `resume_library_analysis` (client present), or track a per-session "library run started" flag and only finish after it has actually run. Prefer moving the whole lifecycle into an `Analysis` module (see ticket 016).
- Build jobs in `spawn_blocking` (the runner dedups) or build URLs lazily in the worker.
- Keep a running `remaining_secs` in the runner's `State`.
- Allow re-queueing a cleared in-flight track.

## Tests to add

`analysis_resume` across `AllTracks` → `Connected` ordering (and offline: flag survives); `clear()` mid-job then `enqueue`; pause/`waiting` while buffering at the `run()` level.
