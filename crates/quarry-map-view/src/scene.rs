//! Painting: basemap tiles, graticule, geometries and the measure overlay.
//!
//! Everything is drawn from the precomputed world coordinates with one multiply-add per
//! vertex. Per frame work is bounded by what is *visible*: the grid index culls features,
//! the LOD cache reduces vertex counts to what a pixel grid can show, tiny features collapse
//! to dots, and rings are clipped to the viewport so `f32` tessellation stays accurate.

use crate::camera::Transform;
use crate::clip::{P, Rect, all_inside, all_outside, clip_polyline, clip_ring, earclip};
use crate::core::{Core, grid_spec};
use crate::palette::{Hsl, layer_color};
use crate::tiles::ancestor_region;
use crate::world::{Mode, Part, WorldFeature, for_each_part};
use gpui_kit::{
    App, Background, BorderStyle, Bounds, ContentMask, Corners, Hsla, Path, PathBuilder, Pixels, Point,
    Window, fill, hsla, point, px, quad, size,
};
use quarry_geo::{Coord, Geometry};

/// Theme colours the painter needs, captured once per frame.
#[derive(Clone, Copy)]
pub struct Colors {
    /// Panel background.
    pub bg: Hsla,
    /// Foreground text colour.
    pub fg: Hsla,
    /// Map placeholder (behind tiles / no basemap).
    pub placeholder: Hsla,
    /// Accent for selection and the measure tool.
    pub accent: Hsla,
    /// Dark theme?
    pub dark: bool,
}

/// Maximum points drawn as full circles; above this squares are used.
const CIRCLE_LIMIT: usize = 5_000;
/// Input vertices per stroke path; lyon's index buffer is `u16` and strokes emit several
/// vertices per input point.
const STROKE_BATCH: usize = 1_500;
/// Most vertices in one fill path (`u16` index buffer).
const FILL_LIMIT: usize = 20_000;
/// Clip margin in pixels around the viewport.
const MARGIN: f64 = 24.0;
/// Polygons whose extent is below this many pixels are drawn compactly (fill only).
const COMPACT_PX: f64 = 12.0;

fn to_hsla(c: Hsl, a: f32) -> Hsla {
    hsla(c.h, c.s, c.l, a)
}

struct Cx<'a> {
    origin: Point<Pixels>,
    t: Transform,
    clip: Rect,
    colors: &'a Colors,
}

impl Cx<'_> {
    fn pt(&self, p: P) -> Point<Pixels> {
        point(self.origin.x + px(p[0] as f32), self.origin.y + px(p[1] as f32))
    }
}

struct Batch {
    builder: PathBuilder,
    n: usize,
}

/// Accumulates stroke geometry per layer and flushes before the index buffer overflows.
struct Strokes {
    width: f32,
    batches: Vec<Option<Batch>>,
}

impl Strokes {
    fn new(width: f32, layers: usize) -> Self {
        Strokes { width, batches: (0..layers.max(1)).map(|_| None).collect() }
    }

    fn add(&mut self, cx: &Cx, layer: usize, pts: &[P], closed: bool, color: Hsla, window: &mut Window) {
        if pts.len() < 2 {
            return;
        }
        let slot = layer.min(self.batches.len() - 1);
        if self.batches[slot].as_ref().is_some_and(|b| b.n + pts.len() > STROKE_BATCH) {
            self.flush_slot(slot, color, window);
        }
        let width = self.width;
        let b =
            self.batches[slot].get_or_insert_with(|| Batch { builder: PathBuilder::stroke(px(width)), n: 0 });
        b.builder.move_to(cx.pt(pts[0]));
        for p in &pts[1..] {
            b.builder.line_to(cx.pt(*p));
        }
        if closed {
            b.builder.close();
        }
        b.n += pts.len();
    }

    fn flush_slot(&mut self, slot: usize, color: Hsla, window: &mut Window) {
        if let Some(b) = self.batches[slot].take()
            && let Ok(path) = b.builder.build()
        {
            window.paint_path(path, color);
        }
    }

