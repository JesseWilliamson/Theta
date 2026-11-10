//! Geodesic-ish measurement on lon/lat input, display formatting and cell summaries.
//!
//! Everything here treats the Earth as a sphere. That is within ~0.5% for distances and
//! ~1% for areas of the true ellipsoidal answer, which is plenty for an IDE read-out.

use crate::crs::{Crs, guess_crs, to_lon_lat};
use crate::geometry::{Coord, Feature, Geometry};

/// Mean Earth radius (IUGG) in metres, used for distances.
const EARTH_R: f64 = 6_371_008.8;
/// Authalic radius: the sphere with the same surface area as WGS 84. Using it for
/// areas removes the systematic bias a mean radius would introduce.
const AUTHALIC_R: f64 = 6_371_007.180_9;

/// Great-circle distance in metres between two lon/lat points (haversine formula:
/// `a = sin^2(dphi/2) + cos phi1 cos phi2 sin^2(dlambda/2)`, `d = 2 R asin(sqrt a)`).
/// Numerically stable for very small distances, unlike the spherical law of cosines.
pub fn haversine_m(a: Coord, b: Coord) -> f64 {
    let (p1, p2) = (a[1].to_radians(), b[1].to_radians());
    let dp = p2 - p1;
    let dl = (b[0] - a[0]).to_radians();
    let h = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH_R * h.sqrt().min(1.0).asin()
}

fn path_len(l: &[Coord]) -> f64 {
    l.windows(2).map(|w| haversine_m(w[0], w[1])).sum()
}

/// Total length in metres: line strings, plus the perimeter of every polygon ring.
/// Points contribute 0. Input must be lon/lat degrees.
pub fn length_m(geometry: &Geometry) -> f64 {
    match geometry {
        Geometry::Point(_) | Geometry::MultiPoint(_) => 0.0,
        Geometry::LineString(l) => path_len(l),
        Geometry::Polygon(r) | Geometry::MultiLineString(r) => r.iter().map(|l| path_len(l)).sum(),
        Geometry::MultiPolygon(p) => p.iter().flatten().map(|l| path_len(l)).sum(),
        Geometry::GeometryCollection(g) => g.iter().map(length_m).sum(),
    }
}

/// Unsigned area (in steradians) enclosed by a ring of lon/lat vertices whose edges are
/// great-circle arcs.
///
/// Spherical excess: the area of a spherical polygon is `R^2 * E`, where `E` is the sum
/// of the angles minus `(n-2) pi`. Rather than computing angles, we sum signed triangle
/// excesses between each edge and the north pole. For an edge from `(phi1, l1)` to
/// `(phi2, l2)` with `dl = l2 - l1` wrapped to `[-pi, pi]`, that excess is
/// `2 atan2( tan(dl/2) (tan(phi1/2) + tan(phi2/2)), 1 + tan(phi1/2) tan(phi2/2) )`.
/// Triangles outside the polygon cancel, leaving the enclosed area, so no special case
/// is needed for concave shapes or the antimeridian.
fn ring_excess(ring: &[Coord]) -> f64 {
    if ring.len() < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    // `windows(2)` skips the implicit closing edge when a ring is not explicitly
    // closed, so add it separately.
    let closing = if ring.first() == ring.last() { None } else { Some([*ring.last().unwrap(), ring[0]]) };
    let edges = ring.windows(2).map(|w| [w[0], w[1]]).chain(closing);
    for [a, b] in edges {
        let mut dl = (b[0] - a[0]).to_radians();
        dl = dl.sin().atan2(dl.cos()); // wrap to [-pi, pi]
        let (t1, t2) = ((a[1].to_radians() / 2.0).tan(), (b[1].to_radians() / 2.0).tan());
        sum += 2.0 * ((dl / 2.0).tan() * (t1 + t2)).atan2(1.0 + t1 * t2);
    }
    sum.abs()
}

fn polygon_area(rings: &[Vec<Coord>]) -> f64 {
    let mut it = rings.iter();
    let outer = it.next().map_or(0.0, |r| ring_excess(r));
    let holes: f64 = it.map(|r| ring_excess(r)).sum();
    ((outer - holes) * AUTHALIC_R * AUTHALIC_R).max(0.0)
}

