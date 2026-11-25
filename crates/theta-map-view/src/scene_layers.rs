//! Painting of the layer kinds beyond Auto: tiled geometry, heatmaps, hexagon/grid bins,
//! clusters and the tile-boundary debug overlay.

use crate::agg::{AggView, cluster_radius, hex_corners};
use crate::core::Core;
use crate::layerstate::AggState;
use crate::scene::{
    CIRCLE_LIMIT, CellGrid, Cx, MARGIN, MIN_POLY_HALF, Meshes, SlotInfo, Strokes, circle, fill_path, line_pieces,
    paint_text, polygon_rings,
};
use crate::style::LayerKind;
use crate::tiler::{Shape, Tile, TileId, TileJob, TileKey, TileOpts};
use gpui_kit::{App, BorderStyle, Bounds, ContentMask, Corners, Hsla, Window, fill, hsla, point, px, quad, size};
use std::collections::HashSet;
use std::sync::Arc;
use theta_geo::{BBox, Coord};

/// Draw the aggregate layers (heatmap, hexagons, grid, clusters), asking the worker for
/// fresh results when the view or style changed. The last result stays on screen, mapped
/// onto the current camera, until the new one arrives.
pub fn paint_aggregates(core: &mut Core, cx: &Cx, sz: [f64; 2], window: &mut Window, app: &mut App) {
    let copies: Vec<(f64, BBox)> = core.copy_queries(&cx.t, sz, 0.0);
    for layer in 0..core.layers.len() {
        let kind = core.kind(layer);
        if !core.layers[layer].visible || !kind.is_aggregate() {
            continue;
        }
        core.request_agg(layer, AggView { transform: cx.t, size: sz, copies: copies.clone() });
        let st = core.style(layer).clone();
        let Some(AggState { kind: k2, out, image, view, .. }) = core.ls.agg.get(&layer) else { continue };
        if *k2 != kind {
            continue;
        }
        match out {
            crate::agg::AggOutput::Heat(h) => {
                let Some(img) = image else { continue };
                let a = cx.t.apply(h.origin);
                let b = cx.t.apply([h.origin[0] + h.w as f64 * h.px_world[0], h.origin[1] + h.h as f64 * h.px_world[1]]);
                let r = Bounds { origin: cx.pt(a), size: size(px((b[0] - a[0]) as f32), px((b[1] - a[1]) as f32)) };
                let _ = window.paint_image(r, r, Corners::default(), img.clone(), 0, false);
            }
            crate::agg::AggOutput::Bins(b) => {
                let mut mesh = Meshes::new(b.colors.len().max(1));
                let alpha = 0.85 * st.opacity;
                let scale = cx.t.scale[0].abs();
                let lim = MARGIN + f64::from(st.bins.cell_px);
                {
                    for c in &b.cells {
                        let s = cx.t.apply(c.center);
                        if s[0] < -lim || s[0] > sz[0] + lim || s[1] < -lim || s[1] > sz[1] + lim {
                            continue;
                        }
                        let slot = usize::from(c.class).min(b.colors.len().saturating_sub(1));
                        let col = rgb(b.colors[slot], alpha);
                        let m = mesh.slot(slot);
                        if b.hex {
                            // A 4% gap shows the cells as separate hexagons.
                            let ring = hex_corners(b.cell_world * 0.96, c.center);
                            let pts: Vec<_> = ring.iter().map(|p| cx.pt(cx.t.apply(*p))).collect();
                            for i in 0..6 {
                                m.triangle(cx.pt(s), pts[i], pts[(i + 1) % 6], col, window);
                            }
                        } else {
                            let half = (b.cell_world * scale * 0.5 - 0.5).max(0.5) as f32;
                            m.rect(cx.pt(s), half, half, col, window);
                        }
                    }
                }
                mesh.flush(|s| rgb(b.colors[s.min(b.colors.len().saturating_sub(1))], alpha), window);
            }
            crate::agg::AggOutput::Clusters(cl) => {
                let base = rgb(crate::layers::default_rgb(layer, cx.colors.dark), 0.88 * st.opacity);
                let ink = if cx.colors.dark { hsla(0.0, 0.0, 0.08, 1.0) } else { hsla(0.0, 0.0, 1.0, 1.0) };
                let rim = hsla(0.0, 0.0, 1.0, 0.9);
                let mut labels: Vec<(Coord, String)> = Vec::new();
                for (k, rect) in &view.copies {
                    let k = *k;
                    for c in cl.iter() {
                        if c.xy[0] < rect.min[0] - 0.01 || c.xy[0] > rect.max[0] + 0.01 {
                            continue;
                        }
                        let s = cx.t.apply([c.xy[0] + k, c.xy[1]]);
                        let r = cluster_radius(c.count);
                        if s[0] < -f64::from(r) || s[0] > sz[0] + f64::from(r) || s[1] < -f64::from(r) || s[1] > sz[1] + f64::from(r) {
                            continue;
                        }
                        circle(cx, s, r, base, 1.5, rim, window);
                        if c.count > 1 {
                            labels.push((s, crate::classify::short_number(f64::from(c.count))));
                        }
                    }
                }
                for (s, text) in labels {
                    let origin = cx.pt([s[0], s[1] - 6.5]);
                    paint_text(window, app, &text, origin, 11.0, ink, true);
                }
            }
        }
    }
}

