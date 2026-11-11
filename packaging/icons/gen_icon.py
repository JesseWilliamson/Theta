#!/usr/bin/env python3
"""Generates the Quarry icon SVGs (run from anywhere; writes next to this file, stdlib only).

The mark is a "Q" built from an isometric cube outline: a hexagonal ring (the block seen
corner-on) crossed by a 45 degree tail. It is one flat white glyph on a near-black tile, no
gradients, no second colour.

Outputs
  quarry.svg        full-art 512 tile (rounded square), used for every size >= 32 px
  quarry-small.svg  same tile with the heavier 16/24 px glyph (build.sh uses it for <= 24 px)
  quarry-light.svg  dark glyph on a white tile, for docs
  quarry-mark.svg   glyph only, `currentColor`, tight square viewBox (embedded in the app)

Geometry notes
  * The hexagon uses 2:1 "pixel-art" isometric slopes instead of 30 degrees, so at 16 and 32 px
    every vertex of the outer outline lands on a whole pixel: the 512 canvas is 32 units per
    16-px pixel and 16 units per 32-px pixel, and the outer coordinates are multiples of 16.
"""
import math
import os

INK = "#FFFFFF"      # glyph on the dark tile
FIELD = "#111113"    # tile
LIGHT_FIELD = "#FFFFFF"
LIGHT_INK = "#111113"
TILE_RX = 115        # 22.4% of 512, the usual macOS-style squircle approximation
C = 256


def add(p, q): return (p[0] + q[0], p[1] + q[1])
def sub(p, q): return (p[0] - q[0], p[1] - q[1])
def mul(p, k): return (p[0] * k, p[1] * k)
def unit(p): n = math.hypot(*p); return (p[0] / n, p[1] / n)


def line_x(p, d, q, e):
    """Intersection of the lines p + s*d and q + u*e."""
    det = d[0] * -e[1] - d[1] * -e[0]
    s = ((q[0] - p[0]) * -e[1] - (q[1] - p[1]) * -e[0]) / det
    return add(p, mul(d, s))


def inset(poly, t):
    """Offset a convex clockwise (screen coords) polygon inwards by t along every edge normal."""
    n = len(poly)
    lines = []
    for i in range(n):
        a, b = poly[i], poly[(i + 1) % n]
        e = unit(sub(b, a))
        inward = (-e[1], e[0])           # clockwise on screen: the interior is on the left of e
        lines.append((add(a, mul(inward, t)), e))
    return [line_x(*lines[i - 1], *lines[i]) for i in range(n)]


def glyph(a, r, dy, t, tail_from, tail_to, tail_w):
    """Return the Q as polygons for one nonzero-fill path: ring outline, hole, tail bar.

    a: half width, r: half height, dy: height of the roof slope (a/2 for 2:1),
    t: wall thickness, tail_*: the 45-degree bar through the centre, measured along its axis.
    The ring and the tail are both clockwise and the hole is reversed, so nonzero filling
    gives ring + tail with a clean hole; no boolean geometry (and no seams) needed.
    """
    outer = [(0, -r), (a, -r + dy), (a, r - dy), (0, r), (-a, r - dy), (-a, -r + dy)]
    hole = inset(outer, t)[::-1]
    d = (math.sqrt(0.5), math.sqrt(0.5))
    e = (-d[1], d[0])
    h = tail_w / 2
    tail = [add(mul(d, tail_from), mul(e, -h)), add(mul(d, tail_to), mul(e, -h)),
            add(mul(d, tail_to), mul(e, h)), add(mul(d, tail_from), mul(e, h))]
    # clockwise on screen: the cross product of consecutive edges must be positive
    if (tail[1][0] - tail[0][0]) * (tail[2][1] - tail[1][1]) - (tail[1][1] - tail[0][1]) * (tail[2][0] - tail[1][0]) < 0:
        tail.reverse()
    return [outer, hole, tail]


