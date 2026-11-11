//! The find bar's matching rules. Scans 100k x 60 cells on a background
//! thread, so matching must not allocate per cell.

use std::fmt::Write as _;
use theta_core::{Column, ResultSet, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterQuery {
    /// Restrict matching to this column (`name:text` syntax).
    pub column: Option<usize>,
    /// Lower-cased text to look for.
    pub needle: String,
}

impl FilterQuery {
    /// `None` for blank input. `name:text` restricts to the column called
    /// `name` (case-insensitive) when such a column exists; otherwise the whole
    /// string is searched, so `12:30` still finds times.
    pub fn parse(input: &str, columns: &[Column]) -> Option<FilterQuery> {
        let input = input.trim();
        if input.is_empty() {
            return None;
        }
        if let Some((name, rest)) = input.split_once(':') {
            let name = name.trim();
            if !name.is_empty()
                && let Some(column) = columns.iter().position(|c| c.name.eq_ignore_ascii_case(name))
            {
                let needle = rest.trim().to_lowercase();
                // `price:` with nothing after it means "no text yet".
                if needle.is_empty() {
                    return None;
                }
                return Some(FilterQuery { column: Some(column), needle });
            }
        }
        Some(FilterQuery { column: None, needle: input.to_lowercase() })
    }

    pub fn matches_row(&self, row: &[Value], scratch: &mut String) -> bool {
        match self.column {
            Some(c) => row.get(c).is_some_and(|v| value_contains(v, &self.needle, scratch)),
            None => row.iter().any(|v| value_contains(v, &self.needle, scratch)),
        }
    }

    /// Row numbers of `result` matching the query, in original order.
    pub fn matching_rows(&self, result: &ResultSet) -> Vec<u32> {
        let mut scratch = String::new();
        result
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| self.matches_row(row, &mut scratch))
            .map(|(i, _)| i as u32)
            .collect()
    }
}

/// Case-insensitive substring test of `needle_lower` against a value's text
/// form. `scratch` is reused for number formatting.
pub fn value_contains(value: &Value, needle_lower: &str, scratch: &mut String) -> bool {
    match value {
        Value::Null | Value::Geometry(_) | Value::Bytes(_) => false,
        Value::Text(s)
        | Value::Json(s)
        | Value::Uuid(s)
        | Value::Date(s)
        | Value::Time(s)
        | Value::Timestamp(s)
        | Value::TimestampTz(s)
        | Value::Interval(s)
        | Value::Unknown(s)
        | Value::Decimal(s) => contains_ci(s, needle_lower),
        Value::Bool(b) => contains_ci(if *b { "true" } else { "false" }, needle_lower),
        Value::Int(i) => {
            scratch.clear();
            let _ = write!(scratch, "{i}");
            contains_ci(scratch, needle_lower)
        }
        Value::UInt(i) => {
            scratch.clear();
            let _ = write!(scratch, "{i}");
            contains_ci(scratch, needle_lower)
        }
        Value::Float(_) => {
            let s = value.display();
            contains_ci(&s, needle_lower)
        }
        Value::Array(items) => items.iter().any(|v| value_contains(v, needle_lower, scratch)),
    }
}

/// `needle_lower` must already be lower-case. ASCII on both sides takes an
/// allocation-free path; anything else lower-cases the haystack once.
pub fn contains_ci(haystack: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if haystack.is_ascii() && needle_lower.is_ascii() {
        let (h, n) = (haystack.as_bytes(), needle_lower.as_bytes());
        if n.len() > h.len() {
            return false;
        }
        return h.windows(n.len()).any(|w| w.iter().zip(n).all(|(a, b)| a.to_ascii_lowercase() == *b));
    }
    haystack.to_lowercase().contains(needle_lower)
}

#[cfg(test)]
mod tests {
    use super::*;
    use theta_core::TypeKind;

    fn col(n: &str) -> Column {
        Column { name: n.into(), type_name: "t".into(), kind: TypeKind::Text, origin: None }
    }

    #[test]
    fn parse_variants() {
        let cols = [col("id"), col("Name"), col("note")];
        assert_eq!(FilterQuery::parse("  ", &cols), None);
        assert_eq!(
            FilterQuery::parse("Bob", &cols),
            Some(FilterQuery { column: None, needle: "bob".into() })
        );
        assert_eq!(
            FilterQuery::parse("name: Bob ", &cols),
            Some(FilterQuery { column: Some(1), needle: "bob".into() })
        );
        // unknown column keeps the colon in the text
        assert_eq!(
            FilterQuery::parse("12:30", &cols),
            Some(FilterQuery { column: None, needle: "12:30".into() })
        );
        assert_eq!(FilterQuery::parse("name:", &cols), None);
    }

    #[test]
    fn contains_is_case_insensitive() {
        assert!(contains_ci("Hello World", "lo wo"));
        assert!(!contains_ci("Hello", "x"));
        assert!(contains_ci("anything", ""));
        assert!(!contains_ci("ab", "abc"));
        assert!(contains_ci("ÄPFEL", "äpf"));
        assert!(contains_ci("Straße ÄPFEL", "straße"));
    }

    #[test]
    fn value_matching_covers_types() {
        let mut s = String::new();
        assert!(value_contains(&Value::Int(12345), "234", &mut s));
        assert!(value_contains(&Value::Bool(true), "tru", &mut s));
        assert!(value_contains(&Value::Decimal("19.99".into()), "9.9", &mut s));
        assert!(value_contains(&Value::Float(2.5), "2.5", &mut s));
        assert!(value_contains(&Value::Array(vec![Value::Text("Foo".into())]), "foo", &mut s));
        assert!(!value_contains(&Value::Null, "null", &mut s));
    }

    #[test]
    fn filters_rows_any_column_or_one() {
        let rs = ResultSet {
            columns: vec![col("id"), col("name")],
            rows: vec![
                vec![Value::Int(1), Value::Text("Alice".into())],
                vec![Value::Int(2), Value::Text("bob".into())],
                vec![Value::Int(3), Value::Text("Carol 2".into())],
            ],
            truncated: false,
        };
        let q = FilterQuery::parse("2", &rs.columns).unwrap();
        assert_eq!(q.matching_rows(&rs), vec![1, 2]);
        let q = FilterQuery::parse("name:2", &rs.columns).unwrap();
        assert_eq!(q.matching_rows(&rs), vec![2]);
        let q = FilterQuery::parse("ALICE", &rs.columns).unwrap();
        assert_eq!(q.matching_rows(&rs), vec![0]);
    }
}
