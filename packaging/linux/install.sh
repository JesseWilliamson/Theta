#!/usr/bin/env sh
# Installs Quarry for the current user (no root needed):
#   ./install.sh              -> ~/.local/{bin,share/applications,share/icons}
#   PREFIX=/usr/local sudo -E ./install.sh
#   ./install.sh uninstall
set -eu

here=$(cd "$(dirname "$0")" && pwd)
prefix=${PREFIX:-$HOME/.local}
bin="$prefix/bin"
apps="$prefix/share/applications"
icons="$prefix/share/icons/hicolor"

if [ "${1:-}" = "uninstall" ]; then
  rm -f "$bin/quarry" "$apps/quarry.desktop"
  find "$icons" -name quarry.png -o -name quarry.svg 2>/dev/null | xargs -r rm -f
  echo "Quarry removed from $prefix"
  exit 0
fi

install -Dm755 "$here/quarry" "$bin/quarry"
sed "s|^Exec=quarry|Exec=$bin/quarry|" "$here/quarry.desktop" > "$here/.quarry.desktop.tmp"
install -Dm644 "$here/.quarry.desktop.tmp" "$apps/quarry.desktop"
rm -f "$here/.quarry.desktop.tmp"
(cd "$here/icons/hicolor" && find . -type f) | while read -r f; do
  install -Dm644 "$here/icons/hicolor/$f" "$icons/$f"
done

command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$apps" >/dev/null 2>&1 || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$icons" >/dev/null 2>&1 || true

echo "Installed Quarry to $prefix"
case ":$PATH:" in *":$bin:"*) ;; *) echo "Note: $bin is not on your PATH." ;; esac
