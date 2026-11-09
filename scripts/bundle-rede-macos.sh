#!/usr/bin/env bash
# Builds Rede.app separately from Ond's release packaging.
#
#   scripts/bundle-rede-macos.sh --dev   # release build, ad-hoc app only
#   scripts/bundle-rede-macos.sh         # app, zip and dmg
#   scripts/bundle-rede-macos.sh --universal
#
# REDE_SIGN_IDENTITY optionally selects a Developer ID Application identity.
# Dev bundles are always ad-hoc signed. macOS must launch the bundle (open),
# rather than cargo run, to route OAuth Apple Events to it.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
dev=false
universal=false
for arg in "$@"; do
  case "$arg" in
    --dev) dev=true ;;
    --universal) universal=true ;;
    -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done
export MACOSX_DEPLOYMENT_TARGET=11.0
version="$(cargo pkgid -p rede)"
version="${version##*#}"
version="${version##*@}"
if [[ -z "$version" || "$version" == *"/"* ]]; then
  echo "could not read the rede package version from cargo pkgid" >&2
  exit 1
fi
build="$(git rev-list --count HEAD)"
year="$(date +%Y)"
out=target/bundle
mkdir -p "$out"
if $universal; then
  for arch_target in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --release -p rede --target "$arch_target"
  done
  binary="$out/rede-universal"
  lipo -create -output "$binary" target/aarch64-apple-darwin/release/Rede target/x86_64-apple-darwin/release/Rede
  arch=universal
else
  cargo build --release -p rede
  binary=target/release/Rede
  arch="$(uname -m)"
fi
app="$out/Rede.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/Rede"
cp packaging/rede/macos/Rede.icns "$app/Contents/Resources/Rede.icns"
sed -e "s/@VERSION@/$version/" -e "s/@BUILD@/$build/" \
    -e "s/@MIN_MACOS@/$MACOSX_DEPLOYMENT_TARGET/" -e "s/@COPYRIGHT@/© $year Rede contributors/" \
    packaging/rede/macos/Info.plist.in > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
identity="${REDE_SIGN_IDENTITY:--}"
if $dev; then identity=-; fi
if [[ "$identity" == - ]]; then
  codesign --force --sign - "$app"
else
  codesign --force --options runtime --timestamp --sign "$identity" "$app"
fi
codesign --verify --strict "$app"
if $dev; then
  echo "built $app; launch with: open $app"
  exit 0
fi
name="Rede-$version-macos-$arch"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
hdiutil create -volname "Rede $version" -srcfolder "$stage" -ov -format UDZO "$out/$name.dmg"
ditto -c -k --keepParent "$app" "$out/$name.zip"
echo "built $app, $out/$name.zip and $out/$name.dmg"
