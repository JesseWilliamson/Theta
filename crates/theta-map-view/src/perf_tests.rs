//! Timing of the heavy paths without a window. Ignored by default (they print numbers rather
//! than assert on them):
//!
//!     cargo test -p theta-map-view --lib perf_ -- --ignored --nocapture

use crate::agg::{AggView, bin, cluster, cluster_level, fine_grid, kde};
use crate::camera::Transform;
use crate::core::build;
use crate::index::GridIndex;
use crate::model::{MapFeature, MapLayer, MapValue};
use crate::style::{LayerStyle, TilingOptions};
use crate::tiler::{TileOpts, TileSpace, build_tile};
use std::sync::Arc;
use std::time::Instant;
use theta_geo::{Feature, Geometry};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }
    fn normal(&mut self) -> f64 {
        let (a, b) = (self.next().max(1e-12), self.next());
        (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
    }
}

fn points(n: usize) -> Vec<MapFeature> {
    let mut r = Rng(5);
    let centres: Vec<(f64, f64)> = (0..40).map(|_| (r.next() * 340.0 - 170.0, r.next() * 120.0 - 60.0)).collect();
    (0..n)
        .map(|i| {
            let (cx, cy) = centres[i % centres.len()];
            let p = [cx + r.normal() * 2.5, (cy + r.normal() * 1.8).clamp(-80.0, 80.0)];
            MapFeature {
                feature: Feature::new(Geometry::Point(Some(p)), Some(4326)),
                row: i,
                label: None,
                properties: vec![],
                values: vec![MapValue::Num((r.normal() * 0.9 + 2.0).exp())],
                layer: 0,
            }
        })
        .collect()
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn view(scale: f64, centre: [f64; 2]) -> AggView {
    AggView {
        transform: Transform { half: [800.0, 500.0], center: centre, scale: [scale, scale] },
        size: [1600.0, 1000.0],
        copies: vec![(0.0, theta_geo::BBox { min: [-1e9, -1e9], max: [1e9, 1e9] })],
    }
}

#[test]
#[ignore]
fn perf_one_million_points() {
    let feats = points(1_000_000);
    let layers = vec![MapLayer::new("p", vec!["w".into()])];
    let t = Instant::now();
    let built = build(layers, Arc::new(feats));
    println!("build (project + index + stats), 1,000,000 points: {:.0} ms", ms(t));
    let prep = &built.prepared;
    let t = Instant::now();
    let set = crate::agg::point_set(prep, 0);
    println!("point set: {:.0} ms", ms(t));
    let w = crate::agg::weights(&built.source, prep, &set, 0);

    let v = view(256.0 * 4.0, [0.5, 0.35]);
    let t = Instant::now();
    let fine = fine_grid(&set, Some(&w), v.transform.lod_level());
    println!("heatmap pre-bin at this zoom (O(n)): {:.0} ms ({} bins)", ms(t), fine.bins.len());
    let o = LayerStyle::default().heatmap;
    let t = Instant::now();
    let r = kde(&fine, &v, &o, 1.0, &|| false).unwrap();
    println!("heatmap KDE of the view (1600x1000, radius 20): {:.0} ms ({}x{} raster)", ms(t), r.w, r.h);
    let pan = view(256.0 * 4.0, [0.52, 0.35]);
    let t = Instant::now();
    let _ = kde(&fine, &pan, &o, 1.0, &|| false).unwrap();
    println!("heatmap pan (same zoom, pre-bin reused): {:.0} ms", ms(t));
    let zoom = view(256.0 * 8.0, [0.5, 0.35]);
    let t = Instant::now();
    let fine2 = fine_grid(&set, Some(&w), zoom.transform.lod_level());
    let _ = kde(&fine2, &zoom, &o, 1.0, &|| false).unwrap();
    println!("heatmap zoom step (new pre-bin + KDE): {:.0} ms", ms(t));

    let bo = LayerStyle::default().bins;
    let t = Instant::now();
    let b = bin(&set, Some(&w), &v, true, &crate::style::BinOptions { value: crate::style::BinValue::Sum, ..bo.clone() }, &|| false).unwrap();
    println!("hexagon bins, sum (28 px cells): {:.0} ms ({} cells)", ms(t), b.cells.len());
    let t = Instant::now();
    let c = cluster(&set, cluster_level(v.transform.scale[0]), 56.0, &|| false).unwrap();
    println!("clusters at one zoom: {:.0} ms ({} clusters)", ms(t), c.len());

    // Tiles: the whole world at z=2 (every point in view) and a city-zoom tile.
    let space = TileSpace::geographic();
    let opts = TileOpts::from_style(&TilingOptions::default());
    let idx: &GridIndex = &built.index;
    let all = crate::tiler::TileId { z: 2, x: 2, y: 1 };
    let t = Instant::now();
    let tile = build_tile(prep, idx, 0, &space, all, &opts);
    println!("tile z2 (up to 1M candidates, budget 20k): {:.0} ms, kept {} of {}", ms(t), tile.stats.kept, tile.stats.candidates);
    let c0 = set.xy[0];
    let around = theta_geo::BBox { min: [c0[0] - 0.004, c0[1] - 0.004], max: [c0[0] + 0.004, c0[1] + 0.004] };
    let ids = space.tiles_for(8, &around);
    let t = Instant::now();
    let mut kept = 0;
    for id in ids.iter().take(8) {
        kept += build_tile(prep, idx, 0, &space, *id, &opts).stats.kept;
    }
    println!("{} tiles at z8 around the first point: {:.0} ms total, {} kept", ids.len().min(8), ms(t), kept);
}

fn polygons(n: usize, verts: usize) -> Vec<MapFeature> {
    let mut r = Rng(9);
    let side = (n as f64).sqrt().ceil() as usize;
    (0..n)
        .map(|j| {
            let (gx, gy) = ((j % side) as f64, (j / side) as f64);
            let (x, y) = (-170.0 + (gx + 0.5) * 340.0 / side as f64, -70.0 + (gy + 0.5) * 140.0 / side as f64);
            let rad = 340.0 / side as f64 * (0.3 + 0.2 * r.next());
            let ph = r.next() * 6.0;
            let mut ring: Vec<[f64; 2]> = (0..verts)
                .map(|k| {
                    let a = std::f64::consts::TAU * k as f64 / verts as f64;
                    let rr = rad * (1.0 + 0.2 * (a * 5.0 + ph).sin() + 0.04 * (a * 31.0).sin() + 0.008 * (a * 211.0).sin() + 0.002 * (a * 1013.0).sin());
                    [x + rr * a.cos(), y + rr * a.sin() * 0.8]
                })
                .collect();
            ring.push(ring[0]);
            MapFeature {
                feature: Feature::new(Geometry::Polygon(vec![ring]), Some(4326)),
                row: j,
                label: None,
                properties: vec![],
                values: vec![MapValue::Num(j as f64)],
                layer: 0,
            }
        })
        .collect()
}

#[test]
#[ignore]
fn perf_complex_polygons() {
    // 3,000 polygons x 2,000 vertices = 6,000,000 vertices.
    let feats = polygons(3000, 2000);
    let layers = vec![MapLayer::new("z", vec!["v".into()])];
    let t = Instant::now();
    let built = build(layers, Arc::new(feats));
    println!("build, 3,000 polygons / 6,000,000 vertices: {:.0} ms", ms(t));
    let space = TileSpace::geographic();
    for (name, o) in [
        ("simplification on (Douglas-Peucker 0.6 px)", TileOpts::from_style(&TilingOptions::default())),
        ("simplification on (Visvalingam 0.6 px)", TileOpts { simplify: crate::style::Simplify::Visvalingam, ..TileOpts::from_style(&TilingOptions::default()) }),
        ("simplification off", TileOpts { simplify: crate::style::Simplify::Off, ..TileOpts::from_style(&TilingOptions::default()) }),
    ] {
        for z in [2u8, 5] {
            let ids = space.tiles_for(z, &crate::style::tests_rect());
            let t = Instant::now();
            let (mut vin, mut vout) = (0, 0);
            for id in ids.iter().take(6) {
                let tile = build_tile(&built.prepared, &built.index, 0, &space, *id, &o);
                vin += tile.stats.vertices_in;
                vout += tile.stats.vertices_out;
            }
            println!("{name}, z{z}: 6 tiles in {:.0} ms, {vin} -> {vout} vertices", ms(t));
        }
    }
}
