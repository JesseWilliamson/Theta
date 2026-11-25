//! The layer model at run time: resolved styles, colour classes, the tile pyramid and the
//! aggregation results, plus the `Core` methods that keep them consistent.

use crate::agg::{AggDone, AggJob, AggOutput, AggSource, AggView, AggWorker, BinResult, Cluster};
use crate::camera::Transform;
use crate::core::Core;
use crate::layers::{ColStat, Coloring, Legend, bins_legend, color_layer, default_rgb, heat_legend, radii};
use crate::style::{LayerKind, LayerStyle, tiling_enabled};
use crate::tiler::{CACHE_BYTES, Tile, TileCache, TileDone, TileKey, TilePool, TileSource, TileSpace};
use crate::world::Mode;
use gpui_kit::RenderImage;
use image::{Frame, ImageBuffer};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

static DEFAULT_STYLE: LazyLock<LayerStyle> = LazyLock::new(LayerStyle::default);

/// Per-layer facts derived from the data and the style.
#[derive(Clone, Debug, Default)]
pub struct LayerRt {
    /// Features in the layer.
    pub features: usize,
    /// Vertices in the layer.
    pub vertices: usize,
    /// Only point geometry.
    pub only_points: bool,
    /// The colour classes.
    pub coloring: Coloring,
    /// First slot of the layer (slot = base + class).
    pub base_slot: usize,
    /// Drawn from the tile pyramid.
    pub tiled: bool,
}

/// The latest aggregation result of a layer and what it was computed for.
pub struct AggState {
    /// The kind it belongs to (a result for another kind is not shown).
    pub kind: LayerKind,
    /// The view it was computed for; painting maps it onto the current one.
    pub view: AggView,
    /// The result.
    pub out: AggOutput,
    /// Heatmap image.
    pub image: Option<Arc<RenderImage>>,
    /// Column used for the legend title of bins.
    pub bin_column: Option<String>,
}

/// Counters for the performance report.
#[derive(Clone, Copy, Debug, Default)]
pub struct Perf {
    /// Tiles built since the last take.
    pub tiles_built: u32,
    /// Milliseconds the last aggregation took on its worker.
    pub last_agg_ms: f64,
}

/// See the module docs.
pub struct LayerState {
    /// Data-set counter.
    pub generation: u64,
    /// Per-layer facts.
    pub rt: Vec<LayerRt>,
    /// Column statistics per layer.
    pub stats: Vec<Vec<ColStat>>,
    /// Colour class per prepared feature.
    pub class_of: Vec<u8>,
    /// Marker radius per prepared feature.
    pub radius_of: Vec<f32>,
    /// Total colour slots.
    pub n_slots: usize,
    /// Bumped when a layer's style changes.
    pub style_version: Vec<u64>,
    /// Inputs of tile builds.
    pub tile_source: Option<Arc<TileSource>>,
    /// Built tiles.
    pub tiles: TileCache,
    /// Tile workers.
    pub pool: TilePool,
    /// The last job list sent to the pool.
    pub last_plan: Vec<TileKey>,
    /// Tiles painted in the last frame (kept feature ids are used by hit testing).
    pub drawn: Vec<(usize, Arc<Tile>)>,
    /// Aggregation worker.
    pub agg_worker: AggWorker,
    /// Inputs of aggregation jobs.
    pub agg_source: Option<Arc<AggSource>>,
    /// Results per layer.
    pub agg: HashMap<usize, AggState>,
    /// What each layer's last request was for.
    pub agg_req: HashMap<usize, (Transform, [f64; 2], u64)>,
    /// Request counter.
    pub seq: u64,
    /// Dark theme (default hues differ).
    pub dark: bool,
    /// Images to release from the GPU atlas on the next paint.
    pub retired: Vec<Arc<RenderImage>>,
    /// Counters.
    pub perf: Perf,
    /// Debug overlay: screen rectangle `[x0, y0, x1, y1]` and label of each exact tile drawn.
    pub overlay: Vec<([f64; 4], String)>,
    /// Tiles requested but not yet built for the current view.
    pub tiles_missing: usize,
}

