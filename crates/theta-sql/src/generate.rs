//! SQL text generation: literals, UPDATE/INSERT/DELETE for the editable grid,
//! the "open table" query and best-effort DDL reconstruction.

use crate::keywords::{qualified_name, quote_if_needed};
use theta_core::Family;
use theta_core::{DatabaseKind, Table, TableKind, Value};

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Quote a string literal for the dialect.
fn quote_str(s: &str, kind: DatabaseKind) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    // T-SQL needs the N prefix or a non-ASCII literal goes through the database's code page.
    if kind.family() == Family::SqlServer {
        out.push('N');
    }
    out.push('\'');
    let escapes = matches!(kind.family(), Family::MySql | Family::ClickHouse);
    for c in s.chars() {
        match c {
            '\'' => out.push_str("''"),
            // MySQL and ClickHouse treat backslash as an escape character; the other
            // dialects take it literally (standard_conforming_strings = on).
            '\\' if escapes => out.push_str("\\\\"),
            '\0' if escapes => out.push_str("\\0"),
            _ => out.push(c),
        }
    }
    out.push('\'');
    out
}

fn is_plain_number(s: &str) -> bool {
    let t = s.strip_prefix(['-', '+']).unwrap_or(s);
    !t.is_empty()
        && t.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '.')
        && t.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
        && t.chars().any(|c| c.is_ascii_digit())
}

fn json_text(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::UInt(u) => u.to_string(),
        Value::Float(f) if f.is_finite() => theta_core::value::format_float(*f),
        Value::Decimal(s) if is_plain_number(s) => s.clone(),
        Value::Json(s) => s.clone(),
        Value::Array(items) => format!("[{}]", items.iter().map(json_text).collect::<Vec<_>>().join(",")),
        other => crate::export::json_string(&other.display()),
    }
}

/// A SQL literal for `value`, correctly escaped for the dialect.
///
/// Typed text forms (dates, uuids, json) are emitted as plain string literals:
/// Postgres coerces an untyped literal to the column's type, which is exactly what
/// an UPDATE from the grid needs, and an explicit cast would fail for `json`
/// columns that receive `jsonb` casts and vice versa.
pub fn sql_literal(value: &Value, kind: DatabaseKind) -> String {
    match value {
        Value::Null => "NULL".into(),
        Value::Bool(b) => match kind.family() {
            Family::Sqlite | Family::SqlServer => (if *b { "1" } else { "0" }).into(),
            _ => (if *b { "TRUE" } else { "FALSE" }).into(),
        },
        Value::Int(i) => i.to_string(),
        Value::UInt(u) => u.to_string(),
        Value::Float(f) => {
            if f.is_nan() {
                match kind.family() {
                    Family::Postgres => "'NaN'::float8".into(),
                    Family::DuckDb => "'NaN'::DOUBLE".into(),
                    Family::ClickHouse => "nan".into(),
                    _ => "NULL".into(),
                }
            } else if f.is_infinite() {
                match (kind.family(), *f > 0.0) {
                    (Family::Postgres, true) => "'Infinity'::float8".into(),
                    (Family::Postgres, false) => "'-Infinity'::float8".into(),
                    // SQLite parses an out-of-range real literal as infinity.
                    (Family::Sqlite, true) => "9e999".into(),
                    (Family::Sqlite, false) => "-9e999".into(),
                    (Family::DuckDb, true) => "'Infinity'::DOUBLE".into(),
                    (Family::DuckDb, false) => "'-Infinity'::DOUBLE".into(),
                    (Family::ClickHouse, true) => "inf".into(),
                    (Family::ClickHouse, false) => "-inf".into(),
                    _ => "NULL".into(),
                }
            } else {
                f.to_string()
            }
        }
        Value::Decimal(s) => {
            if is_plain_number(s) {
                s.clone()
            } else {
                quote_str(s, kind)
            }
        }
        Value::Text(s)
        | Value::Json(s)
        | Value::Uuid(s)
        | Value::Date(s)
        | Value::Time(s)
        | Value::Timestamp(s)
        | Value::TimestampTz(s)
        | Value::Interval(s)
        | Value::Unknown(s) => quote_str(s, kind),
        Value::Bytes(b) => match kind.family() {
            Family::Postgres => format!("'\\x{}'::bytea", hex(b)),
            Family::DuckDb | Family::ClickHouse => format!("unhex('{}')", hex(b)),
            Family::SqlServer if b.is_empty() => "CAST('' AS varbinary(max))".into(),
            Family::SqlServer => format!("0x{}", hex(b)),
            _ => format!("X'{}'", hex(b)),
        },
        Value::Array(items) if matches!(kind.family(), Family::DuckDb | Family::ClickHouse) => {
            format!("[{}]", items.iter().map(|v| sql_literal(v, kind)).collect::<Vec<_>>().join(", "))
        }
        Value::Array(items) => match kind.family() {
            Family::Postgres => {
                if items.is_empty() {
                    "'{}'".into()
                } else {
                    format!(
                        "ARRAY[{}]",
                        items.iter().map(|v| sql_literal(v, kind)).collect::<Vec<_>>().join(", ")
                    )
                }
            }
            // No array type in MySQL, SQLite or T-SQL: JSON text is the closest faithful form.
            _ => quote_str(&json_text(value), kind),
        },
        Value::Geometry(b) => match kind.family() {
            Family::Postgres => format!("ST_GeomFromEWKB('\\x{}'::bytea)", hex(b)),
            Family::MySql => mysql_geometry_literal(b),
            Family::Sqlite => format!("GeomFromEWKB(X'{}')", hex(b)),
            // DuckDB's spatial extension is not loaded by Theta (no GEOMETRY support yet); SQL Server's
            // STGeomFromWKB needs the SRID the EWKB carries; ClickHouse has no geometry column type.
            Family::DuckDb | Family::ClickHouse => format!("unhex('{}')", hex(b)),
            Family::SqlServer => {
                let (wkb, srid) = ewkb_to_wkb(b);
                format!("geometry::STGeomFromWKB(0x{}, {srid})", hex(&wkb))
            }
        },
    }
}