    fn flush(&mut self, colors: impl Fn(usize) -> Hsla, window: &mut Window) {
        for slot in 0..self.batches.len() {
            self.flush_slot(slot, colors(slot), window);
        }
    }
}

/// Reduce a vertex list to at most `limit` by striding (only reached for absurd inputs).
fn decimate(pts: Vec<P>, limit: usize) -> Vec<P> {
    if pts.len() <= limit {
        return pts;
    }
    let step = pts.len().div_ceil(limit);
    pts.into_iter().step_by(step).collect()
}

fn project(cx: &Cx, k: f64, coords: &[Coord], out: &mut Vec<P>) {
    out.clear();
    out.extend(coords.iter().map(|c| cx.t.apply([c[0] + k, c[1]])));
}

/// Screen-space rings of a polygon, clipped; each ring non-empty with at least 3 points.
fn polygon_rings(cx: &Cx, k: f64, rings: &[Vec<Coord>], buf: &mut Vec<P>) -> Vec<Vec<P>> {
    let mut out = Vec::with_capacity(rings.len());
    for (i, ring) in rings.iter().enumerate() {
        project(cx, k, ring, buf);
        if all_outside(buf, &cx.clip) {
            if i == 0 {
                return vec![]; // exterior entirely off-screen: nothing of the polygon is visible
            }
            continue;
        }
        let pts = if all_inside(buf, &cx.clip) { std::mem::take(buf) } else { clip_ring(buf, &cx.clip) };
        if pts.len() >= 3 {
            out.push(decimate(pts, FILL_LIMIT));
        } else if i == 0 {
            return vec![];
        }
    }
    out
}

fn paint_fill(cx: &Cx, rings: &[Vec<P>], color: Hsla, window: &mut Window) {
    if let Some(path) = fill_path(cx, rings) {
        window.paint_path(path, color);
    }
}

/// Tessellate polygon rings (even-odd, so holes and islands in holes work).
fn fill_path(cx: &Cx, rings: &[Vec<P>]) -> Option<Path<Pixels>> {
    if rings.is_empty() {
        return None;
    }
    let mut b = PathBuilder::fill();
    for r in rings {
        b.move_to(cx.pt(r[0]));
        for p in &r[1..] {
            b.line_to(cx.pt(*p));
        }
        b.close();
    }
    b.build().ok()
}

fn line_pieces(cx: &Cx, k: f64, line: &[Coord], buf: &mut Vec<P>) -> Vec<Vec<P>> {
    project(cx, k, line, buf);
    if all_outside(buf, &cx.clip) {
        return vec![];
    }
    if all_inside(buf, &cx.clip) {
        return vec![std::mem::take(buf)];
    }
    clip_polyline(buf, &cx.clip)
}

fn circle(cx: &Cx, c: P, r: f32, fill_c: Hsla, border: f32, border_c: Hsla, window: &mut Window) {
    let p = cx.pt(c);
    let b = Bounds { origin: point(p.x - px(r), p.y - px(r)), size: size(px(r * 2.0), px(r * 2.0)) };
    window.paint_quad(quad(b, Corners::all(px(r)), fill_c, px(border), border_c, BorderStyle::default()));
}

/// How a feature is drawn.
#[derive(Clone, Copy)]
struct Style {
    color: Hsla,
    fill_alpha: f32,
    outline: f32,
    line: f32,
    radius: f32,
    halo: Option<Hsla>,
}

