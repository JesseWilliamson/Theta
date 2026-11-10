//! Rendering helpers: simplification before drawing, and pointer hit testing.

use crate::geometry::{Coord, Geometry};

/// Distance from `p` to the segment `a`-`b`; a zero-length segment degrades to point distance.
fn dist_to_segment(p: Coord, a: Coord, b: Coord) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (p[0] - (a[0] + t * dx)).hypot(p[1] - (a[1] + t * dy))
}

/// Douglas-Peucker without recursion. Returns, for every point, the distance at which
/// it was split off (`INFINITY` for the two endpoints). Segments are only subdivided
/// when their farthest point exceeds `stop_below`, so `stop_below = tolerance` yields the
/// ordinary algorithm and `0` yields a complete ranking of all points by significance.
fn dp_significance(pts: &[Coord], stop_below: f64) -> Vec<f64> {
    let n = pts.len();
    let mut sig = vec![0.0; n];
    if n == 0 {
        return sig;
    }
    sig[0] = f64::INFINITY;
    sig[n - 1] = f64::INFINITY;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((i, j)) = stack.pop() {
        if j <= i + 1 {
            continue;
        }
        let (mut best, mut best_d) = (i, -1.0);
        for k in i + 1..j {
            let d = dist_to_segment(pts[k], pts[i], pts[j]);
            if d > best_d {
                (best, best_d) = (k, d);
            }
        }
        if best_d > stop_below || (stop_below == 0.0 && best_d >= 0.0) {
            sig[best] = best_d;
            stack.push((i, best));
            stack.push((best, j));
        }
    }
    sig
}

/// Simplify one vertex list, keeping at least `min_points` (when the input has that many).
fn simplify_path(pts: &[Coord], tolerance: f64, min_points: usize) -> Vec<Coord> {
    if pts.len() <= min_points {
        return pts.to_vec();
    }
    let sig = dp_significance(pts, tolerance);
    let mut keep: Vec<bool> = sig.iter().map(|s| *s > tolerance).collect();
    let kept = keep.iter().filter(|k| **k).count();
    if kept < min_points {
        // Top up with the most significant dropped vertices (full ranking, no cut-off).
        let full = dp_significance(pts, 0.0);
        let mut dropped: Vec<usize> = (0..pts.len()).filter(|i| !keep[*i]).collect();
        dropped.sort_by(|a, b| full[*b].total_cmp(&full[*a]));
        for i in dropped.into_iter().take(min_points - kept) {
            keep[i] = true;
        }
    }
    pts.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

/// Douglas-Peucker simplification with `tolerance` in the geometry's own units.
///
/// Rings keep their closure and never drop below 4 points; lines never drop below 2.
/// Points are returned unchanged. A tolerance that is not a positive finite number
/// returns a plain copy.
pub fn simplify(geometry: &Geometry, tolerance: f64) -> Geometry {
    if !(tolerance.is_finite() && tolerance > 0.0) {
        return geometry.clone();
    }
    let line = |l: &Vec<Coord>| simplify_path(l, tolerance, 2);
    let ring = |r: &Vec<Coord>| simplify_path(r, tolerance, 4);
    let rings = |rs: &Vec<Vec<Coord>>| rs.iter().map(ring).collect::<Vec<_>>();
    match geometry {
        Geometry::Point(_) | Geometry::MultiPoint(_) => geometry.clone(),
        Geometry::LineString(l) => Geometry::LineString(line(l)),
        Geometry::Polygon(r) => Geometry::Polygon(rings(r)),
        Geometry::MultiLineString(ls) => Geometry::MultiLineString(ls.iter().map(line).collect()),
        Geometry::MultiPolygon(ps) => Geometry::MultiPolygon(ps.iter().map(rings).collect()),
        Geometry::GeometryCollection(g) => {
            Geometry::GeometryCollection(g.iter().map(|g| simplify(g, tolerance)).collect())
        }
    }
}

type Pt = [f64; 2];

fn project_all(cs: &[Coord], project: &dyn Fn(Coord) -> Pt) -> Vec<Pt> {
    cs.iter().map(|c| project(*c)).collect()
}

fn near_path(pts: &[Pt], closed: bool, p: Pt, tol: f64) -> bool {
    if pts.len() == 1 {
        return (pts[0][0] - p[0]).hypot(pts[0][1] - p[1]) <= tol;
    }
    let closing = (closed && pts.len() > 2).then(|| (pts[pts.len() - 1], pts[0]));
    pts.windows(2).map(|w| (w[0], w[1])).chain(closing).any(|(a, b)| dist_to_segment(p, a, b) <= tol)
}

/// Even-odd point-in-polygon over all rings together, which handles holes (and
/// islands inside holes) without caring about ring orientation.
fn inside_even_odd(rings: &[Vec<Pt>], p: Pt) -> bool {
    let mut inside = false;
    for ring in rings {
        let n = ring.len();
        if n < 3 {
            continue;
        }
        let mut j = n - 1;
        for i in 0..n {
            let (a, b) = (ring[i], ring[j]);
            if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
                inside = !inside;
            }
            j = i;
        }
    }
    inside
}

