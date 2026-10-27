# 007 · App: library analysis silently cancelled at startup; UI-thread cost of enqueueing

- Area: `crates/app` (analyze.rs, app.rs) · Kind: bug + perf · Priority: P1
- Status: done (branch fix/app-core) · Claimed by: app-core agent
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

## Resolution

1. Fixed. `Analysis::library_run` is set when `resume_library_analysis` has actually queued the library, and `check_library_analysis` (called from `logic()`) only finishes the run when that flag is set and the runner has gone idle. Startup with `AllTracks`/`AnalysesLoaded` before `Connected`, and offline startup, no longer clear `analysis_resume`. Signing out now drops the runner's queue and its server (the old account's jobs used to keep fetching with its credentials) and resets `library_run`. The preference survives, so the run resumes for whoever signs in next.
2. The stream URL (salt + MD5 + URL building per track, the bulk of the cost) is now made lazily: `Job` has no `url`; `Runner::set_server` holds a `StreamUrl` closure (set on `Connected`, cleared on sign-out) and the worker makes the URL as it starts a fetch. Enqueueing still filters on the UI thread (a hash lookup, plus the `local_path` lookup only for unanalysed tracks), which is cheap. I didn't move the whole build to `spawn_blocking`: the run's "nothing to do → finished" decision needs the count straight away.
3. `State` keeps `queued_secs` as jobs are pushed, popped and cleared, so `progress()` is O(1). `logic()` only calls it during a library run.
4. Fixed: `enqueue` treats a cleared in-flight job (empty stamp) as not running, so it can be queued again straight away.
5. The watcher waits on a Condvar with a 100 ms timeout instead of sleeping. The poll stays because "playback needs the network" has no signal, but the end of a job now wakes it at once, so each network job no longer waits up to 100 ms for its watcher.

Tests: `a_library_analysis_resumes_once_the_server_answers` (app.rs: AllTracks/AnalysesLoaded before Connected, offline, sign-out, run finishing), `a_cleared_job_can_be_queued_again_while_it_stops` (analyze.rs), plus running-total checks in `progress_counts_queue_and_estimates_time`. Not added: a `run()`-level pause/`waiting` test, which would rely on timing.

Outside scope: `crates/app/src/app/snapshots.rs` drops the removed `url` field from one `Job` literal (one line).
