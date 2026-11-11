//! Camera wrapper over `quarry_geo`'s `MapView` / `PlanarView`, plus the linear
//! world-to-pixel transform used for bulk vertex projection.

use crate::world::{Mode, lon_lat_to_world, world_bbox_to_lon_lat};
use quarry_geo::{BBox, Coord, MapView, PlanarView};

/// Pixels per world unit transform: `screen = half + (world - center) * scale`, with the
/// y scale negative in planar mode (world y is up there).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    /// Viewport centre in pixels.
    pub half: [f64; 2],
    /// World coordinate at the viewport centre.
    pub center: Coord,
    /// Pixels per world unit on x and y.
    pub scale: [f64; 2],
}

impl Transform {
    /// World to screen pixels.
    #[inline]
    pub fn apply(&self, w: Coord) -> [f64; 2] {
        [
            self.half[0] + (w[0] - self.center[0]) * self.scale[0],
            self.half[1] + (w[1] - self.center[1]) * self.scale[1],
        ]
    }

    /// Screen pixels to world.
    pub fn invert(&self, s: [f64; 2]) -> Coord {
        [
            self.center[0] + (s[0] - self.half[0]) / self.scale[0],
            self.center[1] + (s[1] - self.half[1]) / self.scale[1],
        ]
    }

    /// The world rectangle covered by a viewport of `size`, grown by `margin_px`.
    pub fn world_rect(&self, size: [f64; 2], margin_px: f64) -> BBox {
        let a = self.invert([-margin_px, -margin_px]);
        let b = self.invert([size[0] + margin_px, size[1] + margin_px]);
        BBox { min: [a[0].min(b[0]), a[1].min(b[1])], max: [a[0].max(b[0]), a[1].max(b[1])] }
    }

    /// Integer detail level for simplification caching: `floor(log2(pixels per unit))`.
    pub fn lod_level(&self) -> i32 {
        self.scale[0].abs().log2().floor() as i32
    }
}

/// The map camera in either mode.
#[derive(Clone, Copy, Debug)]
pub enum Camera {
    /// Slippy-map camera.
    Geo(MapView),
    /// Planar camera.
    Planar(PlanarView),
}

pub(crate) const FIT_PADDING: f64 = 40.0;

impl Camera {
    /// A camera for `mode` showing `bbox` (world coordinates).
    pub fn fit(mode: Mode, bbox: Option<BBox>, size: [f64; 2]) -> Camera {
        Camera::fit_padded(mode, bbox, size, FIT_PADDING)
    }

    /// Like [`Camera::fit`] with explicit padding.
    pub fn fit_padded(mode: Mode, bbox: Option<BBox>, size: [f64; 2], pad: f64) -> Camera {
        match (mode, bbox) {
            (Mode::Geographic, Some(b)) => Camera::Geo(MapView::fit(world_bbox_to_lon_lat(b), size, pad)),
            (Mode::Geographic, None) => Camera::Geo(MapView { center: [0.0, 20.0], zoom: 1.5, size }),
            (Mode::Planar, Some(b)) => Camera::Planar(PlanarView::fit(b, size, pad)),
            (Mode::Planar, None) => {
                Camera::Planar(PlanarView { center: [0.0, 0.0], units_per_px: 1.0, size })
            }
        }
    }

    /// A camera showing the union of `boxes` (world coordinates). Unlike fitting their union box,
    /// geographic data is framed along the shortest longitude arc, so features at lon 179 and
    /// -179 are centred on the antimeridian instead of spread over the whole world.
    pub fn fit_boxes(
        mode: Mode,
        boxes: impl Iterator<Item = BBox> + Clone,
        size: [f64; 2],
        pad: f64,
    ) -> Camera {
        let Some(union) = boxes.clone().reduce(BBox::union) else {
            return Camera::fit_padded(mode, None, size, pad);
        };
        if mode == Mode::Planar {
            return Camera::fit_padded(mode, Some(union), size, pad);
        }
        let mut ll = world_bbox_to_lon_lat(union);
        let arc =
            quarry_geo::min_lon_arc(boxes.map(|b| [b.min[0] * 360.0 - 180.0, b.max[0] * 360.0 - 180.0]));
        if let Some([west, east]) = arc {
            ll.min[0] = west;
            ll.max[0] = east;
        }
        Camera::Geo(MapView::fit(ll, size, pad))
    }

