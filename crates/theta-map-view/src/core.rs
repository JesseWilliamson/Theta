//! The panel's mutable state and the logic that does not need a window: camera
//! animation, hit testing, tile bookkeeping and data replacement.

use crate::camera::{Camera, FIT_PADDING, Transform};
use crate::grid::{degree_step, nice_step, tick_label, ticks};
use crate::index::GridIndex;
use crate::loader::{TileLoader, TileResult};
use crate::lod::LodCache;
use crate::measure_tool::{Measurement, Metric};
use crate::model::{MapFeature, MapLayer};
use crate::tiles::{Lru, MEMORY_TILES, TileKey, cache_root, plan_requests, template_hash};
use crate::world::{Mode, Prepared, WorldFeature, lon_lat_to_world, prepare, world_to_lon_lat};
use gpui_kit::RenderImage;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use theta_geo::{BBox, Coord, TileId, hit_test};

/// Seconds for an animation to cover ~63% of the remaining distance.
const SMOOTHING_S: f64 = 0.085;
/// Consecutive failed tile fetches before the "offline?" chip appears.
const OFFLINE_AFTER: u32 = 3;
/// How long new tile requests stay paused once the network looks down.
const PAUSE_AFTER_OFFLINE: std::time::Duration = std::time::Duration::from_secs(20);

/// Animation targets; each frame moves the camera a fraction of the way there.
#[derive(Default)]
pub struct Anim {
    /// Zoom towards this level keeping `anchor` (screen px) fixed.
    pub zoom: Option<(f64, [f64; 2])>,
    /// Fly the viewport centre (world coords) and level.
    pub fly: Option<(Coord, f64)>,
    last_tick: Option<Instant>,
}

impl Anim {
    /// Whether something is still moving.
    pub fn active(&self) -> bool {
        self.zoom.is_some() || self.fly.is_some()
    }

    /// Stop all animation (the user grabbed the map).
    pub fn cancel(&mut self) {
        self.zoom = None;
        self.fly = None;
        self.last_tick = None;
    }
}

/// An in-progress mouse drag.
pub struct Drag {
    /// Last pointer position (viewport px).
    pub last: [f64; 2],
    /// Total pointer travel, to tell a click from a drag.
    pub travel: f64,
}

/// A hover card pinned by a click.
#[derive(Clone, Copy)]
pub struct Pinned {
    /// Feature index.
    pub feature: usize,
    /// Where the card is anchored (viewport px).
    pub at: [f64; 2],
}

/// Decoded tiles plus the bookkeeping of what is being fetched.
pub struct TileStore {
    /// Decoded tiles, most recently used last.
    pub lru: Lru<TileKey, Arc<RenderImage>>,
    /// The worker pool.
    pub loader: TileLoader,
    /// The URL template in use.
    pub template: String,
    /// Deepest zoom level the basemap serves; the map overzooms past it.
    pub max_zoom: u8,
    hash: u64,
    /// Tiles whose fetch failed; not retried until the user presses Retry.
    pub failed: HashSet<TileId>,
    /// Failures since the last success.
    pub consecutive_failures: u32,
    last_wanted: Vec<TileId>,
    pub(crate) last_plan: Vec<TileId>,
    /// While set and in the future, no new requests are started (the network looks down).
    paused_until: Option<Instant>,
}

impl TileStore {
    /// A store caching under the standard cache directory.
    pub fn new(template: String) -> Self {
        TileStore {
            lru: Lru::new(MEMORY_TILES),
            loader: TileLoader::new(cache_root()),
            hash: template_hash(&template),
            template,
            max_zoom: 19,
            failed: HashSet::new(),
            consecutive_failures: 0,
            last_wanted: Vec::new(),
            last_plan: Vec::new(),
            paused_until: None,
        }
    }

    /// The URL template changed (settings edit).
    pub fn set_template(&mut self, template: String) {
        if template != self.template {
            self.hash = template_hash(&template);
            self.template = template;
            self.failed.clear();
            self.consecutive_failures = 0;
            self.paused_until = None;
            self.last_wanted.clear();
        }
    }

    /// Key for `tile` under the current template.
    pub fn key(&self, tile: TileId) -> TileKey {
        TileKey { template_hash: self.hash, tile }
    }

    /// The decoded image for `tile`, if loaded.
    pub fn image(&self, tile: TileId) -> Option<&Arc<RenderImage>> {
        self.lru.peek(&self.key(tile))
    }

