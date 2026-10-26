# 016 · Refactor: extract app.rs into subsystem modules; nest Action; make Cmd a pure translation

- Area: `crates/app/src/app.rs` (3150 lines), `app/commands.rs` · Kind: refactor · Priority: P2
- Status: open · Claimed by: — · Depends on: 007, 008, 009 landing first (they touch the same code) or being folded in.
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
