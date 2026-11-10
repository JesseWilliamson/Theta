//! Mapping of MySQL/MariaDB text-protocol cells into [`quarry_core::Value`].
//!
//! The text protocol is used (rather than prepared statements) because it works
//! for every statement the server accepts, and because the server's own text for
//! DECIMAL / DATE / DATETIME / TIME is exactly the canonical form we want. We
//! only parse what has to be typed: integers, floats, bit and geometry.

use crate::util::compact_json;
use mysql_async::Column as MyColumn;
use mysql_async::consts::{ColumnFlags, ColumnType};
use quarry_core::{TypeKind, Value};

/// Character set id 63 is `binary`: the column holds bytes, not text.
const BINARY_CHARSET: u16 = 63;

pub fn is_binary(col: &MyColumn) -> bool {
    col.character_set() == BINARY_CHARSET
}

pub fn type_kind(col: &MyColumn) -> TypeKind {
    use ColumnType::*;
    let flags = col.flags();
    match col.column_type() {
        MYSQL_TYPE_TINY | MYSQL_TYPE_SHORT | MYSQL_TYPE_LONG | MYSQL_TYPE_INT24 | MYSQL_TYPE_LONGLONG
        | MYSQL_TYPE_YEAR => TypeKind::Integer,
        MYSQL_TYPE_FLOAT | MYSQL_TYPE_DOUBLE => TypeKind::Float,
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => TypeKind::Decimal,
        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => TypeKind::Date,
        MYSQL_TYPE_TIME | MYSQL_TYPE_TIME2 => TypeKind::Time,
        MYSQL_TYPE_DATETIME | MYSQL_TYPE_DATETIME2 | MYSQL_TYPE_TIMESTAMP | MYSQL_TYPE_TIMESTAMP2 => {
            TypeKind::Timestamp
        }
        MYSQL_TYPE_JSON => TypeKind::Json,
        MYSQL_TYPE_BIT => {
            if col.column_length() == 1 {
                TypeKind::Bool
            } else {
                TypeKind::Integer
            }
        }
        MYSQL_TYPE_GEOMETRY => TypeKind::Geometry,
        MYSQL_TYPE_ENUM | MYSQL_TYPE_SET => TypeKind::Text,
        MYSQL_TYPE_TINY_BLOB
        | MYSQL_TYPE_MEDIUM_BLOB
        | MYSQL_TYPE_LONG_BLOB
        | MYSQL_TYPE_BLOB
        | MYSQL_TYPE_VARCHAR
        | MYSQL_TYPE_VAR_STRING
        | MYSQL_TYPE_STRING => {
            if flags.contains(ColumnFlags::ENUM_FLAG) || flags.contains(ColumnFlags::SET_FLAG) {
                TypeKind::Text
            } else if is_binary(col) {
                TypeKind::Bytes
            } else {
                TypeKind::Text
            }
        }
        _ => TypeKind::Other,
    }
}

