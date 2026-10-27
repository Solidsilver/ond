# 016 · Refactor: extract app.rs into subsystem modules; nest Action; make Cmd a pure translation

- Area: `crates/app/src/app.rs` (3150 lines), `app/commands.rs` · Kind: refactor · Priority: P2
- Status: done (branch refactor/app) · Claimed by: — · Depends on: 007, 008, 009 landing first (they touch the same code) or being folded in.
- Source: app-core review (E), UI review (E). Line numbers at commit 669bb00.

## Assessment

`OndApp` already groups state (`Session`, `SyncState`, `Data`, `Downloads`, `Analysis`, `UiState`, `SearchState`) but behaviour stays in `impl OndApp`: `handle()` (~210 lines, 40 arms), `apply()` (~190 lines, 55 arms), connect/sign-out/offline (`Connected` arm + `sign_out` + `set_offline` + `probe` + `forget_library`), pins (`refresh_pin`/`keep_offline`/`wait_to_pin`/`pin_label`), analysis (`enqueue_analysis`/`resume_…`/`finish_…`), window tracking (`Settle`, `follow_plan`, `track_window`, `resize_to`, `set_mode`), 15 one-line `prefs.get` getters, and the login form UI. `Action` is a flat 55-variant enum; `Cmd` is a second vocabulary mapped onto it in `commands.rs:43-100`, except `Cmd::Shortcuts`/`NewPlaylist`/`ToggleBitPerfect` which mutate state directly, and `Action::Command(Cmd)` makes `apply()` re-enter `command()`, hence the bounded 4-pass drain at `app.rs:2022-2029`. `Downloads::apply` and `Analysis::apply` already own 3–5 variants; finish the pattern. Views also bypass `ViewCtx` and touch `self.bridge`/`self.output`/`self.diagnostics` directly (`sidebar.rs:203-205,261`, `settings_page.rs:300,432`, `toolbar.rs:253,316`, `compact.rs:150`), so the doc comment at `view_ctx.rs:1-5` overstates the guarantee.

## Plan (no behaviour change, one PR per step)

1. `app/session.rs`: `Session`, `LoginForm` + `login_ui`, `handle_connected/connect_failed/unreachable`, `sign_out`, `set_offline`, `probe`, `forget_library` (~350 lines). Fold in ticket 009's `begin()` fix.
2. `app/library.rs`: `Data`, `SyncState`, `Pending` + `wait_for`/`run_pending`/`queue_tracks`, and the `Albums/Artists/AllTracks/Tracks/PlaylistTracks/TrackBatch/Credited/Sync*` arms as `Data::handle(msg) -> Vec<Effect>` (~300).
3. `app/pins.rs`: `Downloads` + `refresh_pin`, `keep_offline`, `wait_to_pin`, `pin_label`, `Keep` (~200).
4. `app/analysis.rs`: `Analysis` + `enqueue_analysis`, `resume_library_analysis`, `finish_library_analysis`, `unanalysed` (~120). Home of ticket 007.
5. `app/window_state.rs`: `Settle`, `UNMAXIMIZE_WAIT`, `follow_plan`, `track_window`, `resize_to`, `set_mode`, `edit_windows` (~150).
6. Replace the 15 prefs getters with a per-frame `PrefsView` snapshot (also removes a dozen locks per frame).
7. `impl Cmd { fn action(self, app: &OndApp) -> Option<Action> }` so `command()` only translates; nest `Action` by subsystem (`Action::Queue(QueueEdit)`, `Window(..)`, `Prefs(..)`, `Offline(..)`) so each sub-struct owns its `apply`; drop the 4-pass drain.
8. Dedupe: `AppSources { .. }` built twice (`app.rs:1276-1282`, `1314-1320`); `title_bar_h()` duplicates the match in `main_ui` (`app.rs:1972-1978` vs `2014-2018`); `sign_out` resets `unreachable`/`server_label` by hand instead of via `set_offline`. `Downloads::apply` returns `Result<Option<String>, _>` whose `Option` is always `None`.
9. Doc/dead code: stray "Fisher–Yates" comment above `AppSources` (`app.rs:2366`) and above `QueueEdit` (`player.rs:311`); duplicated `Msg::AllTracks` doc (`bridge.rs:52-54`); `TrackSet` doc says "plus the albums opened since" but `tracks.all` excludes them; `playback.rs` vs `output.rs` prefs split is confusing.

## Acceptance

`app.rs` ≈ 900 lines (OndApp, build, dispatchers, `main_ui`/`full_ui`); all 235 app tests pass unchanged at each step.

