# Playing books

The book page's Play button opens an Audiobookshelf session. Resume uses the
session's position unless the shelf has a newer local position. Finishing a
book starts its next play at the beginning. Equal timestamps favour the server.

The bottom bar shows the current chapter. Its large scrubber seeks within that
chapter; the thin line underneath shows progress through the whole book. The
speed menu also holds skip intervals, rewind on resume, chapter navigation and
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
requests frames while source time moves; paused and stopped views stay idle.
Engine events and status changes wake the UI through the scoped bridge.

`listening.rs` has a journal stage and one serialized server worker. The journal
appends every report before forwarding it, even if a server call is stalled.
The server worker reports every 15 seconds and on pause, seek and stop. Stop,
book switches and exit close the session. A 404 on report opens another session
and reports the existing position and listening delta without restarting audio.

Failed reports remain pending. Replay runs at startup, before a newer live
report, and every 30 seconds while the account remains connected. It sends
entries individually through `set_progress`, in log order, marking each synced
only after success. Server progress timestamps protect newer listening on
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

The bar uses the existing shell buttons, icons, theme and focus ring. It needs
no extraction from Ond, so Ond's rendering code is unchanged. The cache retains
streamcache's existing file-based eviction; whole-book eviction and pinned
downloads remain later work.

## Live check

With `ABS_TEST_URL`, `ABS_TEST_USER` and `ABS_TEST_PASSWORD` supplied privately:

```sh
cargo test -p rede --release a_real_book_plays_twenty_seconds_and_leaves_no_progress -- --ignored --nocapture
```

This is one opt-in test, run at most twice for a verification pass. It requires
an account with no progress, signs in once, uses the real engine and system
output for 20 seconds, syncs and closes, then deletes the progress and verifies
that the account is empty again. It never prints the address or credentials.
