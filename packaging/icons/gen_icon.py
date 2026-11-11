#!/usr/bin/env python3
"""Generates the Theta icon SVGs (run from anywhere; writes next to this file, stdlib only).

The mark is the lowercase Greek letter theta, drawn as geometry rather than typeset: a ring with a
short horizontal bar inside it, as thin white line art on a quiet dark grey gradient tile. Theta is the
name of the generalised join of relational algebra (the theta join), and the product's signature
feature is suggesting joins. The bar is short and floats inside the ring (it touches nothing), which
keeps the letter from reading as a zero with a slash.

Outputs
  theta.svg        full-detail 512 tile, used for every size >= 64 px
  theta-mid.svg    tile drawn on the 32 px pixel grid, used for 32 and 48 px
  theta-small.svg  tile drawn on the 16 px pixel grid, used for 16 px
  theta-24.svg     tile drawn on the 24 px pixel grid, used for 24 px
  theta-light.svg  dark line on a light grey gradient, for docs
  theta-mark.svg   glyph only, `currentColor`, tight square viewBox (embedded in the app)

Why three drawings: a stroke that is elegant at 512 px (about 2.7% of the tile) is a smear at
16 px. The mid and small drawings therefore keep the proportions of the letter but place every edge
on a whole pixel of the size they are used at, so they never rely on anti-aliasing to stay legible
and the counters above and below the bar stay open.

Geometry notes
  * All coordinates are in a 512 unit tile; the glyph is centred on the tile.
  * Skew (italic) is applied to the points, not as an SVG transform, so the stroke stays uniform.
  * The ring is a polyline of many short segments: it keeps one code path for true ellipses,
    superellipses and skewed rings, and 0.03 px of sagitta at 512 px is invisible.
"""
import math
import os

TILE_RX = 115        # 22.4% of 512, the usual macOS-style squircle approximation
C = 256

INK = "#FFFFFF"
INK_OPACITY = 0.92

# Tile backgrounds. Lighter top-left to darker bottom-right; the red/green/blue spread stays
# within a few units so it reads as neutral grey, not as blue-black.
DARK_BG = dict(kind="diag", stops=[(0, "#3b3d42"), (1, "#18191b")])
LIGHT_BG = dict(kind="diag", stops=[(0, "#f4f4f5"), (1, "#d3d4d7")])
LIGHT_INK = "#1b1c1f"


def fmt(v):
    s = f"{v:.2f}".rstrip("0").rstrip(".")
    return "0" if s == "-0" else s


# Glyph parameters (centre of the ring at the origin, y down, units of the 512 tile).
#   rx, ry   centre-line radii of the ring
#   n        superellipse exponent (2 = true ellipse)
#   slant    italic angle in degrees (top leans right)
#   sw       stroke width; sw_y: stroke at the top and bottom of the ring (calligraphic contrast)
#   bar      "inner" ends on the ring's centre line, "over" pokes out by `over`,
#            "short" stops `gap` short of the inner edge, "cursor" runs on right by `over`,
#            "frac" is `bar_len` (a fraction) of the ring's inner width, centred, so a gap is left
#            on both sides; "len" is `bar_len` in tile units, centred. Both measure the visible
#            length, round caps included
#   bar_y    bar centre relative to the ring centre (negative = above)
#   bar_w    bar stroke width (default sw)
#   cap      "butt" or "round" (bar ends)
#   arc_gap  half-angle in degrees removed from the ring on both sides at the bar (two arcs)
BASE = dict(rx=96, ry=132, n=2.0, slant=0, sw=14, sw_y=None, bar="inner", over=0, gap=0, bar_y=0,
            bar_w=None, cap="butt", arc_gap=0, bar_len=0)


def ring_pts(P, a0=0.0, a1=360.0, steps=None):
    """Centre line of the ring from angle a0 to a1 (degrees, 0 = right, counter-clockwise on screen)."""
    rx, ry, n = P["rx"], P["ry"], P["n"]
    steps = steps or max(8, int(abs(a1 - a0) / 2.5))
    out = []
    for i in range(steps + 1):
        t = math.radians(a0 + (a1 - a0) * i / steps)
        c, s = math.cos(t), math.sin(t)
        x = rx * math.copysign(abs(c) ** (2 / n), c)
        y = -ry * math.copysign(abs(s) ** (2 / n), s)
        out.append((x, y))
    return out


def skew(P, pts):
    k = math.tan(math.radians(P["slant"]))
    return [(x - y * k, y) for x, y in pts]


def half_width_at(P, y):
    """Half width of the ring's centre line at height y."""
    r = min(abs(y) / P["ry"], 1.0)
    return P["rx"] * (1 - r ** P["n"]) ** (1 / P["n"])


