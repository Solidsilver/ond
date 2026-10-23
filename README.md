# Listener

A native Navidrome (OpenSubsonic) music player in Rust + egui, built for
high-fidelity playback. Milestone 0: the plumbing works end to end; the UI is a
throwaway spike.

## Run

```sh
cargo run -p listener            # debug
cargo run -p listener --release  # use this for listening
RUST_LOG=debug cargo run -p listener
```

Enter your server URL, username and password. The password goes into the macOS
Keychain; server and username go into
`~/Library/Application Support/dev.listener.Listener/settings.json`, next to the
local library database (`library.redb`). On later launches it reconnects
automatically.

Double-click a track to play it. The rest of the album then plays in order.

## Crates

| crate       | role |
|-------------|------|
| `app`       | `listener` binary: eframe/egui (wgpu) UI, settings, the bridge that runs IO on tokio and posts results back over a channel |
| `subsonic`  | OpenSubsonic client: token+salt auth, typed models, `ping`, `getAlbumList2`, `getAlbum`, `getArtists`, `stream` URLs with `format=raw` |
| `library`   | Local mirror in redb 4 (postcard rows + hand-written tuple-key index tables for artist, album and track order; schema version in a `meta` table). Artwork will live on disk, not in the db |
| `audio`     | Engine: fetch → symphonia decode → DSP → `rtrb` ring → `OutputBackend`. `PlaybackMode::{Normal, BitPerfect}`, seeking (ring flushed by the output callback), next-track prefetch. Only the cpal backend exists today |
| `streamcache` | Progressive, seekable download cache: a tokio task fills `<id>.<suffix>` with HTTP Range requests while a blocking `Read + Seek` reader plays from it. LRU-capped (2 GB default, `stream_cache_mb` in settings.json); partial files are discarded. Lives in `~/Library/Caches/dev.listener.Listener/streams` |
| `offline`   | Offline downloads: pinned albums / playlists / tracks (refcounted, `offline.json`), a 2-at-a-time download queue that resumes with Range after interruptions and restarts, retries with backoff, verifies sizes, and hard-links complete stream-cache files instead of re-downloading. Never evicted; optional size limit. Lives in `~/Library/Application Support/dev.listener.Listener/Offline` (changeable from the Offline page) |
| `analysis`  | Placeholder for waveform, spectrum, loudness/DR and fake-hi-res detection |

## Checks

```sh
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
