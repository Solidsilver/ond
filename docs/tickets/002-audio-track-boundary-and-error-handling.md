# 002 · Audio engine: wrong track stopped on error; SetNext lost at the track boundary

- Area: `crates/audio` · Kind: bug · Priority: P0
- Status: done (branch fix/audio-engine) · Claimed by: —
- Source: audio review (A1, A3, B2, B5, B6, B7). Line numbers at commit 669bb00.

## Problem

1. **A failing *next* track stops the *current* one** (`crates/audio/src/lib.rs:838-845`). `let seq = seq.min(self.tracks.len().saturating_sub(1))` runs *before* `seq <= self.out_seq`. A next track that fails in `open()` has no `Track` entry, so with `tracks = [A]` its `seq = 1` clamps to 0 → `stop()`, and `Event::Error { track_id: "A" }` blames A. Verified. The comment on the line below says the opposite was intended.
2. **`SetNext` is lost in the decoder gap** (`lib.rs:617-636`). The decision uses `self.tracks.get(out_seq + 1)`, which is `None` between A's EOF and B's first decoded packet (open + probe: tens of ms to seconds). `session.set_next(out_seq, C)` stores `(after = A, C)`; the decoder at `seq = B` does `take_if(after == B)` (`decode.rs:305`) → `None`, so B plays and C only arrives via `Ended`. Verified. `seek()` already handles this correctly with `session.decoding() != self.out_seq` (`lib.rs:602`).
3. Marker pushed before `DecoderMsg::Track` (`decode.rs:489-502`): with an empty ring at the boundary the callback pops the marker at once, `poll_session` sees `crossed` with no `tracks[seq]`, `announce` (`lib.rs:917`) returns silently → no `TrackStarted`, no `report_signal_path`. Plausible.
4. A session that never produces audio (zero-length file, every packet skipped by the format-change guard `decode.rs:437-447`) ends via `lib.rs:875-881` with no `Ended`/`Error`. Plausible; a queue-advancing UI stalls.
5. `next`/`prefetched` survive `Stop` and `Command::Play` (`lib.rs:545-562,564-583`): a fresh `Play(X)` inherits the previous queue's next unless the UI re-sends `SetNext` first.
6. `SetNext` to a finished session (decoder exited `buffer_secs` early) is dropped on a dead thread (`lib.rs:625`); gapless silently lost.

## Fix

- Clamp only for the `track_id` lookup; compare the raw `seq` against `out_seq`.
- In `set_next`, branch on `session.decoding()` vs `out_seq` exactly as `seek()` does; take the restart branch when the decoder has moved on.
- Send `Track` before pushing the marker, or defer `out_seq` advancement until `tracks.len() > out_seq + 1`.
- Keep the `Play` src and emit `Ended`/`Error` when a session ends without audio.
- Clear `next`/`prefetched` in `handle(Command::Play)` and `Command::Stop`; keep in `resume`.

## Tests to add (engine tests drive the real callback through `FakeBackend`)

- Next track fails to open while current plays → current plays out, error names the next track.
- `SetNext` sent after the decoder moved on but before its `Track` message → replacement honoured.
- Empty / no-audio session emits an event.
- `Play` after a queue does not inherit a stale `next`.

## Acceptance

All four tests pass; existing 81 audio tests still pass.

## Resolution

1. Fixed. The decoder's `Error` now carries the failing track's id (it knows what it was opening), and the engine compares the raw `seq` with `out_seq`. A next track that fails to open is reported by its own id while the current one plays out and ends normally.
2. Fixed. `Session::set_next` only accepts the choice while the decoder is still on `after`, checked under the slot lock that the decoder takes the next track (and bumps `at_seq`) under, so the answer can't go stale. When it's refused, the engine compares against what the decoder has reported, or, in the gap before the new track's first audio, against what it was last told, and restarts the current track at its position when they differ (same branch as before).
3. The mechanism described isn't quite right (the callback only pops a marker once samples after it are queued, and those are pushed after `Track` is sent), but a narrower race is real: the callback can cross between `poll_session` draining messages and loading `track_seq`. `out_seq` now only advances onto tracks whose `Track` report is in; the crossing is announced on the next poll. No deterministic test (a two-statement window).
4. Fixed. The engine keeps the session's first source (`opening`) and emits `Ended` for it when the session ends without ever producing audio.
5. Fixed. `Play` and `Stop` clear `next`/`prefetched`; `resume` (restarts) still carries them over.
6. Fixed differently: at the end of the last queued track the decoder now waits (`linger`) while the output has more than 0.1 s of that track left, so a next track named once the output reaches it (what the UI does) still follows gaplessly. Before, a track shorter than the buffer was fully decoded before the UI had a chance to name its next, so its successor always reopened the device. A seek that arrives during the wait lands in place. Follow-up (branch fix/audio-engine-seek-stall): the wait also ends when the output is prebuffering (not holding for a seek). Without that, a last track shorter than the 0.4 s prebuffer, or one that ran dry near its end, deadlocked: the output waited for more audio or the end of input, the decoder for the output to drain. Test: `a_track_shorter_than_the_prebuffer_plays_out`.

Tests added (`crates/audio/src/tests.rs`): `a_next_track_that_fails_to_open_lets_the_current_one_play_out`, `a_next_named_while_the_decoder_opens_the_old_one_is_honoured` (uses a `test-slow://` source that takes 0.5 s to open), `a_track_without_audio_still_ends`, `play_does_not_inherit_the_previous_queues_next`, `a_short_track_waits_for_its_next_to_be_named`. All five failed before the fix.

Hardware: nothing specific; the TESTING.md gapless check (a live album) covers it.