fn hit_polygon(rings: &[Vec<Coord>], project: &dyn Fn(Coord) -> Pt, p: Pt, tol: f64) -> bool {
    let screen: Vec<Vec<Pt>> = rings.iter().map(|r| project_all(r, project)).collect();
    screen.iter().any(|r| !r.is_empty() && near_path(r, true, p, tol)) || inside_even_odd(&screen, p)
}

/// Is `point` within `tolerance_px` of the geometry once each vertex is mapped through
/// `project`? Done in screen space so the tolerance means the same on-screen distance at
/// any zoom. Points use distance, lines use distance to segments, polygons hit when the
/// point is inside (even-odd, holes respected) or near any ring boundary.
pub fn hit_test(geometry: &Geometry, project: &dyn Fn(Coord) -> Pt, point: Pt, tolerance_px: f64) -> bool {
    match geometry {
        Geometry::Point(None) => false,
        Geometry::Point(Some(c)) => {
            let s = project(*c);
            (s[0] - point[0]).hypot(s[1] - point[1]) <= tolerance_px
        }
        Geometry::MultiPoint(cs) => cs.iter().any(|c| {
            let s = project(*c);
            (s[0] - point[0]).hypot(s[1] - point[1]) <= tolerance_px
        }),
        Geometry::LineString(l) => {
            !l.is_empty() && near_path(&project_all(l, project), false, point, tolerance_px)
        }
        Geometry::MultiLineString(ls) => ls
            .iter()
            .any(|l| !l.is_empty() && near_path(&project_all(l, project), false, point, tolerance_px)),
        Geometry::Polygon(r) => hit_polygon(r, project, point, tolerance_px),
        Geometry::MultiPolygon(ps) => ps.iter().any(|r| hit_polygon(r, project, point, tolerance_px)),
        Geometry::GeometryCollection(g) => g.iter().any(|g| hit_test(g, project, point, tolerance_px)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wkt::parse_wkt;

    fn g(s: &str) -> Geometry {
        parse_wkt(s).unwrap().geometry
    }

    fn id(c: Coord) -> [f64; 2] {
        c
    }

    #[test]
    fn dp_removes_collinear_points() {
        let s = simplify(&g("LINESTRING(0 0,1 0,2 0,3 0,4 0)"), 0.1);
        assert_eq!(s, g("LINESTRING(0 0,4 0)"));
    }

    #[test]
    fn dp_keeps_significant_bends() {
        let s = simplify(&g("LINESTRING(0 0,1 0.01,2 5,3 0.01,4 0)"), 0.9);
        assert_eq!(s, g("LINESTRING(0 0,1 0.01,2 5,3 0.01,4 0)"));
        let s = simplify(&g("LINESTRING(0 0,1 0.01,2 5,3 0.01,4 0)"), 1.0);
        assert_eq!(s, g("LINESTRING(0 0,2 5,4 0)"));
        let s = simplify(&g("LINESTRING(0 0,1 0.01,2 5,3 0.01,4 0)"), 10.0);
        assert_eq!(s, g("LINESTRING(0 0,4 0)"));
    }

    #[test]
    fn dp_tolerance_boundary() {
        // Middle vertex is exactly 1.0 away: kept only when strictly greater than tolerance.
        let l = g("LINESTRING(0 0,1 1,2 0)");
        assert_eq!(simplify(&l, 1.0), g("LINESTRING(0 0,2 0)"));
        assert_eq!(simplify(&l, 0.999), l);
    }

    #[test]
    fn line_never_below_two_points() {
        let s = simplify(&g("LINESTRING(0 0,0.1 0,0.2 0)"), 100.0);
        assert_eq!(s, g("LINESTRING(0 0,0.2 0)"));
        assert_eq!(simplify(&g("LINESTRING(0 0,1 1)"), 100.0), g("LINESTRING(0 0,1 1)"));
        assert_eq!(simplify(&g("LINESTRING EMPTY"), 1.0), g("LINESTRING EMPTY"));
    }

    #[test]
    fn ring_never_below_four_points_and_stays_closed() {
        let tiny = g("POLYGON((0 0,1 0,1 1,0 1,0 0))");
        let Geometry::Polygon(r) = simplify(&tiny, 1000.0) else { panic!() };
        assert!(r[0].len() >= 4, "{:?}", r[0]);
        assert_eq!(r[0].first(), r[0].last());
        // Many-vertex circle simplifies a lot but remains a valid ring.
        let pts: Vec<String> = (0..=360)
            .map(|d| {
                let a = f64::from(d % 360).to_radians();
                format!("{} {}", a.cos() * 100.0, a.sin() * 100.0)
            })
            .collect();
        let circle = g(&format!("POLYGON(({}))", pts.join(",")));
        let Geometry::Polygon(r) = simplify(&circle, 2.0) else { panic!() };
        assert!(r[0].len() < 60 && r[0].len() >= 8, "{}", r[0].len());
        assert_eq!(r[0].first(), r[0].last());
    }

    #[test]
    fn simplify_error_bounded() {
        // Every dropped vertex stays within tolerance of the simplified line.
        let pts: Vec<Coord> = (0..200).map(|i| [f64::from(i), (f64::from(i) * 0.3).sin() * 5.0]).collect();
        let tol = 0.7;
        let Geometry::LineString(s) = simplify(&Geometry::LineString(pts.clone()), tol) else { panic!() };
        assert!(s.len() < pts.len());
        for p in &pts {
            let d = s.windows(2).map(|w| dist_to_segment(*p, w[0], w[1])).fold(f64::INFINITY, f64::min);
            assert!(d <= tol + 1e-9, "{p:?} {d}");
        }
    }

    #[test]
    fn simplify_misc() {
        let p = g("POINT(1 2)");
        assert_eq!(simplify(&p, 5.0), p);
        let mp = g("MULTIPOINT(1 2,1 2)");
        assert_eq!(simplify(&mp, 5.0), mp);
        let l = g("LINESTRING(0 0,1 1,2 0)");
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(simplify(&l, bad), l);
        }
        let c = g("GEOMETRYCOLLECTION(LINESTRING(0 0,1 0,2 0),POLYGON((0 0,4 0,4 4,0 0)))");
        let Geometry::GeometryCollection(s) = simplify(&c, 0.1) else { panic!() };
        assert_eq!(s[0], g("LINESTRING(0 0,2 0)"));
        let m = g("MULTILINESTRING((0 0,1 0,2 0),(5 5,6 5,7 5))");
        assert_eq!(simplify(&m, 0.1), g("MULTILINESTRING((0 0,2 0),(5 5,7 5))"));
        let mp = g("MULTIPOLYGON(((0 0,4 0,4 4,0 4,0 0)))");
        assert_eq!(simplify(&mp, 0.1), mp);
    }

    #[test]
    fn hit_point() {
        let p = g("POINT(10 10)");
        assert!(hit_test(&p, &id, [12.0, 10.0], 3.0));
        assert!(!hit_test(&p, &id, [14.0, 10.0], 3.0));
        assert!(!hit_test(&g("POINT EMPTY"), &id, [0.0, 0.0], 100.0));
        assert!(hit_test(&g("MULTIPOINT(0 0,50 50)"), &id, [49.0, 50.0], 2.0));
        assert!(!hit_test(&g("MULTIPOINT(0 0,50 50)"), &id, [25.0, 25.0], 2.0));
    }

    #[test]
    fn hit_line() {
        let l = g("LINESTRING(0 0,100 0,100 100)");
        assert!(hit_test(&l, &id, [50.0, 3.0], 4.0));
        assert!(!hit_test(&l, &id, [50.0, 10.0], 4.0));
        assert!(hit_test(&l, &id, [103.0, 50.0], 4.0));
        // Beyond the end the distance is to the endpoint, not the infinite line.
        assert!(!hit_test(&l, &id, [100.0, 130.0], 4.0));
        assert!(hit_test(&g("MULTILINESTRING((0 0,10 0),(50 50,60 50))"), &id, [55.0, 51.0], 2.0));
        assert!(!hit_test(&g("LINESTRING EMPTY"), &id, [0.0, 0.0], 100.0));
    }

    #[test]
    fn hit_polygon_inside_edge_outside() {
        let p = g("POLYGON((0 0,100 0,100 100,0 100,0 0))");
        assert!(hit_test(&p, &id, [50.0, 50.0], 1.0));
        assert!(hit_test(&p, &id, [101.0, 50.0], 2.0));
        assert!(!hit_test(&p, &id, [110.0, 50.0], 2.0));
        assert!(!hit_test(&p, &id, [-5.0, -5.0], 2.0));
    }

    #[test]
    fn hit_polygon_hole() {
        let p = g("POLYGON((0 0,100 0,100 100,0 100,0 0),(25 25,75 25,75 75,25 75,25 25))");
        assert!(!hit_test(&p, &id, [50.0, 50.0], 1.0));
        assert!(hit_test(&p, &id, [10.0, 10.0], 1.0));
        assert!(hit_test(&p, &id, [26.0, 50.0], 2.0)); // near the hole's border
        let island = g("POLYGON((0 0,100 0,100 100,0 100,0 0),(10 10,90 10,90 90,10 90,10 10))");
        assert!(!hit_test(&island, &id, [50.0, 50.0], 1.0));
    }

    #[test]
    fn hit_multipolygon_collection_and_concave() {
        let m = g("MULTIPOLYGON(((0 0,10 0,10 10,0 0)),((50 50,60 50,60 60,50 60,50 50)))");
        assert!(hit_test(&m, &id, [55.0, 55.0], 0.0));
        assert!(!hit_test(&m, &id, [30.0, 30.0], 1.0));
        let c = g("GEOMETRYCOLLECTION(POINT(1 1),POLYGON((50 50,60 50,60 60,50 50)))");
        assert!(hit_test(&c, &id, [1.0, 1.5], 1.0));
        assert!(hit_test(&c, &id, [58.0, 55.0], 0.0));
        let u = g("POLYGON((0 0,30 0,30 30,20 30,20 10,10 10,10 30,0 30,0 0))");
        assert!(!hit_test(&u, &id, [15.0, 20.0], 1.0));
        assert!(hit_test(&u, &id, [5.0, 20.0], 1.0));
        assert!(!hit_test(&g("POLYGON EMPTY"), &id, [0.0, 0.0], 10.0));
    }

    #[test]
    fn hit_uses_projection() {
        // y flipped and scaled x2: screen = (2x, 100 - y)
        let proj = |c: Coord| [c[0] * 2.0, 100.0 - c[1]];
        let p = g("POINT(10 20)");
        assert!(hit_test(&p, &proj, [20.0, 80.0], 0.5));
        assert!(!hit_test(&p, &id, [20.0, 80.0], 0.5));
    }

    #[test]
    fn hit_unclosed_ring_uses_closing_edge() {
        let open = Geometry::Polygon(vec![vec![[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]]]);
        assert!(hit_test(&open, &id, [0.5, 50.0], 1.0));
    }
}