/// Paint all content of one frame into `bounds`.
pub fn paint_map(
    core: &mut Core,
    bounds: Bounds<Pixels>,
    colors: &Colors,
    window: &mut Window,
    _cx: &mut App,
) {
    let sz = [f64::from(bounds.size.width), f64::from(bounds.size.height)];
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        window.paint_quad(fill(bounds, colors.placeholder));
        let t = core.camera.transform();
        let cx =
            Cx { origin: bounds.origin, t, clip: [-MARGIN, -MARGIN, sz[0] + MARGIN, sz[1] + MARGIN], colors };
        let mut tiles_drawn = 0;
        if core.prepared.mode == Mode::Geographic && core.basemap {
            tiles_drawn = paint_tiles(core, &cx, window);
            if colors.dark && core.dim && tiles_drawn > 0 {
                window.paint_quad(fill(bounds, colors.bg.opacity(0.35)));
            }
        }
        core.grid_shown = tiles_drawn == 0;
        if core.grid_shown {
            paint_grid(core, &cx, sz, window);
        }
        paint_features(core, &cx, sz, window);
        paint_measure(core, &cx, window);
    });
}

fn paint_tiles(core: &mut Core, cx: &Cx, window: &mut Window) -> usize {
    let crate::camera::Camera::Geo(view) = core.camera else { return 0 };
    let visible = view.visible_tiles();
    let ids: Vec<_> = visible.iter().map(|(t, _, _)| *t).collect();
    core.tiles.want(&ids);
    let mut drawn = 0;
    for (tile, pos, drawn_size) in visible {
        // Snap both edges to whole pixels: adjacent tiles then share an edge exactly, so no
        // hairline of background shows between them.
        let (ox, oy) = (f32::from(cx.origin.x), f32::from(cx.origin.y));
        let x0 = (ox + pos[0] as f32).round();
        let y0 = (oy + pos[1] as f32).round();
        let x1 = (ox + (pos[0] + drawn_size) as f32).round();
        let y1 = (oy + (pos[1] + drawn_size) as f32).round();
        let r = Bounds { origin: point(px(x0), px(y0)), size: size(px(x1 - x0), px(y1 - y0)) };
        let drawn_size = f64::from(x1 - x0);
        if let Some(img) = core.tiles.image(tile) {
            if window.paint_image(r, r, Corners::default(), img.clone(), 0, false).is_ok() {
                drawn += 1;
            }
            continue;
        }
        // Not loaded yet: scale up the nearest cached ancestor so panning/zooming never
        // flashes empty tiles.
        for up in 1..=4u8 {
            let Some((parent, uv)) = ancestor_region(tile, up) else { break };
            if let Some(img) = core.tiles.image(parent) {
                let full = drawn_size * f64::from(1u32 << up);
                let image_bounds = Bounds {
                    origin: point(
                        r.origin.x - px((f64::from(uv[0]) * full) as f32),
                        r.origin.y - px((f64::from(uv[1]) * full) as f32),
                    ),
                    size: size(px(full as f32), px(full as f32)),
                };
                if window.paint_image(r, image_bounds, Corners::default(), img.clone(), 0, false).is_ok() {
                    drawn += 1;
                }
                break;
            }
        }
    }
    drawn
}

fn paint_grid(core: &Core, cx: &Cx, sz: [f64; 2], window: &mut Window) {
    let spec = grid_spec(&core.camera);
    let line = cx.colors.fg.opacity(0.07);
    let axis = cx.colors.fg.opacity(0.18);
    let o = cx.origin;
    for l in &spec.vertical {
        let b = Bounds { origin: point(o.x + px(l.at as f32), o.y), size: size(px(1.0), px(sz[1] as f32)) };
        window.paint_quad(fill(b, if l.axis { axis } else { line }));
    }
    for l in &spec.horizontal {
        let b = Bounds { origin: point(o.x, o.y + px(l.at as f32)), size: size(px(sz[0] as f32), px(1.0)) };
        window.paint_quad(fill(b, if l.axis { axis } else { line }));
    }
}

fn is_highlight(core: &Core, id: usize, f: &WorldFeature) -> (bool, bool) {
    (core.hover == Some(id), core.selected_row == Some(f.row))
}

