//! Aggregation layers: heatmap (kernel density), hexagon and grid bins, and clusters.
//!
//! All pure functions over a [`PointSet`] (one representative point per feature: the point
//! itself, or the centre of a line's or polygon's box) so they can run on a worker thread
//! and be tested without a window. The expensive part of each is done once per zoom level
//! and cached by the worker (`FineGrid`, cluster levels), so panning at a zoom is cheap.

use crate::camera::Transform;
use crate::classify;
use crate::model::MapFeature;
use crate::style::{BinOptions, BinValue, HeatmapOptions, LayerStyle};
use crate::world::{Part, Prepared, for_each_part};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use theta_geo::{BBox, Coord};

/// Raster pixels per two screen pixels' worth: the heatmap is computed at half resolution.
pub const HEAT_RES: f64 = 2.0;

/// Representative points of one layer's features.
#[derive(Clone, Debug, Default)]
pub struct PointSet {
    /// World coordinates.
    pub xy: Vec<Coord>,
    /// Index into `Prepared::features`.
    pub feat: Vec<u32>,
}

/// Build the point set of `layer`: every point vertex, or the box centre of lines and polygons.
pub fn point_set(prep: &Prepared, layer: usize) -> PointSet {
    let mut s = PointSet::default();
    for (i, f) in prep.features.iter().enumerate() {
        if f.layer != layer {
            continue;
        }
        if f.kinds.points && !f.kinds.lines && !f.kinds.polygons {
            for_each_part(&f.geom, &mut |p| {
                if let Part::Point(c) = p {
                    s.xy.push(c);
                    s.feat.push(i as u32);
                }
            });
        } else {
            s.xy.push(f.bbox.center());
            s.feat.push(i as u32);
        }
    }
    s
}

/// Numeric weights of `col` for each point (`NaN` for NULL and non-numbers).
pub fn weights(src: &[MapFeature], prep: &Prepared, set: &PointSet, col: usize) -> Vec<f32> {
    set.feat
        .iter()
        .map(|f| {
            let s = &src[prep.features[*f as usize].src];
            s.values.get(col).and_then(|v| v.num()).map_or(f32::NAN, |x| x as f32)
        })
        .collect()
}

/// What the view looks like when an aggregation is computed.
#[derive(Clone, Debug)]
pub struct AggView {
    /// World to screen.
    pub transform: Transform,
    /// Viewport size in px.
    pub size: [f64; 2],
    /// World copies to draw: offset `k` and the data-space rectangle visible in that copy
    /// (`Core::copy_queries`). Data is never drawn outside its copy's rectangle, so a zoomed-out
    /// world shows each feature once.
    pub copies: Vec<(f64, BBox)>,
}

// ---- heatmap -------------------------------------------------------------------------

/// Points pre-binned into cells of about one pixel at an integer zoom level.
#[derive(Clone, Debug)]
pub struct FineGrid {
    /// Cell side in world units.
    pub cell: f64,
    /// Occupied cells sorted by `(cx, cy)`, with summed weight.
    pub bins: Vec<Bin>,
}

/// One occupied fine cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bin {
    /// Cell column.
    pub cx: i32,
    /// Cell row.
    pub cy: i32,
    /// Summed weight (1 per point when unweighted).
    pub w: f32,
}

fn cell_index(v: f64, cell: f64) -> i32 {
    (v / cell).floor().clamp(-1.0e9, 1.0e9) as i32
}

/// Bin `pts` for zoom `level`: O(n) to key plus one sort. NaN weights count as zero.
pub fn fine_grid(pts: &PointSet, w: Option<&[f32]>, level: i32) -> FineGrid {
    let level = level.clamp(-30, 30);
    let cell = 2f64.powi(-level);
    let mut v: Vec<Bin> = pts
        .xy
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let wt = w.map_or(1.0, |w| if w[i].is_nan() { 0.0 } else { w[i] });
            Bin { cx: cell_index(c[0], cell), cy: cell_index(c[1], cell), w: wt }
        })
        .collect();
    v.sort_unstable_by_key(|b| (b.cx, b.cy));
    let mut bins: Vec<Bin> = Vec::with_capacity(v.len() / 2 + 1);
    for b in v {
        match bins.last_mut() {
            Some(l) if l.cx == b.cx && l.cy == b.cy => l.w += b.w,
            _ => bins.push(b),
        }
    }
    FineGrid { cell, bins }
}

/// The computed heatmap image.
#[derive(Clone, Debug)]
pub struct HeatRaster {
    /// Width in raster px.
    pub w: usize,
    /// Height in raster px.
    pub h: usize,
    /// Premultiplied BGRA, row-major.
    pub bgra: Vec<u8>,
    /// World coordinates of the raster's top-left corner.
    pub origin: Coord,
    /// World size of one raster px.
    pub px_world: [f64; 2],
    /// The density that mapped to the top of the ramp.
    #[cfg_attr(not(test), allow(dead_code))]
    pub max_density: f32,
}

