//! What the user asks for, and the defaults that make a chart appear before they ask for anything.

use theta_core::{Column, ResultSet, TypeKind, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChartKind {
    Bar,
    Line,
    Area,
    Scatter,
    Pie,
}

impl ChartKind {
    pub const ALL: [ChartKind; 5] =
        [ChartKind::Bar, ChartKind::Line, ChartKind::Area, ChartKind::Scatter, ChartKind::Pie];

    pub fn label(self) -> &'static str {
        match self {
            ChartKind::Bar => "Bar",
            ChartKind::Line => "Line",
            ChartKind::Area => "Area",
            ChartKind::Scatter => "Scatter",
            ChartKind::Pie => "Pie",
        }
    }
}

/// How the values of Y that share an X (and a series) become one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Aggregate {
    Sum,
    Avg,
    Count,
    Min,
    Max,
}

impl Aggregate {
    pub const ALL: [Aggregate; 5] =
        [Aggregate::Sum, Aggregate::Avg, Aggregate::Count, Aggregate::Min, Aggregate::Max];

    pub fn label(self) -> &'static str {
        match self {
            Aggregate::Sum => "Sum",
            Aggregate::Avg => "Average",
            Aggregate::Count => "Count",
            Aggregate::Min => "Min",
            Aggregate::Max => "Max",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartSpec {
    /// Column index of X.
    pub x: usize,
    /// Column indexes of Y, one series each. Empty is not drawable.
    pub ys: Vec<usize>,
    /// Column whose distinct values split every Y into one series each.
    pub series: Option<usize>,
    pub kind: ChartKind,
    pub aggregate: Aggregate,
}

/// Columns that can be a Y: numbers.
pub fn is_numeric(c: &Column) -> bool {
    matches!(c.kind, TypeKind::Integer | TypeKind::Float | TypeKind::Decimal)
}

pub fn is_temporal(c: &Column) -> bool {
    matches!(c.kind, TypeKind::Date | TypeKind::Timestamp)
}

/// Columns that can be an X or a series: everything with a short text form.
pub fn is_plottable(c: &Column) -> bool {
    !matches!(c.kind, TypeKind::Json | TypeKind::Bytes | TypeKind::Array | TypeKind::Geometry)
}

/// The most Y columns chosen by default; more lines than this stop being readable.
const DEFAULT_YS: usize = 3;

/// The chart to show before the user touches a control, from the column types alone (and, for numbers
/// on X, whether the rows are in ascending order). `None` when nothing in the result can be plotted.
pub fn suggest(rs: &ResultSet) -> Option<ChartSpec> {
    let cols = &rs.columns;
    let x = cols
        .iter()
        .position(is_temporal)
        .or_else(|| cols.iter().position(|c| is_plottable(c) && !is_numeric(c)))
        .or_else(|| cols.iter().position(is_numeric))?;
    let candidates: Vec<usize> = (0..cols.len()).filter(|&i| i != x && is_numeric(&cols[i])).collect();
    let ys = comparable_scale(rs, &candidates);
    if ys.is_empty() {
        // One column of categories: count how often each occurs. A lone number column has nothing to plot.
        return (!is_numeric(&cols[x])).then_some(ChartSpec {
            x,
            ys: vec![x],
            series: None,
            kind: ChartKind::Bar,
            aggregate: Aggregate::Count,
        });
    }
    let kind = if is_temporal(&cols[x]) {
        ChartKind::Line
    } else if is_numeric(&cols[x]) {
        if ascending_distinct(rs, x) { ChartKind::Line } else { ChartKind::Scatter }
    } else {
        ChartKind::Bar
    };
    Some(ChartSpec { x, ys, series: None, kind, aggregate: Aggregate::Sum })
}

/// Y columns worth sharing one axis: the first candidate, then the following ones whose largest value is
/// within a factor of `SCALE_RATIO` of it. Revenue in the thousands next to an order count in the tens would
/// squash the smaller line flat; it stays one click away in the Y chips.
fn comparable_scale(rs: &ResultSet, candidates: &[usize]) -> Vec<usize> {
    const SCALE_RATIO: f64 = 20.0;
    let peak = |col: usize| {
        rs.rows.iter().filter_map(|r| r.get(col).and_then(number)).fold(0.0f64, |m, v| m.max(v.abs()))
    };
    let mut out = Vec::new();
    let mut first = 0.0;
    for &c in candidates {
        let p = peak(c);
        if out.is_empty() {
            first = p;
            out.push(c);
        } else if out.len() < DEFAULT_YS {
            let ratio = p / first;
            if first == 0.0 || p == 0.0 || (1.0 / SCALE_RATIO..=SCALE_RATIO).contains(&ratio) {
                out.push(c);
            }
        }
    }
    out
}

/// Numbers that only grow from one row to the next are a sequence (a day number, an id): a line reads
/// them better than a cloud of points.
fn ascending_distinct(rs: &ResultSet, col: usize) -> bool {
    let mut prev: Option<f64> = None;
    for row in &rs.rows {
        let Some(v) = row.get(col).and_then(number) else { continue };
        if prev.is_some_and(|p| v <= p) {
            return false;
        }
        prev = Some(v);
    }
    true
}

/// A cell as a number, for the kinds that are numbers.
pub fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Int(i) => Some(*i as f64),
        Value::UInt(u) => Some(*u as f64),
        Value::Float(f) if f.is_finite() => Some(*f),
        Value::Decimal(s) => s.trim().parse::<f64>().ok().filter(|f| f.is_finite()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, kind: TypeKind) -> Column {
        Column { name: name.into(), type_name: format!("{kind:?}"), kind, origin: None }
    }

    fn rs(columns: Vec<Column>, rows: Vec<Vec<Value>>) -> ResultSet {
        ResultSet { columns, rows, truncated: false }
    }

    #[test]
    fn a_text_and_a_number_make_a_bar_chart() {
        let r = rs(
            vec![col("country", TypeKind::Text), col("n", TypeKind::Integer)],
            vec![vec![Value::Text("SE".into()), Value::Int(3)]],
        );
        let s = suggest(&r).unwrap();
        assert_eq!((s.x, s.ys.clone(), s.kind, s.aggregate), (0, vec![1], ChartKind::Bar, Aggregate::Sum));
    }

    #[test]
    fn a_date_makes_a_line_chart_and_the_date_is_the_x_wherever_it_is() {
        let r = rs(
            vec![
                col("revenue", TypeKind::Decimal),
                col("day", TypeKind::Date),
                col("orders", TypeKind::Integer),
            ],
            vec![],
        );
        let s = suggest(&r).unwrap();
        assert_eq!((s.x, s.ys, s.kind), (1, vec![0, 2], ChartKind::Line));
    }

    #[test]
    fn two_numbers_are_a_scatter_unless_x_is_a_sequence() {
        let cols = vec![col("a", TypeKind::Float), col("b", TypeKind::Float)];
        let cloud = rs(
            cols.clone(),
            vec![vec![Value::Float(3.0), Value::Float(1.0)], vec![Value::Float(1.0), Value::Float(2.0)]],
        );
        assert_eq!(suggest(&cloud).unwrap().kind, ChartKind::Scatter);
        let seq =
            rs(cols, vec![vec![Value::Int(1), Value::Float(1.0)], vec![Value::Int(2), Value::Float(2.0)]]);
        assert_eq!(suggest(&seq).unwrap().kind, ChartKind::Line);
    }

    #[test]
    fn a_lone_category_column_is_counted() {
        let r = rs(vec![col("city", TypeKind::Text)], vec![]);
        let s = suggest(&r).unwrap();
        assert_eq!((s.x, s.ys, s.kind, s.aggregate), (0, vec![0], ChartKind::Bar, Aggregate::Count));
    }

    #[test]
    fn nothing_plottable_gives_none() {
        assert!(suggest(&rs(vec![col("n", TypeKind::Integer)], vec![])).is_none());
        assert!(suggest(&rs(vec![col("j", TypeKind::Json), col("b", TypeKind::Bytes)], vec![])).is_none());
        assert!(suggest(&rs(vec![], vec![])).is_none());
    }

    #[test]
    fn a_column_on_a_very_different_scale_is_not_charted_with_the_first_by_default() {
        let r = rs(
            vec![
                col("day", TypeKind::Date),
                col("revenue", TypeKind::Decimal),
                col("orders", TypeKind::Integer),
                col("profit", TypeKind::Decimal),
            ],
            vec![
                vec![
                    Value::Date("2024-01-01".into()),
                    Value::Decimal("9000.50".into()),
                    Value::Int(30),
                    Value::Decimal("4000".into()),
                ],
                vec![
                    Value::Date("2024-01-02".into()),
                    Value::Decimal("12000".into()),
                    Value::Int(41),
                    Value::Decimal("5100".into()),
                ],
            ],
        );
        assert_eq!(suggest(&r).unwrap().ys, vec![1, 3], "orders is 300 times smaller than revenue");
    }

    #[test]
    fn at_most_three_y_columns_are_chosen() {
        let r = rs(
            (0..6)
                .map(|i| col(&format!("c{i}"), if i == 0 { TypeKind::Text } else { TypeKind::Integer }))
                .collect(),
            vec![],
        );
        assert_eq!(suggest(&r).unwrap().ys, vec![1, 2, 3]);
    }

    #[test]
    fn numbers_come_from_numeric_values_only() {
        assert_eq!(number(&Value::Decimal(" 12.50 ".into())), Some(12.5));
        assert_eq!(number(&Value::Float(f64::NAN)), None);
        assert_eq!(number(&Value::Text("5".into())), None);
        assert_eq!(number(&Value::Null), None);
    }
}
