# Listener

A native macOS music player for [Navidrome](https://www.navidrome.org) and other OpenSubsonic servers, written in Rust with [egui](https://www.egui.rs).

I wanted a client that treats playback quality as the main feature and stays out of the way otherwise. Listener plays your original files, tells you exactly what happens to the signal on its way to the speakers, and keeps the interface quiet unless you ask for more.

## What it does

### Playback

- Bit-perfect output through CoreAudio. Listener takes exclusive access to the device and switches its sample rate to match each file. It shows a green dot only when every condition holds, and it says why when it can't.
- Normal mode for everyday listening, with ReplayGain (track, album, or automatic), a sinc resampler and TPDF dither when a file needs converting for your device.
- Gapless playback, and seeking into parts of a track that haven't downloaded yet.
- Every output device remembers its own mode. A USB DAC can stay bit-perfect while the laptop speakers stay in Normal mode.

### Library

- A local copy of your library, so browsing and the ⌘K palette search are instant.
- Offline downloads for albums, playlists and tracks. When the server is unreachable, the app keeps working with what's on disk.
- Library analysis: loudness (EBU R128), dynamic range, and a check for files that claim to be hi-res but were upsampled or made from lossy sources.
- Credits as links, synced lyrics, favorites, ratings and playlist editing.

### Windows and keys

- Three window modes. Full is the whole library. Player has large artwork with the queue below and lyrics in a drawer. Mini is a small square that stays on top.
- A queue that shows where each track came from and when it will start.
- Keyboard shortcuts for nearly everything. Press ⌘/ to see them all.
- Media keys, Control Center, and play counts and scrobbles sent through Navidrome.
- An optional diagnostics bar (⌥⌘I) with the signal path, buffer, download speed and server latency in one line.

## Install

Download the `.dmg` from [Releases](../../releases) and drag Listener to Applications.

The app isn't signed with an Apple Developer ID yet, so macOS blocks the first launch. Right-click Listener in Applications and choose Open, or run:

```sh
xattr -dr com.apple.quarantine /Applications/Listener.app
```

Listener needs macOS 11 (Big Sur) or later and a Navidrome server. Credits need Navidrome 0.55 or newer; everything else works with any OpenSubsonic server.

## Build from source

You need a recent stable Rust toolchain (`rustup`) and the Xcode command line tools.

```sh
cargo run -p listener --release
```

Use `--release` for listening. Debug builds are slow enough to cause dropouts.

To build `Listener.app`, a `.zip` and a `.dmg` in `target/bundle/`:

```sh
scripts/bundle-macos.sh              # this Mac's architecture
scripts/bundle-macos.sh --universal  # Apple silicon and Intel
```

## Where things are stored

| What | Where |
|---|---|
| Password | macOS Keychain |
| Settings | `~/Library/Application Support/dev.listener.Listener/settings.json` |
| Library and analysis databases | `~/Library/Application Support/dev.listener.Listener/` |
| Offline downloads | `~/Library/Application Support/dev.listener.Listener/Offline` (you can move it in Settings) |
| Stream and artwork caches | `~/Library/Caches/dev.listener.Listener/` |
| Logs | `~/Library/Logs/Listener/` |

The library database is a copy of what's on your server. If it ever breaks, Listener moves it aside and syncs again.

## Development

The workspace has seven crates:

| Crate | Job |
|---|---|
| `app` | The `listener` binary: UI, queue, sync, settings, macOS integration |
| `subsonic` | OpenSubsonic API client |
| `library` | Local library mirror in redb, with hand-written indexes and migrations |
| `streamcache` | Download cache that playback can read and seek while it fills |
| `offline` | Pinned downloads |
| `audio` | Decoding, the playback engine, resampling, dither, CoreAudio and cpal output |
| `analysis` | Loudness, dynamic range and fake hi-res detection |

Before sending changes:

```sh
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The UI tests can render every screen to PNG without a server, which is how most of the visual work gets checked:

```sh
cargo test -p listener --release snapshots -- --ignored --nocapture
# images land in target/snapshots/
```

Some things need real hardware or a real server. [TESTING.md](TESTING.md) lists them.

`main` holds releases and `dev` is where work happens. Pushing a `v*` tag builds the app and publishes a GitHub release.

## How this was made

I built Listener with Claude Opus 5.5. I set the direction, made the design and product calls, and tested on my own library and hardware. Claude wrote most of the code. I'd rather say that plainly than leave you to guess.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option.
