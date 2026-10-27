# 015 · Workspace: release profile and dev dependency opt-level

- Area: `Cargo.toml` · Kind: build · Priority: P2
- Status: done (branch build/profiles)
- Source: lead review.

## Problem

There is no `[profile.release]` and no `[profile.dev.package."*"]` override, although the README says "Debug builds are slow enough to cause dropouts." DSP-heavy dependencies (symphonia, rubato, ebur128, realfft, wgpu/naga) run unoptimized in dev. Release gets default codegen-units=16 and no LTO.

## Fix

```toml
[profile.release]
lto = "thin"          # measure "fat" vs "thin": binary size and CoreAudio callback timing
codegen-units = 1
panic = "unwind"      # keep: decoder panics are converted to errors (decode.rs)

[profile.dev.package."*"]
opt-level = 2
```

Check `scripts/bundle-*.sh` and `flake.nix` still build, and compare the release build time in CI (`.github/workflows/release.yml`).

## Acceptance

`cargo run -p ond-player` (debug) plays a 24/96 FLAC without dropouts on the dev machine; release bundle builds in CI.

## Resolution

Added to the workspace `Cargo.toml`: `[profile.release]` with `lto = "thin"` and `codegen-units = 1`, and `[profile.dev.package."*"] opt-level = 2`. `panic` stays at the default (unwind): `audio/src/decode.rs` and `app/src/analyze.rs` turn panics into errors with `catch_unwind`. No script, Nix or CI file overrides the profiles.

Measured on the M-series dev machine:

| | Before | After |
|---|---|---|
| Release binary (`target/release/Ond`) | 33.8 MB | 27.9 MB |
| Release build of `ond-player`, all crates | not measured cold | 92 s |
| Debug test build after the profile change (dependencies rebuilt once) | — | 82 s |
| `cargo test`: analysis / audio / app suites | 6.6 s / 6.0 s / 10.1 s | 1.1 s / 3.1 s / 3.3 s |

Still to check: the CI release job's time (universal macOS build compiles twice), and `cargo run -p ond-player` (debug) playing a 24/96 FLAC without dropouts — added to TESTING.md.
