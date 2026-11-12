//! The display order of rows: filter, then sort. Pure so it can run on a
//! background thread against an `Arc<ResultSet>`.

use crate::compare::{SortKey, sort_indices};
use crate::filter::FilterQuery;
use theta_core::ResultSet;

/// Original row numbers in display order.
pub fn build_view(result: &ResultSet, filter: Option<&FilterQuery>, sorts: &[SortKey]) -> Vec<u32> {
    let mut indices = match filter {
        Some(q) => q.matching_rows(result),
        None => (0..result.rows.len() as u32).collect(),
    };
    sort_indices(result, &mut indices, sorts);
    indices
}

/// The persistent footer notice for a sort or filter that only saw part of the result.
///
/// A truncated result is a prefix of what the server holds, so a descending sort shows the biggest
/// of the loaded rows and a filter can say "no rows" while matches exist beyond them. Nothing is
/// said when neither is active (the plain "limit reached" label covers that) or when all rows are in.
pub fn partial_notice(sorted: bool, filtered: bool, loaded: usize, truncated: bool) -> Option<String> {
    if !truncated || !(sorted || filtered) {
        return None;
    }
    let what = match (sorted, filtered) {
        (true, true) => "Sorted and filtered",
        (true, false) => "Sorted",
        _ => "Filtered",
    };
    Some(format!("{what} within the {} rows loaded", crate::format::format_count(loaded)))
}

/// Above this many rows, filtering/sorting leaves the UI thread.
pub const BACKGROUND_THRESHOLD: usize = 20_000;

#[cfg(test)]
mod tests {
    use super::*;
    use theta_core::{Column, TypeKind, Value};

    fn rs() -> ResultSet {
        let col =
            |n: &str| Column { name: n.into(), type_name: "t".into(), kind: TypeKind::Text, origin: None };
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

    /// Not a benchmark (the bound is generous so a loaded CI box passes), but
    /// the printed numbers are what the README quotes: `cargo test -- --nocapture`.
    #[test]
    fn sorting_and_filtering_100k_rows() {
        let col = |n: &str, k| Column { name: n.into(), type_name: "t".into(), kind: k, origin: None };
        let mut x = 12345u64;
        let mut next = || {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            x >> 33
        };
        let rows: Vec<Vec<Value>> = (0..100_000)
            .map(|i| {
                vec![
                    Value::Int((next() % 1_000_000) as i64),
                    Value::Text(format!("name-{}-{}", next() % 5000, i)),
                    Value::Decimal(format!("{}.{:02}", next() % 100_000, next() % 100)),
                ]
            })
            .collect();
        let rs = ResultSet {
            columns: vec![col("n", TypeKind::Integer), col("s", TypeKind::Text), col("d", TypeKind::Decimal)],
            rows,
            truncated: false,
        };
        for (name, c) in [("int", 0usize), ("text", 1), ("decimal", 2)] {
            let t = std::time::Instant::now();
            let v = build_view(&rs, None, &[SortKey { column: c, descending: false }]);
            let ms = t.elapsed().as_millis();
            println!("sort 100k rows by {name}: {ms} ms");
            assert_eq!(v.len(), 100_000);
            assert!(ms < 5_000);
        }
        let q = FilterQuery::parse("name-42", &rs.columns).unwrap();
        let t = std::time::Instant::now();
        let v = build_view(&rs, Some(&q), &[]);
        println!("filter 100k x 3 cells for 'name-42': {} ms, {} hits", t.elapsed().as_millis(), v.len());
        assert!(t.elapsed().as_millis() < 5_000);
    }

    #[test]
    fn partial_notice_wording() {
        assert_eq!(partial_notice(true, false, 1000, true).as_deref(), Some("Sorted within the 1,000 rows loaded"));
        assert_eq!(
            partial_notice(false, true, 1000, true).as_deref(),
            Some("Filtered within the 1,000 rows loaded")
        );
        assert_eq!(
            partial_notice(true, true, 2500, true).as_deref(),
            Some("Sorted and filtered within the 2,500 rows loaded")
        );
        assert_eq!(partial_notice(true, true, 1000, false), None, "everything is loaded");
        assert_eq!(partial_notice(false, false, 1000, true), None, "nothing applied");
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

/// Where original row `row` sits in the display order, if it is visible (a filter may hide it).
pub fn display_position(view: Option<&[u32]>, n_rows: usize, row: usize) -> Option<usize> {
    match view {
        None => (row < n_rows).then_some(row),
        Some(v) => v.iter().position(|&r| r as usize == row),
    }
}

#[cfg(test)]
mod display_position_tests {
    use super::display_position;

    #[test]
    fn identity_and_reordered_views() {
        assert_eq!(display_position(None, 5, 3), Some(3));
        assert_eq!(display_position(None, 5, 5), None);
        let v = [4u32, 0, 2];
        assert_eq!(display_position(Some(&v), 5, 2), Some(2));
        assert_eq!(display_position(Some(&v), 5, 4), Some(0));
        assert_eq!(display_position(Some(&v), 5, 1), None, "filtered out");
    }
}
