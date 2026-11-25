//! On-the-fly vector tiling, tippecanoe style.
//!
//! A pyramid of tiles is built lazily, only for what the viewport needs, from the
//! full-detail features and the grid index. A tile `(z, x, y)` holds the features that
//! touch it (plus a pixel buffer), clipped to it and reduced to what a tile of 256 px can
//! show:
//! * lines and polygons are simplified with a tolerance tied to the tile's pixel size;
//! * polygons under a pixel are dropped, but their area accumulates so the tile still
//!   shows one representative for every pixel's worth (tippecanoe's tiny-polygon reduction);
//! * points beyond a budget are thinned on a grid that coarsens until the budget fits,
//!   keeping the first feature per cell, so the result depends only on the data and the
//!   tile, never on how the user got there.
//!
//! Geographic data uses the Web-Mercator quadtree (world fractions `0..1`); planar data
//! uses a square quadtree over the data's own bounding box. Pure functions; the worker
//! pool and cache live at the bottom.

use crate::clip::{clip_polyline, clip_ring};
use crate::index::GridIndex;
use crate::style::{Simplify, TilingOptions};
use crate::world::{Part, Prepared, for_each_part};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use theta_geo::{BBox, Coord, Geometry, simplify};

/// Nominal tile size in px: simplification and tiny-feature thresholds are measured in it.
pub const TILE_PX: f64 = 256.0;
/// Features are clipped this many px beyond the tile so strokes and markers on the edge are whole.
pub const BUFFER_PX: f64 = 8.0;
/// Most tiles requested for one view.
pub const MAX_VISIBLE_TILES: usize = 48;

/// A tile address. `x` and `y` are signed because geographic data may reach past the
/// antimeridian (unwrapped longitudes), which simply lands in tiles outside `0..2^z`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileId {
    /// Zoom: the tile is `1 / 2^z` of the root.
    pub z: u8,
    /// Column.
    pub x: i64,
    /// Row (grows with world y).
    pub y: i64,
}

/// The root square of the pyramid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileSpace {
    /// Lower-left corner in world coordinates.
    pub origin: Coord,
    /// Side length in world units.
    pub size: f64,
}

impl TileSpace {
    /// The Web-Mercator world.
    pub fn geographic() -> TileSpace {
        TileSpace { origin: [0.0, 0.0], size: 1.0 }
    }

    /// A square quadtree over the data of a planar layer.
    pub fn planar(bbox: Option<BBox>) -> TileSpace {
        match bbox {
            Some(b) => TileSpace { origin: b.min, size: b.width().max(b.height()).max(1e-9) },
            None => TileSpace { origin: [0.0, 0.0], size: 1.0 },
        }
    }

    /// World units per pixel of a `TILE_PX` tile at zoom `z`.
    pub fn px_world(&self, z: u8) -> f64 {
        self.size / f64::from(1u32 << z.min(30)) / TILE_PX
    }

    /// World bounds of a tile.
    pub fn bounds(&self, id: TileId) -> BBox {
        let s = self.size / (1u64 << id.z.min(40)) as f64;
        let min = [self.origin[0] + id.x as f64 * s, self.origin[1] + id.y as f64 * s];
        BBox { min, max: [min[0] + s, min[1] + s] }
    }

    /// The zoom whose tiles are nearest 256 px at `px_per_unit`, capped at `max_zoom`.
    pub fn zoom_for(&self, px_per_unit: f64, max_zoom: u8) -> u8 {
        let z = (px_per_unit.abs() * self.size / TILE_PX).max(1.0).log2().round();
        (z as u8).min(max_zoom)
    }

    /// Tiles of zoom `z` touching `rect`, nearest the middle of `rect` first, at most
    /// `MAX_VISIBLE_TILES`.
    pub fn tiles_for(&self, z: u8, rect: &BBox) -> Vec<TileId> {
        let n = (1u64 << z.min(40)) as f64;
        let to = |v: f64, axis: usize| ((v - self.origin[axis]) / self.size * n).floor() as i64;
        // A rectangle that only touches a tile's edge does not need that tile.
        let to_hi = |v: f64, axis: usize, lo: i64| (((v - self.origin[axis]) / self.size * n).ceil() as i64 - 1).max(lo);
        let (x0, y0) = (to(rect.min[0], 0), to(rect.min[1], 1));
        let (x1, y1) = (to_hi(rect.max[0], 0, x0), to_hi(rect.max[1], 1, y0));
        if x1 < x0 || y1 < y0 || (x1 - x0 + 1).saturating_mul(y1 - y0 + 1) > 4096 {
            return Vec::new();
        }
        let (cx, cy) = ((x0 + x1) as f64 / 2.0, (y0 + y1) as f64 / 2.0);
        let mut v: Vec<TileId> =
            (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| TileId { z, x, y })).collect();
        v.sort_by(|a, b| {
            let d = |t: &TileId| (t.x as f64 - cx).hypot(t.y as f64 - cy);
            d(a).total_cmp(&d(b)).then(a.cmp(b))
        });
        v.truncate(MAX_VISIBLE_TILES);
        v
    }
}

