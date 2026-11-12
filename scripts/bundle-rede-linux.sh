#!/usr/bin/env bash
# Builds Rede for Linux as a .tar.gz with its desktop entry and installer.
# Needs ALSA and PipeWire headers, clang, cmake and pkg-config, as Ond does.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

version="$(cargo pkgid -p rede)"
version="${version##*#}"
version="${version##*@}"
if [[ -z "$version" || "$version" == *"#"* ]]; then
  echo "could not read the rede package version from cargo pkgid" >&2
  exit 1
fi
cargo build --release -p rede
name="Rede-$version-linux-$(uname -m)"
out=target/bundle
stage="$out/$name"
rm -rf "$stage"
mkdir -p "$stage"
trap 'rm -rf "$stage"' EXIT
install -m 755 target/release/Rede "$stage/rede"
cp packaging/rede/linux/dev.solidsilver.rede.desktop packaging/rede/linux/dev.solidsilver.rede.metainfo.xml "$stage/"
cp -R packaging/rede/linux/hicolor "$stage/icons"
install -m 755 packaging/linux/install.sh "$stage/install.sh"
cp docs/rede/README.md "$stage/README.md"
cp LICENSE-MIT LICENSE-APACHE "$stage/"
tar -C "$out" -czf "$out/$name.tar.gz" "$name"
echo "built: $out/$name.tar.gz"