    /// Tell the workers which tiles the viewport needs now (nearest first).
    pub fn want(&mut self, visible: &[TileId]) {
        // Tiles on screen are the most recently used, so eviction takes the ones that
        // scrolled away first.
        for t in visible {
            let key = TileKey { template_hash: self.hash, tile: *t };
            let _ = self.lru.get(&key);
        }
        if visible == self.last_wanted.as_slice() {
            return;
        }
        self.last_wanted = visible.to_vec();
        let hash = self.hash;
        let paused = self.paused_until.is_some_and(|t| Instant::now() < t);
        let plan = if paused {
            Vec::new()
        } else {
            plan_requests(
                visible,
                |t| self.lru.contains(&TileKey { template_hash: hash, tile: t }),
                &HashSet::new(),
                &self.failed,
                usize::MAX,
            )
        };
        if plan != self.last_plan {
            self.loader.set_wanted(&self.template, plan.clone());
            self.last_plan = plan;
        }
    }

    /// Absorb one worker result; true when the view should repaint.
    pub fn accept(&mut self, r: TileResult) -> bool {
        match r.image {
            Ok(img) => {
                self.consecutive_failures = 0;
                self.failed.remove(&r.key.tile);
                self.lru.insert(r.key, img);
                true
            }
            Err(_) => {
                if r.key.template_hash == self.hash {
                    self.failed.insert(r.key.tile);
                    self.consecutive_failures += 1;
                    if self.consecutive_failures >= OFFLINE_AFTER {
                        // Stop hammering a dead network; Retry (or a template change) lifts this.
                        self.paused_until = Some(Instant::now() + PAUSE_AFTER_OFFLINE);
                    }
                    self.last_wanted.clear();
                    return self.consecutive_failures == OFFLINE_AFTER;
                }
                false
            }
        }
    }

    /// Forget failures and fetch again.
    pub fn retry(&mut self) {
        self.failed.clear();
        self.consecutive_failures = 0;
        self.paused_until = None;
        self.last_plan.clear();
        self.last_wanted.clear();
    }

    /// Whether the "Basemap unavailable" chip should show.
    pub fn offline(&self) -> bool {
        self.consecutive_failures >= OFFLINE_AFTER
    }
}

/// Everything the panel and its paint code share.
pub struct Core {
    /// The data in world space.
    pub prepared: Prepared,
    /// Grid index over feature boxes.
    pub index: GridIndex,
    /// The original features (properties, source CRS).
    pub source: Vec<MapFeature>,
    /// Layers as given.
    pub layers: Vec<MapLayer>,
    /// Features per layer.
    pub layer_counts: Vec<usize>,
    /// The camera.
    pub camera: Camera,
    /// Whether paint has reported a real viewport size yet.
    pub size_known: bool,
    /// Fit the data on the next paint.
    pub pending_fit: bool,
    /// Simplification cache.
    pub lod: LodCache,
    /// Animation targets.
    pub anim: Anim,
    /// Hovered feature (index into `prepared.features`).
    pub hover: Option<usize>,
    /// Selected result row.
    pub selected_row: Option<usize>,
    /// Pinned card.
    pub pinned: Option<Pinned>,
    /// Pointer position in viewport px while over the map.
    pub pointer: Option<[f64; 2]>,
    /// Active drag.
    pub drag: Option<Drag>,
    /// Measure tool on.
    pub measuring: bool,
    /// Measure tool state.
    pub measure: Measurement,
    /// Basemap tiles on (the selected basemap has tiles).
    pub basemap: bool,
    /// The selected basemap: attribution, dark/light, deepest zoom.
    pub basemap_info: crate::basemap::Basemap,
    /// Dim tiles in dark themes.
    pub dim: bool,
    /// Tile cache and loader.
    pub tiles: TileStore,
    /// Whether the last paint drew the graticule/grid (so labels are shown).
    pub grid_shown: bool,
    /// Paint timing since the last `take_paint_stats`: frames, total ms, max ms.
    pub paint_stats: (u32, f64, f64),
}

impl Core {
    /// Switch to `b` (a settings change or first paint); a no-op when nothing differs.
    pub fn apply_basemap(&mut self, b: &crate::basemap::Basemap) {
        if self.basemap_info == *b {
            return;
        }
        self.basemap = b.template.is_some();
        if let Some(t) = &b.template {
            self.tiles.set_template(t.clone());
        }
        self.tiles.max_zoom = b.max_zoom;
        self.basemap_info = b.clone();
    }

    /// An empty core.
    pub fn new(template: String) -> Self {
        Core {
            prepared: prepare(&[]),
            index: GridIndex::new(&[]),
            source: Vec::new(),
            layers: Vec::new(),
            layer_counts: Vec::new(),
            camera: Camera::fit(Mode::Geographic, None, [800.0, 400.0]),
            size_known: false,
            pending_fit: false,
            lod: LodCache::default(),
            anim: Anim::default(),
            hover: None,
            selected_row: None,
            pinned: None,
            pointer: None,
            drag: None,
            measuring: false,
            measure: Measurement::new(Metric::Geodesic),
            basemap: true,
            basemap_info: crate::basemap::builtins().remove(0),
            dim: true,
            tiles: TileStore::new(template),
            grid_shown: false,
            paint_stats: (0, 0.0, 0.0),
        }
    }

