# Updates

How Ond and Rede find, fetch and install newer versions of themselves. The checker lives in `shell::updates`; each app wires it to its own settings and About page.

## Finding one

Both apps publish from one repository, so the checker reads the repository's `releases.atom` and keeps the tags that are the app's own (`ond-v`, `v` or `rede-v`). It looks 30 seconds after launch and then once a day, and right away on Check Now.

A user follows one of two channels: Releases or Betas. Until they pick one, a beta build follows Betas and a release build follows Releases. On Releases, someone running a beta is offered the next full release above it, never an older one.

Only copies built by the release workflow look (`OND_DISTRIBUTION` / `REDE_DISTRIBUTION` = `github`). A package manager such as Nix updates its own copies, and a source build has nothing to compare against.

## Installing one

`updates` has three settings: `off`, `notify` (say a newer version is out and link to it) and `install` (also fetch it and install it when the app quits).

### Order of events

1. **Fetch.** A newer version is found on the followed channel and isn't skipped. The app downloads this platform's file from the release, and that file's `.minisig`, into a staging folder in its cache directory. A half-finished download is never installed.
2. **Verify.** The signature is checked against the public key built into the app ([Signing](#signing)). A file that fails is deleted and the failure is shown. The app never installs anything unsigned, and never falls back to installing without the check.
3. **Ready.** Settings › About and the update notice say the update is ready, with Restart to Update.
4. **Install.** On quit, or on Restart to Update, the platform's installer runs. Nothing is replaced while the app is playing. Restart to Update stops playback first, the same way quitting does.
5. **Fall back.** If the app can't install itself where it lives (no write permission, run from a disk image, an unknown layout), it says so and behaves as `notify`. The staged file is removed.

Staged files are deleted once installed, when a newer version replaces them, and when the user skips that version.

### Files

The release files are already named predictably from the tag, so no manifest is needed:

| Platform | File | Installed by |
|---|---|---|
| macOS | `<App>-<version>-macos-universal.zip` | swapping the `.app` bundle |
| Linux | `<App>-<version>-linux-<arch>.tar.gz` | the archive's own `install.sh` |
| Windows (Ond) | `Ond-<version>-windows-x86_64.zip` | swapping `Ond.exe` |

### macOS

The `.zip` holds the signed, notarized, stapled `.app` (`ditto -c -k --keepParent`). To install:

1. Unpack it with `ditto -x -k` next to the staged file.
2. Check it with `codesign --verify --deep --strict`. Its Team ID must match the running app's. An ad-hoc-signed running app (built without the signing secrets) skips the Team ID check but still requires minisign.
3. Start a detached helper (`/bin/sh` is enough) and quit. The helper waits for the app's process to exit, renames the old bundle aside, moves the new one into its place, deletes the old one, and reopens the app with `open`. If the move fails, it puts the old bundle back.

Files fetched by the app itself carry no quarantine attribute, so Gatekeeper doesn't ask again. Two cases fall back to `notify`:

- the bundle is running from a translocated path (opened straight from Downloads or the `.dmg`)
- the bundle's parent folder isn't writable (a standard user and `/Applications`)

### Linux

`install.sh` puts the binary at `<prefix>/bin/<app>` and the menu entry at `<prefix>/share/applications/dev.solidsilver.<app>.desktop`. If the running binary sits in such a layout, the app unpacks the new archive and runs its `install.sh <prefix>` on quit, which also refreshes the menu entry and icons. Otherwise (a binary run where it was unpacked) it replaces the binary alone, using a rename, which is safe while the binary is running. A prefix the user can't write to (`/usr/local`) falls back to `notify`. The app restarts from `<prefix>/bin/<app>`.

### Windows

A running `.exe` can be renamed but not overwritten. The app renames `Ond.exe` to `Ond.exe.old`, moves the new one in, and deletes `Ond.exe.old` on the next launch ([`self-replace`](https://docs.rs/self-replace) does this). A file the app downloads itself carries no Mark of the Web, so SmartScreen doesn't warn again. A folder the user can't write to falls back to `notify`.

## Signing

HTTPS proves a file came from GitHub, not that it came from us: anyone who could publish a release could otherwise run code on every machine set to `install`. So every release file is signed with [minisign](https://jedisct1.github.io/minisign/) (Ed25519):

- The secret key lives in two repository secrets, `UPDATE_SIGNING_KEY` and `UPDATE_SIGNING_PASSWORD`. The publish job signs each file in `dist/` and uploads the `.minisig` files beside them. Without the secret it publishes unsigned, and the apps then won't install that release (they still notify).
- The public key is a constant in `shell::updates`, shared by both apps. The apps verify with [`minisign-verify`](https://docs.rs/minisign-verify).
- To make the key: `minisign -G -p update.pub -s update.key`. Put the contents of `update.key` and its password into the two secrets, and the public key's second line into the constant. Keep `update.key` somewhere safe and offline. To change keys, ship one release signed with the old key that carries the new public key. If the key is lost or leaks, users must install the next version by hand.

## Not yet

- Delta updates: each update downloads the whole file.
- Installing while the app keeps running (macOS and Linux could, Windows couldn't).
- A Windows installer or Linux packages (Flatpak, AppImage).

## Backend and UI handoff

The shared drawing code has not yet changed for installation. Both apps route
`UpdateAction` through `app/updating.rs`. The UI can offer
`UpdateAction::Check(UpdateCheck::Install)` in the existing update control,
`UpdateAction::Restart` when ready, and `UpdateAction::Retry` after a failure.
Restart requests the normal window close; each app's quit handler stops its
playback engine before calling `Updates::install_on_quit`.

Read `Updates::install` (`shell::updates::install::State`):

- `Idle`: no installation requested.
- `Downloading { version, received, total }`: byte progress; total may be absent.
  Progress messages request repaints at most twice per second during transfer,
  plus the start and completion. The UI needs no polling or indefinite spinner.
- `Ready { version, .. }`: verified and unpacked; offer Restart to Update.
- `Failed { version, error }`: typed `install::Error`; format it where shown.
- `NotifyOnly { version, reason }`: keep the release link and show why automatic
  installation cannot run. `Fallback` is `UnsupportedPlatform`, `UnknownLayout`,
  `Translocated`, `DiskImage` or `ReadOnly`. Supply short wording in the UI.

`Updates::install_error` holds startup cleanup errors or an installation failure
reported by a helper from the previous quit. The helper reports rollback
failures with the backup's location so the previous app can be recovered.

Bridges send `UpdateMsg::Install(Event { generation, kind })`; `EventKind` is
`Progress`, `Finished(Result<Prepared, Error>)` or `NotifyOnly(Fallback)`.
`UpdateMsg::InstallError(Error)` reports startup/previous-quit errors. The app
handles these messages without account scoping: releases belong to no account.
`Updates::stage_update` reconciles the download with the effective channel,
setting and skipped version. Generations discard late results; changing the
choice, skipping or retrying aborts the previous worker and disposes its stage.

Stages use cache subdirectories with locks so another app instance's cleanup
cannot remove an active download. The Unix helper owns a stage after launch;
an abandoned helper marker expires after a day. Ownership markers also reclaim
replacement copies beside the app; a failed rollback keeps its backup for
manual recovery. A replacement on another volume
is copied into a temporary directory beside the installed app before Ready,
so the final bundle or binary swap uses a rename. No unsigned archive reaches
the platform preparation or quit path.
