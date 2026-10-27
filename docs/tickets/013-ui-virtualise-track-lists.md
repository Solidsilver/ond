# 013 · UI perf: virtualise album/playlist/favorites track lists

- Area: `crates/app/src/app/album_page.rs` · Kind: perf · Priority: P2
- Status: done (branch fix/ui-lists) · Claimed by: fix/ui-lists
- Source: UI review (C9). Line numbers at commit 669bb00.

## Problem

`album_page.rs:496-504` does `allocate_exact_size(.., Sense::click_and_drag())` for every track, registering a widget rect per row even off screen (drawing is skipped via `is_rect_visible`, `:513`). Fine for albums; a 10k-song playlist or Favorites pays ~10k widget registrations per frame. `perf.rs` covers 2,000 rows inside the 4 ms budget; past that it won't hold.

## Fix

Use `show_rows` for `ListOpts{numbers:false}` lists (fixed row heights 38/44), keeping `reveal`/`scroll_to_rect` by computing a row's y from its index. Keep the drag-reorder arithmetic (`album_page.rs:539-543`), which is correct today.

## Acceptance

A 20k-row playlist scene in `perf.rs` stays under the 4 ms mean; drag-reorder and reveal tests still pass.

## Resolution

Playlists and Favorites › Songs draw their tracks with `track_list_rows`, which hands the list to `ScrollArea::show_rows` (rows 44 pt, or 38 without the artist line). Only the rows on screen are laid out. The album page keeps `track_list`, which lays out every row (albums are short and have disc headings). Both draw each row with the same `track_row`.

- Reveal: the revealed track's index is looked up once, and the row is scrolled to by its y (index × row height) in the frame it's asked for, laid out or not. The track is still selected and the reveal used up.
- Drag-reorder: the arithmetic is unchanged (moved into `track_row_drag`). The drop line is drawn inside the visible range.
- Keys: ↑/↓ now also bring the selection into view, in the long lists (by index) and on the album page (by its rect). Before, the selection could walk off screen.
- Disc headings show only on album pages (`ListOpts::numbers`). They were shown in Favorites and in other people's playlists when a song came from a second disc, and they would break the fixed row height.

Test: `a_long_playlist_reorders_reveals_and_follows_the_keys` (kittest, `app/snapshots.rs`, a 3,000-song playlist). It checks that rows off screen aren't laid out, that dragging the first song onto the lower half of the third moves it there, that revealing song 2000 selects it and scrolls it on screen, and that 30 × ↓ keeps the selection in view.

Perf (`cargo test -p ond-player --release perf -- --ignored`, Apple Silicon, mean / p95 ms). The new scene is a 20,000-song playlist of the user's own, scrolled part-way down with a row hovered:

| Scene | Before (old list code) | After |
|---|---|---|
| playlist, 20000 songs | 9.94 / 12.27 | 0.26 / 0.26 |
| favorites, 2000 songs | 0.81 / 0.98 | 0.26 / 0.29 |
| album page | 0.25 / 0.34 | 0.25 / 0.29 |

The other scenes are unchanged (0.23–0.26 ms means; offline 1.41 ms, which this ticket doesn't touch).
