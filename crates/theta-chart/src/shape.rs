//! From rows to drawable series: grouping, aggregation, bucketing and category limits.
//!
//! Every limit here exists so that a chart of an arbitrary result stays legible and cheap, and each one
//! that trims data leaves a note for the user (`ChartData::notes`), never a silent cut.

use crate::spec::{Aggregate, ChartKind, ChartSpec, number};
use crate::time;
use std::collections::HashMap;
use theta_core::{ResultSet, TypeKind, Value};

/// Rows read from a result. Results are already bounded by the row limit; this is a backstop.
pub const MAX_ROWS: usize = 500_000;
/// Distinct x positions on a continuous axis before points are merged into equal-width buckets.
pub const MAX_POINTS: usize = 400;
/// Categories on a bar chart; the largest are kept.
pub const MAX_CATEGORIES: usize = 50;
/// Slices of a pie, "Other" included.
pub const MAX_SLICES: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XKind {
    /// Slots: `Point::x` is the index into `ChartData::categories`.
    Category,
    Number,
    /// Seconds since the epoch.
    Time,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub name: String,
    pub points: Vec<Point>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartData {
    pub kind: ChartKind,
    pub x_kind: XKind,
    pub x_title: String,
    pub y_title: String,
    pub categories: Vec<String>,
    pub series: Vec<Series>,
    pub x_range: (f64, f64),
    pub y_range: (f64, f64),
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChartError {
    /// A column index in the spec is out of range.
    BadColumn,
    NoY,
    /// No row had both an x and a number to plot; the string says what was skipped.
    NothingToPlot(String),
}

impl std::fmt::Display for ChartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChartError::BadColumn => f.write_str("A chosen column is not in this result."),
            ChartError::NoY => f.write_str("Choose at least one column for Y."),
            ChartError::NothingToPlot(why) => f.write_str(why),
        }
    }
}

#[derive(Clone, Copy)]
struct Acc {
    sum: f64,
    count: u64,
    min: f64,
    max: f64,
}

impl Acc {
    fn new() -> Acc {
        Acc { sum: 0.0, count: 0, min: f64::INFINITY, max: f64::NEG_INFINITY }
    }
    fn add(&mut self, v: f64) {
        self.sum += v;
        self.count += 1;
        self.min = self.min.min(v);
        self.max = self.max.max(v);
    }
    fn value(&self, agg: Aggregate) -> f64 {
        match agg {
            Aggregate::Sum => self.sum,
            Aggregate::Avg => self.sum / self.count as f64,
            Aggregate::Count => self.count as f64,
            Aggregate::Min => self.min,
            Aggregate::Max => self.max,
        }
    }
}

fn raw_x_kind(kind: TypeKind) -> XKind {
    match kind {
        TypeKind::Integer | TypeKind::Float | TypeKind::Decimal => XKind::Number,
        TypeKind::Date | TypeKind::Timestamp => XKind::Time,
        _ => XKind::Category,
    }
}

/// The text of a category cell: NULL is a category of its own and says so.
fn category_text(v: &Value) -> String {
    if v.is_null() { "(null)".into() } else { v.display() }
}