/// Merges many small meshes (fills, point squares) into one `Path` primitive.
///
/// GPUI orders every primitive against everything it overlaps (a bounds tree), so tens of
/// thousands of separate quads or paths cost microseconds *each*. One path with many
/// triangles is a single primitive.
struct Mesh {
    path: Option<Path<Pixels>>,
    triangles: usize,
}

/// Triangles per merged path before flushing (keeps each rasterisation pass bounded).
const MESH_TRIANGLES: usize = 24_000;

impl Mesh {
    fn new() -> Self {
        Mesh { path: None, triangles: 0 }
    }

    /// Append a tessellated path (its vertices are already triangles).
    fn append(&mut self, p: Path<Pixels>, color: Hsla, window: &mut Window) {
        let n = p.vertices.len() / 3;
        if n == 0 {
            return;
        }
        if self.triangles + n > MESH_TRIANGLES {
            self.flush(color, window);
        }
        match self.path.as_mut() {
            None => self.path = Some(p),
            Some(base) => {
                base.bounds = base.bounds.union(&p.bounds);
                base.vertices.extend(p.vertices);
            }
        }
        self.triangles += n;
    }

    fn square(&mut self, c: Point<Pixels>, half: f32, color: Hsla, window: &mut Window) {
        self.rect(c, half, half, color, window);
    }

    /// One triangle given as absolute window points.
    fn triangle(
        &mut self,
        a: Point<Pixels>,
        b: Point<Pixels>,
        c: Point<Pixels>,
        color: Hsla,
        window: &mut Window,
    ) {
        if self.triangles + 1 > MESH_TRIANGLES {
            self.flush(color, window);
        }
        let st = (point(0.0, 1.0), point(0.0, 1.0), point(0.0, 1.0));
        self.path.get_or_insert_with(|| Path::new(a)).push_triangle((a, b, c), st);
        self.triangles += 1;
    }

    /// A rectangle of half-extents `hw` x `hh` around `c`.
    fn rect(&mut self, c: Point<Pixels>, hw: f32, hh: f32, color: Hsla, window: &mut Window) {
        if self.triangles + 2 > MESH_TRIANGLES {
            self.flush(color, window);
        }
        let (w, h) = (px(hw), px(hh));
        let (a, b, d, e) = (
            point(c.x - w, c.y - h),
            point(c.x + w, c.y - h),
            point(c.x + w, c.y + h),
            point(c.x - w, c.y + h),
        );
        let st = (point(0.0, 1.0), point(0.0, 1.0), point(0.0, 1.0));
        let path = self.path.get_or_insert_with(|| Path::new(a));
        path.push_triangle((a, b, d), st);
        path.push_triangle((a, d, e), st);
        self.triangles += 2;
    }

    fn flush(&mut self, color: Hsla, window: &mut Window) {
        if let Some(p) = self.path.take() {
            window.paint_path(p, color);
        }
        self.triangles = 0;
    }
}

/// One mesh per layer.
struct Meshes(Vec<Mesh>);

impl Meshes {
    fn new(layers: usize) -> Self {
        Meshes((0..layers.max(1)).map(|_| Mesh::new()).collect())
    }

    fn slot(&mut self, layer: usize) -> &mut Mesh {
        let i = layer.min(self.0.len() - 1);
        &mut self.0[i]
    }

    fn flush(&mut self, color: impl Fn(usize) -> Hsla, window: &mut Window) {
        for (i, m) in self.0.iter_mut().enumerate() {
            m.flush(color(i), window);
        }
    }
}

/// Screen-space de-duplication of dense points: at most one square per `CELL` px cell per
/// layer, since squares closer than that are indistinguishable anyway.
struct CellGrid {
    w: usize,
    h: usize,
    layers: Vec<Vec<u8>>,
}

const CELL: f64 = 2.0;

impl CellGrid {
    fn new(sz: [f64; 2], layers: usize) -> Self {
        let (w, h) = ((sz[0] / CELL).ceil() as usize + 1, (sz[1] / CELL).ceil() as usize + 1);
        CellGrid { w, h, layers: (0..layers.max(1)).map(|_| vec![0u8; w * h]).collect() }
    }