def path(pts, close=False):
    d = "M" + " L".join(f"{fmt(x)} {fmt(y)}" for x, y in pts)
    return d + ("Z" if close else "")


def glyph_shapes(P):
    """(ring paths, bar path, bar width, ring stroke width, ring filled?) in glyph coordinates."""
    sw = P["sw"]
    contrast = P["sw_y"] is not None and P["sw_y"] != sw
    ring = []
    if contrast:
        # Filled outline between an outer and an inner superellipse: thick at the sides, thin
        # at the top and bottom, as a broad-nib pen would draw it.
        o = dict(P, rx=P["rx"] + sw / 2, ry=P["ry"] + P["sw_y"] / 2)
        i = dict(P, rx=P["rx"] - sw / 2, ry=P["ry"] - P["sw_y"] / 2)
        ring.append(path(skew(P, ring_pts(o, 0, 360, 144)), True) + " " +
                    path(skew(P, ring_pts(i, 360, 0, 144)), True))
    elif P["arc_gap"] > 0:
        g = P["arc_gap"]
        ring.append(path(skew(P, ring_pts(P, g, 180 - g))))
        ring.append(path(skew(P, ring_pts(P, 180 + g, 360 - g))))
    else:
        ring.append(path(skew(P, ring_pts(P, 0, 360, 144)), True))
    by = P["bar_y"]
    w = half_width_at(P, by)
    bw = P["bar_w"] or sw
    mode = P["bar"]
    inner = w - sw / 2
    if mode == "inner":
        xl, xr = -w, w
    elif mode == "over":
        xl, xr = -w - P["over"], w + P["over"]
    elif mode == "short":
        xl, xr = -inner + P["gap"], inner - P["gap"]
    elif mode == "cursor":
        xl, xr = -w, w + P["over"]
    elif mode in ("frac", "len"):
        length = P["bar_len"] * 2 * inner if mode == "frac" else P["bar_len"]
        # A round cap sticks out by half the stroke past the end of the path.
        half = (length - (bw if P["cap"] == "round" else 0)) / 2
        xl, xr = -half, half
    else:
        raise ValueError(mode)
    bar = path(skew(P, [(xl, by), (xr, by)]))
    return ring, bar, bw, contrast


def bbox(P):
    pts = skew(P, ring_pts(P, 0, 360, 144))
    xs = [x for x, _ in pts]
    ys = [y for _, y in pts]
    _, bar, _, _ = glyph_shapes(P)
    bx = [float(v) for v in bar.replace("M", "").replace("L", " ").split()][0::2]
    half = max(P["sw"], P["sw_y"] or 0) / 2
    return (min(xs + bx) - half, min(ys) - half, max(xs + bx) + half, max(ys) + half)


def glyph_group(P, color, opacity=None, at=None, extra=""):
    """The glyph as one SVG <g>, centred on `at` (default the tile centre) by its bounding box."""
    x0, y0, x1, y1 = bbox(P)
    cx, cy = at or (C, C)
    tx, ty = cx - (x0 + x1) / 2, cy - (y0 + y1) / 2
    ring, bar, bw, contrast = glyph_shapes(P)
    cap = P["cap"]
    sw = P["sw"]
    op = f' opacity="{opacity}"' if opacity is not None else ""
    parts = []
    if contrast:
        parts.append(f'<path d="{ring[0]}" fill="{color}" fill-rule="evenodd" stroke="none"/>')
    else:
        parts += [f'<path d="{d}"/>' for d in ring]
    parts.append(f'<path d="{bar}" stroke-width="{fmt(bw)}"/>')
    ringcap = "round" if P["arc_gap"] > 0 and cap == "round" else "butt"
    return (f'<g transform="translate({fmt(tx)} {fmt(ty)})" fill="none" stroke="{color}" stroke-width="{fmt(sw)}" '
            f'stroke-linejoin="round" stroke-linecap="{cap}"{op}{extra}>' + "".join(parts) + "</g>")


def defs(bg, uid="bg"):
    st = "".join(f'<stop offset="{o}" stop-color="{c}"/>' for o, c in bg["stops"])
    grad = f'<linearGradient id="{uid}" x1="0" y1="0" x2="1" y2="1">{st}</linearGradient>'
    # 1 px inner highlight along the top edge only; fades out within the top quarter
    hi = ('<linearGradient id="hi" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fff" stop-opacity="0.16"/>'
          '<stop offset="0.12" stop-color="#fff" stop-opacity="0"/></linearGradient>')
    return f"<defs>{grad}{hi}</defs>"