/// The type name as MySQL users write it (`bigint unsigned`, `varchar`, `text`).
pub fn type_name(col: &MyColumn) -> String {
    use ColumnType::*;
    let flags = col.flags();
    let unsigned = flags.contains(ColumnFlags::UNSIGNED_FLAG);
    let int = |n: &str| if unsigned { format!("{n} unsigned") } else { n.to_string() };
    let binary = is_binary(col);
    match col.column_type() {
        MYSQL_TYPE_TINY => int("tinyint"),
        MYSQL_TYPE_SHORT => int("smallint"),
        MYSQL_TYPE_INT24 => int("mediumint"),
        MYSQL_TYPE_LONG => int("int"),
        MYSQL_TYPE_LONGLONG => int("bigint"),
        MYSQL_TYPE_YEAR => "year".into(),
        MYSQL_TYPE_FLOAT => "float".into(),
        MYSQL_TYPE_DOUBLE => "double".into(),
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => "decimal".into(),
        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => "date".into(),
        MYSQL_TYPE_TIME | MYSQL_TYPE_TIME2 => "time".into(),
        MYSQL_TYPE_DATETIME | MYSQL_TYPE_DATETIME2 => "datetime".into(),
        MYSQL_TYPE_TIMESTAMP | MYSQL_TYPE_TIMESTAMP2 => "timestamp".into(),
        MYSQL_TYPE_JSON => "json".into(),
        MYSQL_TYPE_BIT => "bit".into(),
        MYSQL_TYPE_GEOMETRY => "geometry".into(),
        MYSQL_TYPE_ENUM => "enum".into(),
        MYSQL_TYPE_SET => "set".into(),
        MYSQL_TYPE_TINY_BLOB => if binary { "tinyblob" } else { "tinytext" }.into(),
        MYSQL_TYPE_MEDIUM_BLOB => if binary { "mediumblob" } else { "mediumtext" }.into(),
        MYSQL_TYPE_LONG_BLOB => if binary { "longblob" } else { "longtext" }.into(),
        MYSQL_TYPE_BLOB => if binary { "blob" } else { "text" }.into(),
        MYSQL_TYPE_VARCHAR | MYSQL_TYPE_VAR_STRING => if binary { "varbinary" } else { "varchar" }.into(),
        MYSQL_TYPE_STRING => {
            if flags.contains(ColumnFlags::ENUM_FLAG) {
                "enum".into()
            } else if flags.contains(ColumnFlags::SET_FLAG) {
                "set".into()
            } else if binary {
                "binary".into()
            } else {
                "char".into()
            }
        }
        MYSQL_TYPE_NULL => "null".into(),
        other => format!("{other:?}").trim_start_matches("MYSQL_TYPE_").to_ascii_lowercase(),
    }
}

/// Convert one text-protocol cell. `None` is SQL NULL.
pub fn cell(col: &MyColumn, raw: Option<&[u8]>) -> Value {
    use ColumnType::*;
    let Some(raw) = raw else { return Value::Null };
    let text = || String::from_utf8_lossy(raw).into_owned();
    let flags = col.flags();
    match col.column_type() {
        MYSQL_TYPE_TINY | MYSQL_TYPE_SHORT | MYSQL_TYPE_LONG | MYSQL_TYPE_INT24 | MYSQL_TYPE_LONGLONG
        | MYSQL_TYPE_YEAR => {
            let s = text();
            // YEAR carries the unsigned flag but is a calendar year, not a counter.
            if flags.contains(ColumnFlags::UNSIGNED_FLAG) && col.column_type() != MYSQL_TYPE_YEAR {
                s.parse::<u64>().map(Value::UInt).unwrap_or(Value::Unknown(s))
            } else {
                s.parse::<i64>().map(Value::Int).unwrap_or(Value::Unknown(s))
            }
        }
        MYSQL_TYPE_FLOAT | MYSQL_TYPE_DOUBLE => {
            let s = text();
            s.parse::<f64>().map(Value::Float).unwrap_or(Value::Unknown(s))
        }
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => Value::Decimal(text()),
        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => Value::Date(text()),
        MYSQL_TYPE_TIME | MYSQL_TYPE_TIME2 => Value::Time(text()),
        MYSQL_TYPE_DATETIME | MYSQL_TYPE_DATETIME2 | MYSQL_TYPE_TIMESTAMP | MYSQL_TYPE_TIMESTAMP2 => {
            Value::Timestamp(text())
        }
        MYSQL_TYPE_JSON => Value::Json(compact_json(&text())),
        MYSQL_TYPE_BIT => {
            // Big-endian bytes of the bit string; at most 64 bits.
            let n = raw.iter().fold(0u64, |acc, b| (acc << 8) | *b as u64);
            if col.column_length() == 1 { Value::Bool(n != 0) } else { Value::UInt(n) }
        }
        MYSQL_TYPE_GEOMETRY => Value::Geometry(mysql_geometry_to_ewkb(raw)),
        MYSQL_TYPE_NULL => Value::Null,
        MYSQL_TYPE_ENUM | MYSQL_TYPE_SET => Value::Text(text()),
        MYSQL_TYPE_TINY_BLOB
        | MYSQL_TYPE_MEDIUM_BLOB
        | MYSQL_TYPE_LONG_BLOB
        | MYSQL_TYPE_BLOB
        | MYSQL_TYPE_VARCHAR
        | MYSQL_TYPE_VAR_STRING
        | MYSQL_TYPE_STRING => {
            if is_binary(col)
                && !flags.contains(ColumnFlags::ENUM_FLAG)
                && !flags.contains(ColumnFlags::SET_FLAG)
            {
                Value::Bytes(raw.to_vec())
            } else {
                Value::Text(text())
            }
        }
        _ => Value::Unknown(text()),
    }
}

