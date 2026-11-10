//! Accumulating state of the Measure tool.

use quarry_geo::{Coord, Geometry, area_m2, format_area, format_distance, haversine_m};

/// How distances are computed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    /// Great-circle metres on lon/lat points.
    Geodesic,
    /// Euclidean distance in the data's own units.
    Planar,
}

/// The path the user is building.
#[derive(Clone, Debug)]
pub struct Measurement {
    /// How to measure.
    pub metric: Metric,
    /// Clicked points: lon/lat (geodesic) or world units (planar).
    pub points: Vec<Coord>,
    /// Closed by a double-click: the last edge back to the start counts and an area is shown.
    pub closed: bool,
}

impl Measurement {
    /// An empty measurement.
    pub fn new(metric: Metric) -> Self {
        Measurement { metric, points: Vec::new(), closed: false }
    }

    /// Add a vertex. Ignored once the shape is closed.
    pub fn push(&mut self, p: Coord) {
        if !self.closed {
            self.points.push(p);
        }
    }

    /// Close the ring (needs three vertices). A double-click also delivers two single
    /// clicks at the same spot, so duplicates of the last vertex are dropped first.
    pub fn close(&mut self) {
        while self.points.len() > 1
            && self.points[self.points.len() - 1] == self.points[self.points.len() - 2]
        {
            self.points.pop();
        }
        if self.points.len() >= 3 {
            self.closed = true;
        }
    }

    fn dist(&self, a: Coord, b: Coord) -> f64 {
        match self.metric {
            Metric::Geodesic => haversine_m(a, b),
            Metric::Planar => (a[0] - b[0]).hypot(a[1] - b[1]),
        }
    }

    /// Length of every edge in order, including the closing edge when closed.
    pub fn segments(&self) -> Vec<f64> {
        let mut out: Vec<f64> = self.points.windows(2).map(|w| self.dist(w[0], w[1])).collect();
        if self.closed {
            out.push(self.dist(self.points[self.points.len() - 1], self.points[0]));
        }
        out
    }

    /// Sum of [`Measurement::segments`].
    pub fn total(&self) -> f64 {
        self.segments().iter().sum()
    }

    /// Enclosed area (square metres or square units) once closed.
    pub fn area(&self) -> Option<f64> {
        if !self.closed || self.points.len() < 3 {
            return None;
        }
        Some(match self.metric {
            Metric::Geodesic => {
                let mut ring = self.points.clone();
                ring.push(self.points[0]);
                area_m2(&Geometry::Polygon(vec![ring]))
            }
            Metric::Planar => {
                let n = self.points.len();
                let twice: f64 = (0..n)
                    .map(|i| {
                        let (a, b) = (self.points[i], self.points[(i + 1) % n]);
                        a[0] * b[1] - b[0] * a[1]
                    })
                    .sum();
                twice.abs() / 2.0
            }
        })
    }

    /// Format one length for display.
    pub fn format_len(&self, v: f64) -> String {
        match self.metric {
            Metric::Geodesic => format_distance(v),
            Metric::Planar => format_units(v),
        }
    }

    /// Format an area for display.
    pub fn format_area(&self, v: f64) -> String {
        match self.metric {
            Metric::Geodesic => format_area(v),
            Metric::Planar => format!("{} sq. units", format_units(v)),
        }
    }
}

/// A plain-number rendering for planar lengths: 3 significant-ish decimals, no unit.
pub fn format_units(v: f64) -> String {
    let a = v.abs();
    if a >= 1000.0 {
        format!("{v:.0}")
    } else if a >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.3}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_segments_and_total() {
        let mut m = Measurement::new(Metric::Geodesic);
        m.push([0.0, 0.0]);
        m.push([0.0, 1.0]);
        m.push([1.0, 1.0]);
        let seg = m.segments();
        assert_eq!(seg.len(), 2);
        assert!((seg[0] - 111_195.0).abs() < 200.0, "{seg:?}");
        assert!((m.total() - seg[0] - seg[1]).abs() < 1e-6);
        assert!(m.area().is_none());
    }

    #[test]
    fn closing_adds_edge_and_area() {
        let mut m = Measurement::new(Metric::Geodesic);
        for p in [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]] {
            m.push(p);
        }
        m.close();
        assert!(m.closed);
        assert_eq!(m.segments().len(), 4);
        let area = m.area().unwrap();
        assert!((area - 12_364e6).abs() < 100e6, "{area}");
        m.push([5.0, 5.0]);
        assert_eq!(m.points.len(), 4, "closed shapes accept no more points");
    }

    #[test]
    fn double_click_duplicates_are_dropped() {
        let mut m = Measurement::new(Metric::Planar);
        for p in [[0.0, 0.0], [4.0, 0.0], [4.0, 3.0], [4.0, 3.0], [4.0, 3.0]] {
            m.push(p);
        }
        m.close();
        assert!(m.closed);
        assert_eq!(m.points.len(), 3);
        assert_eq!(m.segments(), vec![4.0, 3.0, 5.0]);
        assert_eq!(m.area(), Some(6.0));
    }

    #[test]
    fn cannot_close_with_two_points() {
        let mut m = Measurement::new(Metric::Planar);
        m.push([0.0, 0.0]);
        m.push([1.0, 1.0]);
        m.close();
        assert!(!m.closed);
        assert_eq!(m.area(), None);
    }

    #[test]
    fn unit_formatting() {
        assert_eq!(format_units(5.0), "5.000");
        assert_eq!(format_units(12.34), "12.3");
        assert_eq!(format_units(12345.6), "12346");
    }
}
