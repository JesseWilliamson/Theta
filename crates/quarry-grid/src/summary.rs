//! Selection summary text for the footer ("3 rows × 2 cols · sum 1,234.5").

use crate::format::{format_count, format_number};
use quarry_core::Value;

/// Running numeric aggregate over a selection.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NumericAgg {
    pub count: usize,
    pub sum: f64,
    /// A non-numeric, non-NULL cell was seen: no aggregate is shown.
    pub mixed: bool,
}

impl NumericAgg {
    pub fn add(&mut self, v: &Value) {
        match v {
            Value::Null => {}
            Value::Int(i) => self.push(*i as f64),
            Value::UInt(u) => self.push(*u as f64),
            Value::Float(f) if f.is_finite() => self.push(*f),
            Value::Decimal(s) => match s.trim().parse::<f64>() {
                Ok(f) if f.is_finite() => self.push(f),
                _ => self.mixed = true,
            },
            _ => self.mixed = true,
        }
    }

    fn push(&mut self, f: f64) {
        self.count += 1;
        self.sum += f;
    }
}

/// `rows x cols` is the selected rectangle. The aggregate is shown only when
/// more than one cell is selected and every non-NULL cell is a number.
pub fn selection_summary(rows: usize, cols: usize, agg: Option<NumericAgg>) -> String {
    let cells = rows * cols;
    let mut out = if cells == 1 {
        "1 cell".to_string()
    } else if cols == 1 {
        format!("{} rows", format_count(rows))
    } else if rows == 1 {
        format!("{} cols", format_count(cols))
    } else {
        format!("{} rows × {} cols", format_count(rows), format_count(cols))
    };
    if let Some(a) = agg
        && cells > 1
        && !a.mixed
        && a.count > 0
    {
        out.push_str(&format!(
            " · sum {} · avg {}",
            format_number(a.sum),
            format_number(a.sum / a.count as f64)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agg(vals: &[Value]) -> NumericAgg {
        let mut a = NumericAgg::default();
        vals.iter().for_each(|v| a.add(v));
        a
    }

    #[test]
    fn shapes() {
        assert_eq!(selection_summary(1, 1, None), "1 cell");
        assert_eq!(selection_summary(5, 1, None), "5 rows");
        assert_eq!(selection_summary(1, 3, None), "3 cols");
        assert_eq!(selection_summary(3, 2, None), "3 rows × 2 cols");
        assert_eq!(selection_summary(2000, 2, None), "2,000 rows × 2 cols");
    }

    #[test]
    fn numeric_aggregate() {
        let a = agg(&[Value::Int(1000), Value::Decimal("234.5".into()), Value::Float(0.0), Value::Null]);
        // count ignores NULLs: avg over 3 numbers
        assert_eq!(selection_summary(2, 2, Some(a)), "2 rows × 2 cols · sum 1,234.5 · avg 411.5");
    }

    #[test]
    fn mixed_or_single_cells_have_no_aggregate() {
        let a = agg(&[Value::Int(1), Value::Text("x".into())]);
        assert_eq!(selection_summary(2, 1, Some(a)), "2 rows");
        let a = agg(&[Value::Int(1)]);
        assert_eq!(selection_summary(1, 1, Some(a)), "1 cell");
        let a = agg(&[Value::Null, Value::Null]);
        assert_eq!(selection_summary(2, 1, Some(a)), "2 rows");
    }
}