impl TileId {
    /// The tile `up` levels above this one, and the part of it that this tile covers as
    /// `(u0, v0, size)` fractions of the ancestor.
    pub fn ancestor(self, up: u8) -> Option<(TileId, [f64; 3])> {
        if up == 0 || up > self.z {
            return None;
        }
        let f = 1i64 << up;
        let anc = TileId { z: self.z - up, x: self.x.div_euclid(f), y: self.y.div_euclid(f) };
        let (rx, ry) = (self.x.rem_euclid(f), self.y.rem_euclid(f));
        Some((anc, [rx as f64 / f as f64, ry as f64 / f as f64, 1.0 / f as f64]))
    }
}

/// Everything that changes what a tile contains.
#[derive(Clone, Debug, PartialEq)]
pub struct TileOpts {
    /// Simplification tolerance in tile px.
    pub detail_px: f64,
    /// Algorithm.
    pub simplify: Simplify,
    /// Points kept per tile.
    pub point_budget: usize,
    /// Drop sub-pixel polygons (accumulating their area).
    pub drop_tiny: bool,
    /// Keep every feature and vertex.
    pub keep_all: bool,
    /// Deepest zoom with its own tiles; tiles at it are kept at full detail.
    pub max_zoom: u8,
}

impl TileOpts {
    /// From a layer's tiling options.
    pub fn from_style(t: &TilingOptions) -> TileOpts {
        TileOpts {
            detail_px: f64::from(t.detail_px),
            simplify: t.simplify,
            point_budget: t.point_budget,
            drop_tiny: t.drop_tiny_polygons,
            keep_all: t.keep_every_feature,
            max_zoom: t.max_zoom,
        }
    }

    /// A stable hash for cache keys.
    pub fn key(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for v in [
            self.detail_px.to_bits(),
            self.simplify as u64,
            self.point_budget as u64,
            u64::from(self.drop_tiny),
            u64::from(self.keep_all),
            u64::from(self.max_zoom),
        ] {
            h = (h ^ v).wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }
}

/// A clipped line or polygon.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// An open line.
    Line(Vec<Coord>),
    /// Rings, exterior first, each closed.
    Poly(Vec<Vec<Coord>>),
}

/// What building a tile kept and dropped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileStats {
    /// Features whose box touched the tile.
    pub candidates: usize,
    /// Features with something in the tile.
    pub kept: usize,
    /// Sub-pixel polygons dropped.
    pub dropped_tiny: usize,
    /// Points dropped by thinning.
    pub dropped_thinned: usize,
    /// Vertices of the clipped input.
    pub vertices_in: usize,
    /// Vertices after simplification.
    pub vertices_out: usize,
}

/// One built tile.
#[derive(Clone, Debug)]
pub struct Tile {
    /// Address.
    #[cfg_attr(not(test), allow(dead_code))]
    pub id: TileId,
    /// Lines and polygons with their feature index.
    pub shapes: Vec<(u32, Shape)>,
    /// Points with their feature index.
    pub points: Vec<(u32, Coord)>,
    /// Sorted unique feature indexes present in the tile (for hit testing).
    pub ids: Vec<u32>,
    /// Counts for the debug overlay.
    pub stats: TileStats,
    /// Approximate memory use, for the cache budget.
    pub bytes: usize,
}

fn ring_area(r: &[Coord]) -> f64 {
    let mut a = 0.0;
    for w in r.windows(2) {
        a += w[0][0] * w[1][1] - w[1][0] * w[0][1];
    }
    (a / 2.0).abs()
}

fn tri_area(a: Coord, b: Coord, c: Coord) -> f64 {
    ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0
}

/// Visvalingam-Whyatt: repeatedly remove the vertex whose triangle with its neighbours has
/// the smallest area, until every remaining triangle is at least `min_area`. Endpoints and
/// at least `min_points` vertices are kept.
pub fn visvalingam(pts: &[Coord], min_area: f64, min_points: usize) -> Vec<Coord> {
    let n = pts.len();
    if n <= min_points.max(2) || min_area <= 0.0 {
        return pts.to_vec();
    }
    let mut prev: Vec<usize> = (0..n).map(|i| i.wrapping_sub(1)).collect();
    let mut next: Vec<usize> = (1..=n).collect();
    let mut alive = vec![true; n];
    let mut area = vec![f64::INFINITY; n];
    // Min-heap on area by ordering the bits of the (non-negative) f64.
    let mut heap = std::collections::BinaryHeap::new();
    let push = |heap: &mut std::collections::BinaryHeap<std::cmp::Reverse<(u64, usize)>>, a: f64, i: usize| {
        heap.push(std::cmp::Reverse((a.to_bits(), i)));
    };
    for i in 1..n - 1 {
        area[i] = tri_area(pts[i - 1], pts[i], pts[i + 1]);
        push(&mut heap, area[i], i);
    }
    let mut count = n;
    while let Some(std::cmp::Reverse((bits, i))) = heap.pop() {
        if !alive[i] || f64::from_bits(bits) != area[i] {
            continue; // stale entry
        }
        if area[i] >= min_area || count <= min_points {
            break;
        }
        alive[i] = false;
        count -= 1;
        let (p, q) = (prev[i], next[i]);
        next[p] = q;
        prev[q] = p;
        for j in [p, q] {
            if j != 0 && j != n - 1 {
                // Never let a neighbour's area drop below the one just removed: that keeps the
                // removal order monotone, as in the original algorithm.
                area[j] = tri_area(pts[prev[j]], pts[j], pts[next[j]]).max(area[i]);
                push(&mut heap, area[j], j);
            }
        }
    }
    pts.iter().zip(alive).filter(|(_, a)| *a).map(|(p, _)| *p).collect()
}

