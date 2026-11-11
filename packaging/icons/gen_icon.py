#!/usr/bin/env python3
"""Generates the Theta icon SVGs (run from anywhere; writes next to this file, stdlib only).

The mark is the lowercase Greek letter theta: a tall, upright, high-contrast oval (thick sides,
hairline top and bottom) with a thin crossbar that runs the full inner width, joins both sides and sits
a little above the centre. White on a quiet dark grey gradient tile. Theta is the name of the generalised
join of relational algebra (the theta join), and the product's signature feature is suggesting joins.

The letterform at 64 px and above IS the theta (U+03B8) of the typeface GFS Didot, (c) 2007 Greek Font
Society, SIL Open Font License 1.1: its outline is embedded below as path data, so the build does not
need the font installed. See GLYPH_D for the licence notes and docs/THIRD_PARTY.md for the credit.

Outputs
  theta.svg        full-detail 512 tile, used for every size >= 64 px
  theta-mid.svg    tile drawn on the 32 px pixel grid, used for 32 and 48 px
  theta-small.svg  tile drawn on the 16 px pixel grid, used for 16 px
  theta-24.svg     tile drawn on the 24 px pixel grid, used for 24 px
  theta-light.svg  dark ink on a light grey gradient, for docs
  theta-mark.svg   glyph only, `currentColor`, tight square viewBox (embedded in the app and, via
                   site/tools/mark.py, the website)

Why several drawings: the real glyph has a hairline top and bottom (3% of the letter's height) and a
bar 5% thick. At 512 px that is the whole charm; at 32 px it is a fraction of a pixel and vanishes,
leaving an open ellipse that is no longer recognisably a theta. The small drawings therefore keep the
design (oval, thick sides, a full-width bar joined to both sides, a little above centre) but are built
from parts that can be thickened: a heavier top and bottom, and a bar a whole pixel thick, with every edge
on a whole pixel of the size they are used at, so they never rely on anti-aliasing to stay legible. The
in-app mark (about 44 px, `currentColor`) gets its own medium-weight drawing for the same reason.

Geometry notes
  * Everything is a FILLED shape, nonzero fill rule (the SVG default; no drawing may rely on even-odd,
    because the bar overlaps the walls of the oval). The font's outline already winds correctly; the
    parametric oval is the outer ellipse clockwise and the inner one counter-clockwise, and the bar is a
    clockwise rectangle, so where it overlaps the wall the winding number is 2 and stays filled.
  * Drawings are specified in their own units (`u` tile-pixels each, origin at the glyph centre) and
    placed with one transform, so a 16 px drawing is written in whole pixels.
"""
import os
import re

TILE_RX = 115        # 22.4% of 512, the usual macOS-style squircle approximation
SIZE = 512

# Tile backgrounds. Lighter top-left to darker bottom-right; the red/green/blue spread stays
# within a few units so it reads as neutral grey, not as blue-black.
DARK_BG = dict(stops=[(0, "#3b3d42"), (1, "#18191b")])
LIGHT_BG = dict(stops=[(0, "#f4f4f5"), (1, "#d3d4d7")])
LIGHT_INK = "#1b1c1f"


def fmt(v):
    s = f"{v:.2f}".rstrip("0").rstrip(".")
    return "0" if s == "-0" else s


# The theta of GFS Didot Regular (U+03B8, glyph "theta"), outline in font units (1000 per em, y up),
# extracted from GFSDidot.otf with fontTools. Bounding box (59, -10)-(466, 710): 407 x 720 units.
# Copyright (c) 2007 Greek Font Society (www.greekfontsociety.org), with Reserved Font Name 'GFS Didot'.
# SIL Open Font License 1.1 (docs/THIRD_PARTY.md, crates/theta-ui/assets/fonts/GFSDidot-OFL.txt). The licence allows
# embedding and modifying the font in software, provided the notice and licence travel with it and no
# modified version uses the reserved name; this file is neither sold as a font nor named "GFS Didot".
GLYPH_D = ("M466 361C466 442 452 515 424 580C387 667 335 710 266 710C189 710 132 665 97 575C72 511 59 433 59 341"
           "C59 250 72 174 97 115C132 32 188 -10 266 -10C340 -10 394 36 429 129C454 194 466 272 466 361Z"
           "M154 418C158 479 166 532 178 576C197 650 226 687 264 687C311 687 343 651 360 579C368 550 374 495 378 416Z"
           "M381 339C381 276 374 213 359 152C338 60 307 14 266 14C215 14 182 50 165 121C155 159 150 214 150 287"
           "V382H381Z")