fn gaussian_kernel(sigma: f64) -> Vec<f32> {
    let half = (sigma * 3.0).ceil().max(1.0) as isize;
    let mut k: Vec<f32> = (-half..=half).map(|i| (-((i * i) as f64) / (2.0 * sigma * sigma)).exp() as f32).collect();
    let sum: f32 = k.iter().sum();
    k.iter_mut().for_each(|x| *x /= sum);
    k
}

fn blur(src: &[f32], w: usize, h: usize, kernel: &[f32], cancel: &dyn Fn() -> bool) -> Option<Vec<f32>> {
    let half = (kernel.len() / 2) as isize;
    let mut tmp = vec![0f32; w * h];
    for y in 0..h {
        if y % 32 == 0 && cancel() {
            return None;
        }
        let row = &src[y * w..(y + 1) * w];
        // Skip empty rows quickly: most of a sparse map is empty.
        if row.iter().all(|v| *v == 0.0) {
            continue;
        }
        for x in 0..w {
            let mut a = 0.0;
            for (ki, kv) in kernel.iter().enumerate() {
                let sx = x as isize + ki as isize - half;
                if sx >= 0 && (sx as usize) < w {
                    a += row[sx as usize] * kv;
                }
            }
            tmp[y * w + x] = a;
        }
    }
    let mut out = vec![0f32; w * h];
    for x in 0..w {
        if x % 32 == 0 && cancel() {
            return None;
        }
        for y in 0..h {
            let mut a = 0.0;
            for (ki, kv) in kernel.iter().enumerate() {
                let sy = y as isize + ki as isize - half;
                if sy >= 0 && (sy as usize) < h {
                    a += tmp[sy as usize * w + x] * kv;
                }
            }
            out[y * w + x] = a;
        }
    }
    Some(out)
}

/// Map a normalised density to a premultiplied BGRA pixel (transparent below `threshold`).
pub fn heat_color(t: f32, o: &HeatmapOptions, opacity: f32) -> [u8; 4] {
    let t = t.clamp(0.0, 1.0);
    if t <= o.threshold || t <= 0.0 {
        return [0; 4];
    }
    // Fade in over the first part of the ramp so the edge of a hot spot is soft, not a contour
    // (and ramps that start near black do not leave a dark ring on a dark map).
    let a = (((t - o.threshold) / 0.4).clamp(0.0, 1.0)).powf(1.2) * 0.92 * opacity;
    let [r, g, b] = o.palette.sample(t, o.reversed);
    let p = |c: u8| (f32::from(c) * a).round() as u8;
    [p(b), p(g), p(r), (a * 255.0).round() as u8]
}

/// Gaussian kernel density of `fine` in screen space, at half the viewport resolution.
/// `None` when `cancel` fired.
pub fn kde(fine: &FineGrid, view: &AggView, o: &HeatmapOptions, opacity: f32, cancel: &dyn Fn() -> bool) -> Option<HeatRaster> {
    let (rw, rh) = ((view.size[0] / HEAT_RES).ceil().max(1.0) as usize, (view.size[1] / HEAT_RES).ceil().max(1.0) as usize);
    let sigma_screen = f64::from(o.radius_px) / 2.0;
    let sigma = (sigma_screen / HEAT_RES).max(0.5);
    let margin = sigma_screen * 3.0;
    let t = &view.transform;
    let mut acc = vec![0f32; rw * rh];
    let mw = margin / t.scale[0].abs().max(1e-300);
    for (k, r) in &view.copies {
        let k = *k;
        let (x0, x1) = (cell_index(r.min[0] - mw, fine.cell), cell_index(r.max[0] + mw, fine.cell));
        let (y0, y1) = (cell_index(r.min[1] - mw, fine.cell), cell_index(r.max[1] + mw, fine.cell));
        let start = fine.bins.partition_point(|b| b.cx < x0);
        for (n, b) in fine.bins[start..].iter().enumerate() {
            if b.cx > x1 {
                break;
            }
            if n % 65536 == 0 && cancel() {
                return None;
            }
            if b.cy < y0 || b.cy > y1 || b.w == 0.0 {
                continue;
            }
            let s = t.apply([(f64::from(b.cx) + 0.5) * fine.cell + k, (f64::from(b.cy) + 0.5) * fine.cell]);
            let (fx, fy) = (s[0] / HEAT_RES - 0.5, s[1] / HEAT_RES - 0.5);
            let (ix, iy) = (fx.floor(), fy.floor());
            let (ax, ay) = ((fx - ix) as f32, (fy - iy) as f32);
            for (dx, dy, wt) in [(0, 0, (1.0 - ax) * (1.0 - ay)), (1, 0, ax * (1.0 - ay)), (0, 1, (1.0 - ax) * ay), (1, 1, ax * ay)] {
                let (px, py) = (ix as isize + dx, iy as isize + dy);
                if px >= 0 && py >= 0 && (px as usize) < rw && (py as usize) < rh {
                    acc[py as usize * rw + px as usize] += b.w * wt;
                }
            }
        }
    }
    let dens = blur(&acc, rw, rh, &gaussian_kernel(sigma), cancel)?;
    let max = dens.iter().copied().fold(0f32, f32::max);
    let mut bgra = vec![0u8; rw * rh * 4];
    if max > 0.0 {
        for (i, d) in dens.iter().enumerate() {
            let px = heat_color(d / max * o.intensity, o, opacity);
            bgra[i * 4..i * 4 + 4].copy_from_slice(&px);
        }
    }
    let origin = t.invert([0.0, 0.0]);
    let far = t.invert([rw as f64 * HEAT_RES, rh as f64 * HEAT_RES]);
    Some(HeatRaster {
        w: rw,
        h: rh,
        bgra,
        origin,
        px_world: [(far[0] - origin[0]) / rw as f64, (far[1] - origin[1]) / rh as f64],
        max_density: max,
    })
}

