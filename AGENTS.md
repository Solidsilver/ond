# Working on Ond

Ond is a native music player for Navidrome and other OpenSubsonic servers: Rust, egui with wgpu, a Cargo workspace of eight crates, one of them the start of Rede, an audiobook player for Audiobookshelf. This file is for coding agents and anyone else who wants the conventions without reading the whole tree. [CONTRIBUTING.md](CONTRIBUTING.md) has the human-facing process; [README.md](README.md) has the build and install instructions.

## Checks that must pass

```sh
cargo fmt --all                                            # before every commit
cargo clippy --workspace --all-targets -- -D warnings      # CI fails on any warning
cargo test --workspace                                     # hardware and PNG tests are #[ignore]d
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items
```

Run a crate alone with `cargo test -p audio` (package names: `audio`, `analysis`, `library`, `offline`, `streamcache`, `subsonic`, `abs`, and `ond-player` for the app). The UI renders every screen to PNG without a server: `cargo test -p ond-player --release snapshots -- --ignored --nocapture` writes `target/snapshots/*.png`. Use `--release` whenever audio plays; debug builds drop out.

Things that need real hardware or a real server are listed in [TESTING.md](TESTING.md). If you change something there, say so in the pull request so a human can run it.

## Where things are

| Crate | Job |
|---|---|
| `apps/ond` (package `ond-player`, binary `Ond`) | UI, queue, sync, settings, each platform's integration |
| `crates/subsonic` | OpenSubsonic API client |
| `crates/library` | Local mirror in redb with hand-written indexes and migrations |
| `crates/streamcache` | Download cache playback can read and seek while it fills |
| `crates/offline` | Pinned downloads |
| `crates/audio` | Decoding, the playback engine, resampling, dither, CoreAudio, ALSA and cpal output |
| `crates/analysis` | Loudness, dynamic range and fake hi-res detection |
| `crates/abs` | Rede: Audiobookshelf API client (tokens, single-flight refresh, play sessions, progress). Its live tests need a server; see its `tests/live.rs` |

Inside the app crate, `src/app.rs` is the root: the `Action` enum, `handle` for messages, `apply` for actions, and the frame. Its module header lists which submodule owns what. Views live in `src/app/<view>.rs`; shared widgets in `src/widgets.rs`; background work in `src/bridge.rs`; the audio engine is driven from `src/player.rs`.

## How the app is wired

- **Views don't change state.** A view gets a `ViewCtx` and pushes `Action`s; `apply` runs them after the frame. Views written as `OndApp` methods (the toolbar, the sidebar, Settings) are the older style. Two known exceptions change things while drawing: `output/panel.rs` edits prefs and sends engine commands, and `visualizer_view.rs` saves the visual it switches to. Don't add more.
- **Background work posts `Msg`s.** Every `Bridge` method returns at once; results come back as a `Msg` on the inbox, each followed by a repaint request. Messages are grouped by the part of the app that handles them (`Msg::Library(..)`, `Msg::Offline(..)`, ...), so a variant in the wrong group is a compile error. Errors cross as a `Failure`: `Network` ones make the app check on the server and stay quiet while it's known to be offline; `Server` and `Local` ones are always shown. "Not connected" for something the user asked for is `Local`.
- **Sessions scope results.** `Bridge::scoped()` ties work to the current sign-in; a result from before a sign-out, a Work Offline toggle or an account switch is dropped by `send()`. New background work that can outlive a session must be scoped.
- **Prefs is the one copy of settings**, saved in the background. Frames read it through `PrefsView`, one copy per frame; a view shouldn't lock `Prefs` while drawing (the Settings page still does, and is the known holdout).
- **Idle is idle.** Repaints are requested only while something is moving; `snapshots/perf.rs` asserts zero idle frames. An animation tied to playback goes through `visuals::Live`. One that runs while paused (toasts, window easing, a spinner) asks for its own repaints and must stop on its own: if it waits on something that may never arrive, cap it, as the sidebar's spinners do.
- **Derived data is memoised on versioned keys** (`memo.rs`, `TrackSet`, `search.generation`, the album and analysis versions). Never key a memo on an `Arc`'s address without holding that `Arc` in the memo's value.

### Recipes