GLYPH_BOX = (59, -10, 466, 710)


def glyph_path(cx, cy, height):
    """GLYPH_D scaled so the letter is `height` tall, flipped to y-down and centred by its bounding
    box on (cx, cy). Absolute M/C/V/H/Z only, which is all the outline uses."""
    x0, y0, x1, y1 = GLYPH_BOX
    k = height / (y1 - y0)
    mx, my = (x0 + x1) / 2, (y0 + y1) / 2

    def X(v):
        return fmt(cx + (v - mx) * k)

    def Y(v):
        return fmt(cy - (v - my) * k)

    out, cur = [], [0.0, 0.0]
    for cmd, args in re.findall(r"([MCVHZ])([^MCVHZ]*)", GLYPH_D):
        n = [float(v) for v in args.replace("-", " -").split()]
        if cmd in "MC":
            pts = [(n[i], n[i + 1]) for i in range(0, len(n), 2)]
            out.append(cmd + " ".join(f"{X(x)} {Y(y)}" for x, y in pts))
            cur = list(pts[-1])
        elif cmd == "V":
            cur[1] = n[0]
            out.append(f"L{X(cur[0])} {Y(cur[1])}")
        elif cmd == "H":
            cur[0] = n[0]
            out.append(f"L{X(cur[0])} {Y(cur[1])}")
        else:
            out.append("Z")
    return "".join(out)


def oval(cx, cy, rx, ry, side, top, bar_h, rise):
    """The parametric letter: an oval plus a full-width bar.

    Outer ellipse rx x ry, inner counter (rx - side) x (ry - top): thick sides and thin top/bottom are
    what make it a Didot-style letter rather than a ring. The bar is a rectangle of half thickness
    `bar_h` centred `rise` above the middle (a real theta carries its bar a little high); it runs
    to the middle of each wall so there is no seam where it joins.
    """
    def ellipse(a, b, sweep):
        return (f"M{fmt(cx - a)} {fmt(cy)}A{fmt(a)} {fmt(b)} 0 1 {sweep} {fmt(cx + a)} {fmt(cy)}"
                f"A{fmt(a)} {fmt(b)} 0 1 {sweep} {fmt(cx - a)} {fmt(cy)}Z")

    a, by = rx - side / 2, cy - rise
    bar = f"M{fmt(cx - a)} {fmt(by - bar_h)}H{fmt(cx + a)}V{fmt(by + bar_h)}H{fmt(cx - a)}Z"
    return ellipse(rx, ry, 1) + ellipse(rx - side, ry - top, 0) + bar


def shapes(D, cx=0.0, cy=0.0, ink="currentColor"):
    """The letter of drawing D as one filled path, centred on (cx, cy) in the drawing's own units."""
    if "glyph" in D:
        d = glyph_path(cx, cy, D["height"])
    else:
        d = oval(cx, cy, D["rx"], D["ry"], D["side"], D["top"], D["h"], D["rise"])
    return f'<path fill="{ink}" d="{d}"/>'


def glyph_group(D, ink):
    """The glyph placed on the tile: origin at the glyph centre, `u` tile pixels per drawing unit."""
    tx, ty = D.get("at", (SIZE / 2, SIZE / 2))
    return f'<g transform="translate({fmt(tx)} {fmt(ty)}) scale({fmt(D["u"])})">{shapes(D, ink=ink)}</g>'


def defs(bg):
    st = "".join(f'<stop offset="{o}" stop-color="{c}"/>' for o, c in bg["stops"])
    grad = f'<linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">{st}</linearGradient>'
    # 1 px inner highlight along the top edge only; fades out within the top quarter
    hi = ('<linearGradient id="hi" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fff" stop-opacity="0.16"/>'
          '<stop offset="0.12" stop-color="#fff" stop-opacity="0"/></linearGradient>')
    return f"<defs>{grad}{hi}</defs>"


