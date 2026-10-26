# 017 · Refactor: shared TrackRow widget, one elide helper, named column offsets, dead code

- Area: `crates/app/src/app/*` · Kind: refactor · Priority: P3
- Status: open · Claimed by: —
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
