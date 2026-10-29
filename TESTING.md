# Manual test checklist

What automated tests can't cover: real hardware, the real server, and macOS integration.
Run with `cargo run -p ond-player --release`. Visual review without a server:
`cargo test -p ond-player --release snapshots -- --ignored --nocapture` → `target/snapshots/*.png`.

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

## Analysis
- [ ] Settings → Library → Analyze library: progress, pauses while playback buffers; album pages show `DR · LUFS` and, only when suspicious, a "likely upsampled / lossy source" chip.

## Visualizer (Settings › Experimental)
Just the visualizers as PNGs: `cargo test -p ond-player --release visualizer_snapshots -- --ignored --nocapture`.
- [ ] ⌘T opens the chosen one; ← → switch with its name showing briefly; the choice is there after a restart. Shuffle changes it at each new track.
- [ ] Apple-silicon laptop, full screen (F): smooth at the display's rate (120 Hz on ProMotion); the fans stay quiet.
- [ ] Surface Pro 6 (Intel iGPU, Linux, Vulkan), full screen: Flow and Particles stay smooth; the log doesn't say the pictures fell back to 8-bit.
- [ ] A quiet recording (classical, solo piano) still moves; a track with a kick drum makes Flow and Particles jump on the beat.
- [ ] The cover, title, artist and album show bottom-left for a few seconds when a track starts, then fade.
- [ ] Paused: the picture holds still and the app goes back to its idle frame rate.