// ---- hexagon and grid bins -----------------------------------------------------------

/// One hexagon or square.
#[derive(Clone, Debug, PartialEq)]
pub struct BinCell {
    /// Lattice column (axial `q` for hexagons).
    pub cx: i32,
    /// Lattice row (axial `r` for hexagons).
    pub cy: i32,
    /// Centre in world coordinates.
    pub center: Coord,
    /// Features in the cell.
    pub count: u32,
    /// Sum of the column over features that have a number.
    pub sum: f64,
    /// Smallest such number.
    pub min: f64,
    /// Largest such number.
    pub max: f64,
    /// How many features had a number.
    pub numeric: u32,
    /// The cell's displayed value.
    pub value: f64,
    /// Colour class of `value`.
    pub class: u8,
}

/// Binned cells plus what is needed to draw, colour and look them up.
#[derive(Clone, Debug)]
pub struct BinResult {
    /// Hexagons (else squares).
    pub hex: bool,
    /// Cell width in world units (hexagon across flats, square side).
    pub cell_world: f64,
    /// The scale (px per world unit) the lattice was built at.
    pub scale: f64,
    /// Occupied cells.
    pub cells: Vec<BinCell>,
    /// `(cx, cy)` to index in `cells`.
    pub lookup: HashMap<(i32, i32), usize>,
    /// Class edges of the values (see `classify::breaks`).
    pub edges: Vec<f64>,
    /// Colour per class.
    pub colors: Vec<[u8; 3]>,
    /// What `value` is.
    pub what: BinValue,
}

const SQRT3: f64 = 1.732_050_807_568_877_2;

fn hex_round(q: f64, r: f64) -> (i32, i32) {
    let (x, z) = (q, r);
    let y = -x - z;
    let (mut rx, mut ry, mut rz) = (x.round(), y.round(), z.round());
    let (dx, dy, dz) = ((rx - x).abs(), (ry - y).abs(), (rz - z).abs());
    if dx > dy && dx > dz {
        rx = -ry - rz;
    } else if dy > dz {
        ry = -rx - rz;
    } else {
        rz = -rx - ry;
    }
    let _ = ry;
    (rx as i32, rz as i32)
}

/// The cell of `p` (world) for a lattice of `cell_world`-wide cells at `scale`.
pub fn cell_at(hex: bool, cell_world: f64, scale: f64, p: Coord) -> (i32, i32) {
    if hex {
        let r = cell_world * scale / SQRT3;
        let (u, v) = (p[0] * scale, p[1] * scale);
        hex_round((SQRT3 / 3.0 * u - v / 3.0) / r, (2.0 / 3.0 * v) / r)
    } else {
        (cell_index(p[0], cell_world), cell_index(p[1], cell_world))
    }
}

/// Centre of a lattice cell in world coordinates.
pub fn cell_center(hex: bool, cell_world: f64, scale: f64, c: (i32, i32)) -> Coord {
    if hex {
        let r = cell_world * scale / SQRT3;
        let (q, rr) = (f64::from(c.0), f64::from(c.1));
        [r * SQRT3 * (q + rr / 2.0) / scale, r * 1.5 * rr / scale]
    } else {
        [(f64::from(c.0) + 0.5) * cell_world, (f64::from(c.1) + 0.5) * cell_world]
    }
}

/// The six corners of a hexagon of width `cell_world` around `center` (pointy top).
pub fn hex_corners(cell_world: f64, center: Coord) -> [Coord; 6] {
    let r = cell_world / SQRT3;
    std::array::from_fn(|k| {
        let a = std::f64::consts::PI / 3.0 * k as f64 - std::f64::consts::PI / 6.0;
        [center[0] + r * a.cos(), center[1] + r * a.sin()]
    })
}