def svg_tile(P, bg, ink, ink_opacity=INK_OPACITY, highlight=True, title="Theta"):
    hl = (f'<rect x="0.5" y="0.5" width="511" height="511" rx="{TILE_RX - 0.5}" fill="none" stroke="url(#hi)"/>'
          if highlight else "")
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512">\n'
            f'  <title>{title}</title>\n  {defs(bg)}\n'
            f'  <rect width="512" height="512" rx="{TILE_RX}" fill="url(#bg)"/>\n  {hl}\n'
            f'  {glyph_group(P, ink, ink_opacity)}\n</svg>\n')


def svg_mark(P, pad=6):
    """Glyph only: a tight square viewBox, so `Icon::size(44)` is 44 px of mark. Strokes only."""
    x0, y0, x1, y1 = bbox(P)
    side = max(x1 - x0, y1 - y0) + 2 * pad
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {fmt(side)} {fmt(side)}">\n'
            f'  <title>Theta</title>\n  {glyph_group(P, "currentColor", at=(side / 2, side / 2))}\n</svg>\n')


# ---- the chosen drawings ------------------------------------------------------------------------
# Upright, because the small drawings must sit on whole pixels and a skewed ring cannot. The ring is
# 0.77 as wide as it is tall. The bar is deliberately short: 55% of the ring's inner width, centred
# and a hair above the middle, with a clear gap to the ring on both sides. Spanning the whole gap
# made the letter read as a Phi-like "O with a line"; with air around it the eye reads the bar as
# the theta's crossbar. It is a little lighter than the ring (13 against 15) and has butt ends, so
# it cannot be mistaken for the dash of a no-entry sign.
FULL = dict(BASE, rx=98, ry=130, sw=15, bar_w=13, bar="frac", bar_len=0.55, bar_y=-5)
# Heavier, uniform strokes for 24-48 px in-app use; strokes only (no fills) so the renderer's lack of
# fill-opacity support cannot matter.
MARK = dict(BASE, rx=98, ry=130, sw=20, bar_w=17, bar="frac", bar_len=0.55, bar_y=-6)


def on_grid(px, outer_w, outer_h, stroke, bar_len, bar_w=None, bar_y=0):
    """A drawing whose every edge lies on a whole pixel of a `px` pixel icon.

    All sizes are in pixels. The ring is centred on the tile, `bar_len` is the visible bar length
    and `bar_y` (negative = up) moves its centre from the ring's centre; choose them so the bar
    covers whole pixel rows and leaves whole pixels of gap to the ring on both sides.
    """
    u = 512 / px
    bw = bar_w or stroke
    return dict(BASE, rx=(outer_w - stroke) / 2 * u, ry=(outer_h - stroke) / 2 * u, sw=stroke * u,
                bar="len", bar_len=bar_len * u, bar_w=bw * u, bar_y=bar_y * u)


# 32 px grid (also used at 48): 14 x 18 px ring (the full icon's 0.77 proportion), 2 px stroke, inner
# 10 x 14. The bar is 6 px long (60%), 2 px gaps either side and 6 px counters above and below.
MID = on_grid(32, 14, 18, 2, 6)
# 16 px grid: 10 x 12 px ring, 2 px stroke, inner 6 x 8. The bar is 4 px long and 1 px thick with 1 px
# gaps to the ring, and sits on the row that leaves 3 px above and 4 below (the large drawing's bar is
# also a hair high and lighter than the ring). An 8 x 10 ring would leave only 4 px inside, where a bar
# and two gaps do not fit; a 2 px bar in this counter looks like a block rather than a crossbar.
SMALL = on_grid(16, 10, 12, 2, 4, 1, -0.5)
# 24 px grid: 12 x 16 px ring, 2 px stroke, inner 8 x 12: 4 px bar, 2 px gaps, 5 px counters.
SMALL24 = on_grid(24, 12, 16, 2, 4)


def main():
    here = os.path.dirname(os.path.abspath(__file__))

    def w(name, text):
        with open(os.path.join(here, name), "w") as f:
            f.write(text)

    w("theta.svg", svg_tile(FULL, DARK_BG, INK))
    w("theta-mid.svg", svg_tile(MID, DARK_BG, "#FFFFFF", 1))
    w("theta-small.svg", svg_tile(SMALL, DARK_BG, "#FFFFFF", 1, highlight=False))
    w("theta-24.svg", svg_tile(SMALL24, DARK_BG, "#FFFFFF", 1, highlight=False))
    w("theta-light.svg", svg_tile(FULL, LIGHT_BG, LIGHT_INK, 0.9, highlight=False))
    w("theta-mark.svg", svg_mark(MARK))


if __name__ == "__main__":
    main()
