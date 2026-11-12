//! Copy Table: the `CREATE TABLE` for the target, and how every column's type is carried across products.
//!
//! **How the type mapping decides.** A column type is never translated product to product (that would need
//! a table for every pair). Each source type name is first read into a *canonical* type (`Canon`: integer of
//! some width, decimal with precision, text with or without a length, timestamp with or without a zone, ...)
//! by one table per product family (`canon_of`), and each canonical type is then written in the target's own
//! words by one table per target family (`render`). Anything the target has no faithful equivalent for is
//! still given a type (so the copy can run), and the substitution is reported with the reason
//! (`Substitution`) so the dialog can show it before anything is executed. Copying between two tables of the
//! same product (`DatabaseKind` equal) skips the mapping and copies the type text verbatim, which is always
//! the most faithful form.
//!
//! What is copied: column names, mapped types, NOT NULL and the primary key. What is not, and said so in
//! [`CopyPlan::notes`]: defaults, identity and generated columns (their values are copied as plain data),
//! indexes, foreign keys, checks, triggers and comments.

use crate::generate::sql_literal;
use theta_core::{ColumnInfo, DatabaseKind, Family, Table, TypeKind, Value};

/// A column type in no product's words.
#[derive(Debug, Clone, PartialEq)]
pub enum Canon {
    SmallInt,
    Int,
    BigInt,
    Bool,
    Real,
    Double,
    /// Precision and scale when the source gave them.
    Decimal(Option<(u32, u32)>),
    Text,
    VarChar(u32),
    Char(u32),
    Bytes,
    Date,
    Time,
    Timestamp,
    TimestampTz,
    Interval,
    Uuid,
    Json,
    Geometry,
    Array(Box<Canon>),
    /// A type with no canonical form (enums, ranges, user types, ...): the source's own name, for the note.
    Other(String),
}