/// Bin the visible points into hexagons or squares of `o.cell_px` px at the view's scale.
/// The lattice is anchored at the world origin, so panning never reshuffles membership.
/// `w` holds the numeric column for Sum/Mean/Min/Max.
pub fn bin(pts: &PointSet, w: Option<&[f32]>, view: &AggView, hex: bool, o: &BinOptions, cancel: &dyn Fn() -> bool) -> Option<BinResult> {
    let scale = view.transform.scale[0].abs();
    let cell_world = f64::from(o.cell_px) / scale;
    let mw = f64::from(o.cell_px) / scale;
    let mut map: HashMap<(i32, i32), BinCell> = HashMap::new();
    for (k, r) in &view.copies {
        for (i, c) in pts.xy.iter().enumerate() {
            if i % 65536 == 0 && cancel() {
                return None;
            }
            if c[0] < r.min[0] - mw || c[0] > r.max[0] + mw || c[1] < r.min[1] - mw || c[1] > r.max[1] + mw {
                continue;
            }
            // Cells live in the world of their copy (offset `k` included).
            let p = [c[0] + k, c[1]];
            let key = cell_at(hex, cell_world, scale, p);
            let cell = map.entry(key).or_insert_with(|| BinCell {
                cx: key.0,
                cy: key.1,
                center: cell_center(hex, cell_world, scale, key),
                count: 0,
                sum: 0.0,
                min: f64::INFINITY,
                max: f64::NEG_INFINITY,
                numeric: 0,
                value: 0.0,
                class: 0,
            });
            cell.count += 1;
            if let Some(x) = w.map(|w| w[i]).filter(|x| !x.is_nan()) {
                let x = f64::from(x);
                cell.sum += x;
                cell.min = cell.min.min(x);
                cell.max = cell.max.max(x);
                cell.numeric += 1;
            }
        }
    }
    let mut cells: Vec<BinCell> = map.into_values().collect();
    cells.sort_by_key(|c| (c.cy, c.cx));
    let what = if w.is_some() { o.value } else { BinValue::Count };
    cells.retain_mut(|c| {
        c.value = match what {
            BinValue::Count => f64::from(c.count),
            _ if c.numeric == 0 => return false,
            BinValue::Sum => c.sum,
            BinValue::Mean => c.sum / f64::from(c.numeric),
            BinValue::Min => c.min,
            BinValue::Max => c.max,
        };
        true
    });
    let values: Vec<f64> = cells.iter().map(|c| c.value).collect();
    let edges = classify::breaks(&values, o.method, o.classes);
    let n = edges.len().saturating_sub(1).max(1);
    for c in &mut cells {
        c.class = classify::class_of(&edges, c.value) as u8;
    }
    let lookup = cells.iter().enumerate().map(|(i, c)| ((c.cx, c.cy), i)).collect();
    Some(BinResult { hex, cell_world, scale, cells, lookup, edges, colors: o.palette.classes(n, o.reversed), what })
}

// ---- clusters ------------------------------------------------------------------------

/// A group of nearby points (or one point).
#[derive(Clone, Debug, PartialEq)]
pub struct Cluster {
    /// Mean position (world).
    pub xy: Coord,
    /// Points in it.
    pub count: u32,
    /// Box of its points.
    pub bbox: BBox,
    /// First feature in it (the feature itself when `count == 1`).
    pub feature: u32,
}

/// Half-octave zoom level used to key cluster results: radius error stays under 41%.
pub fn cluster_level(scale: f64) -> i32 {
    (scale.abs().log2() * 2.0).floor() as i32
}

/// Greedy clustering for `level`: each point joins the first cluster within `radius_px`
/// (found through a cell grid), else starts one. Deterministic in input order. `None` when
/// `cancel` fired.
pub fn cluster(pts: &PointSet, level: i32, radius_px: f64, cancel: &dyn Fn() -> bool) -> Option<Vec<Cluster>> {
    let scale = 2f64.powf(f64::from(level) / 2.0);
    let r = radius_px / scale;
    let r2 = r * r;
    let mut out: Vec<Cluster> = Vec::new();
    let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
    for (i, c) in pts.xy.iter().enumerate() {
        if i % 65536 == 0 && cancel() {
            return None;
        }
        let (gx, gy) = (cell_index(c[0], r), cell_index(c[1], r));
        let mut hit: Option<u32> = None;
        'search: for dy in -1..=1 {
            for dx in -1..=1 {
                if let Some(v) = grid.get(&(gx + dx, gy + dy)) {
                    for &ci in v {
                        let cl = &out[ci as usize];
                        let (ex, ey) = (cl.xy[0] - c[0], cl.xy[1] - c[1]);
                        if ex * ex + ey * ey <= r2 {
                            hit = Some(ci);
                            break 'search;
                        }
                    }
                }
            }
        }
        match hit {
            Some(ci) => {
                let cl = &mut out[ci as usize];
                let n = f64::from(cl.count);
                cl.xy = [(cl.xy[0] * n + c[0]) / (n + 1.0), (cl.xy[1] * n + c[1]) / (n + 1.0)];
                cl.count += 1;
                cl.bbox = cl.bbox.union(BBox { min: *c, max: *c });
            }
            None => {
                grid.entry((gx, gy)).or_default().push(out.len() as u32);
                out.push(Cluster { xy: *c, count: 1, bbox: BBox { min: *c, max: *c }, feature: pts.feat[i] });
            }
        }
    }
    Some(out)
}

