#!/usr/bin/env bash
# Builds Quarry.app and Quarry-<version>-macos-<arch>.zip (macOS only: sips, iconutil, codesign, ditto).
#   packaging/macos/bundle.sh <quarry-binary> <version> <arch> <out-dir>
set -euo pipefail
bin=$1 version=$2 arch=$3 out=$4
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
mkdir -p "$out"
stage=$(mktemp -d)   # the bare .app is not an output; only the zip is
app="$stage/Quarry.app"

mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/quarry"
chmod +x "$app/Contents/MacOS/quarry"
# CFBundleShortVersionString / CFBundleVersion must be numeric (1-3 dot-separated integers):
# 0.1.0-alpha.1 -> 0.1.0. The full string stays in the zip name and in the plist's
# QuarryVersion key.
numeric=${version%%-*}
sed -e "s/@VERSION@/$numeric/g" -e "s/@FULL_VERSION@/$version/g" "$here/Info.plist.in" > "$app/Contents/Info.plist"

# .icns from the 1024px master (itself rendered from icons/quarry.svg by icons/build.sh).
set_dir=$(mktemp -d)/Quarry.iconset && mkdir -p "$set_dir"
src="$root/packaging/icons/quarry-macos-1024.png"
for s in 16 32 128 256 512; do
  sips -z $s $s "$src" --out "$set_dir/icon_${s}x${s}.png" >/dev/null
  sips -z $((s * 2)) $((s * 2)) "$src" --out "$set_dir/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$set_dir" -o "$app/Contents/Resources/Quarry.icns"

cp "$root/README.md" "$app/Contents/Resources/README.md"

# Ad-hoc signature: gives the bundle a stable identity (required for arm64 to run at all) but is
# not notarised, so Gatekeeper still asks on first launch (see docs/USER_GUIDE.md).
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"

zip="$out/Quarry-$version-macos-$arch.zip"
rm -f "$zip"
ditto -c -k --sequesterRsrc --keepParent "$app" "$zip"
echo "$zip"
