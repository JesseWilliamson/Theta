//! Binary-format decoding of PostgreSQL values into [`theta_core::Value`].
//!
//! `tokio-postgres` always requests results in binary format, which is what lets
//! us keep exact numerics and microsecond timestamps without going through
//! floats or a local time zone. This module implements the receive side of the
//! types a database IDE meets in practice; anything else is shown as text when
//! it happens to be valid UTF-8 (enums, citext, ...) and as hex otherwise.

use crate::util::{compact_json, hex};
use std::error::Error;
use theta_core::{TypeKind, Value};
use tokio_postgres::types::{FromSql, Kind, Type};

type DynErr = Box<dyn Error + Sync + Send>;

/// A cell that accepts every PostgreSQL type; see [`decode`].
pub struct Cell(pub Value);

impl<'a> FromSql<'a> for Cell {
    fn from_sql(ty: &Type, raw: &'a [u8]) -> Result<Self, DynErr> {
        // Decoding must never fail a whole query because of one exotic cell.
        Ok(Cell(decode(ty, raw).unwrap_or_else(|_| fallback(raw))))
    }

    fn from_sql_null(_: &Type) -> Result<Self, DynErr> {
        Ok(Cell(Value::Null))
    }

    fn accepts(_: &Type) -> bool {
        true
    }
}

fn fallback(raw: &[u8]) -> Value {
    match std::str::from_utf8(raw) {
        Ok(s) if !s.chars().any(|c| c.is_control() && c != '\n' && c != '\t') => {
            Value::Unknown(s.to_string())
        }
        _ => Value::Unknown(format!("\\x{}", hex(raw))),
    }
}

