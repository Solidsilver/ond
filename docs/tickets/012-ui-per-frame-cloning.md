# 012 · UI perf: per-frame cloning in views (one root cause in ViewCtx)

- Area: `crates/app/src/app/*`, `art.rs`, `view_ctx.rs` · Kind: perf · Priority: P1
- Status: done (branch fix/ui) · Claimed by: fix/ui
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

## Resolution

Views copy the `&'a` references out of `ViewCtx` (`let data = self.data;`) and draw from the library, player, lyrics and analysis in place. Where an `OndApp` method needed `&mut self` while holding a borrow, the drawing moved into a `ViewCtx` method (`lcd_display`, `player_main`, `sidebar_playlists`). Derived text is kept between frames in `UiState::memos` (`view_ctx::Memos`, using `crate::memo::Memo`).

1. Queue: rows borrow their `Item`; History walks the queue in place; the "at 10:56 PM" label is formatted only on hover; `Queue::groups()` is now a borrowing iterator (`Group<'a>` holds `&Source`, so no clone per group). `start_offsets` is still one `Vec<f64>` per frame: the queue has no version counter to cache by, and the cost is small.
2. Album page: album, tracks and analyses are borrowed. `AlbumFacts` (summary, shared format, credits, footer, `Source`) is memoized on (album id hash, `data.version`, `analysis.version`, track list address and length). Fetching an album doesn't bump `tracks_version`, so the key uses the list's identity, and the memo holds that list so its address can't be reused while cached. `runner.pending` still runs every frame: it changes on the runner thread, with no version to key on.
3. Grid: tiles borrow the album, find the download pin without building a `Pin`, and build the a11y label lazily. `Art` numbers cover ids (`Key { cover: u32, size }`), so `paint`/`texture` build no `String`, and `look` touches its LRU by `&str`.
4. Now playing: the toolbar display (`lcd_display`), the Player (`player_main`/`player_block`), Mini, the Info tab and the visualizer borrow the playing item and album.
5. Lyrics: the pane borrows the lyrics. Rows are memoized on (track id hash, lines address, count, synced), and line labels are built lazily. `lyric_galley` still copies each line's text into its `LayoutJob`, because egui owns that text.
6. Sidebar: the playlist list is drawn from `&data.playlists` and builds its `Route` only on click. The active place is an index, `owns` is a free function, `ServerState::label` returns `&str`, and tooltips are built only when needed. `server_state()` still builds the host string each frame.
7. Info tab: `InfoFacts` (credits, track place, year, file facts, measurements) is memoized on (track id hash, `data.version`, `analysis.version`). `links_row` takes `(&str, FnOnce() -> Route)` and builds the route only on click. The Playback row is still per frame, because the signal path changes without a version.
8. Songs rows: the album name is borrowed, the a11y label is built lazily, and the row's `Source` is made inside the context menu. The duration and format strings stay, one each per visible row.
9. `Route` and `FilterKind` derive `Hash`; the grid's `id_salt` hashes the route.
10. Palette: results are read in place (no `Results` clone, no `Vec<Hit>` per group), covers and titles are borrowed.
11. `Feel::wave` is an `Arc<Vec<f32>>`, written in place while unshared (`Feel::copy_from` replaces `clone_from`), so `listen()`'s `Frame` clone no longer copies the waveform.
12. `csd::layout()` returns a `&'static Layout` from `OnceLock`s; `XDG_CURRENT_DESKTOP` is read once.
13. Shortcut sheet: rows per group and the 2/3-column splits are computed once (`OnceLock`).
14. Partly done: queue rows and grid tiles take `&Tokens` from their list. Not done: resolving tokens once in `full_ui`. `full_ui` swaps in the page tint around the page mid-frame, so one `Tokens` per frame would be wrong, and passing `&Tokens` through every view changes every signature, which fits better with 016/017.
15. `Art::poll` uploads at most 8 textures per frame and requests a repaint for the rest. `art_tint` (`app.rs`) borrows the cover ids; `cover_tints` is now a closure over `art` and `tint_cache`, and `page_cover` is test-only.
16. GPU: uniforms fill a fixed array and are written from a stack byte array; lights go through a reused staging `Vec`.

Also: the playlist page and its mosaic borrow their tracks and covers.

Perf (`cargo test -p ond-player --release perf -- --ignored`, Apple Silicon; mean / p95 in ms; "before" is a559478, run twice):

| Scene | Before | After |
|---|---|---|
| recent + 500-item queue | 0.30 / 0.32, 0.31 / 0.37 | 0.23 / 0.26 |
| album page | 0.31 / 0.32, 0.32 / 0.33 | 0.24 / 0.25 |
| albums grid | 0.31 / 0.32, 0.32 / 0.33 | 0.24 / 0.28 |
| songs, mid-scroll | 0.32 / 0.33, 0.32 / 0.34 | 0.24 / 0.25 |
| palette open | 0.33 / 0.34, 0.34 / 0.35 | 0.27 / 0.30 |
| favorites, 2000 songs | 0.89 / 0.95, 0.88 / 0.92 | 0.79 / 0.82 |

Every scene draws the 500-item queue column, so item 1 shows up in all of them. p95 is within 1.2× of the mean in each scene.
