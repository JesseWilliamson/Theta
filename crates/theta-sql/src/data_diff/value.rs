//! When two cells are the same value, and the order of keys, stated once for both products.
//!
//! The rules are deliberately explicit, because two products print one value in different ways. Always:
//!
//! * `NULL` equals `NULL`, and nothing else.
//! * Numbers compare by value, never by text or scale: `1.50` is `1.5` is `1`.. (`Int`, `UInt`, `Decimal`).
//! * A timestamp's trailing zeros in the fraction do not count (`10:00:00.100` is `10:00:00.1`); a timestamp with a
//!   zone is compared as the same instant in UTC against one without (the same convention the table copy uses).
//! * JSON is compared as JSON (key order and spacing do not count) when both sides parse; otherwise as text.
//! * Everything else of one kind is compared exactly (text, bytes, uuid ignoring case, arrays element by element).
//!
//! Between two *different products* (`Rules::cross_product`) the rules also forgive what a product cannot say:
//!
//! * `true` is `1` and `false` is `0` (MySQL and SQLite have no boolean type);
//! * a number equals text that is that number (`10` and `'10'`);
//! * floating point values agree within a relative 1e-6 (a `real` is not a `double`);
//! * timestamps agree when equal at the *coarser* of the two precisions: PostgreSQL keeps microseconds and a
//!   MariaDB `datetime(3)` keeps milliseconds, so `.123456` is `.123` (truncation, not rounding).
//!
//! Within one product none of those four applies: a value that differs there is a difference.

use crate::copy::to_utc_wall;
use std::cmp::Ordering;
use theta_core::{TypeKind, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rules {
    /// The two sides are different products: forgive what a product cannot represent.
    pub cross_product: bool,
}

/// An exact decimal: sign, integer digits without leading zeros, fraction digits without trailing zeros.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Dec {
    neg: bool,
    int: String,
    frac: String,
}

fn parse_dec(s: &str) -> Option<Dec> {
    let s = s.trim();
    let (neg, body) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    if body.is_empty()
        || !int.chars().all(|c| c.is_ascii_digit())
        || !frac.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    if int.is_empty() && frac.is_empty() {
        return None;
    }
    let int = int.trim_start_matches('0').to_string();
    let frac = frac.trim_end_matches('0').to_string();
    // -0 is 0.
    let neg = neg && !(int.is_empty() && frac.is_empty());
    Some(Dec { neg, int, frac })
}

fn cmp_dec(a: &Dec, b: &Dec) -> Ordering {
    let mag = |a: &Dec, b: &Dec| {
        a.int.len().cmp(&b.int.len()).then_with(|| a.int.cmp(&b.int)).then_with(|| a.frac.cmp(&b.frac))
    };
    match (a.neg, b.neg) {
        (false, true) => Ordering::Greater,
        (true, false) => Ordering::Less,
        (false, false) => mag(a, b),
        (true, true) => mag(b, a),
    }
}

/// The exact decimal of a numeric value (a boolean counts as 0 and 1 only where `bool_ok`).
fn as_dec(v: &Value, bool_ok: bool, text_ok: bool) -> Option<Dec> {
    match v {
        Value::Int(i) => parse_dec(&i.to_string()),
        Value::UInt(u) => parse_dec(&u.to_string()),
        Value::Decimal(s) => parse_dec(s),
        Value::Bool(b) if bool_ok => parse_dec(if *b { "1" } else { "0" }),
        Value::Text(s) if text_ok => parse_dec(s),
        _ => None,
    }
}

fn as_f64(v: &Value, text_ok: bool) -> Option<f64> {
    match v {
        Value::Float(f) => Some(*f),
        Value::Int(i) => Some(*i as f64),
        Value::UInt(u) => Some(*u as f64),
        Value::Decimal(s) => s.trim().parse().ok(),
        Value::Text(s) if text_ok => s.trim().parse().ok(),
        _ => None,
    }
}

fn is_number(v: &Value) -> bool {
    matches!(v, Value::Int(_) | Value::UInt(_) | Value::Decimal(_) | Value::Float(_))
}

/// A timestamp text as `(date and clock without fraction, fraction digits without trailing zeros)`, with a
/// zone converted to UTC.
fn split_time(v: &str) -> (String, String) {
    let wall = to_utc_wall(v.trim());
    let wall = wall.replacen('T', " ", 1);
    match wall.split_once('.') {
        Some((a, f)) => (a.to_string(), f.trim_end_matches('0').to_string()),
        None => (wall, String::new()),
    }
}

fn is_time(v: &Value) -> bool {
    matches!(v, Value::Timestamp(_) | Value::TimestampTz(_) | Value::Date(_) | Value::Time(_))
}

fn time_text(v: &Value) -> &str {
    match v {
        Value::Timestamp(s) | Value::TimestampTz(s) | Value::Date(s) | Value::Time(s) => s,
        _ => "",
    }
}

