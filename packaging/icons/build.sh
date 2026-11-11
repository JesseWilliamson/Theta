#!/usr/bin/env bash
# Regenerates every icon output from gen_icon.py: the SVGs, the hicolor PNGs, the macOS and Windows
# masters. quarry-small.svg is used for the 16 and 24 px renders.
# Needs: python3, node with playwright (Chromium rasterises the SVG exactly), ImageMagick.
#   PLAYWRIGHT_MODULE=/path/to/node_modules/playwright packaging/icons/build.sh
set -euo pipefail
cd "$(dirname "$0")"
python3 gen_icon.py
tmp=$(mktemp -d)
node render.js quarry.svg "$tmp" quarry-small.svg
for s in 16 24 32 48 64 128 256 512; do
  mkdir -p "hicolor/${s}x${s}/apps"
  cp "$tmp/quarry-$s.png" "hicolor/${s}x${s}/apps/quarry.png"
done
# The app embeds the glyph-only SVG (currentColor) for the welcome view.
cp quarry-mark.svg ../../crates/quarry-ui/assets/icons/quarry-mark.svg
mkdir -p hicolor/scalable/apps && cp quarry.svg hicolor/scalable/apps/quarry.svg
cp "$tmp/quarry-1024.png" quarry-1024.png   # source for the macOS .icns (see packaging/macos)
# macOS does not mask icons (before macOS 26), so the artwork gets the standard ~10% margin.
convert "$tmp/quarry-1024.png" -resize 824x824 -background none -gravity center -extent 1024x1024 quarry-macos-1024.png
mkdir -p ../windows
convert "$tmp/quarry-16.png" "$tmp/quarry-24.png" "$tmp/quarry-32.png" "$tmp/quarry-48.png" \
        "$tmp/quarry-64.png" "$tmp/quarry-128.png" "$tmp/quarry-256.png" ../windows/quarry.ico
rm -rf "$tmp"