/// Simplify one vertex list with `algo` at tolerance `tol` (world units), keeping at least
/// `min_points` vertices.
fn simplify_path(pts: Vec<Coord>, algo: Simplify, tol: f64, min_points: usize) -> Vec<Coord> {
    if tol <= 0.0 || pts.len() <= min_points {
        return pts;
    }
    match algo {
        Simplify::Off => pts,
        Simplify::Visvalingam => visvalingam(&pts, tol * tol, min_points),
        Simplify::DouglasPeucker => {
            let g = if min_points >= 4 { Geometry::Polygon(vec![pts]) } else { Geometry::LineString(pts) };
            match simplify(&g, tol) {
                Geometry::Polygon(mut r) => r.remove(0),
                Geometry::LineString(l) => l,
                _ => Vec::new(),
            }
        }
    }
}

fn close(mut r: Vec<Coord>) -> Vec<Coord> {
    if r.first() != r.last()
        && let Some(f) = r.first().copied()
    {
        r.push(f);
    }
    r
}

/// Cell counts per side for thinning, finest first (steps of sqrt 2).
fn thinning_resolutions() -> Vec<usize> {
    (0..16).map(|j| ((1024.0 / 2f64.powf(j as f64 / 2.0)).round() as usize).max(1)).collect()
}

/// Keep at most about `budget` of `pts` (feature index, coordinate), spread evenly over `area`:
/// the first point in input order claims each cell of the finest grid whose occupied-cell
/// count fits the budget. Returns the kept points in input order.
pub fn thin_points(pts: &[(u32, Coord)], area: &BBox, budget: usize) -> Vec<(u32, Coord)> {
    if pts.len() <= budget {
        return pts.to_vec();
    }
    let (w, h) = (area.width().max(f64::MIN_POSITIVE), area.height().max(f64::MIN_POSITIVE));
    let cell_of = |c: Coord, n: usize| {
        let cx = (((c[0] - area.min[0]) / w * n as f64) as isize).clamp(0, n as isize - 1) as usize;
        let cy = (((c[1] - area.min[1]) / h * n as f64) as isize).clamp(0, n as isize - 1) as usize;
        cy * n + cx
    };
    let mut chosen = 1;
    let mut seen: Vec<u64> = Vec::new();
    for n in thinning_resolutions() {
        chosen = n;
        seen.clear();
        seen.resize((n * n).div_ceil(64), 0);
        let mut occupied = 0usize;
        for (_, c) in pts {
            let i = cell_of(*c, n);
            let (word, bit) = (i / 64, 1u64 << (i % 64));
            if seen[word] & bit == 0 {
                seen[word] |= bit;
                occupied += 1;
            }
        }
        if occupied <= budget {
            break;
        }
    }
    seen.clear();
    seen.resize((chosen * chosen).div_ceil(64), 0);
    let mut out = Vec::new();
    for p in pts {
        let i = cell_of(p.1, chosen);
        let (word, bit) = (i / 64, 1u64 << (i % 64));
        if seen[word] & bit == 0 {
            seen[word] |= bit;
            out.push(*p);
        }
    }
    out
}

