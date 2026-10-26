# 006 · streamcache: cancel_all leaves dead downloads attachable; range edge cases

- Area: `crates/streamcache` · Kind: bug · Priority: P1
- Status: open · Claimed by: —
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
