//! Screen-space clipping.
//!
//! GPUI tessellates paths with `f32`. A country polygon at street zoom has vertices at
//! ±10⁷ px, where `f32` has several pixels of error, which visibly shears edges. Clipping
//! to a rectangle just outside the viewport first keeps every coordinate small.

/// A screen point.
pub type P = [f64; 2];

/// Axis-aligned clip rectangle `[x0, y0, x1, y1]`.
pub type Rect = [f64; 4];

/// Whether every point lies inside `r` (so clipping can be skipped).
pub fn all_inside(pts: &[P], r: &Rect) -> bool {
    pts.iter().all(|p| p[0] >= r[0] && p[0] <= r[2] && p[1] >= r[1] && p[1] <= r[3])
}

/// Whether all points are on the outer side of one edge (the shape is entirely invisible).
pub fn all_outside(pts: &[P], r: &Rect) -> bool {
    pts.iter().all(|p| p[0] < r[0])
        || pts.iter().all(|p| p[0] > r[2])
        || pts.iter().all(|p| p[1] < r[1])
        || pts.iter().all(|p| p[1] > r[3])
}

fn clip_edge(input: &[P], inside: impl Fn(P) -> bool, cut: impl Fn(P, P) -> P, out: &mut Vec<P>) {
    out.clear();
    let n = input.len();
    for i in 0..n {
        let (cur, prev) = (input[i], input[(i + n - 1) % n]);
        match (inside(cur), inside(prev)) {
            (true, true) => out.push(cur),
            (true, false) => {
                out.push(cut(prev, cur));
                out.push(cur);
            }
            (false, true) => out.push(cut(prev, cur)),
            (false, false) => {}
        }
    }
}

/// Sutherland-Hodgman: clip a closed ring (no repeated closing vertex needed; one is
/// tolerated) to `r`. May return an empty vector.
pub fn clip_ring(ring: &[P], r: &Rect) -> Vec<P> {
    let mut pts: Vec<P> = ring.to_vec();
    if pts.len() > 1 && pts.first() == pts.last() {
        pts.pop();
    }
    if pts.len() < 3 {
        return vec![];
    }
    let mut tmp = Vec::with_capacity(pts.len() + 8);
    let lerp_x = |a: P, b: P, x: f64| [x, a[1] + (b[1] - a[1]) * (x - a[0]) / (b[0] - a[0])];
    let lerp_y = |a: P, b: P, y: f64| [a[0] + (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]), y];
    clip_edge(&pts, |p| p[0] >= r[0], |a, b| lerp_x(a, b, r[0]), &mut tmp);
    if tmp.is_empty() {
        return tmp;
    }
    clip_edge(&tmp, |p| p[0] <= r[2], |a, b| lerp_x(a, b, r[2]), &mut pts);
    if pts.is_empty() {
        return pts;
    }
    clip_edge(&pts, |p| p[1] >= r[1], |a, b| lerp_y(a, b, r[1]), &mut tmp);
    if tmp.is_empty() {
        return tmp;
    }
    clip_edge(&tmp, |p| p[1] <= r[3], |a, b| lerp_y(a, b, r[3]), &mut pts);
    pts
}

/// Liang-Barsky for one segment: the visible part, if any.
fn clip_segment(a: P, b: P, r: &Rect) -> Option<(P, P)> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [(-dx, a[0] - r[0]), (dx, r[2] - a[0]), (-dy, a[1] - r[1]), (dy, r[3] - a[1])] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                if t > t1 {
                    return None;
                }
                t0 = t0.max(t);
            } else {
                if t < t0 {
                    return None;
                }
                t1 = t1.min(t);
            }
        }
    }
    Some(([a[0] + t0 * dx, a[1] + t0 * dy], [a[0] + t1 * dx, a[1] + t1 * dy]))
}

