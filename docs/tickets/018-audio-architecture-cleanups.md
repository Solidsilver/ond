# 018 · Audio engine: gapless across bit depths; single source of truth for dither; API tidy-ups

- Area: `crates/audio` · Kind: refactor/enhancement · Priority: P3
- Status: open · Claimed by: — · Do after 002–005.
- Source: audio review (E, D). Line numbers at commit 669bb00.

## Items

- `StreamFormat.bits_per_sample` is part of ring identity (`decode.rs:545`), so a 16-bit track followed by a 24-bit one at the same rate, or lossy→lossless, reopens the device: gapless lost for mixed-depth albums. Make `source_bits` per-track (like the gain table).
- `lib.rs:752-756` re-derives `RingInput::needs_dither` (`output.rs:149-151`): two sources of truth for "dithered".
- `attach` returns `Option<Option<RingInput>>` (`decode.rs:543`): use an enum. `play()` is ~150 lines. `Track.out_rate` computed twice (`open`:375, `play`:482). `RingInput.resampled` means "processed" (resampled or downmixed); rename.
- Gapless path clears `next`/`prefetched` (`lib.rs:867-868`) but the format-change reopen path doesn't.
- Sentinels: `DecoderMsg::Error { seq: u64::MAX }` (`decode.rs:585`); marker ring cap 16 and `GAIN_SLOTS` 64 are implicit limits; document or assert.
- `fill_converted` chunking when `out.len() > scratch.len()`, `fade_out` with fewer queued samples than the fade, and >16 queued markers have no tests.
- Mixed bit-depth and lossy→lossless gapless tests.
