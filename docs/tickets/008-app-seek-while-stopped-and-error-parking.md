# 008 · App: seek while stopped leaves a pending seek (repaint storm); errors park the queue

- Area: `crates/app` (player.rs) · Kind: bug · Priority: P1
- Status: done (branch fix/app-core) · Claimed by: app-core agent
- Source: app-core review (A2, B6). Line numbers at commit 669bb00.

## Problem

1. `Player::seek` (`crates/app/src/player.rs:253-259`) only checks `queue.current.is_some()`, then sets `pending_seek` and sends `Command::Seek`. The engine's `seek()` (`crates/audio/src/lib.rs:596-597`) returns at once with no `Seeked`/`Error` when there's no session. `queue.current` is `Some` while Stopped after `start_current` gave up (`player.rs:176-180`, offline with nothing downloaded) and after `Event::Error`. Any seek (bar drag, `Cmd::SeekForward`, MPRIS/SMTC `Remote::Seek`, `previous()` when shown position > 3 s) then leaves `pending_seek = Some(to)` for good: `position_secs()` reports the fake value and `logic()` (`app.rs:2331`) repaints every 33 ms until the next `TrackStarted`/`Error`. Verified.
2. `Event::Error` (`player.rs:149-153`) reports and probes but doesn't advance; a track deleted on the server stops an album until the user presses Next. Design choice; consider auto-skip when `track_id: Some`.

## Fix

- In `Player::seek`, if `cx.engine.state() == Stopped`: either ignore, or start the current item at `to`.
- Clear `pending_seek` on every `TrackStarted` in `handle_engine`.
- Optional: on a track-specific error, advance with a notice; keep stopping on `track_id: None`.

## Tests to add (fake-engine rig in player.rs tests)

Seek while Stopped; `Seeked`/`TrackStarted` clearing `pending_seek`; `QueueEdit::Remove/Move/MoveBy/SetEnd` updating `sent_next`; `JumpTo`/`Replay` resetting `sent_next`; `remote()` mappings.

## Resolution

1. `Player::seek` no longer sets `pending_seek` while the engine is Stopped (the `Seek` command still goes out, so a `Play` sent just before that hasn't reached the engine yet still gets it). Every `TrackStarted` now clears `pending_seek`, not only a gapless advance. Seeking while stopped does nothing; it doesn't start playback.
2. Auto-skip is in, kept narrow. On `Event::Error { track_id: Some(id) }`, when `id` is the current item, that item never reported `TrackStarted`, and something follows it, the player moves on and starts the next track with one toast: "Skipped “title”: message". Everything else stops as before: `track_id: None` (device errors), a track that fails partway through (dropped connection), an error that names a later track, and the last track in the queue. A track started by an auto-skip that fails too stops playback, so a gone server costs at most one extra request rather than a run through the queue (the probe takes over from there). Caveat: until ticket 002 lands, the engine can blame the current track when it's really the next one that fails to open, so this can skip a track that would have played.

Tests (player.rs): `a_seek_while_stopped_holds_no_position`, `a_seek_shows_until_the_engine_confirms_it_or_starts_a_track`, `queue_edits_keep_the_engine_s_next_up_to_date` (Remove/Move/MoveBy/Clear/SetEnd), `jump_and_replay_start_over_and_line_up_afresh`, `remote_commands_map_onto_the_player`, `a_track_that_cannot_be_opened_is_skipped`, `a_second_failure_in_a_row_stops`, `errors_that_are_not_a_track_failing_to_open_stop_as_before`.