## Resolution

`app.rs` went from 3,603 lines (at `dev`, 37c4cc0) to 906: `OndApp`, the `Action` enum, building the app, the dispatch of bridge messages (`handle`) and of actions (`apply`), and the frame (`main_ui`, `full_ui`, the queue sheet, `eframe::App`). One commit per step; clippy and the app tests (269, unchanged in number) passed after each.

Module layout (`wc -l`):

| Module | Lines | What |
|---|---|---|
| `app.rs` | 906 | `OndApp`, `Action`, `build`, `handle`/`apply`, `main_ui`/`full_ui` |
| `app/session.rs` | 273 | `Session`, `LoginForm` + `login_ui`, `connected`/`connect_failed`/`unreachable`, `probe`, `refresh_latency`, `set_offline`, `forget_library`, `sign_out` |
| `app/data.rs` | 401 | `Data`, `SyncState`, `SearchState`, `Pending`; `library_msg` (the library/sync/track-list messages), the search index, `queue_tracks`/`wait_for`/`run_pending`, `play_album`/`play_artist`/`play_playlist` |
| `app/pins.rs` | 473 | `Downloads` (and how it opens at start-up: `opener`, `start`), `Keep`, `OfflineAction` + `apply_offline`, `offline_msg`, `poll_downloads`, `refresh_pin`/`keep_offline`/`wait_to_pin`/`pin_label`, `track_ref` |
| `app/analyzing.rs` | 167 | `Analysis`, `AnalysisAction` + `apply_analysis`, `analysis_msg`, `enqueue_analysis`, the library-wide run, `unanalysed` |
| `app/playing.rs` | 193 | `AppSources`, `with_player` and the player wrappers, engine events, remote commands, `OutputAction` + `apply_output` |
| `app/window_state.rs` | 218 | `Settle`, `WindowState`, `WindowAction` + `apply_window`, `set_mode`/`follow_plan`/`track_window`/`resize_to`, `edit_windows`, `set_columns`, `title_bar_h` |
| `app/prefs_view.rs` | 108 | `PrefsView` (the per-frame copy), `prefs_now`, `PrefsAction` + `apply_prefs` |
| `app/commands.rs` | 254 | `Cmd::action` (keys and menu → `Action`), hearts, ratings, playlist edits |
| `app/ui_state.rs` | 203 | `UiState` and the small UI types (`QueueTab`, `Focus`, `AlbumSort`, `QueueUi`, `DragTracks`, `SongStats`) |
| `app/frame.rs` | 134 | per-frame work around drawing: `art_tint`, `backdrop_layers`, `want_waveform`, `scroll_bars`, `drag_ghost` |
| `app/tests.rs` | 916 | the tests that were at the bottom of `app.rs`, moved as they were |

The ticket's `app/library.rs` and `app/analysis.rs` are `data.rs` and `analyzing.rs`: `library` and `analysis` are crate names, and a module of the same name inside `app` would shadow them for `app.rs` and for every `use super::*` (the tests, the snapshots).

Steps:

1. **session.rs**: done. Ticket 009 had already fixed the reconnect pass in the bridge, so there was no `begin()` change to fold in.
2. **data.rs**: done. The library messages stay one `match` (`library_msg`) rather than `Data::handle(msg) -> Vec<Effect>`: the arms also touch the sync state, waiting queue actions, pins and analysis, so effects would have been a second vocabulary for little gain.
3. **pins.rs**: done, plus the store's start-up open out of `build` and the per-frame snapshot check out of `logic`.
4. **analyzing.rs**: done.
5. **window_state.rs**: done. The six window fields of `UiState` are one `WindowState` (`ui.window`).
6. **PrefsView**: done. Fifteen getters (`window_mode`, `show_queue`, `experimental`, …) are gone. `logic` and `main_ui` copy the preferences once (`read_prefs`) into `ui.prefs`, and drawing reads that. Code that applies actions reads `prefs_now()`, because an action applied just before may have changed them (`Lyrics` after `TogglePlayer`, say). A frame takes the lock twice instead of a dozen-plus times.
7. **Cmd and Action**: done. `Cmd::action(self, &OndApp) -> Option<Action>` only translates. The commands that changed state directly got actions of their own (`ToggleShortcuts`, `NewPlaylist`, `ToggleVisualizer`, `Edit`, `Reveal`, `Output(Volume)`, `Output(ToggleBitPerfect)`). `Action` is nested: `Queue(QueueEdit)`, `Window(WindowAction)`, `Prefs(PrefsAction)`, `Output(OutputAction)`, `Offline(OfflineAction)`, `Analysis(AnalysisAction)`, each applied in its module; `Downloads::apply` and `Analysis::apply` take their own enums. Nothing pushes an action while another applies any more (`toggle_side_tab` and `Lyrics` in the Player apply theirs at once), so the bounded four-pass drain is one pass. The only timing change: a menu-bar command now applies as it arrives in `logic`, before the frame is drawn, instead of after it (keys were and are applied after the frame).
   Views that bypassed `ViewCtx`: the sidebar's latency refresh and "Stop sync" (sidebar and Settings) and the toolbar's volume slider now go through actions (`MeasureLatency`, `StopSync`, `Output(Volume)`). The output picker (`Outputs::picker_ui`, toolbar and Player status bar) and the visualizer's ←/→ switch still change state on the spot; the doc comment on `view_ctx.rs` now says so and lists the views that are still `OndApp` methods.
