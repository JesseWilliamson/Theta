//! The display order of rows: filter, then sort. Pure so it can run on a
//! background thread against an `Arc<ResultSet>`.

use crate::compare::{SortKey, sort_indices};
use crate::filter::FilterQuery;
use quarry_core::ResultSet;

/// Original row numbers in display order.
pub fn build_view(result: &ResultSet, filter: Option<&FilterQuery>, sorts: &[SortKey]) -> Vec<u32> {
    let mut indices = match filter {
        Some(q) => q.matching_rows(result),
        None => (0..result.rows.len() as u32).collect(),
    };
    sort_indices(result, &mut indices, sorts);
    indices
}

/// Above this many rows, filtering/sorting leaves the UI thread.
pub const BACKGROUND_THRESHOLD: usize = 20_000;

#[cfg(test)]
mod tests {
    use super::*;
    use quarry_core::{Column, TypeKind, Value};

    fn rs() -> ResultSet {
        let col = |n: &str| Column { name: n.into(), type_name: "t".into(), kind: TypeKind::Text, origin: None };
        ResultSet {
            columns: vec![col("n"), col("s")],
            rows: vec![
                vec![Value::Int(3), Value::Text("c".into())],
                vec![Value::Int(1), Value::Text("a".into())],
                vec![Value::Int(2), Value::Text("b".into())],
                vec![Value::Null, Value::Text("a".into())],
            ],
            truncated: false,
        }
    }

    #[test]
    fn identity_without_filter_or_sort() {
        assert_eq!(build_view(&rs(), None, &[]), vec![0, 1, 2, 3]);
    }

    #[test]
    fn filter_then_sort() {
        let r = rs();
        let q = FilterQuery::parse("s:a", &r.columns).unwrap();
        let v = build_view(&r, Some(&q), &[SortKey { column: 0, descending: true }]);
        assert_eq!(v, vec![1, 3], "nulls last even when descending");
        let v = build_view(&r, None, &[SortKey { column: 0, descending: false }]);
        assert_eq!(v, vec![1, 2, 0, 3]);
    }
}
