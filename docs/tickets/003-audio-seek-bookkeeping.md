# 003 · Audio engine: seek request race and stale held seek

- Area: `crates/audio` · Kind: bug · Priority: P1
- Status: done (branch fix/audio-engine) · Claimed by: —
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

## Resolution

1. Fixed. `Session::seek` sends the request first and sets `seek_pending` after, so a decoder that sees the flag always finds the request (clear-then-drain on the decoder side covers the rest). Keeping the flag set when the decoder finds nothing wasn't needed and would spin. `push_all` now gives way to a seek only on a frame boundary (it finishes a half-pushed frame first), so a stale flag can't leave the ring mid-frame with no flush to realign it.
2. Fixed both ways: the decoder reports a past-the-end seek as `Seeked` (at the track's end; the engine sends one `Event::Seeked`), and the engine drops `held_seek` whenever the output crosses into another track (it can't cross while genuinely holding). The seek-error path already reported an `Error` (and the engine stops); it now has a test.
3. Documented at the call site and fixed for alignment as above. The remainder of the interrupted packet is still dropped if the seek turns out to be a no-op.

Found while testing: a session whose first track has no audio (a restart at a position past the end, with a next track queued) gave the next track sequence number 1 while the engine's track list was empty, which hit a `debug_assert` (and misindexed tracks in release builds). A track that produces no audio now hands its sequence number to the next one. The restart flag is only set on the restarted track itself. A past-the-end seek before any audio no longer sets `frames_played`, so the next track starts its count at zero.

Tests added: `a_seek_past_the_end_moves_on_to_the_next_track_from_its_start`, `a_seek_past_the_end_after_the_decoder_moved_on_starts_the_next_track`, `a_seek_storm_settles_on_the_last_target_and_plays_on`, `a_seek_the_source_cant_serve_is_an_error_not_a_stuck_hold` (uses a `test-unseekable://` source) in `tests.rs`, and `a_seek_interrupts_a_push_on_a_frame_boundary` in `decode/tests.rs`. The two past-the-end tests and the push test failed before the fix. The storm and seek-error tests passed before too: the flag race is a window between two statements on the engine thread that a test can't hit on demand, so the ordering fix is correct by construction and the storm test is regression coverage.

Hardware: nothing new beyond TESTING.md's "Seek into an undownloaded part" and "Previous in the last seconds of the queue" checks.
