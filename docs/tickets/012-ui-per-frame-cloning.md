# 012 · UI perf: per-frame cloning in views (one root cause in ViewCtx)

- Area: `crates/app/src/app/*`, `art.rs`, `view_ctx.rs` · Kind: perf · Priority: P1
- Status: open · Claimed by: —
- Source: UI review (C1–C8, C10–C15; A3). Line numbers at commit 669bb00.

## Context

Repaint *scheduling* is correct and tested (idle = 0 frames; `perf.rs:228-243`). This ticket is about the cost of each frame. Root cause for most items: `ViewCtx` holds `data: &'a Data`, `player: &'a Player`, `lyrics: &'a HashMap` but methods clone out of them because `&mut self` is needed later. Copying the reference out first (`let data = self.data;`) ends the borrow conflict. `perf.rs` (release, `#[ignore]`) asserts mean < 4 ms per scene; these show up in p95 today and in the mean with long queues/large albums.

## Items, ranked

1. Queue column clones every drawn `Item`/`Track` per row, up to `MAX_ROWS=300` (`queue_panel.rs:169`); History clones up to 300 Items into a `Vec` per frame (`:184`). Plus `start_offsets`/`clock_label` per row (`:135,:170`) and `groups()` (`queue.rs:232`) each frame; cache by (queue version, remaining-second).
2. Album page clones the `Album` (`album_page.rs:128`), every `TrackAnalysis` (`:137`), runs `analysis::summarize` (`:138`) and `runner.pending(..)` (`:139`) each frame, `credits(tracks)` (`:301`) when details are open, footer `format!` chain (`:265-271`). Borrow and memoize on `(album id, analysis version, tracks_version)` with the existing `Memo`.
3. Grid tiles allocate ~8 Strings per visible tile per frame (`browse.rs:239-243,246,282,284-285`); `Art::paint` → `Art::texture` (`art.rs:263-272`) allocates a `Key{cover: to_owned}` per paint and iterates `SIZES` on a miss. Key on `(&str, u32)`; build the a11y label inside the lazy `widget_info` closure.
4. Now-playing display clones the playing `Item` and `Album` per frame (`toolbar.rs:131,137,138,156-161,217`); same in `now_playing_view.rs:94,122-123,626`, `info_panel.rs:21,40`, `visualizer_view.rs:168-170` (up to 120 Hz), `compact.rs:40`.
5. Lyrics pane clones the whole `Lyrics` (`now_playing_view.rs:101`), rebuilds rows (`:343` → `lyrics.rs:250-263`), `lyric_galley` `to_owned` + `format!` per line (`:376,:382`). Cache rows per track id.
6. Sidebar clones the playlist list per frame (`sidebar.rs:78`, `:148`), `Route::Playlist` per row (`:79`), `sidebar_active()` clones a `Route` (`:303-309`), `server_state()`/`tooltip()` Strings (`:187-215,:423`). Make `owns(session, playlist)` a free fn.
7. Info tab rebuilds all text rows per frame (`info_panel.rs:58,79-85,91,103,179`). Memoize on track id + analysis version.
8. Songs rows: 5–6 Strings per visible row per frame (`songs.rs:176,178,180,182,191`); build `Source` lazily inside the `context_menu` closure.
9. `browse.rs:324` `id_salt(("grid", format!("{route:?}")))` formats a `Route` per frame; derive `Hash` on `Route` (`nav.rs:4`).
10. Palette: `Results` clone and `Vec<Hit>` groups per frame (`palette.rs:138,146-151`); `cover_art.clone()` per hit (`:204,:214`).
11. `listen()` clones the whole `Frame` per caller (`view_ctx.rs:69`; `Feel.wave` is 512 floats). Return `&Frame` or `Arc` the wave.
12. `csd::layout()` clones `Vec<Button>`s and re-reads `XDG_CURRENT_DESKTOP` 2–3× per frame on Linux (`csd.rs:141-161`). Cache in a `OnceLock`.
13. Shortcuts sheet recomputes `rows()`/`balance()` 3× per frame while open (`dialogs.rs:179,201-206,233-245`).
14. `Tokens::of(ui)` is a ctx data read-lock per widget; resolve once in `full_ui` and pass `&Tokens`.
15. `Art::poll` uploads every decoded cover in one frame (`art.rs:201-214`); cap uploads per frame. Per-frame Strings in `art_tint`/`Art::look` (`app.rs:2057`, `art.rs:234,265,271`).
16. GPU: `bytes()`/`prepare` allocate two small Vecs per frame (`gpu.rs:291-293,482,490`); `bytemuck`-cast slices instead. Everything else in `CallbackResources` is correctly cached.

## Acceptance

`cargo test -p ond-player --release perf -- --ignored` shows the 500-item queue and album-page scenes with p95 within ~1.5× of mean; no `Track`/`Album`/`Lyrics` clones in draw code (grep).