/// Spherical area in square metres of the polygonal parts (holes subtracted) of a
/// lon/lat geometry; 0 for points and lines.
pub fn area_m2(geometry: &Geometry) -> f64 {
    match geometry {
        Geometry::Polygon(r) => polygon_area(r),
        Geometry::MultiPolygon(p) => p.iter().map(|r| polygon_area(r)).sum(),
        Geometry::GeometryCollection(g) => g.iter().map(area_m2).sum(),
        _ => 0.0,
    }
}

/// Group digits with commas: `1204` becomes `1,204`.
pub(crate) fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Format a decimal with a fixed number of places and thousands separators.
fn grouped(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    let (int, frac) = s.split_once('.').map_or((s.as_str(), None), |(i, f)| (i, Some(f)));
    let int = thousands(int.parse().unwrap_or(0));
    match frac {
        Some(f) => format!("{int}.{f}"),
        None => int,
    }
}

/// Human distance: `"7.5 m"`, `"532 m"`, `"12.4 km"`, `"1,234 km"`.
pub fn format_distance(m: f64) -> String {
    let m = if m.is_finite() { m.max(0.0) } else { 0.0 };
    if m < 10.0 {
        return format!("{m:.1} m");
    }
    if m.round() < 1000.0 {
        return format!("{m:.0} m");
    }
    let km = m / 1000.0;
    if km < 100.0 { format!("{} km", grouped(km, 1)) } else { format!("{} km", grouped(km, 0)) }
}