pub fn build(rs: &ResultSet, spec: &ChartSpec) -> Result<ChartData, ChartError> {
    let ncols = rs.columns.len();
    if spec.x >= ncols || spec.series.is_some_and(|s| s >= ncols) || spec.ys.iter().any(|&y| y >= ncols) {
        return Err(ChartError::BadColumn);
    }
    if spec.ys.is_empty() {
        return Err(ChartError::NoY);
    }
    let raw = raw_x_kind(rs.columns[spec.x].kind);
    // Bars and slices are slots; lines, areas and scatters want a real axis when X is a number or a time.
    let x_kind = match spec.kind {
        ChartKind::Bar | ChartKind::Pie => XKind::Category,
        _ => raw,
    };
    let aggregated = spec.kind != ChartKind::Scatter || x_kind == XKind::Category;
    let mut notes = Vec::new();
    let use_series = spec.series.filter(|_| spec.kind != ChartKind::Pie);
    if spec.series.is_some() && use_series.is_none() {
        notes.push("A pie chart shows one series; the series column is ignored.".to_string());
    }
    let ys: &[usize] = if spec.kind == ChartKind::Pie { &spec.ys[..1] } else { &spec.ys };
    if spec.kind == ChartKind::Pie && spec.ys.len() > 1 {
        notes.push(format!("A pie chart shows one column; showing “{}”.", rs.columns[ys[0]].name));
    }

    // Pass 1: category slots, series groups and the raw (x, y) pairs of each (series, y).
    let mut cat_index: HashMap<String, usize> = HashMap::new();
    let mut categories: Vec<String> = Vec::new();
    let mut cat_sort: Vec<f64> = Vec::new();
    let mut group_index: HashMap<String, usize> = HashMap::new();
    let mut groups: Vec<String> = Vec::new();
    let mut raw_pairs: Vec<Vec<(f64, f64)>> = Vec::new(); // [group * ys.len() + y]
    let (mut null_x, mut bad_x, mut bad_y) = (0usize, 0usize, vec![0usize; ys.len()]);
    for row in rs.rows.iter().take(MAX_ROWS) {
        let xv = &row[spec.x];
        if xv.is_null() && x_kind != XKind::Category {
            null_x += 1;
            continue;
        }
        let x = match x_kind {
            XKind::Category => {
                let text = category_text(xv);
                let next = categories.len();
                let i = *cat_index.entry(text.clone()).or_insert(next);
                if i == next {
                    cat_sort.push(match raw {
                        XKind::Number => number(xv).unwrap_or(f64::INFINITY),
                        XKind::Time => time::parse(&xv.display()).unwrap_or(f64::INFINITY),
                        XKind::Category => next as f64,
                    });
                    categories.push(text);
                }
                i as f64
            }
            XKind::Number => match number(xv) {
                Some(n) => n,
                None => {
                    bad_x += 1;
                    continue;
                }
            },
            XKind::Time => match time::parse(&xv.display()) {
                Some(t) => t,
                None => {
                    bad_x += 1;
                    continue;
                }
            },
        };
        let g = match use_series {
            Some(c) => {
                let name = category_text(&row[c]);
                let next = groups.len();
                let g = *group_index.entry(name.clone()).or_insert(next);
                if g == next {
                    groups.push(name);
                    raw_pairs.extend(ys.iter().map(|_| Vec::new()));
                }
                g
            }
            None => {
                if groups.is_empty() {
                    groups.push(String::new());
                    raw_pairs.extend(ys.iter().map(|_| Vec::new()));
                }
                0
            }
        };
        for (j, &yc) in ys.iter().enumerate() {
            let cell = &row[yc];
            if cell.is_null() {
                continue;
            }
            let y = if spec.aggregate == Aggregate::Count && aggregated {
                1.0
            } else {
                match number(cell) {
                    Some(n) => n,
                    None => {
                        bad_y[j] += 1;
                        continue;
                    }
                }
            };
            raw_pairs[g * ys.len() + j].push((x, y));
        }
    }
    if null_x > 0 {
        notes.push(format!("{null_x} rows with no value in “{}” were left out.", rs.columns[spec.x].name));
    }
    if bad_x > 0 {
        notes.push(format!(
            "{bad_x} rows whose “{}” could not be read as a {} were left out.",
            rs.columns[spec.x].name,
            if x_kind == XKind::Time { "date" } else { "number" }
        ));
    }
    for (j, n) in bad_y.iter().enumerate() {
        if *n > 0 {
            notes.push(format!("{n} rows with a non-numeric “{}” were left out.", rs.columns[ys[j]].name));
        }
    }
    if rs.truncated {
        notes.push(
            "The result was cut at the row limit; the chart shows the rows that were loaded.".to_string(),
        );
    }

    // Category order: numbers and dates sort by value, text keeps the order it first appeared in.
    let mut order: Vec<usize> = (0..categories.len()).collect();
    if raw != XKind::Category && x_kind == XKind::Category {
        order.sort_by(|&a, &b| cat_sort[a].total_cmp(&cat_sort[b]));
    }
    let mut slot_of = vec![0usize; categories.len()];
    for (slot, &orig) in order.iter().enumerate() {
        slot_of[orig] = slot;
    }
    let ordered_categories: Vec<String> = order.iter().map(|&o| categories[o].clone()).collect();

    // Pass 2: aggregate each (group, y) into points.
    let multi_y = ys.len() > 1;
    let mut series: Vec<Series> = Vec::new();
    let mut bucketed = 0usize;
    for (g, gname) in groups.iter().enumerate() {
        for (j, &yc) in ys.iter().enumerate() {
            let mut pairs = std::mem::take(&mut raw_pairs[g * ys.len() + j]);
            if pairs.is_empty() {
                continue;
            }
            if x_kind == XKind::Category {
                for p in &mut pairs {
                    p.0 = slot_of[p.0 as usize] as f64;
                }
            }
            pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
            let points = if aggregated {
                let continuous = x_kind != XKind::Category;
                let distinct = 1 + pairs.windows(2).filter(|w| w[0].0 != w[1].0).count();
                if continuous
                    && distinct > MAX_POINTS
                    && matches!(spec.kind, ChartKind::Line | ChartKind::Area)
                {
                    bucketed = bucketed.max(distinct);
                    bucket(&pairs, MAX_POINTS, spec.aggregate)
                } else {
                    group_equal_x(&pairs, spec.aggregate)
                }
            } else {
                pairs.iter().map(|&(x, y)| Point { x, y }).collect()
            };
            let name = match (use_series.is_some(), multi_y) {
                (true, true) => format!("{gname} · {}", rs.columns[yc].name),
                (true, false) => gname.clone(),
                (false, _) => rs.columns[yc].name.clone(),
            };
            series.push(Series { name, points });
        }
    }
    if bucketed > 0 {
        notes.push(format!(
            "{bucketed} distinct x values were merged into {MAX_POINTS} equal intervals ({}).",
            spec.aggregate.label().to_lowercase()
        ));
    }
    if series.is_empty() {
        let why = if rs.rows.is_empty() {
            "The result has no rows.".to_string()
        } else {
            format!(
                "Nothing to plot: no row has both a usable “{}” and a number in the chosen Y columns.",
                rs.columns[spec.x].name
            )
        };
        return Err(ChartError::NothingToPlot(why));
    }

    let mut categories = ordered_categories;
    if x_kind == XKind::Category {
        drop_unused_categories(&mut categories, &mut series);
    }
    if spec.kind == ChartKind::Bar && categories.len() > MAX_CATEGORIES {
        notes.push(format!("Showing the {MAX_CATEGORIES} largest of {} categories.", categories.len()));
        keep_largest(&mut categories, &mut series, MAX_CATEGORIES);
    }
    if spec.kind == ChartKind::Pie {
        let s = &mut series[0];
        let before = s.points.len();
        s.points.retain(|p| p.y > 0.0);
        if s.points.len() < before {
            notes.push(format!(
                "{} slices with a zero or negative value were left out.",
                before - s.points.len()
            ));
        }
        if s.points.is_empty() {
            return Err(ChartError::NothingToPlot("A pie chart needs positive values.".into()));
        }
        series.truncate(1);
        if s_len(&series) > MAX_SLICES {
            let adds_up = matches!(spec.aggregate, Aggregate::Sum | Aggregate::Count);
            let keep = if adds_up { MAX_SLICES - 1 } else { MAX_SLICES };
            let total_before = s_len(&series);
            fold_small_slices(&mut categories, &mut series[0], keep, adds_up);
            notes.push(if adds_up {
                format!(
                    "The {} smallest of {total_before} slices are combined as “Other”.",
                    total_before - keep
                )
            } else {
                format!("Showing the {keep} largest of {total_before} slices.")
            });
        }
    }

    let x_range = match x_kind {
        XKind::Category => (-0.5, categories.len().max(1) as f64 - 0.5),
        _ => {
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for p in series.iter().flat_map(|s| &s.points) {
                lo = lo.min(p.x);
                hi = hi.max(p.x);
            }
            (lo, hi)
        }
    };
    let (mut ylo, mut yhi) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in series.iter().flat_map(|s| &s.points) {
        ylo = ylo.min(p.y);
        yhi = yhi.max(p.y);
    }
    if matches!(spec.kind, ChartKind::Bar | ChartKind::Area) {
        ylo = ylo.min(0.0);
        yhi = yhi.max(0.0);
    }
    let agg_prefix = if aggregated { format!("{} of ", spec.aggregate.label()) } else { String::new() };
    let y_title = format!(
        "{agg_prefix}{}",
        ys.iter().map(|&y| rs.columns[y].name.as_str()).collect::<Vec<_>>().join(", ")
    );
    Ok(ChartData {
        kind: spec.kind,
        x_kind,
        x_title: rs.columns[spec.x].name.clone(),
        y_title,
        categories,
        series,
        x_range,
        y_range: (ylo, yhi),
        notes,
    })
}