    /// Replace the content; the camera fits on the next paint.
    pub fn set_features(&mut self, layers: Vec<MapLayer>, features: Vec<MapFeature>) {
        self.prepared = prepare(&features);
        let boxes: Vec<BBox> = self.prepared.features.iter().map(|f| f.bbox).collect();
        self.index = GridIndex::new(&boxes);
        self.layer_counts = vec![0; layers.len()];
        for f in &self.prepared.features {
            if let Some(c) = self.layer_counts.get_mut(f.layer) {
                *c += 1;
            }
        }
        self.layers = layers;
        self.source = features;
        self.lod.clear();
        self.anim.cancel();
        self.hover = None;
        self.pinned = None;
        self.selected_row = None;
        self.measure = Measurement::new(self.metric());
        self.measuring = false;
        let size = self.camera.size();
        self.camera = self.fit_camera(false, size);
        self.pending_fit = true;
    }

    /// Drop all content.
    pub fn clear(&mut self) {
        self.set_features(Vec::new(), Vec::new());
        self.pending_fit = false;
    }

    /// Measuring metric for the current mode.
    pub fn metric(&self) -> Metric {
        match self.prepared.mode {
            Mode::Geographic => Metric::Geodesic,
            Mode::Planar => Metric::Planar,
        }
    }

