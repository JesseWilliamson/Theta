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
use crate::style::LayerKind;
use crate::tiles::ancestor_region;
use crate::world::{Mode, Part, WorldFeature, for_each_part};
use gpui_kit::{
    App, Background, BorderStyle, Bounds, ContentMask, Corners, Hsla, Path, PathBuilder, Pixels, Point,
    SharedString, TextAlign, TextRun, Window, fill, hsla, point, px, quad, size,
};
use theta_geo::{Coord, Geometry};

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
pub(crate) const CIRCLE_LIMIT: usize = 5_000;
/// Input vertices per stroke path; lyon's index buffer is `u16` and strokes emit several
/// vertices per input point.
const STROKE_BATCH: usize = 1_500;
/// Most vertices in one fill path (`u16` index buffer).
const FILL_LIMIT: usize = 20_000;
/// Clip margin in pixels around the viewport.
pub(crate) const MARGIN: f64 = 24.0;
/// Polygons whose extent is below this many pixels are drawn compactly (fill only).
const COMPACT_PX: f64 = 12.0;
/// Half the side of the smallest marker a polygon is drawn as, in pixels.
pub(crate) const MIN_POLY_HALF: f64 = 3.0;
/// Up to this many collapsed polygons get an outlined marker each; more are merged into one mesh.
const OUTLINED_SMALL_LIMIT: usize = 2_000;

pub(crate) fn to_hsla(c: Hsl, a: f32) -> Hsla {
    hsla(c.h, c.s, c.l, a)
}

pub(crate) struct Cx<'a> {
    pub(crate) origin: Point<Pixels>,
    pub(crate) t: Transform,
    pub(crate) clip: Rect,
    pub(crate) colors: &'a Colors,
}

impl Cx<'_> {
    pub(crate) fn pt(&self, p: P) -> Point<Pixels> {
        point(self.origin.x + px(p[0] as f32), self.origin.y + px(p[1] as f32))
    }
}

struct Batch {
    builder: PathBuilder,
    n: usize,
}

/// Accumulates stroke geometry per layer and flushes before the index buffer overflows.
pub(crate) struct Strokes {
    /// Stroke width per slot.
    widths: Vec<f32>,
    batches: Vec<Option<Batch>>,
}

impl Strokes {
    pub(crate) fn new(widths: Vec<f32>) -> Self {
        let n = widths.len().max(1);
        Strokes { widths, batches: (0..n).map(|_| None).collect() }
    }

