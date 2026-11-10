#!/usr/bin/env bash
# Builds quarry-<version>-linux-x86_64.tar.gz and, when appimagetool is available, the AppImage.
#   packaging/linux/package.sh <quarry-binary> <version> <out-dir>
set -euo pipefail
bin=$1 version=$2 out=$3
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
name="quarry-$version-linux-x86_64"
mkdir -p "$out"
stage=$(mktemp -d)/$name
mkdir -p "$stage/icons"

install -m755 "$bin" "$stage/quarry"
install -m644 "$here/quarry.desktop" "$stage/quarry.desktop"
install -m755 "$here/install.sh" "$stage/install.sh"
cp -r "$root/packaging/icons/hicolor" "$stage/icons/hicolor"
cp "$root/README.md" "$root/CHANGELOG.md" "$stage/"
cp "$root/docs/THIRD_PARTY.md" "$root/crates/quarry-ui/NOTICE" "$stage/"
for f in LICENSE LICENSE-APACHE LICENSE-MIT; do [ -f "$root/$f" ] && cp "$root/$f" "$stage/"; done

tar -C "$(dirname "$stage")" -czf "$out/$name.tar.gz" "$name"
echo "$out/$name.tar.gz"

# AppImage: the binary loads libvulkan, libxkbcommon, libwayland and fontconfig from the host at
# run time, so nothing is bundled; it is the same program as the tarball in a single file.
if [ -n "${APPIMAGETOOL:-}" ]; then
  appdir=$(mktemp -d)/Quarry.AppDir
  mkdir -p "$appdir/usr/bin" "$appdir/usr/share/icons/hicolor" "$appdir/usr/share/applications"
  install -m755 "$bin" "$appdir/usr/bin/quarry"
  cp -r "$root/packaging/icons/hicolor/." "$appdir/usr/share/icons/hicolor/"
  install -m644 "$here/quarry.desktop" "$appdir/quarry.desktop"
  install -m644 "$here/quarry.desktop" "$appdir/usr/share/applications/quarry.desktop"
  cp "$root/packaging/icons/hicolor/256x256/apps/quarry.png" "$appdir/quarry.png"
  cp "$root/packaging/icons/hicolor/256x256/apps/quarry.png" "$appdir/.DirIcon"
  cat > "$appdir/AppRun" <<'RUN'
#!/bin/sh
exec "$(dirname "$(readlink -f "$0")")/usr/bin/quarry" "$@"
RUN
  chmod +x "$appdir/AppRun"
  ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$APPIMAGETOOL" --no-appstream "$appdir" "$out/Quarry-$version-linux-x86_64.AppImage"
  echo "$out/Quarry-$version-linux-x86_64.AppImage"
fi
