# Contributing to Ond

Thanks for looking. Ond is a small project with one maintainer, so the process is light; the conventions matter more than the ceremony.

## Before you start

- Build and run it: the [README](README.md#build-from-source) has the toolchain and system packages per platform. `nix develop` gives you everything on Linux and macOS.
- Read [AGENTS.md](AGENTS.md). It's written for coding agents but it's the shortest accurate description of how the code is organised, what each crate owns, and the invariants that aren't enforced by the compiler.
- For anything bigger than a bug fix, open an issue first and say what you have in mind. Ond is a tool-first player (see the README's "What it does"); features that don't fit that get a no, politely, and an issue saves you the work.

## Branches and commits

`dev` is where work happens and `main` holds the latest release. Branch from `dev` and open your pull request against `dev`.

Commits: a subject of at most 72 characters, a blank line, then a body that says why. Commit in logical units; a rename or module move goes in its own commit so `git blame` survives it. Don't add AI attribution trailers; AI use is disclosed in the README.

## Before you open a pull request

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items
```

CI runs the same on macOS, Linux and Windows (the docs build on Linux only), plus a Nix build on Linux. If your change touches audio output, device handling, the offline store's files, or anything in [TESTING.md](TESTING.md), say which checklist items you ran and on what hardware. Some can only be run by the maintainer; that's fine, just say so.

For UI changes, render the screens: `cargo test -p ond-player --release snapshots -- --ignored --nocapture` writes PNGs to `target/snapshots/`. Attach the ones that changed.

## Tests

Every crate has a `tests.rs` beside its code. Tests check behaviour, not implementation: the network crates spin up real sockets, the engine runs against a fake backend, the library hand-builds old-format databases and checks its indexes, the UI tests drive a headless egui. A new behaviour gets a test with a sentence for a name; a bug fix gets a test that fails without the fix.

## Using an AI assistant

Fine, and most of Ond was written that way. The result is held to the same bar as anything else: you've read it, it has tests, the checks pass, and the pull request explains the change in your own words. Point your assistant at AGENTS.md.

## Releases

The maintainer merges `dev` into `main`, bumps the workspace version, writes `.github/release-notes/ond/vX.Y.Z.md`, and pushes an `ond-vX.Y.Z` tag. The release workflow builds and signs the bundles and publishes them as "Ond X.Y.Z".

Each app in the repository has its own tags and notes, because Ond and Rede release separately from the same repository:

- Ond: `ond-v0.5.0`, notes in `.github/release-notes/ond/`. Releases before 0.5 were tagged `v0.4.4`, and a `v*` tag still means Ond.
- Rede: `rede-v0.1.0`, notes in `.github/release-notes/rede/`. Rede has no bundles yet, so its tags build nothing.

The workflow builds only the app a tag names, and stops if the tag's version isn't that app's crate version. Each app's update check reads the repository's releases feed and looks only at its own tags, so a release of one never shows up as an update to the other. The feed lists the latest ten or so releases, so a long run of one app's releases can hide the other's newest until its next one.

## License

By contributing you agree your work is licensed under the project's terms, MIT or Apache 2.0 at the user's option.