    /// True when there is nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.prepared.features.is_empty()
    }

    /// Is the feature's layer visible?
    pub fn layer_visible(&self, f: &WorldFeature) -> bool {
        self.layers.get(f.layer).is_none_or(|l| l.visible)
    }

    /// A camera framing the data (the visible layers when `visible_only`, falling back to
    /// everything), along the shortest longitude arc so antimeridian data is not spread
    /// across the world.
    pub fn fit_camera(&self, visible_only: bool, size: [f64; 2]) -> Camera {
        let feats = &self.prepared.features;
        let shown = feats.iter().filter(|f| !visible_only || self.layer_visible(f)).map(|f| f.bbox);
        if visible_only && feats.iter().all(|f| !self.layer_visible(f)) {
            return Camera::fit_boxes(self.prepared.mode, feats.iter().map(|f| f.bbox), size, FIT_PADDING);
        }
        Camera::fit_boxes(self.prepared.mode, shown, size, FIT_PADDING)
    }

    /// Fit everything, animated.
    pub fn fit_all(&mut self) {
        let target = self.fit_camera(true, self.camera.size());
        self.fly_to(&target);
    }

    /// Fly to a camera state (the viewport size of `target` is ignored).
    pub fn fly_to(&mut self, target: &Camera) {
        self.anim.zoom = None;
        // A target below the minimum zoom (the window was resized since) could never be
        // reached and would keep the animation running forever.
        let (lo, hi) = self.camera.level_limits();
        self.anim.fly = Some((target.center_world(), target.level().clamp(lo, hi)));
    }

    /// Zoom by `delta` levels around `anchor` (viewport px), smoothly. Repeated calls
    /// accumulate on the pending target so fast wheel spins feel continuous.
    pub fn zoom_by(&mut self, delta: f64, anchor: [f64; 2]) {
        let (lo, hi) = self.camera.level_limits();
        self.anim.fly = None;
        let base = self.anim.zoom.map_or(self.camera.level(), |(t, _)| t);
        self.anim.zoom = Some(((base + delta).clamp(lo, hi), anchor));
    }

    /// Pan the viewport by a screen delta in the direction the *viewport* moves.
    pub fn nudge(&mut self, dx: f64, dy: f64) {
        self.anim.zoom = None;
        let t = self.camera.transform();
        let (cw, level) = self.anim.fly.unwrap_or((t.center, self.camera.level()));
        // Viewport moving right means world centre moving right.
        let w = [cw[0] + dx / t.scale[0], cw[1] + dy / t.scale[1]];
        self.anim.fly = Some((w, level));
    }

    /// Advance animations; true while more frames are needed.
    pub fn step_anim(&mut self, now: Instant) -> bool {
        if !self.anim.active() {
            self.anim.last_tick = None;
            return false;
        }
        let dt =
            self.anim.last_tick.map_or(0.016, |t| now.duration_since(t).as_secs_f64()).clamp(0.001, 0.05);
        self.anim.last_tick = Some(now);
        let k = 1.0 - (-dt / SMOOTHING_S).exp();
        let size = self.camera.size();
        if let Some((target, anchor)) = self.anim.zoom {
            let d = target - self.camera.level();
            if d.abs() < 0.002 {
                self.camera.zoom_around(anchor, d);
                self.anim.zoom = None;
            } else {
                self.camera.zoom_around(anchor, d * k);
            }
        }
        if let Some((center, level)) = self.anim.fly {
            let d = level - self.camera.level();
            let mid = [size[0] / 2.0, size[1] / 2.0];
            let t = self.camera.transform();
            let px = ((center[0] - t.center[0]) * t.scale[0]).hypot((center[1] - t.center[1]) * t.scale[1]);
            if d.abs() < 0.002 && px < 0.3 {
                self.camera.zoom_around(mid, d);
                self.camera.center_on(center);
                self.anim.fly = None;
            } else {
                self.camera.zoom_around(mid, d * k);
                let t = self.camera.transform();
                self.camera.center_on([
                    t.center[0] + (center[0] - t.center[0]) * k,
                    t.center[1] + (center[1] - t.center[1]) * k,
                ]);
            }
        }
        if !self.anim.active() {
            self.anim.last_tick = None;
        }
        self.anim.active()
    }

    /// The world copies (whole-world offsets `k`: the data's x range is `[k, k + 1]`) in which
    /// features are drawn, each with the query rectangle in *data* space that finds the features
    /// to paint there.
    ///
    /// Zoomed out far enough, the viewport is wider than one world and the basemap repeats
    /// sideways. Features do not: each is drawn once, in the copy centred on the camera (see
    /// [`copy_window`]). A feature that straddles an edge of that window (a polygon over the
    /// antimeridian) can show at both ends of it, which is how it stays in one piece.
    pub fn copy_queries(&self, t: &Transform, size: [f64; 2], margin_px: f64) -> Vec<(f64, BBox)> {
        if self.prepared.mode == Mode::Planar {
            let r = t.world_rect(size, margin_px);
            return vec![(0.0, r)];
        }
        let r = t.world_rect(size, margin_px);
        let w = copy_window(t);
        let (x0, x1) = (r.min[0].max(w[0]), r.max[0].min(w[1]));
        if x0 > x1 {
            return Vec::new();
        }
        let (a, b) = ((x0.floor() as i64 - 1).max(-4), (x1.floor() as i64 + 1).min(4));
        (a..=b)
            .filter_map(|k| {
                let k = k as f64;
                // Data space is `[0, 1]` wide: copies that cannot reach it hold nothing.
                (x1 - k >= 0.0 && x0 - k <= 1.0)
                    .then(|| (k, BBox { min: [x0 - k, r.min[1]], max: [x1 - k, r.max[1]] }))
            })
            .collect()
    }

    /// The top-most feature under `p` (viewport px), if any.
    pub fn hit_at(&mut self, p: [f64; 2]) -> Option<usize> {
        let t = self.camera.transform();
        let level = t.lod_level();
        let w = t.invert(p);
        let reach = 8.0 / t.scale[0].abs();
        let window = copy_window(&t);
        let ks: Vec<f64> = match self.prepared.mode {
            Mode::Planar => vec![0.0],
            Mode::Geographic => {
                // The copies under the pointer (it may be a few worlds from the camera when the
                // map is zoomed far out); `drawn_at` then keeps only those that are painted.
                let (a, b) = ((w[0] - reach).floor() as i64 - 1, (w[0] + reach).ceil() as i64);
                (a.max(-6)..=b.min(6)).map(|k| k as f64).collect()
            }
        };
        let mut best: Option<(u8, usize)> = None;
        let mut ids = Vec::new();
        for k in ks {
            let c = [w[0] - k, w[1]];
            let rect = BBox { min: [c[0] - reach, c[1] - reach], max: [c[0] + reach, c[1] + reach] };
            self.index.query_into(&rect, &mut ids);
            for &id in &ids {
                let id = id as usize;
                let f = &self.prepared.features[id];
                if !self.layer_visible(f) {
                    continue;
                }
                if self.prepared.mode == Mode::Geographic && !drawn_at(&f.bbox, k, window) {
                    continue; // that copy is not painted: nothing to hit there
                }
                let (rank, tol): (u8, f64) = if f.kinds.points {
                    (2, 7.0)
                } else if f.kinds.lines {
                    (1, 5.0)
                } else {
                    (0, 3.0)
                };
                if best.is_some_and(|b| (rank, id) <= b) {
                    continue;
                }
                let tiny = {
                    let (bw, bh) = (f.bbox.width() * t.scale[0].abs(), f.bbox.height() * t.scale[1].abs());
                    bw < 1.0 && bh < 1.0
                };
                let geom = self.lod.pick(level, id, &f.geom, f.vertices);
                let project = |c: Coord| t.apply([c[0] + k, c[1]]);
                let tol = if tiny { tol.max(5.0) } else { tol };
                if hit_test(&geom, &project, p, tol) {
                    best = Some((rank, id));
                }
            }
        }
        best.map(|(_, id)| id)
    }

    /// Feature indices that belong to `row`.
    pub fn features_of_row(&self, row: usize) -> Vec<usize> {
        self.prepared.features.iter().enumerate().filter(|(_, f)| f.row == row).map(|(i, _)| i).collect()
    }

    /// Highlight a row, optionally flying to it.
    pub fn select_row(&mut self, row: Option<usize>, zoom_to: bool) {
        self.selected_row = row;
        self.pinned = None;
        let Some(row) = row else { return };
        if !zoom_to {
            return;
        }
        let ids = self.features_of_row(row);
        let Some(bbox) = ids.iter().map(|i| self.prepared.features[*i].bbox).reduce(BBox::union) else {
            return;
        };
        let boxes = ids.iter().map(|i| self.prepared.features[*i].bbox);
        let mut target = Camera::fit_boxes(self.prepared.mode, boxes, self.camera.size(), 60.0);
        // Do not zoom in past useful detail for a lone point, and never zoom *out*
        // on a point that is already comfortably visible.
        let is_point = self.prepared.mode == Mode::Geographic && bbox.width() == 0.0 && bbox.height() == 0.0;
        if is_point {
            let want = target.level().min(self.camera.level().max(12.0));
            let d = want - target.level();
            target.zoom_around([self.camera.size()[0] / 2.0, self.camera.size()[1] / 2.0], d);
            let ctr = bbox.min;
            target.center_on(ctr);
        }
        self.fly_to(&target);
    }

    /// Screen position of a coordinate in the data's own space as the measure tool stores it.
    pub fn measure_screen(&self, c: Coord) -> [f64; 2] {
        self.camera.project(c)
    }

    /// The pointer position as the measure tool stores coordinates.
    pub fn measure_coord_at(&self, p: [f64; 2]) -> Coord {
        self.camera.unproject(p)
    }
}