impl Default for LayerState {
    fn default() -> Self {
        LayerState {
            generation: 0,
            rt: Vec::new(),
            stats: Vec::new(),
            class_of: Vec::new(),
            radius_of: Vec::new(),
            n_slots: 1,
            style_version: Vec::new(),
            tile_source: None,
            tiles: TileCache::new(CACHE_BYTES),
            pool: TilePool::new(),
            last_plan: Vec::new(),
            drawn: Vec::new(),
            agg_worker: AggWorker::new(),
            agg_source: None,
            agg: HashMap::new(),
            agg_req: HashMap::new(),
            seq: 0,
            dark: true,
            retired: Vec::new(),
            perf: Perf::default(),
            overlay: Vec::new(),
            tiles_missing: 0,
        }
    }
}

impl Core {
    /// The resolved style of `layer`.
    pub fn style(&self, layer: usize) -> &LayerStyle {
        self.layers.get(layer).and_then(|l| l.style.as_ref()).unwrap_or(&DEFAULT_STYLE)
    }

    /// The kind of `layer`.
    pub fn kind(&self, layer: usize) -> LayerKind {
        self.style(layer).kind
    }

    /// Whether `layer` is drawn from the tile pyramid.
    pub fn is_tiled(&self, layer: usize) -> bool {
        self.ls.rt.get(layer).is_some_and(|r| r.tiled)
    }

    /// The colour slot of prepared feature `id`.
    pub fn slot(&self, id: usize) -> usize {
        let l = self.prepared.features[id].layer;
        self.ls.rt.get(l).map_or(0, |r| r.base_slot) + usize::from(self.ls.class_of[id])
    }

    /// RGB of every slot for the current theme.
    pub fn slot_rgb(&self) -> Vec<[u8; 3]> {
        let mut out = Vec::with_capacity(self.ls.n_slots);
        for (li, r) in self.ls.rt.iter().enumerate() {
            if r.coloring.colors.is_empty() {
                out.push(default_rgb(li, self.ls.dark));
            } else {
                out.extend_from_slice(&r.coloring.colors);
            }
        }
        out.resize(self.ls.n_slots.max(1), [128, 128, 128]);
        out
    }

    /// Start over for a new data set: counters, caches, workers' inputs, resolved styles.
    pub fn reset_layer_state(&mut self) {
        let ls = &mut self.ls;
        ls.generation += 1;
        ls.tiles.clear();
        ls.last_plan.clear();
        ls.drawn.clear();
        ls.agg_worker.cancel_all();
        ls.pool.set_wanted(Vec::new());
        let retired: Vec<_> = ls.agg.drain().filter_map(|(_, a)| a.image).collect();
        ls.retired.extend(retired);
        ls.agg_req.clear();
        let space = match self.prepared.mode {
            Mode::Geographic => TileSpace::geographic(),
            Mode::Planar => TileSpace::planar(self.prepared.bbox),
        };
        self.ls.tile_source = Some(Arc::new(TileSource {
            generation: self.ls.generation,
            prepared: self.prepared.clone(),
            index: self.index.clone(),
            space,
        }));
        self.ls.agg_source = Some(Arc::new(AggSource {
            generation: self.ls.generation,
            prepared: self.prepared.clone(),
            features: self.source.clone(),
            columns: self.layers.iter().map(|l| l.columns.clone()).collect(),
        }));
        self.ls.style_version = vec![0; self.layers.len()];
        self.apply_styles();
    }

    /// Recompute colours, radii and per-layer flags from the styles.
    pub fn apply_styles(&mut self) {
        let n = self.prepared.features.len();
        let nl = self.layers.len();
        let mut feats = vec![0usize; nl];
        let mut verts = vec![0usize; nl];
        let mut non_points = vec![false; nl];
        for f in self.prepared.features.iter() {
            if f.layer < nl {
                feats[f.layer] += 1;
                verts[f.layer] += f.vertices;
                non_points[f.layer] |= f.kinds.lines || f.kinds.polygons;
            }
        }
        self.ls.class_of = vec![0; n];
        self.ls.radius_of = vec![5.0; n];
        let mut rt = Vec::with_capacity(nl);
        let mut base = 0;
        for li in 0..nl {
            let only_points = !non_points[li];
            let columns = self.layers[li].columns.clone();
            let st = self.layers[li].style.get_or_insert_with(|| LayerStyle::automatic(feats[li], only_points));
            st.sanitize(&columns);
            let st = st.clone();
            let coloring = color_layer(&st, li, &columns, &self.prepared, &self.source, &mut self.ls.class_of);
            radii(&st, li, &columns, &self.prepared, &self.source, &mut self.ls.radius_of);
            let slots = coloring.colors.len().max(1);
            let tiled = matches!(st.kind, LayerKind::Auto | LayerKind::Points)
                && tiling_enabled(&st.tiling, feats[li], verts[li]);
            rt.push(LayerRt { features: feats[li], vertices: verts[li], only_points, coloring, base_slot: base, tiled });
            base += slots;
        }
        self.ls.n_slots = base.max(1);
        self.ls.rt = rt;
    }

