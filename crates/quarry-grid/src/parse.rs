//! Typed text -> `Value` for the inline cell editor.

use quarry_core::{TypeKind, Value};

/// Convert what the user typed into a `Value` for a column of `kind`.
///
/// Empty input is `NULL` for every kind except text (where `''` is a real,
/// distinct value; "Set NULL" is a separate command). The error message is
/// shown inline under the editor, so it says what was expected.
pub fn parse_input(text: &str, kind: TypeKind) -> Result<Value, String> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(match kind {
            TypeKind::Text | TypeKind::Other => Value::Text(text.to_string()),
            _ => Value::Null,
        });
    }
    match kind {
        TypeKind::Integer => parse_integer(t),
        TypeKind::Float => parse_float(t),
        TypeKind::Decimal => parse_decimal(t),
        TypeKind::Bool => parse_bool(t),
        TypeKind::Text => Ok(Value::Text(text.to_string())),
        TypeKind::Other => Ok(Value::Unknown(text.to_string())),
        TypeKind::Json => serde_json::from_str::<serde_json::Value>(t)
            .map(|_| Value::Json(t.to_string()))
            .map_err(|e| format!("Not valid JSON: {e}")),
        TypeKind::Uuid => parse_uuid(t),
        TypeKind::Date => parse_date(t).map(|_| Value::Date(t.to_string())),
        TypeKind::Time => parse_time(t).map(|_| Value::Time(t.to_string())),
        TypeKind::Timestamp => parse_timestamp(t),
        TypeKind::Interval => Ok(Value::Interval(t.to_string())),
        TypeKind::Bytes => parse_bytes(t),
        TypeKind::Array => Err("Arrays can't be edited in the grid".into()),
        TypeKind::Geometry => quarry_geo::parse_any_text(t)
            .map(|f| Value::Geometry(quarry_geo::to_ewkb(&f)))
            .ok_or_else(|| "Expected WKT, EWKT, hex WKB or GeoJSON".to_string()),
    }
}

fn parse_integer(t: &str) -> Result<Value, String> {
    let clean: String = t.chars().filter(|c| *c != '_').collect();
    if let Ok(i) = clean.parse::<i64>() {
        Ok(Value::Int(i))
    } else if let Ok(u) = clean.parse::<u64>() {
        Ok(Value::UInt(u))
    } else {
        Err("Expected a whole number".into())
    }
}

fn parse_float(t: &str) -> Result<Value, String> {
    match t.to_ascii_lowercase().as_str() {
        "nan" => return Ok(Value::Float(f64::NAN)),
        "inf" | "infinity" | "+inf" => return Ok(Value::Float(f64::INFINITY)),
        "-inf" | "-infinity" => return Ok(Value::Float(f64::NEG_INFINITY)),
        _ => {}
    }
    t.parse::<f64>().map(Value::Float).map_err(|_| "Expected a number".into())
}

fn parse_decimal(t: &str) -> Result<Value, String> {
    let body = t.strip_prefix(['-', '+']).unwrap_or(t);
    let (mantissa, exp) = match body.split_once(['e', 'E']) {
        Some((m, e)) => (m, Some(e)),
        None => (body, None),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits_ok = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let exp_ok = exp.is_none_or(|e| {
        let e = e.strip_prefix(['-', '+']).unwrap_or(e);
        !e.is_empty() && digits_ok(e)
    });
    if (int.is_empty() && frac.is_empty()) || !digits_ok(int) || !digits_ok(frac) || !exp_ok {
        return Err("Expected a decimal number".into());
    }
    Ok(Value::Decimal(t.strip_prefix('+').unwrap_or(t).to_string()))
}

fn parse_bool(t: &str) -> Result<Value, String> {
    match t.to_ascii_lowercase().as_str() {
        "true" | "t" | "1" | "yes" | "y" | "on" => Ok(Value::Bool(true)),
        "false" | "f" | "0" | "no" | "n" | "off" => Ok(Value::Bool(false)),
        _ => Err("Expected true or false".into()),
    }
}

fn parse_uuid(t: &str) -> Result<Value, String> {
    let t = t.trim_matches(['{', '}']);
    let hex: String = t.chars().filter(|c| *c != '-').collect();
    let hyphens_ok = t.len() == hex.len()
        || (t.len() == 36 && t.matches('-').count() == 4 && [8, 13, 18, 23].iter().all(|&i| t.as_bytes()[i] == b'-'));
    if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) || !hyphens_ok {
        return Err("Expected a UUID like 123e4567-e89b-12d3-a456-426614174000".into());
    }
    let h = hex.to_ascii_lowercase();
    Ok(Value::Uuid(format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])))
}