def inside(poly, x, y):
    n, c = len(poly), False
    for k in range(n):
        (x0, y0), (x1, y1) = poly[k], poly[(k + 1) % n]
        if (y0 > y) != (y1 > y) and x < x0 + (y - y0) * (x1 - x0) / (y1 - y0):
            c = not c
    return c


def optical_offset(polys, snap=16):
    """Translation that centres the glyph optically, snapped to the pixel grid.

    The tail sticks out of the lower-right of the ring, so the bounding box is not
    symmetric. Centring on the bounding box alone would shift the heavy ring up-left of
    the tile's centre; centring on the ink's centre of mass alone would crop the padding
    unevenly. The average of the two is the usual compromise. `snap` keeps vertices on whole
    pixels after the move: 16 units for the full glyph (32 px), 32 for the small one (16 px).
    """
    outer, hole, tail = polys
    xs = [p[0] for q in polys for p in q]; ys = [p[1] for q in polys for p in q]
    bx, by = (min(xs) + max(xs)) / 2, (min(ys) + max(ys)) / 2
    n = sx = sy = 0
    for y in range(-256, 256, 2):
        for x in range(-256, 256, 2):
            if (inside(outer, x, y) and not inside(hole, x, y)) or inside(tail, x, y):
                n += 1; sx += x; sy += y
    mx, my = sx / n, sy / n
    off = (-(bx + mx) / 2, -(by + my) / 2)
    return tuple(round(v / snap) * snap for v in off)


def path_d(polys, off=(0, 0)):
    out = []
    for poly in polys:
        out.append("M" + " L".join(f"{fmt(x + C + off[0])} {fmt(y + C + off[1])}" for x, y in poly) + "Z")
    return " ".join(out)


def fmt(v):
    s = f"{v:.2f}".rstrip("0").rstrip(".")
    return "0" if s == "-0" else s


# Full glyph: a 256 x 320 hexagon (50% x 62.5% of the tile) plus the tail bulge.
FULL = dict(a=128, r=160, dy=64, t=48, tail_from=40, tail_to=186, tail_w=48)
# Small glyph for 16/24 px: wider, heavier walls (2 px at 16 px), a 2 px tail.
SMALL = dict(a=128, r=160, dy=64, t=64, tail_from=56, tail_to=190, tail_w=64)


def svg_tile(d, field, ink, title="Quarry"):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512">\n'
            f'  <title>{title}</title>\n'
            f'  <rect width="512" height="512" rx="{TILE_RX}" fill="{field}"/>\n'
            f'  <path d="{d}" fill="{ink}"/>\n</svg>\n')


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    def w(name, text): open(os.path.join(here, name), "w").write(text)

    polys = glyph(**FULL)
    off = optical_offset(polys)
    d = path_d(polys, off)
    w("quarry.svg", svg_tile(d, FIELD, INK))
    w("quarry-light.svg", svg_tile(d, LIGHT_FIELD, LIGHT_INK))

    small = glyph(**SMALL)
    w("quarry-small.svg", svg_tile(path_d(small, optical_offset(small, snap=32)), FIELD, INK))

    # Glyph only, for in-app use: a square viewBox just big enough for the glyph plus a hair of
    # air, so `Icon::size(34)` means 34 px of mark rather than 34 px of mostly padding.
    xs = [p[0] + C + off[0] for q in polys for p in q]; ys = [p[1] + C + off[1] for q in polys for p in q]
    side = max(max(xs) - min(xs), max(ys) - min(ys)) + 16
    vx, vy = (min(xs) + max(xs)) / 2 - side / 2, (min(ys) + max(ys)) / 2 - side / 2
    w("quarry-mark.svg",
      f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{fmt(vx)} {fmt(vy)} {fmt(side)} {fmt(side)}" fill="currentColor">\n'
      f'  <title>Quarry</title>\n  <path d="{d}"/>\n</svg>\n')


if __name__ == "__main__":
    main()
