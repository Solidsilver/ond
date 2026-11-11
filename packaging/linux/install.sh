#!/bin/sh
# Installs the bundled app for this user (~/.local), or under a prefix:
#
#   ./install.sh                  # ~/.local
#   sudo ./install.sh /usr/local  # everyone
#   ./install.sh --uninstall [prefix]
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
uninstall=false
if [ "${1:-}" = "--uninstall" ]; then
  uninstall=true
  shift
fi
prefix="${1:-$HOME/.local}"
if [ -f "$here/dev.solidsilver.rede.desktop" ]; then
  app=rede
  name=Rede
else
  app=ond
  name=Ond
fi
id="dev.solidsilver.$app"

if $uninstall; then
  rm -f "$prefix/bin/$app" "$prefix/share/applications/$id.desktop" "$prefix/share/metainfo/$id.metainfo.xml"
  rm -f "$prefix"/share/icons/hicolor/*/apps/$id.png
else
  install -Dm755 "$here/$app" "$prefix/bin/$app"
  # The menu entry names the binary by its full path: the desktop's own PATH
  # often lacks ~/.local/bin.
  mkdir -p "$prefix/share/applications"
  exec_path="$(printf '%s' "$prefix/bin/$app" | sed 's/[\\`"$]/\\&/g; s/[\\&|]/\\&/g')"
  sed "s|^Exec=$app$|Exec=\"$exec_path\"|" "$here/$id.desktop" > "$prefix/share/applications/$id.desktop"
  chmod 644 "$prefix/share/applications/$id.desktop"
  install -Dm644 "$here/$id.metainfo.xml" "$prefix/share/metainfo/$id.metainfo.xml"
  for icon in "$here"/icons/*/apps/$id.png; do
    size="$(basename "$(dirname "$(dirname "$icon")")")"
    install -Dm644 "$icon" "$prefix/share/icons/hicolor/$size/apps/$id.png"
  done
fi

# Let the desktop notice the change now rather than at next login.
command -v update-desktop-database >/dev/null && update-desktop-database -q "$prefix/share/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" || true

if $uninstall; then
  echo "removed $name from $prefix (your library and settings are kept)"
else
  echo "installed $name to $prefix"
  case ":$PATH:" in *":$prefix/bin:"*) ;; *) echo "note: $prefix/bin is not on your PATH, so start $name from the app menu (or add it to PATH to run \`$app\`)" ;; esac
fi
