# 003 · Audio engine: seek request race and stale held seek

- Area: `crates/audio` · Kind: bug · Priority: P1
- Status: open · Claimed by: —
- Source: audio review (A4, A5, Push::Interrupted note). Line numbers at commit 669bb00.

## Problem

1. **Seek flag race.** Engine side (`crates/audio/src/decode.rs:165-168`): `seek_pending.store(true)` then `seeks.send(req)`. Decoder side (`decode.rs:600-602`): `store(false)` then `try_iter().last()`. If the decoder runs between the two engine statements it returns `Seeked::Nothing`; the request then lands with `seek_pending == false` and is never read. The output is holding (`hold_gen` bumped at `lib.rs:609-614`) → silence/Buffering indefinitely; `held_seek` is only rescued when the session finishes. Verified ordering. `push_all` may also have returned `Interrupted` leaving `pushed` mid-frame with no flush to realign.
2. **Stale `held_seek` restarts the next track.** `Seeked::End` (`decode.rs:619`) and the seek-error path (`:620-625`) send no message; `lib.rs:825` clears `held_seek` only on `Seeked`. With B queued: A ends via `End`, B plays gaplessly, then when the decoder exits (`lib.rs:852-859`) `restart_at(B, to)` jumps B to A's seek target plus a spurious `Seeked` event.
3. `Push::Interrupted` (`decode.rs:519-522`) is handled like `Done`; correctness relies on a flush always following. Document or realign.

## Fix

- Send first, then `store(true)` (pending implies readable); have the decoder's `seek()` keep `seek_pending` true when it finds nothing.
- Report every seek outcome (`Seeked` / `SeekEnded` / `SeekFailed`), or clear `held_seek` whenever `out_seq` advances.

## Tests to add

- Seek storm (many seeks within a few ms) exercising hold/flush generations.
- Seek past end with a queued next → next track starts at 0, exactly one `Seeked`-class event.
- Seek error path.