/// Build one tile of `layer`. Features come from `index` (candidates by bounding box) in
/// ascending feature order, which is what makes the result deterministic.
pub fn build_tile(
    prep: &Prepared,
    index: &GridIndex,
    layer: usize,
    space: &TileSpace,
    id: TileId,
    o: &TileOpts,
) -> Tile {
    let pxw = space.px_world(id.z);
    let bounds = space.bounds(id);
    let buf = BUFFER_PX * pxw;
    let area = BBox {
        min: [bounds.min[0] - buf, bounds.min[1] - buf],
        max: [bounds.max[0] + buf, bounds.max[1] + buf],
    };
    let rect = [area.min[0], area.min[1], area.max[0], area.max[1]];
    let full_detail = o.keep_all || id.z >= o.max_zoom;
    let tol = if full_detail || o.simplify == Simplify::Off { 0.0 } else { o.detail_px * pxw };

    let mut ids = Vec::new();
    index.query_into(&area, &mut ids);
    let mut stats = TileStats::default();
    let mut shapes: Vec<(u32, Shape)> = Vec::new();
    let mut points: Vec<(u32, Coord)> = Vec::new();
    let mut tiny_accum = 0.0f64;
    let mut present: Vec<u32> = Vec::new();

    for fid in ids {
        let f = &prep.features[fid as usize];
        if f.layer != layer {
            continue;
        }
        stats.candidates += 1;
        let before = (shapes.len(), points.len());
        // A feature that is sub-pixel and polygonal is a candidate for tiny-polygon reduction.
        let tiny_poly = o.drop_tiny
            && !full_detail
            && f.kinds.polygons
            && !f.kinds.points
            && !f.kinds.lines
            && f.bbox.width() / pxw * (f.bbox.height() / pxw) < 4.0;
        for_each_part(&f.geom, &mut |part| match part {
            Part::Point(c) => {
                if c[0] >= area.min[0] && c[0] <= area.max[0] && c[1] >= area.min[1] && c[1] <= area.max[1] {
                    points.push((fid, c));
                }
            }
            Part::Line(l) => {
                if l.iter().all(|c| c[0] < rect[0]) || l.iter().all(|c| c[0] > rect[2]) {
                    return;
                }
                stats.vertices_in += l.len();
                for piece in clip_polyline(l, &rect) {
                    let piece = simplify_path(piece, o.simplify, tol, 2);
                    if piece.len() >= 2 {
                        stats.vertices_out += piece.len();
                        shapes.push((fid, Shape::Line(piece)));
                    }
                }
            }
            Part::Polygon(rings) => {
                if tiny_poly {
                    let a = ring_area(&rings[0]) / (pxw * pxw);
                    tiny_accum += a;
                    if tiny_accum < 1.0 {
                        stats.dropped_tiny += 1;
                        return;
                    }
                    tiny_accum -= 1.0;
                }
                if rings.is_empty() {
                    return;
                }
                stats.vertices_in += rings.iter().map(Vec::len).sum::<usize>();
                let mut out: Vec<Vec<Coord>> = Vec::with_capacity(rings.len());
                for (i, r) in rings.iter().enumerate() {
                    let clipped = clip_ring(r, &rect);
                    if clipped.len() < 3 {
                        if i == 0 {
                            return;
                        }
                        continue;
                    }
                    let simple = simplify_path(close(clipped), o.simplify, tol, 4);
                    // A hole that collapsed to a sliver is not worth drawing; a collapsed
                    // exterior is the whole polygon being sub-pixel.
                    if simple.len() < 4 || (i > 0 && tol > 0.0 && ring_area(&simple) < tol * tol) {
                        if i == 0 {
                            return;
                        }
                        continue;
                    }
                    out.push(simple);
                }
                if !out.is_empty() {
                    stats.vertices_out += out.iter().map(Vec::len).sum::<usize>();
                    shapes.push((fid, Shape::Poly(out)));
                }
            }
        });
        if (shapes.len(), points.len()) != before {
            present.push(fid);
        }
    }

    let n_points = points.len();
    if !o.keep_all {
        points = thin_points(&points, &area, o.point_budget);
    }
    stats.dropped_thinned = n_points - points.len();
    // Feature ids that survived: thinning may have removed all points of a feature.
    let mut kept: Vec<u32> = shapes.iter().map(|(i, _)| *i).chain(points.iter().map(|(i, _)| *i)).collect();
    kept.sort_unstable();
    kept.dedup();
    stats.kept = kept.len();
    let _ = present;
    let bytes = 128
        + points.len() * 24
        + kept.len() * 4
        + shapes
            .iter()
            .map(|(_, s)| match s {
                Shape::Line(l) => 40 + l.len() * 16,
                Shape::Poly(r) => 40 + r.iter().map(|r| 24 + r.len() * 16).sum::<usize>(),
            })
            .sum::<usize>();
    Tile { id, shapes, points, ids: kept, stats, bytes }
}

// ---- cache ---------------------------------------------------------------------------

/// A tile in the cache: which layer, which option set, which address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileKey {
    /// Layer index.
    pub layer: usize,
    /// `TileOpts::key`.
    pub opts: u64,
    /// Address.
    pub id: TileId,
}

/// Built tiles, least recently used evicted once their total size passes a budget.
pub struct TileCache {
    map: HashMap<TileKey, Arc<Tile>>,
    order: VecDeque<TileKey>,
    bytes: usize,
    cap: usize,
}

/// Default budget: 256 MB of tiles.
pub const CACHE_BYTES: usize = 256 * 1024 * 1024;

impl TileCache {
    /// An empty cache of at most `cap` bytes (a single tile larger than that is still kept).
    pub fn new(cap: usize) -> Self {
        TileCache { map: HashMap::new(), order: VecDeque::new(), bytes: 0, cap }
    }

