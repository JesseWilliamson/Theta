//! Slippy-map camera maths (Web-Mercator tiles) and a planar camera for data without a CRS.
//!
//! Conventions: tiles are 256 px; at zoom `z` the whole world is `256 * 2^z` px wide and
//! tall (fractional zoom allowed). Screen origin is the top-left, y grows downwards.
//! World pixel `(0,0)` is lon -180, lat +85.0511.

use crate::crs::MAX_MERCATOR_LAT;
use crate::geometry::{BBox, Coord};
use std::f64::consts::PI;

const TILE: f64 = 256.0;
/// Highest zoom a camera may have (continuous), and the highest tile level requested.
const MAX_ZOOM: f64 = 20.0;
const MAX_TILE_ZOOM: u8 = 19;
const MAX_TILES: usize = 64;
/// Metres per pixel at the equator at zoom 0: Earth's circumference over 256 px.
const EQUATOR_M_PER_PX: f64 = 156_543.033_928_041;

/// A 256-px slippy-map tile address (XYZ scheme, y grows southwards).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileId {
    /// Zoom level.
    pub z: u8,
    /// Column, `0..2^z`.
    pub x: u32,
    /// Row, `0..2^z`.
    pub y: u32,
}

impl TileId {
    /// Fill a URL template, replacing `{z}`, `{x}` and `{y}`.
    pub fn url(&self, template: &str) -> String {
        template
            .replace("{z}", &self.z.to_string())
            .replace("{x}", &self.x.to_string())
            .replace("{y}", &self.y.to_string())
    }

    /// The tile's extent as lon/lat: `min = [west, south]`, `max = [east, north]`.
    pub fn bounds_lon_lat(&self) -> BBox {
        let n = f64::from(1u32 << self.z.min(31));
        let lon = |x: f64| x / n * 360.0 - 180.0;
        let lat = |y: f64| lat_from_frac(y / n);
        BBox {
            min: [lon(f64::from(self.x)), lat(f64::from(self.y) + 1.0)],
            max: [lon(f64::from(self.x) + 1.0), lat(f64::from(self.y))],
        }
    }
}

/// Mercator y as a fraction of the world height: 0 at the north edge, 1 at the south.
/// `y = 1/2 - ln(tan(pi/4 + phi/2)) / (2 pi)`.
fn frac_from_lat(lat: f64) -> f64 {
    let phi = lat.clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT).to_radians();
    0.5 - (PI / 4.0 + phi / 2.0).tan().ln() / (2.0 * PI)
}

/// Inverse of [`frac_from_lat`]: `phi = atan(sinh(pi (1 - 2y)))`.
fn lat_from_frac(y: f64) -> f64 {
    (PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees()
}

/// Wrap a longitude into `[-180, 180)`.
pub fn wrap_lon(lon: f64) -> f64 {
    (lon + 180.0).rem_euclid(360.0) - 180.0
}

fn world_size(zoom: f64) -> f64 {
    TILE * zoom.exp2()
}

/// A map camera. `center` is lon/lat; `zoom` is fractional; `size` is the viewport in
/// logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapView {
    /// Lon/lat under the middle of the viewport.
    pub center: Coord,
    /// Continuous zoom level, 0..=20.
    pub zoom: f64,
    /// Viewport size in logical pixels.
    pub size: [f64; 2],
}

impl MapView {
    /// World pixel position of a lon/lat at this zoom (no longitude wrapping).
    fn world(&self, ll: Coord) -> [f64; 2] {
        let w = world_size(self.zoom);
        [(ll[0] + 180.0) / 360.0 * w, frac_from_lat(ll[1]) * w]
    }

    /// A camera showing `bbox_lon_lat` with `padding_px` on every side. The zoom is the
    /// largest that fits, capped at 18; a single point (or a degenerate box) gets zoom 14.
    pub fn fit(bbox_lon_lat: BBox, size: [f64; 2], padding_px: f64) -> MapView {
        let b = bbox_lon_lat;
        let usable = [(size[0] - 2.0 * padding_px).max(1.0), (size[1] - 2.0 * padding_px).max(1.0)];
        let (y_top, y_bot) = (frac_from_lat(b.max[1]), frac_from_lat(b.min[1]));
        let fx = b.width() / 360.0;
        let fy = (y_bot - y_top).abs();
        let center = [b.center()[0], lat_from_frac((y_top + y_bot) / 2.0)];
        // Zoom z shows `256 * 2^z * f` px of a feature spanning world fraction f.
        let zx = if fx > 1e-12 { (usable[0] / (TILE * fx)).log2() } else { f64::INFINITY };
        let zy = if fy > 1e-12 { (usable[1] / (TILE * fy)).log2() } else { f64::INFINITY };
        let zoom = zx.min(zy);
        let zoom = if zoom.is_finite() { zoom.clamp(0.0, 18.0) } else { 14.0 };
        MapView { center, zoom, size }
    }

