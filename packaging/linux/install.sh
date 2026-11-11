#!/usr/bin/env sh
# Installs Theta for the current user (no root needed):
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
  rm -f "$bin/theta" "$apps/theta.desktop"
  find "$icons" -name theta.png -o -name theta.svg 2>/dev/null | xargs -r rm -f
  echo "Theta removed from $prefix"
  exit 0
fi

install -Dm755 "$here/theta" "$bin/theta"
sed "s|^Exec=theta|Exec=$bin/theta|" "$here/theta.desktop" > "$here/.theta.desktop.tmp"
install -Dm644 "$here/.theta.desktop.tmp" "$apps/theta.desktop"
rm -f "$here/.theta.desktop.tmp"
(cd "$here/icons/hicolor" && find . -type f) | while read -r f; do
  install -Dm644 "$here/icons/hicolor/$f" "$icons/$f"
done

command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$apps" >/dev/null 2>&1 || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$icons" >/dev/null 2>&1 || true

echo "Installed Theta to $prefix"
case ":$PATH:" in *":$bin:"*) ;; *) echo "Note: $bin is not on your PATH." ;; esac