    /// Total bytes held.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Number of tiles.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether `k` is cached.
    pub fn contains(&self, k: &TileKey) -> bool {
        self.map.contains_key(k)
    }

    /// Look up without touching recency.
    pub fn peek(&self, k: &TileKey) -> Option<&Arc<Tile>> {
        self.map.get(k)
    }

    /// Mark `k` most recently used.
    pub fn touch(&mut self, k: &TileKey) {
        if self.map.contains_key(k) {
            if let Some(pos) = self.order.iter().position(|x| x == k) {
                self.order.remove(pos);
            }
            self.order.push_back(*k);
        }
    }

    /// Insert, evicting from the old end while over budget.
    pub fn insert(&mut self, k: TileKey, t: Arc<Tile>) {
        if let Some(old) = self.map.insert(k, t.clone()) {
            self.bytes -= old.bytes;
            if let Some(pos) = self.order.iter().position(|x| *x == k) {
                self.order.remove(pos);
            }
        }
        self.bytes += t.bytes;
        self.order.push_back(k);
        while self.bytes > self.cap && self.order.len() > 1 {
            if let Some(old) = self.order.pop_front()
                && let Some(t) = self.map.remove(&old)
            {
                self.bytes -= t.bytes;
            }
        }
    }

    /// Forget everything.
    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
        self.bytes = 0;
    }
}

// ---- worker pool ---------------------------------------------------------------------

/// The immutable data tiles are built from; replaced wholesale when the result changes.
pub struct TileSource {
    /// Counter distinguishing data sets, so a late result for an old one is ignored.
    pub generation: u64,
    /// The projected features.
    pub prepared: Arc<Prepared>,
    /// Their grid index.
    pub index: Arc<GridIndex>,
    /// The pyramid root.
    pub space: TileSpace,
}

/// A tile to build.
pub struct TileJob {
    /// Cache key.
    pub key: TileKey,
    /// What to build from.
    pub source: Arc<TileSource>,
    /// Options.
    pub opts: TileOpts,
}

/// A finished tile.
pub struct TileDone {
    /// Key it was built for.
    pub key: TileKey,
    /// The data set it belongs to.
    pub generation: u64,
    /// The tile.
    pub tile: Arc<Tile>,
}

struct Queue {
    wanted: Vec<TileJob>,
    in_flight: HashSet<TileKey>,
    shutdown: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
    results: async_channel::Sender<TileDone>,
}

/// Builds tiles on worker threads. Like the basemap loader, the queue is *replaced* each
/// frame, which is how work for tiles that scrolled away is cancelled.
pub struct TilePool {
    shared: Arc<Shared>,
    /// Finished tiles; awaited by a task in the panel.
    pub results: async_channel::Receiver<TileDone>,
    started: bool,
}

/// Worker threads; one core is left for the UI.
const WORKERS: usize = 3;

impl TilePool {
    /// A pool with no threads yet (they start on first use).
    pub fn new() -> Self {
        let (tx, rx) = async_channel::unbounded();
        TilePool {
            shared: Arc::new(Shared {
                queue: Mutex::new(Queue { wanted: Vec::new(), in_flight: HashSet::new(), shutdown: false }),
                wake: Condvar::new(),
                results: tx,
            }),
            results: rx,
            started: false,
        }
    }

    /// Replace the queue with `jobs` (most wanted first).
    pub fn set_wanted(&mut self, jobs: Vec<TileJob>) {
        if !self.started && !jobs.is_empty() {
            self.started = true;
            for i in 0..WORKERS {
                let shared = self.shared.clone();
                let _ = std::thread::Builder::new().name(format!("theta-vtiles-{i}")).spawn(move || worker(shared));
            }
        }
        let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        q.wanted = jobs.into_iter().filter(|j| !q.in_flight.contains(&j.key)).collect();
        drop(q);
        self.shared.wake.notify_all();
    }
}