fn rgb(c: [u8; 3], a: f32) -> Hsla {
    Hsla::from(gpui_kit::Rgba { r: f32::from(c[0]) / 255.0, g: f32::from(c[1]) / 255.0, b: f32::from(c[2]) / 255.0, a })
}

/// Round a screen rectangle to whole pixels (both edges), as the basemap tiles are, so
/// neighbouring tiles share an edge exactly.
fn snapped(cx: &Cx, a: [f64; 2], b: [f64; 2]) -> [f64; 4] {
    let (ox, oy) = (f64::from(f32::from(cx.origin.x)), f64::from(f32::from(cx.origin.y)));
    let (x0, x1) = (a[0].min(b[0]), a[0].max(b[0]));
    let (y0, y1) = (a[1].min(b[1]), a[1].max(b[1]));
    [(ox + x0).round() - ox, (oy + y0).round() - oy, (ox + x1).round() - ox, (oy + y1).round() - oy]
}

/// Draw the layers that use the tile pyramid and request the tiles the view needs.
pub fn paint_tiled(core: &mut Core, cx: &Cx, sz: [f64; 2], window: &mut Window, app: &mut App) {
    core.ls.drawn.clear();
    core.ls.overlay.clear();
    let Some(src) = core.ls.tile_source.clone() else { return };
    let queries = core.copy_queries(&cx.t, sz, MARGIN);
    let info = SlotInfo::new(core);
    let mut coarse: Vec<TileJob> = Vec::new();
    let mut wanted: Vec<TileJob> = Vec::new();
    let mut plan: Vec<TileKey> = Vec::new();
    let mut queued: HashSet<TileKey> = HashSet::new();
    let mut missing = 0;
    let mut cells = CellGrid::new(sz, core.ls.n_slots.max(1));
    let mut many_points = 0usize;
    // First pass: decide tiles, queue missing ones and count points for the marker style.
    let mut layer_tiles: Vec<(usize, TileOpts, Vec<(f64, TileId)>)> = Vec::new();
    for layer in 0..core.layers.len() {
        if !core.layers[layer].visible || !core.is_tiled(layer) {
            continue;
        }
        let st = core.style(layer);
        let opts = TileOpts::from_style(&st.tiling);
        let z = src.space.zoom_for(cx.t.scale[0].abs(), opts.max_zoom);
        let mut vis: Vec<(f64, TileId)> = Vec::new();
        for (k, rect) in &queries {
            // The query rectangle is in data space of that copy already (see `copy_queries`).
            for id in src.space.tiles_for(z, rect) {
                vis.push((*k, id));
            }
        }
        let opts_key = opts.key();
        for (_, id) in &vis {
            let key = TileKey { layer, opts: opts_key, id: *id };
            core.ls.tiles.touch(&key);
            match core.ls.tiles.peek(&key) {
                Some(t) => many_points += t.points.len(),
                None => {
                    missing += 1;
                    for up in [3u8, 2] {
                        if let Some((a, _)) = id.ancestor(up) {
                            let ak = TileKey { layer, opts: opts_key, id: a };
                            if !core.ls.tiles.contains(&ak) && queued.insert(ak) {
                                coarse.push(TileJob { key: ak, source: src.clone(), opts: opts.clone() });
                                plan.push(ak);
                            }
                        }
                    }
                    if queued.insert(key) {
                        wanted.push(TileJob { key, source: src.clone(), opts: opts.clone() });
                        plan.push(key);
                    }
                }
            }
        }
        layer_tiles.push((layer, opts, vis));
    }
    core.ls.tiles_missing = missing;
    if plan != core.ls.last_plan {
        core.ls.last_plan = plan;
        coarse.extend(wanted);
        core.ls.pool.set_wanted(coarse);
    }
    let squares = many_points > CIRCLE_LIMIT;
    let mut buf: Vec<[f64; 2]> = Vec::new();
    for (layer, opts, vis) in layer_tiles {
        let st = core.style(layer).clone();
        let opts_key = opts.key();
        for (k, id) in vis {
            let b = src.space.bounds(id);
            let a = cx.t.apply([b.min[0] + k, b.min[1]]);
            let c = cx.t.apply([b.max[0] + k, b.max[1]]);
            let rect = snapped(cx, a, c);
            let key = TileKey { layer, opts: opts_key, id };
            let (tile, exact) = match core.ls.tiles.peek(&key) {
                Some(t) => (Some(t.clone()), true),
                None => {
                    let mut found = None;
                    for up in 1..=6u8 {
                        let Some((anc, _)) = id.ancestor(up) else { break };
                        if let Some(t) = core.ls.tiles.peek(&TileKey { layer, opts: opts_key, id: anc }) {
                            found = Some(t.clone());
                            break;
                        }
                    }
                    (found, false)
                }
            };
            let Some(tile) = tile else { continue };
            core.ls.drawn.push((layer, tile.clone()));
            if st.tiling.show_boundaries {
                let s = tile.stats;
                let label = if exact {
                    format!("{}/{}/{}  kept {}  dropped {}", id.z, id.x, id.y, s.kept, s.dropped_tiny + s.dropped_thinned)
                } else {
                    format!("{}/{}/{}  building\u{2026}", id.z, id.x, id.y)
                };
                core.ls.overlay.push((rect, label));
            }
            let mask = Bounds {
                origin: cx.pt([rect[0], rect[1]]),
                size: size(px((rect[2] - rect[0]) as f32), px((rect[3] - rect[1]) as f32)),
            };
            window.with_content_mask(Some(ContentMask { bounds: mask }), |window| {
                draw_tile(core, cx, &info, layer, &tile, k, src.space.bounds(id), squares, &mut cells, &mut buf, window);
            });
        }
    }
    crate::scene::paint_tiled_highlights(core, cx, sz, window);
    let _ = app;
}