/// Human area: `"532 m²"`, `"3.2 ha"`, `"18.7 km²"`, `"1,204 km²"`.
pub fn format_area(m2: f64) -> String {
    let m2 = if m2.is_finite() { m2.max(0.0) } else { 0.0 };
    if m2 < 10_000.0 {
        format!("{} m²", grouped(m2, 0))
    } else if m2 < 1_000_000.0 {
        format!("{:.1} ha", m2 / 10_000.0)
    } else if m2 < 1_000_000_000.0 {
        format!("{} km²", grouped(m2 / 1e6, 1))
    } else {
        format!("{} km²", grouped(m2 / 1e6, 0))
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", thousands(n as u64), if n == 1 { one } else { many })
}

fn short(v: f64) -> String {
    let s = format!("{v:.4}");
    let t = s.trim_end_matches('0').trim_end_matches('.');
    if t == "-0" { "0".into() } else { t.into() }
}

/// A one-line summary for a grid cell or the inspector, e.g.
/// `Point (151.2093, -33.8688) · SRID 4326` or
/// `MultiPolygon · 3 parts · 1,204 vertices · 18.7 km²`.
///
/// Length/area are included when the coordinates can be placed on the globe (a known
/// SRID, or unlabelled data that fits lon/lat); projected data is reprojected first.
pub fn describe(feature: &Feature) -> String {
    let g = &feature.geometry;
    let mut parts: Vec<String> = Vec::new();
    match g {
        Geometry::Point(Some(c)) => parts.push(format!("Point ({}, {})", short(c[0]), short(c[1]))),
        _ if g.is_empty() => parts.push(format!("{} EMPTY", g.type_name())),
        _ => {
            parts.push(g.type_name().to_string());
            match g {
                Geometry::MultiPoint(p) => parts.push(plural(p.len(), "part", "parts")),
                Geometry::MultiLineString(p) => parts.push(plural(p.len(), "part", "parts")),
                Geometry::MultiPolygon(p) => parts.push(plural(p.len(), "part", "parts")),
                Geometry::GeometryCollection(p) => parts.push(plural(p.len(), "geometry", "geometries")),
                _ => {}
            }
            parts.push(plural(g.coord_count(), "vertex", "vertices"));
            let crs = guess_crs(feature.srid, g.bbox());
            if crs != Crs::Planar {
                let ll = g.map_coords(&|c| to_lon_lat(c, crs).unwrap_or(c));
                let area = area_m2(&ll);
                if area > 0.0 {
                    parts.push(format_area(area));
                } else {
                    let len = length_m(&ll);
                    if len > 0.0 {
                        parts.push(format_distance(len));
                    }
                }
            }
        }
    }
    if let Some(s) = feature.srid {
        parts.push(format!("SRID {s}"));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wkt::parse_wkt;

    fn f(s: &str) -> Feature {
        parse_wkt(s).unwrap()
    }

    #[test]
    fn haversine_basics() {
        assert_eq!(haversine_m([10.0, 20.0], [10.0, 20.0]), 0.0);
        // One degree of latitude is ~111.2 km.
        let d = haversine_m([0.0, 0.0], [0.0, 1.0]);
        assert!((d - 111_195.0).abs() < 50.0, "{d}");
        // Antipodes: half the circumference.
        let d = haversine_m([0.0, 0.0], [180.0, 0.0]);
        assert!((d - std::f64::consts::PI * EARTH_R).abs() < 1.0);
    }

    #[test]
    fn haversine_symmetric_and_antimeridian() {
        let (a, b) = ([151.2, -33.9], [-122.4, 37.8]);
        assert!((haversine_m(a, b) - haversine_m(b, a)).abs() < 1e-6);
        let d = haversine_m([179.9, 0.0], [-179.9, 0.0]);
        assert!((d - 22_239.0).abs() < 20.0, "{d}");
    }

    #[test]
    fn length_sums_segments_and_rings() {
        let l = f("LINESTRING(0 0,0 1,0 2)").geometry;
        assert!((length_m(&l) - 2.0 * 111_195.0).abs() < 100.0);
        let p = f("POLYGON((0 0,1 0,1 1,0 1,0 0))").geometry;
        assert!((length_m(&p) - 4.0 * 111_195.0).abs() < 1500.0);
        assert_eq!(length_m(&f("POINT(1 2)").geometry), 0.0);
        assert_eq!(length_m(&f("MULTIPOINT(1 2,3 4)").geometry), 0.0);
        assert!(
            length_m(&f("GEOMETRYCOLLECTION(LINESTRING(0 0,0 1),LINESTRING(0 0,0 1))").geometry) > 220_000.0
        );
        assert!(length_m(&f("MULTILINESTRING((0 0,0 1),(0 0,0 1))").geometry) > 220_000.0);
    }

    #[test]
    fn area_one_degree_square_at_equator() {
        // 111.195 km x 111.195 km ~ 12 364 km² (slightly less: the arcs bulge poleward).
        let a = area_m2(&f("POLYGON((0 0,1 0,1 1,0 1,0 0))").geometry);
        assert!((a / 1e6 - 12_364.0).abs() < 30.0, "{}", a / 1e6);
    }

    #[test]
    fn area_orientation_independent() {
        let ccw = area_m2(&f("POLYGON((0 0,1 0,1 1,0 1,0 0))").geometry);
        let cw = area_m2(&f("POLYGON((0 0,0 1,1 1,1 0,0 0))").geometry);
        assert!((ccw - cw).abs() < 1.0);
    }

    #[test]
    fn area_holes_subtracted() {
        let full = area_m2(&f("POLYGON((0 0,2 0,2 2,0 2,0 0))").geometry);
        let holed =
            area_m2(&f("POLYGON((0 0,2 0,2 2,0 2,0 0),(0.5 0.5,1.5 0.5,1.5 1.5,0.5 1.5,0.5 0.5))").geometry);
        let hole = area_m2(&f("POLYGON((0.5 0.5,1.5 0.5,1.5 1.5,0.5 1.5,0.5 0.5))").geometry);
        assert!((full - hole - holed).abs() < 1.0);
    }

    #[test]
    fn area_antimeridian_and_unclosed() {
        let a = area_m2(&f("POLYGON((179 0,-179 0,-179 1,179 1,179 0))").geometry);
        let b = area_m2(&f("POLYGON((0 0,2 0,2 1,0 1,0 0))").geometry);
        assert!((a - b).abs() / b < 1e-6);
        let open = area_m2(&Geometry::Polygon(vec![vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]]));
        assert!(open > 1.2e10);
    }

    #[test]
    fn area_zero_for_non_areal_and_degenerate() {
        assert_eq!(area_m2(&f("LINESTRING(0 0,1 1)").geometry), 0.0);
        assert_eq!(area_m2(&f("POINT(0 0)").geometry), 0.0);
        assert_eq!(area_m2(&f("POLYGON EMPTY").geometry), 0.0);
        assert_eq!(area_m2(&Geometry::Polygon(vec![vec![[0.0, 0.0], [1.0, 1.0]]])), 0.0);
    }

    #[test]
    fn area_multi_and_collection_sum() {
        let one = area_m2(&f("POLYGON((0 0,1 0,1 1,0 1,0 0))").geometry);
        let two = area_m2(&f("MULTIPOLYGON(((0 0,1 0,1 1,0 1,0 0)),((10 0,11 0,11 1,10 1,10 0)))").geometry);
        assert!((two - 2.0 * one).abs() / one < 1e-3);
        let c = area_m2(&f("GEOMETRYCOLLECTION(POLYGON((0 0,1 0,1 1,0 1,0 0)),POINT(1 1))").geometry);
        assert!((c - one).abs() < 1.0);
    }

    #[test]
    fn thousands_grouping() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(1_204), "1,204");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn distance_format() {
        assert_eq!(format_distance(0.0), "0.0 m");
        assert_eq!(format_distance(7.46), "7.5 m");
        assert_eq!(format_distance(532.2), "532 m");
        assert_eq!(format_distance(999.6), "1.0 km");
        assert_eq!(format_distance(12_400.0), "12.4 km");
        assert_eq!(format_distance(123_456.0), "123 km");
        assert_eq!(format_distance(1_234_567.0), "1,235 km");
        assert_eq!(format_distance(f64::NAN), "0.0 m");
        assert_eq!(format_distance(-5.0), "0.0 m");
    }

    #[test]
    fn area_format() {
        assert_eq!(format_area(532.0), "532 m²");
        assert_eq!(format_area(32_000.0), "3.2 ha");
        assert_eq!(format_area(18_700_000.0), "18.7 km²");
        assert_eq!(format_area(1_204_000_000.0), "1,204 km²");
        assert_eq!(format_area(f64::INFINITY), "0 m²");
        assert_eq!(format_area(5_000.0), "5,000 m²");
    }

    #[test]
    fn describe_point() {
        assert_eq!(
            describe(&f("SRID=4326;POINT(151.2093 -33.8688)")),
            "Point (151.2093, -33.8688) · SRID 4326"
        );
        assert_eq!(describe(&f("POINT(1 2)")), "Point (1, 2)");
        assert_eq!(describe(&f("POINT EMPTY")), "Point EMPTY");
        assert_eq!(describe(&f("SRID=4326;POINT EMPTY")), "Point EMPTY · SRID 4326");
    }

    #[test]
    fn describe_areal_and_linear() {
        let d = describe(&f("SRID=4326;POLYGON((0 0,1 0,1 1,0 1,0 0))"));
        assert!(d.starts_with("Polygon · 5 vertices · 12,3") && d.ends_with("km² · SRID 4326"), "{d}");
        let d = describe(&f("SRID=4326;LINESTRING(0 0,0 1)"));
        assert_eq!(d, "LineString · 2 vertices · 111 km · SRID 4326");
        let d = describe(&f("MULTIPOLYGON(((0 0,1 0,1 1,0 0)),((5 5,6 5,6 6,5 5)))"));
        assert!(d.starts_with("MultiPolygon · 2 parts · 8 vertices · "), "{d}");
        let d = describe(&f("MULTIPOINT(1 2)"));
        assert_eq!(d, "MultiPoint · 1 part · 1 vertex");
        let d = describe(&f("GEOMETRYCOLLECTION(POINT(1 2),POINT(2 3))"));
        assert_eq!(d, "GeometryCollection · 2 geometries · 2 vertices");
    }

    #[test]
    fn describe_planar_skips_measures_and_projected_reprojects() {
        let d = describe(&f("SRID=27700;LINESTRING(0 0,100000 0)"));
        assert_eq!(d, "LineString · 2 vertices · SRID 27700");
        let d = describe(&f("SRID=32631;LINESTRING(500000 0,600000 0)"));
        assert!(d.contains("km"), "{d}");
        let d = describe(&f("LINESTRING(0 0,100000 0)"));
        assert_eq!(d, "LineString · 2 vertices");
        assert_eq!(describe(&f("POLYGON EMPTY")), "Polygon EMPTY");
    }
}