/// Circle radius in px for a cluster of `count` points.
pub fn cluster_radius(count: u32) -> f32 {
    if count <= 1 { 5.0 } else { (11.0 + 5.0 * (count as f32).log10()).min(26.0) }
}

// ---- worker --------------------------------------------------------------------------

/// Immutable inputs of aggregation jobs; replaced when the result changes.
pub struct AggSource {
    /// Data-set counter.
    pub generation: u64,
    /// Projected features.
    pub prepared: Arc<Prepared>,
    /// Source features (typed values).
    pub features: Arc<Vec<MapFeature>>,
    /// Value-column names of each layer.
    pub columns: Vec<Vec<String>>,
}

/// One aggregation to compute.
pub struct AggJob {
    /// Layer.
    pub layer: usize,
    /// Inputs.
    pub source: Arc<AggSource>,
    /// The layer's style.
    pub style: LayerStyle,
    /// The view.
    pub view: AggView,
    /// Request number; results echo it so the panel can tell what they answer.
    pub seq: u64,
}

/// What a job produced.
pub enum AggOutput {
    /// A heatmap image.
    Heat(HeatRaster),
    /// Hexagon or grid cells.
    Bins(BinResult),
    /// Clusters at a level.
    Clusters(Arc<Vec<Cluster>>),
}

/// A finished job.
pub struct AggDone {
    /// Layer.
    pub layer: usize,
    /// Data set.
    pub generation: u64,
    /// The view it was computed for.
    pub view: AggView,
    /// The result.
    pub out: AggOutput,
    /// Milliseconds spent computing (for the performance report).
    pub millis: f64,
}

struct AggShared {
    pending: Mutex<Vec<AggJob>>,
    latest: Mutex<HashMap<usize, u64>>,
    wake: Condvar,
    shutdown: AtomicU64,
    results: async_channel::Sender<AggDone>,
}

/// One background thread computing aggregations. Submitting a job for a layer replaces any
/// queued job for it and makes a running one for it cancel itself at its next check.
pub struct AggWorker {
    shared: Arc<AggShared>,
    /// Finished jobs.
    pub results: async_channel::Receiver<AggDone>,
    started: bool,
}

/// Wait this long after a request for a newer one before computing: a drag or a wheel spin
/// issues dozens of views per second and only the last is worth the work.
const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(60);

impl Default for AggWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl AggWorker {
    /// A worker whose thread starts on the first job.
    pub fn new() -> Self {
        let (tx, rx) = async_channel::unbounded();
        AggWorker {
            shared: Arc::new(AggShared {
                pending: Mutex::new(Vec::new()),
                latest: Mutex::new(HashMap::new()),
                wake: Condvar::new(),
                shutdown: AtomicU64::new(0),
                results: tx,
            }),
            results: rx,
            started: false,
        }
    }

    /// Queue `job`, replacing older work for its layer.
    pub fn submit(&mut self, job: AggJob) {
        if !self.started {
            self.started = true;
            let shared = self.shared.clone();
            let _ = std::thread::Builder::new().name("theta-agg".into()).spawn(move || worker_loop(shared));
        }
        self.shared.latest.lock().unwrap_or_else(|e| e.into_inner()).insert(job.layer, job.seq);
        let mut p = self.shared.pending.lock().unwrap_or_else(|e| e.into_inner());
        p.retain(|j| j.layer != job.layer);
        p.push(job);
        drop(p);
        self.shared.wake.notify_all();
    }

