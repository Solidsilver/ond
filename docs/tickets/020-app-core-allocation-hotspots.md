# 020 · App core: allocation hot spots outside the draw code

- Area: `crates/app` (app.rs, prefs.rs, tracks.rs, art.rs) · Kind: perf · Priority: P3
- Status: done (branch refactor/app) · Claimed by: —
- Source: app-core review (D3, D4, D5, D7, D8). Line numbers at commit 669bb00.

## Items

- `after_messages` clones `data.albums` and `data.artists` for every index rebuild (`app.rs:1246`); make `Data.albums` an `Arc<Vec<Album>>` like `tracks.all`.
- `Prefs::edit` clones the entire `Settings` for the no-change check (`prefs.rs:98-108`); `OndApp::ui` calls `set_mode(Some(Full))` every frame on the login screen (`app.rs:2342`) → `edit_windows` → clone per frame. Check `window.mode == Full` first; compare only when the closure reports a change (or hash).
- `TrackSet::index: HashMap<String, (String, usize)>` (`tracks.rs:44,101-106`) stores an owned `album_id` per track: 20k extra Strings per rebuild and per drop (drop happens on the UI thread in `set_tracks`). Index by album slot (`u32`) into a `Vec<String>`.
- `Art::poll` uploads every decoded cover in one frame (`art.rs:201-214`); cap per frame (shared with ticket 012 item 15).
- `Art::set_client` (`art.rs:193-198`) retains `looks` but not `looks_lru`; the LRU count includes ghosts.

## Resolution

- **`Data.albums`** is an `Arc<Vec<Album>>`, and so is `Data.artists`. `after_messages` hands the search build `Arc` clones instead of copying both lists. `update_album` goes through `Arc::make_mut`, so it copies the list only while a search build still holds it (`an_album_update_leaves_a_list_handed_out_as_it_was`, `app/data.rs`). The perf harness's "sync messages + rebuild on the UI thread" (2000 albums, 25,000 tracks) went from a median of 1.09 ms to 0.55 ms, with the next item.
- **`TrackSet::index`** maps a track id to `(album slot: u32, place)`. Album ids live once each in `album_ids` (slot → id) and `slots` (id → slot), so a rebuild builds, and the UI thread drops, one string per track (its id) instead of two. Slots are never reused, so a stale index entry can't point at another album. `index` is private now (nothing outside used it). Test: `a_track_moved_to_another_album_is_found_there` (`tracks.rs`).
- **`Prefs::edit` / the login screen**: the login screen asks for the full window only when it isn't in it (`prefs.get` of the mode, no copy), so it no longer clones `Settings` every frame. `Prefs::edit` itself still clones for its no-change check. Every remaining caller is a user action or a drag; making the closure report whether it changed something would move that judgement to two dozen call sites, where it's easy to get wrong.
- **`Art::poll` upload cap**: already done in ticket 012 #15 (`UPLOADS_PER_FRAME`, 8 a frame, with a repaint for the rest). Nothing changed.
- **`Art::set_client`** drops the forgotten looks from `looks_lru` too (new `Lru::retain`, which also takes their weight off the total). Test: `lru_retain_drops_entries_and_their_weight` (`art.rs`).
