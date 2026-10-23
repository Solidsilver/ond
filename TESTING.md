# Manual test checklist

What automated tests can't cover: real hardware, the real server, and macOS integration.
Run with `cargo run -p listener --release`. Visual review without a server:
`cargo test -p listener --release snapshots -- --ignored --nocapture` → `target/snapshots/*.png`.

## Audio output
- [ ] UMC22, 16/44.1 file, bit-perfect: green dot; Audio MIDI Setup shows 44.1 kHz; other apps are silent on the UMC22 (hog mode).
- [ ] UMC22, 24/96 file: plays; status reads `24/96 → 16/48 · resampled, dithered · device limit`; no clicks at track changes.
- [ ] Headphone jack, 24/96 file: verified at 96 kHz; 24/192 plays at 96 kHz (resampled).
- [ ] Speakers: Normal by default; bit-perfect shows "not verified (macOS speaker processing)".
- [ ] Toggle Normal ↔ bit-perfect mid-track (⇧⌘B): no restart. Back in Normal, other apps can play again.
- [ ] Stop playback in bit-perfect: other apps can use the DAC again (hog released). Same after quitting.
- [ ] Unplug the UMC22 mid-track: continues on speakers; "Switch back" appears when it's plugged back in.
- [ ] Per-device mode remembered across devices and restarts.

## Playback
- [ ] Gapless: a live album plays without gaps (also a hi-res one, resampled on the UMC22).
- [ ] Seek into an undownloaded part: old audio stops at once, spinner, then plays from the new spot.
- [ ] Previous / ⌥⌘← in the last seconds of the queue: lands and plays (no stuck silence).
- [ ] ReplayGain Track/Album/Auto: status shows `RG … track|album`; levels even out; bit-perfect shows "ReplayGain bypassed".
- [ ] Streaming → Always AAC 256 (check Navidrome has the AAC transcoder): `AAC 256 · transcoded`.

## macOS integration
- [ ] Control Center shows title/artwork/progress; scrubbing there seeks.
- [ ] F7/F8/F9 and headphone buttons work, also with the window minimized.
- [ ] Menu bar: all menus present; checkmarks follow queue/bit-perfect/appearance; ⌘C/⌘V still work in the search field.
- [ ] Mini (⇧⌘M): on top, draggable; "Up next" grows into Compact; geometry restored per mode after relaunch.

## Server features
- [ ] Navidrome "now playing" shows the track; play count increments once after half the track (or 4 min).
- [ ] Hearts and ratings show up in the Navidrome web UI; quick repeated toggles end in the right state.
- [ ] Playlists: ⌘N create, rename, delete; Add to Playlist; drag a song onto a sidebar playlist; reorder and remove persist.
- [ ] First sync: the status shows "Syncing tracks N/M" (bulk path); the palette finds songs; Songs view scrolls smoothly.

## Offline
- [ ] Download an album (⇧⌘D): ring → "Downloaded · N MB"; an album already played to the end downloads almost instantly.
- [ ] Stop Navidrome and relaunch: opens offline; downloaded tracks play; others are dimmed and skipped with a notice; reconnects by itself.
- [ ] Stop a track sync midway: downloads are NOT removed.

## Analysis
- [ ] Settings → Library → Analyze library: progress, pauses while playback buffers; album pages show `DR · LUFS` and, only when suspicious, a "likely upsampled / lossy source" chip.
