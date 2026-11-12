use serde::{Deserialize, Serialize};
use std::time::Duration;

/// A single cell. Drivers normalise their native types into this enum so the
/// grid, exporters and editors never need to know which database produced it.
///
/// Temporal and exact-numeric values are kept as their canonical text form:
/// a database IDE must display exactly what the server holds, and round-tripping
/// through binary floats or a local timezone would silently change it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    /// Arbitrary-precision number in canonical decimal text.
    Decimal(String),
    Text(String),
    Bytes(Vec<u8>),
    /// JSON/JSONB as compact text.
    Json(String),
    Uuid(String),
    /// ISO-8601 `YYYY-MM-DD`.
    Date(String),
    Time(String),
    /// Timestamp without zone, `YYYY-MM-DD HH:MM:SS[.ffffff]`.
    Timestamp(String),
    /// Timestamp with zone, rendered with an explicit offset.
    TimestampTz(String),
    Interval(String),
    Array(Vec<Value>),
    /// Spatial value as (E)WKB bytes. `theta-geo` decodes it.
    Geometry(Vec<u8>),
    /// A type the driver has no mapping for, shown as the server's text form.
    Unknown(String),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// True for values that should be right-aligned in the grid.
    pub fn is_numeric(&self) -> bool {
        matches!(self, Value::Int(_) | Value::UInt(_) | Value::Float(_) | Value::Decimal(_))
    }

    /// Single-line text for a grid cell. `NULL` is rendered by the caller (it
    /// needs distinct styling), so this returns an empty string for it.
    /// Long values are **not** truncated here; that is a layout decision.
    pub fn display(&self) -> String {
        match self {
            Value::Null => String::new(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::UInt(u) => u.to_string(),
            Value::Float(f) => format_float(*f),
            Value::Decimal(s)
            | Value::Text(s)
            | Value::Json(s)
            | Value::Uuid(s)
            | Value::Date(s)
            | Value::Time(s)
            | Value::Timestamp(s)
            | Value::TimestampTz(s)
            | Value::Interval(s)
            | Value::Unknown(s) => s.clone(),
            Value::Bytes(b) => {
                let mut out = String::with_capacity(2 + b.len().min(32) * 2);
                out.push_str("\\x");
                for byte in b.iter().take(32) {
                    out.push_str(&format!("{byte:02x}"));
                }
                if b.len() > 32 {
                    out.push_str(&format!("… ({} bytes)", b.len()));
                }
                out
            }
            Value::Array(items) => {
                let inner: Vec<String> = items
                    .iter()
                    .map(|v| if v.is_null() { "NULL".to_string() } else { v.display() })
                    .collect();
                format!("{{{}}}", inner.join(", "))
            }
            Value::Geometry(b) => format!("<geometry {} bytes>", b.len()),
        }
    }
}

/// Text for a float that parses back to exactly the same `f64`.
///
/// Rust's `{}` prints the shortest round-trip digits but never switches to an
/// exponent, so `1.5e300` becomes 300 digits and `1e-300` a screenful of zeros.
/// Outside `1e-5 <= |f| < 1e16` we use exponent notation. A whole float keeps a
/// `.0` so a REAL `1.0` stays distinguishable from the INTEGER `1`; `-0.0` keeps
/// its sign because the two zeros differ for the server too.
pub fn format_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    let abs = f.abs();
    if abs != 0.0 && !(1e-5..1e16).contains(&abs) {
        return format!("{f:e}");
    }
    let s = format!("{f}");
    if s.contains('.') { s } else { format!("{s}.0") }
}

/// Coarse type family, used for alignment, icons, editors and export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum TypeKind {
    Bool,
    Integer,
    Float,
    Decimal,
    Text,
    Bytes,
    Json,
    Uuid,
    Date,
    Time,
    Timestamp,
    Interval,
    Array,
    Geometry,
    #[default]
    Other,
}