    /// Viewport size in pixels.
    pub fn size(&self) -> [f64; 2] {
        match self {
            Camera::Geo(v) => v.size,
            Camera::Planar(v) => v.size,
        }
    }

    /// Change the viewport size, keeping centre and zoom.
    pub fn set_size(&mut self, size: [f64; 2]) {
        match self {
            Camera::Geo(v) => {
                v.size = size;
                // A wider viewport raises the minimum zoom (one world copy must cover it).
                v.clamp_zoom();
            }
            Camera::Planar(v) => v.size = size,
        }
    }

    /// The bulk transform for the current state.
    pub fn transform(&self) -> Transform {
        match self {
            Camera::Geo(v) => {
                let s = 256.0 * v.zoom.exp2();
                Transform {
                    half: [v.size[0] / 2.0, v.size[1] / 2.0],
                    center: lon_lat_to_world(v.center),
                    scale: [s, s],
                }
            }
            Camera::Planar(v) => {
                let s = 1.0 / v.units_per_px;
                Transform { half: [v.size[0] / 2.0, v.size[1] / 2.0], center: v.center, scale: [s, -s] }
            }
        }
    }

    /// Zoom as a log2 level (geographic: the map zoom; planar: `log2(px per unit)`).
    pub fn level(&self) -> f64 {
        match self {
            Camera::Geo(v) => v.zoom,
            Camera::Planar(v) => (1.0 / v.units_per_px).log2(),
        }
    }

    /// Drag the content by a screen delta.
    pub fn pan_by(&mut self, dx: f64, dy: f64) {
        match self {
            Camera::Geo(v) => v.pan_by(dx, dy),
            Camera::Planar(v) => v.pan_by(dx, dy),
        }
    }

    /// Zoom by `delta` levels (positive zooms in) keeping the point under `anchor` fixed.
    pub fn zoom_around(&mut self, anchor: [f64; 2], delta: f64) {
        match self {
            Camera::Geo(v) => v.zoom_around(anchor, delta),
            Camera::Planar(v) => v.zoom_around(anchor, delta.exp2()),
        }
    }

    /// Allowed zoom range in levels; the lower end keeps the world from repeating.
    pub fn level_limits(&self) -> (f64, f64) {
        match self {
            Camera::Geo(v) => (v.min_zoom(), 20.0),
            Camera::Planar(_) => (-40.0, 40.0),
        }
    }

    /// Move the viewport centre to world coordinate `target` (instantly).
    pub fn center_on(&mut self, target: Coord) {
        let t = self.transform();
        let d = [(target[0] - t.center[0]) * t.scale[0], (target[1] - t.center[1]) * t.scale[1]];
        // Dragging the content by -d brings `target` to the middle.
        self.pan_by(-d[0], -d[1]);
    }

    /// Screen pixels to a coordinate in the data's own space: lon/lat for geographic
    /// cameras (wrapped), world units for planar.
    pub fn unproject(&self, s: [f64; 2]) -> Coord {
        match self {
            Camera::Geo(v) => v.unproject(s),
            Camera::Planar(v) => v.unproject(s),
        }
    }

    /// Screen position of a coordinate in the data's own space.
    pub fn project(&self, c: Coord) -> [f64; 2] {
        match self {
            Camera::Geo(v) => v.project(c),
            Camera::Planar(v) => v.project(c),
        }
    }

    /// Ground metres per pixel, for the scale bar (None in planar mode).
    pub fn meters_per_pixel(&self) -> Option<f64> {
        match self {
            Camera::Geo(v) => Some(v.meters_per_pixel()),
            Camera::Planar(_) => None,
        }
    }

    /// Mode of this camera.
    pub fn mode(&self) -> Mode {
        match self {
            Camera::Geo(_) => Mode::Geographic,
            Camera::Planar(_) => Mode::Planar,
        }
    }