/// Splits EWKB into (plain WKB, SRID). The MySQL driver delivers geometries as EWKB
/// (flag `0x2000_0000` in the type word means a 4-byte SRID follows it), but
/// `ST_GeomFromWKB` only understands plain WKB and takes the SRID separately.
/// Bytes that do not look like EWKB with an SRID are passed through with SRID 0.
fn ewkb_to_wkb(b: &[u8]) -> (Vec<u8>, u32) {
    const SRID_FLAG: u32 = 0x2000_0000;
    let (Some(&order), Some(ty)) = (b.first(), b.get(1..5)) else { return (b.to_vec(), 0) };
    let little = match order {
        1 => true,
        0 => false,
        _ => return (b.to_vec(), 0),
    };
    let word = |s: &[u8]| {
        let a: [u8; 4] = s.try_into().unwrap();
        if little { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) }
    };
    let ty = word(ty);
    let Some(srid) = b.get(5..9).filter(|_| ty & SRID_FLAG != 0).map(word) else {
        return (b.to_vec(), 0);
    };
    let plain = ty & !SRID_FLAG;
    let mut out = Vec::with_capacity(b.len() - 4);
    out.push(order);
    out.extend_from_slice(&if little { plain.to_le_bytes() } else { plain.to_be_bytes() });
    out.extend_from_slice(&b[9..]);
    (out, srid)
}

fn mysql_geometry_literal(b: &[u8]) -> String {
    let (wkb, srid) = ewkb_to_wkb(b);
    if srid == 0 {
        format!("ST_GeomFromWKB(X'{}')", hex(&wkb))
    } else {
        format!("ST_GeomFromWKB(X'{}', {srid})", hex(&wkb))
    }
}

fn target(kind: DatabaseKind, schema: Option<&str>, table: &str) -> String {
    qualified_name(schema, table, kind)
}

fn where_clause(kind: DatabaseKind, key: &[(String, Value)]) -> String {
    if key.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = key
        .iter()
        .map(|(c, v)| {
            let c = quote_if_needed(c, kind);
            if v.is_null() { format!("{c} IS NULL") } else { format!("{c} = {}", sql_literal(v, kind)) }
        })
        .collect();
    format!(" WHERE {}", parts.join(" AND "))
}