impl TypeKind {
    /// Coarse family from a SQL type name as typed or printed (`integer`, `varchar(20)`, `int4[]`). Shared
    /// by the drivers, which only get text from SQLite and `format_type`, and by the table editor, whose
    /// draft columns are free text, so both classify a type the same way.
    pub fn from_type_name(name: &str) -> TypeKind {
        let n = name.trim().to_ascii_lowercase();
        if n.ends_with("[]") || n.starts_with('_') {
            return TypeKind::Array;
        }
        let base = n.split(['(', ' ']).next().unwrap_or("");
        match base {
            "bool" | "boolean" => TypeKind::Bool,
            "int" | "integer" | "int2" | "int4" | "int8" | "smallint" | "bigint" | "tinyint"
            | "mediumint" | "oid" | "serial" | "bigserial" | "smallserial" | "year" => TypeKind::Integer,
            "float" | "float4" | "float8" | "real" | "double" => TypeKind::Float,
            "numeric" | "decimal" | "money" | "dec" | "fixed" => TypeKind::Decimal,
            "text" | "varchar" | "char" | "character" | "bpchar" | "name" | "citext" | "clob"
            | "nvarchar" | "nchar" | "tinytext" | "mediumtext" | "longtext" | "string" | "enum" | "set"
            | "xml" => TypeKind::Text,
            "bytea" | "blob" | "binary" | "varbinary" | "tinyblob" | "mediumblob" | "longblob" | "bit" => {
                TypeKind::Bytes
            }
            "json" | "jsonb" => TypeKind::Json,
            "uuid" => TypeKind::Uuid,
            "date" => TypeKind::Date,
            "time" | "timetz" => TypeKind::Time,
            "timestamp" | "timestamptz" | "datetime" => TypeKind::Timestamp,
            "interval" => TypeKind::Interval,
            "geometry" | "geography" | "point" | "linestring" | "polygon" | "multipoint"
            | "multilinestring" | "multipolygon" | "geometrycollection" => TypeKind::Geometry,
            _ => TypeKind::Other,
        }
    }
}

/// Where a result column came from, when the driver can tell. This is what
/// makes a result grid editable: we can only write back cells whose source
/// table (and its primary key) is known.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ColumnOrigin {
    pub schema: Option<String>,
    pub table: String,
    pub column: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    /// The database's own name for the type, e.g. `timestamptz`, `varchar(40)`.
    pub type_name: String,
    pub kind: TypeKind,
    pub origin: Option<ColumnOrigin>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResultSet {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Value>>,
    /// True when the driver stopped at `ExecOptions::row_limit` and more rows exist.
    pub truncated: bool,
}

/// What running one statement produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatementOutcome {
    /// Present for statements that return rows (SELECT, RETURNING, SHOW, ...).
    pub result: Option<ResultSet>,
    /// Present for statements that modify rows.
    pub rows_affected: Option<u64>,
    /// Server-side notices/warnings raised while running (e.g. `RAISE NOTICE`).
    pub notices: Vec<String>,
    /// Wall-clock time measured by the driver, request to last row.
    pub elapsed: Duration,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecOptions {
    /// Stop fetching after this many rows and set `ResultSet::truncated`.
    /// `None` means fetch everything.
    pub row_limit: Option<usize>,
}

impl Default for ExecOptions {
    fn default() -> Self {
        Self { row_limit: Some(1000) }
    }
}

#[cfg(test)]
mod float_tests {
    use super::*;

    fn rt(f: f64) {
        let text = format_float(f);
        let back: f64 = text.parse().unwrap_or_else(|_| panic!("{text} does not parse"));
        assert_eq!(back.to_bits(), f.to_bits(), "{f:?} printed as {text}");
    }

    #[test]
    fn whole_floats_keep_a_point() {
        assert_eq!(Value::Float(1.0).display(), "1.0");
        assert_eq!(Value::Int(1).display(), "1");
        assert_eq!(Value::Float(-250.0).display(), "-250.0");
        assert_eq!(Value::Float(1.5).display(), "1.5");
        assert_eq!(Value::Float(0.1).display(), "0.1");
    }

    #[test]
    fn extreme_magnitudes_use_exponents() {
        assert_eq!(format_float(1.5e300), "1.5e300");
        assert_eq!(format_float(1e-7), "1e-7");
        assert_eq!(format_float(1e16), "1e16");
        assert_eq!(format_float(-2.5e-300), "-2.5e-300");
        assert_eq!(format_float(123456789012345.0), "123456789012345.0");
        assert_eq!(format_float(0.00001), "0.00001");
    }

    #[test]
    fn round_trips() {
        for f in [
            0.0, -0.0, 1.0, 0.1, 0.2 + 0.1, 1.5e300, 1e-300, f64::MAX, f64::MIN_POSITIVE, 5e-324, 1e15,
            9999999999999998.0, 1e16, 123.456, -7.25e-9, f64::EPSILON,
        ] {
            rt(f);
        }
    }

    #[test]
    fn specials() {
        assert_eq!(format_float(f64::NAN), "NaN");
        assert_eq!(format_float(f64::INFINITY), "Infinity");
        assert_eq!(format_float(f64::NEG_INFINITY), "-Infinity");
        assert_eq!(format_float(-0.0), "-0.0");
        assert_eq!(format_float(0.0), "0.0");
        assert!("NaN".parse::<f64>().is_ok_and(f64::is_nan));
        assert_eq!("-Infinity".parse::<f64>(), Ok(f64::NEG_INFINITY));
    }

    #[test]
    fn decimals_are_untouched() {
        assert_eq!(Value::Decimal("1.50".into()).display(), "1.50");
        assert_eq!(
            Value::Decimal("12345678901234567890.123456789".into()).display(),
            "12345678901234567890.123456789"
        );
        assert_eq!(Value::Decimal("1E+3".into()).display(), "1E+3");
    }
}