    /// Lon/lat to screen pixels. Longitude differences are taken the short way round the
    /// globe, so features straddling the antimeridian stay together near the centre.
    pub fn project(&self, lon_lat: Coord) -> [f64; 2] {
        let (c, p) = (self.world(self.center), self.world(lon_lat));
        let w = world_size(self.zoom);
        let mut dx = p[0] - c[0];
        dx -= w * (dx / w).round();
        [self.size[0] / 2.0 + dx, self.size[1] / 2.0 + p[1] - c[1]]
    }

    fn unproject_raw(&self, screen: [f64; 2]) -> Coord {
        let (c, w) = (self.world(self.center), world_size(self.zoom));
        let x = c[0] + screen[0] - self.size[0] / 2.0;
        let y = c[1] + screen[1] - self.size[1] / 2.0;
        [x / w * 360.0 - 180.0, lat_from_frac(y / w)]
    }

    /// Screen pixels to lon/lat (longitude wrapped into `[-180, 180)`).
    pub fn unproject(&self, screen: [f64; 2]) -> Coord {
        let p = self.unproject_raw(screen);
        [wrap_lon(p[0]), p[1]]
    }

    fn set_center_world(&mut self, x: f64, y: f64) {
        let w = world_size(self.zoom);
        self.center = [wrap_lon(x / w * 360.0 - 180.0), lat_from_frac(y.clamp(0.0, w) / w)];
    }

    /// Drag the map by a screen-space delta (content follows the pointer).
    pub fn pan_by(&mut self, dx_px: f64, dy_px: f64) {
        let c = self.world(self.center);
        self.set_center_world(c[0] - dx_px, c[1] - dy_px);
    }

    /// Change zoom by `delta_zoom` while the lon/lat under `screen_anchor` stays put
    /// (zoom-to-cursor). The zoom is clamped to 0..=20.
    pub fn zoom_around(&mut self, screen_anchor: [f64; 2], delta_zoom: f64) {
        let pinned = self.unproject_raw(screen_anchor);
        self.zoom = (self.zoom + delta_zoom).clamp(0.0, MAX_ZOOM);
        let p = self.world(pinned);
        self.set_center_world(
            p[0] - (screen_anchor[0] - self.size[0] / 2.0),
            p[1] - (screen_anchor[1] - self.size[1] / 2.0),
        );
    }

    /// Tiles needed to cover the viewport, nearest the centre first, each with the screen
    /// position of its top-left corner and its drawn size in px.
    ///
    /// The tile level is `round(zoom)` (clamped to 0..=19), so tiles are drawn scaled by
    /// `2^(zoom - level)`, between about 0.7x and 1.4x. Columns wrap across the
    /// antimeridian; rows outside the world are dropped. If a huge viewport would need
    /// more than 64 tiles the level is lowered until it fits.
    pub fn visible_tiles(&self) -> Vec<(TileId, [f64; 2], f64)> {
        let mut level = (self.zoom.round().clamp(0.0, f64::from(MAX_TILE_ZOOM))) as u8;
        loop {
            let tiles = self.tiles_at(level);
            if tiles.len() <= MAX_TILES || level == 0 {
                let mut tiles = tiles;
                tiles.truncate(MAX_TILES);
                return tiles;
            }
            level -= 1;
        }
    }

