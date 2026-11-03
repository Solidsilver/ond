#!/usr/bin/env bash
# Builds Ond.app and packages it as a .zip and a .dmg in target/bundle.
#
#   scripts/bundle-macos.sh              # host architecture
#   scripts/bundle-macos.sh --universal  # arm64 + x86_64 in one binary
#
# Needs cargo and the Xcode command line tools (lipo, ditto, hdiutil).
#
# Signing: with OND_SIGN_IDENTITY set (a Developer ID Application identity in
# the keychain, e.g. "Developer ID Application: Name (TEAMID)"), the app and
# the .dmg get a Developer ID signature with the hardened runtime. With
# OND_NOTARY_KEY (path to an App Store Connect API key, .p8),
# OND_NOTARY_KEY_ID and OND_NOTARY_ISSUER also set, the .dmg goes to Apple's
# notary service and both it and the app get the ticket stapled. Without
# OND_SIGN_IDENTITY the app gets an ad-hoc signature and macOS blocks its
# first launch.
set -euo pipefail

MIN_MACOS="11.0"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

universal=false
for arg in "$@"; do
  case "$arg" in
    --universal) universal=true ;;
    -h|--help) sed -n '2,17p' "$0"; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

# "path+file:///…/crates/app#ond-player@0.1.0" → "0.1.0"
version="$(cargo pkgid -p ond-player)"
version="${version##*@}"
if [[ -z "$version" || "$version" == *"#"* ]]; then
  echo "could not read the ond-player package version from cargo pkgid" >&2
  exit 1
fi
build="$(git rev-list --count HEAD 2>/dev/null || echo 1)"
year="$(date +%Y)"

export MACOSX_DEPLOYMENT_TARGET="$MIN_MACOS"
out="target/bundle"
mkdir -p "$out"

if $universal; then
  targets=(aarch64-apple-darwin x86_64-apple-darwin)
  installed="$(rustup target list --installed 2>/dev/null || true)"
  for t in "${targets[@]}"; do
    if ! grep -qx "$t" <<<"$installed"; then
      echo "missing Rust target $t; install it with: rustup target add $t" >&2
      exit 1
    fi
  done
  for t in "${targets[@]}"; do
    cargo build --release -p ond-player --target "$t"
  done
  binary="$out/ond-universal"
  lipo -create -output "$binary" \
    target/aarch64-apple-darwin/release/Ond \
    target/x86_64-apple-darwin/release/Ond
  arch="universal"
else
  cargo build --release -p ond-player
  binary="target/release/Ond"
  arch="$(uname -m)"
fi

app="$out/Ond.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/Ond"
cp packaging/macos/Ond.icns "$app/Contents/Resources/Ond.icns"
sed -e "s/@VERSION@/$version/" \
    -e "s/@BUILD@/$build/" \
    -e "s/@MIN_MACOS@/$MIN_MACOS/" \
    -e "s/@COPYRIGHT@/© $year Ond contributors/" \
    packaging/macos/Info.plist.in > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" >/dev/null
identity="${OND_SIGN_IDENTITY:-}"
notarize=false
if [[ -n "$identity" && -n "${OND_NOTARY_KEY:-}" ]]; then
  if [[ -z "${OND_NOTARY_KEY_ID:-}" || -z "${OND_NOTARY_ISSUER:-}" ]]; then
    echo "OND_NOTARY_KEY needs OND_NOTARY_KEY_ID and OND_NOTARY_ISSUER" >&2
    exit 1
  fi
  notarize=true
fi
if [[ -n "$identity" ]]; then
  # The hardened runtime and a secure timestamp are what notarization asks for.
  codesign --force --options runtime --timestamp --sign "$identity" "$app"
  codesign --verify --strict --verbose=2 "$app"
else
  # An ad-hoc signature keeps Apple Silicon happy and lets macOS track the app
  # consistently; it is not a Developer ID signature.
  codesign --force --sign - "$app" >/dev/null 2>&1 || echo "note: ad-hoc codesign failed; continuing unsigned" >&2
fi

name="Ond-$version-macos-$arch"
rm -f "$out/$name.zip" "$out/$name.dmg"

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
# hdiutil sometimes fails with "Resource busy" on CI machines; a retry clears it.
for attempt in 1 2 3; do
  if hdiutil create -volname "Ond $version" -srcfolder "$stage" -ov -format UDZO "$out/$name.dmg"; then
    break
  fi
  if [[ $attempt == 3 ]]; then
    echo "hdiutil failed three times" >&2
    exit 1
  fi
  sleep 5
done

if [[ -n "$identity" ]]; then
  codesign --force --timestamp --sign "$identity" "$out/$name.dmg"
fi
if $notarize; then
  # Notarizing the .dmg covers the app inside it, so one submission gives
  # tickets for both.
  xcrun notarytool submit "$out/$name.dmg" \
    --key "$OND_NOTARY_KEY" --key-id "$OND_NOTARY_KEY_ID" --issuer "$OND_NOTARY_ISSUER" \
    --wait --timeout 30m
  xcrun stapler staple "$out/$name.dmg"
  xcrun stapler staple "$app"
  spctl --assess --type execute --verbose=2 "$app"
fi
# Zipped last, so the zip's app carries the stapled ticket.
ditto -c -k --keepParent "$app" "$out/$name.zip"

echo "built:"
du -sh "$app" "$out/$name.zip" "$out/$name.dmg"