/// The half-open world-x window `[start, end)` holding the single copy of the data that is
/// drawn and hit-tested: one world width centred on the camera.
pub fn copy_window(t: &Transform) -> [f64; 2] {
    [t.center[0] - 0.5, t.center[0] + 0.5 - 1e-9]
}

/// Whether the copy of a feature with `bbox` shifted by `k` worlds reaches into `window`.
/// A point is in exactly one copy; the window is half open so one on its edge is not in two.
pub fn drawn_at(bbox: &BBox, k: f64, window: [f64; 2]) -> bool {
    bbox.max[0] + k >= window[0] && bbox.min[0] + k <= window[1]
}

/// One graticule / grid line with its label.
pub struct GridLine {
    /// Position along the perpendicular axis, viewport px.
    pub at: f64,
    /// Label.
    pub label: String,
    /// The world axis (value 0) gets drawn stronger.
    pub axis: bool,
}

/// Lines to draw behind features when there is no basemap.
pub struct GridSpec {
    /// Vertical lines (x positions).
    pub vertical: Vec<GridLine>,
    /// Horizontal lines (y positions).
    pub horizontal: Vec<GridLine>,
}

/// Graticule (geographic) or grid (planar) for the camera, roughly 90px apart.
pub fn grid_spec(camera: &Camera) -> GridSpec {
    let size = camera.size();
    let t = camera.transform();
    let rect = t.world_rect(size, 0.0);
    let target = 90.0;
    match camera.mode() {
        Mode::Planar => {
            let step = nice_step(target / t.scale[0].abs());
            let xs = ticks(rect.min[0], rect.max[0], step, 120);
            let ys = ticks(rect.min[1], rect.max[1], step, 120);
            GridSpec {
                vertical: xs
                    .iter()
                    .map(|x| GridLine {
                        at: t.apply([*x, 0.0])[0],
                        label: tick_label(*x, step),
                        axis: *x == 0.0,
                    })
                    .collect(),
                horizontal: ys
                    .iter()
                    .map(|y| GridLine {
                        at: t.apply([0.0, *y])[1],
                        label: tick_label(*y, step),
                        axis: *y == 0.0,
                    })
                    .collect(),
            }
        }
        Mode::Geographic => {
            let (nw, se) =
                (world_to_lon_lat([rect.min[0], rect.min[1]]), world_to_lon_lat([rect.max[0], rect.max[1]]));
            let (lon0, lon1) = (nw[0], se[0]);
            let (lat0, lat1) = (se[1].min(nw[1]), se[1].max(nw[1]));
            let lon_step = degree_step(lon1 - lon0, size[0], target).max(1e-6);
            let lat_step = degree_step(lat1 - lat0, size[1], target).max(1e-6);
            GridSpec {
                vertical: ticks(lon0, lon1, lon_step, 120)
                    .iter()
                    .map(|lon| GridLine {
                        at: t.apply(lon_lat_to_world([*lon, 0.0]))[0],
                        label: tick_label(wrap(*lon), lon_step),
                        axis: *lon == 0.0,
                    })
                    .collect(),
                horizontal: ticks(lat0, lat1, lat_step, 120)
                    .iter()
                    .map(|lat| GridLine {
                        at: t.apply(lon_lat_to_world([0.0, *lat]))[1],
                        label: tick_label(*lat, lat_step),
                        axis: *lat == 0.0,
                    })
                    .collect(),
            }
        }
    }
}