    pub(crate) fn add(&mut self, cx: &Cx, layer: usize, pts: &[P], closed: bool, color: Hsla, window: &mut Window) {
        if pts.len() < 2 {
            return;
        }
        let slot = layer.min(self.batches.len() - 1);
        if self.batches[slot].as_ref().is_some_and(|b| b.n + pts.len() > STROKE_BATCH) {
            self.flush_slot(slot, color, window);
        }
        let width = self.widths.get(slot).copied().unwrap_or(1.5).max(0.25);
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

    pub(crate) fn flush(&mut self, colors: impl Fn(usize) -> Hsla, window: &mut Window) {
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
pub(crate) fn polygon_rings(cx: &Cx, k: f64, rings: &[Vec<Coord>], buf: &mut Vec<P>) -> Vec<Vec<P>> {
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

pub(crate) fn paint_fill(cx: &Cx, rings: &[Vec<P>], color: Hsla, window: &mut Window) {
    if let Some(path) = fill_path(cx, rings) {
        window.paint_path(path, color);
    }
}

/// Tessellate polygon rings (even-odd, so holes and islands in holes work).
pub(crate) fn fill_path(cx: &Cx, rings: &[Vec<P>]) -> Option<Path<Pixels>> {
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

pub(crate) fn line_pieces(cx: &Cx, k: f64, line: &[Coord], buf: &mut Vec<P>) -> Vec<Vec<P>> {
    project(cx, k, line, buf);
    if all_outside(buf, &cx.clip) {
        return vec![];
    }
    if all_inside(buf, &cx.clip) {
        return vec![std::mem::take(buf)];
    }
    clip_polyline(buf, &cx.clip)
}

pub(crate) fn circle(cx: &Cx, c: P, r: f32, fill_c: Hsla, border: f32, border_c: Hsla, window: &mut Window) {
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
    app: &mut App,
) {
    let sz = [f64::from(bounds.size.width), f64::from(bounds.size.height)];
    core.ls.dark = colors.dark;
    for img in std::mem::take(&mut core.ls.retired) {
        let _ = window.drop_image(img);
    }
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        window.paint_quad(fill(bounds, colors.placeholder));
        let t = core.camera.transform();
        let cx =
            Cx { origin: bounds.origin, t, clip: [-MARGIN, -MARGIN, sz[0] + MARGIN, sz[1] + MARGIN], colors };
        let mut tiles_drawn = 0;
        if core.prepared.mode == Mode::Geographic && core.basemap {
            tiles_drawn = paint_tiles(core, &cx, window);
            if colors.dark && core.dim && !core.basemap_info.dark && tiles_drawn > 0 {
                window.paint_quad(fill(bounds, colors.bg.opacity(0.35)));
            }
            if tiles_drawn > 0 {
                dim_repeated_worlds(&cx, sz, window);
            }
        }
        core.grid_shown = tiles_drawn == 0;
        if core.grid_shown {
            paint_grid(core, &cx, sz, window);
        }
        crate::scene_layers::paint_aggregates(core, &cx, sz, window, app);
        crate::scene_layers::paint_tiled(core, &cx, sz, window, app);
        paint_features(core, &cx, sz, window);
        crate::scene_layers::paint_boundaries(core, &cx, window, app);
        paint_measure(core, &cx, window);
    });
}

/// Zoomed out in a wide panel, the basemap repeats sideways. Features are drawn only on the
/// world copy centred on the camera (`core::copy_window`); tinting the repeats with the
/// background makes that copy read as the live area, so nobody looks for data out there.
fn dim_repeated_worlds(cx: &Cx, sz: [f64; 2], window: &mut Window) {
    let world_w = cx.t.scale[0].abs();
    let (left, right) = (cx.t.half[0] - 0.5 * world_w, cx.t.half[0] + 0.5 * world_w);
    if left <= 0.0 && right >= sz[0] {
        return;
    }
    // Only the world's own height carries tiles; above and below it is plain background.
    let (top, bottom) = (cx.t.apply([0.0, 0.0])[1].max(0.0), cx.t.apply([0.0, 1.0])[1].min(sz[1]));
    if bottom <= top {
        return;
    }
    let tint = cx.colors.bg.opacity(REPEAT_DIM);
    let band = |x0: f64, x1: f64, window: &mut Window| {
        let (x0, x1) = (x0.max(0.0), x1.min(sz[0]));
        if x1 > x0 {
            let b = Bounds {
                origin: point(cx.origin.x + px(x0 as f32), cx.origin.y + px(top as f32)),
                size: size(px((x1 - x0) as f32), px((bottom - top) as f32)),
            };
            window.paint_quad(fill(b, tint));
        }
    };
    band(0.0, left, window);
    band(right, sz[0], window);
}

/// Background tint over the repeated copies of the basemap.
const REPEAT_DIM: f32 = 0.35;

fn paint_tiles(core: &mut Core, cx: &Cx, window: &mut Window) -> usize {
    let crate::camera::Camera::Geo(view) = core.camera else { return 0 };
    let visible = view.visible_tiles_capped(core.tiles.max_zoom);
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
pub(crate) struct Mesh {
    path: Option<Path<Pixels>>,
    triangles: usize,
}

/// Triangles per merged path before flushing (keeps each rasterisation pass bounded).
const MESH_TRIANGLES: usize = 24_000;

impl Mesh {
    pub(crate) fn new() -> Self {
        Mesh { path: None, triangles: 0 }
    }

    /// Append a tessellated path (its vertices are already triangles).
    pub(crate) fn append(&mut self, p: Path<Pixels>, color: Hsla, window: &mut Window) {
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

    pub(crate) fn square(&mut self, c: Point<Pixels>, half: f32, color: Hsla, window: &mut Window) {
        self.rect(c, half, half, color, window);
    }

    /// One triangle given as absolute window points.
    pub(crate) fn triangle(
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
    pub(crate) fn rect(&mut self, c: Point<Pixels>, hw: f32, hh: f32, color: Hsla, window: &mut Window) {
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

    pub(crate) fn flush(&mut self, color: Hsla, window: &mut Window) {
        if let Some(p) = self.path.take() {
            window.paint_path(p, color);
        }
        self.triangles = 0;
    }
}

/// One mesh per layer.
pub(crate) struct Meshes(pub(crate) Vec<Mesh>);

impl Meshes {
    pub(crate) fn new(layers: usize) -> Self {
        Meshes((0..layers.max(1)).map(|_| Mesh::new()).collect())
    }

    pub(crate) fn slot(&mut self, layer: usize) -> &mut Mesh {
        let i = layer.min(self.0.len() - 1);
        &mut self.0[i]
    }

    pub(crate) fn flush(&mut self, color: impl Fn(usize) -> Hsla, window: &mut Window) {
        for (i, m) in self.0.iter_mut().enumerate() {
            m.flush(color(i), window);
        }
    }
}

/// Screen-space de-duplication of dense points: at most one square per `CELL` px cell per
/// layer, since squares closer than that are indistinguishable anyway.
pub(crate) struct CellGrid {
    w: usize,
    h: usize,
    cell: f64,
    layers: Vec<Vec<u8>>,
}

const CELL: f64 = 2.0;

impl CellGrid {
    pub(crate) fn new(sz: [f64; 2], layers: usize) -> Self {
        Self::with_cell(sz, layers, CELL)
    }

    /// A grid of `cell` px cells (coarser when there are very many points to draw).
    pub(crate) fn with_cell(sz: [f64; 2], layers: usize, cell: f64) -> Self {
        let (w, h) = ((sz[0] / cell).ceil() as usize + 1, (sz[1] / cell).ceil() as usize + 1);
        CellGrid { w, h, cell, layers: (0..layers.max(1)).map(|_| vec![0u8; w * h]).collect() }
    }

    /// Cell side in px.
    pub(crate) fn cell(&self) -> f64 {
        self.cell
    }

    /// True the first time a cell is claimed.
    pub(crate) fn claim(&mut self, layer: usize, s: P) -> bool {
        let (cx, cy) = ((s[0] / self.cell).floor(), (s[1] / self.cell).floor());
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
    let layers = core.ls.n_slots.max(1);
    let level = cx.t.lod_level();
    let queries = core.copy_queries(&cx.t, sz, MARGIN);
    let info = SlotInfo::new(core);
    let slot_col = |s: usize| info.color(s);
    // Layers drawn by the tiler or as aggregates are not painted here.
    let legacy: Vec<bool> =
        (0..core.layers.len()).map(|l| !core.is_tiled(l) && !core.kind(l).is_aggregate()).collect();
    let is_legacy = |l: usize| legacy.get(l).copied().unwrap_or(true);
    // Nothing to do (and no 500,000-id index walk to throw away) when every visible layer is
    // drawn by the tiler or as an aggregate.
    if !core.layers.iter().enumerate().any(|(l, ly)| ly.visible && is_legacy(l)) {
        return;
    }
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
    for (k, q) in &queries {
        let k = *k;
        core.index.query_into(q, &mut ids);
        let visible: Vec<usize> = ids
            .iter()
            .map(|i| *i as usize)
            .filter(|i| {
                let f = &core.prepared.features[*i];
                core.layer_visible(f) && is_legacy(f.layer)
            })
            .collect();
        if !visible.is_empty() {
            copies.push((k, visible));
        }
    }
    let n_points: usize = copies
        .iter()
        .flat_map(|(_, v)| v.iter())
        .map(|i| &core.prepared.features[*i])
        .filter(|f| f.kinds.points || core.kind(f.layer) == LayerKind::Points)
        .map(|f| if f.kinds.points { f.vertices } else { 1 })
        .sum();
    let squares = n_points > CIRCLE_LIMIT;
    let few_small = copies
        .iter()
        .flat_map(|(_, v)| v.iter())
        .map(|i| &core.prepared.features[*i])
        .filter(|f| f.kinds.polygons && core.kind(f.layer) == LayerKind::Auto && small_poly(f))
        .count()
        <= OUTLINED_SMALL_LIMIT;
    let mut cells = squares.then(|| CellGrid::new(sz, layers));
    let mut dot_cells = CellGrid::new(sz, layers);

    for (k, visible) in &copies {
        let k = *k;
        let mut dots: Vec<(P, usize)> = Vec::new();

        // Pass 1: polygons.
        let mut outlines = Strokes::new(info.widths(core, 0.0));
        let mut fills = Meshes::new(layers);
        let mut box_mesh = Meshes::new(layers);
        for &id in visible {
            let f = &core.prepared.features[id];
            if !f.kinds.polygons || core.kind(f.layer) != LayerKind::Auto {
                continue;
            }
            let slot = core.slot(id);
            let (fill_a, op) = info.fill_alpha(core, f.layer);
            if small_poly(f) {
                let c = cx.t.apply([f.bbox.center()[0] + k, f.bbox.center()[1]]);
                if c[0] > -8.0 && c[0] < sz[0] + 8.0 && c[1] > -8.0 && c[1] < sz[1] + 8.0 {
                    // Never smaller than a point marker: at the zoom that fits a world-wide result a
                    // 10 km zone is a fraction of a pixel and must still be seen and told from the sea.
                    let (hw, hh) = (
                        (f.bbox.width() * scale / 2.0).max(MIN_POLY_HALF) as f32,
                        (f.bbox.height() * scale / 2.0).max(MIN_POLY_HALF) as f32,
                    );
                    if few_small {
                        let b = Bounds {
                            origin: cx.pt([c[0] - hw as f64, c[1] - hh as f64]),
                            size: size(px(hw * 2.0), px(hh * 2.0)),
                        };
                        let r = Corners::all(px(1.5));
                        let rim = hsla(0.0, 0.0, 1.0, 0.9);
                        window.paint_quad(quad(
                            b,
                            r,
                            slot_col(slot).opacity(0.85 * op),
                            px(1.0),
                            rim,
                            BorderStyle::default(),
                        ));
                    } else {
                        box_mesh.slot(slot).rect(cx.pt(c), hw, hh, slot_col(slot).opacity(0.85 * op), window);
                    }
                }
                continue;
            }
            let vertices = f.vertices;
            let compact = f.bbox.width() * scale < COMPACT_PX && f.bbox.height() * scale < COMPACT_PX;
            let geom = core.lod.pick(level, id, &core.prepared.features[id].geom, vertices);
            let color = slot_col(slot);
            for_each_part(&geom, &mut |part| {
                if let Part::Polygon(rings) = part {
                    let rings = polygon_rings(cx, k, rings, &mut buf);
                    if compact
                        && let Some(ext) = rings.first()
                        && let Some(tris) = earclip(ext)
                    {
                        // Small polygon: triangulate ourselves, no outline (it would cover
                        // most of the shape) and skip the general tessellator.
                        let m = box_mesh.slot(slot);
                        let c = color.opacity(0.6 * op);
                        for t in tris {
                            m.triangle(cx.pt(ext[t[0]]), cx.pt(ext[t[1]]), cx.pt(ext[t[2]]), c, window);
                        }
                        return;
                    }
                    if fill_a > 0.0
                        && let Some(path) = fill_path(cx, &rings)
                    {
                        fills.slot(slot).append(path, color.opacity(fill_a), window);
                    }
                    for r in &rings {
                        outlines.add(cx, slot, r, true, color.opacity(op.max(0.3)), window);
                    }
                }
            });
        }
        fills.flush(|s| slot_col(s).opacity(info.fill_alpha_slot(core, s)), window);
        box_mesh.flush(|s| slot_col(s).opacity(0.85 * info.opacity_slot(core, s)), window);
        outlines.flush(|s| slot_col(s).opacity(info.opacity_slot(core, s).max(0.3)), window);

        // Pass 2: lines.
        let mut lines = Strokes::new(info.widths(core, 0.5));
        for &id in visible {
            let f = &core.prepared.features[id];
            if !f.kinds.lines || core.kind(f.layer) != LayerKind::Auto {
                continue;
            }
            let slot = core.slot(id);
            if tiny(f) {
                dots.push((cx.t.apply([f.bbox.center()[0] + k, f.bbox.center()[1]]), slot));
                continue;
            }
            let vertices = f.vertices;
            let op = core.style(f.layer).opacity;
            let geom = core.lod.pick(level, id, &core.prepared.features[id].geom, vertices);
            for_each_part(&geom, &mut |part| {
                if let Part::Line(l) = part {
                    for piece in line_pieces(cx, k, l, &mut buf) {
                        lines.add(cx, slot, &piece, false, slot_col(slot).opacity(op), window);
                    }
                }
            });
        }
        lines.flush(|s| slot_col(s).opacity(info.opacity_slot(core, s)), window);

        // Pass 3: points and collapsed features.
        let outline = hsla(0.0, 0.0, 1.0, 0.95);
        let inside = |c: P| c[0] >= -8.0 && c[0] <= sz[0] + 8.0 && c[1] >= -8.0 && c[1] <= sz[1] + 8.0;
        let mut squares_mesh = Meshes::new(layers);
        for &id in visible {
            let f = &core.prepared.features[id];
            let scatter = core.kind(f.layer) == LayerKind::Points;
            if !f.kinds.points && !scatter {
                continue;
            }
            let slot = core.slot(id);
            let op = core.style(f.layer).opacity;
            let color = slot_col(slot).opacity(op);
            let r = core.ls.radius_of[id];
            let sized = core.style(f.layer).points.radius_by.is_some();
            let mut draw = |c: Coord| {
                let s = cx.t.apply([c[0] + k, c[1]]);
                if !inside(s) {
                    return;
                }
                match cells.as_mut() {
                    Some(cells) => {
                        if cells.claim(f.layer, s) {
                            squares_mesh.slot(slot).square(cx.pt(s), square_half(r, sized, cells.cell()), color, window);
                        }
                    }
                    None => circle(cx, s, r, color, 1.5, outline, window),
                }
            };
            if f.kinds.points && !f.kinds.lines && !f.kinds.polygons {
                for_each_part(&f.geom, &mut |part| {
                    if let Part::Point(c) = part {
                        draw(c);
                    }
                });
            } else if scatter {
                draw(f.bbox.center());
            } else {
                for_each_part(&f.geom, &mut |part| {
                    if let Part::Point(c) = part {
                        draw(c);
                    }
                });
            }
        }
        for (s, slot) in dots {
            if inside(s) && dot_cells.claim(slot, s) {
                squares_mesh.slot(slot).square(cx.pt(s), 1.5, slot_col(slot), window);
            }
        }
        squares_mesh.flush(|s| slot_col(s).opacity(info.opacity_slot(core, s)), window);

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

/// Colours and per-layer drawing parameters of every slot, captured once per frame.
pub(crate) struct SlotInfo {
    rgb: Vec<[u8; 3]>,
    layer: Vec<usize>,
}

impl SlotInfo {
    pub(crate) fn new(core: &Core) -> SlotInfo {
        let rgb = core.slot_rgb();
        let mut layer = vec![0; rgb.len()];
        for (li, r) in core.ls.rt.iter().enumerate() {
            let n = r.coloring.colors.len().max(1);
            for s in r.base_slot..(r.base_slot + n).min(layer.len()) {
                layer[s] = li;
            }
        }
        SlotInfo { rgb, layer }
    }

    pub(crate) fn color(&self, slot: usize) -> Hsla {
        let [r, g, b] = self.rgb[slot.min(self.rgb.len() - 1)];
        Hsla::from(gpui_kit::Rgba { r: f32::from(r) / 255.0, g: f32::from(g) / 255.0, b: f32::from(b) / 255.0, a: 1.0 })
    }

    pub(crate) fn layer_of(&self, slot: usize) -> usize {
        self.layer[slot.min(self.layer.len() - 1)]
    }

    pub(crate) fn opacity_slot(&self, core: &Core, slot: usize) -> f32 {
        core.style(self.layer_of(slot)).opacity
    }

    /// Fill alpha of a slot: translucent by default, stronger when colour carries data.
    pub(crate) fn fill_alpha_slot(&self, core: &Core, slot: usize) -> f32 {
        let l = self.layer_of(slot);
        fill_alpha(core, l)
    }

    /// `(fill alpha, opacity)` of layer `l`.
    pub(crate) fn fill_alpha(&self, core: &Core, l: usize) -> (f32, f32) {
        (fill_alpha(core, l), core.style(l).opacity)
    }

    /// Stroke widths per slot (`extra` is added for lines).
    pub(crate) fn widths(&self, core: &Core, extra: f32) -> Vec<f32> {
        (0..self.rgb.len()).map(|s| core.style(self.layer_of(s)).stroke_width + extra).collect()
    }
}

pub(crate) fn fill_alpha(core: &Core, l: usize) -> f32 {
    let st = core.style(l);
    if !st.fill {
        return 0.0;
    }
    let by = matches!(st.color, crate::style::ColorSpec::ByColumn(_));
    (if by { 0.78 } else { 0.25 }) * st.opacity
}

/// Hover and selection highlights for features of tiled layers (the legacy painter handles
/// the others in its own loop). Uses the full-detail geometry: only a few features are drawn.
pub(crate) fn paint_tiled_highlights(core: &mut Core, cx: &Cx, sz: [f64; 2], window: &mut Window) {
    let mut ids: Vec<(usize, bool)> = Vec::new();
    if let Some(h) = core.hover {
        ids.push((h, false));
    }
    if let Some(row) = core.selected_row {
        ids.extend(core.features_of_row(row).into_iter().map(|i| (i, true)));
    }
    ids.retain(|(i, _)| {
        let l = core.prepared.features[*i].layer;
        core.is_tiled(l) && core.layers.get(l).is_some_and(|l| l.visible)
    });
    if ids.is_empty() {
        return;
    }
    let dark = cx.colors.dark;
    let level = cx.t.lod_level();
    let mut buf: Vec<P> = Vec::new();
    for (k, _) in core.copy_queries(&cx.t, sz, MARGIN) {
        for &(id, selected) in &ids {
            let (layer, vertices) = (core.prepared.features[id].layer, core.prepared.features[id].vertices);
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
                    color: to_hsla(layer_color(layer, dark).brighter(), 1.0),
                    fill_alpha: 0.38,
                    outline: 2.5,
                    line: 3.5,
                    radius: 6.5,
                    halo: None,
                }
            };
            let geom = core.lod.pick(level, id, &core.prepared.features[id].geom, vertices);
            paint_single(cx, k, &geom, &style, sz, &mut buf, window);
        }
    }
}

/// Half-side of the square a dense point is drawn as. Squares are claimed one per 2 px cell, so
/// sizing them to the cell keeps neighbours from covering each other (which would let the
/// colour class painted last win everywhere); only an explicit size-by column makes them larger.
pub(crate) fn square_half(r: f32, sized: bool, cell: f64) -> f32 {
    if sized { (r * 0.6).clamp(1.25, 6.0) } else { (cell / 2.0) as f32 + 0.25 }
}

pub(crate) fn stroke_path(cx: &Cx, pts: &[P], closed: bool, width: f32, color: Hsla, window: &mut Window) {
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

/// Shape and paint one line of text at `origin` (top-left), `size` px tall.
pub(crate) fn paint_text(
    window: &mut Window,
    app: &mut App,
    text: &str,
    origin: Point<Pixels>,
    size_px: f32,
    color: Hsla,
    centered: bool,
) -> Pixels {
    let font = window.text_style().font();
    let run = TextRun { len: text.len(), font, color, background_color: None, underline: None, strikethrough: None };
    let line = window.text_system().shape_line(SharedString::from(text.to_string()), px(size_px), &[run], None);
    let w = line.width;
    let origin = if centered { point(origin.x - w / 2.0, origin.y) } else { origin };
    let _ = line.paint(origin, px(size_px + 3.0), TextAlign::Left, None, window, app);
    w
}
