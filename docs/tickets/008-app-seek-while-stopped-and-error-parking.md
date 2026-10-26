# 008 · App: seek while stopped leaves a pending seek (repaint storm); errors park the queue

- Area: `crates/app` (player.rs) · Kind: bug · Priority: P1
- Status: open · Claimed by: —
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