/// `UPDATE t SET ... WHERE key...`. Key columns holding NULL use `IS NULL`.
/// An empty key produces no WHERE clause; callers must refuse that case themselves.
pub fn build_update(
    kind: DatabaseKind,
    schema: Option<&str>,
    table: &str,
    set: &[(String, Value)],
    key: &[(String, Value)],
) -> String {
    let assignments: Vec<String> =
        set.iter().map(|(c, v)| format!("{} = {}", quote_if_needed(c, kind), sql_literal(v, kind))).collect();
    format!(
        "UPDATE {} SET {}{}",
        target(kind, schema, table),
        assignments.join(", "),
        where_clause(kind, key)
    )
}

/// Multi-row `INSERT`. Rows go on separate lines for readability.
pub fn build_insert(
    kind: DatabaseKind,
    schema: Option<&str>,
    table: &str,
    columns: &[String],
    rows: &[Vec<Value>],
) -> String {
    let t = target(kind, schema, table);
    if columns.is_empty() || rows.is_empty() {
        return match kind.family() {
            Family::MySql => format!("INSERT INTO {t} () VALUES ()"),
            _ => format!("INSERT INTO {t} DEFAULT VALUES"),
        };
    }
    let cols: Vec<String> = columns.iter().map(|c| quote_if_needed(c, kind)).collect();
    let rendered: Vec<String> = rows
        .iter()
        .map(|r| format!("({})", r.iter().map(|v| sql_literal(v, kind)).collect::<Vec<_>>().join(", ")))
        .collect();
    let sep = if rows.len() > 1 { ",\n       " } else { ", " };
    let values = rendered.join(sep);
    format!("INSERT INTO {t} ({}) VALUES {values}", cols.join(", "))
}

pub fn build_delete(
    kind: DatabaseKind,
    schema: Option<&str>,
    table: &str,
    key: &[(String, Value)],
) -> String {
    format!("DELETE FROM {}{}", target(kind, schema, table), where_clause(kind, key))
}

/// The "open table" query.
pub fn select_top(kind: DatabaseKind, schema: Option<&str>, table: &str, limit: usize) -> String {
    if kind.family() == Family::SqlServer {
        // T-SQL has no LIMIT.
        return format!("SELECT TOP {limit} * FROM {}", target(kind, schema, table));
    }
    format!("SELECT * FROM {} LIMIT {limit}", target(kind, schema, table))
}

