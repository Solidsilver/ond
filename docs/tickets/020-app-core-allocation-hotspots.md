# 020 · App core: allocation hot spots outside the draw code

- Area: `crates/app` (app.rs, prefs.rs, tracks.rs, art.rs) · Kind: perf · Priority: P3
- Status: open · Claimed by: —
- Source: app-core review (D3, D4, D5, D7, D8). Line numbers at commit 669bb00.

## Items

- `after_messages` clones `data.albums` and `data.artists` for every index rebuild (`app.rs:1246`); make `Data.albums` an `Arc<Vec<Album>>` like `tracks.all`.
- `Prefs::edit` clones the entire `Settings` for the no-change check (`prefs.rs:98-108`); `OndApp::ui` calls `set_mode(Some(Full))` every frame on the login screen (`app.rs:2342`) → `edit_windows` → clone per frame. Check `window.mode == Full` first; compare only when the closure reports a change (or hash).
- `TrackSet::index: HashMap<String, (String, usize)>` (`tracks.rs:44,101-106`) stores an owned `album_id` per track: 20k extra Strings per rebuild and per drop (drop happens on the UI thread in `set_tracks`). Index by album slot (`u32`) into a `Vec<String>`.
- `Art::poll` uploads every decoded cover in one frame (`art.rs:201-214`); cap per frame (shared with ticket 012 item 15).
- `Art::set_client` (`art.rs:193-198`) retains `looks` but not `looks_lru`; the LRU count includes ghosts.