fn wrap(lon: f64) -> f64 {
    theta_geo::wrap_lon(lon)
}

#[cfg(test)]
mod tests {
    use super::*;
    use theta_geo::{Feature, Geometry};

    fn pt(x: f64, y: f64, row: usize) -> MapFeature {
        MapFeature {
            feature: Feature::new(Geometry::Point(Some([x, y])), Some(4326)),
            row,
            label: None,
            properties: vec![],
            layer: 0,
        }
    }

    fn core_with(features: Vec<MapFeature>) -> Core {
        core_with_size(features, [800.0, 600.0])
    }

    fn core_with_size(features: Vec<MapFeature>, size: [f64; 2]) -> Core {
        let mut c = Core::new("http://127.0.0.1:9/{z}/{x}/{y}.png".into());
        c.set_features(vec![MapLayer { name: "geom".into(), visible: true }], features);
        c.camera.set_size(size);
        c.camera = Camera::fit(c.prepared.mode, c.prepared.bbox, size);
        c
    }

    #[test]
    fn hover_finds_point_under_pointer_and_misses_elsewhere() {
        let mut c = core_with(vec![pt(10.0, 50.0, 0), pt(10.5, 50.2, 1)]);
        let s = c.camera.project([10.5, 50.2]);
        assert_eq!(c.hit_at(s).map(|i| c.prepared.features[i].row), Some(1));
        assert_eq!(c.hit_at([s[0] + 40.0, s[1] + 40.0]), None);
    }

    #[test]
    fn hidden_layers_are_not_hit() {
        let mut c = core_with(vec![pt(10.0, 50.0, 0), pt(10.5, 50.2, 1)]);
        let s = c.camera.project([10.0, 50.0]);
        c.layers[0].visible = false;
        assert_eq!(c.hit_at(s), None);
    }

    #[test]
    fn points_win_over_polygons_beneath() {
        let poly = MapFeature {
            feature: Feature::new(
                Geometry::Polygon(vec![vec![
                    [9.0, 49.0],
                    [11.0, 49.0],
                    [11.0, 51.0],
                    [9.0, 51.0],
                    [9.0, 49.0],
                ]]),
                Some(4326),
            ),
            row: 0,
            label: None,
            properties: vec![],
            layer: 0,
        };
        let mut c = core_with(vec![poly, pt(10.0, 50.0, 1)]);
        let s = c.camera.project([10.0, 50.0]);
        assert_eq!(c.hit_at(s).map(|i| c.prepared.features[i].row), Some(1));
        let inside = c.camera.project([9.3, 49.3]);
        assert_eq!(c.hit_at(inside).map(|i| c.prepared.features[i].row), Some(0));
    }

