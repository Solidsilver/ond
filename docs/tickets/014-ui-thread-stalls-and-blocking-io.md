# 014 · UI-thread stalls and blocking file I/O on the tokio runtime

- Area: `crates/app` (app.rs, offline usage), `crates/offline`, `crates/streamcache` · Kind: perf/robustness · Priority: P2
- Status: open · Claimed by: —
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