/// A category whose every y was NULL has no point; it would be an empty slot, so it goes.
fn drop_unused_categories(categories: &mut Vec<String>, series: &mut [Series]) {
    let mut used = vec![false; categories.len()];
    for p in series.iter().flat_map(|s| &s.points) {
        used[p.x as usize] = true;
    }
    if used.iter().all(|u| *u) {
        return;
    }
    let mut remap = vec![0usize; categories.len()];
    let mut next = 0;
    for (i, u) in used.iter().enumerate() {
        if *u {
            remap[i] = next;
            next += 1;
        }
    }
    for p in series.iter_mut().flat_map(|s| &mut s.points) {
        p.x = remap[p.x as usize] as f64;
    }
    let mut i = 0;
    categories.retain(|_| {
        i += 1;
        used[i - 1]
    });
}

fn s_len(series: &[Series]) -> usize {
    series[0].points.len()
}

/// Collapse sorted `(x, y)` pairs that share an x into one point each.
fn group_equal_x(pairs: &[(f64, f64)], agg: Aggregate) -> Vec<Point> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < pairs.len() {
        let x = pairs[i].0;
        let mut acc = Acc::new();
        while i < pairs.len() && pairs[i].0 == x {
            acc.add(pairs[i].1);
            i += 1;
        }
        out.push(Point { x, y: acc.value(agg) });
    }
    out
}

