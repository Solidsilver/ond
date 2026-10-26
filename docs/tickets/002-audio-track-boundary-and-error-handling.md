# 002 · Audio engine: wrong track stopped on error; SetNext lost at the track boundary

- Area: `crates/audio` · Kind: bug · Priority: P0
- Status: open · Claimed by: —
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
