# 017 · Refactor: shared TrackRow widget, one elide helper, named column offsets, dead code

- Area: `crates/app/src/app/*` · Kind: refactor · Priority: P3
- Status: done (branch fix/ui-lists) · Claimed by: fix/ui-lists
- Source: UI review (E). Line numbers at commit 669bb00.

## Items

- The "name + heart + 'Not downloaded'/mark + hover actions" row exists three times (`album_page::track_list`, `songs_page` rows, `queue_row`), each with its own column math. A `TrackRow` widget taking `ListOpts` would collapse them (do after ticket 012 so the borrow pattern is settled).
- Two `elide` helpers: `widgets::elide` (galley-based) and `visualizer_view::elide` (binary search with `format!` per probe, `:285-298`). `info_header`/`title_links`/`lcd` each elide title/artist/album with slightly different rules.
- Long functions: `album_page::album_page` (~150 lines), `track_list` (~150), `toolbar::lcd` (~120), `palette_ui` (~200 in one closure), `visualizer_ui`. Extract header/footer/menus.
- Magic numbers: `now_playing_view.rs` (`BELOW_ART_H=272`, `88.0`), `info_panel.rs:49` (`-12.0`), `compact.rs` (112, 46, 15), `queue_panel.rs` (`rect.right() - 80/22/12`), `album_page.rs` (`rect.right() - 48/70/86`, `150.0`). The row column offsets must agree across three files; name them.
- Dead/odd: duplicated doc on `player_bar` (`now_playing_view.rs:39-44`) and `ensure_lyrics` (`:78-79`); `icons.rs:67-69` comment describes the heart above `star_points`; `queue_panel.rs:88` matches `History | Lyrics | Info` after Lyrics/Info already returned; `dialogs.rs` `Overlay::Settings` arm inside the generic modal is unreachable (handled at `:33-36`); `Column` default is `Artist` while the header's first column is Title.
- Names worth a doc line: `Keep` vs `Pin` vs `offline_item`; `listen()` returns a `Frame` and has the side effect of requesting frames; `OnArt::pick`'s `need` is a contrast ratio.
- `shadowed()` (`visualizer_view.rs:250-254`) casts `alpha as u8` without clamping.
- The seek strip is a `Slider` for accesskit but carries no value/range (`seek.rs:94`).

## Resolution

Nothing drawn moves: the release snapshots (`cargo test -p ond-player --release snapshots -- --ignored`) were rendered before and after and compared pixel by pixel. The only differences are text that reads the clock or the disk ("until 11:11 PM" over the queue, a hovered queue row's start time, "25 GB free" on the Offline page and in Settings › Offline).

- **TrackRow** (`app/track_row.rs`): one row type for the album/playlist/Favorites list, Songs and the queue. It allocates the row and draws what they share: the background, the playing triangle, one line or a title over its artist (cut with the one `elide`), right-aligned text (the length), the download mark, and the screen-reader label. The column offsets are named in `track_row::col` (inset, playing mark, number column, mark/heart/text/actions measured from the right, the queue's cover). Each list still lays out its own columns: Songs is a table, and the queue has covers and start times, so one widget taking `ListOpts` would have been a switchboard. `title_font` and `length` replace three copies each.
- **One elide**: `widgets::elide` and its `elide_job` (for a painter without a `Ui`). The visualizer's caption uses it instead of its binary search with a `format!` per probe. The Info tab's artist/album links use `link_line` (the Player's), which cuts them the same way and shows the whole name on hover. The toolbar display still joins "artist · album" into one line before cutting it, and the Player still lets the album give way first. Those are layout choices, not other elide rules.
- **Long functions split**: `album_page` → `album_header` / `album_header_text` / `album_facts_line` / `album_buttons` (with an `AlbumHeader` of what they draw from). `track_list` → `track_row` / `track_row_drag` / `paint_track_row` / `track_row_aside` / `track_row_actions`. `lcd_display` → `lcd_spectrum` / `lcd_titles` / `lcd_seek`. `palette_ui` → `palette_keys` / `query_field` / `results_list` / `hit_row` / `hints_bar`. `visualizer_ui` → `VisualizerView::follow` / `step` / `draw` and `paint_titles`. Songs rows → `songs_row`.
- **Magic numbers named**: `now_playing_view` (`COMPACT_ART`, `COMPACT_ART_GAP`; `BELOW_ART_H` already was), `info_panel` (`NO_HEADER_PULL`), `compact` (`PANEL_H`, `PANEL_SEEK_Y`, `PANEL_TRANSPORT_Y`), `queue_panel` (the group heading's `GROUP_*`), `album_page` (row heights, `DISC_GAP`, the header's `HEADER_*`, and the row's 48/70/86/150 as `col::*`).
- **Dead/odd**: the doubled docs on `player_bar` and `ensure_lyrics` are merged. The heart line above `star_points` is gone. The queue column matches Up Next / History once, after Lyrics and Info have returned. The generic modal's `Overlay::Settings` arm is empty, with a note that `settings_window` draws it. `Column`'s default stays `Artist` (it starts Songs as each artist's albums, intact), and the reason is now documented.
- **Doc lines**: `Keep` (and how it becomes `offline::Pin`s), `offline_item`, `listen()` (asking for frames is a side effect; the `Frame` copy shares its waveform), `OnArt::pick`'s `need` (a WCAG contrast ratio).
- `shadowed()` takes a galley and clamps `alpha` to 0..=255.
- The seek strip is a slider with a value (seconds), a 0..length range and a spoken value ("2:21 of 6:23"). `seek_line_click_seeks_there` checks it.

Not done: ticket 012 #14 (resolve `Tokens` once in `full_ui`). It changes every view's signature and `app.rs`'s `full_ui`, which ticket 016 restructures. The new code resolves tokens once per list (`List`, `songs_row`, `queue_row` take `&Tokens`).

Edits outside the views: `app.rs` (`mod track_row;` and the `Keep` doc, 3 lines), `icons.rs` (one doc line), `visuals/over_art.rs` (a doc line).