    /// Drop queued work and cancel running work for every layer (the data was replaced).
    pub fn cancel_all(&mut self) {
        self.shared.latest.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.shared.pending.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

impl Drop for AggWorker {
    fn drop(&mut self) {
        self.shared.shutdown.store(1, Ordering::SeqCst);
        self.shared.wake.notify_all();
    }
}

#[derive(Default)]
struct Caches {
    sets: HashMap<(u64, usize), Arc<PointSet>>,
    weights: HashMap<(u64, usize, usize), Arc<Vec<f32>>>,
    fine: Vec<((u64, usize, Option<usize>, i32), Arc<FineGrid>)>,
    clusters: Vec<((u64, usize, i32, u32), Arc<Vec<Cluster>>)>,
}

fn worker_loop(shared: Arc<AggShared>) {
    let mut caches = Caches::default();
    loop {
        let job = {
            let mut p = shared.pending.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if shared.shutdown.load(Ordering::SeqCst) != 0 {
                    return;
                }
                if !p.is_empty() {
                    break;
                }
                p = shared.wake.wait(p).unwrap_or_else(|e| e.into_inner());
            }
            drop(p);
            // Debounce, then take the newest of whatever is queued.
            std::thread::sleep(DEBOUNCE);
            let mut p = shared.pending.lock().unwrap_or_else(|e| e.into_inner());
            if p.is_empty() {
                continue;
            }
            p.remove(0)
        };
        let seq = job.seq;
        let layer = job.layer;
        let cancel = || shared.latest.lock().map_or(true, |l| l.get(&layer) != Some(&seq));
        let started = std::time::Instant::now();
        let Some(out) = run_job(&job, &mut caches, &cancel) else { continue };
        let done = AggDone {
            layer,
            generation: job.source.generation,
            view: job.view,
            out,
            millis: started.elapsed().as_secs_f64() * 1000.0,
        };
        if shared.results.send_blocking(done).is_err() {
            return;
        }
    }
}

fn column_index(cols: &[String], name: &Option<String>) -> Option<usize> {
    name.as_ref().and_then(|n| cols.iter().position(|c| c == n))
}

fn run_job(job: &AggJob, c: &mut Caches, cancel: &dyn Fn() -> bool) -> Option<AggOutput> {
    let src = &job.source;
    let gen_ = src.generation;
    let set = c
        .sets
        .entry((gen_, job.layer))
        .or_insert_with(|| Arc::new(point_set(&src.prepared, job.layer)))
        .clone();
    let cols = src.columns.get(job.layer).map(Vec::as_slice).unwrap_or(&[]);
    let mut weights_for = |col: Option<usize>| -> Option<Arc<Vec<f32>>> {
        let col = col?;
        Some(c.weights.entry((gen_, job.layer, col)).or_insert_with(|| Arc::new(weights(&src.features, &src.prepared, &set, col))).clone())
    };
    let st = &job.style;
    match st.kind {
        crate::style::LayerKind::Heatmap => {
            let col = column_index(cols, &st.heatmap.weight);
            let w = weights_for(col);
            let level = job.view.transform.lod_level();
            let key = (gen_, job.layer, col, level.clamp(-30, 30));
            let fine = match c.fine.iter().find(|(k, _)| *k == key) {
                Some((_, f)) => f.clone(),
                None => {
                    let f = Arc::new(fine_grid(&set, w.as_deref().map(Vec::as_slice), level));
                    c.fine.retain(|(k, _)| k.0 == gen_);
                    if c.fine.len() >= 3 {
                        c.fine.remove(0);
                    }
                    c.fine.push((key, f.clone()));
                    f
                }
            };
            if cancel() {
                return None;
            }
            kde(&fine, &job.view, &st.heatmap, st.opacity, cancel).map(AggOutput::Heat)
        }
        crate::style::LayerKind::Hexagons | crate::style::LayerKind::Grid => {
            let col = column_index(cols, &st.bins.column).filter(|_| st.bins.value != BinValue::Count);
            let w = weights_for(col);
            bin(&set, w.as_deref().map(Vec::as_slice), &job.view, st.kind == crate::style::LayerKind::Hexagons, &st.bins, cancel).map(AggOutput::Bins)
        }
        crate::style::LayerKind::Clusters => {
            let level = cluster_level(job.view.transform.scale[0]);
            let key = (gen_, job.layer, level, st.clusters.radius_px.to_bits());
            let cl = match c.clusters.iter().find(|(k, _)| *k == key) {
                Some((_, v)) => v.clone(),
                None => {
                    let v = Arc::new(cluster(&set, level, f64::from(st.clusters.radius_px), cancel)?);
                    c.clusters.retain(|(k, _)| k.0 == gen_);
                    if c.clusters.len() >= 4 {
                        c.clusters.remove(0);
                    }
                    c.clusters.push((key, v.clone()));
                    v
                }
            };
            Some(AggOutput::Clusters(cl))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(pts: &[Coord]) -> PointSet {
        PointSet { xy: pts.to_vec(), feat: (0..pts.len() as u32).collect() }
    }

    fn view(scale: f64, center: Coord) -> AggView {
        AggView {
            transform: Transform { half: [200.0, 100.0], center, scale: [scale, scale] },
            size: [400.0, 200.0],
            copies: vec![(0.0, BBox { min: [-1e9, -1e9], max: [1e9, 1e9] })],
        }
    }

    fn never() -> bool {
        false
    }

    #[test]
    fn fine_grid_merges_points_in_one_cell_and_sums_weights() {
        let s = set(&[[0.5, 0.5], [0.5000001, 0.5], [0.9, 0.9]]);
        let g = fine_grid(&s, Some(&[1.0, 2.0, 5.0]), 10);
        assert_eq!(g.bins.len(), 2);
        assert!((g.bins.iter().map(|b| b.w).sum::<f32>() - 8.0).abs() < 1e-6);
        assert!(g.bins.windows(2).all(|w| (w[0].cx, w[0].cy) < (w[1].cx, w[1].cy)));
        // Same bins whatever the view: pre-binning depends on the level only.
        assert_eq!(fine_grid(&s, None, 10).bins.len(), 2);
    }

    #[test]
    fn kde_peaks_at_the_point_and_is_symmetric() {
        let s = set(&[[0.0, 0.0]]);
        let v = view(100.0, [0.0, 0.0]);
        let fine = fine_grid(&s, None, v.transform.lod_level());
        let o = HeatmapOptions { radius_px: 20.0, ..LayerStyle::default().heatmap };
        let r = kde(&fine, &v, &o, 1.0, &never).unwrap();
        assert_eq!((r.w, r.h), (200, 100));
        let alpha = |x: usize, y: usize| r.bgra[(y * r.w + x) * 4 + 3];
        let (cx, cy) = (100, 50);
        assert!(alpha(cx, cy) > 200, "{}", alpha(cx, cy));
        assert!(alpha(cx, cy) >= alpha(cx + 5, cy));
        let (l, rr) = (alpha(cx - 6, cy), alpha(cx + 5, cy));
        assert!(l.abs_diff(rr) < 25, "{l} vs {rr}");
        assert_eq!(alpha(5, 5), 0, "far away is transparent");
        assert!(r.max_density > 0.0);
    }

    #[test]
    fn kde_weights_scale_relative_density() {
        let s = set(&[[-0.5, 0.0], [0.5, 0.0]]);
        let v = view(100.0, [0.0, 0.0]);
        let fine = fine_grid(&s, Some(&[1.0, 4.0]), v.transform.lod_level());
        let o = LayerStyle::default().heatmap;
        let r = kde(&fine, &v, &o, 1.0, &never).unwrap();
        let a = |x: usize| {
            let i = (50 * r.w + x) * 4;
            [r.bgra[i], r.bgra[i + 1], r.bgra[i + 2], r.bgra[i + 3]]
        };
        // 100 px/unit, centred at x = 200 px, half resolution: the points are at raster x = 75 and 125.
        // Both are hot enough to be opaque; the denser one sits higher on the colour ramp.
        assert_ne!(a(125), a(75));
        assert!(a(75)[3] > 0 && a(125)[3] > 0);
    }

    #[test]
    fn kde_honours_cancel_and_pan_reuses_the_same_fine_grid() {
        let s = set(&(0..1000).map(|i| [i as f64 * 0.001, 0.0]).collect::<Vec<_>>());
        let v = view(100.0, [0.5, 0.0]);
        let fine = fine_grid(&s, None, v.transform.lod_level());
        assert!(kde(&fine, &v, &LayerStyle::default().heatmap, 1.0, &|| true).is_none());
        // A pan changes the view but not the fine grid.
        let panned = view(100.0, [0.6, 0.0]);
        assert_eq!(panned.transform.lod_level(), v.transform.lod_level());
        assert!(kde(&fine, &panned, &LayerStyle::default().heatmap, 1.0, &never).is_some());
    }

    #[test]
    fn heat_colour_is_transparent_below_the_threshold_and_premultiplied() {
        let o = LayerStyle::default().heatmap;
        assert_eq!(heat_color(0.01, &o, 1.0), [0; 4]);
        let c = heat_color(1.0, &o, 1.0);
        assert!(c[3] > 200);
        assert!(c[0] <= c[3] && c[1] <= c[3] && c[2] <= c[3]);
    }

    #[test]
    fn hex_cell_roundtrip_and_membership() {
        let (cw, scale) = (28.0 / 100.0, 100.0);
        for (i, p) in [[0.12, 0.34], [-1.7, 2.2], [3.3, -0.4], [0.0, 0.0]].iter().enumerate() {
            let c = cell_at(true, cw, scale, *p);
            let ctr = cell_center(true, cw, scale, c);
            // A point is within the hexagon's circumradius of its cell centre.
            let r = cw / SQRT3;
            assert!((p[0] - ctr[0]).hypot(p[1] - ctr[1]) <= r + 1e-9, "{i}");
            assert_eq!(cell_at(true, cw, scale, ctr), c, "centre maps to its own cell");
        }
        let g = cell_at(false, 0.5, 100.0, [1.2, -0.3]);
        assert_eq!(g, (2, -1));
        assert_eq!(cell_center(false, 0.5, 100.0, g), [1.25, -0.25]);
    }

    #[test]
    fn bins_count_every_visible_point_and_aggregate_values() {
        let pts: Vec<Coord> = (0..100).map(|i| [(i % 10) as f64 * 0.01, (i / 10) as f64 * 0.01]).collect();
        let s = set(&pts);
        let w: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let v = view(1000.0, [0.05, 0.05]);
        let o = BinOptions { cell_px: 30.0, ..LayerStyle::default().bins };
        for hex in [true, false] {
            let r = bin(&s, None, &v, hex, &o, &never).unwrap();
            assert_eq!(r.cells.iter().map(|c| c.count).sum::<u32>(), 100, "hex={hex}");
            assert!(r.cells.len() > 1);
            let sum = bin(&s, Some(&w), &v, hex, &BinOptions { value: BinValue::Sum, ..o.clone() }, &never).unwrap();
            assert!((sum.cells.iter().map(|c| c.value).sum::<f64>() - 4950.0).abs() < 1e-6);
            let max = bin(&s, Some(&w), &v, hex, &BinOptions { value: BinValue::Max, ..o.clone() }, &never).unwrap();
            assert_eq!(max.cells.iter().map(|c| c.value).fold(0.0, f64::max), 99.0);
            let mean = bin(&s, Some(&w), &v, hex, &BinOptions { value: BinValue::Mean, ..o.clone() }, &never).unwrap();
            assert!(mean.cells.iter().all(|c| c.value >= c.min && c.value <= c.max));
            // Every cell is found again through the lookup, and classes index the colour list.
            for (i, c) in sum.cells.iter().enumerate() {
                assert_eq!(sum.lookup[&(c.cx, c.cy)], i);
                assert!((c.class as usize) < sum.colors.len());
            }
        }
    }

    #[test]
    fn bin_lattice_is_stable_under_pan() {
        let pts: Vec<Coord> = (0..50).map(|i| [0.3 + i as f64 * 0.002, 0.3]).collect();
        let s = set(&pts);
        let o = LayerStyle::default().bins;
        let a = bin(&s, None, &view(1000.0, [0.35, 0.3]), true, &o, &never).unwrap();
        let b = bin(&s, None, &view(1000.0, [0.351, 0.3]), true, &o, &never).unwrap();
        let ka: Vec<_> = a.cells.iter().map(|c| (c.cx, c.cy, c.count)).collect();
        let kb: Vec<_> = b.cells.iter().map(|c| (c.cx, c.cy, c.count)).collect();
        assert_eq!(ka, kb);
    }

    #[test]
    fn clusters_merge_near_points_and_keep_far_ones_apart() {
        let s = set(&[[0.5, 0.5], [0.5001, 0.5001], [0.9, 0.1], [0.5002, 0.4999]]);
        // scale 2^10: radius 48 px = 0.047 world units.
        let level = cluster_level(1024.0);
        assert_eq!(level, 20);
        let c = cluster(&s, level, 48.0, &never).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c.iter().map(|c| c.count).sum::<u32>(), 4);
        let big = c.iter().find(|c| c.count == 3).unwrap();
        assert_eq!(big.feature, 0);
        assert!(big.bbox.min[0] <= 0.5 && big.bbox.max[0] >= 0.5002);
        // Zoomed in far enough they separate.
        assert_eq!(cluster(&s, cluster_level(1024.0 * 4096.0), 48.0, &never).unwrap().len(), 4);
        // Deterministic.
        assert_eq!(cluster(&s, level, 48.0, &never), Some(c));
    }

    #[test]
    fn cluster_circles_grow_with_count_and_are_capped() {
        assert!(cluster_radius(2) < cluster_radius(50));
        assert_eq!(cluster_radius(10_000_000), 26.0);
        assert_eq!(cluster_radius(1), 5.0);
    }

    #[test]
    fn million_points_heatmap_and_tiles_are_fast_enough() {
        // Not a micro-benchmark: a guard that the O(n) paths stay linear-ish. Release builds
        // do this in well under a second; debug builds get a generous bound.
        let mut s = 99u64;
        let mut rnd = move || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (s >> 33) as f64 / (1u64 << 31) as f64
        };
        let pts: Vec<Coord> = (0..200_000).map(|_| [rnd(), rnd()]).collect();
        let ps = set(&pts);
        let v = view(512.0, [0.5, 0.5]);
        let t = std::time::Instant::now();
        let fine = fine_grid(&ps, None, v.transform.lod_level());
        let r = kde(&fine, &v, &LayerStyle::default().heatmap, 1.0, &never).unwrap();
        assert!(r.max_density > 0.0);
        assert!(t.elapsed().as_secs_f64() < 20.0);
    }
}
