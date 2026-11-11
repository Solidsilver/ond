# Releasing Ond and Rede

The apps release independently from this repository. Release preparation belongs on `dev`; merge the tested changes into `main` before tagging. Don't tag until the bundles have been tried.

## Versions and notes

- **Ond 0.5.0:** workspace version in `Cargo.toml`, including the shared crates. Keep internal path-dependency version requirements in sync. Notes: `.github/release-notes/ond/v0.5.0.md`. Tag: `ond-v0.5.0`.
- **Rede 0.1.0-beta.1:** separate package version in `apps/rede/Cargo.toml`. Notes: `.github/release-notes/rede/v0.1.0-beta.1.md`. Tag: `rede-v0.1.0-beta.1`.

Update `Cargo.lock` with Cargo, not by upgrading every dependency. Check download filenames, documentation links and AppStream metadata. Rede's macOS marketing version is numeric (`0.1.0`); About, archive names and the release tag retain the beta suffix. Its bundle build number is the commit count.

## Automated checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items
python3 scripts/test-release.py
```

Render both apps' UI fixtures when reviewing the shared toolkit:

```sh
cargo test -p ond-player --release snapshots -- --ignored --nocapture
cargo test -p rede --release snapshots -- --ignored --nocapture
```

Use the Release workflow's **Run workflow** button on the candidate branch, once with `app: ond` and once with `app: rede`. These runs upload artifacts without publishing. Ond builds macOS, Linux and Windows; Rede builds macOS and Linux. Linux includes x86_64 and ARM64 on Ubuntu 24.04, requiring glibc 2.39 or newer.

The macOS jobs use the existing `MACOS_CERTIFICATE`, `MACOS_CERTIFICATE_PASSWORD` and `APPLE_NOTARY_*` secrets, as described in [macos-signing.md](macos-signing.md). Rede's script accepts `REDE_SIGN_IDENTITY` / `REDE_NOTARY_*` for local builds and otherwise falls back to `OND_SIGN_IDENTITY` / `OND_NOTARY_*`. Without credentials a build is ad-hoc signed, **not** a notarized release.

## Human checks before the first Rede beta

These are gates, not claims that testing has happened. Record the OS, architecture, server version and results in the pull request. See [TESTING.md](../TESTING.md) for detailed steps.

- [ ] Download the CI artifacts, not just a locally built binary. macOS: install from the dmg and open without a Gatekeeper override; check the app and dmg signatures and stapled tickets. Linux: install, launch from the desktop entry, relaunch and uninstall with the supplied script. Check that uninstall keeps user data and doesn't affect Ond.
- [ ] macOS: password and OpenID sign-in, including a browser redirect while the app is closed. Linux: password sign-in and saved credentials with GNOME Keyring or KWallet. OpenID-only servers must show the password requirement rather than open a flow that cannot return.
- [ ] Single-file m4b and multi-file books: play, seek, cross a file boundary, change speed, pause and resume. Listen with voice boost and silence removal on and off.
- [ ] Sleep timer: duration and end-of-chapter, fade, extend, cancel and resume. Include a hidden/minimized window.
- [ ] Download a book, quit, disconnect the server, relaunch and play. Reconnect and verify final progress, finished status and listening time on ABS. Try progress made on another device before reconnecting.
- [ ] Add/edit/remove a bookmark and check it in ABS. Finish, un-finish and start over; verify the server state.
- [ ] Browse Library, Home, Series, Downloads and Stats in both themes and at minimum window size. Check Player/Mini, shortcuts, idle CPU, window dragging and resizing. On Linux try Wayland and X11 where available.
- [ ] Launch Ond alongside Rede: data, downloads, keyring entries and app icons stay separate. Check Ond's playback, media keys, queue, settings and library against 0.4.4.

Linux beta limitations must stay visible in the release notes: no browser/OpenID sign-in and no MPRIS/media keys. Windows, local folders, podcasts and mixed multi-account browsing are not in this Rede beta.

## Publish

Only after checks and the merge, tag the tested `main` commit. These commands publish releases when pushed:

```sh
git tag -a ond-v0.5.0 -m "Ond 0.5.0"
git tag -a rede-v0.1.0-beta.1 -m "Rede 0.1.0 beta 1"
git push origin ond-v0.5.0 rede-v0.1.0-beta.1
```

A version mismatch stops the workflow before building. Each release waits for all of its supported platforms; Rede intentionally skips Windows. Rede's beta is a GitHub prerelease and cannot become the repository's latest stable release. Ond's update checker ignores it.

Check the published names, notes and all assets. Ond should have five archives (macOS dmg/zip, two Linux tarballs, Windows zip); Rede should have four (macOS dmg/zip and two Linux tarballs). Download and launch one published bundle from each supported platform. Don't move a published tag to fix a binary; bump the version and publish another release.