    /// Edit the style of `layer`; everything derived from it is recomputed.
    pub fn update_style(&mut self, layer: usize, edit: impl FnOnce(&mut LayerStyle)) {
        let Some(l) = self.layers.get_mut(layer) else { return };
        let Some(st) = l.style.as_mut() else { return };
        edit(st);
        if let Some(v) = self.ls.style_version.get_mut(layer) {
            *v += 1;
        }
        self.ls.agg_req.remove(&layer);
        self.ls.last_plan.clear();
        self.apply_styles();
    }

    /// Absorb a built tile; true when it belongs to the current data.
    pub fn accept_tile(&mut self, d: TileDone) -> bool {
        if d.generation != self.ls.generation {
            return false;
        }
        self.ls.perf.tiles_built += 1;
        self.ls.tiles.insert(d.key, d.tile);
        true
    }

    /// Absorb an aggregation result; true when it should be shown.
    pub fn accept_agg(&mut self, d: AggDone) -> bool {
        if d.generation != self.ls.generation || d.layer >= self.layers.len() {
            return false;
        }
        self.ls.perf.last_agg_ms = d.millis;
        let kind = self.kind(d.layer);
        let image = match &d.out {
            AggOutput::Heat(h) => ImageBuffer::from_raw(h.w as u32, h.h as u32, h.bgra.clone())
                .map(|b: ImageBuffer<image::Rgba<u8>, Vec<u8>>| Arc::new(RenderImage::new(vec![Frame::new(b)]))),
            _ => None,
        };
        let bin_column = self.style(d.layer).bins.column.clone();
        if let Some(old) = self.ls.agg.insert(d.layer, AggState { kind, view: d.view, out: d.out, image, bin_column })
            && let Some(img) = old.image
        {
            self.ls.retired.push(img);
        }
        true
    }

    /// Queue an aggregation for `layer` unless the last request already covers this view.
    pub fn request_agg(&mut self, layer: usize, view: AggView) {
        let ver = self.ls.style_version.get(layer).copied().unwrap_or(0);
        let key = (view.transform, view.size, ver);
        if self.ls.agg_req.get(&layer) == Some(&key) {
            return;
        }
        let Some(source) = self.ls.agg_source.clone() else { return };
        self.ls.agg_req.insert(layer, key);
        self.ls.seq += 1;
        let job = AggJob { layer, source, style: self.style(layer).clone(), view, seq: self.ls.seq };
        self.ls.agg_worker.submit(job);
    }

    /// Whether an aggregation is requested but its result has not arrived.
    pub fn agg_pending(&self) -> bool {
        self.layers.iter().enumerate().any(|(i, l)| {
            l.visible
                && self.kind(i).is_aggregate()
                && self.ls.agg.get(&i).is_none_or(|a| a.kind != self.kind(i))
        })
    }

    /// Legends of the visible layers: colour-by-column and aggregation scales.
    pub fn legends(&self) -> Vec<Legend> {
        let mut out = Vec::new();
        for (i, l) in self.layers.iter().enumerate() {
            if !l.visible {
                continue;
            }
            let st = self.style(i);
            match st.kind {
                LayerKind::Auto | LayerKind::Points => out.extend(self.ls.rt[i].coloring.legend.clone()),
                LayerKind::Heatmap => out.push(heat_legend(&st.heatmap, st.heatmap.weight.as_deref())),
                LayerKind::Hexagons | LayerKind::Grid => {
                    if let Some(AggState { out: AggOutput::Bins(b), kind, bin_column, .. }) = self.ls.agg.get(&i)
                        && *kind == st.kind
                    {
                        out.push(bins_legend(b, bin_column.as_deref()));
                    }
                }
                LayerKind::Clusters => {}
            }
        }
        out
    }