    fn tiles_at(&self, level: u8) -> Vec<(TileId, [f64; 2], f64)> {
        let n = 1i64 << level;
        let drawn = TILE * (self.zoom - f64::from(level)).exp2();
        // Centre in tile units at this level.
        let cx = (self.center[0] + 180.0) / 360.0 * n as f64;
        let cy = frac_from_lat(self.center[1]) * n as f64;
        let (hx, hy) = (self.size[0] / 2.0 / drawn, self.size[1] / 2.0 / drawn);
        let (x0, x1) =
            ((cx - hx).floor() as i64, ((cx + hx).ceil() as i64 - 1).max((cx - hx).floor() as i64));
        let (y0, y1) = (((cy - hy).floor() as i64).max(0), ((cy + hy).ceil() as i64 - 1).min(n - 1));
        let mut out = Vec::new();
        for ty in y0..=y1 {
            for tx in x0..=x1 {
                let pos = [
                    self.size[0] / 2.0 + (tx as f64 - cx) * drawn,
                    self.size[1] / 2.0 + (ty as f64 - cy) * drawn,
                ];
                let id = TileId { z: level, x: tx.rem_euclid(n) as u32, y: ty as u32 };
                out.push((id, pos, drawn));
            }
        }
        let mid = [self.size[0] / 2.0, self.size[1] / 2.0];
        let dist = |t: &(TileId, [f64; 2], f64)| {
            (t.1[0] + t.2 / 2.0 - mid[0]).powi(2) + (t.1[1] + t.2 / 2.0 - mid[1]).powi(2)
        };
        out.sort_by(|a, b| dist(a).total_cmp(&dist(b)));
        out
    }

    /// Lon/lat extent of the viewport: `min = [west, south]`, `max = [east, north]`.
    /// Longitudes are not wrapped, so at the antimeridian `east` may exceed 180.
    pub fn bounds_lon_lat(&self) -> BBox {
        let tl = self.unproject_raw([0.0, 0.0]);
        let br = self.unproject_raw(self.size);
        BBox { min: [tl[0], br[1]], max: [br[0], tl[1]] }
    }

    /// Ground resolution at the centre latitude, in metres per screen pixel
    /// (`156543.03 * cos(lat) / 2^zoom`), for the scale bar.
    pub fn meters_per_pixel(&self) -> f64 {
        EQUATOR_M_PER_PX * self.center[1].to_radians().cos() / self.zoom.exp2()
    }
}

/// A camera for planar data (unknown CRS): world units map linearly to pixels, y up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanarView {
    /// World coordinate under the middle of the viewport.
    pub center: Coord,
    /// World units per screen pixel (smaller = more zoomed in).
    pub units_per_px: f64,
    /// Viewport size in logical pixels.
    pub size: [f64; 2],
}

impl PlanarView {
    /// A camera showing `bbox` with `padding_px` of margin. A degenerate box gets 1 unit/px.
    pub fn fit(bbox: BBox, size: [f64; 2], padding_px: f64) -> PlanarView {
        let usable = [(size[0] - 2.0 * padding_px).max(1.0), (size[1] - 2.0 * padding_px).max(1.0)];
        let upp = (bbox.width() / usable[0]).max(bbox.height() / usable[1]);
        let units_per_px = if upp.is_finite() && upp > 0.0 { upp } else { 1.0 };
        PlanarView { center: bbox.center(), units_per_px, size }
    }

    /// World to screen pixels (screen y grows downwards, world y upwards).
    pub fn project(&self, c: Coord) -> [f64; 2] {
        [
            self.size[0] / 2.0 + (c[0] - self.center[0]) / self.units_per_px,
            self.size[1] / 2.0 - (c[1] - self.center[1]) / self.units_per_px,
        ]
    }

    /// Screen pixels to world coordinates.
    pub fn unproject(&self, s: [f64; 2]) -> Coord {
        [
            self.center[0] + (s[0] - self.size[0] / 2.0) * self.units_per_px,
            self.center[1] - (s[1] - self.size[1] / 2.0) * self.units_per_px,
        ]
    }

    /// Drag the content by a screen-space delta.
    pub fn pan_by(&mut self, dx: f64, dy: f64) {
        self.center[0] -= dx * self.units_per_px;
        self.center[1] += dy * self.units_per_px;
    }

    /// Zoom by `factor` (>1 zooms in) keeping the world point under `anchor` fixed.
    /// Non-positive or non-finite factors are ignored.
    pub fn zoom_around(&mut self, anchor: [f64; 2], factor: f64) {
        if !(factor.is_finite() && factor > 0.0) {
            return;
        }
        let pinned = self.unproject(anchor);
        self.units_per_px /= factor;
        self.center = [
            pinned[0] - (anchor[0] - self.size[0] / 2.0) * self.units_per_px,
            pinned[1] + (anchor[1] - self.size[1] / 2.0) * self.units_per_px,
        ];
    }
}

