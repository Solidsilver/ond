# Tickets

From the 2026-10-25 code review (clippy clean; 414 tests passing at commit 669bb00, which all line references use). One file per ticket so agents can claim them independently: set `Claimed by:` and `Status:` in the ticket's header line, and close by deleting the file or setting `Status: done` with the PR/commit.

Each ticket has: Problem (with `path:line` references; "Verified" means the lead reviewer re-checked it against source), Fix, Tests to add, Acceptance.

## Status (2026-10-26)

All 21 tickets are done and merged into `dev`; each ticket's Resolution section says what changed. Tests went from 414 to 530; clippy is clean.

Deliberately left, recorded in the Resolutions:
- Resolve `Tokens` once per frame (012 #14, 016): `full_ui` swaps in the page tint mid-frame, so it means changing every view's signature.
- Two readers blocked far apart on one unfinished stream-cache download switch between them about every 8 KB (006). Nothing in the app does this today.
- CoreAudio rate and format switches still block the engine thread for up to 2 s during `open` (004 #5).
- `Prefs::edit` still clones `Settings` for its no-change check (020); its callers are user actions, not per-frame.
- Hardware and real-server checks added to `TESTING.md` by tickets 004, 014, 015 and 018.

| # | Title | Area | Kind | Priority |
|---|---|---|---|---|
| [001](001-offline-store-data-safety-and-lifecycle.md) | Offline store: pins-file failure deletes downloads; store never shuts down | offline | bug | P0 |
| [002](002-audio-track-boundary-and-error-handling.md) | Audio: wrong track stopped on error; SetNext lost at the boundary | audio | bug | P0 |
| [003](003-audio-seek-bookkeeping.md) | Audio: seek request race and stale held seek | audio | bug | P1 |
| [004](004-audio-device-handling.md) | Audio devices: default change ignored while playing; mode/hog/listener edges | audio | bug | P1 |
| [005](005-audio-decoder-lifecycle.md) | Audio: cancelled decoder still writes shared Status | audio | bug? | P2 |
| [006](006-streamcache-cancel-and-range-edges.md) | streamcache: cancel_all leaves dead downloads attachable; range edges | streamcache | bug | P1 |
| [007](007-app-library-analysis-lifecycle.md) | App: library analysis silently cancelled at startup; enqueue cost | app | bug+perf | P1 |
| [008](008-app-seek-while-stopped-and-error-parking.md) | App: seek while stopped → pending seek repaint storm; errors park queue | app | bug | P1 |
| [009](009-app-session-and-sync-lifecycle.md) | App: session/sync lifecycle gaps (reconnect, replaced sync, sign-out) | app | bug | P1 |
| [010](010-scrobble-restart-race-and-fsync.md) | Scrobble: double scrobble after device switch; saves without fsync | app | bug? | P2 |
| [011](011-ui-interaction-bugs.md) | UI: Songs view pins selection; dialog focus after Esc; view bugs | ui | bug | P1 |
| [012](012-ui-per-frame-cloning.md) | UI perf: per-frame cloning in views (ViewCtx root cause) | ui | perf | P1 |
| [013](013-ui-virtualise-track-lists.md) | UI perf: virtualise album/playlist/favorites track lists | ui | perf | P2 |
| [014](014-ui-thread-stalls-and-blocking-io.md) | UI-thread stalls and blocking file I/O on the tokio runtime | app/offline/streamcache | perf | P2 |
| [015](015-workspace-build-profiles.md) | Workspace: release profile and dev dependency opt-level | build | build | P2 |
| [016](016-refactor-app-rs-extraction.md) | Refactor: extract app.rs into modules; nest Action; Cmd as translation | app | refactor | P2 |
| [017](017-refactor-ui-duplication.md) | Refactor: TrackRow widget, one elide, named offsets, dead code | ui | refactor | P3 |
| [018](018-audio-architecture-cleanups.md) | Audio: gapless across bit depths; dither single source; API tidy | audio | refactor | P3 |
| [019](019-data-layer-robustness.md) | Data layer: lenient decoding, folding, resume validation, DR fidelity | data crates | enhancement | P3 |
| [020](020-app-core-allocation-hotspots.md) | App core: allocation hot spots outside draw code | app | perf | P3 |
| [021](021-offline-surface-set-aside-state.md) | Offline: tell the user when the state file was set aside and cleanup is off | offline/ui | enhancement | P3 |

Suggested order: 001, 002 → 003, 004, 006, 007, 008, 009, 011 → 012 → 005, 010, 013, 014, 015 → 016 → 017, 018, 019, 020. Ticket 016 depends on 007/008/009 landing first; 017 after 012; 018 after 002–005.

Done well (don't "fix"): the lock-free audio callback and hold/flush handshake; session scoping in `Bridge`; `Transport`/`Sources`/`Note` decoupling in `player.rs`; versioned postcard models with mechanical migrations; repaint discipline (idle = 0 frames, asserted in `perf.rs`); Notify/Condvar patterns in offline and streamcache; credentials kept out of logs with a test.
