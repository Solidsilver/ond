#!/usr/bin/env bash
# Builds Rede.app separately from Ond's release packaging.
#
#   scripts/bundle-rede-macos.sh --dev   # release build, ad-hoc app only
#   scripts/bundle-rede-macos.sh         # app, zip and dmg
#   scripts/bundle-rede-macos.sh --universal
#
# Uses REDE_SIGN_IDENTITY / REDE_NOTARY_KEY / REDE_NOTARY_KEY_ID /
# REDE_NOTARY_ISSUER, falling back to Ond's shared signing credentials.
# Dev bundles are always ad-hoc signed and never notarized. macOS must launch
# the bundle (open), rather than cargo run, to route OAuth Apple Events to it.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
dev=false
universal=false
for arg in "$@"; do
  case "$arg" in
    --dev) dev=true ;;
    --universal) universal=true ;;
    -h|--help) sed -n '2,11p' "$0"; exit 0 ;;
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
# Apple requires a numeric marketing version; the archive and About keep beta.
sed -e "s/@VERSION@/${version%%-*}/" -e "s/@BUILD@/$build/" \
    -e "s/@MIN_MACOS@/$MACOSX_DEPLOYMENT_TARGET/" -e "s/@COPYRIGHT@/© $year Rede contributors/" \
    packaging/rede/macos/Info.plist.in > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
identity="${REDE_SIGN_IDENTITY:-${OND_SIGN_IDENTITY:--}}"
notary_key="${REDE_NOTARY_KEY:-${OND_NOTARY_KEY:-}}"
notary_key_id="${REDE_NOTARY_KEY_ID:-${OND_NOTARY_KEY_ID:-}}"
notary_issuer="${REDE_NOTARY_ISSUER:-${OND_NOTARY_ISSUER:-}}"
notarize=false
if $dev; then
  identity=-
elif [[ "$identity" != - && -n "$notary_key" ]]; then
  if [[ -z "$notary_key_id" || -z "$notary_issuer" ]]; then
    echo "Notarization needs a key ID and issuer (REDE_NOTARY_* or OND_NOTARY_*)" >&2
    exit 1
  fi
  notarize=true
fi
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
rm -f "$out/$name.zip" "$out/$name.dmg"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
# hdiutil can report Resource busy on CI even with a fresh staging folder.
for attempt in 1 2 3; do
  if hdiutil create -volname "Rede $version" -srcfolder "$stage" -ov -format UDZO "$out/$name.dmg"; then
    break
  fi
  if [[ $attempt == 3 ]]; then
    echo "hdiutil failed three times" >&2
    exit 1
  fi
  sleep 5
done
if [[ "$identity" != - ]]; then
  codesign --force --timestamp --sign "$identity" "$out/$name.dmg"
fi
if $notarize; then
  xcrun notarytool submit "$out/$name.dmg" \
    --key "$notary_key" --key-id "$notary_key_id" --issuer "$notary_issuer" \
    --wait --timeout 30m
  xcrun stapler staple "$out/$name.dmg"
  xcrun stapler staple "$app"
  spctl --assess --type execute --verbose=2 "$app"
fi
# The zip must contain the app after its notarization ticket is stapled.
ditto -c -k --keepParent "$app" "$out/$name.zip"
echo "built $app, $out/$name.zip and $out/$name.dmg"
