# Releasing Ond and Rede

The apps release independently from this repository. Release preparation belongs on `dev`. Don't tag until the bundles have been tried.

- **Betas** are tagged on `dev`.
- **Full releases** are tagged on `main`, after fast-forwarding it to the tested `dev` commit. `main` never gets a commit of its own, so it stays the latest full release and every release tag is also on `dev`. The changelog of a beta needs that to find the tag before it.

The release workflow refuses a beta tag that isn't on `dev` and a full release tag that isn't on `main`.

## Versions and notes

- **Ond 0.7.0-beta.2:** workspace version in `Cargo.toml`, including the shared crates. Keep internal path-dependency version requirements in sync. Notes: `.github/release-notes/ond/v0.7.0-beta.2.md`. Tag: `ond-v0.7.0-beta.2`.
- **Rede 0.1.0-beta.4:** separate package version in `apps/rede/Cargo.toml`. Notes: `.github/release-notes/rede/v0.1.0-beta.4.md`. Tag: `rede-v0.1.0-beta.4`.

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

## Update signatures

The publish job installs minisign 0.12 from its upstream Linux archive, checks
its pinned SHA-256, then signs every file in `dist/`. Each `.minisig` is published
beside its archive or dmg. A missing `UPDATE_SIGNING_KEY` produces a workflow
warning and an unsigned release: users can download it manually, but the apps
will refuse automatic installation. A configured key that cannot sign fails
publication. Manual workflow runs do not publish or use the update signing key.

The production public key in `shell::updates::install::PUBLIC_KEY` is:

```text
RWS8BILhZyEyhZhjHWOEkGgfkz6ZofGPH/EdWBRiGwB59QyV943Cn4BP
```

Its key id is `85322167E18204BC`. Keep its existing secret key offline and
backed up. Set repository Actions secrets `UPDATE_SIGNING_KEY` to the **entire
secret key file**, including its comment line, and `UPDATE_SIGNING_PASSWORD`
to its password. The job writes a temporary mode-600 key file, feeds the
password to minisign on stdin, and removes the file on success or failure.
Never paste either secret into a workflow, command argument, log or commit.

To create a successor key, on a trusted offline machine with minisign:

```sh
minisign -G -p update.pub -s update.key
```

Choose a strong password and keep both the secret file and password safely
backed up offline. The second line of `update.pub` is the public constant.
Changing that constant alone would strand existing installations: first ship
a release **signed by the old key** that embeds the successor's public key.
After users have installed that bridge release, change the two repository
secrets to the successor and publish subsequent releases with it. Users who
missed the bridge release must install manually; there is no key manifest or
multi-key trust store. If the current secret is lost or compromised, publish
instructions to install the next release by hand and replace both secrets and
the public constant. Never regenerate the current production key accidentally.

Verify a downloaded release before publishing its announcement:

```sh
minisign -Vm Ond-0.6.0-macos-universal.zip -P RWS8BILhZyEyhZhjHWOEkGgfkz6ZofGPH/EdWBRiGwB59QyV943Cn4BP
```

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

Only after the checks, tag the tested commit. These commands publish releases when pushed.

A beta, on `dev`:

```sh
git switch dev && git pull --ff-only
git tag -a rede-v0.1.0-beta.4 -m "Rede 0.1.0 beta 4"
git push origin rede-v0.1.0-beta.4
```

A full release, on `main` moved up to `dev` (if the fast-forward fails, `main` has a commit `dev` lacks; merge `main` into `dev` and test again):

```sh
git switch main && git pull --ff-only
git merge --ff-only dev
git tag -a ond-v0.6.0 -m "Ond 0.6.0"
git push origin main ond-v0.6.0
```

A version mismatch stops the workflow before building. Each release waits for all of its supported platforms; Rede intentionally skips Windows. Rede's beta is a GitHub prerelease and cannot become the repository's latest stable release. Ond's update checker ignores it.

Check the published names, notes, all assets and their `.minisig` files. Ond should have five archives (macOS dmg/zip, two Linux tarballs, Windows zip); Rede should have four (macOS dmg/zip and two Linux tarballs). Download and launch one published bundle from each supported platform. Don't move a published tag to fix a binary; bump the version and publish another release.
