#!/usr/bin/env bash
# Builds theta-<version>-linux-x86_64.tar.gz and, when appimagetool is available, the AppImage.
#   packaging/linux/package.sh <theta-binary> <version> <out-dir>
set -euo pipefail
bin=$1 version=$2 out=$3
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
name="theta-$version-linux-x86_64"
mkdir -p "$out"
stage=$(mktemp -d)/$name
mkdir -p "$stage/icons"

install -m755 "$bin" "$stage/theta"
install -m644 "$here/theta.desktop" "$stage/theta.desktop"
install -m755 "$here/install.sh" "$stage/install.sh"
cp -r "$root/packaging/icons/hicolor" "$stage/icons/hicolor"
cp "$root/README.md" "$root/CHANGELOG.md" "$stage/"
cp "$root/docs/THIRD_PARTY.md" "$root/crates/theta-ui/NOTICE" "$stage/"
for f in LICENSE LICENSE-APACHE LICENSE-MIT; do [ -f "$root/$f" ] && cp "$root/$f" "$stage/"; done

tar -C "$(dirname "$stage")" -czf "$out/$name.tar.gz" "$name"
echo "$out/$name.tar.gz"

# AppImage: the binary loads libvulkan, libxkbcommon, libwayland and fontconfig from the host at
# run time, so nothing is bundled; it is the same program as the tarball in a single file.
if [ -n "${APPIMAGETOOL:-}" ]; then
  appdir=$(mktemp -d)/Theta.AppDir
  mkdir -p "$appdir/usr/bin" "$appdir/usr/share/icons/hicolor" "$appdir/usr/share/applications"
  install -m755 "$bin" "$appdir/usr/bin/theta"
  cp -r "$root/packaging/icons/hicolor/." "$appdir/usr/share/icons/hicolor/"
  install -m644 "$here/theta.desktop" "$appdir/theta.desktop"
  install -m644 "$here/theta.desktop" "$appdir/usr/share/applications/theta.desktop"
  cp "$root/packaging/icons/hicolor/256x256/apps/theta.png" "$appdir/theta.png"
  cp "$root/packaging/icons/hicolor/256x256/apps/theta.png" "$appdir/.DirIcon"
  cat > "$appdir/AppRun" <<'RUN'
#!/bin/sh
exec "$(dirname "$(readlink -f "$0")")/usr/bin/theta" "$@"
RUN
  chmod +x "$appdir/AppRun"
  ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$APPIMAGETOOL" --no-appstream "$appdir" "$out/Theta-$version-linux-x86_64.AppImage"
  echo "$out/Theta-$version-linux-x86_64.AppImage"
fi