    /// True the first time a cell is claimed.
    fn claim(&mut self, layer: usize, s: P) -> bool {
        let (cx, cy) = ((s[0] / CELL).floor(), (s[1] / CELL).floor());
        if cx < 0.0 || cy < 0.0 || cx as usize >= self.w || cy as usize >= self.h {
            return false;
        }
        let i = layer.min(self.layers.len() - 1);
        let cell = &mut self.layers[i][cy as usize * self.w + cx as usize];
        std::mem::replace(cell, 1) == 0
    }
}

fn paint_features(core: &mut Core, cx: &Cx, sz: [f64; 2], window: &mut Window) {
    let dark = cx.colors.dark;
    let layers = core.layers.len().max(1);
    let level = cx.t.lod_level();
    let offsets = core.wrap_offsets(&cx.t, sz);
    let world = cx.t.world_rect(sz, MARGIN);
    let layer_col = |l: usize| to_hsla(layer_color(l, dark), 1.0);
    let mut buf: Vec<P> = Vec::new();
    let mut ids: Vec<u32> = Vec::new();
    let scale = cx.t.scale[0].abs();
    // Features collapse to a dot below this on-screen size: nothing else would be visible.
    let tiny =
        |f: &WorldFeature| f.bbox.width() * scale < 2.0 && f.bbox.height() * scale < 2.0 && !f.kinds.points;
    // Polygons this small show no shape: a filled box of their extent is enough and skips
    // tessellation, which is the dominant per-polygon cost.
    let small_poly = |f: &WorldFeature| f.bbox.width() * scale < 4.0 && f.bbox.height() * scale < 4.0;

    // Cull every world copy first so the circle-vs-square decision sees the whole picture
    // (otherwise a sparse edge copy would draw circles beside a dense squares-only centre).
    let mut copies: Vec<(f64, Vec<usize>)> = Vec::new();
    for &k in &offsets {
        let q =
            quarry_geo::BBox { min: [world.min[0] - k, world.min[1]], max: [world.max[0] - k, world.max[1]] };
        core.index.query_into(&q, &mut ids);
        let visible: Vec<usize> = ids
            .iter()
            .map(|i| *i as usize)
            .filter(|i| core.layer_visible(&core.prepared.features[*i]))
            .collect();
        if !visible.is_empty() {
            copies.push((k, visible));
        }
    }
    let n_points: usize = copies
        .iter()
        .flat_map(|(_, v)| v.iter())
        .map(|i| &core.prepared.features[*i])
        .filter(|f| f.kinds.points)
        .map(|f| f.vertices)
        .sum();
    let squares = n_points > CIRCLE_LIMIT;
    let mut cells = squares.then(|| CellGrid::new(sz, layers));
    let mut dot_cells = CellGrid::new(sz, layers);

    for (k, visible) in &copies {
        let k = *k;
        let mut dots: Vec<(P, usize)> = Vec::new();

        // Pass 1: polygons.
        let mut outlines = Strokes::new(1.5, layers);
        let mut fills = Meshes::new(layers);
        let mut box_mesh = Meshes::new(layers);
        for &id in visible {
            let f = &core.prepared.features[id];
            if !f.kinds.polygons {
                continue;
            }
            if small_poly(f) {
                let c = cx.t.apply([f.bbox.center()[0] + k, f.bbox.center()[1]]);
                if c[0] > -8.0 && c[0] < sz[0] + 8.0 && c[1] > -8.0 && c[1] < sz[1] + 8.0 {
                    let (hw, hh) = (
                        (f.bbox.width() * scale / 2.0).max(1.25) as f32,
                        (f.bbox.height() * scale / 2.0).max(1.25) as f32,
                    );
                    box_mesh.slot(f.layer).rect(cx.pt(c), hw, hh, layer_col(f.layer).opacity(0.6), window);
                }
                continue;
            }
            let (layer, vertices) = (f.layer, f.vertices);
            let compact = f.bbox.width() * scale < COMPACT_PX && f.bbox.height() * scale < COMPACT_PX;
            let geom = core.lod.pick(level, id, &core.prepared.features[id].geom, vertices);
            let color = layer_col(layer);
            for_each_part(&geom, &mut |part| {
                if let Part::Polygon(rings) = part {
                    let rings = polygon_rings(cx, k, rings, &mut buf);
                    if compact
                        && let Some(ext) = rings.first()
                        && let Some(tris) = earclip(ext)
                    {
                        // Small polygon: triangulate ourselves, no outline (it would cover
                        // most of the shape) and skip the general tessellator.
                        let m = box_mesh.slot(layer);
                        let c = color.opacity(0.6);
                        for t in tris {
                            m.triangle(cx.pt(ext[t[0]]), cx.pt(ext[t[1]]), cx.pt(ext[t[2]]), c, window);
                        }
                        return;
                    }
                    if let Some(path) = fill_path(cx, &rings) {
                        fills.slot(layer).append(path, color.opacity(0.25), window);
                    }
                    for r in &rings {
                        outlines.add(cx, layer, r, true, color, window);
                    }
                }
            });
        }
        fills.flush(|l| layer_col(l).opacity(0.25), window);
        box_mesh.flush(|l| layer_col(l).opacity(0.6), window);
        outlines.flush(layer_col, window);

        // Pass 2: lines.
        let mut lines = Strokes::new(2.0, layers);
        for &id in visible {
            let f = &core.prepared.features[id];
            if !f.kinds.lines {
                continue;
            }
            if tiny(f) {
                dots.push((cx.t.apply([f.bbox.center()[0] + k, f.bbox.center()[1]]), f.layer));
                continue;
            }
            let (layer, vertices) = (f.layer, f.vertices);
            let geom = core.lod.pick(level, id, &core.prepared.features[id].geom, vertices);
            for_each_part(&geom, &mut |part| {
                if let Part::Line(l) = part {
                    for piece in line_pieces(cx, k, l, &mut buf) {
                        lines.add(cx, layer, &piece, false, layer_col(layer), window);
                    }
                }
            });
        }
        lines.flush(layer_col, window);

        // Pass 3: points and collapsed features.
        let outline = hsla(0.0, 0.0, 1.0, 0.95);
        let inside = |c: P| c[0] >= -8.0 && c[0] <= sz[0] + 8.0 && c[1] >= -8.0 && c[1] <= sz[1] + 8.0;
        let mut squares_mesh = Meshes::new(layers);
        for &id in visible {
            let f = &core.prepared.features[id];
            if !f.kinds.points {
                continue;
            }
            let layer = f.layer;
            let color = layer_col(layer);
            for_each_part(&f.geom, &mut |part| {
                if let Part::Point(c) = part {
                    let s = cx.t.apply([c[0] + k, c[1]]);
                    if !inside(s) {
                        return;
                    }
                    match cells.as_mut() {
                        Some(cells) => {
                            if cells.claim(layer, s) {
                                squares_mesh.slot(layer).square(cx.pt(s), 1.5, color, window);
                            }
                        }
                        None => circle(cx, s, 5.0, color, 1.5, outline, window),
                    }
                }
            });
        }
        for (s, layer) in dots {
            if inside(s) && dot_cells.claim(layer, s) {
                squares_mesh.slot(layer).square(cx.pt(s), 1.5, layer_col(layer), window);
            }
        }
        squares_mesh.flush(layer_col, window);

        // Highlights go on top of everything of this copy.
        for &id in visible {
            let f = &core.prepared.features[id];
            let (hover, selected) = is_highlight(core, id, f);
            if !(hover || selected) {
                continue;
            }
            let style = if selected {
                Style {
                    color: cx.colors.accent,
                    fill_alpha: 0.3,
                    outline: 2.5,
                    line: 3.5,
                    radius: 7.5,
                    halo: Some(cx.colors.accent.opacity(0.4)),
                }
            } else {
                Style {
                    color: to_hsla(layer_color(f.layer, dark).brighter(), 1.0),
                    fill_alpha: 0.38,
                    outline: 2.5,
                    line: 3.5,
                    radius: 6.5,
                    halo: None,
                }
            };
            let vertices = f.vertices;
            let geom = core.lod.pick(level, id, &core.prepared.features[id].geom, vertices);
            paint_single(cx, k, &geom, &style, sz, &mut buf, window);
        }
    }
}