fn json_equal(a: &str, b: &str) -> bool {
    match (serde_json::from_str::<serde_json::Value>(a), serde_json::from_str::<serde_json::Value>(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

pub fn values_equal(a: &Value, b: &Value, rules: Rules) -> bool {
    use Value::*;
    match (a, b) {
        (Null, Null) => return true,
        (Null, _) | (_, Null) => return false,
        _ => {}
    }
    // Numbers: exact where both are exact, by tolerance where a float is involved.
    let float = matches!(a, Float(_)) || matches!(b, Float(_));
    let lenient = rules.cross_product
        && ((is_number(a) && matches!(b, Text(_) | Bool(_)))
            || (is_number(b) && matches!(a, Text(_) | Bool(_))));
    let numeric = (is_number(a) && is_number(b)) || lenient;
    if numeric {
        if float {
            let (x, y) = (as_f64(a, rules.cross_product), as_f64(b, rules.cross_product));
            return match (x, y) {
                (Some(x), Some(y)) if x.is_nan() && y.is_nan() => true,
                (Some(x), Some(y)) if rules.cross_product => {
                    (x - y).abs() <= 1e-6 * x.abs().max(y.abs()).max(f64::MIN_POSITIVE)
                }
                (Some(x), Some(y)) => x == y,
                _ => false,
            };
        }
        return match (
            as_dec(a, rules.cross_product, rules.cross_product),
            as_dec(b, rules.cross_product, rules.cross_product),
        ) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        };
    }
    if is_time(a) && is_time(b) {
        let ((da, fa), (db, fb)) = (split_time(time_text(a)), split_time(time_text(b)));
        if da != db {
            return false;
        }
        return if rules.cross_product {
            let n = fa.len().min(fb.len());
            fa[..n] == fb[..n]
        } else {
            fa == fb
        };
    }
    match (a, b) {
        (Json(x), Json(y)) | (Json(x), Text(y)) | (Text(x), Json(y)) => json_equal(x, y),
        (Uuid(x), Uuid(y)) | (Uuid(x), Text(y)) | (Text(x), Uuid(y)) => x.eq_ignore_ascii_case(y),
        (Array(x), Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| values_equal(p, q, rules)),
        (Bytes(x), Bytes(y))
        | (Geometry(x), Geometry(y))
        | (Bytes(x), Geometry(y))
        | (Geometry(x), Bytes(y)) => x == y,
        (Bool(x), Bool(y)) => x == y,
        (Text(x), Text(y)) => x == y,
        // Anything else (an interval, a type the driver does not know) is what the server printed.
        _ => {
            let same_kind = std::mem::discriminant(a) == std::mem::discriminant(b);
            let unknown = matches!(a, Unknown(_)) || matches!(b, Unknown(_));
            (same_kind || unknown) && a.display() == b.display()
        }
    }
}

// ------------------------------------------------------------------------------------------ ordering

/// How the values of a key column are ordered on the client. Both sides must put a key column in the same
/// class, or two products' orders cannot be merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyClass {
    Number,
    /// Compared as UTF-8 bytes, which is what a binary collation does (`COLLATE "C"`, `BINARY`).
    Text,
    Time,
    Bytes,
}

pub fn key_class(left: TypeKind, right: TypeKind) -> Result<KeyClass, String> {
    let class = |k: TypeKind| match k {
        TypeKind::Integer | TypeKind::Float | TypeKind::Decimal => Some(KeyClass::Number),
        TypeKind::Text | TypeKind::Uuid => Some(KeyClass::Text),
        TypeKind::Date | TypeKind::Time | TypeKind::Timestamp => Some(KeyClass::Time),
        TypeKind::Bytes => Some(KeyClass::Bytes),
        _ => None,
    };
    match (class(left), class(right)) {
        (Some(a), Some(b)) if a == b => Ok(a),
        (Some(_), Some(_)) => {
            Err("it is a different kind of value on the two sides (for example a number and text)".into())
        }
        _ => Err("a key of this type cannot be ordered the same way by two products".into()),
    }
}

fn text_of(v: &Value) -> String {
    match v {
        Value::Uuid(s) => s.to_ascii_lowercase(),
        other => other.display(),
    }
}

/// The order of two non-NULL key values of `class`. `Equal` means the same key.
pub fn cmp_value(a: &Value, b: &Value, class: KeyClass) -> Ordering {
    match class {
        KeyClass::Number => match (as_dec(a, true, true), as_dec(b, true, true)) {
            (Some(x), Some(y)) => cmp_dec(&x, &y),
            _ => as_f64(a, true).unwrap_or(f64::NAN).total_cmp(&as_f64(b, true).unwrap_or(f64::NAN)),
        },
        KeyClass::Text => text_of(a).as_bytes().cmp(text_of(b).as_bytes()),
        KeyClass::Time => {
            let key = |v: &Value| {
                let (t, f) = split_time(time_text(v));
                format!("{t}.{f}")
            };
            // "10:00:00." and "10:00:00.5": a shorter fraction is the same time with trailing zeros.
            let (x, y) = (key(a), key(b));
            let (xs, xf) = x.split_once('.').unwrap_or((&x, ""));
            let (ys, yf) = y.split_once('.').unwrap_or((&y, ""));
            xs.cmp(ys).then_with(|| xf.cmp(yf))
        }
        KeyClass::Bytes => match (a, b) {
            (Value::Bytes(x), Value::Bytes(y)) => x.cmp(y),
            _ => a.display().cmp(&b.display()),
        },
    }
}

pub fn cmp_keys(a: &[Value], b: &[Value], classes: &[KeyClass]) -> Ordering {
    for ((x, y), c) in a.iter().zip(b).zip(classes) {
        let o = cmp_value(x, y, *c);
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}
