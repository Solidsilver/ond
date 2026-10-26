# 009 · App: session/sync lifecycle gaps (reconnect, replaced sync, sign-out leftovers)

- Area: `crates/app` (app.rs, bridge.rs, sync.rs) · Kind: bug · Priority: P1
- Status: open · Claimed by: —
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
