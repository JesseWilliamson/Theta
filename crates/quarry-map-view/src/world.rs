//! Projected "world" coordinates, computed once per `set_features`.
//!
//! Geographic data is stored as Web-Mercator *fractions* (`x`, `y` in `0..1`, y growing
//! southwards, the same space the slippy-map tile grid lives in), so the per-frame
//! transform to pixels is one multiply-add per vertex with no trigonometry. Planar data
//! is stored as-is. Everything downstream (culling, simplification, hit testing) works
//! in this space.

use crate::model::MapFeature;
use quarry_geo::crs::MAX_MERCATOR_LAT;
use quarry_geo::{BBox, Coord, Crs, Geometry, guess_crs, to_lon_lat};

/// How the data is placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Web-Mercator world fractions with a slippy-map basemap.
    Geographic,
    /// Unknown CRS: raw coordinates, y up, no basemap.
    Planar,
}

/// Mercator y fraction (0 at the north edge, 1 at the south) for a latitude.
pub fn lat_to_frac(lat: f64) -> f64 {
    let lat = lat.clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT).to_radians();
    0.5 - (std::f64::consts::FRAC_PI_4 + lat / 2.0).tan().ln() / (2.0 * std::f64::consts::PI)
}

/// Inverse of [`lat_to_frac`].
pub fn frac_to_lat(y: f64) -> f64 {
    (std::f64::consts::PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees()
}

/// lon/lat degrees to world fractions.
pub fn lon_lat_to_world(c: Coord) -> Coord {
    [(c[0] + 180.0) / 360.0, lat_to_frac(c[1])]
}

/// World fractions to lon/lat degrees (longitude not wrapped).
pub fn world_to_lon_lat(w: Coord) -> Coord {
    [w[0] * 360.0 - 180.0, frac_to_lat(w[1])]
}

/// A feature in world space.
#[derive(Clone, Debug)]
pub struct WorldFeature {
    /// Index into the source feature list.
    pub src: usize,
    /// Result row.
    pub row: usize,
    /// Layer index.
    pub layer: usize,
    /// Geometry in world coordinates.
    pub geom: Geometry,
    /// World bounding box.
    pub bbox: BBox,
    /// Vertex count, cached because `coord_count` walks the geometry.
    pub vertices: usize,
    /// Which primitive kinds the geometry contains.
    pub kinds: Kinds,
}

/// Which primitive kinds a (possibly nested) geometry contains.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Kinds {
    /// Has points.
    pub points: bool,
    /// Has line strings.
    pub lines: bool,
    /// Has polygons.
    pub polygons: bool,
}

impl Kinds {
    /// Scan a geometry.
    pub fn of(g: &Geometry) -> Kinds {
        let mut k = Kinds::default();
        k.scan(g);
        k
    }

    fn scan(&mut self, g: &Geometry) {
        match g {
            Geometry::Point(_) | Geometry::MultiPoint(_) => self.points = true,
            Geometry::LineString(_) | Geometry::MultiLineString(_) => self.lines = true,
            Geometry::Polygon(_) | Geometry::MultiPolygon(_) => self.polygons = true,
            Geometry::GeometryCollection(gs) => gs.iter().for_each(|g| self.scan(g)),
        }
    }
}