impl Default for TilePool {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TilePool {
    fn drop(&mut self) {
        if let Ok(mut q) = self.shared.queue.lock() {
            q.shutdown = true;
            q.wanted.clear();
        }
        self.shared.wake.notify_all();
    }
}

fn worker(shared: Arc<Shared>) {
    loop {
        let job = {
            let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if q.shutdown {
                    return;
                }
                let pos = q.wanted.iter().position(|j| !q.in_flight.contains(&j.key));
                if let Some(pos) = pos {
                    let job = q.wanted.remove(pos);
                    q.in_flight.insert(job.key);
                    break job;
                }
                q = shared.wake.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        let s = &job.source;
        let tile = build_tile(&s.prepared, &s.index, job.key.layer, &s.space, job.key.id, &job.opts);
        if let Ok(mut q) = shared.queue.lock() {
            q.in_flight.remove(&job.key);
        }
        let done = TileDone { key: job.key, generation: s.generation, tile: Arc::new(tile) };
        if shared.results.send_blocking(done).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MapFeature;
    use crate::world::prepare;
    use theta_geo::Feature;

    fn mf(g: Geometry) -> MapFeature {
        MapFeature { feature: Feature::new(g, Some(4326)), row: 0, label: None, properties: vec![], values: vec![], layer: 0 }
    }

    fn opts() -> TileOpts {
        TileOpts::from_style(&TilingOptions::default())
    }

    fn setup(feats: Vec<MapFeature>) -> (Prepared, GridIndex) {
        let p = prepare(&feats);
        let idx = GridIndex::new(&p.features.iter().map(|f| f.bbox).collect::<Vec<_>>());
        (p, idx)
    }

    fn lonlat_poly(x0: f64, y0: f64, x1: f64, y1: f64) -> Geometry {
        Geometry::Polygon(vec![vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]]])
    }

    #[test]
    fn tile_bounds_and_ids() {
        let s = TileSpace::geographic();
        let b = s.bounds(TileId { z: 2, x: 1, y: 3 });
        assert_eq!(b.min, [0.25, 0.75]);
        assert_eq!(b.max, [0.5, 1.0]);
        let t = s.tiles_for(1, &BBox { min: [0.1, 0.1], max: [0.9, 0.2] });
        assert_eq!(t.len(), 2);
        assert!(s.tiles_for(40, &BBox { min: [0.0, 0.0], max: [1.0, 1.0] }).is_empty());
    }

    #[test]
    fn zoom_choice_follows_scale_and_caps() {
        let s = TileSpace::geographic();
        assert_eq!(s.zoom_for(256.0, 14), 0);
        assert_eq!(s.zoom_for(256.0 * 1024.0, 14), 10);
        assert_eq!(s.zoom_for(256.0 * 1e9, 14), 14);
    }

    #[test]
    fn ancestor_region() {
        let t = TileId { z: 3, x: 5, y: 2 };
        let (a, r) = t.ancestor(1).unwrap();
        assert_eq!(a, TileId { z: 2, x: 2, y: 1 });
        assert_eq!(r, [0.5, 0.0, 0.5]);
        assert!(t.ancestor(4).is_none());
    }

    #[test]
    fn polygon_is_clipped_to_tile_plus_buffer() {
        // A polygon covering the whole world; tile (2,1,1) must hold it clipped to its box + buffer.
        let (p, idx) = setup(vec![mf(lonlat_poly(-170.0, -80.0, 170.0, 80.0))]);
        let s = TileSpace::geographic();
        let id = TileId { z: 2, x: 1, y: 1 };
        let t = build_tile(&p, &idx, 0, &s, id, &opts());
        assert_eq!(t.shapes.len(), 1);
        let Shape::Poly(r) = &t.shapes[0].1 else { panic!() };
        let b = s.bounds(id);
        let buf = BUFFER_PX * s.px_world(2);
        for c in &r[0] {
            assert!(c[0] >= b.min[0] - buf - 1e-12 && c[0] <= b.max[0] + buf + 1e-12, "{c:?}");
            assert!(c[1] >= b.min[1] - buf - 1e-12 && c[1] <= b.max[1] + buf + 1e-12, "{c:?}");
        }
        // The buffer is really there: the clipped ring extends past the tile edge.
        assert!(r[0].iter().any(|c| c[0] > b.max[0] || c[0] < b.min[0]));
    }

    #[test]
    fn polygon_spanning_many_tiles_appears_in_all_of_them_and_nowhere_else() {
        let (p, idx) = setup(vec![mf(lonlat_poly(-100.0, -40.0, 100.0, 40.0)), mf(Geometry::Point(Some([170.0, 70.0])))]);
        let s = TileSpace::geographic();
        let mut with = 0;
        for y in 0..8 {
            for x in 0..8 {
                let t = build_tile(&p, &idx, 0, &s, TileId { z: 3, x, y }, &opts());
                let has_poly = t.shapes.iter().any(|(i, _)| *i == 0);
                let b = s.bounds(t.id);
                let buf = BUFFER_PX * s.px_world(3);
                let in_lon = b.max[0] + buf > (-100.0 + 180.0) / 360.0 && b.min[0] - buf < (100.0 + 180.0) / 360.0;
                let lat = |l: f64| crate::world::lat_to_frac(l);
                // The tile is clipped with an 8 px buffer, so features just outside it still appear.
                let buf = BUFFER_PX * s.px_world(3);
                let in_lat = b.max[1] + buf > lat(40.0) && b.min[1] - buf < lat(-40.0);
                assert_eq!(has_poly, in_lon && in_lat, "tile {x},{y} {:?} stats {:?} bounds {:?}", t.shapes.len(), t.stats, b);
                with += usize::from(has_poly);
            }
        }
        assert!(with > 20);
    }

    #[test]
    fn polygon_around_the_tile_without_a_vertex_inside_is_kept() {
        let (p, idx) = setup(vec![mf(lonlat_poly(-170.0, -80.0, 170.0, 80.0))]);
        let t = build_tile(&p, &idx, 0, &TileSpace::geographic(), TileId { z: 6, x: 32, y: 32 }, &opts());
        assert_eq!(t.shapes.len(), 1, "a fully covered tile must still be filled");
    }

    #[test]
    fn lines_are_clipped_and_simplified_by_detail() {
        let pts: Vec<Coord> = (0..2000).map(|i| [i as f64 * 0.005 - 5.0, 50.0 + if i % 2 == 0 { 0.0 } else { 1e-7 }]).collect();
        let (p, idx) = setup(vec![mf(Geometry::LineString(pts))]);
        let s = TileSpace::geographic();
        let id = TileId { z: 5, x: 16, y: 10 };
        let simple = build_tile(&p, &idx, 0, &s, id, &opts());
        let full = build_tile(&p, &idx, 0, &s, id, &TileOpts { simplify: Simplify::Off, ..opts() });
        assert!(simple.stats.vertices_out < full.stats.vertices_out / 10, "{:?} vs {:?}", simple.stats, full.stats);
        let vw = build_tile(&p, &idx, 0, &s, id, &TileOpts { simplify: Simplify::Visvalingam, ..opts() });
        assert!(vw.stats.vertices_out < full.stats.vertices_out / 10);
    }

    #[test]
    fn max_zoom_is_full_detail_and_deeper_zooms_are_not_built() {
        let pts: Vec<Coord> = (0..500).map(|i| [i as f64 * 1e-4, 50.0 + if i % 2 == 0 { 0.0 } else { 1e-8 }]).collect();
        let (p, idx) = setup(vec![mf(Geometry::LineString(pts))]);
        let s = TileSpace::geographic();
        let o = TileOpts { max_zoom: 6, ..opts() };
        assert_eq!(s.zoom_for(256.0 * 2f64.powi(20), o.max_zoom), 6, "overzoom reuses the deepest tile");
        let id = TileId { z: 6, x: 32, y: 21 };
        let t = build_tile(&p, &idx, 0, &s, id, &o);
        assert_eq!(t.stats.vertices_out, t.stats.vertices_in, "no simplification at the maximum zoom");
    }

    #[test]
    fn tiny_polygons_are_reduced_but_not_all_dropped() {
        // 400 sub-pixel rectangles at z=4 (a pixel there is ~0.088 degrees): each about 0.1 px squared.
        let feats: Vec<MapFeature> = (0..400)
            .map(|i| {
                let (x, y) = (10.0 + (i % 20) as f64 * 0.04, 45.0 + (i / 20) as f64 * 0.03);
                mf(lonlat_poly(x, y, x + 0.03, y + 0.02))
            })
            .collect();
        let (p, idx) = setup(feats);
        let s = TileSpace::geographic();
        let id = s.tiles_for(4, &p.bbox.unwrap())[0];
        let on = build_tile(&p, &idx, 0, &s, id, &opts());
        let off = build_tile(&p, &idx, 0, &s, id, &TileOpts { drop_tiny: false, ..opts() });
        let keep = build_tile(&p, &idx, 0, &s, id, &TileOpts { keep_all: true, ..opts() });
        assert_eq!(off.shapes.len(), 400);
        assert_eq!(keep.shapes.len(), 400);
        assert!(on.shapes.len() < 400 && !on.shapes.is_empty(), "{}", on.shapes.len());
        assert_eq!(on.stats.dropped_tiny + on.shapes.len(), 400);
    }

    #[test]
    fn point_budget_is_respected_and_thinning_is_deterministic() {
        let mut s = 7u64;
        let mut rnd = move || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (s >> 33) as f64 / (1u64 << 31) as f64
        };
        let feats: Vec<MapFeature> = (0..30_000).map(|_| mf(Geometry::Point(Some([rnd() * 20.0, 40.0 + rnd() * 10.0])))).collect();
        let (p, idx) = setup(feats);
        let s = TileSpace::geographic();
        let o = TileOpts { point_budget: 2_000, ..opts() };
        let id = s.tiles_for(3, &p.bbox.unwrap())[0];
        let a = build_tile(&p, &idx, 0, &s, id, &o);
        let b = build_tile(&p, &idx, 0, &s, id, &o);
        assert!(a.points.len() <= 2_000, "{}", a.points.len());
        assert!(a.points.len() > 500, "thinning must not be absurdly aggressive: {}", a.points.len());
        assert_eq!(a.points, b.points);
        assert_eq!(a.stats.dropped_thinned + a.points.len(), a.stats.candidates);
        let all = build_tile(&p, &idx, 0, &s, id, &TileOpts { keep_all: true, ..o });
        assert_eq!(all.points.len(), all.stats.candidates);
    }