fn err(msg: &str) -> DynErr {
    msg.to_string().into()
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DynErr> {
        if self.0.len() < n {
            return Err(err("unexpected end of value"));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, DynErr> {
        Ok(self.take(1)?[0])
    }
    fn i16(&mut self) -> Result<i16, DynErr> {
        Ok(i16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u16(&mut self) -> Result<u16, DynErr> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32, DynErr> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, DynErr> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i64(&mut self) -> Result<i64, DynErr> {
        Ok(i64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
}

fn fixed<const N: usize>(raw: &[u8]) -> Result<[u8; N], DynErr> {
    raw.try_into().map_err(|_| err("unexpected value length"))
}

pub fn decode(ty: &Type, raw: &[u8]) -> Result<Value, DynErr> {
    match ty.kind() {
        Kind::Array(inner) => return decode_array(inner, raw),
        Kind::Enum(_) => return Ok(Value::Unknown(std::str::from_utf8(raw)?.to_string())),
        Kind::Domain(inner) => return decode(inner, raw),
        Kind::Range(inner) => return decode_range(inner, raw),
        Kind::Composite(fields) => {
            let types: Vec<Type> = fields.iter().map(|f| f.type_().clone()).collect();
            return decode_composite(&types, raw);
        }
        _ => {}
    }
    Ok(match *ty {
        Type::BOOL => Value::Bool(fixed::<1>(raw)?[0] != 0),
        Type::INT2 => Value::Int(i16::from_be_bytes(fixed(raw)?) as i64),
        Type::INT4 => Value::Int(i32::from_be_bytes(fixed(raw)?) as i64),
        Type::INT8 => Value::Int(i64::from_be_bytes(fixed(raw)?)),
        Type::OID | Type::XID | Type::CID | Type::REGPROC => {
            Value::Int(u32::from_be_bytes(fixed(raw)?) as i64)
        }
        Type::XID8 => Value::UInt(u64::from_be_bytes(fixed(raw)?)),
        Type::FLOAT4 => {
            let f = f32::from_be_bytes(fixed(raw)?);
            // Widening 0.1f32 to f64 would display as 0.10000000149011612; going
            // through the shortest f32 representation keeps what the server prints.
            Value::Float(if f.is_finite() { f.to_string().parse().unwrap_or(f as f64) } else { f as f64 })
        }
        Type::FLOAT8 => Value::Float(f64::from_be_bytes(fixed(raw)?)),
        Type::NUMERIC => Value::Decimal(numeric(raw)?),
        Type::MONEY => {
            let cents = i64::from_be_bytes(fixed(raw)?);
            // `lc_monetary` decides the symbol and scale on the server; the digits
            // are what matter, and the default scale is 2.
            Value::Decimal(format!(
                "{}{}.{:02}",
                if cents < 0 { "-" } else { "" },
                cents.unsigned_abs() / 100,
                cents.unsigned_abs() % 100
            ))
        }
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::UNKNOWN | Type::XML => {
            Value::Text(std::str::from_utf8(raw)?.to_string())
        }
        Type::CHAR => Value::Text((fixed::<1>(raw)?[0] as char).to_string()),
        Type::BYTEA => Value::Bytes(raw.to_vec()),
        Type::JSON => Value::Json(compact_json(std::str::from_utf8(raw)?)),
        Type::JSONB => {
            let (v, body) = raw.split_first().ok_or_else(|| err("empty jsonb"))?;
            if *v != 1 {
                return Err(err("unsupported jsonb version"));
            }
            Value::Json(compact_json(std::str::from_utf8(body)?))
        }
        Type::UUID => Value::Uuid(uuid(&fixed::<16>(raw)?)),
        Type::DATE => Value::Date(date(i32::from_be_bytes(fixed(raw)?))),
        Type::TIME => Value::Time(time_of_day(i64::from_be_bytes(fixed(raw)?))),
        Type::TIMETZ => {
            let mut r = Reader(raw);
            let t = r.i64()?;
            let zone_west = r.i32()?;
            Value::Time(format!("{}{}", time_of_day(t), offset(-zone_west)))
        }
        Type::TIMESTAMP => Value::Timestamp(timestamp(i64::from_be_bytes(fixed(raw)?), false)),
        Type::TIMESTAMPTZ => Value::TimestampTz(timestamp(i64::from_be_bytes(fixed(raw)?), true)),
        Type::INTERVAL => {
            let mut r = Reader(raw);
            let us = r.i64()?;
            let days = r.i32()?;
            let months = r.i32()?;
            Value::Interval(interval(us, days, months))
        }
        Type::INET | Type::CIDR => Value::Unknown(inet(raw)?),
        Type::MACADDR => Value::Unknown(mac(&fixed::<6>(raw)?)),
        Type::MACADDR8 => Value::Unknown(mac(&fixed::<8>(raw)?)),
        Type::BIT | Type::VARBIT => Value::Unknown(bits(raw)?),
        Type::POINT => {
            let mut r = Reader(raw);
            let x = f64::from_bits(r.i64()? as u64);
            let y = f64::from_bits(r.i64()? as u64);
            Value::Unknown(format!("({x},{y})"))
        }
        _ => match ty.name() {
            "geometry" | "geography" => Value::Geometry(raw.to_vec()),
            "hstore" => Value::Unknown(hstore(raw)?),
            _ => fallback(raw),
        },
    })
}

fn uuid(b: &[u8; 16]) -> String {
    let h = hex(b);
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

pub(crate) fn numeric(raw: &[u8]) -> Result<String, DynErr> {
    let mut r = Reader(raw);
    let ndigits = r.i16()? as usize;
    let weight = r.i16()? as i32;
    let sign = r.u16()?;
    let dscale = r.u16()? as usize;
    match sign {
        0xC000 => return Ok("NaN".into()),
        0xD000 => return Ok("Infinity".into()),
        0xF000 => return Ok("-Infinity".into()),
        _ => {}
    }
    let mut digits = Vec::with_capacity(ndigits);
    for _ in 0..ndigits {
        digits.push(r.i16()? as i32);
    }
    let at = |i: i32| -> i32 { if i >= 0 && (i as usize) < digits.len() { digits[i as usize] } else { 0 } };
    let mut out = String::new();
    if sign == 0x4000 {
        out.push('-');
    }
    if weight >= 0 {
        for i in 0..=weight {
            if i == 0 {
                out.push_str(&at(i).to_string());
            } else {
                out.push_str(&format!("{:04}", at(i)));
            }
        }
    } else {
        out.push('0');
    }
    if dscale > 0 {
        let mut frac = String::new();
        let mut k = 0;
        while frac.len() < dscale {
            frac.push_str(&format!("{:04}", at(weight + 1 + k)));
            k += 1;
        }
        frac.truncate(dscale);
        out.push('.');
        out.push_str(&frac);
    }
    Ok(out)
}

/// Days since 1970-01-01 to (year, month, day) in the proleptic Gregorian
/// calendar (Howard Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Days between 1970-01-01 and the PostgreSQL epoch 2000-01-01.
const PG_EPOCH_DAYS: i64 = 10_957;

fn ymd(days_since_unix: i64) -> String {
    let (y, m, d) = civil_from_days(days_since_unix);
    // PostgreSQL has no year 0: year 0 in the astronomical calendar is 1 BC.
    if y <= 0 { format!("{:04}-{m:02}-{d:02} BC", 1 - y) } else { format!("{y:04}-{m:02}-{d:02}") }
}

pub(crate) fn date(days: i32) -> String {
    match days {
        i32::MAX => "infinity".into(),
        i32::MIN => "-infinity".into(),
        d => ymd(d as i64 + PG_EPOCH_DAYS),
    }
}

fn hms(us: i64) -> String {
    let secs = us.div_euclid(1_000_000);
    let frac = us.rem_euclid(1_000_000);
    let mut s = format!("{:02}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60);
    if frac != 0 {
        let f = format!("{frac:06}");
        s.push('.');
        s.push_str(f.trim_end_matches('0'));
    }
    s
}

pub(crate) fn time_of_day(us: i64) -> String {
    hms(us)
}

fn offset(east_secs: i32) -> String {
    let sign = if east_secs < 0 { '-' } else { '+' };
    let a = east_secs.unsigned_abs();
    let (h, m, s) = (a / 3600, (a / 60) % 60, a % 60);
    let mut out = format!("{sign}{h:02}");
    if m != 0 || s != 0 {
        out.push_str(&format!(":{m:02}"));
    }
    if s != 0 {
        out.push_str(&format!(":{s:02}"));
    }
    out
}

pub(crate) fn timestamp(us: i64, tz: bool) -> String {
    match us {
        i64::MAX => return "infinity".into(),
        i64::MIN => return "-infinity".into(),
        _ => {}
    }
    let days = us.div_euclid(86_400_000_000);
    let tod = us.rem_euclid(86_400_000_000);
    let date = ymd(days + PG_EPOCH_DAYS);
    // The BC marker goes after the time in PostgreSQL's output.
    let (date, bc) = match date.strip_suffix(" BC") {
        Some(d) => (d.to_string(), " BC"),
        None => (date, ""),
    };
    // Rendered in UTC: the wire format carries no zone, and the session zone
    // would need a tz database. An explicit offset keeps the value unambiguous.
    format!("{date} {}{}{bc}", hms(tod), if tz { "+00" } else { "" })
}

pub(crate) fn interval(us: i64, days: i32, months: i32) -> String {
    let mut parts: Vec<String> = Vec::new();
    let (years, mons) = (months / 12, months % 12);
    let plural = |n: i32, unit: &str| if n == 1 { format!("{n} {unit}") } else { format!("{n} {unit}s") };
    if years != 0 {
        parts.push(plural(years, "year"));
    }
    if mons != 0 {
        parts.push(plural(mons, "mon"));
    }
    if days != 0 {
        parts.push(plural(days, "day"));
    }
    if us != 0 {
        let any_negative_before = years < 0 || mons < 0 || days < 0;
        let body = hms(us.abs());
        let sign = if us < 0 {
            "-"
        } else if any_negative_before {
            "+"
        } else {
            ""
        };
        parts.push(format!("{sign}{body}"));
    }
    if parts.is_empty() { "00:00:00".into() } else { parts.join(" ") }
}

fn inet(raw: &[u8]) -> Result<String, DynErr> {
    let mut r = Reader(raw);
    let family = r.u8()?;
    let bits = r.u8()?;
    let is_cidr = r.u8()? != 0;
    let n = r.u8()? as usize;
    let addr = r.take(n)?;
    let (text, max) = match (family, n) {
        (2, 4) => (std::net::Ipv4Addr::new(addr[0], addr[1], addr[2], addr[3]).to_string(), 32),
        (3, 16) => (std::net::Ipv6Addr::from(<[u8; 16]>::try_from(addr).unwrap()).to_string(), 128),
        _ => return Err(err("bad inet")),
    };
    Ok(if is_cidr || bits != max { format!("{text}/{bits}") } else { text })
}

fn mac(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(":")
}

fn bits(raw: &[u8]) -> Result<String, DynErr> {
    let mut r = Reader(raw);
    let n = r.i32()? as usize;
    let bytes = r.take(n.div_ceil(8))?;
    Ok((0..n).map(|i| if bytes[i / 8] & (0x80 >> (i % 8)) != 0 { '1' } else { '0' }).collect())
}

fn hstore(raw: &[u8]) -> Result<String, DynErr> {
    let mut r = Reader(raw);
    let n = r.i32()?;
    let mut items = Vec::new();
    for _ in 0..n {
        let kl = r.i32()? as usize;
        let k = std::str::from_utf8(r.take(kl)?)?.to_string();
        let vl = r.i32()?;
        let v = if vl < 0 {
            "NULL".to_string()
        } else {
            format!("\"{}\"", std::str::from_utf8(r.take(vl as usize)?)?)
        };
        items.push(format!("\"{k}\"=>{v}"));
    }
    Ok(items.join(", "))
}

fn decode_array(inner: &Type, raw: &[u8]) -> Result<Value, DynErr> {
    let mut r = Reader(raw);
    let ndim = r.i32()?;
    let _has_null = r.i32()?;
    let _elem_oid = r.u32()?;
    if ndim == 0 {
        return Ok(Value::Array(vec![]));
    }
    let mut dims = Vec::new();
    for _ in 0..ndim {
        dims.push(r.i32()? as usize);
        let _lower_bound = r.i32()?;
    }
    let total: usize = dims.iter().product();
    let mut flat = Vec::with_capacity(total);
    for _ in 0..total {
        let len = r.i32()?;
        if len < 0 {
            flat.push(Value::Null);
        } else {
            let bytes = r.take(len as usize)?;
            flat.push(decode(inner, bytes).unwrap_or_else(|_| fallback(bytes)));
        }
    }
    // Multi-dimensional arrays arrive row-major; rebuild the nesting.
    fn nest(flat: &mut std::vec::IntoIter<Value>, dims: &[usize]) -> Value {
        match dims {
            [] => flat.next().unwrap_or(Value::Null),
            [n, rest @ ..] => Value::Array((0..*n).map(|_| nest(flat, rest)).collect()),
        }
    }
    Ok(nest(&mut flat.into_iter(), &dims))
}

fn quote_if_needed(v: &Value) -> String {
    let s = v.display();
    if s.is_empty()
        || s.chars().any(|c| c.is_whitespace() || matches!(c, ',' | '(' | ')' | '[' | ']' | '"' | '\\'))
    {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

fn decode_range(inner: &Type, raw: &[u8]) -> Result<Value, DynErr> {
    let mut r = Reader(raw);
    let flags = r.u8()?;
    if flags & 0x01 != 0 {
        return Ok(Value::Unknown("empty".into()));
    }
    let mut bound = |present: bool| -> Result<String, DynErr> {
        if !present {
            return Ok(String::new());
        }
        let len = r.i32()? as usize;
        let bytes = r.take(len)?;
        Ok(quote_if_needed(&decode(inner, bytes).unwrap_or_else(|_| fallback(bytes))))
    };
    let lower = bound(flags & 0x08 == 0)?;
    let upper = bound(flags & 0x10 == 0)?;
    Ok(Value::Unknown(format!(
        "{}{},{}{}",
        if flags & 0x02 != 0 { '[' } else { '(' },
        lower,
        upper,
        if flags & 0x04 != 0 { ']' } else { ')' }
    )))
}

fn decode_composite(types: &[Type], raw: &[u8]) -> Result<Value, DynErr> {
    let mut r = Reader(raw);
    let n = r.i32()? as usize;
    let mut parts = Vec::with_capacity(n);
    for i in 0..n {
        let _oid = r.u32()?;
        let len = r.i32()?;
        if len < 0 {
            parts.push(String::new());
            continue;
        }
        let bytes = r.take(len as usize)?;
        let v = match types.get(i) {
            Some(t) => decode(t, bytes).unwrap_or_else(|_| fallback(bytes)),
            None => fallback(bytes),
        };
        parts.push(quote_if_needed(&v));
    }
    Ok(Value::Unknown(format!("({})", parts.join(","))))
}

/// Coarse type family for a column of type `ty`.
pub fn type_kind(ty: &Type) -> TypeKind {
    match ty.kind() {
        Kind::Array(_) => return TypeKind::Array,
        Kind::Domain(inner) => return type_kind(inner),
        _ => {}
    }
    match *ty {
        Type::BOOL => TypeKind::Bool,
        Type::INT2 | Type::INT4 | Type::INT8 | Type::OID | Type::XID | Type::XID8 | Type::CID => {
            TypeKind::Integer
        }
        Type::FLOAT4 | Type::FLOAT8 => TypeKind::Float,
        Type::NUMERIC | Type::MONEY => TypeKind::Decimal,
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::CHAR | Type::XML => TypeKind::Text,
        Type::BYTEA => TypeKind::Bytes,
        Type::JSON | Type::JSONB => TypeKind::Json,
        Type::UUID => TypeKind::Uuid,
        Type::DATE => TypeKind::Date,
        Type::TIME | Type::TIMETZ => TypeKind::Time,
        Type::TIMESTAMP | Type::TIMESTAMPTZ => TypeKind::Timestamp,
        Type::INTERVAL => TypeKind::Interval,
        _ => match ty.name() {
            "geometry" | "geography" => TypeKind::Geometry,
            "citext" => TypeKind::Text,
            _ => TypeKind::Other,
        },
    }
}

/// The type name shown in the grid header: `varchar(40)`, `numeric(10,2)`, `int4[]`.
pub fn display_type_name(ty: &Type, typmod: i32) -> String {
    if let Kind::Array(inner) = ty.kind() {
        return format!("{}[]", display_type_name(inner, typmod));
    }
    let base = ty.name();
    if typmod < 0 {
        return base.to_string();
    }
    match *ty {
        Type::VARCHAR | Type::BPCHAR => format!("{base}({})", typmod - 4),
        Type::NUMERIC => {
            let m = typmod - 4;
            format!("{base}({},{})", (m >> 16) & 0xffff, m & 0xffff)
        }
        Type::TIMESTAMP | Type::TIMESTAMPTZ | Type::TIME | Type::TIMETZ => format!("{base}({typmod})"),
        Type::BIT | Type::VARBIT => format!("{base}({typmod})"),
        _ => base.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(ndigits: i16, weight: i16, sign: u16, dscale: u16, digits: &[i16]) -> Vec<u8> {
        let mut v = Vec::new();
        for x in [ndigits, weight, sign as i16, dscale as i16] {
            v.extend(x.to_be_bytes());
        }
        for d in digits {
            v.extend(d.to_be_bytes());
        }
        v
    }

    #[test]
    fn numeric_text_is_exact() {
        assert_eq!(numeric(&num(2, 0, 0, 2, &[123, 4500])).unwrap(), "123.45");
        assert_eq!(numeric(&num(1, -1, 0, 4, &[1])).unwrap(), "0.0001");
        assert_eq!(numeric(&num(2, 1, 0x4000, 0, &[1, 5])).unwrap(), "-10005");
        assert_eq!(numeric(&num(0, 0, 0, 0, &[])).unwrap(), "0");
        assert_eq!(numeric(&num(0, 0, 0, 3, &[])).unwrap(), "0.000");
        assert_eq!(numeric(&num(1, 1, 0, 0, &[7])).unwrap(), "70000");
        assert_eq!(numeric(&num(0, 0, 0xC000, 0, &[])).unwrap(), "NaN");
    }

    #[test]
    fn dates_and_timestamps() {
        assert_eq!(date(0), "2000-01-01");
        assert_eq!(date(-10_957), "1970-01-01");
        assert_eq!(date(i32::MAX), "infinity");
        assert_eq!(date(8_036), "2022-01-01");
        // 0001-01-01 BC is day -730_119 from 2000-01-01 (year 0 astronomically).
        assert!(date(-730_485).ends_with("BC"));
        assert_eq!(timestamp(0, false), "2000-01-01 00:00:00");
        assert_eq!(timestamp(1_500_000, true), "2000-01-01 00:00:01.5+00");
        assert_eq!(timestamp(-1, false), "1999-12-31 23:59:59.999999");
    }

    #[test]
    fn times_and_offsets() {
        assert_eq!(time_of_day(3_661_000_000), "01:01:01");
        assert_eq!(time_of_day(86_400_000_000), "24:00:00");
        assert_eq!(offset(7200), "+02");
        assert_eq!(offset(-19_800), "-05:30");
    }

    #[test]
    fn intervals_follow_postgres_style() {
        assert_eq!(interval(0, 0, 0), "00:00:00");
        assert_eq!(interval(14_706_000_000, 3, 14), "1 year 2 mons 3 days 04:05:06");
        assert_eq!(interval(-3_600_000_000, 1, 0), "1 day -01:00:00");
        assert_eq!(interval(3_600_000_000, -1, 0), "-1 days +01:00:00");
        assert_eq!(interval(0, 0, 24), "2 years");
    }

    #[test]
    fn uuid_formats() {
        let b =
            [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        assert_eq!(uuid(&b), "12345678-9abc-def0-1122-334455667788");
    }

    #[test]
    fn inet_and_bits() {
        assert_eq!(inet(&[2, 24, 0, 4, 10, 0, 0, 0]).unwrap(), "10.0.0.0/24");
        assert_eq!(inet(&[2, 32, 0, 4, 10, 0, 0, 1]).unwrap(), "10.0.0.1");
        assert_eq!(bits(&[0, 0, 0, 5, 0b1011_0000]).unwrap(), "10110");
    }

    #[test]
    fn arrays_nest_by_dimension() {
        // int4[][] {{1,2},{3,4}}
        let mut raw = Vec::new();
        for x in [2i32, 0, 23, 2, 1, 2, 1] {
            raw.extend(x.to_be_bytes());
        }
        for v in [1i32, 2, 3, 4] {
            raw.extend(4i32.to_be_bytes());
            raw.extend(v.to_be_bytes());
        }
        let v = decode_array(&Type::INT4, &raw).unwrap();
        assert_eq!(
            v,
            Value::Array(vec![
                Value::Array(vec![Value::Int(1), Value::Int(2)]),
                Value::Array(vec![Value::Int(3), Value::Int(4)])
            ])
        );
    }

    #[test]
    fn type_names_include_modifiers() {
        assert_eq!(display_type_name(&Type::VARCHAR, 44), "varchar(40)");
        assert_eq!(display_type_name(&Type::NUMERIC, (10 << 16 | 2) + 4), "numeric(10,2)");
        assert_eq!(display_type_name(&Type::INT4_ARRAY, -1), "int4[]");
    }
}