def svg_tile(D, bg, ink="#FFFFFF", highlight=True, title="Theta"):
    hl = (f'<rect x="0.5" y="0.5" width="511" height="511" rx="{TILE_RX - 0.5}" fill="none" stroke="url(#hi)"/>'
          if highlight else "")
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512">\n'
            f'  <title>{title}</title>\n  {defs(bg)}\n'
            f'  <rect width="512" height="512" rx="{TILE_RX}" fill="url(#bg)"/>\n  {hl}\n'
            f'  {glyph_group(D, ink)}\n</svg>\n')


def svg_mark(D, pad=2):
    """Glyph only: a tight square viewBox, so `Icon::size(44)` is 44 px of mark.

    Coordinates are written directly in the viewBox (no transform) so that consumers such as
    site/tools/mark.py can read the paths without interpreting transforms.
    """
    side = 2 * D["ry"] + 2 * pad
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {fmt(side)} {fmt(side)}">\n'
            f'  <title>Theta</title>\n  {shapes(D, side / 2, side / 2)}\n</svg>\n')


# ---- the drawings --------------------------------------------------------------------------------
# FULL: the GFS Didot theta, 61% of the tile's height (the previous icon's ring filled 54%), centred by
# its bounding box. The glyph's own crossbar sits 7% of its height above the middle of that box.
FULL = dict(u=1, glyph=True, height=0.608 * SIZE)

# In-app mark, 16-48 px in the text colour: the same letter as the parametric oval at medium weight.
# Units are those of the viewBox (the letter is 43 x 76): sides 10 and top/bottom 5 survive as 2.5+ px at
# 44 px, and the bar is 4.2 thick (about 2.3 px).
MARK = dict(rx=21.5, ry=38, side=10, top=5, h=2.1, rise=3.5)


def grid(px, at, **kw):
    """A drawing in whole pixels of a `px` pixel icon, centred at `at` (pixels)."""
    u = SIZE / px
    return dict(u=u, at=(at[0] * u, at[1] * u), **kw)


# Each grid drawing was chosen by rendering variants at the real size and enlarged. What differs from
# the glyph, and why:
#   * sides 3.5 px at 32 and 2.5 px at 16-24 (the glyph has 22% of the width): an ellipse's side stroke
#     narrows quickly away from its widest point, so anything lighter shows as a grey double line;
#   * top/bottom 2-2.5 px at 24-32 and 1.5 px at 16 instead of 3% of the height, which would be gone;
#   * the bar is 1 px thick (the glyph's is 5% of the height: 1 px at 20 px tall), spans the whole counter
#     and lies exactly on a pixel row, one to two rows above the centre line.
# 32 px grid (also used at 48): oval 14 x 20 px, counter 7 x 15; bar on row 14.
MID = grid(32, (16, 16), rx=7, ry=10, side=3.5, top=2.5, h=0.5, rise=1.5)
# 24 px grid: oval 10 x 16 px, counter 5 x 12; bar on row 10.
SMALL24 = grid(24, (12, 12), rx=5, ry=8, side=2.5, top=2, h=0.5, rise=1.5)
# 16 px grid: oval 10 x 12 px, counter 5 x 9; bar on row 7, one row above the centre line.
SMALL = grid(16, (8, 8), rx=5, ry=6, side=2.5, top=1.5, h=0.5, rise=0.5)


def main():
    here = os.path.dirname(os.path.abspath(__file__))

    def w(name, text):
        with open(os.path.join(here, name), "w") as f:
            f.write(text)

    w("theta.svg", svg_tile(FULL, DARK_BG))
    w("theta-mid.svg", svg_tile(MID, DARK_BG))
    w("theta-small.svg", svg_tile(SMALL, DARK_BG, highlight=False))
    w("theta-24.svg", svg_tile(SMALL24, DARK_BG, highlight=False))
    w("theta-light.svg", svg_tile(FULL, LIGHT_BG, LIGHT_INK, highlight=False))
    w("theta-mark.svg", svg_mark(MARK))


if __name__ == "__main__":
    main()
