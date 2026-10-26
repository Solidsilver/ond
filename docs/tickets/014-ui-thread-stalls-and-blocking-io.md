# 014 · UI-thread stalls and blocking file I/O on the tokio runtime

- Area: `crates/app` (app.rs, offline usage), `crates/offline`, `crates/streamcache` · Kind: perf/robustness · Priority: P2
- Status: done (branch fix/blocking-io) · Claimed by: —
- Source: app-core review (C), data-layer review (6, 13-C). Line numbers at commit 669bb00.

## Problem

1. `Downloads::switch_account` on `Msg::Connected` (`crates/app/src/app.rs:905`) → `OfflineStore::open` (`crates/offline/src/lib.rs:248-263`) reads the state file, `is_file()`s every downloaded track and `read_dir`s for strays, on the UI thread. Thousands of downloads → tens of ms on the first `Connected` per account.
2. `offline::move_store` runs before the first frame (`app.rs:795-799` → `offline/src/lib.rs:957-985`), falling back to copy+remove across volumes synchronously: a blank window for the duration of a large move.
3. Both downloaders do blocking fs on the 4-worker runtime (`app.rs:759`): `streamcache/src/download.rs:98,162,165,185` (open, set_len, seek, write per chunk) and `offline/src/lib.rs:747,771,786`; `p.is_file()` runs under the offline state lock in `claim()` (`:553`). Shared with sync, art, keychain.
4. `Runtime` drop (`OndApp._rt`) waits for in-flight `spawn_blocking` with no timeout; quit can lag by one blocking op.
5. `OfflineStore::open` on a non-dedicated directory deletes unrelated files (see ticket 001); `move_files` leaves `.{name}.moving` temp files after a crash and `remove_strays` skips dotfiles (`lib.rs:899`).

## Fix

- Open the offline store off-thread and deliver it via `Msg`; run `move_store` in the background with a notice.
- A dedicated blocking writer (or `spawn_blocking`) for the file side of both downloaders.
- `shutdown_timeout` in `Drop`.
- Clean `.moving` files on open.

## Resolution

1. Fixed. `Downloads::store` is now an `Option`. Switching accounts takes the old store out and hands it, with the open of the new one, to `Bridge::offline_job`: a chain of `spawn_blocking` jobs that run one at a time, in order. The old store's shutdown (which waits for its last save) finishes before a store is opened on the same folder again, even when switching back and forth quickly. The store arrives as `Msg::OfflineStore`, tagged with an open number; one for an open that a newer switch replaced is shut down on the same chain. While it opens, the snapshot is empty, so nothing plays from disk (the old account's files aren't this account's), and download actions (keep, unpin, pause, retry, limit) wait in `Downloads::deferred` and run once it's open. Album and playlist loads skip pin refreshes meanwhile, since there are no pins yet. On arrival the store gets the server if one is connected, pinned playlists are refetched, and an offline queue re-checks what can play. If opening fails, the error is shown and the next `Connected` for that account tries again. The startup open with no move pending stays synchronous, before the first frame: it's one per launch, and the queue and the tests expect the store from the start.
2. Fixed. A pending `offline_move_from` now starts the same background job: `offline::move_store_with` first, then the open. A notice ("Moving downloads to …: X of Y", updated at most every 500 ms through a new `Toasts::replace`) shows progress, followed by "Downloads moved to …" or an error. Offline playback waits for the move to finish. The pref is cleared only after a successful move, so a quit or crash mid-move resumes at the next start. A move was already safe to cut short (copy to a temporary name, rename, then delete the source). It now also finishes cleanly: a file found at both ends with the same size is one whose copy completed, so its original is removed instead of left in the old folder. Choosing another folder while a move runs is refused with a notice, because that would forget where the first move came from.
3. Fixed. `streamcache::disk` has `blocking()` and `PartFile`, a file whose open, `set_len`, positioned write and `sync_all` each run on `spawn_blocking`. Both downloaders use it. In the stream cache, a chunk's range is still published to readers only after its write returns, so read-while-downloading behaves as before. In `offline::download()` only the file mechanics changed (metadata, open, write, remove); the HTTP and resume logic didn't. `claim()` no longer runs `is_file()` under the state lock: a cached copy that has gone fails to promote, and the job downloads instead, or with no server, fails and backs off. A chunk costs one round trip to the blocking pool. Under fast transfers hyper hands over larger chunks, so that cost stays small next to the write. `spawn_blocking` works on both runtime flavours (the bridge tests use a current-thread one).
4. Fixed. `OndApp` holds the runtime in `AppRuntime`, whose drop calls `shutdown_timeout(2 s)`.
5. Fixed for `.moving`: `OfflineStore::open` deletes `.{name}.moving` files (only that pattern) whether or not strays are trusted, since such a file is always an unfinished copy of a source that's still whole. `move_files` also deletes them from the source instead of carrying them along. The non-dedicated-directory part was already covered: the app always keeps downloads in an `Ond Offline` folder of its own.

Tests: `copies_left_by_an_interrupted_move_are_removed_on_open`, `a_move_cut_short_finishes_when_run_again` (offline); `quitting_waits_only_a_moment_for_blocking_work`, `another_accounts_downloads_open_in_the_background_and_wait_for_nothing`, `downloads_moved_to_another_folder_move_in_the_background`, and `another_account_gets_its_own_library_and_downloads` updated (app); `offline_jobs_run_one_after_another_off_the_async_workers` (bridge). Manual checks are in `TESTING.md` › Offline.
