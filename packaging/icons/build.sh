#!/usr/bin/env bash
# Regenerates every icon output from gen_icon.py: the SVGs, the hicolor PNGs, the macOS and Windows
# masters. theta-mid.svg is used for 32 and 48 px, theta-small.svg for 16 px and theta-24.svg for 24 px.
# Needs: python3, node with playwright (Chromium rasterises the SVG exactly), ImageMagick.
#   PLAYWRIGHT_MODULE=/path/to/node_modules/playwright packaging/icons/build.sh
set -euo pipefail
cd "$(dirname "$0")"
python3 gen_icon.py
tmp=$(mktemp -d)
node render.js theta.svg "$tmp" theta-mid.svg theta-small.svg theta-24.svg
for s in 16 24 32 48 64 128 256 512; do
  mkdir -p "hicolor/${s}x${s}/apps"
  cp "$tmp/theta-$s.png" "hicolor/${s}x${s}/apps/theta.png"
done
# The app embeds the glyph-only SVG (currentColor) for the welcome view.
cp theta-mark.svg ../../crates/theta-ui/assets/icons/theta-mark.svg
mkdir -p hicolor/scalable/apps && cp theta.svg hicolor/scalable/apps/theta.svg
cp "$tmp/theta-1024.png" theta-1024.png   # source for the macOS .icns (see packaging/macos)
# macOS does not mask icons (before macOS 26), so the artwork gets the standard ~10% margin.
convert "$tmp/theta-1024.png" -resize 824x824 -background none -gravity center -extent 1024x1024 theta-macos-1024.png
mkdir -p ../windows
convert "$tmp/theta-16.png" "$tmp/theta-24.png" "$tmp/theta-32.png" "$tmp/theta-48.png" \
        "$tmp/theta-64.png" "$tmp/theta-128.png" "$tmp/theta-256.png" ../windows/theta.ico
rm -rf "$tmp"
