# 009 · App: session/sync lifecycle gaps (reconnect, replaced sync, sign-out leftovers)

- Area: `crates/app` (app.rs, bridge.rs, sync.rs) · Kind: bug · Priority: P1
- Status: done (branch fix/app-core) · Claimed by: app-core agent
- Source: app-core review (A3, A4, A5, A6, A7, B3, B4, D6). Line numbers at commit 669bb00.

## Problem

1. **`Msg::Connected` starts an album sync without `SyncState::begin()`** (`crates/app/src/app.rs:911-913`; `Action::SyncNow` at `app.rs:1513-1519` does use it). After `Unreachable` → reconnect → `Connected`, the previous album pass may still run (`Unreachable` only resets UI flags, `app.rs:1142-1149`). Two `sync::albums` passes write concurrently; the old one's failure delivers `AlbumSyncDone(Err)` → "Sync failed" toast + `probe()` right after reconnecting, and clears `sync.albums` while the new pass runs.
2. **Stale `TrackSync` progress.** `sync_tracks` (`bridge.rs:464-471`) rate-limits progress but never checks `is_current(&cancel)`; only `TrackSyncDone` does (`bridge.rs:485-487`). `per_album` keeps reporting for in-flight joins after cancel (`sync.rs:335-352`). "Sync now" during a sync → the label flips between totals; `sync.tracks` is re-set by a dead sync.
3. **Lyrics / song stats across sign-out.** `Action::NeedLyrics` (`app.rs:1394-1399`) inserts `Fetch::Loading` and fetches only when absent; `fetch_lyrics` is session-scoped (`bridge.rs:712`) so a sign-out mid-fetch leaves `Loading` forever. Neither `sign_out` (`app.rs:1843-1862`) nor `forget_library` (`app.rs:1132-1140`) clears `self.lyrics`/`self.song_stats`; a different account inherits them keyed by possibly colliding ids.
4. `connected()` (`bridge.rs:433-439`) checks `live(session)`, then awaits a keychain write that can block on a prompt, then `claim_account` without a scope.
5. `fetch_home` (`bridge.rs:655-657`): `let _ = this.db(upsert_albums).await;` then announces ids that may not be stored.
6. `Action::Album` (`app.rs:1421-1427`) plays `data.tracks.album(&id)` whenever present, including albums only partially stored by an interrupted bulk pass (`sync.rs:1-14`); a subset plays silently.
7. Empty library writes `BULK_DONE` forever (`sync.rs:229-234, 289, 305`): a library that later gets 20k tracks syncs one `getAlbum` at a time. Perf only.
8. A replaced sync still reloads albums and all tracks (`bridge.rs:474-483`): an extra 20k-row read + `TrackSet::new` + `Msg::AllTracks` for nothing.
9. `fetch_tracks` silently `continue`s on fetch/db errors (`bridge.rs:741-745`), so an artist shuffle can be partial with no notice.

## Fix

- Route `Connected` through `begin()` (or cancel the album pass on `Unreachable`).
- `if this.is_current(&cancel) { this.send(TrackSync(p)) }`.
- Clear `lyrics` and `song_stats` in `sign_out`/`forget_library`; drop `Loading` entries on `end_session`.
- Re-check `live(session)` after the keychain await, or use a scoped bridge in `connected`.
- Surface the upsert error; skip the `Newest` message on failure.
- In `Action::Album`, compare `slice.len()` to `album.song_count` and fall through to `wait_for` + `open_album` when short.
- Skip the bulk pass (and don't write `BULK_DONE`) when `albums.is_empty()`.
- Skip the reload when the sync was replaced.

## Tests to add

`Connected` during a running sync; stale `TrackSync` after replace; lyrics/stats cleared on sign-out; `Action::Album` with a partial album; empty-library plan; bridge-level `scoped()` dropping messages after `end_session`; `after_previous` ordering.

## Resolution

1. Fixed in the bridge, not with `begin()`. `Unreachable` → `set_offline` already resets `sync.albums`/`sync.tracks`, so `begin()` at `Connected` would start a second pass anyway. Each `sync_albums` now bumps `Bridge::album_pass`, and a pass whose number is no longer current stops storing (its `live` turns false) and sends nothing: no progress, no `AlbumSyncDone(Err)`, no `Artists`, no track sync. `Connected` still always starts a pass (commented), which replaces any left over from before the outage.
2. Fixed: `TrackSync` progress is sent only while `is_current(&cancel)`.
3. Fixed: `sign_out` and `forget_library` clear `lyrics` and `song_stats`. That drops any `Loading` left by a fetch the ended session will never deliver, and stops the next account inheriting entries under colliding ids.
4. Fixed: `connected()` works through a bridge scoped to its own session (new `scoped_to`). It returns early if the session ended during the keychain write, and `claim_account` goes through the scoped `db`.
5. Fixed: `fetch_home` announces `Newest` only after the upsert succeeds; a failure shows "Couldn't save Recently added: …".
6. Fixed: `Action::Album` treats a stored album with fewer tracks than `song_count` as partial when a server is there to ask. It waits (`Pending::fresh`) for `open_album`'s server listing rather than the cached copy that arrives first. If the server fetch fails (`LoadFailed`), it plays what's stored instead of nothing. Offline, the stored part plays straight away, as before.
7. Mostly not reachable. With an empty album list, `plan()` never chose a bulk pass unless an interrupted offset was saved, or 200+ albums each listed 0 songs. Guarded anyway: `plan()` returns `PerAlbum` for an empty album list, and a bulk pass that stored no songs clears the offset without writing `BULK_DONE`.
8. Fixed: a replaced track sync no longer reloads albums and tracks. If it stored something, it sets `Bridge::reload_owed`, and the next sync to finish reloads even if it fetched nothing itself, so what the replaced one stored still shows.
9. Fixed: `fetch_tracks` counts albums it couldn't fetch or store (online only; offline, unstored albums are expected) and shows "Couldn't load the songs of N albums". The batch still plays.

Also: `sync_albums`/`sync_tracks` are generic over `sync::Fetch` (the app still passes a `Client`) so the bridge can be tested with a fake server. `sync::tests::{album, song}` are `pub(crate)` for that.

Tests: bridge.rs `a_replaced_track_sync_reports_nothing_more`, `a_newer_album_sync_replaces_one_still_running` (both fail without the fix), `work_for_a_session_that_ended_sends_nothing` (scoped copies after `end_session`), `requests_for_one_item_run_in_the_order_made` (`after_previous`). They run on a current-thread runtime and wait on conditions, not sleeps. app.rs `signing_out_forgets_lyrics_and_song_stats`, `playing_an_album_stored_only_in_part_waits_for_the_server`. sync.rs `an_empty_library_never_marks_the_bulk_pass_done`. Not tested: the keychain re-check in `connected()` and the `fetch_home` error path, which need a live server or keychain.
