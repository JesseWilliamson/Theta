#!/usr/bin/env python3
"""Turn docs/screenshots/*.png into the site's web images.

For every screenshot this writes, into site/assets/screens/:
  <name>.webp       1600 px wide (retina / large screens)
  <name>-900.webp   900 px wide (phones, narrow columns)
  <name>.png        optimised PNG fallback for browsers without WebP

The page refers to the stable <name>, never to the numbered source file, so re-shooting
a screenshot (same or new size) and re-running this script is the whole swap procedure.
Edit NAMES to add one. Needs Pillow with WebP support.
"""
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "docs" / "screenshots"
DST = ROOT / "site" / "assets" / "screens"

# source file stem -> stable web name
NAMES = {
    "01-editor-results": "editor-results",
    "02-completion-fk-joins": "completion-fk-joins",
    "03-map-stores-zones": "map",
    "04-plan-insights": "plan-insights",
    "05-editable-grid": "editable-grid",
    "06-explorer-suggested-fks-related": "explorer-suggested-fks",
    "07-quarry-light": "theme-light",
    "08-theme-picker": "theme-picker",
    "09-connection-ssh": "connection-ssh",
    "10-vim-visual-block": "vim-visual-block",
    "11-keymap-choice-welcome": "keymap-choice",
}


def main() -> None:
    DST.mkdir(parents=True, exist_ok=True)
    for stem, name in NAMES.items():
        im = Image.open(SRC / f"{stem}.png").convert("RGB")
        w, h = im.size
        im.save(DST / f"{name}.webp", "WEBP", quality=82, method=6)
        small = im.resize((900, round(h * 900 / w)), Image.LANCZOS)
        small.save(DST / f"{name}-900.webp", "WEBP", quality=82, method=6)
        # UI screenshots have few colours; a 256-colour palette is visually lossless here.
        im.quantize(256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE).save(
            DST / f"{name}.png", optimize=True
        )
        sizes = [(DST / f).stat().st_size // 1024 for f in (f"{name}.webp", f"{name}-900.webp", f"{name}.png")]
        print(f"{name}: {w}x{h}  webp {sizes[0]}K  webp-900 {sizes[1]}K  png {sizes[2]}K")


if __name__ == "__main__":
    main()