    #[test]
    fn thinning_is_independent_of_how_much_else_is_in_the_view() {
        // The same tile built from a data set with extra far-away points is identical.
        let base: Vec<MapFeature> = (0..5000).map(|i| mf(Geometry::Point(Some([i as f64 * 0.001, 45.0 + (i % 7) as f64 * 0.01])))).collect();
        let mut more = base.clone();
        more.push(mf(Geometry::Point(Some([-120.0, -30.0]))));
        let s = TileSpace::geographic();
        let o = TileOpts { point_budget: 500, ..opts() };
        let (p1, i1) = setup(base);
        let (p2, i2) = setup(more);
        let id = s.tiles_for(5, &p1.bbox.unwrap())[0];
        assert_eq!(build_tile(&p1, &i1, 0, &s, id, &o).points, build_tile(&p2, &i2, 0, &s, id, &o).points);
    }

    #[test]
    fn world_wrap_features_past_the_antimeridian_land_in_tiles_beyond_the_root() {
        // A polygon from lon 170 to 190 (unwrapped, as the world module keeps it).
        let g = lonlat_poly(170.0, 0.0, 190.0, 10.0);
        let (p, idx) = setup(vec![mf(g)]);
        let s = TileSpace::geographic();
        let inside = build_tile(&p, &idx, 0, &s, TileId { z: 3, x: 7, y: 3 }, &opts());
        let beyond = build_tile(&p, &idx, 0, &s, TileId { z: 3, x: 8, y: 3 }, &opts());
        assert_eq!(inside.shapes.len(), 1);
        assert_eq!(beyond.shapes.len(), 1, "the part east of lon 180 is in the tile past the edge");
        let wrapped = build_tile(&p, &idx, 0, &s, TileId { z: 3, x: 0, y: 3 }, &opts());
        assert!(wrapped.shapes.is_empty(), "wrapping is the renderer's job (world copies), not the tiler's");
    }