/// Merge sorted pairs into `n` equal-width intervals over their x range. The new x is the mean of the x
/// values that fell in the interval, so a point stays where its data is rather than at a bin edge.
pub fn bucket(pairs: &[(f64, f64)], n: usize, agg: Aggregate) -> Vec<Point> {
    let (lo, hi) = (pairs[0].0, pairs[pairs.len() - 1].0);
    if hi <= lo || n == 0 {
        return group_equal_x(pairs, agg);
    }
    let width = (hi - lo) / n as f64;
    let mut accs = vec![(Acc::new(), 0.0f64); n];
    for &(x, y) in pairs {
        let i = (((x - lo) / width) as usize).min(n - 1);
        accs[i].0.add(y);
        accs[i].1 += x;
    }
    accs.into_iter()
        .filter(|(a, _)| a.count > 0)
        .map(|(a, xs)| Point { x: xs / a.count as f64, y: a.value(agg) })
        .collect()
}

/// Keep the `keep` categories with the largest absolute value and renumber the rest of the points.
fn keep_largest(categories: &mut Vec<String>, series: &mut [Series], keep: usize) {
    let mut score = vec![0.0f64; categories.len()];
    for p in series.iter().flat_map(|s| &s.points) {
        let slot = &mut score[p.x as usize];
        *slot = slot.max(p.y.abs());
    }
    let mut ranked: Vec<usize> = (0..categories.len()).collect();
    ranked.sort_by(|&a, &b| score[b].total_cmp(&score[a]).then(a.cmp(&b)));
    let mut kept: Vec<usize> = ranked.into_iter().take(keep).collect();
    kept.sort_unstable();
    let mut remap = vec![usize::MAX; categories.len()];
    for (new, &old) in kept.iter().enumerate() {
        remap[old] = new;
    }
    for s in series.iter_mut() {
        s.points.retain(|p| remap[p.x as usize] != usize::MAX);
        for p in &mut s.points {
            p.x = remap[p.x as usize] as f64;
        }
    }
    *categories = kept.iter().map(|&k| categories[k].clone()).collect();
}