/// A column whose target type is not an exact equivalent of the source's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Substitution {
    pub column: String,
    pub from: String,
    pub to: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CopyColumn {
    pub name: String,
    pub source_type: String,
    pub target_type: String,
    pub canon: Canon,
    pub nullable: bool,
    /// The target column cannot take the source's value as it is (`Geometry` into a byte column).
    pub bytes_from_geometry: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CopyPlan {
    pub create: String,
    pub columns: Vec<CopyColumn>,
    pub substitutions: Vec<Substitution>,
    /// Things the copy leaves out, and constraints it could not create.
    pub notes: Vec<String>,
    pub verbatim: bool,
}

/// `(base name, parameters)` of a declared type: `"Numeric(10, 2)"` is `("numeric", [10, 2])`, a trailing
/// `[]` or a leading `_` is removed and reported through the bool.
fn parse_type(declared: &str) -> (String, Vec<u32>, bool) {
    let t = declared.trim().to_ascii_lowercase();
    let is_array = t.ends_with("[]") || (t.starts_with('_') && t.len() > 1);
    let t = t.trim_end_matches("[]").trim_start_matches('_').to_string();
    let (base, params) = match t.find('(') {
        Some(i) => {
            let inner = t[i + 1..].split(')').next().unwrap_or("");
            let ps = inner.split(',').filter_map(|p| p.trim().parse::<u32>().ok()).collect();
            // Anything after the closing parenthesis (`timestamp(3) with time zone`) belongs to the name.
            let tail = t.split_once(')').map_or("", |(_, r)| r);
            (format!("{}{}", t[..i].trim(), tail), ps)
        }
        None => (t.clone(), Vec::new()),
    };
    let base = base
        .replace(" unsigned", "")
        .replace(" zerofill", "")
        .replace(" signed", "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (base, params, is_array)
}

/// The canonical form of a source column.
pub fn canon_of(kind: DatabaseKind, c: &ColumnInfo) -> Canon {
    let (base, params, is_array) = parse_type(&c.data_type);
    if is_array || c.kind == TypeKind::Array {
        // Strip the array marker from the name, or the recursion would see it again.
        let name = c.data_type.trim().trim_end_matches("[]");
        let name = if name.starts_with('_') && name.len() > 1 { &name[1..] } else { name };
        let inner = ColumnInfo { data_type: name.to_string(), kind: TypeKind::Other, ..c.clone() };
        return Canon::Array(Box::new(match canon_of(kind, &inner) {
            Canon::Array(i) => *i,
            other => other,
        }));
    }
    let n = |i: usize| params.get(i).copied();
    let family = kind.family();
    // SQLite's declared types are free text; its affinity rules decide what an unknown name stores as.
    if family == Family::Sqlite {
        return sqlite_canon(&base);
    }
    match base.as_str() {
        "smallint" | "int2" | "smallserial" | "tinyint" if !(base == "tinyint" && n(0) == Some(1)) => {
            Canon::SmallInt
        }
        "tinyint" | "bool" | "boolean" => Canon::Bool,
        "integer" | "int" | "int4" | "mediumint" | "serial" | "serial4" => Canon::Int,
        "bigint" | "int8" | "bigserial" | "serial8" => Canon::BigInt,
        "hugeint" | "uhugeint" => Canon::Decimal(Some((38, 0))),
        "real" | "float4" => Canon::Real,
        "float" if family == Family::MySql => Canon::Real,
        "float" | "float8" | "double" | "double precision" => Canon::Double,
        "numeric" | "decimal" | "dec" | "number" => Canon::Decimal(n(0).map(|p| (p, n(1).unwrap_or(0)))),
        "money" => Canon::Decimal(Some((19, 2))),
        "text" | "mediumtext" | "longtext" | "tinytext" | "citext" | "name" | "clob" | "string" => {
            Canon::Text
        }
        "varchar" | "character varying" | "nvarchar" | "varchar2" => match n(0) {
            Some(len) => Canon::VarChar(len),
            None => Canon::Text,
        },
        "char" | "character" | "bpchar" | "nchar" => Canon::Char(n(0).unwrap_or(1)),
        "bytea" | "blob" | "longblob" | "mediumblob" | "tinyblob" | "binary" | "varbinary" | "bit" => {
            Canon::Bytes
        }
        "date" => Canon::Date,
        "time" | "time without time zone" | "timetz" | "time with time zone" => Canon::Time,
        "timestamp" | "timestamp without time zone" | "datetime" | "datetime2" | "smalldatetime" => {
            Canon::Timestamp
        }
        "timestamptz" | "timestamp with time zone" => Canon::TimestampTz,
        "interval" => Canon::Interval,
        "uuid" | "uniqueidentifier" => Canon::Uuid,
        "json" | "jsonb" => Canon::Json,
        "geometry" | "geography" | "point" | "linestring" | "polygon" | "multipoint" | "multilinestring"
        | "multipolygon" | "geometrycollection" => Canon::Geometry,
        _ => fallback(c.kind, &base),
    }
}

/// When the name is unknown, the driver's own classification of the column is the next best evidence.
fn fallback(kind: TypeKind, name: &str) -> Canon {
    match kind {
        TypeKind::Bool => Canon::Bool,
        TypeKind::Integer => Canon::BigInt,
        TypeKind::Float => Canon::Double,
        TypeKind::Decimal => Canon::Decimal(None),
        TypeKind::Text => Canon::Text,
        TypeKind::Bytes => Canon::Bytes,
        TypeKind::Json => Canon::Json,
        TypeKind::Uuid => Canon::Uuid,
        TypeKind::Date => Canon::Date,
        TypeKind::Time => Canon::Time,
        TypeKind::Timestamp => Canon::Timestamp,
        TypeKind::Interval => Canon::Interval,
        TypeKind::Geometry => Canon::Geometry,
        TypeKind::Array | TypeKind::Other => Canon::Other(name.to_string()),
    }
}

/// SQLite: a recognised date or boolean name first, then the five affinity rules in their documented order.
fn sqlite_canon(base: &str) -> Canon {
    match base {
        "boolean" | "bool" => return Canon::Bool,
        "date" => return Canon::Date,
        "datetime" | "timestamp" => return Canon::Timestamp,
        "time" => return Canon::Time,
        "json" => return Canon::Json,
        "uuid" => return Canon::Uuid,
        _ => {}
    }
    if base.contains("int") {
        Canon::BigInt
    } else if base.contains("char") || base.contains("clob") || base.contains("text") {
        Canon::Text
    } else if base.contains("blob") || base.is_empty() {
        Canon::Bytes
    } else if base.contains("real") || base.contains("floa") || base.contains("doub") {
        Canon::Double
    } else {
        Canon::Decimal(None)
    }
}

/// The target's type for `canon`, and the reason when it is not an exact equivalent.
pub fn render(dst: DatabaseKind, canon: &Canon) -> (String, Option<String>) {
    use Canon::*;
    let ok = |s: &str| (s.to_string(), None);
    let sub = |s: &str, why: &str| (s.to_string(), Some(why.to_string()));
    match dst.family() {
        Family::Postgres => match canon {
            SmallInt => ok("smallint"),
            Int => ok("integer"),
            BigInt => ok("bigint"),
            Bool => ok("boolean"),
            Real => ok("real"),
            Double => ok("double precision"),
            Decimal(Some((p, s))) => ok(&format!("numeric({p},{s})")),
            Decimal(None) => ok("numeric"),
            Text => ok("text"),
            VarChar(n) => ok(&format!("varchar({n})")),
            Char(n) => ok(&format!("char({n})")),
            Bytes => ok("bytea"),
            Date => ok("date"),
            Time => ok("time"),
            Timestamp => ok("timestamp"),
            TimestampTz => ok("timestamptz"),
            Interval => ok("interval"),
            Uuid => ok("uuid"),
            Json => ok("jsonb"),
            Geometry => sub(
                "bytea",
                "the geometry is kept as its binary form (EWKB); the spatial type is not recreated",
            ),
            Array(inner) => {
                let (t, why) = render(dst, inner);
                (format!("{t}[]"), why)
            }
            Other(name) => {
                sub("text", &format!("“{name}” has no equivalent here; the values are stored as text"))
            }
        },
        Family::MySql => match canon {
            SmallInt => ok("smallint"),
            Int => ok("int"),
            BigInt => ok("bigint"),
            Bool => ok("boolean"),
            Real => ok("float"),
            Double => ok("double"),
            Decimal(Some((p, s))) if *p <= 65 && *s <= 30 => ok(&format!("decimal({p},{s})")),
            Decimal(Some(_)) => {
                sub("decimal(65,30)", "MySQL allows at most 65 digits, 30 of them after the point")
            }
            Decimal(None) => sub("decimal(38,10)", "the source has no fixed precision; MySQL needs one"),
            Text => ok("longtext"),
            VarChar(n) if *n <= 16_383 => ok(&format!("varchar({n})")),
            VarChar(_) => {
                sub("longtext", "longer than a MySQL VARCHAR can be (16,383 characters in utf8mb4)")
            }
            Char(n) if *n <= 255 => ok(&format!("char({n})")),
            Char(_) => sub("longtext", "longer than a MySQL CHAR can be (255)"),
            Bytes => ok("longblob"),
            Date => ok("date"),
            Time => ok("time(6)"),
            Timestamp => ok("datetime(6)"),
            TimestampTz => sub(
                "datetime(6)",
                "MySQL has no timestamp with time zone; the value is stored as UTC wall-clock time",
            ),
            Interval => sub("varchar(64)", "MySQL has no interval type; the value is stored as text"),
            Uuid => sub("char(36)", "MySQL has no UUID type; the value is stored as 36 characters of text"),
            Json => ok("json"),
            Geometry => sub(
                "longblob",
                "the geometry is kept as its binary form (EWKB); the spatial type is not recreated",
            ),
            Array(_) => sub("json", "MySQL has no array type; the values are stored as JSON"),
            Other(name) => {
                sub("longtext", &format!("“{name}” has no equivalent here; the values are stored as text"))
            }
        },
        Family::Sqlite => match canon {
            SmallInt | Int | BigInt => ok("integer"),
            Bool => sub("integer", "SQLite has no boolean type; true and false are stored as 1 and 0"),
            Real | Double => ok("real"),
            Decimal(_) => sub(
                "numeric",
                "SQLite stores numbers it cannot hold exactly as floating point, so a long decimal can lose digits",
            ),
            Text | VarChar(_) | Char(_) => ok("text"),
            Bytes => ok("blob"),
            Date => sub("text", "SQLite has no date type; the value is stored as ISO text"),
            Time => sub("text", "SQLite has no time type; the value is stored as ISO text"),
            Timestamp => sub("text", "SQLite has no timestamp type; the value is stored as ISO text"),
            TimestampTz => {
                sub("text", "SQLite has no timestamp type; the value is stored as ISO text with its offset")
            }
            Interval => sub("text", "SQLite has no interval type; the value is stored as text"),
            Uuid => sub("text", "SQLite has no UUID type; the value is stored as text"),
            Json => sub(
                "text",
                "SQLite has no JSON type; the value is stored as text (its json_* functions read it)",
            ),
            Geometry => sub(
                "blob",
                "the geometry is kept as its binary form (EWKB); the spatial type is not recreated",
            ),
            Array(_) => sub("text", "SQLite has no array type; the values are stored as JSON text"),
            Other(name) => {
                sub("text", &format!("“{name}” has no equivalent here; the values are stored as text"))
            }
        },
        Family::DuckDb => match canon {
            SmallInt => ok("smallint"),
            Int => ok("integer"),
            BigInt => ok("bigint"),
            Bool => ok("boolean"),
            Real => ok("real"),
            Double => ok("double"),
            Decimal(Some((p, s))) if *p <= 38 => ok(&format!("decimal({p},{s})")),
            Decimal(Some(_)) => sub("decimal(38,10)", "DuckDB allows at most 38 digits"),
            Decimal(None) => sub(
                "decimal(38,10)",
                "the source has no fixed precision; DuckDB defaults to 18 digits, so a precision is chosen",
            ),
            Text | VarChar(_) | Char(_) => ok("varchar"),
            Bytes => ok("blob"),
            Date => ok("date"),
            Time => ok("time"),
            Timestamp => ok("timestamp"),
            TimestampTz => ok("timestamptz"),
            Interval => ok("interval"),
            Uuid => ok("uuid"),
            Json => ok("json"),
            Geometry => sub(
                "blob",
                "the geometry is kept as its binary form (EWKB); the spatial type is not recreated",
            ),
            Array(inner) => {
                let (t, why) = render(dst, inner);
                (format!("{t}[]"), why)
            }
            Other(name) => {
                sub("varchar", &format!("“{name}” has no equivalent here; the values are stored as text"))
            }
        },
        // Refused earlier by `Feature::CopyTable`; kept total so the match cannot panic.
        Family::SqlServer | Family::ClickHouse => {
            sub("text", "this product is not supported as a copy target")
        }
    }
}

/// Drop the `+HH[:MM]` zone of a timestamp text after converting the wall time to UTC. MySQL's DATETIME has
/// no zone and MariaDB rejects an offset in a literal, so the zone has to go; keeping the instant (not the
/// local wall time) is what makes two copies of the same row agree.
pub fn to_utc_wall(text: &str) -> String {
    let t = text.trim();
    let Some(split) = t.find(' ').or_else(|| t.find('T')) else { return t.to_string() };
    let (date, rest) = (&t[..split], &t[split + 1..]);
    let off_at = rest.rfind(['+', '-']).filter(|i| *i >= 5);
    let (clock, offset) = match off_at {
        Some(i) => (&rest[..i], Some(&rest[i..])),
        None => (rest.trim_end_matches('Z'), None),
    };
    let Some(offset) = offset else { return format!("{date} {clock}") };
    let sign = if offset.starts_with('-') { -1i64 } else { 1 };
    let mut o = offset[1..].split(':');
    let (oh, om): (i64, i64) = (
        o.next().and_then(|v| v.parse().ok()).unwrap_or(0),
        o.next().and_then(|v| v.parse().ok()).unwrap_or(0),
    );
    let shift_min = sign * (oh * 60 + om);
    let mut d = date.splitn(3, '-').map(|p| p.parse::<i64>().unwrap_or(0));
    let (y, m, day) = (d.next().unwrap_or(0), d.next().unwrap_or(1) as u32, d.next().unwrap_or(1) as u32);
    let mut c = clock.splitn(3, ':');
    let h: i64 = c.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let mi: i64 = c.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let s = c.next().unwrap_or("00");
    let total = h * 60 + mi - shift_min;
    let days = days_from_civil(y, m, day) + total.div_euclid(1440);
    let minutes = total.rem_euclid(1440);
    let (y2, m2, d2) = civil_from_days(days);
    format!("{y2:04}-{m2:02}-{d2:02} {:02}:{:02}:{s}", minutes / 60, minutes % 60)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn qualified(kind: DatabaseKind, schema: &str, table: &str) -> String {
    if schema.is_empty() {
        kind.quote_ident(table)
    } else {
        format!("{}.{}", kind.quote_ident(schema), kind.quote_ident(table))
    }
}

/// Plan the copy of `table` (from a `src` product) into `dst_schema`.`dst_table` of a `dst` product.
pub fn plan_copy(
    src: DatabaseKind,
    table: &Table,
    dst: DatabaseKind,
    dst_schema: &str,
    dst_table: &str,
) -> Result<CopyPlan, String> {
    for k in [src, dst] {
        if let Some(why) = k.unsupported(theta_core::Feature::CopyTable) {
            return Err(format!("{}: {why}", k.label()));
        }
    }
    if dst_table.trim().is_empty() {
        return Err("Enter a name for the new table.".into());
    }
    if table.columns.is_empty() {
        return Err("The table has no columns to copy.".into());
    }
    let verbatim = src == dst;
    let mut columns = Vec::new();
    let mut substitutions = Vec::new();
    for c in &table.columns {
        let canon = canon_of(src, c);
        let (target_type, why) = if verbatim { (c.data_type.clone(), None) } else { render(dst, &canon) };
        if let Some(reason) = why {
            substitutions.push(Substitution {
                column: c.name.clone(),
                from: c.data_type.clone(),
                to: target_type.clone(),
                reason,
            });
        }
        columns.push(CopyColumn {
            name: c.name.clone(),
            source_type: c.data_type.clone(),
            bytes_from_geometry: !verbatim && matches!(canon, Canon::Geometry),
            target_type,
            canon,
            nullable: c.nullable,
        });
    }
    let mut notes = vec![
        "Defaults, identity and generated columns, indexes, foreign keys, checks and comments are not copied; values are.".to_string(),
    ];
    let mut lines: Vec<String> = columns
        .iter()
        .map(|c| {
            format!(
                "{} {}{}",
                dst.quote_ident(&c.name),
                c.target_type,
                if c.nullable { "" } else { " NOT NULL" }
            )
        })
        .collect();
    if !table.primary_key.is_empty() {
        let keyed: Vec<&CopyColumn> =
            table.primary_key.iter().filter_map(|k| columns.iter().find(|c| &c.name == k)).collect();
        // MySQL cannot index a TEXT, BLOB or JSON column without a prefix length, which a copy cannot choose.
        let unkeyable = dst.family() == Family::MySql
            && keyed.iter().any(|c| {
                matches!(
                    c.canon,
                    Canon::Text
                        | Canon::Bytes
                        | Canon::Json
                        | Canon::Geometry
                        | Canon::Array(_)
                        | Canon::Other(_)
                ) || c.target_type == "longtext"
                    || c.target_type == "longblob"
            });
        if keyed.len() != table.primary_key.len() {
            notes.push("The primary key was not created: a key column is missing from the table.".into());
        } else if unkeyable {
            notes.push("The primary key was not created: MySQL cannot key a TEXT or BLOB column without a prefix length.".into());
        } else {
            lines.push(format!(
                "PRIMARY KEY ({})",
                table.primary_key.iter().map(|k| dst.quote_ident(k)).collect::<Vec<_>>().join(", ")
            ));
        }
    }
    let create = format!(
        "CREATE TABLE {} (\n{}\n)",
        qualified(dst, dst_schema, dst_table),
        lines.iter().map(|l| format!("    {l}")).collect::<Vec<_>>().join(",\n")
    );
    if verbatim {
        notes.push("Both ends are the same product, so the column types are copied as they are.".into());
    }
    Ok(CopyPlan { create, columns, substitutions, notes, verbatim })
}

/// A source value in the form the target column takes.
fn convert(v: &Value, col: &CopyColumn, dst: DatabaseKind) -> Value {
    match v {
        // MySQL (and SQLite) report a boolean as the integer 0 or 1; PostgreSQL and DuckDB refuse an integer
        // for a boolean column, so the value is made a real boolean whenever the column is one.
        Value::Int(i) if col.canon == Canon::Bool && (*i == 0 || *i == 1) => Value::Bool(*i == 1),
        Value::UInt(u) if col.canon == Canon::Bool && *u <= 1 => Value::Bool(*u == 1),
        Value::Geometry(b) if col.bytes_from_geometry => Value::Bytes(b.clone()),
        Value::TimestampTz(s) if dst.family() == Family::MySql => Value::Timestamp(to_utc_wall(s)),
        // A time zone offset on a plain timestamp text (the PostgreSQL driver prints one for timestamptz
        // read as text) cannot be stored by MySQL either.
        Value::Timestamp(s) if dst.family() == Family::MySql && s.contains('+') => {
            Value::Timestamp(to_utc_wall(s))
        }
        other => other.clone(),
    }
}

/// `INSERT` statements for `rows`, at most `per_statement` rows each. Every value is a literal built by
/// [`sql_literal`] for the target product; names are quoted by it.
pub fn insert_statements(
    dst: DatabaseKind,
    schema: &str,
    table: &str,
    columns: &[CopyColumn],
    rows: &[Vec<Value>],
    per_statement: usize,
) -> Vec<String> {
    let target = qualified(dst, schema, table);
    let names = columns.iter().map(|c| dst.quote_ident(&c.name)).collect::<Vec<_>>().join(", ");
    rows.chunks(per_statement.max(1))
        .map(|chunk| {
            let values = chunk
                .iter()
                .map(|r| {
                    let lits = columns
                        .iter()
                        .zip(r)
                        .map(|(c, v)| sql_literal(&convert(v, c, dst), dst))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("({lits})")
                })
                .collect::<Vec<_>>()
                .join(",\n    ");
            format!("INSERT INTO {target} ({names}) VALUES\n    {values}")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, ty: &str, kind: TypeKind, nullable: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            data_type: ty.into(),
            kind,
            nullable,
            default: None,
            comment: None,
            geometry: None,
            identity: None,
            generated: None,
        }
    }

    fn canon(kind: DatabaseKind, ty: &str, tk: TypeKind) -> Canon {
        canon_of(kind, &col("c", ty, tk, true))
    }

    #[test]
    fn source_types_are_read_into_canonical_types() {
        let pg = DatabaseKind::Postgres;
        for (ty, tk, want) in [
            ("integer", TypeKind::Integer, Canon::Int),
            ("int8", TypeKind::Integer, Canon::BigInt),
            ("numeric(12,2)", TypeKind::Decimal, Canon::Decimal(Some((12, 2)))),
            ("numeric", TypeKind::Decimal, Canon::Decimal(None)),
            ("character varying(40)", TypeKind::Text, Canon::VarChar(40)),
            ("varchar(60)", TypeKind::Text, Canon::VarChar(60)),
            ("character varying", TypeKind::Text, Canon::Text),
            ("bpchar", TypeKind::Text, Canon::Char(1)),
            ("timestamp with time zone", TypeKind::Timestamp, Canon::TimestampTz),
            ("timestamp without time zone", TypeKind::Timestamp, Canon::Timestamp),
            ("jsonb", TypeKind::Json, Canon::Json),
            ("uuid", TypeKind::Uuid, Canon::Uuid),
            ("bytea", TypeKind::Bytes, Canon::Bytes),
            ("geometry", TypeKind::Geometry, Canon::Geometry),
            ("order_status", TypeKind::Other, Canon::Other("order_status".into())),
            ("double precision", TypeKind::Float, Canon::Double),
        ] {
            assert_eq!(canon(pg, ty, tk), want, "{ty}");
        }
        assert_eq!(canon(pg, "integer[]", TypeKind::Array), Canon::Array(Box::new(Canon::Int)));
        assert_eq!(canon(pg, "_text", TypeKind::Array), Canon::Array(Box::new(Canon::Text)));
        let my = DatabaseKind::MySql;
        assert_eq!(canon(my, "tinyint(1)", TypeKind::Integer), Canon::Bool);
        assert_eq!(canon(my, "tinyint(4)", TypeKind::Integer), Canon::SmallInt);
        assert_eq!(canon(my, "int(11) unsigned", TypeKind::Integer), Canon::Int);
        assert_eq!(canon(my, "float", TypeKind::Float), Canon::Real, "a MySQL FLOAT is single precision");
        assert_eq!(
            canon(pg, "float", TypeKind::Float),
            Canon::Double,
            "a PostgreSQL float is double precision"
        );
        assert_eq!(canon(my, "datetime(6)", TypeKind::Timestamp), Canon::Timestamp);
        assert_eq!(canon(my, "longtext", TypeKind::Text), Canon::Text);
        assert_eq!(canon(DatabaseKind::DuckDb, "HUGEINT", TypeKind::Integer), Canon::Decimal(Some((38, 0))));
    }

    #[test]
    fn sqlite_declared_types_follow_the_affinity_rules() {
        let s = DatabaseKind::Sqlite;
        for (ty, want) in [
            ("INTEGER", Canon::BigInt),
            ("UNSIGNED BIG INT", Canon::BigInt),
            ("VARCHAR(255)", Canon::Text),
            ("TEXT", Canon::Text),
            ("BLOB", Canon::Bytes),
            ("", Canon::Bytes),
            ("REAL", Canon::Double),
            ("DOUBLE PRECISION", Canon::Double),
            ("NUMERIC", Canon::Decimal(None)),
            ("DECIMAL(10,5)", Canon::Decimal(None)),
            ("BOOLEAN", Canon::Bool),
            ("DATETIME", Canon::Timestamp),
            ("DATE", Canon::Date),
        ] {
            assert_eq!(canon(s, ty, TypeKind::Other), want, "{ty:?}");
        }
    }

    #[test]
    fn the_mapping_table_for_every_target() {
        use Canon::*;
        let cases: &[(Canon, &str, &str, &str, &str)] = &[
            // canon,                  postgres,          mysql,           sqlite,    duckdb
            (Int, "integer", "int", "integer", "integer"),
            (BigInt, "bigint", "bigint", "integer", "bigint"),
            (Bool, "boolean", "boolean", "integer", "boolean"),
            (Double, "double precision", "double", "real", "double"),
            (Decimal(Some((10, 2))), "numeric(10,2)", "decimal(10,2)", "numeric", "decimal(10,2)"),
            (Text, "text", "longtext", "text", "varchar"),
            (VarChar(40), "varchar(40)", "varchar(40)", "text", "varchar"),
            (Bytes, "bytea", "longblob", "blob", "blob"),
            (Timestamp, "timestamp", "datetime(6)", "text", "timestamp"),
            (TimestampTz, "timestamptz", "datetime(6)", "text", "timestamptz"),
            (Uuid, "uuid", "char(36)", "text", "uuid"),
            (Json, "jsonb", "json", "text", "json"),
            (Interval, "interval", "varchar(64)", "text", "interval"),
            (Array(Box::new(Int)), "integer[]", "json", "text", "integer[]"),
        ];
        for (c, pg, my, lite, duck) in cases {
            for (kind, want) in [
                (DatabaseKind::Postgres, pg),
                (DatabaseKind::MySql, my),
                (DatabaseKind::Sqlite, lite),
                (DatabaseKind::DuckDb, duck),
            ] {
                assert_eq!(render(kind, c).0, *want, "{c:?} -> {kind:?}");
            }
        }
    }

    #[test]
    fn only_inexact_equivalents_are_reported_as_substitutions() {
        use Canon::*;
        for (canon, kind, exact) in [
            (Int, DatabaseKind::MySql, true),
            (Uuid, DatabaseKind::MySql, false),
            (Uuid, DatabaseKind::Sqlite, false),
            (Uuid, DatabaseKind::Postgres, true),
            (Bool, DatabaseKind::Sqlite, false),
            (Bool, DatabaseKind::MySql, true),
            (TimestampTz, DatabaseKind::MySql, false),
            (TimestampTz, DatabaseKind::DuckDb, true),
            (Interval, DatabaseKind::MySql, false),
            (Json, DatabaseKind::Sqlite, false),
            (Geometry, DatabaseKind::Postgres, false),
            (Array(Box::new(Text)), DatabaseKind::Postgres, true),
            (Array(Box::new(Text)), DatabaseKind::MySql, false),
            (Other("mood".into()), DatabaseKind::MySql, false),
            (VarChar(50_000), DatabaseKind::MySql, false),
            (Decimal(Some((70, 2))), DatabaseKind::MySql, false),
            (Decimal(None), DatabaseKind::MySql, false),
            (Text, DatabaseKind::Sqlite, true),
        ] {
            let (_, why) = render(kind, &canon);
            assert_eq!(why.is_none(), exact, "{canon:?} -> {kind:?}: {why:?}");
        }
        let (_, why) = render(DatabaseKind::MySql, &Uuid);
        assert!(why.unwrap().contains("36 characters"));
    }

    fn orders() -> Table {
        Table {
            name: "orders".into(),
            columns: vec![
                col("id", "integer", TypeKind::Integer, false),
                col("ref", "uuid", TypeKind::Uuid, true),
                col("placed", "timestamp with time zone", TypeKind::Timestamp, false),
                col("note", "text", TypeKind::Text, true),
            ],
            primary_key: vec!["id".into()],
            ..Default::default()
        }
    }

    #[test]
    fn the_create_statement_uses_the_targets_words_and_quoting() {
        let p = plan_copy(DatabaseKind::Postgres, &orders(), DatabaseKind::MySql, "archive", "orders_copy")
            .unwrap();
        assert_eq!(
            p.create,
            "CREATE TABLE `archive`.`orders_copy` (\n    `id` int NOT NULL,\n    `ref` char(36),\n    `placed` datetime(6) NOT NULL,\n    `note` longtext,\n    PRIMARY KEY (`id`)\n)"
        );
        let cols: Vec<_> = p.substitutions.iter().map(|s| s.column.as_str()).collect();
        assert_eq!(cols, ["ref", "placed"]);
        assert!(!p.verbatim);
        let lite = plan_copy(DatabaseKind::Postgres, &orders(), DatabaseKind::Sqlite, "", "o").unwrap();
        assert!(
            lite.create.starts_with("CREATE TABLE \"o\" (\n    \"id\" integer NOT NULL,"),
            "{}",
            lite.create
        );
    }

    #[test]
    fn the_same_product_copies_types_verbatim_and_says_so() {
        let mut t = orders();
        t.columns.push(col("mood", "order_status", TypeKind::Other, true));
        let p = plan_copy(DatabaseKind::Postgres, &t, DatabaseKind::Postgres, "public", "orders2").unwrap();
        assert!(p.verbatim && p.substitutions.is_empty());
        assert!(p.create.contains("\"mood\" order_status"), "{}", p.create);
        assert!(p.notes.iter().any(|n| n.contains("same product")));
    }

    #[test]
    fn a_mysql_key_on_a_text_column_is_left_out_with_a_note() {
        let t = Table {
            name: "t".into(),
            columns: vec![col("k", "text", TypeKind::Text, false)],
            primary_key: vec!["k".into()],
            ..Default::default()
        };
        let p = plan_copy(DatabaseKind::Postgres, &t, DatabaseKind::MySql, "", "t").unwrap();
        assert!(!p.create.contains("PRIMARY KEY"));
        assert!(p.notes.iter().any(|n| n.contains("primary key was not created")), "{:?}", p.notes);
        let v = Table { columns: vec![col("k", "varchar(20)", TypeKind::Text, false)], ..t };
        let p = plan_copy(DatabaseKind::Postgres, &v, DatabaseKind::MySql, "", "t").unwrap();
        assert!(p.create.contains("PRIMARY KEY (`k`)"));
    }

    #[test]
    fn unsupported_products_and_empty_input_are_refused() {
        assert!(
            plan_copy(DatabaseKind::SqlServer, &orders(), DatabaseKind::Postgres, "", "x")
                .unwrap_err()
                .starts_with("SQL Server: ")
        );
        assert!(plan_copy(DatabaseKind::Postgres, &orders(), DatabaseKind::ClickHouse, "", "x").is_err());
        assert!(plan_copy(DatabaseKind::Postgres, &orders(), DatabaseKind::Sqlite, "", " ").is_err());
        assert!(plan_copy(DatabaseKind::Postgres, &Table::default(), DatabaseKind::Sqlite, "", "x").is_err());
    }

    #[test]
    fn a_timestamp_with_an_offset_becomes_utc_wall_time() {
        assert_eq!(to_utc_wall("2024-03-05 12:00:00+02"), "2024-03-05 10:00:00");
        assert_eq!(to_utc_wall("2024-03-05 01:30:00.5+05:30"), "2024-03-04 20:00:00.5");
        assert_eq!(to_utc_wall("2024-03-01 00:10:00-01"), "2024-03-01 01:10:00");
        assert_eq!(to_utc_wall("2024-12-31 23:30:00-02:00"), "2025-01-01 01:30:00");
        assert_eq!(to_utc_wall("2024-03-05 12:00:00"), "2024-03-05 12:00:00");
        assert_eq!(to_utc_wall("2024-03-05T12:00:00Z"), "2024-03-05 12:00:00");
        assert_eq!(to_utc_wall("2024-03-05"), "2024-03-05");
    }

    #[test]
    fn inserts_are_batched_and_every_value_is_a_target_literal() {
        let p = plan_copy(DatabaseKind::Postgres, &orders(), DatabaseKind::MySql, "d", "o").unwrap();
        let rows = vec![
            vec![
                Value::Int(1),
                Value::Uuid("u-1".into()),
                Value::TimestampTz("2024-03-05 12:00:00+02".into()),
                Value::Text("O'Brien \\ x".into()),
            ],
            vec![
                Value::Int(2),
                Value::Null,
                Value::TimestampTz("2024-03-06 00:00:00+00".into()),
                Value::Null,
            ],
            vec![
                Value::Int(3),
                Value::Null,
                Value::TimestampTz("2024-03-07 00:00:00+00".into()),
                Value::Null,
            ],
        ];
        let stmts = insert_statements(DatabaseKind::MySql, "d", "o", &p.columns, &rows, 2);
        assert_eq!(stmts.len(), 2);
        assert!(stmts[0].starts_with("INSERT INTO `d`.`o` (`id`, `ref`, `placed`, `note`) VALUES\n    (1, 'u-1', '2024-03-05 10:00:00', "), "{}", stmts[0]);
        assert!(!stmts[0].contains("O'Brien"), "the quote is escaped: {}", stmts[0]);
        assert!(stmts[0].contains("(2, NULL, '2024-03-06 00:00:00', NULL)"));
        assert_eq!(stmts[1].matches("\n    (").count(), 1);
    }

    #[test]
    fn an_integer_flag_becomes_a_boolean_for_a_boolean_column() {
        let t = Table {
            name: "f".into(),
            columns: vec![
                col("on", "tinyint(1)", TypeKind::Integer, true),
                col("n", "int(11)", TypeKind::Integer, true),
            ],
            ..Default::default()
        };
        let p = plan_copy(DatabaseKind::MySql, &t, DatabaseKind::Postgres, "", "f").unwrap();
        let s = insert_statements(
            DatabaseKind::Postgres,
            "",
            "f",
            &p.columns,
            &[vec![Value::Int(1), Value::Int(1)]],
            10,
        );
        assert!(s[0].contains("(TRUE, 1)"), "only the boolean column changes: {}", s[0]);
    }

    #[test]
    fn a_geometry_goes_into_a_byte_column_as_bytes() {
        let t = Table {
            name: "g".into(),
            columns: vec![col("shape", "geometry", TypeKind::Geometry, true)],
            ..Default::default()
        };
        let p = plan_copy(DatabaseKind::Postgres, &t, DatabaseKind::Sqlite, "", "g").unwrap();
        assert_eq!(p.substitutions.len(), 1);
        let s = insert_statements(
            DatabaseKind::Sqlite,
            "",
            "g",
            &p.columns,
            &[vec![Value::Geometry(vec![1, 2, 3])]],
            10,
        );
        assert!(s[0].contains("X'010203'") && !s[0].contains("GeomFromEWKB"), "{}", s[0]);
    }
}