#[allow(clippy::too_many_arguments)]
fn draw_tile(
    core: &Core,
    cx: &Cx,
    info: &SlotInfo,
    layer: usize,
    tile: &Arc<Tile>,
    k: f64,
    own: BBox,
    squares: bool,
    cells: &mut CellGrid,
    buf: &mut Vec<[f64; 2]>,
    window: &mut Window,
) {
    let n = core.ls.n_slots.max(1);
    let st = core.style(layer);
    let scatter = st.kind == LayerKind::Points;
    let (fill_a, op) = info.fill_alpha(core, layer);
    let mut fills = Meshes::new(n);
    let mut boxes = Meshes::new(n);
    let mut outlines = Strokes::new(info.widths(core, 0.0));
    let mut lines = Strokes::new(info.widths(core, 0.5));
    let mut marks = Meshes::new(n);
    let inside = |c: [f64; 2]| c[0] >= -8.0 && c[0] <= cx.clip[2] && c[1] >= -8.0 && c[1] <= cx.clip[3];
    let white = hsla(0.0, 0.0, 1.0, 0.95);
    let mut centred: HashSet<u32> = HashSet::new();
    if !scatter {
        for (fid, shape) in &tile.shapes {
            let slot = core.slot(*fid as usize);
            let col = info.color(slot);
            match shape {
                Shape::Poly(rings) => {
                    let rings = polygon_rings(cx, k, rings, buf);
                    let Some(ext) = rings.first() else { continue };
                    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
                    for p in ext {
                        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
                        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
                    }
                    if hi[0] - lo[0] < 3.0 && hi[1] - lo[1] < 3.0 {
                        let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
                        let half = ((hi[0] - lo[0]) / 2.0).max(MIN_POLY_HALF) as f32;
                        boxes.slot(slot).square(cx.pt(c), half, col.opacity(0.85 * op), window);
                        continue;
                    }
                    if fill_a > 0.0
                        && let Some(path) = fill_path(cx, &rings)
                    {
                        fills.slot(slot).append(path, col.opacity(fill_a), window);
                    }
                    for r in &rings {
                        outlines.add(cx, slot, r, true, col.opacity(op.max(0.3)), window);
                    }
                }
                Shape::Line(l) => {
                    for piece in line_pieces(cx, k, l, buf) {
                        lines.add(cx, slot, &piece, false, col.opacity(op), window);
                    }
                }
            }
        }
    } else {
        // A scatterplot draws each non-point feature once, at its centre, in the tile that
        // contains that centre.
        for (fid, _) in &tile.shapes {
            let c = core.prepared.features[*fid as usize].bbox.center();
            if c[0] >= own.min[0] && c[0] < own.max[0] && c[1] >= own.min[1] && c[1] < own.max[1] && centred.insert(*fid) {
                let s = cx.t.apply([c[0] + k, c[1]]);
                let slot = core.slot(*fid as usize);
                let r = core.ls.radius_of[*fid as usize];
                marker(cx, cells, &mut marks, layer, slot, s, r, info.color(slot).opacity(op), squares, white, &inside, window);
            }
        }
    }
    fills.flush(|s| info.color(s).opacity(fill_a), window);
    boxes.flush(|s| info.color(s).opacity(0.85 * op), window);
    outlines.flush(|s| info.color(s).opacity(op.max(0.3)), window);
    lines.flush(|s| info.color(s).opacity(op), window);
    for (fid, c) in &tile.points {
        let s = cx.t.apply([c[0] + k, c[1]]);
        let slot = core.slot(*fid as usize);
        let r = core.ls.radius_of[*fid as usize];
        marker(cx, cells, &mut marks, layer, slot, s, r, info.color(slot).opacity(op), squares, white, &inside, window);
    }
    marks.flush(|s| info.color(s).opacity(op), window);
}

