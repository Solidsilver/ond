# 001 · Offline store: pins-file failure deletes downloads; store never shuts down

- Area: `crates/offline` · Kind: bug · Priority: P0
- Status: done (branch fix/offline-store) · Claimed by: —
- Source: data-layer review (findings 11, 12, 14, 15, 16, Drop note). Line numbers at commit 669bb00.

## Problem

1. **A bad `offline.json` wipes every download.** `crates/offline/src/lib.rs:250-256` falls back to `Pins::default()` on *any* read or parse error (not only `NotFound`), then `remove_strays(&cfg.dir, &pins)` (`lib.rs:263`, fn at `lib.rs:890-907`) deletes every file not referenced by the now-empty pins. A permissions error, a torn file, or a serde change on upgrade becomes silent deletion of gigabytes. Verified.
2. **The store never stops.** `OfflineStore::open` spawns two `worker(inner)` tasks (`lib.rs:295-298`) whose loop has no exit (`lib.rs:693-711`); `Drop` only waits for saves (`lib.rs:140-146`). `Inner` is never freed, so the deleter's `Weak` check never fails. The app replaces the store per account (`crates/app/src/app.rs:645,805`), leaking two tasks and two OS threads per switch. The old store keeps its `CacheLookup` and (until `set_offline`) its `UrlFn`, so a pending track completing in the stream cache is promoted by the *old* store, whose `complete()` → `commit()` → saver writes stale pins over the `offline.json` of a *new* store on the same directory (switch A→B→A). Verified leak; cross-write traced.
3. `Drop` blocks up to 5 s on **every** clone drop (`lib.rs:143-145`), not just the last; a UI-thread clone drop can stall behind a large save.
4. `promote()` with a successful hard link leaves any stale `.part` behind (`lib.rs:738-743`); `remove_strays` keeps it because `known` includes `{n}.part`.
5. `Pins::store` (`pins.rs:98-105`) refreshes `s.r` (incl. suffix) for a track currently downloading; the job captured the old names (`lib.rs:569-570`), so on success `local_path` (`lib.rs:408-413`) points at a file that doesn't exist and the real file is a stray.
6. Deleter race (`lib.rs:861-864`): re-pin between the check and the delete unlinks a `.part` being written; self-heals with one failed attempt.

## Fix

- On any `offline.json` error other than `NotFound`: rename to `offline.json.bad-<timestamp>`, log at warn, and **skip `remove_strays` for that session**. Consider refusing to prune when the dir has files not matching the naming scheme and no state file.
- Add `closed: AtomicBool` + `Notify` to `Inner`; keep worker `JoinHandle`s; on the last strong handle dropping (separate `Arc<()>` token or `strong_count`) set `closed`, cancel actives, return from `worker`, and abort the handles. Only wait for flush on the last handle.
- `remove_file(part)` after a successful link in `promote()`.
- Skip the `r` refresh in `Pins::store` while `active` contains the id, or store the file name used in `Stored`.

## Tests to add

- Corrupt / unreadable `offline.json` → files untouched, file moved aside.
- Drop a store with an active download → worker exits, no further writes to the dir; reopen on the same dir sees the right pins.
- Unpin mid-download deletes the `.part`; `update()` shrinking a pin whose dropped track is downloading; `extend()`; limit with unknown sizes.

## Acceptance

Dropping the store ends its tasks within a bounded time; no code path deletes files when the state file failed to load; `cargo test -p offline` covers both.

## Resolution

All six items were real and are fixed, all inside `crates/offline` (no change to `app.rs` was needed: replacing `Downloads::store` drops the old handle, which now shuts the store down).

1. `load_pins` replaces the inline read. A state file that can't be read or parsed is renamed to `offline.json.bad-<ms>`. A state file that's missing while downloads are in the folder gets a note under that name instead. While any `offline.json.bad-*` is in the folder, `remove_strays` is skipped in that session and in every later one. Skipping it for one session wasn't enough: the next save writes a new `offline.json` that doesn't name the old files, so the next launch would delete them. Deleting the `.bad-` file turns tidying back on. Nothing in the UI mentions it yet.
2. and 3. `OfflineStore` now holds an `Arc<Owner>` that the workers don't hold, so the `Owner`'s drop runs when the last handle goes. That drop sets `State::closed` (under the state lock, so a racing commit either counts toward the flush or doesn't happen), cancels active downloads, wakes the workers and aborts them. Only after that does it wait for queued saves. Copies of the handle drop without blocking. When closed, `claim` returns `Next::Stop`, `complete` records nothing and the deleter stops. `Inner` is then freed, which ends the deleter and saver threads.
4. `promote` removes the `.part` after a successful hard link.
5. `complete` records the job's `TrackRef` (the name and size it actually downloaded) along with `done`, so a re-pin mid-download can't point `local_path` at a file that doesn't exist. `Pins::store` is unchanged.
6. The deleter marks the ids it's about to remove in `State::deleting`. `claim` skips them, and when the files are gone the deleter clears the marks and wakes the workers if any of those tracks were pinned again.

Tests: corrupt, unreadable (a folder named `offline.json`) and missing state files; dropping the store mid-download ends it and doesn't overwrite a store reopened on the same folder; a re-pin with another suffix mid-download; promotion leaves no `.part`; re-pin while deleting; unpin and `update()` shrinking mid-download; `extend()`; limit with unknown sizes. `strays_are_removed_on_open` now starts from a saved state file. `unpinning_a_big_playlist…` used to race the deleter (it failed under load) and now hands the deleter a stale batch directly.
