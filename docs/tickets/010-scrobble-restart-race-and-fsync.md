# 010 · Scrobble: possible double scrobble after a device switch; atomic saves without fsync

- Area: `crates/app` (now_playing/scrobble.rs, now_playing/mod.rs, prefs.rs) · Kind: bug (plausible) · Priority: P2
- Status: open · Claimed by: —
- Source: app-core review (B1, B7). Line numbers at commit 669bb00.

## Problem

1. `Worker::poll` (`crates/app/src/now_playing/mod.rs:321-324`) drops `listen` whenever it observes `PlayState::Stopped`. The engine's `device_switched` (`crates/audio/src/lib.rs:668-688`) calls `stop()` (state Stopped), enumerates devices, then `resume` → `start` (Loading). If the 500 ms poller lands in that window the listen is lost; the following `TrackStarted{restart:true}` fails the `restart && listen.track_id == id` check (`mod.rs:280`) and creates a fresh `Listen` with `submitted = false`, so a track that already scrobbled scrobbles again after another half. Also applies to `SetNext`-triggered restarts (`audio/src/lib.rs:625-633`). Needs a repro: make the test rig call `poll()` with a `Status` previewed as Stopped between `started(false)` and `started(true)`.
2. `prefs.rs:184-197` and `scrobble.rs:162-173` write tmp then rename with no `sync_all`; on power loss ext4/APFS can leave an empty file (prefs are then set aside and the user is logged out).

## Fix

- On `Stopped`, keep `listen` for a grace period, or key the restart check on the last track id rather than on `listen`.
- `File::sync_all()` before the rename in both writers.

## Tests to add

Stopped-window race; `submit` ordering behind a non-empty retry queue; `Unreachable` → `next_flush`; `Known` eviction keeping the current track.
