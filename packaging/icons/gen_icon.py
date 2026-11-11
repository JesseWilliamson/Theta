#!/usr/bin/env python3
"""Generates the Theta icon SVGs (run from anywhere; writes next to this file, stdlib only).

The mark is three isometric slabs stacked on each other, drawn as thin white lines: it reads as a
database stack and as quarried marble blocks at the same time. The tile is a quiet dark grey
gradient; there is no second colour.

Outputs
  theta.svg        full-detail 512 tile, used for every size >= 64 px
  theta-mid.svg    simplified tile with a heavier stroke, used for 32 and 48 px
  theta-small.svg  minimal tile (top rhombus and two chevrons), used for 16 and 24 px
  theta-light.svg  dark lines on a light grey gradient, for docs
  theta-mark.svg   glyph only, `currentColor`, tight square viewBox (embedded in the app)

Why three detail levels: a stroke that is elegant at 512 px (about 2% of the tile) is a
sub-pixel smudge at 32 px, and the slab seams are narrower than a pixel there. Each variant
keeps the same silhouette (an isometric stack) but drops lines and thickens the rest, so it
never has to rely on anti-aliasing to stay legible.

Geometry notes
  * Slopes are 2:1 ("pixel-art isometric", 26.57 degrees) instead of true 30 degrees: a 2:1 line
    steps one pixel down for every two across, which rasterises evenly at every size.
  * Hidden edges are omitted. With a gap between the slabs, the only lower-slab edges drawn
    are its own front edges, so each lower slab is a "cup" of two walls under a chevron.
"""
import os
import re

TILE_RX = 115        # 22.4% of 512, the usual macOS-style squircle approximation
C = 256

INK = "#FFFFFF"
INK_OPACITY = 0.92

# Tile backgrounds. Lighter top-left to darker bottom-right; the red/green/blue spread stays
# within a few units so it reads as neutral grey, not as blue-black.
DARK_BG = dict(kind="diag", stops=[(0, "#3b3d42"), (1, "#18191b")])
LIGHT_BG = dict(kind="diag", stops=[(0, "#f4f4f5"), (1, "#d3d4d7")])
LIGHT_INK = "#1b1c1f"

# a: half width of a slab; the top face is a 2:1 rhombus (a wide, a/2 high).
# h: slab wall height, g: vertical gap between slabs, sw: stroke width.
FULL = dict(mode="full", n=3, a=128, h=40, g=28, sw=10, join="miter", ring=0.0, fill=0.04, hidden=False)
MID = dict(mode="mid", n=3, a=152, h=48, g=40, sw=16, join="miter", ring=0.0, fill=0.0, hidden=False, front=False,
           open_top=True)
SMALL = dict(mode="mini", n=3, a=176, h=0, g=0, p=96, sw=40, join="miter", ring=0.0, fill=0.0, hidden=False, front=False)


def fmt(v):
    s = f"{v:.2f}".rstrip("0").rstrip(".")
    return "0" if s == "-0" else s


def pts(points, close=False):
    d = "M" + " L".join(f"{fmt(x)} {fmt(y)}" for x, y in points)
    return d + ("Z" if close else "")


def slab_lines(P, k):
    """Visible stroke paths and the visible face polygon of slab k (centre of its top face at y = k*p)."""
    a, h, g = P["a"], P["h"], P["g"]
    y = k * (h + g)
    q = a / 2
    top = (k == 0)
    paths = []
    if top:
        # outer silhouette including the back edges, closed so the corners join
        paths.append(pts([(-a, y), (0, y - q), (a, y), (a, y + h), (0, y + h + q), (-a, y + h)], close=True))
        face = [(-a, y), (0, y - q), (a, y), (a, y + h), (0, y + h + q), (-a, y + h)]
        if P["hidden"]:
            paths.append(("hidden", pts([(-a, y + h), (0, y + h - q), (a, y + h)]) + " " + pts([(0, y - q), (0, y + h - q)])))
    else:
        # Physically a sliver of the lower slab's back edges would show in each gap, but at icon scale it
        # reads as a stray tick at the corners. Drawing only the slab's own front edges is cleaner.
        if P.get("open_top"):
            # no chevron at the top of the walls: the upper slab's underside already draws one just above
            paths.append(pts([(-a, y), (-a, y + h), (0, y + h + q), (a, y + h), (a, y)]))
        else:
            paths.append(pts([(-a, y), (0, y + q), (a, y), (a, y + h), (0, y + h + q), (-a, y + h)], close=True))
        face = [(-a, y), (0, y + q), (a, y), (a, y + h), (0, y + h + q), (-a, y + h)]
    if top:
        paths.append(pts([(-a, y), (0, y + q), (a, y)]))
    # The front vertical edge is the one line a 32 px render cannot place on a pixel centre (the
    # tile centre is a pixel boundary), so the mid variant leaves it out.
    if P.get("front", True):
        paths.append(pts([(0, y + q), (0, y + q + h)]))
    return paths, face


def mini_lines(P, k):
    """Minimal variant: slab 0 is the closed top rhombus, the others are single chevrons."""
    a, p = P["a"], P["p"]
    q = a / 2
    y = k * p
    if k == 0:
        return [pts([(-a, y), (0, y - q), (a, y), (0, y + q)], close=True)], [(-a, y), (0, y - q), (a, y), (0, y + q)]
    return [pts([(-a, y), (0, y + q), (a, y)])], []