    /// World coordinate at the centre.
    pub fn center_world(&self) -> Coord {
        self.transform().center
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_matches_map_view() {
        let cam = Camera::Geo(MapView { center: [16.37, 48.2], zoom: 9.3, size: [900.0, 500.0] });
        let t = cam.transform();
        for p in [[16.5, 48.3], [15.0, 47.0]] {
            let a = t.apply(lon_lat_to_world(p));
            let b = cam.project(p);
            assert!((a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5);
        }
    }

    #[test]
    fn transform_matches_planar_view() {
        let cam =
            Camera::Planar(PlanarView { center: [500.0, 200.0], units_per_px: 2.5, size: [800.0, 400.0] });
        let t = cam.transform();
        for p in [[100.0, 50.0], [900.0, 300.0]] {
            let a = t.apply(p);
            let b = cam.project(p);
            assert!((a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9);
        }
        let back = t.invert(t.apply([123.0, 456.0]));
        assert!((back[0] - 123.0).abs() < 1e-9 && (back[1] - 456.0).abs() < 1e-9);
    }

    #[test]
    fn geo_limits_follow_the_viewport_size() {
        // A short wide panel may zoom out until the world is 80% as tall as it: well below one world width.
        let mut cam = Camera::Geo(MapView { center: [0.0, 0.0], zoom: 0.0, size: [256.0, 100.0] });
        let low = cam.level_limits().0;
        assert!((256.0 * low.exp2() - 80.0).abs() < 1e-9, "{low}");
        cam.set_size([1024.0, 100.0]);
        assert_eq!(cam.level_limits().0, low, "the height decides, not the width");
        assert!(cam.level() >= low);
        cam.zoom_around([512.0, 50.0], -9.0);
        assert_eq!(cam.level(), low);
        // A taller panel raises the floor and set_size re-clamps the zoom to it.
        cam.set_size([1024.0, 400.0]);
        assert!(cam.level_limits().0 > low);
        assert!(cam.level() >= cam.level_limits().0);
    }

    #[test]
    fn fit_boxes_frames_the_short_arc() {
        let at = |lon: f64| {
            let w = lon_lat_to_world([lon, 11.0]);
            BBox { min: w, max: w }
        };
        let cam =
            Camera::fit_boxes(Mode::Geographic, [at(179.0), at(-179.0)].into_iter(), [900.0, 420.0], 40.0);
        let Camera::Geo(v) = cam else { panic!("geographic") };
        assert!((v.center[0].abs() - 180.0).abs() < 1e-9, "{:?}", v.center);
        let (a, b) = (cam.project([179.0, 11.0]), cam.project([-179.0, 11.0]));
        assert!(a[0] > 0.0 && b[0] < 900.0 && (b[0] - a[0] - 820.0).abs() < 1e-6, "{a:?} {b:?}");
        // The plain union would have been the whole world on the other side: shown whole, far
        // further out than the short-arc camera.
        let wide = Camera::fit(Mode::Geographic, Some(at(179.0).union(at(-179.0))), [900.0, 420.0]);
        assert!(wide.level() < cam.level() - 4.0, "{} vs {}", wide.level(), cam.level());
        assert!(256.0 * wide.level().exp2() <= 900.0, "the whole world fits the width");
    }

    #[test]
    fn center_on_moves_target_to_middle() {
        for mut cam in [
            Camera::Geo(MapView { center: [0.0, 0.0], zoom: 6.0, size: [800.0, 600.0] }),
            Camera::Planar(PlanarView { center: [0.0, 0.0], units_per_px: 2.0, size: [800.0, 600.0] }),
        ] {
            let target = match cam {
                Camera::Geo(_) => lon_lat_to_world([4.0, 3.0]),
                Camera::Planar(_) => [120.0, -80.0],
            };
            cam.center_on(target);
            let c = cam.center_world();
            assert!((c[0] - target[0]).abs() < 1e-7 * target[0].abs().max(1.0), "{c:?} vs {target:?}");
            assert!((c[1] - target[1]).abs() < 1e-7 * target[1].abs().max(1.0), "{c:?} vs {target:?}");
        }
    }

    #[test]
    fn world_rect_covers_viewport() {
        let cam = Camera::Planar(PlanarView { center: [0.0, 0.0], units_per_px: 1.0, size: [100.0, 50.0] });
        let r = cam.transform().world_rect([100.0, 50.0], 10.0);
        assert_eq!(r.min, [-60.0, -35.0]);
        assert_eq!(r.max, [60.0, 35.0]);
    }

    #[test]
    fn lod_level_is_floor_log2_scale() {
        let cam = Camera::Geo(MapView { center: [0.0, 0.0], zoom: 3.7, size: [10.0, 10.0] });
        assert_eq!(cam.transform().lod_level(), 8 + 3);
    }
}
