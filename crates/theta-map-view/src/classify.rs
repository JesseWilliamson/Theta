//! Turning a column into colour classes: numeric classification (quantile, equal interval,
//! logarithmic) and categorical top-N-plus-Other.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Fewest and most classes a numeric colouring uses.
pub const MIN_CLASSES: usize = 5;
/// See [`MIN_CLASSES`].
pub const MAX_CLASSES: usize = 9;

/// How numeric values are cut into classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Method {
    /// Each class holds about the same number of values.
    Quantile,
    /// Classes of equal width.
    EqualInterval,
    /// Equal width on a logarithmic scale (heavy-tailed data).
    Logarithmic,
}

impl Method {
    /// Every method, in menu order.
    pub const ALL: [Method; 3] = [Method::Quantile, Method::EqualInterval, Method::Logarithmic];

    /// Menu label.
    pub fn label(self) -> &'static str {
        match self {
            Method::Quantile => "Quantile",
            Method::EqualInterval => "Equal interval",
            Method::Logarithmic => "Logarithmic",
        }
    }
}

/// Class edges `[min, e1, .., max]` for the finite `values`; at most `classes` classes, fewer
/// when values repeat so much that edges coincide. Empty input gives no edges.
pub fn breaks(values: &[f64], method: Method, classes: usize) -> Vec<f64> {
    let classes = classes.clamp(1, MAX_CLASSES);
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return Vec::new();
    }
    v.sort_unstable_by(f64::total_cmp);
    let (min, max) = (v[0], v[v.len() - 1]);
    let mut edges = vec![min];
    for i in 1..classes {
        let f = i as f64 / classes as f64;
        let e = match method {
            Method::Quantile => v[((v.len() - 1) as f64 * f).round() as usize],
            Method::EqualInterval => min + (max - min) * f,
            // ln(v - min + 1) keeps zero and negative values legal.
            Method::Logarithmic => min + (((max - min + 1.0).ln() * f).exp() - 1.0),
        };
        if e > *edges.last().unwrap_or(&min) && e < max {
            edges.push(e);
        }
    }
    edges.push(max);
    edges.dedup();
    if edges.len() == 1 {
        edges.push(max);
    }
    edges
}

/// The class of `x` for `edges` from [`breaks`]: values on an interior edge go up, the
/// maximum stays in the last class. Out-of-range values clamp.
pub fn class_of(edges: &[f64], x: f64) -> usize {
    let n = edges.len().saturating_sub(1).max(1);
    let i = edges[1..edges.len().saturating_sub(1)].partition_point(|e| *e <= x);
    i.min(n - 1)
}

/// The `n` most frequent values (ties broken alphabetically, so the legend is stable) with
/// their counts, and how many rows fall outside them.
pub fn top_values<'a>(values: impl Iterator<Item = &'a str>, n: usize) -> (Vec<(String, usize)>, usize) {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut total = 0;
    for v in values {
        *counts.entry(v).or_default() += 1;
        total += 1;
    }
    let mut all: Vec<(&str, usize)> = counts.into_iter().collect();
    all.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let top: Vec<(String, usize)> = all.into_iter().take(n).map(|(k, c)| (k.to_string(), c)).collect();
    let other = total - top.iter().map(|(_, c)| c).sum::<usize>();
    (top, other)
}

/// Compact number for legends: thousands separators are avoided in favour of k/M suffixes
/// and at most three significant digits.
pub fn short_number(x: f64) -> String {
    if !x.is_finite() {
        return "-".into();
    }
    let a = x.abs();
    let (v, suffix) = if a >= 1e9 {
        (x / 1e9, "G")
    } else if a >= 1e6 {
        (x / 1e6, "M")
    } else if a >= 1e4 {
        (x / 1e3, "k")
    } else {
        (x, "")
    };
    let s = if v.abs() >= 100.0 || (v.fract() == 0.0 && suffix.is_empty()) {
        format!("{v:.0}")
    } else if v.abs() >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    };
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    format!("{s}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_interval_edges() {
        let e = breaks(&[0.0, 10.0], Method::EqualInterval, 5);
        assert_eq!(e, vec![0.0, 2.0, 4.0, 6.0, 8.0, 10.0]);
        assert_eq!(class_of(&e, 0.0), 0);
        assert_eq!(class_of(&e, 2.0), 1);
        assert_eq!(class_of(&e, 9.99), 4);
        assert_eq!(class_of(&e, 10.0), 4);
        assert_eq!(class_of(&e, 99.0), 4);
        assert_eq!(class_of(&e, -5.0), 0);
    }

    #[test]
    fn quantile_balances_class_sizes() {
        let v: Vec<f64> = (0..1000).map(|i| (i as f64).powi(3)).collect();
        let e = breaks(&v, Method::Quantile, 5);
        let mut counts = [0usize; 5];
        for x in &v {
            counts[class_of(&e, *x)] += 1;
        }
        for c in counts {
            assert!((190..=210).contains(&c), "{counts:?}");
        }
    }

    #[test]
    fn logarithmic_edges_grow_geometrically() {
        let e = breaks(&[1.0, 10000.0], Method::Logarithmic, 4);
        assert_eq!(e.len(), 5);
        let r1 = (e[2] - 1.0 + 1.0) / (e[1] - 1.0 + 1.0);
        let r2 = (e[3] + 1.0 - 1.0) / (e[2] + 1.0 - 1.0);
        assert!((r1 - r2).abs() / r1 < 0.2, "{e:?}");
        // Zero and negatives are legal.
        let z = breaks(&[-5.0, 0.0, 100.0], Method::Logarithmic, 5);
        assert!(z.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn constant_and_empty_inputs() {
        assert!(breaks(&[], Method::Quantile, 5).is_empty());
        let e = breaks(&[3.0, 3.0, 3.0], Method::Quantile, 5);
        assert_eq!(e, vec![3.0, 3.0]);
        assert_eq!(class_of(&e, 3.0), 0);
        assert!(breaks(&[f64::NAN, 1.0, 2.0], Method::EqualInterval, 5).iter().all(|x| x.is_finite()));
    }

    #[test]
    fn top_n_plus_other() {
        let data = ["a", "b", "a", "c", "a", "b", "d", "e"];
        let (top, other) = top_values(data.iter().copied(), 2);
        assert_eq!(top, vec![("a".to_string(), 3), ("b".to_string(), 2)]);
        assert_eq!(other, 3);
        // Ties are alphabetical.
        let (top, _) = top_values(["z", "y"].iter().copied(), 1);
        assert_eq!(top[0].0, "y");
    }

    #[test]
    fn short_numbers() {
        assert_eq!(short_number(0.0), "0");
        assert_eq!(short_number(12.0), "12");
        assert_eq!(short_number(0.5), "0.5");
        assert_eq!(short_number(1234.0), "1234");
        assert_eq!(short_number(25_000.0), "25k");
        assert_eq!(short_number(1_500_000.0), "1.5M");
        assert_eq!(short_number(-3.25), "-3.25");
    }
}
