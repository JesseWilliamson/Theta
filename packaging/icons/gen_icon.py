#!/usr/bin/env python3
"""Generates packaging/icons/quarry.svg (run from anywhere; writes next to this file).

The icon is an isometric block of stone cut into three benches (the strata of a
quarry face); the top face carries a ring that hints at a database cylinder.
"""
import os

cx, top, h, w, depth = 256, 92, 82, 138, 160   # block geometry (512 viewBox)
P = lambda x, y: f"{x:.1f},{y:.1f}"

def lerp(a, b, t): return a + (b - a) * t

def layer_polys(side):
    """Three layers of one side face, separated by narrow gaps."""
    x0, x1 = (cx - w, cx) if side == "L" else (cx, cx + w)
    # y of the face's top edge at x0 / x1 (isometric slope); the face is `depth` tall
    ya0, ya1 = (top + h, top + 2 * h) if side == "L" else (top + 2 * h, top + h)
    polys = []
    bands = [(0.0, 0.31), (0.355, 0.645), (0.69, 1.0)]
    for a, b in bands:
        pts = [(x0, ya0 + depth * a), (x1, ya1 + depth * a), (x1, ya1 + depth * b), (x0, ya0 + depth * b)]
        polys.append(" ".join(P(*p) for p in pts))
    return polys

top_face = " ".join(P(*p) for p in [(cx, top), (cx + w, top + h), (cx, top + 2 * h), (cx - w, top + h)])
left = layer_polys("L")
right = layer_polys("R")
lc = ["#f0883e", "#e57a32", "#d96d28"]
rc = ["#b9531f", "#ad4a1a", "#9f4216"]

# ring on the top face: an ellipse in the face's plane is a rhombus-inscribed ellipse
ecx, ecy = cx, top + h
svg = f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512">
  <title>Quarry</title>
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#262c3a"/>
      <stop offset="1" stop-color="#141720"/>
    </linearGradient>
    <linearGradient id="top" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#ffd28a"/>
      <stop offset="1" stop-color="#ffb55a"/>
    </linearGradient>
  </defs>
  <rect width="512" height="512" rx="112" fill="url(#bg)"/>
  <g stroke-linejoin="round">
'''
for p, c in zip(left, lc): svg += f'    <polygon points="{p}" fill="{c}"/>\n'
for p, c in zip(right, rc): svg += f'    <polygon points="{p}" fill="{c}"/>\n'
svg += f'''    <polygon points="{top_face}" fill="url(#top)"/>
    <ellipse cx="{ecx}" cy="{ecy}" rx="{w*0.52:.1f}" ry="{h*0.52:.1f}" fill="none" stroke="#d9852c" stroke-width="9" opacity="0.55"/>
    <ellipse cx="{ecx}" cy="{ecy}" rx="{w*0.22:.1f}" ry="{h*0.22:.1f}" fill="#d9852c" opacity="0.55"/>
  </g>
</svg>
'''
open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "quarry.svg"), "w").write(svg)
