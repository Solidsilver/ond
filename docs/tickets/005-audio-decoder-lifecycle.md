# 005 · Audio engine: cancelled decoder still writes shared Status

- Area: `crates/audio` · Kind: bug (plausible) · Priority: P2
- Status: done (branch fix/audio-lifecycle) · Claimed by: —
- Source: audio review (B1, B8, B9, B10, E notes). Line numbers at commit 669bb00.

## Problem

- `Session::cancel` (`crates/audio/src/decode.rs:176-178`) only stores `cancel`; the thread isn't joined. After `Engine::stop()` + a new `Session::start`, the old thread can still run `status.gains.set(seq, ..)` (`decode.rs:487`, no cancel check between the cfg read and the set), `status.frames_played.store(0)` (`:572`) or `frames_played.store(played)` (`:657`). Both sessions use slot 0 → the new track can get the old track's ReplayGain or a zeroed position. `lib.rs:569` comment "no callback runs after this" is true for the device, not the decoder.
- `Status` fields are written from three threads (`frames_played` by decoder, callback and engine) with no per-field owner documentation.
- Format change cuts the old track's last callback buffer (`decode.rs:563-568`, `lib.rs:889`): audibly truncated (not gapless anyway).
- Spurious underrun at end of input (`output.rs:231` loads `done` before consuming). Cosmetic.
- `needs_dither` evaluated after `fill` (`output.rs:166-172`): one undithered buffer when a ramp reaches unity. Negligible.
- Theoretical division by zero on `channels` in the callback (`output.rs:255,287,359`) if a device reports 0 channels; `channels.max(1)` is applied elsewhere.

## Fix

- Join in `cancel()` (the abort makes reads return quickly), or tag `Status` writes with a session epoch and ignore stale ones.
- Document ownership of each `Status` field.

## Tests to add

- Rapid `Play`/`Play` and `Stop` during Loading (session replaced while decoder is in `open`) with a check that the new session's gain/position are not clobbered.
- Engine-level underrun → rebuffer → resume.

## Resolution

1. Fixed, without joining. Every write the decode thread makes to `Status` (a track's gain, a seek's position) goes through `Ctx::publish`, which takes a per-session lock and does nothing once the session is cancelled; `Session::cancel` sets the flag under the same lock. When `cancel()` returns, the old thread can't write any more, with no window between check and write. Joining was the other option, but the thread can be in a network read (up to ~50 ms to notice) or decoding a packet, and every Play and Stop would wait for that on the engine thread. A session epoch would have meant packing an epoch into `frames_played` (which the callback also adds to) and every gain slot. What the old thread still does while it winds down only touches its own ring and channels, which nothing reads.
2. Documented: `Status` now says which of the engine, callback and decoder writes each field, and the two rules that keep them apart. The decoder's `frames_played.store(0)` on a format change moved to the engine (`open_ring`, while no callback runs), so the decoder's only position write is a seek's, ordered after the ring flush.
3. Format change cut the last buffer: fixed, and the end of the queue had the same problem. `fill` now reports drained only from a callback that had nothing left to write (the one that writes the last samples returns false, because the device hasn't played that buffer yet). `fill_converted` ANDs its chunks. Before reopening, the engine waits up to 250 ms for the old stream to report drained (normally one device period; the cap covers a pause pressed at that moment).
4. Spurious underrun at end of input: fixed. On a shortfall the callback loads `input_done` again before counting an underrun. No deterministic test: the window is between two loads inside one callback.
5. `needs_dither` after the ramp: fixed together with 018. The callback decides dither per stretch it writes (a stretch is gained if the gain was off unity at its start or end), so a ramp that ends mid-buffer is dithered all the way.
6. `channels == 0`: `fill` clamps to 1 (mono) at the top. It covers `fade_out` and `apply_gain`, which only run through `fill`.

Tests added (`crates/audio/src/tests.rs` unless noted):
- `a_replaced_decoder_cannot_overwrite_the_new_tracks_gain` and `a_decoder_stopped_while_loading_cannot_move_the_next_tracks_position`. Tracks with ids starting `late` stall 300 ms before each `Status` write, ignoring cancel, like a descheduled thread. The test waits until the old decoder is inside the stall, starts the new track paused, waits until the old write has been attempted, then checks the new track's gain and position. Both failed before the fix (gain −12 dB instead of −3 dB, position 5 s instead of 0).
- `rapid_plays_settle_on_the_last_track_with_its_own_gain_and_position`: 40 Plays with Seeks and Stops mixed in, then a paused Play. A smoke test for hangs and leftovers.
- `a_format_change_and_the_queue_end_play_out_to_the_last_sample`: failed before the fix (7,364 samples lost).
- `an_underrun_rebuffers_and_resumes_where_it_left_off`: an engine-level underrun → rebuffer → resume over the slow test server, bit-perfect so every sample can be checked.
- `output.rs`: `a_device_without_channels_gets_silence_not_a_panic`, `a_volume_ramp_that_reaches_unity_mid_buffer_is_dithered_all_the_way` (failed before), and `starvation_after_input_done_is_drain_not_underrun` updated for the new drained rule.

`FakeBackend` changes: like a real device, it plays a buffer during the period after the callback filled it, so stopping in between loses that buffer (needed to see item 3). Its period and buffer size can be set per test. It dithers through `fill_for_device` when its caps have `active_bits`.

Every new threaded test ran 20× in a loop while the whole `audio` suite ran alongside in a loop: no failures.

Hardware (added to TESTING.md): the first track's last note before a 44.1 → 48 kHz change, and the queue's tail, aren't clipped.
