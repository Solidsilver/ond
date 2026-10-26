# 004 · Audio devices: default change ignored while playing; mode/hog/listener edges

- Area: `crates/audio` (CoreAudio) · Kind: bug · Priority: P1
- Status: open · Claimed by: —
- Source: audio review (A2, B3, B4, perf notes, E notes). Line numbers at commit 669bb00.

## Problem

1. **Playback never follows a default-device change while a stream is open.** `crates/audio/src/coreaudio/mod.rs:298-301` `current_device()` returns `self.open.dev` when open; `lib.rs:710-714` switches only when `backend.current_device().uid != current`, which is therefore never true mid-playback. Plugging headphones into an Apple-silicon Mac leaves audio on the speakers until the next reopen. Verified. `FakeBackend::current_device` (`tests.rs:49-53`) models the *target* device, so tests can't see it.
2. `Render::device_bits` is frozen at `start_unit` (`coreaudio/mod.rs:173`); `set_mode(BitPerfect)` (`:338-340`) may change the physical format and updates only `Open::active_bits`. Callback dithers to the old width.
3. `hal::Listener` drop (`coreaudio/hal.rs:270-282`) frees the `Box` right after `AudioObjectRemovePropertyListener`; Apple doesn't guarantee in-flight completion. `hal.rs:263` derives `*mut` from `&*callback` and `shim` (`:286`) creates `&mut` through it (aliasing UB under Stacked Borrows, harmless today).
4. Every same-device reopen releases and retakes hog (`coreaudio/mod.rs:353-363,384-390`): extra HAL notifications, possible pops.
5. `set_nominal_rate` / `set_physical_format` block the engine thread up to 2 s each during `open` (`mod.rs:158-159`, `hal.rs:175-218`); `Pause` queues behind them.
6. `device_switched(Some(Normal))` on device loss (`lib.rs:709`) ignores the remembered mode for the fallback device; `lib.rs:520` uses the UID as the display name.
7. `cache.progress()` is called twice per 10 ms tick (`lib.rs:773-806`).

## Fix

- Split the trait: `resolve()` ("device a new open would use") vs `open_device()` (for caps); compare `poll_devices` against the former. Give `FakeBackend` an open-device notion.
- Share `device_bits` via `Arc<AtomicU32>` or reopen on mode change.
- `Box::into_raw` on registration and defer/leak the free (or `Arc` + `Weak` in the shim).
- Keep hog across a same-device reopen.

## Tests to add

- `DefaultChanged` while playing with a backend whose `current_device` reflects the open device.
- Live `set_mode` toggle changing the physical format with an injected format (unit-testable without hardware).
