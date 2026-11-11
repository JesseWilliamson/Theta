//! Turning one parsed field into a [`Value`] for a target column, with an error that says what is wrong.
//!
//! The conversion is deliberately strict where the server would be lenient or lossy (an out-of-range
//! integer, `2024-02-30`, a `varchar(10)` overflow) because the whole point of validating a file before
//! writing is to learn about the problem with a row number attached, not as one batched INSERT failing.

use crate::infer::ColumnType;
use crate::parser::Field;
use crate::scalar::{NumberKind, is_uuid, parse_bool, parse_date, parse_number, parse_time, parse_timestamp};
use theta_core::{ColumnInfo, DatabaseKind, TypeKind, Value};

/// What a column accepts. Derived from the catalog (existing table) or from an inferred type (new table).
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub db: DatabaseKind,
    pub kind: TypeKind,
    /// Integer width in bits; unsigned columns use the full range of that width.
    pub bits: u8,
    pub unsigned: bool,
    pub max_len: Option<usize>,
    /// `(precision, scale)` of `numeric(p,s)` / `DECIMAL(p,s)`.
    pub precision: Option<(u32, u32)>,
    pub nullable: bool,
    /// The type as the database prints it, for messages.
    pub type_name: String,
}

fn parens(ty: &str) -> Vec<u32> {
    let (Some(a), Some(b)) = (ty.find('('), ty.find(')')) else { return Vec::new() };
    if a >= b {
        return Vec::new();
    }
    ty[a + 1..b].split(',').filter_map(|p| p.trim().parse().ok()).collect()
}

impl Target {
    pub fn from_column(info: &ColumnInfo, db: DatabaseKind) -> Self {
        let ty = info.data_type.to_ascii_lowercase();
        let p = parens(&ty);
        let bits = if db == DatabaseKind::Sqlite {
            64
        } else if ty.contains("tinyint") {
            8
        } else if ty.contains("smallint") || ty.contains("int2") || ty.contains("smallserial") {
            16
        } else if ty.contains("mediumint") {
            24
        } else if ty.contains("bigint") || ty.contains("int8") || ty.contains("bigserial") {
            64
        } else {
            32
        };
        let is_char = ty.contains("char") || ty.contains("varchar");
        Target {
            db,
            kind: info.kind,
            bits,
            unsigned: ty.contains("unsigned"),
            max_len: if is_char { p.first().map(|&n| n as usize) } else { None },
            precision: if matches!(info.kind, TypeKind::Decimal) && !p.is_empty() {
                Some((p[0], p.get(1).copied().unwrap_or(0)))
            } else {
                None
            },
            nullable: info.nullable,
            type_name: info.data_type.clone(),
        }
    }

    pub fn from_inferred(ty: &ColumnType, db: DatabaseKind) -> Self {
        let (kind, bits) = match ty {
            ColumnType::Integer => (TypeKind::Integer, 32),
            ColumnType::BigInt => (TypeKind::Integer, 64),
            ColumnType::Numeric { .. } => (TypeKind::Decimal, 0),
            ColumnType::Float => (TypeKind::Float, 0),
            ColumnType::Boolean => (TypeKind::Bool, 0),
            ColumnType::Date => (TypeKind::Date, 0),
            ColumnType::Timestamp { .. } => (TypeKind::Timestamp, 0),
            ColumnType::Text { .. } => (TypeKind::Text, 0),
        };
        let sql = ty.sql_type(db);
        // The new column's own type decides, not the sample: a VARCHAR(255) picked for a short sample
        // must reject the 300-character value in row 9000 now, not at the server.
        let max_len = if db == DatabaseKind::MySql && matches!(ty, ColumnType::Text { .. }) {
            parens(&sql).first().map(|&n| n as usize)
        } else {
            None
        };
        let precision = if db == DatabaseKind::MySql && matches!(ty, ColumnType::Numeric { .. }) {
            let p = parens(&sql);
            Some((p[0], p[1]))
        } else {
            None
        };
        Target { db, kind, bits, unsigned: false, max_len, precision, nullable: true, type_name: sql }
    }

