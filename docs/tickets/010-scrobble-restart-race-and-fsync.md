# 010 · Scrobble: possible double scrobble after a device switch; atomic saves without fsync

- Area: `crates/app` (now_playing/scrobble.rs, now_playing/mod.rs, prefs.rs) · Kind: bug (plausible) · Priority: P2
- Status: done (branch fix/scrobble) · Claimed by: —
- Source: app-core review (B1, B7). Line numbers at commit 669bb00.

## Problem

1. `Worker::poll` (`crates/app/src/now_playing/mod.rs:321-324`) drops `listen` whenever it observes `PlayState::Stopped`. The engine's `device_switched` (`crates/audio/src/lib.rs:668-688`) calls `stop()` (state Stopped), enumerates devices, then `resume` → `start` (Loading). If the 500 ms poller lands in that window the listen is lost; the following `TrackStarted{restart:true}` fails the `restart && listen.track_id == id` check (`mod.rs:280`) and creates a fresh `Listen` with `submitted = false`, so a track that already scrobbled scrobbles again after another half. Also applies to `SetNext`-triggered restarts (`audio/src/lib.rs:625-633`). Needs a repro: make the test rig call `poll()` with a `Status` previewed as Stopped between `started(false)` and `started(true)`.
2. `prefs.rs:184-197` and `scrobble.rs:162-173` write tmp then rename with no `sync_all`; on power loss ext4/APFS can leave an empty file (prefs are then set aside and the user is logged out).

## Fix

- On `Stopped`, keep `listen` for a grace period, or key the restart check on the last track id rather than on `listen`.
- `File::sync_all()` before the rename in both writers.

## Tests to add

Stopped-window race; `submit` ordering behind a non-empty retry queue; `Unreachable` → `next_flush`; `Known` eviction keeping the current track.

## Resolution

1. The double scrobble was real. `a_stop_seen_during_an_engine_restart_does_not_scrobble_twice` reproduced it before the fix: four minutes of a 10-minute track, a poll that sees Stopped, `TrackStarted { restart: true }`, then four more minutes left two plays in the retry queue. Every engine restart passes through Stopped, not only `device_switched`: `start()` calls `stop()` first, so `restart_at` (seek misses) and the `SetNext` restart have the window too. The poller also cleared `current` there, and the restart path returned early without restoring it, so the system's Now Playing stayed empty until the next track.

   Fix, all in `now_playing/mod.rs`: `poll` still clears `current` on Stopped but keeps `listen`. A restart only ever follows a session that was playing that track, and any real new start (`restart: false`) replaces the listen, so a kept listen can only be picked up by a restart of the same play. A real stop followed by the same track again still counts again (`a_real_stop_then_the_same_track_again_is_a_new_listen`). The restart path now restores `current` from `known` when it was cleared. The `Known` eviction keeps the listen's track as well as the current one, so the description is still there when the restart wants it.
2. `prefs::replace_file` writes the temporary file, `sync_all`s it, renames it over the target and, on Unix, syncs the parent directory (best effort). The settings writer and `RetryQueue::save` both use it. Both still run off the UI thread (the settings writer thread and the now-playing worker); the one exception is the settings flush on quit, as before.
3. The worker tests use a `Rig` that drives `poll` from a `Status` set with `preview` and moves the worker's clock by hand, so nothing sleeps for timing. Added: the Stopped-window race; a real stop then the same track; a new play queued behind a retry in flight and sent after it; a play sent directly when nothing waits, then queued with `next_flush` pushed back when unreachable; `Unreachable` from a retry keeps the queue in order and sets `next_flush`, and `Refused` drops it and carries on; `Known` eviction keeps the current track (and the listen's track while a restart has cleared `current`). Plus `replace_file` replaces contents, leaves no temporary file, and returns an error when the directory is missing.