    /// The bin result of `layer`, if current.
    pub fn bins_of(&self, layer: usize) -> Option<&BinResult> {
        match self.ls.agg.get(&layer) {
            Some(AggState { out: AggOutput::Bins(b), kind, .. }) if *kind == self.kind(layer) => Some(b),
            _ => None,
        }
    }

    /// A line describing the hexagon or grid cell under `p`, e.g. "Sum 1,234 · 56 features".
    pub fn bin_hover(&self, p: [f64; 2]) -> Option<String> {
        let t = self.camera.transform();
        let w = t.invert(p);
        for (i, l) in self.layers.iter().enumerate().rev() {
            if !l.visible || !matches!(self.kind(i), LayerKind::Hexagons | LayerKind::Grid) {
                continue;
            }
            let Some(b) = self.bins_of(i) else { continue };
            {
                let key = crate::agg::cell_at(b.hex, b.cell_world, b.scale, w);
                if let Some(&ci) = b.lookup.get(&key) {
                    let c = &b.cells[ci];
                    let v = crate::classify::short_number(c.value);
                    let n = c.count;
                    let feats = format!("{n} feature{}", if n == 1 { "" } else { "s" });
                    return Some(match b.what {
                        crate::style::BinValue::Count => feats,
                        w => format!("{} {v} \u{00b7} {feats}", w.label()),
                    });
                }
            }
        }
        None
    }

    /// The cluster circle under `p` (screen px), topmost first. Single points report their feature.
    pub fn cluster_under(&self, p: [f64; 2]) -> Option<(usize, Cluster)> {
        let t = self.camera.transform();
        for (i, l) in self.layers.iter().enumerate().rev() {
            if !l.visible || self.kind(i) != LayerKind::Clusters {
                continue;
            }
            let Some(AggState { out: AggOutput::Clusters(cl), kind: LayerKind::Clusters, view, .. }) = self.ls.agg.get(&i) else {
                continue;
            };
            let mut best: Option<(f64, &Cluster)> = None;
            for (k, rect) in &view.copies {
                let k = *k;
                for c in cl.iter() {
                    if c.xy[0] < rect.min[0] - 0.01 || c.xy[0] > rect.max[0] + 0.01 {
                        continue;
                    }
                    let s = t.apply([c.xy[0] + k, c.xy[1]]);
                    let d = (s[0] - p[0]).hypot(s[1] - p[1]);
                    if d <= f64::from(crate::agg::cluster_radius(c.count)) + 2.0 && best.is_none_or(|b| d < b.0) {
                        best = Some((d, c));
                    }
                }
            }
            if let Some((_, c)) = best {
                return Some((i, c.clone()));
            }
        }
        None
    }

    /// True when `id` was kept by the tiles currently drawn (or its layer is not tiled), so
    /// hover and click only find what the user can actually see.
    pub fn kept_in_tiles(&self, id: usize) -> bool {
        let layer = self.prepared.features[id].layer;
        if !self.is_tiled(layer) {
            return true;
        }
        let id = id as u32;
        self.ls.drawn.iter().any(|(l, t)| *l == layer && t.ids.binary_search(&id).is_ok())
    }

    /// Fly to frame a cluster's points.
    pub fn zoom_to_cluster(&mut self, c: &Cluster) {
        let size = self.camera.size();
        let mut target = crate::camera::Camera::fit_boxes(self.prepared.mode, std::iter::once(c.bbox), size, 90.0);
        // Identical points would zoom to the camera limit; two levels in is a useful step.
        if c.bbox.width() == 0.0 && c.bbox.height() == 0.0 {
            let mid = [size[0] / 2.0, size[1] / 2.0];
            target.zoom_around(mid, (self.camera.level() + 2.0 - target.level()).clamp(-30.0, 30.0));
            target.center_on(c.xy);
        }
        self.fly_to(&target);
    }
}
