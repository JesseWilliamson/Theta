//! Column type inference from a sample, and the per-dialect type names for `CREATE TABLE`.

use crate::scalar::{NumberKind, parse_bool, parse_date, parse_number, parse_timestamp};
use theta_core::DatabaseKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnType {
    Integer,
    BigInt,
    Numeric {
        precision: u32,
        scale: u32,
    },
    Float,
    Boolean,
    Date,
    Timestamp {
        tz: bool,
    },
    /// `max_len` is the longest value in the sample, in characters.
    Text {
        max_len: usize,
    },
}

impl ColumnType {
    pub fn label(&self) -> &'static str {
        match self {
            ColumnType::Integer => "integer",
            ColumnType::BigInt => "bigint",
            ColumnType::Numeric { .. } => "numeric",
            ColumnType::Float => "float",
            ColumnType::Boolean => "boolean",
            ColumnType::Date => "date",
            ColumnType::Timestamp { .. } => "timestamp",
            ColumnType::Text { .. } => "text",
        }
    }

    /// The type to write in `CREATE TABLE` for `kind`.
    pub fn sql_type(&self, kind: DatabaseKind) -> String {
        use DatabaseKind::*;
        match (self, kind) {
            (ColumnType::Integer, Postgres) => "integer".into(),
            (ColumnType::Integer, MySql) => "INT".into(),
            (ColumnType::Integer, Sqlite) => "INTEGER".into(),
            (ColumnType::BigInt, Postgres) => "bigint".into(),
            (ColumnType::BigInt, MySql) => "BIGINT".into(),
            (ColumnType::BigInt, Sqlite) => "INTEGER".into(),
            // Unconstrained NUMERIC keeps every digit later rows may bring; MySQL has no such type, so it
            // gets headroom instead.
            (ColumnType::Numeric { .. }, Postgres) => "numeric".into(),
            (ColumnType::Numeric { precision, scale }, MySql) => {
                let scale = (*scale).min(30);
                let p = (precision + 6).clamp(scale + 1, 65);
                format!("DECIMAL({p},{scale})")
            }
            (ColumnType::Numeric { .. }, Sqlite) => "NUMERIC".into(),
            (ColumnType::Float, Postgres) => "double precision".into(),
            (ColumnType::Float, MySql) => "DOUBLE".into(),
            (ColumnType::Float, Sqlite) => "REAL".into(),
            (ColumnType::Boolean, Postgres) => "boolean".into(),
            (ColumnType::Boolean, MySql) => "TINYINT(1)".into(),
            // SQLite has no boolean type, but a declared BOOLEAN gets NUMERIC affinity and Theta reads it back as a
            // boolean, which is what the file said.
            (ColumnType::Boolean, Sqlite) => "BOOLEAN".into(),
            (ColumnType::Date, Postgres) => "date".into(),
            (ColumnType::Date, MySql) => "DATE".into(),
            (ColumnType::Date, Sqlite) => "DATE".into(),
            (ColumnType::Timestamp { tz: true }, Postgres) => "timestamptz".into(),
            (ColumnType::Timestamp { tz: false }, Postgres) => "timestamp".into(),
            (ColumnType::Timestamp { .. }, MySql) => "DATETIME(6)".into(),
            (ColumnType::Timestamp { .. }, Sqlite) => "TIMESTAMP".into(),
            (ColumnType::Text { max_len }, MySql) => {
                if *max_len <= 100 {
                    "VARCHAR(255)".into()
                } else {
                    "TEXT".into()
                }
            }
            (ColumnType::Text { .. }, Postgres) => "text".into(),
            (ColumnType::Text { .. }, Sqlite) => "TEXT".into(),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct ColState {
    non_null: u64,
    max_len: usize,
    all_bool: bool,
    all_int: bool,
    all_i32: bool,
    all_i64: bool,
    all_num: bool,
    all_float: bool,
    all_date: bool,
    all_ts: bool,
    ts_tz: bool,
    int_digits: u32,
    scale: u32,
}

impl ColState {
    fn new() -> Self {
        Self {
            all_bool: true,
            all_int: true,
            all_i32: true,
            all_i64: true,
            all_num: true,
            all_float: true,
            all_date: true,
            all_ts: true,
            ..Default::default()
        }
    }
}

/// Accumulates what a sample of values looks like, one column at a time.
#[derive(Debug, Clone)]
pub struct TypeInferer {
    cols: Vec<ColState>,
}

impl TypeInferer {
    pub fn new(columns: usize) -> Self {
        Self { cols: (0..columns).map(|_| ColState::new()).collect() }
    }

    pub fn columns(&self) -> usize {
        self.cols.len()
    }

    /// Observe one non-NULL value. Empty text is the caller's NULL decision, not ours.
    pub fn observe(&mut self, col: usize, value: &str) {
        let Some(st) = self.cols.get_mut(col) else { return };
        let v = value.trim();
        st.non_null += 1;
        st.max_len = st.max_len.max(value.chars().count());
        if st.all_bool && parse_bool(v).is_none() {
            st.all_bool = false;
        }
        match parse_number(v) {
            // "007" is an identifier (zip code, SKU), not a number: it forces text.
            Some(n) if !n.leading_zero => {
                if n.kind == NumberKind::Integer {
                    match v.parse::<i64>() {
                        Ok(i) => {
                            if i32::try_from(i).is_err() {
                                st.all_i32 = false;
                            }
                        }
                        Err(_) => {
                            st.all_i32 = false;
                            st.all_i64 = false;
                        }
                    }
                } else {
                    st.all_int = false;
                }
                if n.kind == NumberKind::Float {
                    st.all_num = false;
                }
                st.int_digits = st.int_digits.max(n.int_digits);
                st.scale = st.scale.max(n.scale);
            }
            _ => {
                st.all_int = false;
                st.all_num = false;
                st.all_float = false;
            }
        }
        if st.all_date && parse_date(v).is_none() {
            st.all_date = false;
        }
        if st.all_ts {
            match parse_timestamp(v) {
                Some((_, tz)) => st.ts_tz |= tz,
                None => st.all_ts = false,
            }
        }
    }

    /// The narrowest type every observed value of the column fits.
    pub fn infer(&self, col: usize) -> ColumnType {
        let Some(st) = self.cols.get(col) else { return ColumnType::Text { max_len: 0 } };
        if st.non_null == 0 {
            return ColumnType::Text { max_len: 0 };
        }
        // "1"/"0" columns are integers, never booleans: only word forms count for booleans, and a
        // column of numbers cannot be all words.
        if st.all_bool {
            return ColumnType::Boolean;
        }
        if st.all_int {
            return if st.all_i32 {
                ColumnType::Integer
            } else if st.all_i64 {
                ColumnType::BigInt
            } else {
                ColumnType::Numeric { precision: st.int_digits, scale: 0 }
            };
        }
        if st.all_num {
            return ColumnType::Numeric { precision: st.int_digits + st.scale, scale: st.scale };
        }
        if st.all_float {
            return ColumnType::Float;
        }
        if st.all_date {
            return ColumnType::Date;
        }
        if st.all_ts {
            return ColumnType::Timestamp { tz: st.ts_tz };
        }
        ColumnType::Text { max_len: st.max_len }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn infer(values: &[&str]) -> ColumnType {
        let mut t = TypeInferer::new(1);
        for v in values {
            t.observe(0, v);
        }
        t.infer(0)
    }

    #[test]
    fn integers_widen() {
        assert_eq!(infer(&["1", "-20", "300"]), ColumnType::Integer);
        assert_eq!(infer(&["1", "3000000000"]), ColumnType::BigInt);
        assert_eq!(infer(&["1", "99999999999999999999"]), ColumnType::Numeric { precision: 20, scale: 0 });
    }

    #[test]
    fn numerics_floats_and_mixes() {
        assert_eq!(infer(&["1", "2.50", "100.125"]), ColumnType::Numeric { precision: 6, scale: 3 });
        assert_eq!(infer(&["1", "1e3"]), ColumnType::Float);
        assert_eq!(infer(&["1.5", "abc"]), ColumnType::Text { max_len: 3 });
    }

    #[test]
    fn leading_zeros_stay_text() {
        assert_eq!(infer(&["00501", "10001"]), ColumnType::Text { max_len: 5 });
        assert_eq!(infer(&["0", "10"]), ColumnType::Integer);
    }

    #[test]
    fn booleans_dates_timestamps() {
        assert_eq!(infer(&["true", "FALSE", "t"]), ColumnType::Boolean);
        assert_eq!(infer(&["0", "1"]), ColumnType::Integer);
        assert_eq!(infer(&["2024-01-31", "2024-02-29"]), ColumnType::Date);
        assert_eq!(
            infer(&["2024-01-31 10:00:00", "2024-02-29T01:02:03"]),
            ColumnType::Timestamp { tz: false }
        );
        assert_eq!(infer(&["2024-01-31 10:00:00Z"]), ColumnType::Timestamp { tz: true });
        assert_eq!(infer(&["2024-01-31", "2024-01-31 10:00:00"]), ColumnType::Text { max_len: 19 });
    }

    #[test]
    fn empty_sample_is_text() {
        assert_eq!(infer(&[]), ColumnType::Text { max_len: 0 });
    }

    #[test]
    fn dialect_types() {
        use DatabaseKind::*;
        let n = ColumnType::Numeric { precision: 6, scale: 3 };
        assert_eq!(n.sql_type(Postgres), "numeric");
        assert_eq!(n.sql_type(MySql), "DECIMAL(12,3)");
        assert_eq!(ColumnType::Boolean.sql_type(MySql), "TINYINT(1)");
        assert_eq!(ColumnType::Timestamp { tz: true }.sql_type(Postgres), "timestamptz");
        assert_eq!(ColumnType::Text { max_len: 10 }.sql_type(MySql), "VARCHAR(255)");
        assert_eq!(ColumnType::Text { max_len: 1000 }.sql_type(MySql), "TEXT");
        assert_eq!(ColumnType::BigInt.sql_type(Sqlite), "INTEGER");
    }
}
