# Playing books

The book page's Play button opens an Audiobookshelf session when reachable,
or a local listening session for a downloaded book while offline. Resume uses
the session's position unless the shelf has a newer local position. Finishing a
book starts its next play at the beginning. Equal timestamps favour the server.

The title bar is the player, as Ond's toolbar is: the transport at the left,
then a capsule with the cover, the book, the current chapter and the book's
time left at the current speed. Its scrubber seeks within that chapter; the
thin line underneath shows progress through the whole book. The capsule grows
with the window (0.47 of its width, from 330 to 1000 points), so a wide window
gets a finer scrubber. Clicking the cover opens the Player; clicking the
chapter's name opens the chapters column at the right (⌥⌘U shows and hides
it), which lists the chapters, bookmarks and the book's details.
At the capsule's right end, two small chips show the speed and the sleep
timer (lit, with what's left, while it runs) and open their menus. The speed
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
| `voice_boost` | Speech compressor, presence boost and limiter | `false` |
| `skip_silence` | Shorten long quiet pauses | `false` |
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
timestamps protect newer listening on another device from old replay entries.
Offline listening is also replayed as timestamped ABS local sessions before
position replay, so it counts towards server listening statistics without
rewinding newer progress. Automatic completion sends `isFinished: true`;
explicit Un-finish and Start over actions can clear it. The offline store and
session replay are described in the [offline design](design.md#milestones).

File ids use `ItemKey`'s account-qualified string form. Rede supplies the default
`ReplyCheck`: ABS uses real HTTP errors, which streamcache rejects before it
accepts a body. A decoder fetch error preserves `http 401` without the URL.
`Source::refresh_tracks` refreshes credentials only when the rejected URLs still
carry the current token, then rebuilds all URLs through `Client::file_url`.
Playback retries at its saved book position; a second rejection before a
successful start stops it. No audio-engine changes were needed.

The player uses the existing shell buttons, icons, theme and focus ring. It
follows Ond's toolbar without sharing its code, so Ond's rendering code is
unchanged. The stream cache retains file-based eviction. Pinned books live in
a separate per-account offline store and play from local files when fully
downloaded.

## Speech processing

Both switches are saved under `playback`, are off for a new installation, and
are applied when the engine is created and whenever preferences change. Views
can send `PrefsAction::VoiceBoost(bool)` and `PrefsAction::SkipSilence(bool)`;
`PrefsView::playback` contains the current values. No speech controls are added
by the engine implementation; the playback popover supplies those switches.

The decoder runs channel mapping, silence removal, pitch-preserving stretch,
voice boost and device-rate resampling, in that order. Volume remains in the
output callback. With both switches off the optional processors are absent;
the unity-speed path still passes original float samples through bit for bit.
Ond sends neither switch and keeps its existing audio path.

Silence removal measures RMS across all channels in 10 ms windows. The
threshold is absolute −45 dBFS: no cheap book loudness measurement is currently
available. Runs of at most 400 ms are kept unchanged. A longer run keeps its
first 200 ms, fades the last 5 ms to zero, and discards the remaining quiet
frames. The last two or three quiet windows (20–30 ms) are held back and played
into the returning speech, ramped up from silence, so a soft onset is kept and
the cut doesn't land on a consonant; the speech itself is not faded. Detection
is quantized to windows; a window containing speech is preserved.

Lookahead is bounded at 410 ms of source audio. Its samples and origins are
stored together in one buffer whose capacity is reserved for the format when
the detector is built; a seek or a restart clears it in place, so a book's
seeks don't allocate. Origins are kept per run of frames rather than per frame:
a 410 ms window is a few files, not tens of thousands of records. Short pauses
can therefore add up to 400 ms of decoder lookahead, independently of speed.

Buffered frames retain their file sequence and source frame. A quiet run can
span gapless files, including a boundary inside the first 400 ms, without
retaining another pause for each file. A file whose own output is shorter than
the 100 ms the decoder waits out at its end — a first file entirely inside
lookahead, or one silence removal shrank below the floor — gets that same
100 ms for the engine to name its next file. File format changes flush lookahead
and restart the detector. Seeks discard pending windows and start detection at
the new source position. An interrupted push (a seek arriving mid-piece) marks
a cut even when the seek then resolves to nothing, so the next kept frames
publish their own source origin rather than leaving the clock behind.

At a cut, the stretch tail is drained and its storage reused. The resampler
is drained/reset there too, so a wholly quiet tail cannot strand source-time
metadata behind its filter. Consecutive removed windows become one source-time
jump at the next retained frame or EOF. Gapless file boundaries retain their
own markers and source origins; EOF has an exact source endpoint. Between
jumps, speed still gives the source/output ratio. Sonic's existing local
pitch alignment tolerance (up to two 65 Hz periods, about 31 ms) remains;
silence removal adds no accumulating position drift.

`Status::silence_saved_secs()` reports removed **source seconds** after the
output reaches each cut, including gapless files. It excludes savings still
queued ahead, retains heard savings through seeks, speed changes and internal
restarts, and counts another pass through a pause again. Stop retains the last
total; a new `Play`/`PlayFrom` resets it. It is a play-session counter, not a
persistent counter for the book.

Rede keeps the book's total: `Player::silence_saved` adds the sessions that
`PlayFrom` ended (chapter jumps, a file's end, a token refresh) to the current
one, and starts over only when a different book loads. Wall-clock savings from
silence alone are this value divided by speed; the remaining-book display stays
an estimate because future silences have not been decoded.

Voice boost uses a stereo-linked 2:1 peak compressor with a −18 dBFS threshold,
20 ms attack and 150 ms release. A peaking EQ adds 3 dB at 3 kHz (Q 0.8),
moving to 0.2 × the sample rate below 15 kHz, with 3 dB makeup gain. A linked
limiter has instantaneous attack and 50 ms release; a final full-scale guard
also catches resampler overshoot. A 5 ms wet ramp starts the stage. Non-finite
input is replaced with silence before the filters, and their state is flushed
where it would go subnormal, so one bad sample can't poison the rest of the
session. The slow compressor attack preserves consonants; the limiter catches
exceptional peaks. Per-book loudness levelling is deferred.

`Command::SetVoiceBoost(bool)` and `Command::SetSkipSilence(bool)` use the
existing hold/fade/flush seek handshake at the current source position. They
take effect before refilling, preserve pause and the latest pending seek,
and survive file and speed changes. `Status::voice_boost()` and
`Status::skip_silence()` expose the requested settings. Speech processing is
reported as unavailable for bit-perfect output when explicitly enabled.

The new silence and voice processors do not allocate while processing blocks;
window, run and filter storage is allocated when one is built (or when a new
format needs another), reused across seeks, and voice processing uses a fixed
stack buffer. Existing Sonic and resampler buffering
remain on the decoder thread. The callback only consumes bounded sample,
marker and segment queues and publishes atomics; it adds no locks, allocation,
channels or logging.

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
