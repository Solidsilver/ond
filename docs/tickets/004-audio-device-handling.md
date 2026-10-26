# 004 · Audio devices: default change ignored while playing; mode/hog/listener edges

- Area: `crates/audio` (CoreAudio) · Kind: bug · Priority: P1
- Status: done (branch fix/audio-engine) · Claimed by: —
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

## Resolution

1. Fixed. `OutputBackend` gains `target_device()` (the device a new `open` would use; defaults to `current_device()`), and `poll_devices` compares a `DefaultChanged` against it. CoreAudio's `current_device()` keeps reporting the open stream's device (what the UI shows and what `device_caps()` reads `active_bits` from). `FakeBackend` now has an open device like CoreAudio's, so `playback_follows_the_default_device_when_it_changes_mid_track` failed before the fix.
2. Fixed. The render callback reads the device's integer width from an `Arc<AtomicU32>` (0 = float) shared with `Open`; `set_mode(BitPerfect)` stores what `set_integer_format` settled on, and `set_mode(Normal)` re-reads the physical format after releasing hog mode. One relaxed atomic load per callback, nothing else added to the realtime path. Unit test: `the_callback_dithers_to_the_format_in_effect_now` (Render dithers to 16 bits, then passes 24-bit audio untouched once the shared width becomes 24). The wiring from `set_mode` to the atomic needs hardware.
3. Fixed. The HAL gets a pointer to a `Slot` (a `Mutex<Option<callback>>`) from `Box::into_raw`; removal empties the slot under its lock and leaks only the slot (a few bytes per listener; one per rate change plus the two system ones). The shim only makes shared references and reaches the callback through the lock, so the `&mut` aliasing is gone too. Smoke-tested with the ignored `list_devices` test (registers and drops the listeners) on a MacBook Pro.
4. Fixed. The engine calls a new `close_for_reopen()` before reopening for a format change (default: `close()`); CoreAudio stops the unit but keeps hog mode, and `open()` no longer releases it first, so the same device in bit-perfect keeps exclusive access across the reopen. `close()` (stop, device switch, shutdown) still releases it.
5. Skipped. Moving device setup off the engine thread is a structural change (open would become asynchronous to the engine loop); a `Pause` queued behind a rate switch still waits up to the HAL's confirmation. Left for a follow-up.
6. Remembered mode: fixed. A fallback after losing a device uses the mode the user picked for the fallback device, and Normal only when there is none (before: always Normal); the `DeviceLost` doc says so. Test: `a_lost_device_falls_back_in_its_remembered_mode`. UID as name (`lib.rs:520`): not a bug. The name stored in `lost` is never shown; `DeviceReturned` takes the name from the device list.
7. Fixed. `poll_fetch` asks the cache for the current track's progress once and passes it to `network_in_use`.

Needs hardware (added to TESTING.md): following a default-device change mid-track; hog kept across a rate-change reopen; the dither width following a live mode toggle.
