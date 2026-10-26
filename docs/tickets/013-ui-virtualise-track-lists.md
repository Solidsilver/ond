# 013 · UI perf: virtualise album/playlist/favorites track lists

- Area: `crates/app/src/app/album_page.rs` · Kind: perf · Priority: P2
- Status: open · Claimed by: —
- Source: UI review (C9). Line numbers at commit 669bb00.

## Problem

`album_page.rs:496-504` does `allocate_exact_size(.., Sense::click_and_drag())` for every track, registering a widget rect per row even off screen (drawing is skipped via `is_rect_visible`, `:513`). Fine for albums; a 10k-song playlist or Favorites pays ~10k widget registrations per frame. `perf.rs` covers 2,000 rows inside the 4 ms budget; past that it won't hold.

## Fix

Use `show_rows` for `ListOpts{numbers:false}` lists (fixed row heights 38/44), keeping `reveal`/`scroll_to_rect` by computing a row's y from its index. Keep the drag-reorder arithmetic (`album_page.rs:539-543`), which is correct today.

## Acceptance

A 20k-row playlist scene in `perf.rs` stays under the 4 ms mean; drag-reorder and reveal tests still pass.
