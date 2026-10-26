# 019 · Data layer robustness: lenient decoding, Unicode folding, resume validation, DR fidelity

- Area: `crates/subsonic`, `crates/library`, `crates/offline`, `crates/analysis`, `crates/app/src/lyrics.rs` · Kind: enhancement/plausible bugs · Priority: P3
- Status: partly done (branch fix/data-layer); left: lyrics offset, offline `update()` · Claimed by: —
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

## Resolution

### subsonic
- List items decode one at a time (`lenient::list`): an item that doesn't decode is logged and skipped, and a list sent as `null` or as a single object (servers converting from XML do this) is accepted. This covers every payload list plus nested ones (contributors, artist refs, genres, labels, lyric lines), so a bad contributor drops the contributor and keeps the song. Optional numbers (`track`, `discNumber`, `year`, `duration`, `bitRate`, `size`, ratings, counts) also accept strings and floats, and a value out of range becomes `None`. A malformed `replayGain` becomes `None`. Required fields stay strict: an item without `id` or `title` is skipped, and a reply missing its payload key (or a `getSong` without an id) is still an error.
- On an HTTP error status, the body is read: the Subsonic error message, an HTML page's `<title>`, or the first line of plain text is kept. `Error::Http` now wraps `HttpError`, which derefs to `reqwest::Error`, so the app's `h.is_connect()`/`h.status()` matches compile unchanged. `HttpError::message()` exposes the text, and `Display` includes it. The app's connect message doesn't show it yet.
- JSON calls and covers have a 120 s total timeout (`CALL_TIMEOUT`, settable with `Client::with_call_timeout`). Streams have none.
- Found while writing the base-URL tests: `"host:4533"` parsed with `host` as the scheme, and `endpoint()` then panicked on `join`. `Client::new` now refuses anything that isn't http(s) with `Error::Url`. A query string or fragment on the address is dropped.
- Tests: a malformed song/album/artist/playlist entry skipped in each list; numbers as strings and floats; out-of-range values; null and lone-object lists; a missing payload still fails; a `failed` envelope without `error`; addresses with a query string, without a scheme, or with another scheme; HTTP 503 (HTML), 401 (Subsonic JSON), 500 (text) and an empty-body 502 against a local server; a trickling reply hitting the total timeout (it can only end by timing out, so it isn't timing-sensitive).

### library
- `fold` spells out letters NFKD leaves alone: ø→o, ł→l, ß/ẞ→ss, æ→ae, œ→oe, đ/ð→d, þ→th, ħ→h, ŧ→t, ı→i (capitals go through lowercasing first). Stored index keys come from `fold` (artists by name, albums by sort, credits), and removing an old index entry recomputes its key from the old row, so the old keys had to go. Otherwise a re-sync would have left duplicate album entries and stale credits. `SCHEMA_VERSION` is now 4, and the 3→4 step rebuilds those three indexes from the rows. Rows don't change, so album stamps are kept and nothing is refetched. An older build opening a v4 database gets the existing "newer version of Ond" error.
- The v2→v3 step now clears `ALBUM_STAMPS`, so migrated tracks really are refetched (no real v2 database had stamps, so this only matters for correctness), and it leaves the credits index to the 3→4 rebuild. The v2→v3 fixture has a populated stamp row, so the "stamps are empty after migration" assertion is no longer trivially true.
- `retain_albums` scans and deletes in one write transaction. Album removal opens its tables once (`AlbumTables`, which holds a `TrackTables`) instead of three per orphan; `remove_album` uses it too.
- `albums()`, `artists()`, `album_tracks()` and `all_tracks()` log and skip a row that doesn't decode. Single-row reads, `check_indexes` and migrations stay strict.
- Albums with no year sort first within an artist (`year.unwrap_or(0)`). Left as is; it reads as intended (undated releases before the dated discography), but it was never decided explicitly.
- Tests: `fold` table; Øystein sorts between N and Z; a v3 database with old-fold keys is rebuilt (consistent indexes, credits found by the plain spelling, stamps kept, a later upsert doesn't duplicate); corrupt album/track/artist rows left out of lists. Both migration tests fail without the 3→4 step.

### offline
- A resume asks for the last 64 KiB of the `.part` again (`RESUME_OVERLAP`) and appends only if those bytes match what's on disk, and, when the pin has a size, if the `Content-Range` total equals it. Otherwise it starts over from byte 0 in the same attempt. A 416 on a resume now also means "start over": the request begins inside the `.part`, so a 416 means the file shrank. This catches a same-size replacement whose bytes differ at the resume point, which the size check alone can't. A replacement that only rewrote bytes earlier in the file (a tag edit that fit in the padding) still resumes, which gives the old tags on the same audio, not a broken file.
- `If-Range` wasn't used: `streamcache::fetch::get_from` neither sends extra headers nor exposes the response's `ETag`/`Last-Modified`, the validator would have to be persisted next to the `.part` to survive a restart, and `crates/streamcache` was being changed in parallel. If `get_from` grows that, `If-Range` can replace the overlap check.
- Tests: a same-size replaced file starts over and ends up as the new file; a resume whose total isn't the pinned size starts over; a `.part` shorter than the overlap resumes from 0. The existing resume tests now expect the overlap in the `Range`.
- Not done: `update()` still ignores `suffix`/`size` changes when the track ids are the same. With a stale pinned size, a changed file fails `finish()` on every attempt until the pin is refreshed.

### analysis
- DR, checked against `compute_dr14` in dr14_t.meter (simon-r/dr14_t.meter, `dr14tmeter/compute_dr14.py`). The trailing partial block is now always a block. The block count is `floor(frames / block) + 1`, so an empty trailing slot counts toward the 20%. The top-20% count was already `floor`, not `round`, and the reference also reports with fewer than 3 blocks (a single block uses its own peak), so those two points needed no change of their own. Also matched: 44.1 kHz blocks are 3 × 44 160 frames. Deliberately not matched, as noted in `dr.rs`: the reference leaves the last sample out of the last block; a lone exactly-3 s block gets a zero second peak (DR 0); and silent or >24 dB channels count as 0 in the channel mean, where we leave them out.
- `ANALYSIS_VERSION` is 2, so stored results are recomputed once.
- `stamp()` takes the duration. Navidrome gives no modification time, so there is none to add. The one call in `crates/app/src/analyze.rs` passes `t.duration_secs`.
- `ResetRequired` from `next_packet` reopens the default audio track and its decoder. From `decode` it rebuilds the decoder and retries the packet once. There's no test: it needs a chained-Ogg fixture.
- "Upsampled" also needs a sharp edge: `Edge::sharp_db` is the drop across the cutoff's middle 800 Hz, and it must be at least 25 dB (about 45 dB/kHz). Measured on the synthetic noise: a 15 dB/kHz rolloff gives ~11, 30 dB/kHz ~22, a brickwall ~78. A 15 dB/kHz rolloff at 96 kHz used to read "possibly upsampled". All existing spectral tests pass unchanged. "Lossy source" keeps its criteria. Its evidence text for cutoffs ≥ 19.5 kHz now says early digital recordings stop there too.
- Tests: the reference ported into the tests and compared (±0.02 dB) at 31.7 s, 7 s, 2 s, 9.0144 s at 44.1 kHz and 12 s (exact multiple), 20.5 s at 48 kHz; a loud trailing partial block sets the DR; 3 channels; a gradual rolloff isn't called upsampled; the 20 kHz copy; `summarize` with mixed verdicts; a duration change changes the stamp. The reference and partial-block tests fail on the old meter.

### lyrics
Not done (another branch was editing `lyrics.rs`). Still open as described above.
