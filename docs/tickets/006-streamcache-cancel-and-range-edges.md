# 006 · streamcache: cancel_all leaves dead downloads attachable; range edge cases

- Area: `crates/streamcache` · Kind: bug · Priority: P1
- Status: done (branch fix/streamcache) · Claimed by: —
- Source: data-layer review (findings 1–10). Line numbers at commit 669bb00.

## Problem

1. **`cancel_all()` doesn't drain `active`** (`crates/streamcache/src/lib.rs:262-267`); removal happens only in the task's `finish` (`lib.rs:373-376`). An `open()` right after → `acquire()` (`lib.rs:333-337`) attaches to the cancelled entry, and the first `read()` fails at `lib.rs:514` with "read aborted". `crates/audio/src/lib.rs:474` calls `cancel_all()` then opens the next track. Verified.
2. **416 loop when the declared total exceeds the real length** (`download.rs:118-126,191-194,227-231`): 416 → `failures = 1; start = 0` → chunk arrives → `failures = 0` → `first_gap` returns the unreachable tail → 416 → … one full HTTP request per cycle, never reaching `MAX_FAILURES`. Plausible (transcode with estimated length, truncated file).
3. **206 with `Content-Range: bytes a-b/*` plus a relocation** can finalize a file with a hole: `download.rs:220-226` treats "no length and body ended" as complete. Plausible for non-Navidrome servers.
4. `cancelled(dl)` (`download.rs:255-259`) eats the `Notify` permit a reader stores via `wake.notify_one()` (`lib.rs:519`) while the task is in `get_from`/backoff; a stalled response makes the reader wait the full 20 s read timeout instead of relocating.
5. Eviction / `clear()` delete deferred past a concurrent re-download's rename (`lib.rs:286-287,381-384`); self-heals via `open_complete` (`lib.rs:323-327`).
6. `Abort` closures run with the download-state mutex held (`lib.rs:220,514`); callers today only read atomics (`audio/src/decode.rs:338`, `app/src/analyze.rs:346`). Document the constraint or evaluate `abort()` after dropping the guard. Also note reliance on `reqwest::Response::chunk()` cancel-safety in `select!` (`download.rs:170-182`).
7. Blocking fs on tokio workers per chunk (`download.rs:98,162,165,185`); see ticket 014.

## Fix

- In `cancel_all`, `drain()` `active` under the lock after cancelling (the `ptr_eq` checks in `release`/`finish` already tolerate it).
- On 416 at a gap start, shrink `st.len` to the gap start, or count 416s separately from chunk-reset failures.
- In the `None`-length branch, require `ranged != Some(true)` or `ranges.covers(pos)` before declaring done.
- Check `relocation()` right after response headers, or use a separate `Notify` for cancel.

## Tests to add

`cancel_all` then immediate `open`; declared total > real length; Content-Length missing (chunked 200); 206 with `*` total; two readers seeking independently; backward seek into a gap; eviction racing a re-download.

## Resolution

1. Fixed. `cancel_all` drains `active` as it cancels, and `acquire` also skips a cancelled entry. Test: `cancel_all_then_open_starts_a_fresh_download`.
2. Fixed. A 416 at offset `n > 0` cuts the length to `n` (and truncates the `.part`), then the download fetches whatever is still missing below it. Each cut is strictly smaller, so it ends; a 416 at 0 still counts as a failure. Readers take the shrunk length. Tests: `a_declared_length_past_the_real_end_ends_at_the_data`, `a_seek_past_the_end_of_a_file_without_a_total_reads_as_the_end`.
3. Fixed, and wider than described: the end-of-response step mixed up "body ended" and "ran into bytes already on disk", so with no total, a gap fill that reached a relocated span also finalized the file with a hole. A response now reports how it stopped. Only a body that ends sets an unknown length, and either way the file completes only when every byte below the length is on disk. Test: `ranges_without_a_total_fill_the_skipped_part` (failed before the fix).
4. Not a real delay. The swallowed wake-up only happens in `get_from` or a backoff, and `get_from` returns holding the first chunk, after which `relocation()` checks `wanted`. Documented on `cancelled()`, with a regression test: `a_seek_while_reconnecting_is_not_lost` (passes on the old code too).
5. Fixed. `finish` renames under the index lock. `remove` skips a name that's back in `complete`; otherwise it moves the file to a `.part` name under the same lock and deletes it after. Test: `a_late_eviction_spares_a_file_downloaded_again`.
6. Documented on `Abort` (called with the state lock held, so it must only read a flag) and at the `select!` over `body.chunk()` (cancel-safe).
7. Left for ticket 014.

Also added: `a_200_without_content_length_is_read_to_its_end`, `a_backward_seek_into_a_gap_fetches_it`, `two_readers_seek_independently`. The last one shows that two readers blocked far apart on one download make it alternate between them every chunk or so: correct, but one request per few KB. Nothing in the app does that today (analysis reads the playback cache only for complete files), so it's left as is.
