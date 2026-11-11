#!/usr/bin/env python3
"""Generates the Theta icon SVGs (run from anywhere; writes next to this file, stdlib only).

The mark is the lowercase Greek letter theta, drawn as geometry rather than typeset: a tall, upright,
high-contrast oval in the manner of a Didot face (thick sides, hairline top and bottom) with a short
crossbar that floats inside it, touching nothing. The bar has soft bracketed serifs: the thin bar
curves smoothly up into small serif ends, so there is no hard corner anywhere. It is white on a quiet
dark grey gradient tile. Theta is the name of the generalised join of relational algebra (the theta
join), and the product's signature feature is suggesting joins. The detached bar keeps the letter from
reading as a zero with a slash.

Outputs
  theta.svg        full-detail 512 tile, used for every size >= 64 px
  theta-mid.svg    tile drawn on the 32 px pixel grid, used for 32 and 48 px
  theta-small.svg  tile drawn on the 16 px pixel grid, used for 16 px
  theta-24.svg     tile drawn on the 24 px pixel grid, used for 24 px
  theta-light.svg  dark ink on a light grey gradient, for docs
  theta-mark.svg   glyph only, `currentColor`, tight square viewBox (embedded in the app and, via
                   site/tools/mark.py, the website)

Why several drawings: the approved drawing has a 2% hairline at the top and bottom of the bowl and a
1% bar waist. At 512 px that is the whole charm; at 32 px it is a fraction of a pixel and vanishes,
leaving an open ellipse with a floating dash. The small drawings therefore keep the design (oval,
thick sides, detached bar with serifs) but thicken the parts that cannot be resolved and put every edge
on a whole pixel of the size they are used at, so they never rely on anti-aliasing to stay legible.
The in-app mark (about 44 px, `currentColor`) gets its own medium-weight drawing for the same reason.

Geometry notes
  * Everything is a FILLED shape. The bowl is one path: the outer ellipse clockwise and the inner
    one counter-clockwise, so it is a ring under the nonzero rule as well as even-odd and does not
    depend on how a renderer treats `fill-rule`.
  * The bar is cubic Béziers (a polyline would show facets when the 512 px tile is enlarged). A thin
    round-joined outline in the same colour rounds off the corners where the bracket meets the end cap.
  * Drawings are specified in their own units (`u` tile-pixels each, origin at the glyph centre) and
    placed with one transform, so a 16 px drawing is written in whole pixels.
"""
import os

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


def bowl(cx, cy, rx, ry, side, top):
    """The oval: outer ellipse rx x ry, inner counter (rx - side) x (ry - top).

    Thick sides and thin top/bottom are what make it a Didot-style letter rather than a ring.
    """
    irx, iry = rx - side, ry - top

    def ellipse(a, b, sweep):
        return (f"M{fmt(cx - a)} {fmt(cy)}A{fmt(a)} {fmt(b)} 0 1 {sweep} {fmt(cx + a)} {fmt(cy)}"
                f"A{fmt(a)} {fmt(b)} 0 1 {sweep} {fmt(cx - a)} {fmt(cy)}Z")

    return ellipse(rx, ry, 1) + ellipse(irx, iry, 0)


def bracket(cx, cy, L, h, s, b, e):
    """A bar with bracketed serifs, centred on (cx, cy).

    L  half length          h  half thickness of the flat run (the waist)
    s  half height of the serif ends
    b  length over which the bracket curves from the waist up to the serif
    e  how far the end cap bulges (slightly convex ends look softer than cut ones)
    Each flank is one cubic from the end cap's corner to the start of the flat run; the control
    points hug the end first and the run second, which makes the curve leave the end steeply and
    arrive on the run tangentially: the "bracket".
    """
    k = (s - h) * 0.12
    x0, x1 = cx - L, cx + L
    up, dn = cy - h, cy + h
    return (f"M{fmt(x0)} {fmt(cy - s)}"
            f"C{fmt(x0 + b * .2)} {fmt(up - k)} {fmt(x0 + b * .55)} {fmt(up)} {fmt(x0 + b)} {fmt(up)}"
            f"L{fmt(x1 - b)} {fmt(up)}"
            f"C{fmt(x1 - b * .55)} {fmt(up)} {fmt(x1 - b * .2)} {fmt(up - k)} {fmt(x1)} {fmt(cy - s)}"
            f"Q{fmt(x1 + e)} {fmt(cy)} {fmt(x1)} {fmt(cy + s)}"
            f"C{fmt(x1 - b * .2)} {fmt(dn + k)} {fmt(x1 - b * .55)} {fmt(dn)} {fmt(x1 - b)} {fmt(dn)}"
            f"L{fmt(x0 + b)} {fmt(dn)}"
            f"C{fmt(x0 + b * .55)} {fmt(dn)} {fmt(x0 + b * .2)} {fmt(dn + k)} {fmt(x0)} {fmt(cy + s)}"
            f"Q{fmt(x0 - e)} {fmt(cy)} {fmt(x0)} {fmt(cy - s)}Z")


