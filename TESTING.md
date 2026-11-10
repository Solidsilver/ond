# Manual test checklist

What automated tests can't cover: real hardware, the real server, and macOS integration.
Run with `cargo run -p ond-player --release`. Visual review without a server:
`cargo test -p ond-player --release snapshots -- --ignored --nocapture` → `target/snapshots/*.png`.
Frame times on a big library and the search index's keystroke latency:
`cargo test -p ond-player --release perf -- --ignored --nocapture`.

## Audio output
- [ ] UMC22, 16/44.1 file, bit-perfect: green dot; Audio MIDI Setup shows 44.1 kHz; other apps are silent on the UMC22 (hog mode).
- [ ] UMC22, 24/96 file: plays; status reads `24/96 → 16/48 · resampled, dithered · device limit`; no clicks at track changes.
- [ ] Headphone jack, 24/96 file: verified at 96 kHz; 24/192 plays at 96 kHz (resampled).
- [ ] Speakers: Normal by default; bit-perfect shows "not verified (macOS speaker processing)".
- [ ] Toggle Normal ↔ bit-perfect mid-track (⇧⌘B): no restart. Back in Normal, other apps can play again.
- [ ] Stop playback in bit-perfect: other apps can use the DAC again (hog released). Same after quitting.
- [ ] Unplug the UMC22 mid-track: continues on speakers; "Switch back" appears when it's plugged back in.
- [ ] Per-device mode remembered across devices and restarts.
- [ ] Output panel: System output follows the system default (no device list, every device plays Normal); Choose a device goes back to the device last chosen; picking one in the list leaves System output. Settings › Output shows the same choice.
- [ ] UMC22 in bit-perfect: the toolbar slider sets the device's own level (a green dot in its knob; the dB beside it as Settings › Output has it), the panel's row has the exact dB (↑↓ 0.5 dB, ⇧ 0.1, scrolling) and mute. Dragging it doesn't jump back after letting go; a change made in Audio MIDI Setup (or `alsamixer`) shows in Ond within a second; turning it while muted unmutes.
- [ ] Settings › Output: the eye on a device's row takes it out of the output menu and puts it back; a device put in a new group shows under that heading in the menu, between the hardware and Other.
- [ ] Following the system default, plug in headphones mid-track (or pick another default in Control Center): playback moves there and carries on from where it was.
- [ ] UMC22 in bit-perfect, a 44.1 kHz track followed by a 96 kHz one: other apps stay silent across the switch (hog kept through the reopen); no extra click beyond the rate change.
- [ ] Toggle Normal → bit-perfect mid-track with a 24-bit file on a DAC that shares at 16 bits: the status shows the wider format, and a bit-exact check (loopback recording, or the DAC's own bit meter) shows the samples weren't dithered down to 16 bits.
- [ ] A 16-bit track followed by a 24-bit one at the same rate (queue them by hand if no album mixes them), bit-perfect on the headphone jack: no gap or click between them, Audio MIDI Setup shows no reopen, and the status changes from `16/44.1` to `24/44.1` at the boundary. On the UMC22 (16-bit) the 24-bit track reads `dithered` and the 16-bit one doesn't; a loopback recording of the 16-bit one is bit-exact.
- [ ] A 44.1 kHz track followed by a 48 kHz one (a format change): the first track's last note isn't clipped before the switch. Same at the end of the queue: the last track's tail plays out before playback stops.

## Playback
- [ ] Debug build (`cargo run -p ond-player`, no `--release`): a 24/96 FLAC plays without dropouts (dependencies are optimised in debug builds).
- [ ] Gapless: a live album plays without gaps (also a hi-res one, resampled on the UMC22).
- [ ] Seek into an undownloaded part: old audio stops at once, spinner, then plays from the new spot.
- [ ] Previous / ⌥⌘← in the last seconds of the queue: lands and plays (no stuck silence).
- [ ] ReplayGain Track/Album/Auto: status shows `RG … track|album`; levels even out; bit-perfect shows "ReplayGain bypassed".
- [ ] Streaming → Always AAC 256 (check Navidrome has the AAC transcoder): `AAC 256 · transcoded`.
- [ ] Settings › Queue: the Apple Music and Spotify presets set the four rows at once and the queue follows them (Play next goes to the top with Apple Music, after the next track with Ond); changing a row turns the preset to Custom; the choice survives a restart.

## macOS integration
- [ ] Control Center shows title/artwork/progress; scrubbing there seeks.
- [ ] F7/F8/F9 and headphone buttons work, also with the window minimized.
- [ ] Menu bar: all menus present; checkmarks follow queue/bit-perfect/appearance; ⌘C/⌘V still work in the search field.
- [ ] Mini (⇧⌘M): on top, draggable; "Up next" grows into Compact; geometry restored per mode after relaunch.

## Zoom (Ond and Rede)
Snapshots draw each zoom offscreen; they can't move a real window. Check in both apps:
- [ ] ⌘+ / ⌘= / ⌘− / ⌘0 step 80 → 200% one step a press (no extra tenth from egui), with one "Zoom N%" toast that replaces the last and fades. Ond: View › Zoom In / Zoom Out / Actual Size do the same.
- [ ] Quit at 150% and relaunch: the first frame is already at 150% (no flash at 100%), and the window has its saved size.
- [ ] At 200% the traffic lights sit centred in the title bar (Full and Player) and nothing is drawn under them; at 80% the transport doesn't slide under them either.
- [ ] Full window: zooming in leaves its size alone unless it's under the new minimum, when it grows; shrinking it by hand stops at the minimum for that zoom. On a 1440×900 or 13" screen at 175–200% it stops at 90% of the screen instead (below the content's minimum; Ond's folded sidebar rail crowds at 200% there).
- [ ] Player and Mini at 200%: switch to each (⌥⌘2, ⌥⌘3) and back. Each opens at least at its minimum for that zoom, Mini stays square, nothing is clipped, and Full comes back at its saved size. Zoom from 100% to 200% while in Mini: the window grows to fit, still square.
- [ ] Ond: a managed `zoom` in managed.json locks the stepper ("Set in your Nix config") and the keys do nothing.
- [ ] Linux (our own window buttons): at 150% the buttons and resize edges still work.

## Linux
Try at least one GNOME and one KDE Plasma session, Wayland and X11 if you can.
- [ ] Installs with `./install.sh`; Ond appears in the app grid with its icon, and the running window gets the same icon in the dock/taskbar (app id `dev.solidsilver.ond`).
- [ ] Playback goes through PipeWire in Normal mode: `pw-top` lists Ond while playing. The output picker lists the ALSA sound cards; bit-perfect takes the card from PipeWire and gives it back a few seconds after playback stops (`pw-top` shows the sink go and come back). Unplugging the active device falls back cleanly.
- [ ] Media controls: GNOME's top-bar player / KDE's media widget show title, artist, album and cover; play/pause, next, previous and the seek bar work. Clicking the player should bring Ond forward; on Wayland the compositor may only flag it as wanting attention (it won't hand over focus without an activation token), which is fine. Media keys work. `playerctl metadata` shows the track.
- [ ] Ctrl+Alt+←/→ (skip 10 s): GNOME uses these to switch workspaces by default, so they may never reach Ond there. Note whether they do.
- [ ] KDE Connect or GSConnect controls playback from a phone.
- [ ] The password survives a relaunch (GNOME Keyring / KWallet).
- [ ] UI font is the desktop's sans-serif; Japanese titles render.
- [ ] Shortcut overlay (Ctrl+/) shows Ctrl/Alt/Shift names; Ctrl+K, Ctrl+1..6, Ctrl+V into the login fields work.
- [ ] Mini mode: undecorated window can be dragged, resized and closed (Alt+F4); on Wayland check resizing in particular.
- [ ] Settings → Offline → Move opens the desktop's folder dialog; "Show in File Manager" opens the folder.

## Windows
CI builds and runs the tests; nothing below has been tried on real Windows yet.
- [ ] `Ond.exe` has the icon in Explorer and the taskbar; no console window opens.
- [ ] The volume flyout's media panel shows the track and cover, and its buttons and seek work; keyboard media keys work.
- [ ] Password survives a relaunch (Credential Manager).
- [ ] Segoe UI is the UI font; Japanese titles render.
- [ ] Folder picker, "Show in File Explorer", offline free-space figure.

## Home Manager
- [ ] `programs.ond.settings` with a theme, ReplayGain and an output device mode, then `home-manager switch`: `managed.json` appears next to `settings.json` (`~/.config/ond/`, or `~/Library/Application Support/dev.solidsilver.ond/` on macOS). Ond shows those rows greyed with "Set in your Nix config"; the menu, ⇧⌘B and the output picker can't change them; everything else still can.
- [ ] Remove one from the config, switch, restart Ond: it's back to what was picked in the app before.
- [ ] Set `server` and `username` for a server you're not signed in to: Ond opens on the sign-in form with both filled in and locked; after signing in, the next start connects as usual.
- [ ] Set `offline_dir` to another folder, restart: downloads move into `Ond Offline` inside it, and nothing else in that folder is touched.

## Server features
- [ ] Navidrome "now playing" shows the track; play count increments once after half the track (or 4 min).
- [ ] Hearts and ratings show up in the Navidrome web UI; quick repeated toggles end in the right state.
- [ ] Playlists: ⌘N create, rename, delete; Add to Playlist; drag a song onto a sidebar playlist; reorder and remove persist.
- [ ] First sync: the status shows "Syncing tracks N/M" (bulk path); the palette finds songs; Songs view scrolls smoothly.

## Offline
- [ ] Download an album (⇧⌘D): ring → "Downloaded · N MB"; an album already played to the end downloads almost instantly.
- [ ] Stop Navidrome and relaunch: opens offline; downloaded tracks play; others are dimmed and skipped with a notice; reconnects by itself.
- [ ] Stop a track sync midway: downloads are NOT removed.
- [ ] Move downloads to a folder on another drive (a USB stick), relaunch: the window opens at once with "Moving downloads to …: X of Y" counting up, then "Downloads moved to …"; downloaded songs play once it's done (offline: not before), and an album downloaded meanwhile starts then.
- [ ] Quit mid-move (⌘Q), relaunch: the move carries on and finishes; the old folder ends up gone, no `.…moving` files are left in the new one, and no download is lost.
- [ ] Sign in as another user with many downloads: no hitch when it connects; switch back and that user's downloads are all still there.
- [ ] Quit while a big download or a move runs: the app is gone within a couple of seconds.

## Releases and updates
- [ ] Push an `ond-v` tag whose version isn't the crate's: the release workflow stops at its first job. With the right version, it builds and publishes "Ond X.Y.Z" with the notes from `.github/release-notes/ond/`, and GitHub's changelog starts at Ond's previous tag.
- [ ] Push a `rede-v` tag: only the first job runs, with a notice that Rede release packaging is not enabled in CI; no release is published.
- [ ] A GitHub build older than the newest Ond release: Settings › About says the new version is available, shows its notes, and Download opens its page. A Rede release newer than it doesn't change that.

## Analysis
- [ ] Settings → Library → Analyze library: progress, pauses while playback buffers; album pages show `DR · LUFS` and, only when suspicious, a "likely upsampled / lossy source" chip.

## Visualizer (Settings › Experimental)
- [ ] ⌘T opens the chosen one; ← → switch with its name showing briefly; the choice is there after a restart. Shuffle changes it at each new track.
- [ ] Apple-silicon laptop, full screen (F): smooth at the display's rate (120 Hz on ProMotion); the fans stay quiet.
- [ ] Surface Pro 6 (Intel iGPU, Linux, Vulkan), full screen: Flow and Particles stay smooth; the log doesn't say the pictures fell back to 8-bit.
- [ ] A quiet recording (classical, solo piano) still moves; a track with a kick drum makes Flow and Particles jump on the beat.
- [ ] The cover, title, artist and album show bottom-left for a few seconds when a track starts, then fade.
- [ ] Paused: the picture holds still and the app goes back to its idle frame rate.

## Rede playback

Use a release build with an Audiobookshelf account. The opt-in live test and
its progress cleanup are described in [Rede playback](docs/rede/playback.md).

- [ ] Listen at 0.5×, 1×, 1.5×, 2× and 3×: pitch stays natural, speech stays intelligible, and skips and seeks land where the scrubber says.
- [ ] Play a single-file m4b and a book split across files. Listen through a file boundary for a gap, repeated speech or missing speech; chapters need not line up with files.
- [ ] Pause for more than five minutes, then resume: it rewinds five seconds by default. Set rewind to 0 and try again. A short pause does not rewind.
- [ ] Space, Left/Right, Shift+Left/Right and the bracket keys work; changing the skip intervals and speed survives a relaunch. Typing in an editable field does not control playback.
- [ ] Restart ABS during listening: the next report reopens a session without interrupting the audio. Interrupt the network, keep listening from buffered audio, reconnect and confirm the final position reaches ABS.
- [ ] Switch books, stop, sign out and quit: the old session closes when reachable, and resuming keeps the final position. At the end the book is finished on ABS.
- [ ] Leave the library and book page paused: CPU settles and the UI does not keep repainting. Resize to the minimum window and inspect the title bar and the Playback popover.

## Rede: volume and signing in

By ear, with a real output device; the sign-in checks need a live server.

- [ ] Set the volume in the Playback popover and with ⌘↑ / ⌘↓, mute and unmute with the speaker, and relaunch: the level and the mute survive. Start a sleep timer at 40% and listen to the last ten seconds: the fade goes down from 40%, not from full, and cancelling it (or Extend 10 minutes) comes back to 40%.

## Rede M2: sleep, window modes and macOS controls
Run Rede in release mode. These checks need a real output device and, for
streaming/session persistence, an Audiobookshelf account; automated fixtures
and snapshots do not sign in to a server.

- [ ] Start a sleep timer, hide or minimize Rede through the final minute, and listen for a smooth ten-second fade followed by pause. Show it again: resume is audible at the original volume and starts ten book seconds before the fade ended; restart Rede and confirm that saved position.
- [ ] Choose End of chapter on a chapter spanning two audio files, change speed to 1.5× and then 0.75×, and confirm it still pauses at that chapter's end. Test the final chapter too: sleep must leave the book unfinished with the rewound position.
- [ ] During the final-minute notice, click Extend 10 minutes once. The fade stops, volume returns, and the countdown gains ten minutes. Cancel a fading timer and confirm volume returns. Step the Sleep timer popover's Custom length and start it; on Settings › Playback (⌘,, the account menu, or Settings… in the Playback popover) change Rewind after sleep. Both survive relaunch.
- [ ] Switch Full / Player / Mini using buttons, ⇧⌘N, ⇧⌘M and ⌥⌘1/2/3 (Ctrl/Alt equivalents elsewhere). Mini stays above other windows, drags by the cover, stays square while resizing, and double-click returns to its originating mode. Each mode restores its geometry, including after relaunch; test a maximized Full window on Linux.
- [ ] In Player, check the large cover, book/chapter titles, both progress bars, chapter/skip/play controls, speed and sleep menus in light and dark. In Mini check chapter text, skip/play controls and the extension notice at the smallest size. Check keyboard and screen reader focus, including ⌘/ and Escape for the shortcuts list.
- [ ] macOS Control Center shows Rede's book title, chapter subtitle, chapter number/count where supported, cover and whole-book duration/elapsed time. At 1.5× its progress advances at the real speed; pause and buffering stop the clock. Scrubbing seeks to the same whole-book position across chapters/files. Switching books must not show stale artwork.
- [ ] Change Rede's skip intervals; Control Center's skip commands use the new preferred intervals. F7/F8/F9 and headphone play/pause/skip work with Rede hidden and minimized. Repeat after a sleep stop, and confirm pause/play commands are idempotent.
- [ ] Ond retains its existing macOS controls: next/previous tracks, skip commands disabled, unity playback rate, artwork and seeking, including while hidden.

## Rede: the player in the title bar

- [ ] Drag the window by the title bar's empty parts, including the wide stretch between the capsule and the chapters toggle, and by the capsule (not its scrubber, cover or chips); double-click them to zoom. In full screen the transport moves to the window's edge.
- [ ] Hover the capsule's cover: it dims with the Player's glyph and a tooltip; click it and the Player opens. Mini is ⇧⌘M, and a button in the Player's bar; both return to the full window as before.
- [ ] Click the speed chip and the sleep chip: their menus open beside them. Start a timer: the sleep chip lights with the minutes left ("End" for the chapter's end), counts down, and goes back to the moon when cancelled.
- [ ] Drag the capsule's scrubber through an hour-long chapter at 1440, 1920 and full-screen widths: the times follow the drag and the release lands where they said. Arrow keys, Home and End move it when it has focus.
- [ ] Resize from the widest to the smallest window: the capsule grows to 1000 points and no more, stays centred on the window as the sidebar folds and the chapters column opens, and shrinks without anything overlapping. On macOS the chapter buttons and the cover stay down to the smallest window; inside the capsule the book's time left goes before the chapter's name is cut, then the title.
- [ ] Click the chapter's name: the chapters column opens on Chapters with the playing one in view. Click a chapter: playback goes to its start. Toggle with the button and ⌥⌘U; the choice survives a relaunch. In a narrow window the column is a sheet that Esc or a click outside closes.
- [ ] Play a book with a hundred or more chapters (the test server has one with 178): the column scrolls smoothly and the title bar's book line drops its chapter breaks rather than turning into dashes.
- [ ] On Linux, with the window buttons on the right and on the left, nothing in the title bar runs under them.
- [ ] The sidebar: fold it with its button and ⌥⌘S, relaunch, and it stays as left. Narrow the window and it becomes the rail with no expand button; widen it and the full column returns. The account at its foot shows the server while connected, a turning spinner while syncing (which stops after half a minute if the sync stalls), and "Offline · retrying" with the network off, back to the server when the next sync gets through. Its menu has Sync now, Keyboard shortcuts and Sign out, and opens up from the foot. The ? beside it and ⌘/ open the shortcuts.

Rede's output device picker is deferred: Ond's current panel combines device
preferences and bit-perfect controls. Rede continues following system output;
select AirPlay or another output in the system controls for now. ⌘B bookmarks
and ⌘K search remain for later milestones.

## Rede OpenID sign-in on macOS

These checks require a person signing in to an identity provider. Automated
checks use local fake ABS servers only. Do not put a password, verifier, state,
code or token in terminal commands, screenshots or bug reports.

Build and launch a dev bundle (release code, ad-hoc signature, no archives):

```sh
scripts/bundle-rede-macos.sh --dev
open target/bundle/Rede.app
```

`cargo run -p rede` does not register the bundle's URL schemes and cannot receive
the OAuth redirect. Launch the bundle with `open`. If another Audiobookshelf app
owns `audiobookshelf://`, choose Rede in macOS's handler prompt or explicitly
select Rede as that scheme's handler for the fallback test. `rede://` avoids
this shared-scheme ambiguity. Ad-hoc bundles may need approval in macOS Privacy
& Security on first launch. Keep the keyring unlocked; Rede must save the
pending transaction before it opens a browser.

- [ ] Enter the server address and click Sign in. Test servers with OpenID only,
  local only, and both methods: only enabled methods appear, the configured
  OpenID button text is used, local appears first unless OpenID auto-launch is
  enabled, and a configured custom message is readable. With auto-launch on,
  discovery opens the browser once; Cancel leaves the methods available.
- [ ] Allow `rede://oauth` in ABS's OpenID mobile redirect URIs. Click the OpenID
  button, finish sign-in in the system browser, approve opening Rede, and confirm
  the library loads. Restart Rede and confirm it resumes with stored tokens.
- [ ] Leave ABS's default whitelist (`audiobookshelf://oauth` only). Sign in and
  confirm the waiting screen says the server uses `audiobookshelf://oauth` and
  that the admin can add `rede://oauth`. Finish through that fallback scheme.
  A different server error must produce an error without retrying the fallback.
- [ ] While Rede is already running, finish browser sign-in: the Apple Event
  reaches its inbox and the waiting screen completes. Repeat, but quit Rede
  after the browser opens and before finishing. The redirect must launch the
  bundle, recover its pending verifier/session from the keyring, and finish the
  same login. Repeat with each scheme. Finish within five minutes.
- [ ] Cancel while waiting; finish the old browser page afterward. The late
  redirect is ignored and cannot sign in. Start again and wait five minutes:
  the spinner stops, the attempt times out, and retry creates a fresh attempt.
  Quit while waiting, leave it for five minutes, then finish: the expired
  transaction is discarded on launch. Deny access at the provider and confirm
  Cancel/timeout remains available if it does not return to Rede.
- [ ] Confirm an ABS version below 2.26 is refused and a version below 2.36
  retains the existing warning. Inspect `~/Library/Logs/Rede/rede.log` for
  value-free unmatched-redirect messages and no OAuth credentials.

`scripts/bundle-rede-macos.sh` without `--dev` also builds Rede's zip and dmg;
`--universal` builds both architectures. Ond continues to use its unchanged
`scripts/bundle-macos.sh` and `packaging/macos/Info.plist.in`.

## Rede speech processing (M3)

These checks need real output and a person listening. Use a release build.
The switches are on Settings › Playback (⌘,) and in the Playback popover
(the speed chip, or the Player's speed pill), which also says how much skip
silence has saved in the book.

- [ ] Listen to speech with both switches on at 1×, 1.5× and 2×. Consonants stay
  clear, quiet passages are audible, stereo placement stays steady and loud
  peaks do not crackle. Compare with each switch off.
- [ ] Use long pauses, room tone, very quiet narration and a music-heavy intro.
  Long quiet runs shrink to about 200 ms of source time; short pauses and
  music retain their rhythm. Listen for clicks or pumping at the cuts.
- [ ] Include a pause crossing a gapless file boundary and an entirely quiet
  file/tail. Playback advances without stalling; chapters, scrubber, source
  position and progress jump by the removed time. The saved-time readout moves
  only when that cut reaches output, and stays correct after changing speed.
- [ ] Toggle each switch while playing and paused; seek into and out of a long
  pause, change speed, and cross a file boundary. No old buffered speech leaks
  through a seek, pause remains paused, and settings survive a relaunch.
- [ ] Check the chapter sleep timer near a removed pause; it follows book time.
  Check a minutes timer separately; it follows elapsed listening time.
- [ ] Play music in Ond with its default settings and compare its usual output
  and signal path; neither speech stage is enabled there.

## Rede M3: bookmarks and reconciliation

Use a release build, a dedicated Audiobookshelf account and a second client
(such as a phone).

- [ ] Add a bookmark with ⌘B (in the full window, the Player and Mini), with
  Add bookmark on the Bookmarks tab, and with the Player's bookmark button.
  The default title is the chapter (the row shows the time), and a toast confirms it with the time; its
  Show opens the tab. Rename one from its … menu and from a right-click (Enter
  keeps it, Esc doesn't, an empty title isn't taken), jump to it with a click,
  then delete it and use the toast's Undo. Verify the same changes on the
  phone, including a fractional-second bookmark.
- [ ] Add, rename and delete on the phone, then focus Rede without waiting five
  minutes: its Bookmarks tab reflects the phone. Repeat with an ABS version
  before 2.36 to exercise the `/api/me` fallback.
- [ ] Disconnect the server, make several bookmark edits, quit and relaunch.
  They remain visible locally. Reconnect and confirm they arrive in order and
  deleted bookmarks remain deleted. Edit the same title on the phone while Rede
  is offline: the phone wins on reconnect. A phone deletion defeats a local
  rename rather than resurrecting it.
- [ ] Pause Rede, listen on the phone for more than a minute, and focus Rede.
  It stays paused at the newer position and offers "Picked up where you left off
  on another device, … in." as a toast with Undo; clicking the toast away keeps
  the newer position and the offer doesn't come back. Repeat by quitting/reopening Rede and by resuming a
  paused book; resume asks the server first but plays within about 2.5 s even
  when the server is slow or unreachable.
- [ ] Undo that reconciliation. The old local position returns in Rede and on
  the server; focusing again and restarting Rede must not bring the old phone
  position back. Repeat after the phone finished the book: Undo restores the
  unfinished local position, except inside ABS's automatic finish threshold.
  This also confirms ABS honours the `lastUpdate` Rede sends with the
  un-finish reset; if it doesn't, the book can jump to the start.
- [ ] Change the phone position while Rede is actively playing the same book:
  Rede keeps playing from its local position and reports it. Pause Rede and
  listen later on the phone: the next pull can adopt that newer position.
- [ ] Make a newer local offline position and reconnect after an older phone
  position: local wins and is reported. A server change of exactly one minute
  or less updates paused progress without the reconciliation toast.
- [ ] Sign out or switch accounts while pulls are pending: no old bookmark or
  reconciliation result appears in the new session. Offline network errors are
  quiet after the first; a server refusal or local storage error is shown.
- [ ] Make the server refuse a bookmark edit outright (e.g. remove the
  account's access to the library): the error shows once, the edit is dropped
  and later edits still sync. A 401 or 403 from a lapsed proxy or SSO session
  keeps the edits queued instead. A server answering 5xx is retried less and
  less often, up to every 10 minutes.

The opt-in `abs::a_real_server_round_trip` test uses one login and one explicit
refresh, creates/renames/lists/deletes a fractional bookmark, exercises progress,
and cleans the dedicated account back to zero bookmarks and progress. It refuses
to run against an account that already has progress or bookmarks, so use a
throwaway one. Run with `ABS_TEST_URL`, `ABS_TEST_USER` and `ABS_TEST_PASSWORD` in
the environment: `cargo test -p abs --test live -- --ignored --nocapture`. It
requires no audio hardware. Never include the environment values in logs or
reports.