/// Clip an open polyline to `r`, splitting it where it leaves and re-enters.
pub fn clip_polyline(pts: &[P], r: &Rect) -> Vec<Vec<P>> {
    let mut out: Vec<Vec<P>> = Vec::new();
    let mut cur: Vec<P> = Vec::new();
    for w in pts.windows(2) {
        match clip_segment(w[0], w[1], r) {
            Some((a, b)) => {
                if cur.last() != Some(&a) {
                    if cur.len() > 1 {
                        out.push(std::mem::take(&mut cur));
                    } else {
                        cur.clear();
                    }
                    cur.push(a);
                }
                cur.push(b);
            }
            None => {
                if cur.len() > 1 {
                    out.push(std::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
            }
        }
    }
    if cur.len() > 1 {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: Rect = [0.0, 0.0, 10.0, 10.0];

    #[test]
    fn ring_fully_inside_is_unchanged() {
        let ring = vec![[1.0, 1.0], [9.0, 1.0], [9.0, 9.0], [1.0, 9.0]];
        assert!(all_inside(&ring, &R));
        assert_eq!(clip_ring(&ring, &R).len(), 4);
    }

    #[test]
    fn huge_ring_is_clipped_to_the_rect() {
        let ring = vec![[-1e8, -1e8], [1e8, -1e8], [1e8, 1e8], [-1e8, 1e8], [-1e8, -1e8]];
        let c = clip_ring(&ring, &R);
        assert_eq!(c.len(), 4);
        for p in &c {
            assert!(p[0] >= 0.0 && p[0] <= 10.0 && p[1] >= 0.0 && p[1] <= 10.0, "{p:?}");
        }
    }

    #[test]
    fn ring_outside_vanishes() {
        let ring = vec![[20.0, 20.0], [30.0, 20.0], [30.0, 30.0]];
        assert!(all_outside(&ring, &R));
        assert!(clip_ring(&ring, &R).is_empty());
    }

    #[test]
    fn triangle_crossing_one_edge() {
        let ring = vec![[5.0, 5.0], [15.0, 5.0], [5.0, 15.0]];
        let c = clip_ring(&ring, &R);
        assert!(c.len() >= 4);
        assert!(c.iter().all(|p| p[0] <= 10.0 + 1e-9 && p[1] <= 10.0 + 1e-9));
    }

    #[test]
    fn polyline_split_on_exit_and_reentry() {
        let line = vec![[-5.0, 5.0], [5.0, 5.0], [5.0, 20.0], [8.0, 20.0], [8.0, 5.0]];
        let parts = clip_polyline(&line, &R);
        assert_eq!(parts.len(), 2, "{parts:?}");
        assert_eq!(parts[0].first(), Some(&[0.0, 5.0]));
        assert_eq!(parts[0].last(), Some(&[5.0, 10.0]));
        assert_eq!(parts[1].first(), Some(&[8.0, 10.0]));
    }

    #[test]
    fn polyline_outside_is_empty() {
        assert!(clip_polyline(&[[-5.0, -5.0], [-1.0, -9.0]], &R).is_empty());
    }
}

/// Largest ring handled by [`earclip`]; ear clipping is O(n^2) and only meant for the tiny
/// polygons that dominate dense data sets (bigger ones go through the general tessellator).
pub const EARCLIP_MAX: usize = 40;

fn cross(a: P, b: P, c: P) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn in_triangle(p: P, a: P, b: P, c: P) -> bool {
    cross(a, b, p) >= 0.0 && cross(b, c, p) >= 0.0 && cross(c, a, p) >= 0.0
}

/// Triangulate a simple ring (no holes) by ear clipping. Returns triangles as index triples
/// into `pts`, or `None` when the ring is too big, degenerate or self-intersecting enough
/// that no ear can be found (callers then fall back to a general tessellator).
pub fn earclip(pts: &[P]) -> Option<Vec<[usize; 3]>> {
    let mut n = pts.len();
    if pts.len() > 1 && pts.first() == pts.last() {
        n -= 1;
    }
    if !(3..=EARCLIP_MAX).contains(&n) {
        return None;
    }
    let area2: f64 = (0..n).map(|i| pts[i][0] * pts[(i + 1) % n][1] - pts[(i + 1) % n][0] * pts[i][1]).sum();
    if area2 == 0.0 {
        return None;
    }
    let mut list: Vec<usize> = if area2 > 0.0 { (0..n).collect() } else { (0..n).rev().collect() };
    let mut tris = Vec::with_capacity(n - 2);
    let (mut i, mut stall) = (0usize, 0usize);
    while list.len() > 3 {
        let m = list.len();
        let (ia, ib, ic) = (list[(i + m - 1) % m], list[i], list[(i + 1) % m]);
        let (a, b, c) = (pts[ia], pts[ib], pts[ic]);
        let cr = cross(a, b, c);
        if cr == 0.0 {
            // Collinear (or a repeated vertex): contributes no area, just drop it.
            list.remove(i);
            i %= list.len();
            stall = 0;
            continue;
        }
        let ear =
            cr > 0.0 && !list.iter().any(|&j| j != ia && j != ib && j != ic && in_triangle(pts[j], a, b, c));
        if ear {
            tris.push([ia, ib, ic]);
            list.remove(i);
            i %= list.len();
            stall = 0;
        } else {
            i = (i + 1) % m;
            stall += 1;
            if stall > m {
                return None;
            }
        }
    }
    if cross(pts[list[0]], pts[list[1]], pts[list[2]]) != 0.0 {
        tris.push([list[0], list[1], list[2]]);
    }
    Some(tris)
}

#[cfg(test)]
mod earclip_tests {
    use super::*;

    fn area(pts: &[P], tris: &[[usize; 3]]) -> f64 {
        tris.iter().map(|t| cross(pts[t[0]], pts[t[1]], pts[t[2]]).abs() / 2.0).sum()
    }

    #[test]
    fn square_is_two_triangles() {
        let sq = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]];
        let t = earclip(&sq).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(area(&sq, &t), 16.0);
    }

    #[test]
    fn concave_l_shape_keeps_its_area_in_either_winding() {
        let l = [[0.0, 0.0], [4.0, 0.0], [4.0, 2.0], [2.0, 2.0], [2.0, 4.0], [0.0, 4.0]];
        assert_eq!(area(&l, &earclip(&l).unwrap()), 12.0);
        let rev: Vec<P> = l.iter().rev().copied().collect();
        assert_eq!(area(&rev, &earclip(&rev).unwrap()), 12.0);
    }

    #[test]
    fn closed_rings_and_collinear_points_are_fine() {
        let r = [[0.0, 0.0], [2.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0], [0.0, 0.0]];
        assert_eq!(area(&r, &earclip(&r).unwrap()), 16.0);
    }

    #[test]
    fn degenerate_and_oversized_rings_decline() {
        assert!(earclip(&[[0.0, 0.0], [1.0, 1.0], [2.0, 2.0]]).is_none());
        assert!(earclip(&[[0.0, 0.0], [1.0, 0.0]]).is_none());
        let big: Vec<P> = (0..100).map(|i| [f64::from(i).cos(), f64::from(i).sin()]).collect();
        assert!(earclip(&big).is_none());
    }

    #[test]
    fn star_polygon_area_matches_shoelace() {
        let pts: Vec<P> = (0..20)
            .map(|k| {
                let a = std::f64::consts::TAU * f64::from(k) / 20.0;
                let r = if k % 2 == 0 { 10.0 } else { 4.0 };
                [r * a.cos(), r * a.sin()]
            })
            .collect();
        let t = earclip(&pts).unwrap();
        let shoelace: f64 = (0..20)
            .map(|i| pts[i][0] * pts[(i + 1) % 20][1] - pts[(i + 1) % 20][0] * pts[i][1])
            .sum::<f64>()
            .abs()
            / 2.0;
        assert!((area(&pts, &t) - shoelace).abs() < 1e-9);
        assert_eq!(t.len(), 18);
    }
}
