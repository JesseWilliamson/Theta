//! Type-aware ordering for client-side sort. No allocation per comparison:
//! sorting 100k rows performs ~1.7M of these.

use quarry_core::{ResultSet, Value};
use std::cmp::Ordering;

/// Case-insensitive "natural" order: digit runs compare by numeric value
/// (`item2 < item10`), everything else by lower-cased character. Ties on the
/// folded form fall back to the raw strings so the order is total and stable
/// across runs.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut ai, mut bi) = (a.char_indices().peekable(), b.char_indices().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some((sa, ca)), Some((sb, cb))) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let ea = digit_run_end(a, sa);
                    let eb = digit_run_end(b, sb);
                    let (da, db) = (a[sa..ea].trim_start_matches('0'), b[sb..eb].trim_start_matches('0'));
                    let ord = da.len().cmp(&db.len()).then_with(|| da.cmp(db));
                    if ord != Ordering::Equal {
                        return ord;
                    }
                    while ai.peek().is_some_and(|&(i, _)| i < ea) {
                        ai.next();
                    }
                    while bi.peek().is_some_and(|&(i, _)| i < eb) {
                        bi.next();
                    }
                } else {
                    let ord = fold(ca).cmp(&fold(cb));
                    if ord != Ordering::Equal {
                        return ord;
                    }
                    ai.next();
                    bi.next();
                }
            }
        }
    }
}

fn digit_run_end(s: &str, start: usize) -> usize {
    s[start..].find(|c: char| !c.is_ascii_digit()).map_or(s.len(), |i| start + i)
}

fn fold(c: char) -> char {
    if c.is_ascii() { c.to_ascii_lowercase() } else { c.to_lowercase().next().unwrap_or(c) }
}

/// Exact comparison of two decimal strings (`-12.50`, `3`, `1e5` is *not*
/// understood and yields `None`, so callers fall back to floating point).
pub fn cmp_decimal_str(a: &str, b: &str) -> Option<Ordering> {
    let (na, ia, fa) = split_decimal(a)?;
    let (nb, ib, fb) = split_decimal(b)?;
    let a_zero = ia.is_empty() && fa.is_empty();
    let b_zero = ib.is_empty() && fb.is_empty();
    let (na, nb) = (na && !a_zero, nb && !b_zero);
    if na != nb {
        return Some(if na { Ordering::Less } else { Ordering::Greater });
    }
    let mag = ia.len().cmp(&ib.len()).then_with(|| ia.cmp(ib)).then_with(|| fa.cmp(fb));
    Some(if na { mag.reverse() } else { mag })
}

/// (negative, integer digits without leading zeros, fraction without trailing zeros)
fn split_decimal(s: &str) -> Option<(bool, &str, &str)> {
    let s = s.trim();
    let (neg, rest) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    let (int, frac) = rest.split_once('.').unwrap_or((rest, ""));
    if (int.is_empty() && frac.is_empty())
        || !int.bytes().all(|c| c.is_ascii_digit())
        || !frac.bytes().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    Some((neg, int.trim_start_matches('0'), frac.trim_end_matches('0')))
}

fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Int(i) => Some(*i as f64),
        Value::UInt(u) => Some(*u as f64),
        Value::Float(f) => Some(*f),
        Value::Decimal(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Order of two non-NULL values. Numbers compare numerically across variants
/// (an `Int` against a `Decimal`), unrelated variants by a fixed variant rank.
pub fn compare_values(a: &Value, b: &Value) -> Ordering {
    use Value::*;
    match (a, b) {
        (Int(x), Int(y)) => x.cmp(y),
        (UInt(x), UInt(y)) => x.cmp(y),
        (Int(x), UInt(y)) => (*x as i128).cmp(&(*y as i128)),
        (UInt(x), Int(y)) => (*x as i128).cmp(&(*y as i128)),
        (Float(x), Float(y)) => x.total_cmp(y),
        (Decimal(x), Decimal(y)) => cmp_decimal_str(x, y).unwrap_or_else(|| num_fallback(a, b)),
        (Bool(x), Bool(y)) => x.cmp(y),
        (Bytes(x), Bytes(y)) | (Geometry(x), Geometry(y)) => x.cmp(y),
        (Date(x), Date(y))
        | (Time(x), Time(y))
        | (Timestamp(x), Timestamp(y))
        | (TimestampTz(x), TimestampTz(y)) => x.cmp(y),
        (
            Text(x) | Json(x) | Uuid(x) | Interval(x) | Unknown(x),
            Text(y) | Json(y) | Uuid(y) | Interval(y) | Unknown(y),
        ) => natural_cmp(x, y),
        (Array(x), Array(y)) => {
            for (p, q) in x.iter().zip(y) {
                let ord = compare_nullable(p, q);
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            x.len().cmp(&y.len())
        }
        _ if a.is_numeric() && b.is_numeric() => num_fallback(a, b),
        _ => variant_rank(a).cmp(&variant_rank(b)).then_with(|| natural_cmp(&a.display(), &b.display())),
    }
}

fn num_fallback(a: &Value, b: &Value) -> Ordering {
    match (as_f64(a), as_f64(b)) {
        (Some(x), Some(y)) => x.total_cmp(&y),
        // An unparsable decimal text sorts after real numbers.
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.display().cmp(&b.display()),
    }
}

fn compare_nullable(a: &Value, b: &Value) -> Ordering {
    match (a.is_null(), b.is_null()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        _ => compare_values(a, b),
    }
}

fn variant_rank(v: &Value) -> u8 {
    match v {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Int(_) | Value::UInt(_) | Value::Float(_) | Value::Decimal(_) => 2,
        Value::Text(_) | Value::Unknown(_) => 3,
        Value::Json(_) => 4,
        Value::Uuid(_) => 5,
        Value::Date(_) => 6,
        Value::Time(_) => 7,
        Value::Timestamp(_) => 8,
        Value::TimestampTz(_) => 9,
        Value::Interval(_) => 10,
        Value::Array(_) => 11,
        Value::Bytes(_) => 12,
        Value::Geometry(_) => 13,
    }
}

/// Sort comparison with the grid's NULL rule: NULLs go last in both
/// directions, so a descending sort of a sparse column still starts with data.
pub fn compare_for_sort(a: &Value, b: &Value, descending: bool) -> Ordering {
    match (a.is_null(), b.is_null()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        _ => {
            let ord = compare_values(a, b);
            if descending { ord.reverse() } else { ord }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortKey {
    pub column: usize,
    pub descending: bool,
}

/// Stable sort of `indices` (row numbers into `result`) by `keys`.
pub fn sort_indices(result: &ResultSet, indices: &mut [u32], keys: &[SortKey]) {
    if keys.is_empty() {
        return;
    }
    indices.sort_by(|&a, &b| {
        let (ra, rb) = (&result.rows[a as usize], &result.rows[b as usize]);
        for k in keys {
            let (va, vb) =
                (ra.get(k.column).unwrap_or(&Value::Null), rb.get(k.column).unwrap_or(&Value::Null));
            let ord = compare_for_sort(va, vb, k.descending);
            if ord != Ordering::Equal {
                return ord;
            }
        }
        Ordering::Equal
    });
}

/// Header click: none -> ascending -> descending -> none. With `additive`
/// (shift) the column joins or cycles within the existing sort; otherwise it
/// replaces it.
pub fn cycle_sort(current: &[SortKey], column: usize, additive: bool) -> Vec<SortKey> {
    let existing = current.iter().position(|k| k.column == column);
    let next = match existing.map(|i| current[i].descending) {
        None => Some(false),
        Some(false) => Some(true),
        Some(true) => None,
    };
    let mut out: Vec<SortKey> = if additive { current.to_vec() } else { Vec::new() };
    if let Some(i) = out.iter().position(|k| k.column == column) {
        match next {
            Some(desc) => out[i].descending = desc,
            None => {
                out.remove(i);
            }
        }
    } else if let Some(desc) = next {
        out.push(SortKey { column, descending: desc });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use quarry_core::{Column, TypeKind};

    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }

    #[test]
    fn natural_order() {
        assert_eq!(natural_cmp("item2", "item10"), Ordering::Less);
        assert_eq!(natural_cmp("Item2", "item10"), Ordering::Less);
        assert_eq!(natural_cmp("apple", "Banana"), Ordering::Less);
        assert_eq!(natural_cmp("a007", "a7"), Ordering::Less, "equal folded value ties on raw text");
        assert_eq!(natural_cmp("a01b", "a1c"), Ordering::Less);
        assert_eq!(natural_cmp("", "a"), Ordering::Less);
        assert_eq!(natural_cmp("abc", "abc"), Ordering::Equal);
        assert_eq!(natural_cmp("x9", "x10y"), Ordering::Less);
    }

    #[test]
    fn natural_order_unicode() {
        assert_eq!(natural_cmp("Äpfel", "äpfel2"), Ordering::Less);
        assert_eq!(natural_cmp("é1", "É2"), Ordering::Less);
    }

    #[test]
    fn decimals_compare_exactly() {
        let c = |a: &str, b: &str| cmp_decimal_str(a, b).unwrap();
        assert_eq!(c("9.99", "10.00"), Ordering::Less);
        assert_eq!(c("-9.99", "-10.00"), Ordering::Greater);
        assert_eq!(c("1.50", "1.5"), Ordering::Equal);
        assert_eq!(c("-0.0", "0"), Ordering::Equal);
        assert_eq!(c("123456789012345678901234567890.1", "123456789012345678901234567890.2"), Ordering::Less);
        assert_eq!(c("-1", "1"), Ordering::Less);
        assert_eq!(c("0.001", "0.01"), Ordering::Less);
        assert!(cmp_decimal_str("abc", "1").is_none());
    }

    #[test]
    fn numbers_compare_across_variants() {
        assert_eq!(compare_values(&Value::Int(5), &Value::Decimal("5.5".into())), Ordering::Less);
        assert_eq!(compare_values(&Value::Float(2.5), &Value::Int(2)), Ordering::Greater);
        assert_eq!(compare_values(&Value::Int(-1), &Value::UInt(0)), Ordering::Less);
        assert_eq!(compare_values(&Value::UInt(u64::MAX), &Value::Int(i64::MAX)), Ordering::Greater);
        assert_eq!(
            compare_values(&Value::Decimal("10".into()), &Value::Decimal("9".into())),
            Ordering::Greater
        );
    }

    #[test]
    fn nulls_last_both_directions() {
        let (n, one) = (Value::Null, Value::Int(1));
        assert_eq!(compare_for_sort(&n, &one, false), Ordering::Greater);
        assert_eq!(compare_for_sort(&n, &one, true), Ordering::Greater);
        assert_eq!(compare_for_sort(&one, &n, true), Ordering::Less);
        assert_eq!(compare_for_sort(&n, &n, true), Ordering::Equal);
        assert_eq!(compare_for_sort(&Value::Int(1), &Value::Int(2), true), Ordering::Greater);
    }

    #[test]
    fn mixed_variants_have_total_order() {
        let vals = [
            Value::Bool(true),
            Value::Int(1),
            t("a"),
            Value::Date("2020-01-01".into()),
            Value::Bytes(vec![1]),
        ];
        for a in &vals {
            for b in &vals {
                assert_eq!(compare_values(a, b), compare_values(b, a).reverse());
            }
        }
    }

    fn result(rows: Vec<Vec<Value>>) -> ResultSet {
        let col =
            |n: &str| Column { name: n.into(), type_name: "x".into(), kind: TypeKind::Other, origin: None };
        ResultSet { columns: vec![col("a"), col("b")], rows, truncated: false }
    }

    #[test]
    fn multi_key_sort_is_stable() {
        let rs = result(vec![
            vec![t("b"), Value::Int(1)],
            vec![t("a"), Value::Int(2)],
            vec![t("a"), Value::Int(1)],
            vec![Value::Null, Value::Int(0)],
            vec![t("a"), Value::Int(1)],
        ]);
        let mut idx: Vec<u32> = (0..5).collect();
        sort_indices(
            &rs,
            &mut idx,
            &[SortKey { column: 0, descending: false }, SortKey { column: 1, descending: true }],
        );
        assert_eq!(idx, vec![1, 2, 4, 0, 3]);
    }

    #[test]
    fn sort_cycle() {
        let none: Vec<SortKey> = vec![];
        let s1 = cycle_sort(&none, 2, false);
        assert_eq!(s1, vec![SortKey { column: 2, descending: false }]);
        let s2 = cycle_sort(&s1, 2, false);
        assert_eq!(s2, vec![SortKey { column: 2, descending: true }]);
        assert!(cycle_sort(&s2, 2, false).is_empty());
        // plain click on another column replaces; shift adds
        assert_eq!(cycle_sort(&s1, 0, false), vec![SortKey { column: 0, descending: false }]);
        let both = cycle_sort(&s1, 0, true);
        assert_eq!(both.len(), 2);
        assert_eq!(both[1].column, 0);
        // shift-cycling the first key keeps the secondary
        let flipped = cycle_sort(&both, 2, true);
        assert_eq!(flipped[0], SortKey { column: 2, descending: true });
        assert_eq!(cycle_sort(&flipped, 2, true), vec![SortKey { column: 0, descending: false }]);
    }
}
