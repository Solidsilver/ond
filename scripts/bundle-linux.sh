#!/usr/bin/env bash
# Builds Ond for Linux and packages it as a .tar.gz in target/bundle:
#
#   scripts/bundle-linux.sh
#
# The archive holds the `ond` binary, the desktop entry, AppStream metadata,
# the icons and an install.sh that puts them under ~/.local (or a prefix).
# Building needs libasound2-dev, libpipewire-0.3-dev, clang and pkg-config
# (Debian/Ubuntu names; see the README for others).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# "path+file:///…/crates/app#ond-player@0.1.0" → "0.1.0"
version="$(cargo pkgid -p ond-player)"
version="${version##*@}"
if [[ -z "$version" || "$version" == *"#"* ]]; then
  echo "could not read the ond-player package version from cargo pkgid" >&2
  exit 1
fi

cargo build --release -p ond-player
arch="$(uname -m)"
name="Ond-$version-linux-$arch"
out="target/bundle"
stage="$out/$name"
rm -rf "$stage"
mkdir -p "$stage"

install -m 755 target/release/Ond "$stage/ond"
cp packaging/linux/dev.solidsilver.ond.desktop packaging/linux/dev.solidsilver.ond.metainfo.xml "$stage/"
cp -R packaging/linux/hicolor "$stage/icons"
install -m 755 packaging/linux/install.sh "$stage/install.sh"
cp README.md LICENSE-MIT LICENSE-APACHE "$stage/"

tar -C "$out" -czf "$out/$name.tar.gz" "$name"
rm -rf "$stage"
echo "built:"
du -sh "$out/$name.tar.gz"