    pub fn is_text(&self) -> bool {
        matches!(self.kind, TypeKind::Text | TypeKind::Other | TypeKind::Array | TypeKind::Geometry)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConvertOptions {
    /// Unquoted empty fields become NULL (default). When off, they are the empty string (text targets) or
    /// an error (anything else).
    pub empty_as_null: bool,
    /// An unquoted field equal to this text is NULL (e.g. `NULL`, `\N`).
    pub null_token: Option<String>,
    /// Trim spaces around values for non-text targets (always) and text targets (only when set).
    pub trim_text: bool,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self { empty_as_null: true, null_token: None, trim_text: false }
    }
}

/// Why a field cannot be stored in its column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertError {
    pub reason: String,
}

impl std::fmt::Display for ConvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason)
    }
}

fn err<T>(reason: impl Into<String>) -> Result<T, ConvertError> {
    Err(ConvertError { reason: reason.into() })
}

fn shown(s: &str) -> String {
    let t: String = s.chars().take(40).collect();
    if s.chars().count() > 40 { format!("\"{t}…\"") } else { format!("\"{t}\"") }
}

/// Convert `field` for `target`.
pub fn convert(field: &Field, target: &Target, opts: &ConvertOptions) -> Result<Value, ConvertError> {
    let raw = field.text.as_str();
    if !field.quoted {
        if raw.is_empty() && opts.empty_as_null {
            return null_or_err(target);
        }
        if opts.null_token.as_deref().is_some_and(|t| !t.is_empty() && t == raw) {
            return null_or_err(target);
        }
    }
    if raw.is_empty() {
        // An empty field that is not NULL: a string for text columns, otherwise there is nothing to parse.
        // A quoted empty string in a number column can only mean "no value".
        return if target.is_text() {
            Ok(Value::Text(String::new()))
        } else if field.quoted {
            null_or_err(target)
        } else {
            err(format!("empty value for a {} column (enable \"empty fields are NULL\")", target.type_name))
        };
    }
    let v = if target.is_text() && !opts.trim_text { raw } else { raw.trim() };
    match target.kind {
        TypeKind::Text | TypeKind::Other | TypeKind::Array | TypeKind::Geometry | TypeKind::Interval => {
            if let Some(max) = target.max_len {
                let n = v.chars().count();
                if n > max {
                    return err(format!("{n} characters do not fit {} ({max} maximum)", target.type_name));
                }
            }
            Ok(match target.kind {
                TypeKind::Text => Value::Text(v.to_string()),
                TypeKind::Interval => Value::Interval(v.to_string()),
                _ => Value::Unknown(v.to_string()),
            })
        }
        TypeKind::Integer => integer(v, target),
        TypeKind::Float => match parse_number(v) {
            Some(_) => {
                v.parse::<f64>().map(Value::Float).or_else(|_| err(format!("{} is not a number", shown(v))))
            }
            None => err(format!("{} is not a number", shown(v))),
        },
        TypeKind::Decimal => decimal(v, target),
        TypeKind::Bool => match parse_bool(v) {
            Some(b) => Ok(Value::Bool(b)),
            None => match v {
                "1" => Ok(Value::Bool(true)),
                "0" => Ok(Value::Bool(false)),
                _ => err(format!("{} is not a boolean (true/false, yes/no, t/f, 1/0)", shown(v))),
            },
        },
        TypeKind::Date => match parse_date(v) {
            Some(d) => Ok(Value::Date(d)),
            None => err(format!("{} is not a date (expected YYYY-MM-DD)", shown(v))),
        },
        TypeKind::Time => match parse_time(v) {
            Some(t) => Ok(Value::Time(t)),
            None => err(format!("{} is not a time (expected HH:MM[:SS[.fff]])", shown(v))),
        },
        TypeKind::Timestamp => timestamp(v, target),
        TypeKind::Json => match serde_json::from_str::<serde_json::Value>(v) {
            Ok(_) => Ok(Value::Json(v.to_string())),
            Err(e) => err(format!("invalid JSON: {e}")),
        },
        TypeKind::Uuid => {
            if is_uuid(v) {
                Ok(Value::Uuid(v.to_ascii_lowercase()))
            } else {
                err(format!("{} is not a UUID", shown(v)))
            }
        }
        TypeKind::Bytes => bytes(v),
    }
}