fn stroke_path(cx: &Cx, pts: &[P], closed: bool, width: f32, color: Hsla, window: &mut Window) {
    if pts.len() < 2 {
        return;
    }
    let mut b = PathBuilder::stroke(px(width));
    b.move_to(cx.pt(pts[0]));
    for p in &pts[1..] {
        b.line_to(cx.pt(*p));
    }
    if closed {
        b.close();
    }
    if let Ok(path) = b.build() {
        window.paint_path(path, color);
    }
}

fn paint_single(
    cx: &Cx,
    k: f64,
    geom: &Geometry,
    st: &Style,
    sz: [f64; 2],
    buf: &mut Vec<P>,
    window: &mut Window,
) {
    let inside = |c: P| c[0] >= -12.0 && c[0] <= sz[0] + 12.0 && c[1] >= -12.0 && c[1] <= sz[1] + 12.0;
    let white = hsla(0.0, 0.0, 1.0, 1.0);
    for_each_part(geom, &mut |part| match part {
        Part::Polygon(rings) => {
            let rings = polygon_rings(cx, k, rings, buf);
            if let Some(h) = st.halo {
                for r in &rings {
                    stroke_path(cx, r, true, st.outline + 5.0, h, window);
                }
            }
            paint_fill(cx, &rings, st.color.opacity(st.fill_alpha), window);
            for r in &rings {
                stroke_path(cx, r, true, st.outline, st.color, window);
            }
        }
        Part::Line(l) => {
            for piece in line_pieces(cx, k, l, buf) {
                if let Some(h) = st.halo {
                    stroke_path(cx, &piece, false, st.line + 5.0, h, window);
                }
                stroke_path(cx, &piece, false, st.line, st.color, window);
            }
        }
        Part::Point(c) => {
            let s = cx.t.apply([c[0] + k, c[1]]);
            if inside(s) {
                if let Some(h) = st.halo {
                    circle(cx, s, st.radius + 6.0, h, 0.0, h, window);
                }
                circle(cx, s, st.radius, st.color, 2.0, white, window);
            }
        }
    });
}

fn paint_measure(core: &Core, cx: &Cx, window: &mut Window) {
    let m = &core.measure;
    if m.points.is_empty() {
        return;
    }
    let to_screen = |c: Coord| core.measure_screen(c);
    let mut pts: Vec<P> = m.points.iter().map(|c| to_screen(*c)).collect();
    let accent = cx.colors.accent;
    if m.closed && pts.len() >= 3 {
        paint_fill(cx, &[pts.clone()], accent.opacity(0.15), window);
    }
    // Rubber band to the pointer while the path is open.
    let mut line = pts.clone();
    if core.measuring
        && !m.closed
        && let Some(p) = core.pointer
    {
        line.push(p);
    }
    let casing = cx.colors.fg.opacity(0.92);
    stroke_path(cx, &line, m.closed, 5.0, casing, window);
    stroke_path(cx, &line, m.closed, 2.5, accent, window);
    for p in pts.drain(..) {
        circle(cx, p, 4.0, accent, 1.5, hsla(0.0, 0.0, 1.0, 1.0), window);
    }
}

/// Convert a gpui `Background`-agnostic colour for tests of the palette mapping.
#[allow(dead_code)]
fn _assert_background_from_hsla(c: Hsla) -> Background {
    c.into()
}