def build(P):
    """Returns (svg elements as strings, bbox) in 512 tile coordinates for parameter set P."""
    n = P["n"]
    body, faces, hidden = [], [], []
    for k in range(n):
        paths, face = (mini_lines if P["mode"] == "mini" else slab_lines)(P, k)
        for d in paths:
            if isinstance(d, tuple):
                hidden.append(d[1])
            else:
                body.append(d)
        if face:
            faces.append(face)
    a, h, g = P["a"], P["h"], P["g"]
    p = P.get("p", h + g)
    top, bottom = -a / 2, (n - 1) * p + (h + a / 2 if P["mode"] != "mini" else a / 2)
    dy = -(top + bottom) / 2 + C
    return body, faces, hidden, dy, (top + dy, bottom + dy)


def shift(d, dy):
    """Move a path produced by pts() to tile coordinates: x centred on C, y moved by dy."""
    nums = iter(re.findall(r"-?\d+(?:\.\d+)?", d))
    return re.sub(r"-?\d+(?:\.\d+)? -?\d+(?:\.\d+)?", lambda m: f"{fmt(float(next(nums)) + C)} {fmt(float(next(nums)) + dy)}", d)


def glyph_group(P, color, opacity=None, dy_extra=0):
    body, faces, hidden, dy, bb = build(P)
    dy += dy_extra
    sw = P["sw"]
    parts = []
    if P["fill"] > 0:
        for f in faces:
            parts.append(f'<path d="{shift(pts(f, True), dy)}" fill="{color}" fill-opacity="{P["fill"]}" stroke="none"/>')
    if P["ring"] > 0:
        rx = P["a"] * P["ring"]
        parts.append(f'<ellipse cx="{C}" cy="{fmt(dy)}" rx="{fmt(rx)}" ry="{fmt(rx / 2)}" fill="none"/>')
    for d in body:
        parts.append(f'<path d="{shift(d, dy)}"/>')
    for d in hidden:
        parts.append(f'<path d="{shift(d, dy)}" stroke-opacity="0.35"/>')
    op = f' opacity="{opacity}"' if opacity is not None else ""
    return (f'<g fill="none" stroke="{color}" stroke-width="{sw}" stroke-linejoin="{P["join"]}" '
            f'stroke-linecap="{"round" if P["join"] == "round" else "butt"}" stroke-miterlimit="4"{op}>'
            + "".join(parts) + "</g>"), bb


def defs(bg, uid="bg"):
    st = "".join(f'<stop offset="{o}" stop-color="{c}"/>' for o, c in bg["stops"])
    if bg["kind"] == "vert":
        grad = f'<linearGradient id="{uid}" x1="0" y1="0" x2="0" y2="1">{st}</linearGradient>'
    elif bg["kind"] == "radial":
        grad = f'<radialGradient id="{uid}" cx="0.3" cy="0.22" r="0.95">{st}</radialGradient>'
    else:
        grad = f'<linearGradient id="{uid}" x1="0" y1="0" x2="1" y2="1">{st}</linearGradient>'
    # 1 px inner highlight along the top edge only; fades out within the top quarter
    hi = ('<linearGradient id="hi" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fff" stop-opacity="0.16"/>'
          '<stop offset="0.12" stop-color="#fff" stop-opacity="0"/></linearGradient>')
    return f"<defs>{grad}{hi}</defs>"


def svg_tile(P, bg, ink, ink_opacity=INK_OPACITY, highlight=True, title="Theta"):
    g, _ = glyph_group(P, ink, ink_opacity)
    hl = (f'<rect x="0.5" y="0.5" width="511" height="511" rx="{TILE_RX - 0.5}" fill="none" stroke="url(#hi)"/>'
          if highlight else "")
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512">\n'
            f'  <title>{title}</title>\n  {defs(bg)}\n'
            f'  <rect width="512" height="512" rx="{TILE_RX}" fill="url(#bg)"/>\n  {hl}\n  {g}\n</svg>\n')


def svg_mark(P, side_pad=8):
    """Glyph only: a square viewBox just big enough for the strokes, so `Icon::size(44)` is 44 px of mark."""
    g, bb = glyph_group(P, "currentColor")
    sw = P["sw"]
    # miter tips of the roof and floor stick out past sw/2; allow for them
    ext = sw * (0.75 if P["join"] == "miter" else 0.5)
    half_h = (bb[1] - bb[0]) / 2 + ext
    half_w = P["a"] + ext
    side = 2 * max(half_h, half_w) + side_pad * 2
    cy = (bb[0] + bb[1]) / 2
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{fmt(C - side / 2)} {fmt(cy - side / 2)} {fmt(side)} {fmt(side)}">\n'
            f'  <title>Theta</title>\n  {g}\n</svg>\n')


# The in-app renderer ignores fill-opacity, so the faint face fill is dropped; slab height and gap grow so the
# seams stay open at the ~40 px the welcome view draws it.
MARK = dict(FULL, sw=16, fill=0.0, h=46, g=36)


def main():
    here = os.path.dirname(os.path.abspath(__file__))

    def w(name, text):
        with open(os.path.join(here, name), "w") as f:
            f.write(text)

    w("theta.svg", svg_tile(FULL, DARK_BG, INK))
    w("theta-mid.svg", svg_tile(MID, DARK_BG, INK))
    w("theta-small.svg", svg_tile(SMALL, DARK_BG, INK, highlight=False))
    w("theta-light.svg", svg_tile(FULL, LIGHT_BG, LIGHT_INK, 0.9, highlight=False))
    w("theta-mark.svg", svg_mark(MARK))


if __name__ == "__main__":
    main()
