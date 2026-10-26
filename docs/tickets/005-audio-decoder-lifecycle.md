# 005 · Audio engine: cancelled decoder still writes shared Status

- Area: `crates/audio` · Kind: bug (plausible) · Priority: P2
- Status: open · Claimed by: —
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
