# 011 · UI: Songs view pins the selection; dialog focus lost after Esc; smaller view bugs

- Area: `crates/app/src/app/*` · Kind: bug · Priority: P1
- Status: done (branch fix/ui) · Claimed by: fix/ui
- Source: UI review (A1, A2, A3, B1–B5). Line numbers at commit 669bb00.

## Problem

1. **Selected Songs row can't be scrolled away.** `crates/app/src/app/songs.rs:168` calls `resp.scroll_to_me(None)` every frame the selected row is laid out. `show_rows` lays out partially visible rows and `scroll_to_me(None)` scrolls whenever the rect is partially clipped, so wheel-scrolling snaps back once the row is half cut. Verified. The palette does it right with a one-shot `follow` (`palette.rs:146-150, 173`).
2. **Playlist name dialog stops auto-focusing after one Esc.** `dialogs.rs:117-121,136-138`: the "focused once" flag under `ui.scope_id().with("focused")` is removed only when the dialog returns `Some(_)`. Closing via `modal.should_close()` (`dialogs.rs:82-84`) never clears it; the Modal id is fixed, so the next dialog never calls `request_focus()`. Verified.
3. `songs.rs:245` clones the full `Track` every frame a row is selected (`list_keys` returns `Some` every frame), not only on a key press.
4. One-frame flash leaving the visualizer with Esc (`visualizer_view.rs:147-150` returns before any panel is drawn; `close_visualizer` doesn't request a repaint).
5. Visualizer picture goes black during live resize: `gpu.rs:437-467` recreates both offscreen textures (cleared) whenever `size != pictures.size`, and allocates two `Rgba16Float` textures + bind groups per resize frame.
6. Elapsed + remaining don't add up: `fmt.rs:7-10` truncates both and `seek.rs:91` floors remaining, so `elapsed + remaining = d − 1` most of each second.
7. `UiState::seek_preview` is one slot shared by the lcd, Player and Mini seek bars (`seek.rs:38-50`); a mode switch mid-drag leaves a stale preview.
8. Sidebar spinner holds 20 fps while a probe hangs: `sidebar.rs:424-426` `request_repaint_after(50ms)` for `Connecting`, which persists until the probe fails (`app.rs:954`).

## Fix

- `scroll_to_selected: bool` in `SongsView`, set in `songs_keys` on ↑/↓ and on `reveal`, consumed once.
- Clear the focus flag whenever `self.ui.overlay` changes (in `Action::Overlay` handling and when `done.is_some()`), or key it on a per-open nonce.
- Early-return before the clone unless `k.enter || k.play_next || k.add`.
- After closing the visualizer, fall through to drawing the normal mode (or request a repaint).
- Sample the old picture into the new one in the feedback pass, or round picture size to a 64 px grid.
- Ceil the remaining side.
- Cap the spinner (e.g. 15 s) or drive it from `logic()` only while a request is in flight.

## Tests to add (kittest, in `app/snapshots.rs` style)

Songs keyboard selection + wheel scroll leaves the selection off screen; dialog refocus after Esc; seek-bar click/drag → `Action::Seek` value; Esc from visualizer draws a panel in the same frame.

## Resolution

1. Songs: `SongsView::scroll_to_selected` is set by ↑/↓ and consumed once; the row is scrolled to by its index (it may be off screen and not laid out). The rows' item spacing is now set before `show_rows`, so the scroll area places rows at the height they're drawn (it was adding the outer spacing per row).
2. Name dialog: the "focused once" flag has a fixed id and is cleared whenever no name dialog is up and when the dialog finishes, however it closed.
3. Songs keys: the selected `Track` is borrowed, and only when ⏎/⌥⏎/⇧⏎ was pressed.
4. Visualizer: Esc is taken before the frame is drawn (`visualizer_escaped`, one line in `app.rs` `main_ui`), so the window draws in the same frame; `close_visualizer` also requests a repaint for closes applied after the frame.
5. Visualizer resize: the feedback pass reads the old picture (sampled by position) into the new pair the frame the size changes, so it no longer goes black. Textures are still made per size change (no grid rounding).
6. Elapsed and remaining are whole seconds that add up to the length the lists show (`seek_times`).
7. A seek preview with no drag in progress (one that ended in another window mode) is dropped.
8. The Connecting spinner stops asking for frames after 15 s (the ping times out at 10 s, so this is a backstop).

Tests: `songs_selection_scrolls_away_with_the_wheel`, `name_dialog_focuses_its_field_after_esc`, `esc_from_the_visualizer_draws_the_window_at_once`, `seek_line_click_seeks_there` (kittest, `app/snapshots.rs`; all four fail on the old code), `elapsed_and_remaining_add_up_to_the_length` (`app/seek.rs`).
