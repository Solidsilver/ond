# 015 · Workspace: release profile and dev dependency opt-level

- Area: `Cargo.toml` · Kind: build · Priority: P2
- Status: open · Claimed by: —
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
