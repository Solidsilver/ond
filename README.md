# Ond

Ond is a native music player for [Navidrome](https://www.navidrome.org) and other OpenSubsonic servers, written in Rust with [egui](https://www.egui.rs). It runs on macOS, Linux and Windows. macOS is where it started and has the most complete audio path; the Linux and Windows builds are new.

I wanted a client that treats playback quality as the main feature and stays out of the way otherwise. Ond plays your original files, tells you exactly what happens to the signal on its way to the speakers, and keeps the interface quiet unless you ask for more.

Note: Ond was built using LLMs. See [AI Disclosure](#ai-disclosure)

## What it does

### Playback

- Bit-perfect output through CoreAudio on macOS. Ond takes exclusive access to the device and switches its sample rate to match each file. It shows a green dot only when every condition holds, and it says why when it can't.
- Normal mode for everyday listening, with ReplayGain (track, album, or automatic), a sinc resampler and TPDF dither when a file needs converting for your device.
- PipeWire output on Linux (ALSA where PipeWire isn't running), WASAPI on Windows.
- Gapless playback, and seeking into parts of a track that haven't downloaded yet.

### Library

- A local copy of your library, so browsing and the ⌘K palette search are instant.
- Offline downloads for albums, playlists and tracks. When the server is unreachable, the app keeps working with what's on disk.
- Library analysis: loudness (EBU R128), dynamic range, and a check for files that claim to be hi-res but were upsampled or made from lossy sources.
- Credits as links, synced lyrics, favorites, ratings and playlist editing.

### Windows and keys

- Three window modes. Full is the whole library. Player has large artwork with the queue below and lyrics in a drawer. Mini is a small square that stays on top.
- A queue that shows where each track came from and when it will start.
- Keyboard shortcuts for nearly everything. Press ⌘/ (Ctrl+/ on Linux and Windows) to see them all.
- Media keys and the system's now-playing controls: Control Center on macOS, the GNOME and KDE media controls (MPRIS, which also reaches phones through KDE Connect and GSConnect) on Linux, and the media flyout on Windows. Play counts and scrobbles go through Navidrome.
- An optional diagnostics bar (⌥⌘I, or Ctrl+Alt+I on Linux and Windows) with the signal path, buffer, download speed and server latency in one line.

## Install

Downloads are on the [Releases](../../releases) page.

### macOS

Open the `.dmg` and drag Ond to Applications.

The app isn't signed with an Apple Developer ID yet, so macOS blocks the first launch. Right-click Ond in Applications and choose Open, or run:

```sh
xattr -dr com.apple.quarantine /Applications/Ond.app
```

Ond needs macOS 11 (Big Sur) or later.

### Linux

Unpack the `.tar.gz` and run its install script. It puts `ond`, the menu entry and the icons under `~/.local`:

```sh
tar xf Ond-*-linux-*.tar.gz
cd Ond-*/
./install.sh                 # or: sudo ./install.sh /usr/local
./install.sh --uninstall     # to remove it again
```

Ond needs PipeWire's library (`libpipewire-0.3`), which every current desktop distribution ships, and glibc 2.39 or newer (Ubuntu 24.04, Fedora 40, Debian 13). The password goes in the Secret Service keyring (GNOME Keyring or KWallet).

### Nix and NixOS

The repository is a flake. To try it without installing:

```sh
nix run github:Solidsilver/ond
```

To install it into your profile, `nix profile install github:Solidsilver/ond`. In a NixOS or home-manager config, add the flake as an input and either use `ond.packages.${system}.default` directly or apply `ond.overlays.default` and use `pkgs.ond`. Pin a release with `github:Solidsilver/ond/v0.2.0`.

### Windows

Unzip and run `Ond.exe`. There's no installer yet. Windows SmartScreen may warn on first launch because the app isn't signed; choose More info, then Run anyway.

### Servers

Ond works with Navidrome and other OpenSubsonic servers. Credits need Navidrome 0.55 or newer.

## Build from source

With Nix, `nix develop` gives you a shell with Rust, rust-analyzer and every system library the build needs, on Linux and macOS. With direnv, `direnv allow` once and the shell loads whenever you `cd` in. `nix build` builds the Linux package.

Without Nix, you need a recent stable Rust toolchain (`rustup`), plus:

- macOS: the Xcode command line tools.
- Linux: ALSA and PipeWire headers, clang and pkg-config. On Debian and Ubuntu that's `sudo apt install libasound2-dev libpipewire-0.3-dev clang pkg-config`; on Fedora, `sudo dnf install alsa-lib-devel pipewire-devel clang pkgconf`.
- Windows: Visual Studio's C++ build tools.

```sh
cargo run -p ond-player --release
```

Use `--release` for listening. Debug builds are slow enough to cause dropouts. The package is called `ond-player`; the binary it builds is `Ond`, so the app shows as Ond in the macOS menu bar and Dock even when you run it with cargo. To see the window-mode timings in the log, run with `RUST_LOG=ond::window=debug`.

To package a release build in `target/bundle/`:

```sh
scripts/bundle-macos.sh              # Ond.app, .zip and .dmg for this Mac's architecture
scripts/bundle-macos.sh --universal  # the same for Apple silicon and Intel
scripts/bundle-linux.sh              # .tar.gz with the install script
pwsh scripts/bundle-windows.ps1      # .zip with Ond.exe
```

## Where things are stored

| What | macOS | Linux | Windows |
|---|---|---|---|
| Password | Keychain | Secret Service keyring | Credential Manager |
| Settings | `~/Library/Application Support/dev.solidsilver.ond/` | `~/.config/ond/` | `%APPDATA%\solidsilver\ond\config\` |
| Library, analysis and offline downloads | `~/Library/Application Support/dev.solidsilver.ond/` | `~/.local/share/ond/` | `%LOCALAPPDATA%\solidsilver\ond\data\` |
| Stream and artwork caches | `~/Library/Caches/dev.solidsilver.ond/` | `~/.cache/ond/` | `%LOCALAPPDATA%\solidsilver\ond\cache\` |
| Logs | `~/Library/Logs/Ond/` | `~/.local/state/ond/logs/` | `%LOCALAPPDATA%\solidsilver\ond\data\logs\` |

You can move the offline downloads folder in Settings.

The library database is a copy of what's on your server. If it ever breaks, Ond moves it aside and syncs again.

## Development

The workspace has seven crates:

| Crate | Job |
|---|---|
| `app` | The `ond-player` package, which builds the `Ond` binary: UI, queue, sync, settings, and each system's integration |
| `subsonic` | OpenSubsonic API client |
| `library` | Local library mirror in redb, with hand-written indexes and migrations |
| `streamcache` | Download cache that playback can read and seek while it fills |
| `offline` | Pinned downloads |
| `audio` | Decoding, the playback engine, resampling, dither, CoreAudio and cpal (PipeWire, ALSA, WASAPI) output |
| `analysis` | Loudness, dynamic range and fake hi-res detection |

None of the crates are published to crates.io.

Before sending changes:

```sh
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The UI tests can render every screen to PNG without a server, which is how most of the visual work gets checked:

```sh
cargo test -p ond-player --release snapshots -- --ignored --nocapture
# images land in target/snapshots/
```

Some things need real hardware or a real server. [TESTING.md](TESTING.md) lists them.

`main` holds releases and `dev` is where work happens. Pushing a `v*` tag builds the app and publishes a GitHub release.

## AI Disclosure

I built Ond with Claude Opus 5.5 in Claude Code. I came up with the initial idea, did research on the UI design, made all of the design decisions, and tested on my own library and hardware. Claude wrote most of the code. 

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option.