/// Best-effort `CREATE TABLE` from catalog metadata: columns, primary key, foreign
/// keys, secondary indexes and (Postgres) comments. Check constraints, identity
/// columns and view bodies are not part of the catalog and therefore not emitted.
pub fn table_ddl(kind: DatabaseKind, schema_name: &str, table: &Table) -> String {
    let schema = if schema_name.is_empty() { None } else { Some(schema_name) };
    let name = qualified_name(schema, &table.name, kind);
    if matches!(table.kind, TableKind::View | TableKind::MaterializedView) {
        let cols: Vec<&str> = table.columns.iter().map(|c| c.name.as_str()).collect();
        let what = if table.kind == TableKind::View { "VIEW" } else { "MATERIALIZED VIEW" };
        return format!(
            "-- {what} {name}: the definition is not part of the loaded metadata.\n-- Columns: {}\n",
            cols.join(", ")
        );
    }
    let mut lines: Vec<String> = Vec::new();
    for c in &table.columns {
        let mut l = format!("{} {}", quote_if_needed(&c.name, kind), c.data_type);
        if !c.nullable {
            l.push_str(" NOT NULL");
        }
        if let Some(d) = &c.default {
            l.push_str(&format!(" DEFAULT {d}"));
        }
        if kind.family() == Family::MySql
            && let Some(cm) = &c.comment
        {
            l.push_str(&format!(" COMMENT {}", quote_str(cm, kind)));
        }
        lines.push(l);
    }
    if !table.primary_key.is_empty() {
        let pk_name = table.indexes.iter().find(|i| i.primary).map(|i| i.name.clone());
        let cols = join_cols(&table.primary_key, kind);
        match (kind, pk_name) {
            (DatabaseKind::Postgres, Some(n)) => {
                lines.push(format!("CONSTRAINT {} PRIMARY KEY ({cols})", quote_if_needed(&n, kind)))
            }
            _ => lines.push(format!("PRIMARY KEY ({cols})")),
        }
    }
    for fk in &table.foreign_keys {
        let mut l = format!(
            "CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
            quote_if_needed(&fk.name, kind),
            join_cols(&fk.columns, kind),
            qualified_name(fk.referenced_schema.as_deref(), &fk.referenced_table, kind),
            join_cols(&fk.referenced_columns, kind)
        );
        if let Some(a) = fk.on_delete.as_deref().filter(|a| !a.eq_ignore_ascii_case("NO ACTION")) {
            l.push_str(&format!(" ON DELETE {}", a.to_ascii_uppercase()));
        }
        if let Some(a) = fk.on_update.as_deref().filter(|a| !a.eq_ignore_ascii_case("NO ACTION")) {
            l.push_str(&format!(" ON UPDATE {}", a.to_ascii_uppercase()));
        }
        lines.push(l);
    }
    let mut out = format!("CREATE TABLE {name} (\n");
    out.push_str(&lines.iter().map(|l| format!("    {l}")).collect::<Vec<_>>().join(",\n"));
    out.push_str("\n);\n");
    for idx in table.indexes.iter().filter(|i| !i.primary) {
        if let Some(def) = &idx.definition {
            out.push_str(def.trim_end_matches(';'));
            out.push_str(";\n");
            continue;
        }
        let using = match (&idx.method, kind) {
            (Some(m), DatabaseKind::Postgres) if !m.eq_ignore_ascii_case("btree") => format!(" USING {m}"),
            _ => String::new(),
        };
        out.push_str(&format!(
            "CREATE {}INDEX {} ON {name}{using} ({});\n",
            if idx.unique { "UNIQUE " } else { "" },
            quote_if_needed(&idx.name, kind),
            join_cols(&idx.columns, kind)
        ));
    }
    if kind.family() == Family::Postgres {
        if let Some(c) = &table.comment {
            out.push_str(&format!("COMMENT ON TABLE {name} IS {};\n", quote_str(c, kind)));
        }
        for c in table.columns.iter().filter(|c| c.comment.is_some()) {
            out.push_str(&format!(
                "COMMENT ON COLUMN {name}.{} IS {};\n",
                quote_if_needed(&c.name, kind),
                quote_str(c.comment.as_deref().unwrap_or_default(), kind)
            ));
        }
    }
    out
}

