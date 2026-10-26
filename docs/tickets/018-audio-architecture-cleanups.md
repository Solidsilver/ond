# 018 · Audio engine: gapless across bit depths; single source of truth for dither; API tidy-ups

- Area: `crates/audio` · Kind: refactor/enhancement · Priority: P3
- Status: done (branch fix/audio-lifecycle) · Claimed by: — · Do after 002–005.
- Source: audio review (E, D). Line numbers at commit 669bb00.

## Items

- `StreamFormat.bits_per_sample` is part of ring identity (`decode.rs:545`), so a 16-bit track followed by a 24-bit one at the same rate, or lossy→lossless, reopens the device: gapless lost for mixed-depth albums. Make `source_bits` per-track (like the gain table).
- `lib.rs:752-756` re-derives `RingInput::needs_dither` (`output.rs:149-151`): two sources of truth for "dithered".
- `attach` returns `Option<Option<RingInput>>` (`decode.rs:543`): use an enum. `play()` is ~150 lines. `Track.out_rate` computed twice (`open`:375, `play`:482). `RingInput.resampled` means "processed" (resampled or downmixed); rename.
- Gapless path clears `next`/`prefetched` (`lib.rs:867-868`) but the format-change reopen path doesn't.
- Sentinels: `DecoderMsg::Error { seq: u64::MAX }` (`decode.rs:585`); marker ring cap 16 and `GAIN_SLOTS` 64 are implicit limits; document or assert.
- `fill_converted` chunking when `out.len() > scratch.len()`, `fade_out` with fewer queued samples than the fade, and >16 queued markers have no tests.
- Mixed bit-depth and lossy→lossless gapless tests.

## Resolution

1. Gapless across bit depths: done. A ring is now identified by the rate and channel layout going in and coming out; bit depth isn't part of it (the ring's `StreamFormat` has `bits_per_sample: None`). Each track's depth travels with its boundary marker (`output::Marker { at, source_bits }`), and the callback takes it up on the track's first sample. Nothing is lost by not reopening: the device format never depended on the source's depth (CoreAudio's `set_integer_format` picks by rate and channels). The signal path was already re-reported at each gapless boundary from the track's own format, so `assess`/`describe` judge each track on its own depth. On a 16-bit device a 24-bit track reads "device max 16/…" and `dithered`, and the 16-bit track after it is exact again (tested).
2. One source of truth for "dithered": done. `dither::needs_dither(device_bits, Treatment)` is the rule. The callback applies it to each stretch it writes, which also fixed 005's dither-after-ramp. `report_signal_path` applies it to the current track. `RingInput::needs_dither` is a thin wrapper over the same rule. Side effect: silence padding (paused, buffering, after the end) is no longer dithered, so digital silence stays silent. Before, Normal mode below full volume added dither noise to it.
3. API tidy-ups:
   - `attach` returns `Result<Attached, String>` (`Continued` / `NewRing` / `Cancelled`). A resampler that can't be built is now an ordinary error for its track. Before, it sent an `Error` itself and then posed as a cancel.
   - `play()` is split into `next_audio` (seeks, reads, linger, decode, trim, seek skip), `start_track` (first-packet setup, gain, attach, `Track` report) and `queue` (map, resample, push).
   - `out_rate`: one function, `Ctx::device_rate`. `open` sets it from the codec parameters (a restart's seek before the first packet needs it to convert the position) and `start_track` from the first packet's real format.
   - `RingInput.resampled` is now `processed`.
   - The format-change path now calls `forget_next()`, as the gapless one does.
   - The `seq: u64::MAX` sentinel was already gone (ticket 002 made `Error` carry the failing track's sequence number).
   - The marker cap is named (`MARKER_CAP` = 16, documented as the most tracks the decoder can be ahead), and `const _: () = assert!(MARKER_CAP < GAIN_SLOTS)` in `lib.rs` says why the gain table must be larger.
4. Tests added:
   - `output.rs`: `a_device_buffer_larger_than_the_scratch_is_filled_in_chunks`, `a_seek_fade_with_less_queued_than_its_length_fades_what_there_is`, `each_track_in_a_ring_is_dithered_by_its_own_depth` (16→24, 24→16 and depthless→16 inside one buffer).
   - `dither.rs`: `dither_is_for_integer_devices_and_off_grid_samples_only`.
   - `decode/tests.rs`: `a_decoder_a_full_marker_ring_ahead_waits_for_the_output`. Twenty 0.2 s tracks; the 18th waits for room instead of dropping a marker, then everything plays in order.
   - `tests.rs`: `tracks_of_different_depths_play_gaplessly_on_one_stream` (16 → 24 → float → 16 bits on one stream, bit for bit) and `on_a_16_bit_device_only_the_tracks_off_its_grid_are_dithered` (lossy→lossless and mixed depths; samples and `SignalPath` per track). Both failed with depth put back into the ring identity.
   - New WAV writers for the tests: `wav24`, and `wav_float`, a source without a bit depth like a lossy one.

All threaded tests ran 20× while the whole `audio` suite ran alongside: no failures. The two mixed-depth engine tests first failed under that load: 0.16 s tracks at ~11× real time left the test ~5 ms to name each next track. They now use 1 s tracks on a fake device running at ~2× real time.

Not done: nothing large came up.

Hardware (added to TESTING.md): a 16-bit then 24-bit track at the same rate stays gapless with no reopen, and on the UMC22 only the 24-bit one reads `dithered`.
