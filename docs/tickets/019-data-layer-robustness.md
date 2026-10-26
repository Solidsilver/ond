# 019 · Data layer robustness: lenient decoding, Unicode folding, resume validation, DR fidelity

- Area: `crates/subsonic`, `crates/library`, `crates/offline`, `crates/analysis`, `crates/app/src/lyrics.rs` · Kind: enhancement/plausible bugs · Priority: P3
- Status: open · Claimed by: —
- Source: data-layer review (13, 17–26), app-core review (B2). Line numbers at commit 669bb00.

## subsonic
- One malformed item fails a whole page: all list payloads decode strictly (`subsonic/src/model.rs`). Decode list items as `serde_json::Value` and skip/log failures, or lenient number parsers on `track`, `disc_number`, `year`, `bit_rate`.
- `error_for_status()` (`subsonic/src/lib.rs:117`) discards the body; surface the server message. No total timeout on `call`.
- Tests: `status: "failed"` envelope without `error` (`lib.rs:310`); base URL with query string / no scheme.

## library
- `fold` (`library/src/model.rs:12-14`) doesn't decompose ø, ł, ß, æ, đ, so "Øystein" sorts after "Z". Add a small replacement table after NFKD.
- `retain_albums` reads `gone` in a read txn then deletes in a write txn (`library/src/lib.rs:447-459`); do the scan inside the write txn. Opens three tables per orphan in the loop (`:477-479`); open `TrackTables` once.
- A single corrupt row makes `albums()` fail entirely; skip and log undecodable rows in bulk reads.
- Add a populated `ALBUM_STAMPS` row to the v2→v3 migration fixture (`library/src/tests.rs:543`) so the next migration author sees the assertion fail if stamps need clearing.
- Albums with no year sort first within an artist (`model.rs:110`); confirm intended.

## offline
- Resume appends to `.part` whenever the server honours `Range` (`offline/src/lib.rs:746-801`) with no `If-Range`/`ETag`; a replaced file of equal size is kept as `done` with mixed content. Send `If-Range` or compare the `Content-Range` total with `size` before appending.
- `update()` ignores `suffix`/`size` changes when ids are unchanged (`lib.rs:355-359`).

## analysis
- DR deviates from dr14_t.meter in three places (`analysis/src/dr.rs:56-77`): trailing partial block dropped (`:58-60`), `floor` vs `round` for the top-20% count (`:70`), reports from one partial block for < 3 blocks (`:72`). Verify against the published DR14 test signals and add a reference-value test.
- "Upsampled" (`lib.rs:293-298`) can fire on native 96 kHz captures of band-limited sources; require a *sharp* edge, not only a deep drop. "Lossy source" (`lib.rs:299-307`) at 20 kHz catches early-digital CDs; mention in UI copy.
- `stamp()` (`lib.rs:141-149`) omits duration and mtime, so a replaced file of identical size/suffix/bits/rate is never re-analysed.
- `Err(SymError::ResetRequired)` aborts analysis (`decode.rs:108`); rebuild the decoder instead.
- Tests: DR with trailing partial block / < 9 s; >2 channels; `summarize` with mixed verdicts.

## lyrics
- `lyrics.rs:36-40` applies `start + offset`; LRC convention is that a positive `[offset:]` shows lyrics *earlier*. Check Navidrome's `structuredLyrics.offset` semantics before changing; add a reference fixture.