#[allow(clippy::too_many_arguments)]
fn marker(
    cx: &Cx,
    cells: &mut CellGrid,
    marks: &mut Meshes,
    layer: usize,
    slot: usize,
    s: [f64; 2],
    r: f32,
    color: Hsla,
    squares: bool,
    rim: Hsla,
    inside: &dyn Fn([f64; 2]) -> bool,
    window: &mut Window,
) {
    if !inside(s) {
        return;
    }
    if squares {
        // One marker per screen cell and layer, whatever its colour class, so dense areas are not
        // painted class by class on top of each other.
        if cells.claim(layer, s) {
            marks.slot(slot).square(cx.pt(s), (r * 0.8).clamp(1.5, 6.0), color, window);
        }
    } else {
        circle(cx, s, r, color, 1.5, rim, window);
    }
}

/// The tile-boundary debug overlay: an outline and a label for each tile drawn.
pub fn paint_boundaries(core: &Core, cx: &Cx, window: &mut Window, app: &mut App) {
    if core.ls.overlay.is_empty() {
        return;
    }
    let line = cx.colors.accent.opacity(0.85);
    let ink = cx.colors.fg;
    let halo = cx.colors.bg.opacity(0.8);
    for (r, label) in &core.ls.overlay {
        let (w, h) = (r[2] - r[0], r[3] - r[1]);
        let edge = |x: f64, y: f64, w: f64, h: f64, window: &mut Window| {
            let b = Bounds { origin: cx.pt([x, y]), size: size(px(w as f32), px(h as f32)) };
            window.paint_quad(fill(b, line));
        };
        edge(r[0], r[1], w, 1.0, window);
        edge(r[0], r[3] - 1.0, w, 1.0, window);
        edge(r[0], r[1], 1.0, h, window);
        edge(r[2] - 1.0, r[1], 1.0, h, window);
        if w > 90.0 && h > 24.0 {
            let b = Bounds { origin: cx.pt([r[0] + 4.0, r[1] + 4.0]), size: size(px((label.chars().count() as f32 * 6.0 + 8.0).min(w as f32 - 8.0)), px(16.0)) };
            window.paint_quad(quad(b, Corners::all(px(3.0)), halo, px(0.0), halo, BorderStyle::default()));
            paint_text(window, app, label, cx.pt([r[0] + 8.0, r[1] + 5.0]), 10.0, ink, false);
        }
    }
    let _ = point(px(0.0), px(0.0));
}