def shapes(D, cx=0.0, cy=0.0, ink="currentColor"):
    """Bowl and bar of drawing D, centred on (cx, cy) in the drawing's own units."""
    rise = D["rise"]  # real thetas carry the crossbar a hair above the centre
    bar = bracket(cx, cy - rise, D["L"], D["h"], D["s"], D["b"], D["e"])
    ol = D["outline"]
    stroke = f' stroke="{ink}" stroke-width="{fmt(ol)}" stroke-linejoin="round"' if ol else ""
    return (f'<path fill="{ink}" d="{bowl(cx, cy, D["rx"], D["ry"], D["side"], D["top"])}"/>'
            f'<path fill="{ink}"{stroke} d="{bar}"/>')


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
    side = max(2 * D["rx"], 2 * D["ry"]) + 2 * pad
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {fmt(side)} {fmt(side)}">\n'
            f'  <title>Theta</title>\n  {shapes(D, side / 2, side / 2)}\n</svg>\n')


# ---- the drawings --------------------------------------------------------------------------------
# FULL is the approved drawing (units of a 100 unit tile): bowl 23 x 38 with 7.4 sides and 2.0
# top/bottom, bar bracket(L=10.5, h=1.0, s=3.6, b=5, e=0.9) with a 0.7 outline. Two refinements: it is
# drawn at 80% so the letter fills about 61% of the tile's height (the previous icon's ring filled
# 54%; the approved 76% crowded the tile edge), and the bar sits 1.25% of the tile above centre.
FULL = dict(u=5.12 * 0.8, rx=23, ry=38, side=7.4, top=2.0, L=10.5, h=1.0, s=3.6, b=5, e=0.9, outline=0.7,
            rise=1.25 / 0.8)

# In-app mark, 16-48 px in the text colour: the same oval and bar, medium weight. Sides 9 and
# top/bottom 4.6 of 76 survive as 2+ px at 44 px; the waist is 2.4 units (~1.3 px).
MARK = dict(rx=23, ry=38, side=9.2, top=4.6, L=9, h=1.4, s=4.4, b=4.4, e=1.0, outline=0.8, rise=1.6)


def grid(px, at, **kw):
    """A drawing in whole pixels of a `px` pixel icon, centred at `at` (pixels). Outline is off:
    a fractional-pixel outline is exactly the blur the grid drawings exist to avoid."""
    u = SIZE / px
    return dict(u=u, at=(at[0] * u, at[1] * u), outline=0, **kw)


# Each grid drawing was chosen by rendering variants at the real size and enlarged. What had to change
# against the approved proportions, and why:
#   * sides 3-3.5 px at 32 and 2-2.5 px at 16-24 (the design has 16% of the width): an ellipse's
#     side stroke narrows quickly away from its widest point, so anything lighter shows as a grey
#     double line;
#   * top/bottom 2-2.5 px at 24-32 and 1.5 px at 16 instead of the 2% hairline, which would be gone;
#   * the bar keeps a 1 px waist (the thinnest thing that is still solid white) and, where there is room,
#     3-4 px tall serif ends. The bar always leaves a clear 1 px column to each side wall, so it
#     cannot read as a Phi or as a bar fused to the bowl.
# 32 px grid (also used at 48): oval 14 x 20 px, counter 7 x 15.
MID = grid(32, (16, 16), rx=7, ry=10, side=3.5, top=2.5, L=2.5, h=0.75, s=2, b=1.2, e=0.3, rise=0.5)
# 24 px grid: oval 10 x 16 px, counter 5 x 12; bar 3 px long.
SMALL24 = grid(24, (12, 12), rx=5, ry=8, side=2.5, top=2, L=1.5, h=0.5, s=1.5, b=0.8, e=0.2, rise=0.5)
# 16 px grid: oval 10 x 12 px, counter 5 x 9; a plain 3 x 1 bar: serifs cannot be resolved here.
SMALL = grid(16, (8, 8), rx=5, ry=6, side=2.5, top=1.5, L=1.5, h=0.5, s=0.5, b=0.5, e=0, rise=0.5)


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
