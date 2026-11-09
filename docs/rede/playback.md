# Playing books

The book page's Play button opens an Audiobookshelf session. Resume uses the
session's position unless the shelf has a newer local position. Finishing a
book starts its next play at the beginning. Equal timestamps favour the server.

The title bar is the player, as Ond's toolbar is: the transport at the left,
then a capsule with the cover, the book and the current chapter. Its scrubber
seeks within that chapter; the thin line underneath shows progress through the
whole book. The capsule grows with the window (0.44 of its width, from 180 to
960 points), so a wide window gets a finer scrubber. Clicking the chapter's
name opens the chapters column at the right (⌥⌘U shows and hides it), which
lists the chapters, the bookmarks to come, and the book's details. The speed
menu also holds skip intervals, rewind on resume, the sleep settings and
Stop. Space toggles playback, Left and Right skip, Shift+Left and Shift+Right
change chapters, and `[` and `]` change speed by 0.05. Text fields keep their
normal keyboard behaviour.

Playback settings live under `playback` in Rede's `settings.json`:

| Key | Meaning | Default |
|---|---|---|
| `skip_back` | Book seconds to skip back | `15` |
| `skip_forward` | Book seconds to skip forward | `30` |
| `speed` | Default speed, 0.5–3 in 0.05 steps | `1.0` |
| `rewind` | Book seconds to rewind after more than five minutes paused; 0 disables it | `5` |

## Implementation

`timeline.rs` maps book seconds to a file and its source offset. Chapters are
independent of files. Missing file lengths use the following file's start or
the book's duration. Positions are clamped at both ends.

`player.rs` owns playback policy. Its `Transport`, `Sources` and `Note` surfaces
keep it testable without devices or a server. It issues `PlayFrom`, `Seek` and
`SetNext`, follows the engine's output-side track events, and derives book time
from `Status::source_position_secs()`. Listening time uses monotonic wall time
while the output is playing, excluding pauses and buffering. The `Live` gate
requests frames only for a visible, moving player. Paused and stopped views
stay idle. The existing engine-event forwarder owns a monotonic report clock:
while playing or buffering it posts a scoped `PlaybackTick` every 15 seconds;
state changes also wake accounting. Clock wakeups coalesce with the player's
100 ms repaint deadline. Reporting runs only on clock messages, so delayed UI
processing cannot accidentally skip a report by just missing a second timer's
15-second threshold. Player state remains owned by the UI thread.

`listening.rs` has a journal stage and one serialized server worker. The journal
appends every report before forwarding it, even if a server call is stalled.
The player sends reports every 15 seconds and on pause, seek and stop. Stop,
book switches and exit close the session. A 404 on report opens another session
and reports the existing position and listening delta without restarting audio.

Failed reports remain pending. Replay runs at startup, before a newer live
report, and every 30 seconds while the account remains connected. It collapses
pending entries to the newest timestamp per item and sends one `set_progress`
batch, marking all covered entries synced only after success. Server progress
timestamps protect newer listening on
another device from old replay entries. Replayed entries retain listening time
locally; counting those intervals in ABS's listening statistics through local
sessions belongs to M4. Only completion sends `isFinished: true`; no path sends
`isFinished: false`.

File ids use `ItemKey`'s account-qualified string form. Rede supplies the default
`ReplyCheck`: ABS uses real HTTP errors, which streamcache rejects before it
accepts a body. A decoder fetch error preserves `http 401` without the URL.
`Source::refresh_tracks` refreshes credentials only when the rejected URLs still
carry the current token, then rebuilds all URLs through `Client::file_url`.
Playback retries at its saved book position; a second rejection before a
successful start stops it. No audio-engine changes were needed.

The player uses the existing shell buttons, icons, theme and focus ring. It
follows Ond's toolbar without sharing its code, so Ond's rendering code is
unchanged. The cache retains
streamcache's existing file-based eviction; whole-book eviction and pinned
downloads remain later work.

## Hidden windows

The pinned eframe 0.37 separates `App::logic` from `App::ui`. Its
[`App` contract](https://github.com/emilk/egui/blob/0.37.0/crates/eframe/src/epi.rs)
explicitly runs logic on repaint requests while hidden. The
[wgpu integration](https://github.com/emilk/egui/blob/0.37.0/crates/eframe/src/native/wgpu_integration.rs)
uses `update_logic_only` for minimized or occluded root windows. The
[event loop](https://github.com/emilk/egui/blob/0.37.0/crates/eframe/src/native/run.rs)
schedules both immediate and delayed repaint requests, directly services
invisible/minimized windows (throttled to 100 ms), and falls back to logic-only
after 250 ms if a requested redraw never arrives. In
[winit 0.30.13 on macOS](https://github.com/rust-windowing/winit/blob/v0.30.13/src/platform_impl/macos/window_delegate.rs),
AppKit occlusion notifications become `WindowEvent::Occluded` and redraw
requests are queued through the application delegate.

Rede already drained its inbox and ticked playback in `App::logic`, just as
Ond drains forwarded engine events there. Thus hiding the window did not, by
itself, stop playback logic in this pinned version. The independent clock makes
the reporting dependency explicit and lets hidden playback stop requesting
animation frames. End-of-file and gapless track events still go through the
same scoped inbox: they start/prefetch the next file, or journal a finished
report and close the final session, without a painted frame. Fake-clock tests
exercise reporting, both file-transition paths and completion through
`Context::run_logic` with zero UI passes.

## Live check

With `ABS_TEST_URL`, `ABS_TEST_USER` and `ABS_TEST_PASSWORD` supplied privately:

```sh
cargo test -p rede --release a_real_book_plays_twenty_seconds_and_leaves_no_progress -- --ignored --nocapture
```

This is one opt-in test, run at most twice for a verification pass. It requires
an account with no progress, signs in once, uses the real engine and system
output for 20 seconds, syncs and closes, then deletes the progress and verifies
that the account is empty again. It never prints the address or credentials.

Manually play a book for several minutes with the window minimized, then fully
covered, and confirm server progress continues moving. Include a file boundary
and the end of a book; check that playback advances and completion is recorded.
Pause while hidden and confirm the process settles without periodic repaints.
Repeat with a slow stream and confirm buffering is excluded from listening time.