fn null_or_err(target: &Target) -> Result<Value, ConvertError> {
    if target.nullable {
        Ok(Value::Null)
    } else {
        err(format!("NULL for a NOT NULL column ({})", target.type_name))
    }
}

fn integer(v: &str, t: &Target) -> Result<Value, ConvertError> {
    let n = parse_number(v);
    let Some(n) = n.filter(|n| n.kind == NumberKind::Integer) else {
        // "5.0" is a number but not an integer: say so instead of "not a number".
        return match parse_number(v) {
            Some(_) => err(format!("{} is not a whole number for {}", shown(v), t.type_name)),
            None => err(format!("{} is not an integer", shown(v))),
        };
    };
    let _ = n;
    let bits = if t.bits == 0 { 64 } else { t.bits as u32 };
    if t.unsigned {
        match v.parse::<u64>() {
            Ok(u) if bits >= 64 || u < (1u64 << bits) => {
                Ok(if u <= i64::MAX as u64 { Value::Int(u as i64) } else { Value::UInt(u) })
            }
            _ => err(format!("{} is out of range for {}", shown(v), t.type_name)),
        }
    } else {
        match v.parse::<i64>() {
            Ok(i) if bits >= 64 || (-(1i64 << (bits - 1))..(1i64 << (bits - 1))).contains(&i) => {
                Ok(Value::Int(i))
            }
            _ => err(format!("{} is out of range for {}", shown(v), t.type_name)),
        }
    }
}

fn decimal(v: &str, t: &Target) -> Result<Value, ConvertError> {
    if parse_number(v).is_none() {
        return err(format!("{} is not a number", shown(v)));
    }
    if let Some((p, s)) = t.precision {
        let allowed = p.saturating_sub(s);
        // Count real integer digits: "0.5" has none, "0012" has two.
        let int_part = v.trim_start_matches(['+', '-']).split(['.', 'e', 'E']).next().unwrap_or("");
        let digits = int_part.trim_start_matches('0').len() as u32;
        if digits > allowed {
            return err(format!(
                "{} has {digits} integer digits; {} allows {allowed}",
                shown(v),
                t.type_name
            ));
        }
    }
    Ok(Value::Decimal(v.strip_prefix('+').unwrap_or(v).to_string()))
}

fn timestamp(v: &str, t: &Target) -> Result<Value, ConvertError> {
    // A bare date is accepted as midnight: spreadsheets export date-only values into timestamp columns.
    let (text, tz) = match parse_timestamp(v) {
        Some(x) => x,
        None => match parse_date(v) {
            Some(d) => (format!("{d} 00:00:00"), false),
            None => return err(format!("{} is not a timestamp (expected YYYY-MM-DD HH:MM:SS)", shown(v))),
        },
    };
    if tz && t.db == DatabaseKind::MySql {
        // MySQL and MariaDB DATETIME literals cannot carry an offset (MariaDB rejects it, MySQL only
        // accepts it from 8.0.19), and converting silently would change the instant.
        if let Some(stripped) = text.strip_suffix("+00:00") {
            return Ok(Value::Timestamp(stripped.to_string()));
        }
        return err(format!(
            "{} has a time zone offset, which a MySQL DATETIME cannot store; convert it to UTC first",
            shown(v)
        ));
    }
    Ok(if tz { Value::TimestampTz(text) } else { Value::Timestamp(text) })
}

