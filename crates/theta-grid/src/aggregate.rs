//! Full statistics of a selection, shown in the aggregates popover. Pure, so it can run on a
//! background thread and be unit-tested.
//!
//! The footer's one-line summary (`summary.rs`) stays what it was: sum and average for an all-numeric
//! selection, cheap enough for every click. This is the detailed view behind it.

use crate::compare::compare_values;
use crate::format::{format_count, format_number, raw_text};
use std::cmp::Ordering;
use std::collections::HashSet;
use theta_core::Value;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    /// Cells inspected, NULLs included.
    pub cells: usize,
    pub nulls: usize,
    /// Non-NULL cells.
    pub count: usize,
    pub distinct: usize,
    pub min: Option<String>,
    pub max: Option<String>,
    /// Present only when every non-NULL cell is a number.
    pub sum: Option<f64>,
    pub avg: Option<f64>,
    /// Shortest and longest text value, in characters, when the selection holds text.
    pub text_len: Option<(usize, usize)>,
}

fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Int(i) => Some(*i as f64),
        Value::UInt(u) => Some(*u as f64),
        Value::Float(f) if f.is_finite() => Some(*f),
        Value::Decimal(s) => s.trim().parse::<f64>().ok().filter(|f| f.is_finite()),
        _ => None,
    }
}

fn is_text(v: &Value) -> bool {
    matches!(v, Value::Text(_) | Value::Json(_) | Value::Uuid(_) | Value::Unknown(_))
}

/// Long values are shortened for display; the full text would make a popover unreadable.
fn shown(v: &Value) -> String {
    let text = match v {
        Value::Bytes(b) => format!("{} bytes", format_count(b.len())),
        Value::Geometry(_) => crate::format::cell_text(v).text,
        other => raw_text(other),
    };
    let one_line = crate::format::single_line(&text);
    if one_line.chars().count() > 60 {
        format!("{}…", one_line.chars().take(60).collect::<String>())
    } else {
        one_line
    }
}

pub fn compute<'a>(values: impl Iterator<Item = &'a Value>) -> Stats {
    let mut s = Stats::default();
    let mut seen: HashSet<(u8, String)> = HashSet::new();
    let (mut min, mut max): (Option<&Value>, Option<&Value>) = (None, None);
    let mut sum = 0.0;
    let mut numeric = true;
    let mut len: Option<(usize, usize)> = None;
    for v in values {
        s.cells += 1;
        if v.is_null() {
            s.nulls += 1;
            continue;
        }
        s.count += 1;
        // The variant is part of the key so that the text `1` and the number 1 are two values.
        let tag = if v.is_numeric() { 0 } else { 1 };
        seen.insert((tag, raw_text(v)));
        match min {
            Some(m) if compare_values(v, m) != Ordering::Less => {}
            _ => min = Some(v),
        }
        match max {
            Some(m) if compare_values(v, m) != Ordering::Greater => {}
            _ => max = Some(v),
        }
        match as_f64(v) {
            Some(f) => sum += f,
            None => numeric = false,
        }
        if is_text(v)
            && let Value::Text(t) | Value::Json(t) | Value::Uuid(t) | Value::Unknown(t) = v
        {
            let n = t.chars().count();
            len = Some(len.map_or((n, n), |(lo, hi)| (lo.min(n), hi.max(n))));
        }
    }
    s.distinct = seen.len();
    s.min = min.map(shown);
    s.max = max.map(shown);
    if numeric && s.count > 0 {
        s.sum = Some(sum);
        s.avg = Some(sum / s.count as f64);
    }
    s.text_len = len;
    s
}

impl Stats {
    /// Label and value of each line, in display order; also what the copy button puts on the clipboard.
    pub fn lines(&self) -> Vec<(&'static str, String)> {
        let mut out = vec![
            ("Cells", format_count(self.cells)),
            ("Count", format_count(self.count)),
            ("Distinct", format_count(self.distinct)),
            ("Nulls", format_count(self.nulls)),
        ];
        if let Some(m) = &self.min {
            out.push(("Min", m.clone()));
        }
        if let Some(m) = &self.max {
            out.push(("Max", m.clone()));
        }
        if let Some(v) = self.sum {
            out.push(("Sum", format_number(v)));
        }
        if let Some(v) = self.avg {
            out.push(("Avg", format_number(v)));
        }
        if let Some((lo, hi)) = self.text_len {
            out.push(("Min length", format_count(lo)));
            out.push(("Max length", format_count(hi)));
        }
        out
    }

    pub fn to_text(&self) -> String {
        self.lines().into_iter().map(|(k, v)| format!("{k}\t{v}")).collect::<Vec<_>>().join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }

    #[test]
    fn numbers() {
        let v = [Value::Int(3), Value::Decimal("4.5".into()), Value::Null, Value::Int(3), Value::Float(-1.0)];
        let s = compute(v.iter());
        assert_eq!((s.cells, s.count, s.nulls, s.distinct), (5, 4, 1, 3));
        assert_eq!(s.min.as_deref(), Some("-1.0"));
        assert_eq!(s.max.as_deref(), Some("4.5"));
        assert_eq!(s.sum, Some(9.5));
        assert_eq!(s.avg, Some(2.375));
        assert_eq!(s.text_len, None);
    }

    #[test]
    fn text_has_lengths_and_no_sum() {
        let v = [t("pear"), t("fig"), t("apple"), t("fig"), Value::Null];
        let s = compute(v.iter());
        assert_eq!((s.count, s.distinct, s.nulls), (4, 3, 1));
        assert_eq!(s.min.as_deref(), Some("apple"));
        assert_eq!(s.max.as_deref(), Some("pear"));
        assert_eq!(s.sum, None);
        assert_eq!(s.text_len, Some((3, 5)));
    }

    #[test]
    fn mixed_selection_has_no_sum_and_text_one_is_not_number_one() {
        let v = [Value::Int(1), t("1"), t("x")];
        let s = compute(v.iter());
        assert_eq!(s.distinct, 3);
        assert_eq!(s.sum, None);
    }

    #[test]
    fn all_null_and_empty() {
        let s = compute([Value::Null, Value::Null].iter());
        assert_eq!((s.cells, s.count, s.nulls, s.distinct), (2, 0, 2, 0));
        assert!(s.min.is_none() && s.sum.is_none());
        assert_eq!(compute(std::iter::empty()).cells, 0);
    }

    #[test]
    fn long_text_is_shortened_in_min_max() {
        let long = "x".repeat(100);
        let s = compute([t(&long)].iter());
        assert_eq!(s.min.as_ref().unwrap().chars().count(), 61);
        assert_eq!(s.text_len, Some((100, 100)));
    }

    #[test]
    fn copy_text_has_one_line_per_statistic() {
        let s = compute([Value::Int(1), Value::Int(2)].iter());
        let text = s.to_text();
        assert!(text.contains("Sum\t3"));
        assert!(text.contains("Avg\t1.5"));
        assert!(!text.contains("length"));
    }
}