/// Choose a scale-bar length: the largest "nice" distance (1, 2 or 5 times a power of
/// ten) whose bar fits in `max_px`. Returns `(bar length in px, label)`.
pub fn nice_scale_bar(meters_per_pixel: f64, max_px: f64) -> (f64, String) {
    let max_m = meters_per_pixel * max_px;
    if !(meters_per_pixel.is_finite() && meters_per_pixel > 0.0 && max_m.is_finite() && max_m > 0.0) {
        return (0.0, "0 m".to_string());
    }
    let pow = 10f64.powf(max_m.log10().floor());
    let value = [5.0, 2.0, 1.0].iter().map(|m| m * pow).find(|v| *v <= max_m * (1.0 + 1e-12)).unwrap_or(pow);
    let label = if value >= 1000.0 {
        format!("{} km", trim(value / 1000.0))
    } else if value >= 1.0 {
        format!("{} m", trim(value))
    } else {
        format!("{} cm", trim(value * 100.0))
    };
    (value / meters_per_pixel, label)
}

fn trim(v: f64) -> String {
    let s = format!("{v:.6}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> MapView {
        MapView { center: [151.2093, -33.8688], zoom: 11.3, size: [900.0, 600.0] }
    }

    fn close(a: [f64; 2], b: [f64; 2], eps: f64) -> bool {
        (a[0] - b[0]).abs() < eps && (a[1] - b[1]).abs() < eps
    }

    #[test]
    fn centre_projects_to_middle() {
        let v = view();
        assert!(close(v.project(v.center), [450.0, 300.0], 1e-9));
    }

    #[test]
    fn project_unproject_identity() {
        for zoom in [0.0, 2.5, 11.3, 18.0, 20.0] {
            for center in [[0.0, 0.0], [151.2, -33.9], [-122.4, 37.8], [179.0, 60.0], [-179.5, -70.0]] {
                let v = MapView { center, zoom, size: [800.0, 500.0] };
                for s in [[0.0, 0.0], [799.0, 499.0], [123.4, 321.0], [400.0, 250.0]] {
                    // Beyond the edge of the (small) world the mapping is many-to-one.
                    let p = v.unproject(s);
                    if p[1].abs() > 85.0 || (s[0] - 400.0).abs() >= 128.0 * zoom.exp2() {
                        continue;
                    }
                    let back = v.project(v.unproject(s));
                    assert!(close(back, s, 1e-6), "zoom {zoom} {center:?} {s:?} -> {back:?}");
                }
            }
        }
    }

    #[test]
    fn unproject_project_identity_for_lon_lat() {
        let v = view();
        for p in [[151.0, -33.5], [151.3, -34.0]] {
            let back = v.unproject(v.project(p));
            assert!(close(back, p, 1e-9), "{back:?}");
        }
    }

    #[test]
    fn world_size_scaling() {
        let a = MapView { center: [0.0, 0.0], zoom: 3.0, size: [100.0, 100.0] };
        let p = a.project([90.0, 0.0]);
        assert!((p[0] - 50.0 - 256.0 * 8.0 / 4.0).abs() < 1e-9);
    }

    #[test]
    fn pan_moves_content_with_pointer() {
        let mut v = view();
        let p = [151.2, -33.87];
        let before = v.project(p);
        v.pan_by(30.0, -20.0);
        let after = v.project(p);
        assert!(close(after, [before[0] + 30.0, before[1] - 20.0], 1e-6));
    }

    #[test]
    fn pan_wraps_and_clamps() {
        let mut v = MapView { center: [179.9, 0.0], zoom: 4.0, size: [400.0, 400.0] };
        v.pan_by(-500.0, 0.0);
        assert!(v.center[0] >= -180.0 && v.center[0] < 180.0 && v.center[0] < 0.0, "{:?}", v.center);
        let mut v = MapView { center: [0.0, 80.0], zoom: 2.0, size: [400.0, 400.0] };
        v.pan_by(0.0, 100_000.0);
        assert!(v.center[1] <= MAX_MERCATOR_LAT + 1e-6 && v.center[1] > 84.0);
        v.pan_by(0.0, -1_000_000.0);
        assert!(v.center[1] >= -MAX_MERCATOR_LAT - 1e-6);
    }

    #[test]
    fn zoom_around_keeps_anchor_fixed() {
        for anchor in [[0.0, 0.0], [450.0, 300.0], [800.0, 100.0], [10.0, 590.0]] {
            for dz in [1.0, -1.0, 0.37, 2.5, -3.2] {
                let mut v = view();
                let before = v.unproject(anchor);
                v.zoom_around(anchor, dz);
                let after = v.unproject(anchor);
                assert!(close(before, after, 1e-7), "{anchor:?} {dz}: {before:?} vs {after:?}");
                assert!(close(v.project(before), anchor, 1e-5));
            }
        }
    }

    #[test]
    fn zoom_clamps() {
        let mut v = view();
        v.zoom_around([0.0, 0.0], 100.0);
        assert_eq!(v.zoom, 20.0);
        v.zoom_around([0.0, 0.0], -100.0);
        assert_eq!(v.zoom, 0.0);
    }

    #[test]
    fn zoom_across_antimeridian_keeps_anchor() {
        let mut v = MapView { center: [179.5, 0.0], zoom: 5.0, size: [800.0, 600.0] };
        let anchor = [700.0, 100.0];
        let before = v.unproject(anchor);
        v.zoom_around(anchor, 1.5);
        assert!(close(v.project(before), anchor, 1e-5));
    }

    #[test]
    fn fit_contains_bbox_with_padding() {
        let size = [1000.0, 600.0];
        for bbox in [
            BBox { min: [150.0, -35.0], max: [152.0, -33.0] },
            BBox { min: [-10.0, 40.0], max: [30.0, 60.0] },
            BBox { min: [0.0, 0.0], max: [0.001, 0.5] },
            BBox { min: [-122.5, 37.7], max: [-122.3, 37.8] },
            BBox { min: [-179.0, -80.0], max: [179.0, 80.0] },
        ] {
            let v = MapView::fit(bbox, size, 40.0);
            for c in [bbox.min, bbox.max, [bbox.min[0], bbox.max[1]], [bbox.max[0], bbox.min[1]]] {
                let p = v.project(c);
                assert!(p[0] >= 40.0 - 1e-6 && p[0] <= 960.0 + 1e-6, "{bbox:?} x {p:?}");
                assert!(p[1] >= 40.0 - 1e-6 && p[1] <= 560.0 + 1e-6, "{bbox:?} y {p:?}");
            }
            let mid = v.project(bbox.center());
            assert!((mid[0] - 500.0).abs() < 1.0, "centred x {mid:?}");
        }
    }

    #[test]
    fn fit_is_tight() {
        // The limiting axis should touch the padding.
        let bbox = BBox { min: [150.0, -35.0], max: [152.0, -33.0] };
        let v = MapView::fit(bbox, [1000.0, 600.0], 20.0);
        let a = v.project(bbox.min);
        let b = v.project(bbox.max);
        let w = (b[0] - a[0]).abs();
        let h = (b[1] - a[1]).abs();
        assert!((w - 960.0).abs() < 1.0 || (h - 560.0).abs() < 1.0, "{w} {h}");
    }

    #[test]
    fn fit_single_point_and_caps() {
        let v = MapView::fit(BBox { min: [10.0, 20.0], max: [10.0, 20.0] }, [800.0, 600.0], 20.0);
        assert_eq!(v.zoom, 14.0);
        assert_eq!(v.center, [10.0, 20.0]);
        let v = MapView::fit(BBox { min: [10.0, 20.0], max: [10.000_001, 20.000_001] }, [800.0, 600.0], 20.0);
        assert_eq!(v.zoom, 18.0);
        let v = MapView::fit(BBox { min: [-180.0, -85.0], max: [180.0, 85.0] }, [100.0, 100.0], 0.0);
        assert!(v.zoom >= 0.0 && v.zoom < 0.5);
    }

    #[test]
    fn fit_degenerate_viewport_is_finite() {
        let v = MapView::fit(BBox { min: [0.0, 0.0], max: [1.0, 1.0] }, [10.0, 10.0], 100.0);
        assert!(v.zoom.is_finite() && v.center[0].is_finite());
    }

    #[test]
    fn fit_horizontal_line() {
        let v = MapView::fit(BBox { min: [0.0, 10.0], max: [1.0, 10.0] }, [800.0, 600.0], 0.0);
        assert!(v.zoom > 5.0 && v.zoom <= 18.0);
    }

    fn assert_covers(v: &MapView) {
        let tiles = v.visible_tiles();
        assert!(!tiles.is_empty() && tiles.len() <= 64, "{} tiles", tiles.len());
        // Every sampled screen point is inside some tile (rows beyond the poles excepted).
        let w = v.size[0];
        let h = v.size[1];
        for i in 0..=20 {
            for j in 0..=20 {
                let p = [w * f64::from(i) / 20.0, h * f64::from(j) / 20.0];
                let lat_ok = v.unproject_raw(p)[1].abs() < MAX_MERCATOR_LAT - 1e-6;
                if !lat_ok {
                    continue;
                }
                let hit = tiles.iter().any(|(_, tl, s)| {
                    p[0] >= tl[0] - 1e-6
                        && p[0] <= tl[0] + s + 1e-6
                        && p[1] >= tl[1] - 1e-6
                        && p[1] <= tl[1] + s + 1e-6
                });
                assert!(hit, "uncovered {p:?} in {v:?}");
            }
        }
    }

    #[test]
    fn tiles_cover_viewport() {
        for zoom in [0.0, 1.4, 3.5, 8.49, 8.5, 11.3, 15.9, 19.5, 20.0] {
            for center in
                [[0.0, 0.0], [151.2, -33.9], [-122.4, 37.8], [179.9, 10.0], [-180.0, 0.0], [0.0, 84.0]]
            {
                assert_covers(&MapView { center, zoom, size: [1024.0, 700.0] });
            }
        }
    }

    #[test]
    fn tile_positions_are_contiguous_grid() {
        let v = view();
        let tiles = v.visible_tiles();
        let s = tiles[0].2;
        for (_, p, size) in &tiles {
            assert!((size - s).abs() < 1e-9);
            let off = ((p[0] - tiles[0].1[0]) / s, (p[1] - tiles[0].1[1]) / s);
            assert!((off.0 - off.0.round()).abs() < 1e-6 && (off.1 - off.1.round()).abs() < 1e-6);
        }
        let uniq: std::collections::HashSet<_> = tiles.iter().map(|t| t.0).collect();
        assert_eq!(uniq.len(), tiles.len());
    }

    #[test]
    fn tile_level_and_scale() {
        let v = MapView { center: [0.0, 0.0], zoom: 11.3, size: [800.0, 600.0] };
        let t = v.visible_tiles();
        assert!(t.iter().all(|(id, _, _)| id.z == 11));
        assert!((t[0].2 - 256.0 * 0.3f64.exp2()).abs() < 1e-9);
        let v = MapView { zoom: 11.6, ..v };
        assert!(v.visible_tiles().iter().all(|(id, _, _)| id.z == 12));
        let v = MapView { zoom: 20.0, ..v };
        assert!(v.visible_tiles().iter().all(|(id, _, s)| id.z == 19 && (*s - 512.0).abs() < 1e-9));
    }

    #[test]
    fn tile_ids_known_values() {
        // Cross-checked against the standard OSM slippy-map formula (lon/lat -> tile).
        let v = MapView { center: [151.2153, -33.8568], zoom: 15.0, size: [10.0, 10.0] };
        let t = v.visible_tiles();
        assert!(t.iter().any(|(id, _, _)| *id == TileId { z: 15, x: 30147, y: 19662 }));
    }

    #[test]
    fn antimeridian_wraps_columns() {
        let v = MapView { center: [179.99, 0.0], zoom: 3.0, size: [800.0, 400.0] };
        let tiles = v.visible_tiles();
        let xs: Vec<u32> = tiles.iter().map(|t| t.0.x).collect();
        assert!(xs.contains(&7) && xs.contains(&0), "{xs:?}");
        assert!(xs.iter().all(|x| *x < 8));
        // The wrapped tile must be drawn to the right of the seam.
        let t0 = tiles.iter().find(|t| t.0.x == 0).unwrap();
        let t7 = tiles.iter().find(|t| t.0.x == 7).unwrap();
        assert!(t0.1[0] > t7.1[0]);
        assert_covers(&v);
        let v = MapView { center: [-179.99, 0.0], ..v };
        let xs: Vec<u32> = v.visible_tiles().iter().map(|t| t.0.x).collect();
        assert!(xs.contains(&7) && xs.contains(&0));
    }

    #[test]
    fn rows_do_not_leave_the_world() {
        let v = MapView { center: [0.0, 84.9], zoom: 3.0, size: [800.0, 800.0] };
        let n = 8;
        assert!(v.visible_tiles().iter().all(|t| t.0.y < n));
    }

    #[test]
    fn tile_count_is_capped() {
        let v = MapView { center: [0.0, 0.0], zoom: 8.0, size: [8000.0, 6000.0] };
        let t = v.visible_tiles();
        assert!(t.len() <= 64 && !t.is_empty());
        let v = MapView { center: [0.0, 0.0], zoom: 0.0, size: [100_000.0, 100_000.0] };
        assert!(v.visible_tiles().len() <= 64);
    }

    #[test]
    fn tiles_sorted_centre_first() {
        let v = view();
        let t = v.visible_tiles();
        let mid = [450.0, 300.0];
        let d =
            |t: &(TileId, [f64; 2], f64)| (t.1[0] + t.2 / 2.0 - mid[0]).hypot(t.1[1] + t.2 / 2.0 - mid[1]);
        assert!(t.windows(2).all(|w| d(&w[0]) <= d(&w[1]) + 1e-9));
    }

    #[test]
    fn tile_url_and_bounds() {
        let t = TileId { z: 3, x: 5, y: 2 };
        assert_eq!(t.url("https://t/{z}/{x}/{y}.png"), "https://t/3/5/2.png");
        assert_eq!(t.url("{x}-{x}"), "5-5");
        let b = TileId { z: 0, x: 0, y: 0 }.bounds_lon_lat();
        assert!((b.min[0] + 180.0).abs() < 1e-9 && (b.max[0] - 180.0).abs() < 1e-9);
        assert!((b.max[1] - MAX_MERCATOR_LAT).abs() < 1e-6 && (b.min[1] + MAX_MERCATOR_LAT).abs() < 1e-6);
        let b = TileId { z: 1, x: 1, y: 0 }.bounds_lon_lat();
        assert!((b.min[0]).abs() < 1e-9 && b.min[1].abs() < 1e-9);
    }

    #[test]
    fn tile_bounds_contain_their_points() {
        let v = MapView { center: [151.2, -33.9], zoom: 12.0, size: [300.0, 300.0] };
        let (id, _, _) = v.visible_tiles()[0];
        assert!(
            id.bounds_lon_lat().expand(1e-9).contains([151.2, -33.9]) || id.bounds_lon_lat().width() > 0.0
        );
    }

    #[test]
    fn bounds_match_corners() {
        let v = view();
        let b = v.bounds_lon_lat();
        assert!(b.contains(v.center));
        let tl = v.unproject([0.0, 0.0]);
        assert!((b.min[0] - tl[0]).abs() < 1e-9 && (b.max[1] - tl[1]).abs() < 1e-9);
        assert!(b.width() > 0.0 && b.height() > 0.0);
    }

    #[test]
    fn meters_per_pixel_values() {
        let v = MapView { center: [0.0, 0.0], zoom: 0.0, size: [1.0, 1.0] };
        assert!((v.meters_per_pixel() - 156_543.03).abs() < 0.1);
        let v = MapView { zoom: 1.0, ..v };
        assert!((v.meters_per_pixel() - 78_271.52).abs() < 0.1);
        let v = MapView { center: [0.0, 60.0], zoom: 0.0, ..v };
        assert!((v.meters_per_pixel() - 78_271.5).abs() < 1.0);
    }

    #[test]
    fn meters_per_pixel_matches_projection() {
        let v = view();
        let a = v.unproject([400.0, 300.0]);
        let b = v.unproject([500.0, 300.0]);
        let d = crate::measure::haversine_m(a, b) / 100.0;
        assert!((d / v.meters_per_pixel() - 1.0).abs() < 0.01, "{d} vs {}", v.meters_per_pixel());
    }

    #[test]
    fn wrap_lon_cases() {
        assert_eq!(wrap_lon(190.0), -170.0);
        assert_eq!(wrap_lon(-190.0), 170.0);
        assert_eq!(wrap_lon(180.0), -180.0);
        assert_eq!(wrap_lon(0.0), 0.0);
        assert_eq!(wrap_lon(720.0), 0.0);
    }

    // --- planar ---

    fn pv() -> PlanarView {
        PlanarView { center: [1000.0, -500.0], units_per_px: 2.5, size: [800.0, 600.0] }
    }

    #[test]
    fn planar_identity_and_orientation() {
        let v = pv();
        assert_eq!(v.project(v.center), [400.0, 300.0]);
        for s in [[0.0, 0.0], [123.0, 456.0], [800.0, 600.0]] {
            assert!(close(v.project(v.unproject(s)), s, 1e-9));
        }
        // y is up in the world, down on screen.
        assert!(v.project([1000.0, -400.0])[1] < v.project([1000.0, -500.0])[1]);
        assert!(v.project([1100.0, -500.0])[0] > 400.0);
    }

    #[test]
    fn planar_fit() {
        let bbox = BBox { min: [0.0, 0.0], max: [1000.0, 200.0] };
        let v = PlanarView::fit(bbox, [800.0, 600.0], 50.0);
        for c in [bbox.min, bbox.max] {
            let p = v.project(c);
            assert!(
                p[0] >= 50.0 - 1e-9 && p[0] <= 750.0 + 1e-9 && p[1] >= 50.0 - 1e-9 && p[1] <= 550.0 + 1e-9
            );
        }
        assert!(close(v.project(bbox.center()), [400.0, 300.0], 1e-9));
        assert!((v.units_per_px - 1000.0 / 700.0).abs() < 1e-12);
        let p = PlanarView::fit(BBox { min: [5.0, 5.0], max: [5.0, 5.0] }, [800.0, 600.0], 10.0);
        assert_eq!(p.units_per_px, 1.0);
        assert_eq!(p.center, [5.0, 5.0]);
    }

    #[test]
    fn planar_pan_and_zoom() {
        let mut v = pv();
        let p = [1100.0, -450.0];
        let before = v.project(p);
        v.pan_by(10.0, 20.0);
        assert!(close(v.project(p), [before[0] + 10.0, before[1] + 20.0], 1e-9));
        let anchor = [100.0, 500.0];
        let w = v.unproject(anchor);
        v.zoom_around(anchor, 2.0);
        assert!(close(v.unproject(anchor), w, 1e-9));
        assert!((v.units_per_px - 1.25).abs() < 1e-12);
        let before = v;
        v.zoom_around(anchor, 0.0);
        v.zoom_around(anchor, f64::NAN);
        v.zoom_around(anchor, -2.0);
        assert_eq!(v, before);
    }

    // --- scale bar ---

    #[test]
    fn scale_bar_values() {
        let (px, label) = nice_scale_bar(2.0, 100.0);
        assert_eq!(label, "200 m");
        assert!((px - 100.0).abs() < 1e-9);
        let (px, label) = nice_scale_bar(50.0, 120.0);
        assert_eq!(label, "5 km");
        assert!((px - 100.0).abs() < 1e-9);
        assert_eq!(nice_scale_bar(1.0, 130.0).1, "100 m");
        assert_eq!(nice_scale_bar(1.0, 49.0).1, "20 m");
        assert_eq!(nice_scale_bar(1.0, 9.0).1, "5 m");
        assert_eq!(nice_scale_bar(0.01, 100.0).1, "1 m");
        assert_eq!(nice_scale_bar(0.001, 100.0).1, "10 cm");
        assert_eq!(nice_scale_bar(1000.0, 150.0).1, "100 km");
        assert_eq!(nice_scale_bar(15_000.0, 200.0).1, "2,000 km".replace(',', ""));
    }

    #[test]
    fn scale_bar_never_exceeds_max_and_is_nice() {
        let mut mpp = 0.003;
        while mpp < 1e6 {
            let (px, label) = nice_scale_bar(mpp, 140.0);
            assert!(px <= 140.0 + 1e-6 && px > 140.0 / 5.0 - 1e-6, "{mpp}: {px} {label}");
            mpp *= 1.37;
        }
    }

    #[test]
    fn scale_bar_degenerate() {
        assert_eq!(nice_scale_bar(0.0, 100.0), (0.0, "0 m".to_string()));
        assert_eq!(nice_scale_bar(f64::NAN, 100.0).0, 0.0);
        assert_eq!(nice_scale_bar(1.0, -5.0).0, 0.0);
    }
}