    #[test]
    fn planar_space_covers_the_data_box() {
        let s = TileSpace::planar(Some(BBox { min: [1000.0, 2000.0], max: [1400.0, 2100.0] }));
        assert_eq!(s.origin, [1000.0, 2000.0]);
        assert_eq!(s.size, 400.0);
        let b = s.bounds(TileId { z: 1, x: 1, y: 0 });
        assert_eq!(b.min, [1200.0, 2000.0]);
        assert_eq!(s.tiles_for(0, &BBox { min: [1000.0, 2000.0], max: [1400.0, 2100.0] }).len(), 1);
    }

    #[test]
    fn visvalingam_keeps_endpoints_and_sharp_corners() {
        let pts = vec![[0.0, 0.0], [1.0, 0.001], [2.0, 0.0], [3.0, 5.0], [4.0, 0.0]];
        let v = visvalingam(&pts, 0.1, 2);
        assert_eq!(v.first(), Some(&[0.0, 0.0]));
        assert_eq!(v.last(), Some(&[4.0, 0.0]));
        assert!(v.contains(&[3.0, 5.0]));
        assert!(!v.contains(&[1.0, 0.001]));
        assert_eq!(visvalingam(&pts, 0.0, 2), pts);
        assert_eq!(visvalingam(&pts, 1e9, 4).len(), 4, "min_points is honoured");
    }

    #[test]
    fn thin_points_unit() {
        let area = BBox { min: [0.0, 0.0], max: [1.0, 1.0] };
        let pts: Vec<(u32, Coord)> = (0..10_000).map(|i| (i as u32, [(i % 100) as f64 / 100.0, (i / 100) as f64 / 100.0])).collect();
        let k = thin_points(&pts, &area, 900);
        assert!(k.len() <= 900 && k.len() > 300, "{}", k.len());
        assert!(k.windows(2).all(|w| w[0].0 < w[1].0), "input order is kept");
        assert_eq!(thin_points(&pts, &area, 10_000).len(), 10_000);
    }

    #[test]
    fn cache_evicts_least_recently_used_by_bytes() {
        let tile = |bytes| Arc::new(Tile { id: TileId { z: 0, x: 0, y: 0 }, shapes: vec![], points: vec![], ids: vec![], stats: TileStats::default(), bytes });
        let key = |x| TileKey { layer: 0, opts: 0, id: TileId { z: 1, x, y: 0 } };
        let mut c = TileCache::new(250);
        c.insert(key(0), tile(100));
        c.insert(key(1), tile(100));
        c.touch(&key(0));
        c.insert(key(2), tile(100));
        assert!(c.contains(&key(0)) && c.contains(&key(2)) && !c.contains(&key(1)));
        assert!(c.bytes() <= 250);
        // One oversized tile is still kept.
        c.insert(key(3), tile(1000));
        assert!(c.contains(&key(3)));
    }

    #[test]
    fn opts_key_changes_with_every_option() {
        let a = opts();
        for b in [
            TileOpts { detail_px: 1.0, ..opts() },
            TileOpts { simplify: Simplify::Off, ..opts() },
            TileOpts { point_budget: 5, ..opts() },
            TileOpts { drop_tiny: false, ..opts() },
            TileOpts { keep_all: true, ..opts() },
            TileOpts { max_zoom: 3, ..opts() },
        ] {
            assert_ne!(a.key(), b.key());
        }
        assert_eq!(a.key(), opts().key());
    }
}