    #[test]
    fn polygon_hole_is_not_a_hit() {
        let outer = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0], [0.0, 0.0]];
        let hole = vec![[3.0, 3.0], [7.0, 3.0], [7.0, 7.0], [3.0, 7.0], [3.0, 3.0]];
        let f = MapFeature {
            feature: Feature::new(Geometry::Polygon(vec![outer, hole]), Some(4326)),
            row: 0,
            label: None,
            properties: vec![],
            layer: 0,
        };
        let mut c = core_with(vec![f]);
        let centre = c.camera.project([5.0, 5.0]);
        assert_eq!(c.hit_at(centre), None);
        let ring = c.camera.project([1.5, 5.0]);
        assert_eq!(c.hit_at(ring), Some(0));
    }

    #[test]
    fn zoom_target_accumulates_and_converges() {
        let mut c = core_with(vec![pt(10.0, 50.0, 0)]);
        let z0 = c.camera.level();
        c.zoom_by(1.0, [400.0, 300.0]);
        c.zoom_by(1.0, [400.0, 300.0]);
        let mut now = Instant::now();
        for _ in 0..400 {
            now += std::time::Duration::from_millis(16);
            if !c.step_anim(now) {
                break;
            }
        }
        assert!((c.camera.level() - (z0 + 2.0)).abs() < 1e-6, "{} vs {}", c.camera.level(), z0 + 2.0);
        assert!(!c.anim.active());
    }

    #[test]
    fn nudge_moves_viewport_in_that_direction() {
        let mut c = core_with(vec![pt(10.0, 50.0, 0), pt(12.0, 51.0, 1)]);
        let before = c.camera.center_world();
        c.nudge(80.0, 0.0);
        let mut now = Instant::now();
        for _ in 0..400 {
            now += std::time::Duration::from_millis(16);
            if !c.step_anim(now) {
                break;
            }
        }
        let after = c.camera.center_world();
        assert!(after[0] > before[0], "viewport should move east");
        let moved_px = (after[0] - before[0]) * c.camera.transform().scale[0];
        assert!((moved_px - 80.0).abs() < 0.5, "{moved_px}");
    }

    #[test]
    fn select_row_flies_to_the_feature() {
        let mut c = core_with(vec![pt(-100.0, 40.0, 0), pt(100.0, -30.0, 1)]);
        c.select_row(Some(1), true);
        let mut now = Instant::now();
        for _ in 0..600 {
            now += std::time::Duration::from_millis(16);
            if !c.step_anim(now) {
                break;
            }
        }
        let s = c.camera.project([100.0, -30.0]);
        assert!((s[0] - 400.0).abs() < 1.0 && (s[1] - 300.0).abs() < 1.0, "{s:?}");
        assert_eq!(c.selected_row, Some(1));
    }

    #[test]
    fn offline_chip_after_three_failures_and_retry_clears() {
        let mut c = core_with(vec![pt(0.0, 0.0, 0)]);
        for i in 0..3 {
            let key = c.tiles.key(TileId { z: 1, x: i, y: 0 });
            c.tiles.accept(TileResult { key, image: Err("boom".into()) });
        }
        assert!(c.tiles.offline());
        assert_eq!(c.tiles.failed.len(), 3);
        c.tiles.retry();
        assert!(!c.tiles.offline() && c.tiles.failed.is_empty());
    }

    #[test]
    fn requests_pause_while_offline_until_retry() {
        let mut c = core_with(vec![pt(0.0, 0.0, 0)]);
        for i in 0..3 {
            let key = c.tiles.key(TileId { z: 1, x: i, y: 0 });
            c.tiles.accept(TileResult { key, image: Err("boom".into()) });
        }
        c.tiles.want(&[TileId { z: 2, x: 0, y: 0 }]);
        assert!(c.tiles.last_plan.is_empty(), "paused: nothing is requested");
        c.tiles.retry();
        c.tiles.want(&[TileId { z: 2, x: 1, y: 1 }]);
        assert_eq!(c.tiles.last_plan, vec![TileId { z: 2, x: 1, y: 1 }]);
    }

    /// How many times a feature is painted: the copies from `copy_queries` that find it.
    fn painted_copies(c: &Core, id: usize, size: [f64; 2]) -> Vec<f64> {
        let t = c.camera.transform();
        let mut ids = Vec::new();
        let mut out = Vec::new();
        for (k, q) in c.copy_queries(&t, size, 24.0) {
            c.index.query_into(&q, &mut ids);
            if ids.contains(&(id as u32)) {
                out.push(k);
            }
        }
        out
    }

    #[test]
    fn a_feature_is_painted_once_however_many_worlds_fit_the_viewport() {
        let pts: Vec<_> = [(-170.0, 10.0), (-90.0, 40.0), (0.0, 0.0), (100.0, -30.0), (175.0, 60.0)]
            .iter()
            .enumerate()
            .map(|(i, p)| pt(p.0, p.1, i))
            .collect();
        for size in [[1320.0, 240.0], [900.0, 420.0], [500.0, 800.0]] {
            let mut c = core_with(pts.clone());
            // zoomed out to the floor, centred anywhere
            for center_lon in [0.0, 40.0, -150.0, 179.0] {
                c.camera = Camera::Geo(theta_geo::MapView { center: [center_lon, 20.0], zoom: -5.0, size });
                if let Camera::Geo(v) = &mut c.camera {
                    v.clamp_zoom();
                }
                for id in 0..pts.len() {
                    let at = painted_copies(&c, id, size);
                    // every point whose copy lies in the viewport is painted exactly once
                    let t = c.camera.transform();
                    let x = c.prepared.features[id].bbox.min[0];
                    let visible_copy = at.iter().filter(|k| {
                        let s = t.apply([x + **k, 0.0])[0];
                        (-24.0..=size[0] + 24.0).contains(&s)
                    });
                    assert!(
                        visible_copy.count() <= 1,
                        "{size:?} centre {center_lon}: point {id} copies {at:?}"
                    );
                    assert!(at.len() <= 1, "{size:?} centre {center_lon}: point {id} copies {at:?}");
                }
            }
        }
    }

    #[test]
    fn every_world_point_is_visible_when_the_world_is_narrower_than_the_panel() {
        // 25 points all around the globe, a 1320x240 panel fitted to them: all inside, none twice.
        let pts: Vec<_> = (0..25)
            .map(|i| {
                let f = f64::from(i);
                pt(-170.0 + f * 14.0, -35.0 + (f * 37.0) % 90.0, i as usize)
            })
            .collect();
        let size = [1320.0, 240.0];
        let c = core_with_size(pts.clone(), size);
        let t = c.camera.transform();
        assert!(t.scale[0] < size[0], "more than one world fits the panel");
        for id in 0..pts.len() {
            let at = painted_copies(&c, id, size);
            assert_eq!(at.len(), 1, "point {id} is painted once: {at:?}");
            let b = c.prepared.features[id].bbox;
            let s = t.apply([b.min[0] + at[0], b.min[1]]);
            assert!(s[0] >= 0.0 && s[0] <= size[0] && s[1] >= 0.0 && s[1] <= size[1], "point {id} at {s:?}");
        }
    }

    #[test]
    fn hit_testing_uses_the_painted_copy_only() {
        let mut c = core_with(vec![pt(0.0, 0.0, 0), pt(100.0, 30.0, 1)]);
        let size = [1320.0, 240.0];
        c.camera = Camera::Geo(theta_geo::MapView { center: [0.0, 15.0], zoom: -5.0, size });
        if let Camera::Geo(v) = &mut c.camera {
            v.clamp_zoom();
        }
        let t = c.camera.transform();
        let w = t.scale[0]; // pixels per world
        // On the painted copy (the one centred on the camera) the point is found ...
        let s = c.camera.project([100.0, 30.0]);
        assert_eq!(c.hit_at(s).map(|i| c.prepared.features[i].row), Some(1));
        // ... and one world to the right or left, where only the dimmed basemap repeats, it is not.
        for dir in [-1.0, 1.0] {
            let other = [s[0] + dir * w, s[1]];
            if (0.0..size[0]).contains(&other[0]) {
                assert_eq!(c.hit_at(other), None, "no feature on a repeated copy of the basemap");
            }
        }
        // The measure tool sees the same copy: a click on a repeated copy reads as the real lon/lat.
        let click = [s[0] + w, s[1]];
        let ll = c.measure_coord_at(click);
        assert!((ll[0] - 100.0).abs() < 1e-6 || (ll[0] + 260.0).abs() < 1e-6, "{ll:?}");
        let back = c.measure_screen(ll);
        assert!(
            (back[0] - s[0]).abs() < 1e-6,
            "stored coordinates are drawn on the central copy: {back:?} {s:?}"
        );
    }

    #[test]
    fn a_feature_on_the_window_edge_is_in_one_copy_not_two() {
        let window = copy_window(&Transform { half: [0.0, 0.0], center: [0.5, 0.5], scale: [1.0, 1.0] });
        let at = |x: f64| BBox { min: [x, 0.0], max: [x, 0.0] };
        for x in [0.0, 1.0] {
            let n = (-3..=3).filter(|k| drawn_at(&at(x), f64::from(*k), window)).count();
            assert_eq!(n, 1, "x = {x}");
        }
        // A polygon spanning the seam is in both copies next to it.
        let wide = BBox { min: [0.9, 0.0], max: [1.0, 0.0] };
        let span = BBox { min: [0.0, 0.0], max: [1.0, 0.0] };
        assert!(drawn_at(&wide, 0.0, window) && !drawn_at(&wide, 1.0, window));
        assert!((-1..=1).filter(|k| drawn_at(&span, f64::from(*k), window)).count() >= 1);
    }

    #[test]
    fn planar_data_is_never_repeated() {
        let mut c = Core::new("http://127.0.0.1:9/{z}/{x}/{y}.png".into());
        c.set_features(
            vec![MapLayer { name: "geom".into(), visible: true }],
            vec![MapFeature {
                feature: Feature::new(Geometry::Point(Some([5000.0, 4000.0])), None),
                row: 0,
                label: None,
                properties: vec![],
                layer: 0,
            }],
        );
        c.camera.set_size([800.0, 600.0]);
        let t = c.camera.transform();
        assert_eq!(c.copy_queries(&t, [800.0, 600.0], 24.0).len(), 1);
    }

    #[test]
    fn stale_template_results_are_ignored() {
        let mut c = core_with(vec![pt(0.0, 0.0, 0)]);
        let old = c.tiles.key(TileId { z: 1, x: 0, y: 0 });
        c.tiles.set_template("http://other/{z}/{x}/{y}.png".into());
        c.tiles.accept(TileResult { key: old, image: Err("x".into()) });
        assert_eq!(c.tiles.consecutive_failures, 0);
    }

    #[test]
    fn grid_lines_planar_include_axis() {
        let cam = Camera::Planar(theta_geo::PlanarView {
            center: [0.0, 0.0],
            units_per_px: 1.0,
            size: [600.0, 400.0],
        });
        let g = grid_spec(&cam);
        assert!(g.vertical.iter().any(|l| l.axis));
        assert!(g.vertical.len() >= 4 && g.horizontal.len() >= 3);
    }
}