fn bytes(v: &str) -> Result<Value, ConvertError> {
    let hex = v.strip_prefix("\\x").or_else(|| v.strip_prefix("0x")).unwrap_or(v);
    if !hex.len().is_multiple_of(2) || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
        return err(format!("{} is not hexadecimal bytes (use \\x… or 0x…)", shown(v)));
    }
    let out =
        (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0)).collect();
    Ok(Value::Bytes(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(s: &str) -> Field {
        Field { text: s.into(), quoted: false }
    }
    fn q(s: &str) -> Field {
        Field { text: s.into(), quoted: true }
    }
    fn t(kind: TypeKind, name: &str) -> Target {
        Target::from_column(
            &ColumnInfo {
                name: "c".into(),
                data_type: name.into(),
                kind,
                nullable: true,
                default: None,
                comment: None,
                geometry: None,
            },
            DatabaseKind::Postgres,
        )
    }
    fn conv(field: Field, target: &Target) -> Result<Value, String> {
        convert(&field, target, &ConvertOptions::default()).map_err(|e| e.reason)
    }

    #[test]
    fn null_handling() {
        let text = t(TypeKind::Text, "text");
        assert_eq!(conv(f(""), &text), Ok(Value::Null));
        assert_eq!(conv(q(""), &text), Ok(Value::Text(String::new())));
        let opts = ConvertOptions { null_token: Some("\\N".into()), ..Default::default() };
        assert_eq!(convert(&f("\\N"), &text, &opts).unwrap(), Value::Null);
        // A quoted token is data.
        assert_eq!(convert(&q("\\N"), &text, &opts).unwrap(), Value::Text("\\N".into()));
        let keep = ConvertOptions { empty_as_null: false, ..Default::default() };
        assert_eq!(convert(&f(""), &text, &keep).unwrap(), Value::Text(String::new()));
        let int = t(TypeKind::Integer, "integer");
        assert!(convert(&f(""), &int, &keep).unwrap_err().reason.contains("empty value"));
        assert_eq!(convert(&q(""), &int, &keep).unwrap(), Value::Null);
    }

    #[test]
    fn not_null_target_rejects_null() {
        let mut target = t(TypeKind::Text, "text");
        target.nullable = false;
        assert!(conv(f(""), &target).unwrap_err().contains("NOT NULL"));
    }

    #[test]
    fn integers_by_width() {
        let small = t(TypeKind::Integer, "smallint");
        assert_eq!(conv(f(" 42 "), &small), Ok(Value::Int(42)));
        assert_eq!(conv(f("007"), &small), Ok(Value::Int(7)));
        assert!(conv(f("40000"), &small).unwrap_err().contains("out of range for smallint"));
        assert!(conv(f("1.5"), &small).unwrap_err().contains("whole number"));
        assert!(conv(f("abc"), &small).unwrap_err().contains("not an integer"));
        let unsigned = t(TypeKind::Integer, "tinyint unsigned");
        assert_eq!(conv(f("255"), &unsigned), Ok(Value::Int(255)));
        assert!(conv(f("-1"), &unsigned).is_err());
        assert!(conv(f("256"), &unsigned).is_err());
        let big = t(TypeKind::Integer, "bigint");
        assert!(conv(f("9223372036854775808"), &big).is_err());
        assert_eq!(conv(f("-9223372036854775808"), &big), Ok(Value::Int(i64::MIN)));
    }

    #[test]
    fn decimals_and_floats() {
        let d = t(TypeKind::Decimal, "numeric(5,2)");
        assert_eq!(conv(f("123.45"), &d), Ok(Value::Decimal("123.45".into())));
        assert_eq!(conv(f("+0.5"), &d), Ok(Value::Decimal("0.5".into())));
        assert!(conv(f("1234.5"), &d).unwrap_err().contains("integer digits"));
        assert!(conv(f("1,5"), &d).is_err());
        let fl = t(TypeKind::Float, "double precision");
        assert_eq!(conv(f("1e3"), &fl), Ok(Value::Float(1000.0)));
        assert!(conv(f("NaN"), &fl).is_err());
    }

    #[test]
    fn booleans() {
        let b = t(TypeKind::Bool, "boolean");
        assert_eq!(conv(f("Yes"), &b), Ok(Value::Bool(true)));
        assert_eq!(conv(f("0"), &b), Ok(Value::Bool(false)));
        assert!(conv(f("2"), &b).unwrap_err().contains("not a boolean"));
    }

    #[test]
    fn temporal_values() {
        assert_eq!(conv(f("2024-02-29"), &t(TypeKind::Date, "date")), Ok(Value::Date("2024-02-29".into())));
        assert!(conv(f("2024-02-30"), &t(TypeKind::Date, "date")).unwrap_err().contains("not a date"));
        let ts = t(TypeKind::Timestamp, "timestamp");
        assert_eq!(conv(f("2024-05-06T07:08:09"), &ts), Ok(Value::Timestamp("2024-05-06 07:08:09".into())));
        assert_eq!(conv(f("2024-05-06"), &ts), Ok(Value::Timestamp("2024-05-06 00:00:00".into())));
        assert_eq!(
            conv(f("2024-05-06 07:08:09+02:00"), &ts),
            Ok(Value::TimestampTz("2024-05-06 07:08:09+02:00".into()))
        );
        assert_eq!(conv(f("12:30"), &t(TypeKind::Time, "time")), Ok(Value::Time("12:30:00".into())));
    }

    #[test]
    fn mysql_datetime_rejects_offsets_but_accepts_utc() {
        let mut ts = t(TypeKind::Timestamp, "datetime");
        ts.db = DatabaseKind::MySql;
        assert_eq!(conv(f("2024-05-06 07:08:09Z"), &ts), Ok(Value::Timestamp("2024-05-06 07:08:09".into())));
        assert!(conv(f("2024-05-06 07:08:09+02:00"), &ts).unwrap_err().contains("offset"));
    }

    #[test]
    fn text_length_and_trim() {
        let v = t(TypeKind::Text, "character varying(5)");
        assert_eq!(conv(f("héllo"), &v), Ok(Value::Text("héllo".into())));
        assert!(conv(f("héllo!"), &v).unwrap_err().contains("6 characters"));
        let text = t(TypeKind::Text, "text");
        assert_eq!(conv(f("  keep  "), &text), Ok(Value::Text("  keep  ".into())));
        let trim = ConvertOptions { trim_text: true, ..Default::default() };
        assert_eq!(convert(&f("  cut  "), &text, &trim).unwrap(), Value::Text("cut".into()));
    }

    #[test]
    fn json_uuid_bytes() {
        assert!(conv(f("{\"a\":1}"), &t(TypeKind::Json, "jsonb")).is_ok());
        assert!(conv(f("{a:1}"), &t(TypeKind::Json, "jsonb")).unwrap_err().contains("invalid JSON"));
        let u = t(TypeKind::Uuid, "uuid");
        assert_eq!(
            conv(f("123E4567-E89B-12D3-A456-426614174000"), &u),
            Ok(Value::Uuid("123e4567-e89b-12d3-a456-426614174000".into()))
        );
        assert!(conv(f("nope"), &u).is_err());
        assert_eq!(conv(f("\\xDEAD"), &t(TypeKind::Bytes, "bytea")), Ok(Value::Bytes(vec![0xDE, 0xAD])));
        assert!(conv(f("xyz"), &t(TypeKind::Bytes, "bytea")).is_err());
    }

    #[test]
    fn inferred_targets_enforce_their_own_limits() {
        let ty = ColumnType::Text { max_len: 5 };
        let t = Target::from_inferred(&ty, DatabaseKind::MySql);
        assert_eq!(t.max_len, Some(255));
        let long = "x".repeat(300);
        assert!(conv(f(&long), &t).is_err());
        let n = Target::from_inferred(&ColumnType::Numeric { precision: 4, scale: 2 }, DatabaseKind::MySql);
        assert_eq!(n.precision, Some((10, 2)));
    }
}