- **A server call:** a method on `Bridge` (scoped), a `Msg` variant in the right group, a handler in the matching `src/app/*.rs`. Errors cross the bridge typed as network, server or local; only network failures trigger a reachability probe.
- **A preference:** a field on `Settings` (with `#[serde(default)]` and a lenient fallback if it's an enum), a `PrefsView` field, a `PrefsAction` variant and its `apply_prefs` arm, a `locked()` arm in `prefs_view.rs` if Home Manager may manage it, a row in `settings_page.rs`, and a line in `docs/home-manager.md`.
- **A view:** `src/app/<view>.rs` taking `ViewCtx`, a `Route`, an entry in `every_view_draws` in `src/app/tests.rs`, a route in `every_view_names_its_controls` in `src/app/ui_tests.rs` (every painted control needs a `widget_info`), and a scene in `snapshots/scenes.rs`. Reuse what's there: `virtual_list` for long lists, `track_list` for tracks, `browse::heading_bar` for a page heading with its picker and sort, `widgets::empty_state` for an empty list.
- **Test data:** `test_tracks::track(id, album, title)` builds a `Track` for any test; `app/fixture.rs` has the UI's fixture library and harness helpers.
- **A queue or playback behaviour:** `queue.rs` is pure data with its own tests; `player.rs` drives the engine and is tested with fakes. Keep them that way.

## Invariants the compiler can't see

These are the things a careful reader would still get wrong. Each has a test where one was possible; the rest are here.

- **Postcard is not self-describing.** `library` and `analysis` store postcard. A stored struct's shape is a wire format: adding, removing or reordering a field or an enum variant changes what every existing database means. Frozen versions (`TrackV1`, `AlbumV1`, ...) freeze every struct they contain. To change a stored type: add a new version, add a migration arm, bump the schema version, and update the golden-bytes test. Saved blobs and analysis records carry a layout version in front; a mismatch reads as missing, never as wrong data.
- **The audio callback is lock-free and allocation-free.** `RingInput::fill_with` in `audio/src/output.rs` and everything it calls may use the ring buffer, atomics and arithmetic only. No `Mutex`, no `Vec` growth, no `tracing`, no channels. The `Status` doc says which thread writes each field.
- **Streamcache lock order is index, then download state.** `Abort` runs under the state lock and must not call back into the cache. `holders` changes only under the index lock.
- **Offline never deletes what it can't prove is stray.** File names come from `escape_id`, which never emits `.`; the stem is the identity. The state file is written atomically; a corrupt one is set aside and stray deletion turns off.
- **`OndApp._rt` stays the first field.** The tokio runtime drops before everything else; nothing declared after it may need the runtime in its `Drop`.
- **Managed settings** (`managed.json` from Home Manager) lock their controls. A new managed key needs `ALLOWED` in `managed.rs`, a `locked()` arm, and the row's lock UI.

## Style

- Comments are prose and say why, not what. Module headers explain the design; the code explains itself. Don't add comments that restate a line, and don't strip the ones that carry a reason.
- Test names are sentences in snake_case: `a_seek_storm_settles_on_the_last_target_and_plays_on`. Tests check behaviour through the public surface (real sockets, fake backends, golden bytes), not private state.
- Errors are typed at crate boundaries (`thiserror` enums) and become user-facing strings only where they're shown. `anyhow` is for startup.
- Platform code goes behind `cfg(target_os)` in the module that owns the platform (`now_playing/macos.rs`, `audio/src/alsa/`), not scattered through callers.
- `rustfmt.toml` sets 160 columns; let `cargo fmt` lay things out.
- The user-facing voice is plain and short (toasts, settings labels, errors). Match what's there.

## Git

- `dev` is where work happens; `main` holds the latest release. Branch from `dev` (`fix/...`, `feat/...`) and merge back into `dev`.
- Commit subjects are at most 72 characters; put the why in the body after a blank line. Commit in logical units; a module move is its own commit so blame survives.
- No AI attribution in commits, pull requests or release notes. AI use is disclosed once, in the README.
- Don't commit `docs/tickets/`, `target/`, `result` or editor files; `.gitignore` already covers them.

## Done well; don't "fix"

The lock-free callback and the hold/flush seek handshake; session scoping in `Bridge`; the `Transport`/`Sources`/`Note` split in `player.rs`; versioned postcard models with mechanical migrations; the repaint discipline; the `Notify`/`Condvar` patterns in `offline` and `streamcache`; credentials kept out of logs (there's a test). Changing any of these needs a reason in the pull request.