fn parse_date(t: &str) -> Result<(), String> {
    let err = || "Expected a date like 2024-03-31".to_string();
    let mut it = t.splitn(3, '-');
    let (y, m, d) = (it.next(), it.next(), it.next());
    let (Some(y), Some(m), Some(d)) = (y, m, d) else { return Err(err()) };
    if y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return Err(err());
    }
    let (y, m, d): (i32, u32, u32) = (
        y.parse().map_err(|_| err())?,
        m.parse().map_err(|_| err())?,
        d.parse().map_err(|_| err())?,
    );
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let max_day = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if leap { 29 } else { 28 },
        _ => return Err("Month must be 01-12".into()),
    };
    if d == 0 || d > max_day { Err(format!("Day must be 01-{max_day} for that month")) } else { Ok(()) }
}

fn parse_time(t: &str) -> Result<(), String> {
    let err = || "Expected a time like 13:45 or 13:45:09.25".to_string();
    // A zone suffix on timetz is accepted but not validated.
    let main = t.split(['+', 'Z', 'z']).next().unwrap_or(t);
    let main = main.rsplit_once('-').map_or(main, |(a, _)| a);
    let mut parts = main.splitn(3, ':');
    let h: u32 = parts.next().and_then(|s| s.parse().ok()).ok_or_else(err)?;
    let m: u32 = parts.next().and_then(|s| s.parse().ok()).ok_or_else(err)?;
    let s = parts.next().unwrap_or("0");
    let (sec, frac) = s.split_once('.').unwrap_or((s, ""));
    let sec: u32 = sec.parse().map_err(|_| err())?;
    if !frac.bytes().all(|b| b.is_ascii_digit()) || h > 24 || m > 59 || sec > 60 {
        return Err(err());
    }
    Ok(())
}

fn parse_timestamp(t: &str) -> Result<Value, String> {
    let err = || "Expected a timestamp like 2024-03-31 13:45:09".to_string();
    let normalised = t.replacen('T', " ", 1);
    let (date, rest) = match normalised.split_once(' ') {
        Some((d, r)) => (d, r.trim()),
        None => (normalised.as_str(), "00:00:00"),
    };
    parse_date(date)?;
    let has_zone = rest.ends_with(['Z', 'z'])
        || rest.rfind(['+', '-']).is_some_and(|i| i > 0 && rest[i + 1..].bytes().all(|b| b.is_ascii_digit() || b == b':'));
    parse_time(rest).map_err(|_| err())?;
    let text = format!("{date} {rest}");
    Ok(if has_zone { Value::TimestampTz(text) } else { Value::Timestamp(text) })
}