8. **Dedupes**: `title_bar_h(mode)` is one function, used by `logic`, the CSD overlay and the snapshot harness. The test-only `source()` goes through `with_player`'s sources instead of building `AppSources` a second time. `Downloads::apply` returns `Result<(), String>`. `sign_out` still resets `unreachable` and `server_label` itself, now with a comment: `set_offline` is also the first half of `Unreachable` (which sets them for the outage) and `ConnectFailed` (which leaves them), so moving the reset there would change those.
9. **Docs**: the Fisher–Yates line documents `player::shuffle` again (it was above `QueueEdit` and `AppSources`); `Msg::AllTracks` has one doc; `Data::tracks` and `TrackSet` say that `all` leaves out albums fetched since; `set_offline` has its doc line back (it had slid onto `forget_library`); `playback.rs` and `output.rs` say how their halves of the playback preferences fit together.

Not done: resolving `Tokens` once per frame (ticket 012 #14). `full_ui` still swaps in the page's tint around the central panel, and the views still call `Tokens::of(ui)`; passing `&Tokens` down means changing every view's signature, which none of these steps needed. It's a separate, mechanical change now that the frame lives in `main_ui`/`full_ui` alone.

Tests changed only because an internal path moved, no assertion weakened:
- `app/tests.rs` (moved from `app.rs`): `window_mode()`, `lyrics_drawer()`, `experimental()`, `analysis_library()` → `prefs_now().mode`/`.lyrics_open`/`.experimental`/`.analysis_resume` (step 6); `Action::KeepOffline`, `PauseDownloads`, `SetMode`, `ToggleMini`, `TogglePlayer` → their nested forms (step 7).
- `app/snapshots.rs`: `columns()`, `experimental()`, `window_mode()` → `prefs_now()` (step 6); `SetPlaybackPrefs`/`SetMode`-style actions nested (step 7); the harness calls `title_bar_h(app.prefs_now().mode)` (step 8); one scene edits an album through `Arc::make_mut` (ticket 020).
- `app/snapshots/perf.rs`: `data.albums.to_vec()` for `Msg::Albums` (ticket 020).

Checked with the release snapshots (`cargo test -p ond-player --release snapshots -- --ignored`), rendered before step 1 and after the last step and compared pixel by pixel: of the 294 PNGs (both themes), 112 are identical and 182 differ, all in text that reads the clock or the disk: "until 10:37 AM" over the queue (wherever the layout puts it, including the Player and the narrow queue sheet), a hovered queue row's start time, "N GB free" and the temporary folder's path on the Offline page and in Settings › Offline, plus one pixel one shade apart in two dark scenes (as in ticket 017). The first "after" run also showed Settings › Library's analysis line as "Analyzing 0 of 40 · about 8 min left" instead of "0 of 40 · waiting for playback" in the light theme only: the scene depends on whether the runner's thread has flagged itself waiting when the shot is taken. A second run matched the "before" image apart from the clock.

Perf (`cargo test -p ond-player --release perf -- --ignored`, Apple Silicon, mean ms, before → after): albums grid 0.23 → 0.22, songs mid-scroll 0.24 → 0.23, album page 0.24 → 0.22, recent + 500-item queue 0.21 → 0.21, palette 0.24 → 0.24, settings → library 0.24 → 0.24, offline with 2000 pins 1.28 → 1.32, favorites 0.23 → 0.24, 20,000-song playlist 0.23 → 0.24. Within run-to-run noise. Sync messages plus the rebuild on the UI thread: median 1.09 → 0.55 ms, from ticket 020.
