# Rede beta

Rede is a native audiobook player for [Audiobookshelf](https://www.audiobookshelf.org), built with Rust and egui. It shares Ond's audio engine and interface toolkit, with its own library and controls for books.

**0.1.0-beta.1 is the first public beta, for macOS and Linux.** Expect rough edges. Keep your Audiobookshelf backups: Rede syncs listening progress, finished status and bookmarks with your server.

## What works

- Home, a searchable library, book and series pages, and listening statistics.
- Resume, chapters, configurable skips, and pitch-preserving playback speed.
- A sleep timer, voice boost and silence removal.
- Bookmarks and picking up progress from another device.
- Downloads, listening offline, and sending offline progress and listening sessions when the server returns.
- Full, Player and Mini windows, light and dark themes, interface zoom and keyboard shortcuts (⌘/ on macOS, Ctrl+/ on Linux).
- macOS media keys and Control Center.

## Install

Download from the [Rede 0.1.0-beta.1 release](https://github.com/Solidsilver/ond/releases/tag/rede-v0.1.0-beta.1). Rede betas are marked as prereleases; the repository's latest stable release is Ond, the music player.

### macOS

Open `Rede-0.1.0-beta.1-macos-universal.dmg` and drag Rede to Applications. The universal app runs on Apple silicon and Intel, on macOS 11 or later. A zip is also available.

Release builds use the same Developer ID signing and Apple notarization setup as Ond. Locally built bundles are ad-hoc signed unless signing credentials are supplied.

### Linux

Choose `Rede-0.1.0-beta.1-linux-x86_64.tar.gz` for an Intel/AMD PC, or `Rede-0.1.0-beta.1-linux-aarch64.tar.gz` for ARM64.

```sh
tar xf Rede-0.1.0-beta.1-linux-*.tar.gz
cd Rede-0.1.0-beta.1-linux-*/
./install.sh                 # installs under ~/.local
# or: sudo ./install.sh /usr/local
```

Start Rede from your app menu, or run `~/.local/bin/rede`. To remove it, run `./install.sh --uninstall` from the unpacked archive (add the prefix if you used one). Your settings, library and downloads are kept.

The bundles need glibc 2.39 or newer (Ubuntu 24.04, Fedora 40, Debian 13), PipeWire's library (`libpipewire-0.3`), and a working graphics driver. Tokens are stored in a Secret Service keyring such as GNOME Keyring or KWallet; it must be running and unlocked.

**Linux beta limitations:** sign in with an Audiobookshelf username and password. Browser/OpenID sign-in and system media controls (MPRIS/media keys) are not supported on Linux yet. Rede's own controls and keyboard shortcuts work while its window has focus.

## Connect

Enter your Audiobookshelf server address, then sign in. Audiobookshelf 2.26 or newer is required; 2.36 or newer is recommended. Password sign-in works on both platforms; macOS also supports browser/OpenID sign-in.

For OpenID, allow `rede://oauth` in your server's mobile redirect URIs. Rede can fall back to `audiobookshelf://oauth`, but macOS may ask which app should open that shared scheme. Launch the installed app bundle, not `cargo run`, to receive browser redirects.

There is one signed-in account at a time. Local audiobook folders, podcasts, Windows bundles and a mixed multi-account library are not part of this beta. Output follows your system's selected device; choose another device in system settings. Rede does not check for updates yet; download later betas from the releases page.

## Build

From the repository root, use a current stable Rust toolchain. On Linux, install ALSA and PipeWire headers, clang, cmake and pkg-config (`sudo apt install libasound2-dev libpipewire-0.3-dev clang cmake pkg-config` on Ubuntu). macOS needs the Xcode command line tools.

```sh
cargo run -p rede --release
scripts/bundle-rede-macos.sh --universal  # app, dmg and zip
scripts/bundle-rede-linux.sh             # tar.gz for this machine's architecture
```

Use a release build for listening. For a development macOS bundle with working browser redirects, use `scripts/bundle-rede-macos.sh --dev`, then `open target/bundle/Rede.app`.

## Data and bug reports

Rede keeps its data separate from Ond:

| What | macOS | Linux |
| --- | --- | --- |
| Tokens | Keychain | Secret Service keyring |
| Settings and account databases | `~/Library/Application Support/dev.solidsilver.rede/` | `~/.config/rede/` (settings), `~/.local/share/rede/` (databases) |
| Caches | `~/Library/Caches/dev.solidsilver.rede/` | `~/.cache/rede/` |
| Logs | `~/Library/Logs/Rede/rede.log` | `~/.local/state/rede/logs/rede.log` |

Downloads use the folder shown in Settings › Downloads.

Report problems at [GitHub Issues](https://github.com/Solidsilver/ond/issues), with the Rede version, OS and architecture, Audiobookshelf version, and steps to reproduce. For playback problems, include the audio format and whether the book has one file or several. Never post passwords, tokens or browser redirect URLs; review logs before sharing them.

The repository's [TESTING.md](../../TESTING.md) lists the checks that need real servers, speakers and desktop sessions. The [design](design.md) describes the longer-term plan, not just what ships in this beta.