/// A drawable piece of a geometry.
#[derive(Clone, Copy, Debug)]
pub enum Part<'a> {
    /// A point.
    Point(Coord),
    /// An open line.
    Line(&'a [Coord]),
    /// A polygon: exterior first, then holes.
    Polygon(&'a [Vec<Coord>]),
}

/// Visit every primitive of `g`, flattening multi-geometries and collections.
pub fn for_each_part<'a>(g: &'a Geometry, f: &mut dyn FnMut(Part<'a>)) {
    match g {
        Geometry::Point(p) => p.iter().for_each(|c| f(Part::Point(*c))),
        Geometry::MultiPoint(ps) => ps.iter().for_each(|c| f(Part::Point(*c))),
        Geometry::LineString(l) => f(Part::Line(l)),
        Geometry::MultiLineString(ls) => ls.iter().for_each(|l| f(Part::Line(l))),
        Geometry::Polygon(r) => f(Part::Polygon(r)),
        Geometry::MultiPolygon(ps) => ps.iter().for_each(|r| f(Part::Polygon(r))),
        Geometry::GeometryCollection(gs) => gs.iter().for_each(|g| for_each_part(g, f)),
    }
}

/// The result of preparing a feature list.
#[derive(Clone, Debug)]
pub struct Prepared {
    /// Geographic or planar.
    pub mode: Mode,
    /// Drawable features (empty geometries and skipped ones are not here).
    pub features: Vec<WorldFeature>,
    /// Union of all feature boxes in world space.
    #[cfg_attr(not(test), allow(dead_code))]
    pub bbox: Option<BBox>,
    /// Features dropped because their CRS is unknown while others are geographic.
    pub skipped: usize,
    /// A short note on where the coordinates came from, for the status line.
    pub crs_note: String,
}

fn crs_label(srid: Option<i32>, crs: Crs) -> String {
    match crs {
        Crs::Geographic => "EPSG:4326".into(),
        Crs::WebMercator => "reprojected from EPSG:3857".into(),
        Crs::Utm { .. } => format!("reprojected from EPSG:{}", srid.unwrap_or(0)),
        Crs::Planar => "planar \u{2014} unknown SRID".into(),
    }
}

/// Reproject every feature to world coordinates.
///
/// If every feature has an unknown CRS the result is planar. Otherwise unknown ones are
/// skipped (counted in `skipped`), because their coordinates cannot be placed on a globe.
pub fn prepare(input: &[MapFeature]) -> Prepared {
    let classified: Vec<(usize, Crs)> = input
        .iter()
        .enumerate()
        .filter(|(_, f)| !f.feature.geometry.is_empty())
        .map(|(i, f)| (i, guess_crs(f.feature.srid, f.feature.geometry.bbox())))
        .collect();
    let geographic = classified.iter().filter(|(_, c)| *c != Crs::Planar).count();
    let mode = if geographic == 0 && !classified.is_empty() { Mode::Planar } else { Mode::Geographic };

    let mut features = Vec::with_capacity(classified.len());
    let mut skipped = 0;
    let mut notes: Vec<String> = Vec::new();
    for (i, crs) in classified {
        let f = &input[i];
        let geom = match (mode, crs) {
            (Mode::Planar, _) => f.feature.geometry.clone(),
            (Mode::Geographic, Crs::Planar) => {
                skipped += 1;
                continue;
            }
            (Mode::Geographic, crs) => f.feature.geometry.map_coords(&|c| {
                // `to_lon_lat` is `Some` for every non-planar CRS.
                lon_lat_to_world(to_lon_lat(c, crs).unwrap_or(c))
            }),
        };
        let Some(bbox) = geom.bbox() else { continue };
        if !(bbox.min[0].is_finite()
            && bbox.min[1].is_finite()
            && bbox.max[0].is_finite()
            && bbox.max[1].is_finite())
        {
            continue;
        }
        let note = crs_label(f.feature.srid, crs);
        if !notes.contains(&note) {
            notes.push(note);
        }
        let vertices = geom.coord_count();
        let kinds = Kinds::of(&geom);
        features.push(WorldFeature { src: i, row: f.row, layer: f.layer, geom, bbox, vertices, kinds });
    }
    let bbox = features.iter().map(|f| f.bbox).reduce(BBox::union);
    let crs_note = if notes.len() > 3 { format!("{} CRS mixed", notes.len()) } else { notes.join(", ") };
    Prepared { mode, features, bbox, skipped, crs_note }
}

/// Convert a world-space box to the lon/lat box `MapView::fit` expects.
pub fn world_bbox_to_lon_lat(b: BBox) -> BBox {
    let (a, c) = (world_to_lon_lat(b.min), world_to_lon_lat(b.max));
    BBox { min: [a[0], c[1].min(a[1])], max: [c[0], c[1].max(a[1])] }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quarry_geo::{Feature, MapView};

    fn mf(g: Geometry, srid: Option<i32>) -> MapFeature {
        MapFeature { feature: Feature::new(g, srid), row: 0, label: None, properties: vec![], layer: 0 }
    }

    #[test]
    fn world_matches_map_view_projection() {
        let view = MapView { center: [10.0, 50.0], zoom: 5.0, size: [800.0, 600.0] };
        let scale = 256.0 * 2f64.powf(view.zoom);
        let c = lon_lat_to_world(view.center);
        for p in [[12.5, 48.2], [-3.0, 55.0], [10.0, 50.0]] {
            let w = lon_lat_to_world(p);
            let ours = [400.0 + (w[0] - c[0]) * scale, 300.0 + (w[1] - c[1]) * scale];
            let theirs = view.project(p);
            assert!((ours[0] - theirs[0]).abs() < 1e-6 && (ours[1] - theirs[1]).abs() < 1e-6, "{p:?}");
        }
    }

    #[test]
    fn world_round_trip() {
        for p in [[151.2093, -33.8688], [0.0, 0.0], [-122.4, 37.8]] {
            let back = world_to_lon_lat(lon_lat_to_world(p));
            assert!((back[0] - p[0]).abs() < 1e-9 && (back[1] - p[1]).abs() < 1e-9);
        }
    }

    #[test]
    fn utm_is_reprojected_and_notes_the_source() {
        // Vienna, Stephansdom, in EPSG:32633.
        let p = prepare(&[mf(Geometry::Point(Some([601_800.0, 5_340_000.0])), Some(32633))]);
        assert_eq!(p.mode, Mode::Geographic);
        let ll = world_to_lon_lat(p.features[0].geom.bbox().unwrap().min);
        assert!((ll[0] - 16.37).abs() < 0.05 && (ll[1] - 48.2).abs() < 0.05, "{ll:?}");
        assert_eq!(p.crs_note, "reprojected from EPSG:32633");
    }

    #[test]
    fn all_planar_switches_mode() {
        let p = prepare(&[mf(Geometry::Point(Some([5000.0, 9000.0])), None)]);
        assert_eq!(p.mode, Mode::Planar);
        assert_eq!(p.features.len(), 1);
        assert_eq!(p.skipped, 0);
        assert_eq!(p.features[0].geom, Geometry::Point(Some([5000.0, 9000.0])));
    }

    #[test]
    fn mixed_skips_planar_and_counts_them() {
        let p = prepare(&[
            mf(Geometry::Point(Some([2.0, 48.0])), Some(4326)),
            mf(Geometry::Point(Some([5000.0, 9000.0])), None),
            mf(Geometry::Point(Some([6000.0, 9000.0])), Some(999_999)),
        ]);
        assert_eq!(p.mode, Mode::Geographic);
        assert_eq!(p.features.len(), 1);
        assert_eq!(p.skipped, 2);
    }

    #[test]
    fn empty_geometries_are_dropped_silently() {
        let p = prepare(&[mf(Geometry::Point(None), Some(4326)), mf(Geometry::LineString(vec![]), None)]);
        assert!(p.features.is_empty());
        assert_eq!(p.skipped, 0);
        assert!(p.bbox.is_none());
    }

    #[test]
    fn kinds_and_parts_flatten_collections() {
        let g = Geometry::GeometryCollection(vec![
            Geometry::MultiPoint(vec![[0.0, 0.0], [1.0, 1.0]]),
            Geometry::MultiLineString(vec![vec![[0.0, 0.0], [1.0, 0.0]]]),
        ]);
        assert_eq!(Kinds::of(&g), Kinds { points: true, lines: true, polygons: false });
        let mut n = (0, 0, 0);
        for_each_part(&g, &mut |p| match p {
            Part::Point(_) => n.0 += 1,
            Part::Line(_) => n.1 += 1,
            Part::Polygon(_) => n.2 += 1,
        });
        assert_eq!(n, (2, 1, 0));
    }

    #[test]
    fn unlabelled_lon_lat_is_rescued() {
        let p = prepare(&[mf(Geometry::Point(Some([2.35, 48.85])), None)]);
        assert_eq!(p.mode, Mode::Geographic);
        assert_eq!(p.features.len(), 1);
    }
}