/// MySQL/MariaDB send GEOMETRY as a 4-byte little-endian SRID followed by plain
/// WKB. `quarry-geo` should only have to know one spatial format, so we rewrite
/// that into **EWKB** (the format PostGIS uses): the geometry type word gets the
/// `0x20000000` "has SRID" flag and the SRID is inserted right after it, in the
/// geometry's own byte order. SRID 0 means "unspecified" and yields plain WKB.
pub fn mysql_geometry_to_ewkb(raw: &[u8]) -> Vec<u8> {
    if raw.len() < 9 {
        return raw.to_vec();
    }
    let srid = u32::from_le_bytes(raw[..4].try_into().unwrap());
    let wkb = &raw[4..];
    if srid == 0 {
        return wkb.to_vec();
    }
    let little = match wkb[0] {
        1 => true,
        0 => false,
        _ => return raw.to_vec(),
    };
    let ty = if little {
        u32::from_le_bytes(wkb[1..5].try_into().unwrap())
    } else {
        u32::from_be_bytes(wkb[1..5].try_into().unwrap())
    };
    let (ty, srid) = if little {
        ((ty | 0x2000_0000).to_le_bytes(), srid.to_le_bytes())
    } else {
        ((ty | 0x2000_0000).to_be_bytes(), srid.to_be_bytes())
    };
    let mut out = Vec::with_capacity(wkb.len() + 4);
    out.push(wkb[0]);
    out.extend_from_slice(&ty);
    out.extend_from_slice(&srid);
    out.extend_from_slice(&wkb[5..]);
    out
}

/// The inverse of [`mysql_geometry_to_ewkb`]'s framing, used by tests only.
#[cfg(test)]
pub fn ewkb_srid(ewkb: &[u8]) -> Option<u32> {
    let little = *ewkb.first()? == 1;
    let ty = if little {
        u32::from_le_bytes(ewkb.get(1..5)?.try_into().ok()?)
    } else {
        u32::from_be_bytes(ewkb.get(1..5)?.try_into().ok()?)
    };
    if ty & 0x2000_0000 == 0 {
        return None;
    }
    let b: [u8; 4] = ewkb.get(5..9)?.try_into().ok()?;
    Some(if little { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_with_srid_becomes_ewkb() {
        // SRID 4326, POINT(1 2), little endian WKB.
        let mut raw = 4326u32.to_le_bytes().to_vec();
        raw.push(1);
        raw.extend(1u32.to_le_bytes());
        raw.extend(1f64.to_le_bytes());
        raw.extend(2f64.to_le_bytes());
        let ewkb = mysql_geometry_to_ewkb(&raw);
        assert_eq!(ewkb.len(), raw.len());
        assert_eq!(ewkb[0], 1);
        assert_eq!(u32::from_le_bytes(ewkb[1..5].try_into().unwrap()), 1 | 0x2000_0000);
        assert_eq!(ewkb_srid(&ewkb), Some(4326));
        assert_eq!(&ewkb[9..], &raw[9..]);
    }

    #[test]
    fn srid_zero_yields_plain_wkb() {
        let mut raw = 0u32.to_le_bytes().to_vec();
        raw.extend([1, 1, 0, 0, 0]);
        raw.extend([0u8; 16]);
        assert_eq!(mysql_geometry_to_ewkb(&raw), raw[4..].to_vec());
    }
}
