#!/usr/bin/env bash
# Builds the app icon files from one square PNG (1024 px or larger, transparent
# corners, artwork on Apple's icon grid):
#
# The tile must be exactly Apple's rounded square, 824 x 824 centred on the
# 1024 canvas, with no shadow of its own. macOS 26 and later shrink any other
# shape onto a grey tile, so the icon looks smaller than its neighbours.
#
#   scripts/make-icns.sh path/to/icon.png
#
# Writes packaging/macos/Ond.icns (the bundle icon) plus icon-1024.png and
# icon-256.png (the 256 is the window icon for `cargo run`). The Linux and
# Windows icons are separate artwork: packaging/linux/hicolor and
# packaging/windows/ond.ico.
set -euo pipefail

src=${1:?usage: scripts/make-icns.sh icon.png}
out=packaging/macos
set_dir=$(mktemp -d)/Ond.iconset
mkdir -p "$set_dir"

for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$src" --out "$set_dir/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" "$src" --out "$set_dir/icon_${size}x${size}@2x.png" >/dev/null
done

iconutil -c icns "$set_dir" -o "$out/Ond.icns"
sips -z 1024 1024 "$src" --out "$out/icon-1024.png" >/dev/null
sips -z 256 256 "$src" --out "$out/icon-256.png" >/dev/null
rm -rf "$(dirname "$set_dir")"
echo "wrote $out/Ond.icns, icon-1024.png, icon-256.png"
