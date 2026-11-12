//! Geometry of a chart: where the axes, ticks, bars, points and slices are in pixels, which mark is under
//! the pointer, and what the readout says. Everything the painter needs, computed without a window, so each
//! rule (margins, scales, bar widths, slice angles, hit testing) is tested here.

use crate::hit::{nearest_x, nearest_xy};
use crate::scale::{Band, Linear};
use crate::shape::{ChartData, Point, XKind};
use crate::spec::ChartKind;
use crate::ticks::{self, format_exact, format_number};
use crate::time;

/// Room for the y labels, the x labels and a little air, in pixels.
pub const MARGIN_LEFT: f64 = 58.0;
pub const MARGIN_RIGHT: f64 = 16.0;
pub const MARGIN_TOP: f64 = 12.0;
pub const MARGIN_BOTTOM: f64 = 28.0;
/// Pointer distance within which a scatter point is "under" it.
pub const HIT_RADIUS: f64 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum XScale {
    Band(Band),
    Linear(Linear),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
    /// Pixel position along the axis.
    pub at: f64,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Slice {
    /// Radians, clockwise from twelve o'clock.
    pub start: f64,
    pub end: f64,
    pub point: usize,
    pub fraction: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub plot: Rect,
    pub x: XScale,
    pub y: Linear,
    pub x_ticks: Vec<Tick>,
    pub y_ticks: Vec<Tick>,
    /// Pixel y of the value 0 (bars and areas grow from it), clamped into the plot.
    pub baseline: f64,
    /// Pie only: centre, radius and the slices.
    pub pie: Option<(f64, f64, f64)>,
    pub slices: Vec<Slice>,
}

/// A mark under the pointer or chosen with the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub series: usize,
    pub point: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Readout {
    pub title: String,
    /// `(series index, series name, formatted value)`.
    pub rows: Vec<(usize, String, String)>,
    /// Pixel position the readout points at.
    pub anchor: (f64, f64),
}

impl Layout {
    pub fn new(data: &ChartData, width: f64, height: f64) -> Layout {
        let plot = Rect {
            x: MARGIN_LEFT,
            y: MARGIN_TOP,
            w: (width - MARGIN_LEFT - MARGIN_RIGHT).max(1.0),
            h: (height - MARGIN_TOP - MARGIN_BOTTOM).max(1.0),
        };
        let y_axis = ticks::linear(data.y_range.0, data.y_range.1, ((plot.h / 42.0) as usize).clamp(2, 10));
        let y = Linear::new(y_axis.min, y_axis.max, plot.y + plot.h, plot.y);
        let y_ticks = y_axis
            .ticks
            .iter()
            .map(|&v| Tick { at: y.map(v), label: format_number(v, y_axis.step) })
            .collect();
        let baseline = y.map(0.0).clamp(plot.y, plot.y + plot.h);

        let target_x = ((plot.w / 90.0) as usize).clamp(2, 12);
        let (x, x_ticks) = match data.x_kind {
            XKind::Category => {
                let band = Band { n: data.categories.len(), r0: plot.x, r1: plot.x + plot.w };
                // Show every k-th label so they do not overlap (about 64 px each).
                let every =
                    (data.categories.len() as f64 / (plot.w / 64.0).max(1.0)).ceil().max(1.0) as usize;
                let t = data
                    .categories
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| i % every == 0)
                    .map(|(i, c)| Tick { at: band.centre(i), label: c.clone() })
                    .collect();
                (XScale::Band(band), t)
            }
            XKind::Number => {
                let ax = ticks::linear(data.x_range.0, data.x_range.1, target_x);
                let s = Linear::new(ax.min, ax.max, plot.x, plot.x + plot.w);
                let t = ax
                    .ticks
                    .iter()
                    .map(|&v| Tick { at: s.map(v), label: format_number(v, ax.step) })
                    .collect();
                (XScale::Linear(s), t)
            }
            XKind::Time => {
                let ax = ticks::time_axis(data.x_range.0, data.x_range.1, target_x);
                let s = Linear::new(ax.min, ax.max, plot.x, plot.x + plot.w);
                let t = ax
                    .ticks
                    .iter()
                    .map(|&v| Tick { at: s.map(v), label: time::format(v, ax.resolution) })
                    .collect();
                (XScale::Linear(s), t)
            }
        };

        let (mut pie, mut slices) = (None, Vec::new());
        if data.kind == ChartKind::Pie {
            let r = (plot.w.min(plot.h) / 2.0 - 6.0).max(4.0);
            pie = Some((plot.x + plot.w / 2.0, plot.y + plot.h / 2.0, r));
            let total: f64 = data.series[0].points.iter().map(|p| p.y).sum();
            let mut a = 0.0;
            for (i, p) in data.series[0].points.iter().enumerate() {
                let f = if total > 0.0 { p.y / total } else { 0.0 };
                slices.push(Slice { start: a, end: a + f * std::f64::consts::TAU, point: i, fraction: f });
                a += f * std::f64::consts::TAU;
            }
        }
        Layout { plot, x, y, x_ticks, y_ticks, baseline, pie, slices }
    }

    /// Pixel position of a data point (not meaningful for a pie).
    pub fn point_px(&self, p: &Point) -> (f64, f64) {
        let x = match &self.x {
            XScale::Band(b) => b.centre(p.x as usize),
            XScale::Linear(l) => l.map(p.x),
        };
        (x, self.y.map(p.y))
    }

    /// The rectangle of a bar: series `s` of `n` in the slot of point `p`. Bars fill 80% of a slot, side
    /// by side, and grow from the zero line.
    pub fn bar_rect(&self, s: usize, n: usize, p: &Point) -> Rect {
        let XScale::Band(b) = &self.x else { return Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 } };
        let slot = b.width();
        let group = slot * 0.8;
        let each = group / n.max(1) as f64;
        let x = b.start(p.x as usize) + (slot - group) / 2.0 + each * s as f64;
        let top = self.y.map(p.y);
        let (y0, y1) = if top <= self.baseline { (top, self.baseline) } else { (self.baseline, top) };
        Rect { x: x + 0.5, y: y0, w: (each - 1.0).max(1.0), h: (y1 - y0).max(1.0) }
    }

    /// What is under the pointer at `(px, py)`.
    pub fn hit(&self, data: &ChartData, px: f64, py: f64) -> Option<Target> {
        if let Some((cx, cy, r)) = self.pie {
            let (dx, dy) = (px - cx, py - cy);
            if dx * dx + dy * dy > r * r {
                return None;
            }
            // Clockwise from twelve o'clock, 0..TAU.
            let a = dy.atan2(dx) + std::f64::consts::FRAC_PI_2;
            let a = a.rem_euclid(std::f64::consts::TAU);
            return self
                .slices
                .iter()
                .find(|s| a >= s.start && a < s.end)
                .map(|s| Target { series: 0, point: s.point });
        }
        if !self.plot.contains(px, py) {
            return None;
        }
        match (data.kind, &self.x) {
            (ChartKind::Scatter, _) => {
                let mut flat = Vec::new();
                let mut ids = Vec::new();
                for (si, s) in data.series.iter().enumerate() {
                    for (pi, p) in s.points.iter().enumerate() {
                        flat.push(self.point_px(p));
                        ids.push(Target { series: si, point: pi });
                    }
                }
                nearest_xy(&flat, px, py, HIT_RADIUS).map(|i| ids[i])
            }
            (_, XScale::Band(b)) => {
                let slot = b.index_at(px)? as f64;
                // The series with a point in this slot and nearest to the pointer horizontally.
                let n = data.series.len();
                let mut best: Option<(f64, Target)> = None;
                for (si, s) in data.series.iter().enumerate() {
                    if let Some(pi) = s.points.iter().position(|p| p.x == slot) {
                        let centre = if data.kind == ChartKind::Bar {
                            let r = self.bar_rect(si, n, &s.points[pi]);
                            r.x + r.w / 2.0
                        } else {
                            self.point_px(&s.points[pi]).0
                        };
                        let d = (centre - px).abs();
                        if best.is_none_or(|(bd, _)| d < bd) {
                            best = Some((d, Target { series: si, point: pi }));
                        }
                    }
                }
                best.map(|(_, t)| t)
            }
            (_, XScale::Linear(l)) => {
                let x = l.invert(px);
                let mut best: Option<(f64, Target)> = None;
                for (si, s) in data.series.iter().enumerate() {
                    if let Some(pi) = nearest_x(&s.points, x) {
                        let d = (s.points[pi].x - x).abs();
                        if best.is_none_or(|(bd, _)| d < bd) {
                            best = Some((d, Target { series: si, point: pi }));
                        }
                    }
                }
                best.map(|(_, t)| t)
            }
        }
    }

    /// The text of the readout for a target: the x, and the value of every series at that x.
    pub fn readout(&self, data: &ChartData, t: Target) -> Option<Readout> {
        let p = *data.series.get(t.series)?.points.get(t.point)?;
        let fmt = |v: f64| format_exact(v);
        if data.kind == ChartKind::Pie {
            let total: f64 = data.series[0].points.iter().map(|q| q.y).sum();
            let name = data.categories.get(p.x as usize).cloned().unwrap_or_default();
            let pct = if total > 0.0 { p.y / total * 100.0 } else { 0.0 };
            let slice = self.slices.iter().find(|s| s.point == t.point)?;
            let mid = (slice.start + slice.end) / 2.0;
            let (cx, cy, r) = self.pie?;
            let anchor = (
                cx + (r * 0.6) * (mid - std::f64::consts::FRAC_PI_2).cos(),
                cy + (r * 0.6) * (mid - std::f64::consts::FRAC_PI_2).sin(),
            );
            return Some(Readout {
                title: name,
                rows: vec![(t.point, data.y_title.clone(), format!("{} ({pct:.1}%)", fmt(p.y)))],
                anchor,
            });
        }
        let title = Self::x_label(data, p.x);
        let mut rows = Vec::new();
        let same_x = |q: &Point| match data.x_kind {
            XKind::Category => q.x == p.x,
            _ => data.kind != ChartKind::Scatter && q.x == p.x,
        };
        if data.kind == ChartKind::Scatter {
            rows.push((t.series, data.series[t.series].name.clone(), fmt(p.y)));
        } else {
            for (si, s) in data.series.iter().enumerate() {
                // Continuous series rarely share an exact x; show each series' nearest point's y.
                let q = if data.x_kind == XKind::Category {
                    s.points.iter().find(|q| same_x(q))
                } else {
                    nearest_x(&s.points, p.x).and_then(|i| s.points.get(i))
                };
                if let Some(q) = q {
                    rows.push((si, s.name.clone(), fmt(q.y)));
                }
            }
        }
        Some(Readout { title, rows, anchor: self.point_px(&p) })
    }

    /// A data x as text: the category, a date, or a number.
    pub fn x_label(data: &ChartData, x: f64) -> String {
        match data.x_kind {
            XKind::Category => data.categories.get(x as usize).cloned().unwrap_or_default(),
            XKind::Time => time::format_full(x),
            XKind::Number => format_exact(x),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::build;
    use crate::spec::{Aggregate, ChartSpec};
    use theta_core::{Column, ResultSet, TypeKind, Value};

    fn col(n: &str, k: TypeKind) -> Column {
        Column { name: n.into(), type_name: String::new(), kind: k, origin: None }
    }

    fn data(kind: ChartKind) -> ChartData {
        let rs = ResultSet {
            columns: vec![col("c", TypeKind::Text), col("a", TypeKind::Integer), col("b", TypeKind::Integer)],
            rows: vec![
                vec![Value::Text("x".into()), Value::Int(10), Value::Int(5)],
                vec![Value::Text("y".into()), Value::Int(20), Value::Int(15)],
                vec![Value::Text("z".into()), Value::Int(30), Value::Int(25)],
                vec![Value::Text("w".into()), Value::Int(40), Value::Int(35)],
            ],
            truncated: false,
        };
        build(&rs, &ChartSpec { x: 0, ys: vec![1, 2], series: None, kind, aggregate: Aggregate::Sum })
            .unwrap()
    }

    #[test]
    fn the_plot_area_leaves_room_for_the_labels() {
        let l = Layout::new(&data(ChartKind::Bar), 700.0, 300.0);
        assert_eq!(
            l.plot,
            Rect {
                x: MARGIN_LEFT,
                y: MARGIN_TOP,
                w: 700.0 - MARGIN_LEFT - MARGIN_RIGHT,
                h: 300.0 - MARGIN_TOP - MARGIN_BOTTOM
            }
        );
        let tiny = Layout::new(&data(ChartKind::Bar), 10.0, 10.0);
        assert!(tiny.plot.w >= 1.0 && tiny.plot.h >= 1.0, "never a negative size");
    }

    #[test]
    fn y_ticks_are_in_pixel_order_and_zero_is_the_baseline() {
        let d = data(ChartKind::Bar);
        let l = Layout::new(&d, 700.0, 300.0);
        assert!(l.y_ticks.windows(2).all(|w| w[0].at > w[1].at), "larger values are higher up");
        assert_eq!(l.y_ticks[0].label, "0");
        assert!((l.baseline - l.y.map(0.0)).abs() < 1e-9);
        assert!(l.y_ticks.iter().all(|t| t.at >= l.plot.y - 1e-9 && t.at <= l.plot.y + l.plot.h + 1e-9));
    }

    #[test]
    fn bars_sit_side_by_side_in_their_slot_and_stand_on_the_baseline() {
        let d = data(ChartKind::Bar);
        let l = Layout::new(&d, 700.0, 300.0);
        let XScale::Band(b) = &l.x else { panic!() };
        let a = l.bar_rect(0, 2, &d.series[0].points[1]);
        let c = l.bar_rect(1, 2, &d.series[1].points[1]);
        assert!(a.x >= b.start(1) && c.x + c.w <= b.start(2), "inside the slot");
        assert!(a.x + a.w <= c.x + 1.0, "side by side: {a:?} {c:?}");
        assert!((a.y + a.h - l.baseline).abs() < 1.5, "the bar stands on the zero line");
        assert!(a.h > c.h, "20 is taller than 15");
    }

    #[test]
    fn the_pointer_finds_the_bar_slot_and_the_nearest_series_in_it() {
        let d = data(ChartKind::Bar);
        let l = Layout::new(&d, 700.0, 300.0);
        let r0 = l.bar_rect(0, 2, &d.series[0].points[2]);
        let r1 = l.bar_rect(1, 2, &d.series[1].points[2]);
        let t = l.hit(&d, r0.x + r0.w / 2.0, l.plot.y + 5.0).unwrap();
        assert_eq!(t, Target { series: 0, point: 2 });
        let t = l.hit(&d, r1.x + r1.w / 2.0, l.plot.y + 5.0).unwrap();
        assert_eq!(t, Target { series: 1, point: 2 });
        assert_eq!(l.hit(&d, 1.0, 1.0), None, "outside the plot");
    }

    #[test]
    fn the_readout_lists_every_series_at_the_pointed_x() {
        let d = data(ChartKind::Bar);
        let l = Layout::new(&d, 700.0, 300.0);
        let r = l.readout(&d, Target { series: 0, point: 1 }).unwrap();
        assert_eq!(r.title, "y");
        assert_eq!(r.rows, vec![(0, "a".into(), "20".into()), (1, "b".into(), "15".into())]);
        assert!(l.readout(&d, Target { series: 9, point: 0 }).is_none());
    }

    #[test]
    fn a_line_chart_hits_the_nearest_x_on_a_number_axis() {
        let rs = ResultSet {
            columns: vec![col("x", TypeKind::Integer), col("y", TypeKind::Integer)],
            rows: (0..=10).map(|i| vec![Value::Int(i * 10), Value::Int(i)]).collect(),
            truncated: false,
        };
        let d = build(
            &rs,
            &ChartSpec { x: 0, ys: vec![1], series: None, kind: ChartKind::Line, aggregate: Aggregate::Sum },
        )
        .unwrap();
        let l = Layout::new(&d, 600.0, 300.0);
        let p = d.series[0].points[4];
        let (px, py) = l.point_px(&p);
        assert_eq!(l.hit(&d, px + 3.0, py).unwrap(), Target { series: 0, point: 4 });
        let r = l.readout(&d, Target { series: 0, point: 4 }).unwrap();
        assert_eq!((r.title.as_str(), r.rows[0].2.as_str()), ("40", "4"));
        assert!(l.x_ticks.len() >= 3);
    }

    #[test]
    fn a_time_axis_reads_out_full_dates() {
        let rs = ResultSet {
            columns: vec![col("d", TypeKind::Date), col("n", TypeKind::Integer)],
            rows: vec![
                vec![Value::Date("2024-03-01".into()), Value::Int(1)],
                vec![Value::Date("2024-03-20".into()), Value::Int(2)],
            ],
            truncated: false,
        };
        let d = build(
            &rs,
            &ChartSpec { x: 0, ys: vec![1], series: None, kind: ChartKind::Line, aggregate: Aggregate::Sum },
        )
        .unwrap();
        let l = Layout::new(&d, 600.0, 300.0);
        assert_eq!(l.readout(&d, Target { series: 0, point: 1 }).unwrap().title, "2024-03-20");
        assert!(
            l.x_ticks
                .iter()
                .all(|t| t.label.contains("Mar") || t.label.contains("Feb") || t.label.contains("Apr")),
            "{:?}",
            l.x_ticks
        );
    }

    #[test]
    fn scatter_hits_need_the_pointer_near_a_point() {
        let rs = ResultSet {
            columns: vec![col("x", TypeKind::Float), col("y", TypeKind::Float)],
            rows: vec![
                vec![Value::Float(1.0), Value::Float(1.0)],
                vec![Value::Float(9.0), Value::Float(9.0)],
            ],
            truncated: false,
        };
        let d = build(
            &rs,
            &ChartSpec {
                x: 0,
                ys: vec![1],
                series: None,
                kind: ChartKind::Scatter,
                aggregate: Aggregate::Sum,
            },
        )
        .unwrap();
        let l = Layout::new(&d, 600.0, 300.0);
        let (px, py) = l.point_px(&d.series[0].points[1]);
        assert_eq!(l.hit(&d, px + 4.0, py - 3.0).unwrap(), Target { series: 0, point: 1 });
        assert_eq!(l.hit(&d, l.plot.x + l.plot.w / 2.0, l.plot.y + l.plot.h / 2.0), None);
    }

    #[test]
    fn pie_slices_fill_the_circle_and_the_pointer_finds_them() {
        let d = data(ChartKind::Pie);
        let l = Layout::new(&d, 400.0, 300.0);
        let total: f64 = l.slices.iter().map(|s| s.end - s.start).sum();
        assert!((total - std::f64::consts::TAU).abs() < 1e-9);
        assert!((l.slices.iter().map(|s| s.fraction).sum::<f64>() - 1.0).abs() < 1e-9);
        let (cx, cy, r) = l.pie.unwrap();
        // 10 of 100 is the first slice, which starts at twelve o'clock and runs clockwise: just right of the top.
        let t = l.hit(&d, cx + 2.0, cy - r * 0.5).unwrap();
        assert_eq!(t.point, 0);
        // The last slice (40%) ends at twelve o'clock: just left of the top.
        let t = l.hit(&d, cx - 2.0, cy - r * 0.5).unwrap();
        assert_eq!(t.point, 3);
        assert_eq!(l.hit(&d, cx + r + 20.0, cy), None);
        let ro = l.readout(&d, Target { series: 0, point: 3 }).unwrap();
        assert_eq!((ro.title.as_str(), ro.rows[0].2.as_str()), ("w", "40 (40.0%)"));
    }

    #[test]
    fn many_categories_thin_out_their_labels() {
        let rs = ResultSet {
            columns: vec![col("c", TypeKind::Text), col("v", TypeKind::Integer)],
            rows: (0..40).map(|i| vec![Value::Text(format!("category {i}")), Value::Int(i)]).collect(),
            truncated: false,
        };
        let d = build(
            &rs,
            &ChartSpec { x: 0, ys: vec![1], series: None, kind: ChartKind::Bar, aggregate: Aggregate::Sum },
        )
        .unwrap();
        let l = Layout::new(&d, 600.0, 300.0);
        assert!(l.x_ticks.len() < 40 && l.x_ticks.len() >= 4, "{}", l.x_ticks.len());
        let gaps: Vec<f64> = l.x_ticks.windows(2).map(|w| w[1].at - w[0].at).collect();
        assert!(gaps.iter().all(|g| *g >= 60.0), "labels are at least ~64 px apart: {gaps:?}");
    }
}