/// Largest `keep` slices stay in their order; for sums and counts the rest become one "Other" slice.
fn fold_small_slices(categories: &mut Vec<String>, s: &mut Series, keep: usize, other: bool) {
    let mut ranked: Vec<usize> = (0..s.points.len()).collect();
    ranked.sort_by(|&a, &b| s.points[b].y.total_cmp(&s.points[a].y).then(a.cmp(&b)));
    let (big, small) = ranked.split_at(keep);
    let mut big = big.to_vec();
    big.sort_unstable();
    let rest: f64 = small.iter().map(|&i| s.points[i].y).sum();
    let mut names: Vec<String> = big.iter().map(|&i| categories[s.points[i].x as usize].clone()).collect();
    let mut points: Vec<Point> =
        big.iter().enumerate().map(|(n, &i)| Point { x: n as f64, y: s.points[i].y }).collect();
    if other {
        names.push("Other".into());
        points.push(Point { x: points.len() as f64, y: rest });
    }
    *categories = names;
    s.points = points;
}

#[cfg(test)]
mod tests {
    use super::*;
    use theta_core::Column;

    fn col(name: &str, kind: TypeKind) -> Column {
        Column { name: name.into(), type_name: String::new(), kind, origin: None }
    }

    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }

    fn sales() -> ResultSet {
        ResultSet {
            columns: vec![
                col("region", TypeKind::Text),
                col("kind", TypeKind::Text),
                col("amount", TypeKind::Integer),
            ],
            rows: vec![
                vec![t("north"), t("a"), Value::Int(10)],
                vec![t("south"), t("a"), Value::Int(5)],
                vec![t("north"), t("b"), Value::Int(30)],
                vec![t("north"), t("a"), Value::Int(20)],
                vec![t("south"), t("b"), Value::Null],
            ],
            truncated: false,
        }
    }

    fn spec(kind: ChartKind, agg: Aggregate) -> ChartSpec {
        ChartSpec { x: 0, ys: vec![2], series: None, kind, aggregate: agg }
    }

    fn ys(d: &ChartData, s: usize) -> Vec<f64> {
        d.series[s].points.iter().map(|p| p.y).collect()
    }

    #[test]
    fn repeated_x_values_are_aggregated() {
        let rs = sales();
        for (agg, north, south) in [
            (Aggregate::Sum, 60.0, 5.0),
            (Aggregate::Avg, 20.0, 5.0),
            (Aggregate::Count, 3.0, 1.0),
            (Aggregate::Min, 10.0, 5.0),
            (Aggregate::Max, 30.0, 5.0),
        ] {
            let d = build(&rs, &spec(ChartKind::Bar, agg)).unwrap();
            assert_eq!(d.categories, ["north", "south"], "{agg:?}");
            assert_eq!(ys(&d, 0), [north, south], "{agg:?}");
        }
        let d = build(&rs, &spec(ChartKind::Bar, Aggregate::Sum)).unwrap();
        assert_eq!(d.y_title, "Sum of amount");
        assert_eq!(d.x_range, (-0.5, 1.5));
        assert_eq!(d.y_range, (0.0, 60.0), "a bar chart always includes zero");
    }

    #[test]
    fn a_series_column_splits_every_y() {
        let mut s = spec(ChartKind::Bar, Aggregate::Sum);
        s.series = Some(1);
        let d = build(&sales(), &s).unwrap();
        let names: Vec<_> = d.series.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        // a: north 10 + 20, south 5; b: north 30 (south's NULL is skipped).
        assert_eq!(d.series[0].points, vec![Point { x: 0.0, y: 30.0 }, Point { x: 1.0, y: 5.0 }]);
        assert_eq!(d.series[1].points, vec![Point { x: 0.0, y: 30.0 }]);
    }

    #[test]
    fn several_ys_are_several_series_named_for_their_columns() {
        let rs = ResultSet {
            columns: vec![col("d", TypeKind::Text), col("a", TypeKind::Integer), col("b", TypeKind::Float)],
            rows: vec![vec![t("x"), Value::Int(1), Value::Float(2.5)]],
            truncated: false,
        };
        let s =
            ChartSpec { x: 0, ys: vec![1, 2], series: None, kind: ChartKind::Bar, aggregate: Aggregate::Sum };
        let d = build(&rs, &s).unwrap();
        assert_eq!(d.series.iter().map(|s| s.name.clone()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(d.y_title, "Sum of a, b");
    }

    #[test]
    fn dates_become_a_time_axis_sorted_by_time() {
        let rs = ResultSet {
            columns: vec![col("day", TypeKind::Date), col("n", TypeKind::Integer)],
            rows: vec![
                vec![Value::Date("2024-03-02".into()), Value::Int(2)],
                vec![Value::Date("2024-03-01".into()), Value::Int(1)],
                vec![Value::Date("2024-03-02".into()), Value::Int(3)],
            ],
            truncated: false,
        };
        let s =
            ChartSpec { x: 0, ys: vec![1], series: None, kind: ChartKind::Line, aggregate: Aggregate::Sum };
        let d = build(&rs, &s).unwrap();
        assert_eq!(d.x_kind, XKind::Time);
        assert_eq!(ys(&d, 0), [1.0, 5.0]);
        assert_eq!(d.series[0].points[1].x - d.series[0].points[0].x, 86_400.0);
        assert_eq!(d.x_range.1 - d.x_range.0, 86_400.0);
    }

    #[test]
    fn a_bar_chart_of_numbers_orders_the_bars_numerically() {
        let rs = ResultSet {
            columns: vec![col("n", TypeKind::Integer), col("v", TypeKind::Integer)],
            rows: vec![
                vec![Value::Int(10), Value::Int(1)],
                vec![Value::Int(9), Value::Int(2)],
                vec![Value::Int(100), Value::Int(3)],
            ],
            truncated: false,
        };
        let d = build(
            &rs,
            &ChartSpec { x: 0, ys: vec![1], series: None, kind: ChartKind::Bar, aggregate: Aggregate::Sum },
        )
        .unwrap();
        assert_eq!(d.categories, ["9", "10", "100"]);
        assert_eq!(ys(&d, 0), [2.0, 1.0, 3.0]);
    }

    #[test]
    fn scatter_keeps_every_row() {
        let rs = ResultSet {
            columns: vec![col("x", TypeKind::Float), col("y", TypeKind::Float)],
            rows: vec![
                vec![Value::Float(1.0), Value::Float(1.0)],
                vec![Value::Float(1.0), Value::Float(9.0)],
            ],
            truncated: false,
        };
        let s = ChartSpec {
            x: 0,
            ys: vec![1],
            series: None,
            kind: ChartKind::Scatter,
            aggregate: Aggregate::Sum,
        };
        let d = build(&rs, &s).unwrap();
        assert_eq!(d.series[0].points.len(), 2, "no aggregation: both points stay");
        assert_eq!(d.y_title, "y");
    }

    #[test]
    fn nulls_and_unreadable_cells_are_left_out_and_reported() {
        let rs = ResultSet {
            columns: vec![col("n", TypeKind::Integer), col("v", TypeKind::Text)],
            rows: vec![
                vec![Value::Int(1), Value::Text("abc".into())],
                vec![Value::Null, Value::Text("1".into())],
            ],
            truncated: true,
        };
        let e = build(
            &rs,
            &ChartSpec { x: 0, ys: vec![1], series: None, kind: ChartKind::Line, aggregate: Aggregate::Sum },
        )
        .unwrap_err();
        assert!(matches!(e, ChartError::NothingToPlot(_)), "{e:?}");
        let rs2 = ResultSet {
            columns: vec![col("n", TypeKind::Integer), col("v", TypeKind::Integer)],
            rows: vec![vec![Value::Int(1), Value::Int(5)], vec![Value::Null, Value::Int(6)]],
            truncated: true,
        };
        let d = build(
            &rs2,
            &ChartSpec { x: 0, ys: vec![1], series: None, kind: ChartKind::Line, aggregate: Aggregate::Sum },
        )
        .unwrap();
        assert!(d.notes.iter().any(|n| n.starts_with("1 rows with no value in “n”")), "{:?}", d.notes);
        assert!(d.notes.iter().any(|n| n.contains("row limit")));
    }

    #[test]
    fn count_works_on_a_text_column() {
        let rs = ResultSet {
            columns: vec![col("city", TypeKind::Text)],
            rows: vec![vec![t("a")], vec![t("b")], vec![t("a")], vec![Value::Null]],
            truncated: false,
        };
        let s =
            ChartSpec { x: 0, ys: vec![0], series: None, kind: ChartKind::Bar, aggregate: Aggregate::Count };
        let d = build(&rs, &s).unwrap();
        // The NULL row has no value to count, so it makes no bar (and leaves no empty slot).
        assert_eq!(d.categories, ["a", "b"]);
        assert_eq!(ys(&d, 0), [2.0, 1.0]);
    }

    #[test]
    fn bucketing_merges_crowded_points_and_says_so() {
        let rows: Vec<Vec<Value>> = (0..5000).map(|i| vec![Value::Int(i), Value::Int(1)]).collect();
        let rs = ResultSet {
            columns: vec![col("t", TypeKind::Integer), col("v", TypeKind::Integer)],
            rows,
            truncated: false,
        };
        let s =
            ChartSpec { x: 0, ys: vec![1], series: None, kind: ChartKind::Line, aggregate: Aggregate::Sum };
        let d = build(&rs, &s).unwrap();
        let pts = &d.series[0].points;
        assert_eq!(pts.len(), MAX_POINTS);
        assert_eq!(pts.iter().map(|p| p.y).sum::<f64>(), 5000.0, "a sum survives bucketing");
        assert!(pts.windows(2).all(|w| w[0].x < w[1].x));
        assert!(
            d.notes.iter().any(|n| n.contains("5000 distinct x values were merged into 400")),
            "{:?}",
            d.notes
        );
        let few =
            ResultSet { rows: (0..300).map(|i| vec![Value::Int(i), Value::Int(1)]).collect(), ..rs.clone() };
        assert_eq!(build(&few, &s).unwrap().series[0].points.len(), 300, "below the limit nothing is merged");
    }

    #[test]
    fn bucket_uses_the_mean_x_and_the_chosen_aggregate() {
        let pairs: Vec<(f64, f64)> = (0..10).map(|i| (i as f64, i as f64)).collect();
        let b = bucket(&pairs, 2, Aggregate::Max);
        assert_eq!(b, vec![Point { x: 2.0, y: 4.0 }, Point { x: 7.0, y: 9.0 }]);
        assert_eq!(bucket(&pairs, 2, Aggregate::Count).iter().map(|p| p.y).collect::<Vec<_>>(), [5.0, 5.0]);
    }

    #[test]
    fn long_bar_charts_keep_the_largest_categories_in_order() {
        let rows: Vec<Vec<Value>> = (0..80)
            .map(|i| vec![t(&format!("c{i:02}")), Value::Int(if i % 2 == 0 { 100 + i } else { i })])
            .collect();
        let rs = ResultSet {
            columns: vec![col("c", TypeKind::Text), col("v", TypeKind::Integer)],
            rows,
            truncated: false,
        };
        let d = build(
            &rs,
            &ChartSpec { x: 0, ys: vec![1], series: None, kind: ChartKind::Bar, aggregate: Aggregate::Sum },
        )
        .unwrap();
        assert_eq!(d.categories.len(), MAX_CATEGORIES);
        assert!(d.categories.windows(2).all(|w| w[0] < w[1]), "original order kept");
        assert!(
            d.categories
                .iter()
                .all(|c| c[1..].parse::<usize>().unwrap() % 2 == 0 || c[1..].parse::<usize>().unwrap() > 60)
        );
        assert!(d.series[0].points.iter().all(|p| (p.x as usize) < MAX_CATEGORIES));
        assert!(d.notes.iter().any(|n| n.contains("50 largest of 80")));
    }

    #[test]
    fn a_pie_combines_the_smallest_slices() {
        let rows: Vec<Vec<Value>> = (1..=20).map(|i| vec![t(&format!("s{i}")), Value::Int(i)]).collect();
        let rs = ResultSet {
            columns: vec![col("c", TypeKind::Text), col("v", TypeKind::Integer)],
            rows,
            truncated: false,
        };
        let d = build(
            &rs,
            &ChartSpec { x: 0, ys: vec![1], series: None, kind: ChartKind::Pie, aggregate: Aggregate::Sum },
        )
        .unwrap();
        assert_eq!(d.categories.len(), MAX_SLICES);
        assert_eq!(d.categories.last().unwrap(), "Other");
        let total: f64 = d.series[0].points.iter().map(|p| p.y).sum();
        assert_eq!(total, 210.0, "nothing is lost when slices are combined");
        assert_eq!(d.series[0].points.last().unwrap().y, (1..=9).sum::<i64>() as f64);
    }

    #[test]
    fn a_pie_drops_non_positive_values_and_refuses_when_none_are_left() {
        let rs = ResultSet {
            columns: vec![col("c", TypeKind::Text), col("v", TypeKind::Integer)],
            rows: vec![
                vec![t("a"), Value::Int(3)],
                vec![t("b"), Value::Int(-1)],
                vec![t("c"), Value::Int(0)],
            ],
            truncated: false,
        };
        let s =
            ChartSpec { x: 0, ys: vec![1], series: None, kind: ChartKind::Pie, aggregate: Aggregate::Sum };
        let d = build(&rs, &s).unwrap();
        assert_eq!(d.series[0].points.len(), 1);
        assert!(d.notes.iter().any(|n| n.contains("2 slices")));
        let neg = ResultSet { rows: vec![vec![t("b"), Value::Int(-1)]], ..rs };
        assert!(build(&neg, &s).is_err());
    }

    #[test]
    fn bad_specs_are_errors_not_panics() {
        let rs = sales();
        let mut s = spec(ChartKind::Bar, Aggregate::Sum);
        s.x = 9;
        assert_eq!(build(&rs, &s), Err(ChartError::BadColumn));
        s = spec(ChartKind::Bar, Aggregate::Sum);
        s.ys.clear();
        assert_eq!(build(&rs, &s), Err(ChartError::NoY));
        s = spec(ChartKind::Bar, Aggregate::Sum);
        s.series = Some(42);
        assert_eq!(build(&rs, &s), Err(ChartError::BadColumn));
        let empty = ResultSet { rows: vec![], ..sales() };
        assert!(matches!(
            build(&empty, &spec(ChartKind::Bar, Aggregate::Sum)),
            Err(ChartError::NothingToPlot(_))
        ));
    }

    #[test]
    fn line_x_ranges_cover_the_points_and_area_includes_zero() {
        let rs = ResultSet {
            columns: vec![col("x", TypeKind::Integer), col("y", TypeKind::Integer)],
            rows: vec![vec![Value::Int(10), Value::Int(5)], vec![Value::Int(20), Value::Int(8)]],
            truncated: false,
        };
        let line = build(
            &rs,
            &ChartSpec { x: 0, ys: vec![1], series: None, kind: ChartKind::Line, aggregate: Aggregate::Sum },
        )
        .unwrap();
        assert_eq!((line.x_range, line.y_range), ((10.0, 20.0), (5.0, 8.0)));
        let area = build(
            &rs,
            &ChartSpec { x: 0, ys: vec![1], series: None, kind: ChartKind::Area, aggregate: Aggregate::Sum },
        )
        .unwrap();
        assert_eq!(area.y_range, (0.0, 8.0));
    }
}