fn parse_bytes(t: &str) -> Result<Value, String> {
    let hex = t.strip_prefix("\\x").or_else(|| t.strip_prefix("0x")).unwrap_or(t);
    let hex: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
    if !hex.len().is_multiple_of(2) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Expected hex bytes like \\xdeadbeef".into());
    }
    let bytes = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0)).collect();
    Ok(Value::Bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(text: &str, kind: TypeKind) -> Value {
        parse_input(text, kind).unwrap_or_else(|e| panic!("{text:?} as {kind:?}: {e}"))
    }
    fn bad(text: &str, kind: TypeKind) {
        assert!(parse_input(text, kind).is_err(), "{text:?} as {kind:?} should fail");
    }

    #[test]
    fn integers() {
        assert_eq!(ok(" 42 ", TypeKind::Integer), Value::Int(42));
        assert_eq!(ok("-7", TypeKind::Integer), Value::Int(-7));
        assert_eq!(ok("18446744073709551615", TypeKind::Integer), Value::UInt(u64::MAX));
        assert_eq!(ok("1_000", TypeKind::Integer), Value::Int(1000));
        bad("4.2", TypeKind::Integer);
        bad("abc", TypeKind::Integer);
        bad("99999999999999999999999", TypeKind::Integer);
    }

    #[test]
    fn floats_and_decimals() {
        assert_eq!(ok("2.5", TypeKind::Float), Value::Float(2.5));
        assert!(matches!(ok("NaN", TypeKind::Float), Value::Float(f) if f.is_nan()));
        assert_eq!(ok("-inf", TypeKind::Float), Value::Float(f64::NEG_INFINITY));
        bad("1,5", TypeKind::Float);
        assert_eq!(ok("19.990", TypeKind::Decimal), Value::Decimal("19.990".into()));
        assert_eq!(ok("+5", TypeKind::Decimal), Value::Decimal("5".into()));
        assert_eq!(ok("1.5e3", TypeKind::Decimal), Value::Decimal("1.5e3".into()));
        assert_eq!(ok(".5", TypeKind::Decimal), Value::Decimal(".5".into()));
        bad("1.2.3", TypeKind::Decimal);
        bad("-", TypeKind::Decimal);
        bad("1e", TypeKind::Decimal);
    }

    #[test]
    fn booleans() {
        for t in ["true", "T", "1", "Yes", "on"] {
            assert_eq!(ok(t, TypeKind::Bool), Value::Bool(true));
        }
        for f in ["false", "F", "0", "no"] {
            assert_eq!(ok(f, TypeKind::Bool), Value::Bool(false));
        }
        bad("maybe", TypeKind::Bool);
    }

    #[test]
    fn empty_input() {
        assert_eq!(ok("", TypeKind::Integer), Value::Null);
        assert_eq!(ok("  ", TypeKind::Date), Value::Null);
        assert_eq!(ok("", TypeKind::Text), Value::Text(String::new()));
        assert_eq!(ok("  hi ", TypeKind::Text), Value::Text("  hi ".into()), "text keeps whitespace");
    }

    #[test]
    fn json_validated() {
        assert_eq!(ok(r#" {"a": [1,2]} "#, TypeKind::Json), Value::Json(r#"{"a": [1,2]}"#.into()));
        bad("{oops", TypeKind::Json);
    }

    #[test]
    fn uuid_normalised() {
        assert_eq!(
            ok("123E4567-E89B-12D3-A456-426614174000", TypeKind::Uuid),
            Value::Uuid("123e4567-e89b-12d3-a456-426614174000".into())
        );
        assert_eq!(
            ok("123e4567e89b12d3a456426614174000", TypeKind::Uuid),
            Value::Uuid("123e4567-e89b-12d3-a456-426614174000".into())
        );
        bad("123e4567-e89b-12d3-a456", TypeKind::Uuid);
        bad("zzzzzzzz-e89b-12d3-a456-426614174000", TypeKind::Uuid);
    }

    #[test]
    fn dates_and_times() {
        assert_eq!(ok("2024-02-29", TypeKind::Date), Value::Date("2024-02-29".into()));
        bad("2023-02-29", TypeKind::Date);
        bad("2024-13-01", TypeKind::Date);
        bad("24-1-1", TypeKind::Date);
        assert_eq!(ok("13:45", TypeKind::Time), Value::Time("13:45".into()));
        assert_eq!(ok("13:45:09.25", TypeKind::Time), Value::Time("13:45:09.25".into()));
        bad("25:00", TypeKind::Time);
        bad("noon", TypeKind::Time);
    }

    #[test]
    fn timestamps() {
        assert_eq!(
            ok("2024-03-31T13:45:09", TypeKind::Timestamp),
            Value::Timestamp("2024-03-31 13:45:09".into())
        );
        assert_eq!(ok("2024-03-31", TypeKind::Timestamp), Value::Timestamp("2024-03-31 00:00:00".into()));
        assert_eq!(
            ok("2024-03-31 13:45:09+02", TypeKind::Timestamp),
            Value::TimestampTz("2024-03-31 13:45:09+02".into())
        );
        assert_eq!(
            ok("2024-03-31 13:45:09Z", TypeKind::Timestamp),
            Value::TimestampTz("2024-03-31 13:45:09Z".into())
        );
        bad("2024-03-31 99:00:00", TypeKind::Timestamp);
    }

    #[test]
    fn bytes() {
        assert_eq!(ok("\\xDEad", TypeKind::Bytes), Value::Bytes(vec![0xde, 0xad]));
        assert_eq!(ok("0x00ff", TypeKind::Bytes), Value::Bytes(vec![0, 255]));
        bad("\\xabc", TypeKind::Bytes);
        bad("xyz1", TypeKind::Bytes);
    }

    #[test]
    fn geometry_roundtrip() {
        let v = ok("SRID=4326;POINT(1 2)", TypeKind::Geometry);
        let Value::Geometry(b) = v else { panic!() };
        let f = quarry_geo::parse_wkb(&b).unwrap();
        assert_eq!(f.srid, Some(4326));
        bad("not a geometry", TypeKind::Geometry);
    }

    #[test]
    fn arrays_are_not_editable() {
        bad("{1,2}", TypeKind::Array);
    }
}
