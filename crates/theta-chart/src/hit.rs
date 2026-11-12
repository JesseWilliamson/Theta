//! Which data point is under the pointer.

use crate::shape::Point;

/// The index of the point whose x is closest to `x` (in the same units as the points). Points are
/// expected sorted by x, as [`crate::shape::build`] returns them for lines and areas; for scatter
/// charts use [`nearest_xy`].
pub fn nearest_x(points: &[Point], x: f64) -> Option<usize> {
    if points.is_empty() {
        return None;
    }
    let i = points.partition_point(|p| p.x < x);
    match (i.checked_sub(1), points.get(i)) {
        (Some(a), Some(b)) => Some(if (x - points[a].x) <= (b.x - x) { a } else { i }),
        (Some(a), None) => Some(a),
        (None, _) => Some(0),
    }
}

/// The point nearest to `(x, y)` in pixel space, if it is within `radius` pixels. The caller maps the
/// points to pixels first.
pub fn nearest_xy(pixels: &[(f64, f64)], x: f64, y: f64, radius: f64) -> Option<usize> {
    pixels
        .iter()
        .enumerate()
        .map(|(i, (px, py))| (i, (px - x).powi(2) + (py - y).powi(2)))
        .filter(|(_, d)| *d <= radius * radius)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(xs: &[f64]) -> Vec<Point> {
        xs.iter().map(|&x| Point { x, y: 0.0 }).collect()
    }

    #[test]
    fn the_closest_x_wins_and_ties_go_left() {
        let p = pts(&[0.0, 10.0, 20.0, 40.0]);
        assert_eq!(nearest_x(&p, -5.0), Some(0));
        assert_eq!(nearest_x(&p, 4.0), Some(0));
        assert_eq!(nearest_x(&p, 5.0), Some(0));
        assert_eq!(nearest_x(&p, 6.0), Some(1));
        assert_eq!(nearest_x(&p, 31.0), Some(3));
        assert_eq!(nearest_x(&p, 1e9), Some(3));
        assert_eq!(nearest_x(&[], 1.0), None);
    }

    #[test]
    fn scatter_hits_need_to_be_close() {
        let px = [(10.0, 10.0), (50.0, 50.0)];
        assert_eq!(nearest_xy(&px, 12.0, 11.0, 8.0), Some(0));
        assert_eq!(nearest_xy(&px, 30.0, 30.0, 8.0), None);
        assert_eq!(nearest_xy(&px, 49.0, 52.0, 8.0), Some(1));
    }
}