fn join_cols(cols: &[String], kind: DatabaseKind) -> String {
    cols.iter().map(|c| quote_if_needed(c, kind)).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use DatabaseKind::*;

    #[test]
    fn literal_scalars() {
        assert_eq!(sql_literal(&Value::Null, Postgres), "NULL");
        assert_eq!(sql_literal(&Value::Bool(true), Postgres), "TRUE");
        assert_eq!(sql_literal(&Value::Bool(true), Sqlite), "1");
        assert_eq!(sql_literal(&Value::Int(-5), MySql), "-5");
        assert_eq!(sql_literal(&Value::UInt(u64::MAX), MySql), "18446744073709551615");
        assert_eq!(sql_literal(&Value::Float(1.5), Postgres), "1.5");
        assert_eq!(sql_literal(&Value::Decimal("12.50".into()), Postgres), "12.50");
        assert_eq!(sql_literal(&Value::Decimal("NaN".into()), Postgres), "'NaN'");
    }

    #[test]
    fn literal_non_finite_floats() {
        assert_eq!(sql_literal(&Value::Float(f64::NAN), Postgres), "'NaN'::float8");
        assert_eq!(sql_literal(&Value::Float(f64::NEG_INFINITY), Postgres), "'-Infinity'::float8");
        assert_eq!(sql_literal(&Value::Float(f64::INFINITY), Sqlite), "9e999");
        assert_eq!(sql_literal(&Value::Float(f64::NAN), MySql), "NULL");
    }

    #[test]
    fn strings_are_escaped_per_dialect() {
        let v = Value::Text("O'Brien \\ x".into());
        assert_eq!(sql_literal(&v, Postgres), "'O''Brien \\ x'");
        assert_eq!(sql_literal(&v, Sqlite), "'O''Brien \\ x'");
        assert_eq!(sql_literal(&v, MySql), "'O''Brien \\\\ x'");
    }

    #[test]
    fn injection_attempt_stays_inside_the_literal() {
        let v = Value::Text("x'; DROP TABLE t; --".into());
        let lit = sql_literal(&v, Postgres);
        assert_eq!(lit, "'x''; DROP TABLE t; --'");
    }

    #[test]
    fn typed_text_values_are_quoted() {
        assert_eq!(sql_literal(&Value::Uuid("a-b".into()), Postgres), "'a-b'");
        assert_eq!(
            sql_literal(&Value::Timestamp("2024-01-02 03:04:05".into()), Postgres),
            "'2024-01-02 03:04:05'"
        );
        assert_eq!(sql_literal(&Value::Json("{\"a\":\"it's\"}".into()), Postgres), "'{\"a\":\"it''s\"}'");
    }

    #[test]
    fn bytes_and_geometry() {
        assert_eq!(sql_literal(&Value::Bytes(vec![0xde, 0xad]), Postgres), "'\\xdead'::bytea");
        assert_eq!(sql_literal(&Value::Bytes(vec![0xde, 0xad]), MySql), "X'dead'");
        assert_eq!(sql_literal(&Value::Bytes(vec![]), Sqlite), "X''");
        assert_eq!(sql_literal(&Value::Geometry(vec![1, 2]), Postgres), "ST_GeomFromEWKB('\\x0102'::bytea)");
        assert_eq!(sql_literal(&Value::Geometry(vec![1, 2]), MySql), "ST_GeomFromWKB(X'0102')");
    }

    #[test]
    fn mysql_geometry_literal_strips_ewkb_srid() {
        // Little-endian POINT(1 2) with SRID 4326: 01 | 01000020 | E6100000 | x | y
        let ewkb = hex_to_bytes("0101000020E6100000000000000000F03F0000000000000040");
        assert_eq!(
            sql_literal(&Value::Geometry(ewkb), MySql),
            "ST_GeomFromWKB(X'0101000000000000000000f03f0000000000000040', 4326)"
        );
        // Big-endian flavour of the same thing.
        let be = hex_to_bytes("0020000001000010E63FF00000000000004000000000000000");
        assert_eq!(
            sql_literal(&Value::Geometry(be), MySql),
            "ST_GeomFromWKB(X'00000000013ff00000000000004000000000000000', 4326)"
        );
        // SRID 0 arrives as plain WKB and stays a one-argument call.
        let plain = hex_to_bytes("0101000000000000000000F03F0000000000000040");
        assert_eq!(
            sql_literal(&Value::Geometry(plain), MySql),
            "ST_GeomFromWKB(X'0101000000000000000000f03f0000000000000040')"
        );
    }

    fn hex_to_bytes(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn arrays() {
        let a = Value::Array(vec![Value::Int(1), Value::Null, Value::Int(3)]);
        assert_eq!(sql_literal(&a, Postgres), "ARRAY[1, NULL, 3]");
        assert_eq!(sql_literal(&Value::Array(vec![]), Postgres), "'{}'");
        assert_eq!(sql_literal(&a, MySql), "'[1,null,3]'");
        let t = Value::Array(vec![Value::Text("a'b".into())]);
        assert_eq!(sql_literal(&t, Postgres), "ARRAY['a''b']");
    }

    #[test]
    fn update_with_null_key() {
        let sql = build_update(
            Postgres,
            Some("public"),
            "orders",
            &[("status".into(), Value::Text("paid".into())), ("total".into(), Value::Null)],
            &[("id".into(), Value::Int(7)), ("note".into(), Value::Null)],
        );
        assert_eq!(
            sql,
            "UPDATE public.orders SET status = 'paid', total = NULL WHERE id = 7 AND note IS NULL"
        );
    }

    #[test]
    fn identifiers_quoted_when_needed() {
        let sql = build_delete(Postgres, None, "Order Items", &[("Group".into(), Value::Int(1))]);
        assert_eq!(sql, "DELETE FROM \"Order Items\" WHERE \"Group\" = 1");
        let sql = build_delete(MySql, Some("shop"), "order", &[("id".into(), Value::Int(1))]);
        assert_eq!(sql, "DELETE FROM shop.`order` WHERE id = 1");
    }

    #[test]
    fn insert_single_and_multi() {
        let one =
            build_insert(Postgres, None, "t", &["a".into(), "b".into()], &[vec![Value::Int(1), Value::Null]]);
        assert_eq!(one, "INSERT INTO t (a, b) VALUES (1, NULL)");
        let many =
            build_insert(Sqlite, None, "t", &["a".into()], &[vec![Value::Int(1)], vec![Value::Int(2)]]);
        assert_eq!(many, "INSERT INTO t (a) VALUES (1),\n       (2)");
        assert_eq!(build_insert(MySql, None, "t", &[], &[]), "INSERT INTO t () VALUES ()");
        assert_eq!(build_insert(Postgres, None, "t", &[], &[]), "INSERT INTO t DEFAULT VALUES");
    }

    #[test]
    fn select_top_query() {
        assert_eq!(
            select_top(Postgres, Some("public"), "orders", 100),
            "SELECT * FROM public.orders LIMIT 100"
        );
        assert_eq!(select_top(MySql, None, "t", 5), "SELECT * FROM t LIMIT 5");
        assert_eq!(select_top(DatabaseKind::SqlServer, Some("dbo"), "t", 5), "SELECT TOP 5 * FROM dbo.t");
        assert_eq!(select_top(DatabaseKind::ClickHouse, Some("db"), "t", 5), "SELECT * FROM db.t LIMIT 5");
    }

    #[test]
    fn ddl_for_orders_includes_keys_and_indexes() {
        let c = fixtures::shop();
        let orders = c.find_table(Some("public"), "orders").unwrap().1;
        let ddl = table_ddl(Postgres, "public", orders);
        assert!(ddl.starts_with("CREATE TABLE public.orders (\n"), "{ddl}");
        assert!(ddl.contains("    total numeric(10,2)"));
        assert!(ddl.contains("CONSTRAINT orders_pkey PRIMARY KEY (id)"));
        assert!(ddl.contains(
            "CONSTRAINT orders_customer_id_fkey FOREIGN KEY (customer_id) REFERENCES customers (id)"
        ));
        assert!(ddl.contains("CREATE INDEX orders_customer_id_idx ON public.orders (customer_id);"));
        assert!(!ddl.contains("orders_pkey ON"), "primary index is part of the constraint");
    }

    #[test]
    fn ddl_mysql_and_view() {
        let c = fixtures::shop();
        let mut t = c.find_table(None, "customers").unwrap().1.clone();
        t.columns[1].nullable = false;
        t.columns[1].default = Some("''".into());
        let ddl = table_ddl(MySql, "shop", &t);
        assert!(ddl.contains("CREATE TABLE shop.customers"));
        assert!(ddl.contains("email text NOT NULL DEFAULT ''"));
        assert!(ddl.contains("PRIMARY KEY (id)"));
        let v = c.find_table(None, "active_customers").unwrap().1;
        assert!(table_ddl(Postgres, "public", v).starts_with("-- VIEW public.active_customers"));
    }

    #[test]
    fn ddl_uses_stored_index_definition_and_comments() {
        let c = fixtures::shop();
        let mut t = c.find_table(None, "products").unwrap().1.clone();
        t.indexes.push(theta_core::Index {
            name: "products_sku_key".into(),
            columns: vec!["sku".into()],
            unique: true,
            primary: false,
            method: Some("btree".into()),
            definition: Some(
                "CREATE UNIQUE INDEX products_sku_key ON public.products USING btree (sku);".into(),
            ),
            ..Default::default()
        });
        t.comment = Some("Catalog's products".into());
        let ddl = table_ddl(Postgres, "public", &t);
        assert!(ddl.contains("CREATE UNIQUE INDEX products_sku_key ON public.products USING btree (sku);\n"));
        assert!(ddl.contains("COMMENT ON TABLE public.products IS 'Catalog''s products';"));
    }
}
