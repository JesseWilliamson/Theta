//! Catalog introspection with a fixed number of set-based queries (never one
//! per table), so opening a database with thousands of tables stays fast.

use super::query_error;
use crate::util::kind_from_type_name;
use std::collections::HashMap;
use theta_core::*;
use tokio_postgres::Client;

const SYSTEM_SCHEMAS: &str = "n.nspname NOT IN ('pg_catalog', 'information_schema') \
     AND n.nspname NOT LIKE 'pg\\_toast%' AND n.nspname NOT LIKE 'pg\\_temp\\_%'";

async fn q(client: &Client, sql: &str) -> Result<Vec<tokio_postgres::Row>> {
    client.query(sql, &[]).await.map_err(|e| query_error(e, sql, 0))
}

fn action(c: &str) -> Option<String> {
    match c {
        "c" => Some("CASCADE".into()),
        "r" => Some("RESTRICT".into()),
        "n" => Some("SET NULL".into()),
        "d" => Some("SET DEFAULT".into()),
        _ => None, // 'a' = NO ACTION, the default
    }
}

/// `CHECK ((price > (0)::numeric))` to `(price > (0)::numeric)`'s inner expression: the keyword and the one
/// pair of parentheses that wraps the whole expression come off. `NOT VALID` and the like stay as a suffix.
pub(crate) fn strip_check(def: &str) -> String {
    let d = def.trim();
    let Some(rest) = d.strip_prefix("CHECK") else { return d.to_string() };
    let rest = rest.trim_start();
    if !rest.starts_with('(') {
        return rest.to_string();
    }
    // Find the parenthesis that closes the first one.
    let mut depth = 0usize;
    let mut in_str = false;
    for (i, c) in rest.char_indices() {
        match c {
            '\'' => in_str = !in_str,
            '(' if !in_str => depth += 1,
            ')' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    let inner = &rest[1..i];
                    let tail = rest[i + 1..].trim();
                    return if tail.is_empty() { inner.to_string() } else { format!("{inner} {tail}") };
                }
            }
            _ => {}
        }
    }
    rest.to_string()
}

/// `geometry(Point,4326)` / `geography(MultiPolygon,4326)` / bare `geometry`.
pub(crate) fn parse_geometry_type(formatted: &str) -> Option<GeometryColumn> {
    let base = formatted.split('(').next()?.trim();
    let base = base.rsplit('.').next()?.trim_matches('"');
    let geography = match base {
        "geometry" => false,
        "geography" => true,
        _ => return None,
    };
    let (geometry_type, srid) = match formatted.split_once('(') {
        Some((_, rest)) => {
            let inner = rest.trim_end_matches(')');
            let mut parts = inner.split(',');
            let t = parts.next().unwrap_or("").trim().to_ascii_uppercase();
            let srid = parts.next().and_then(|s| s.trim().parse::<i32>().ok()).filter(|s| *s != 0);
            (if t.is_empty() { "GEOMETRY".to_string() } else { t }, srid)
        }
        None => ("GEOMETRY".to_string(), None),
    };
    Some(GeometryColumn { geometry_type, srid, geography })
}

fn kind_of_formatted(formatted: &str) -> TypeKind {
    // Drop a schema qualifier (`public.citext`) but only from the type name itself.
    let head_end = formatted.find('(').unwrap_or(formatted.len());
    let head = &formatted[..head_end];
    let unqualified = match head.rfind('.') {
        Some(i) if !head[..i].contains(' ') => {
            format!("{}{}", head[i + 1..].trim_matches('"'), &formatted[head_end..])
        }
        _ => formatted.to_string(),
    };
    kind_from_type_name(&unqualified)
}

pub async fn introspect(client: &Client) -> Result<Catalog> {
    let schema_rows =
        q(client, &format!("SELECT n.nspname::text FROM pg_namespace n WHERE {SYSTEM_SCHEMAS} ORDER BY 1"))
            .await?;
    let mut schemas: Vec<Schema> =
        schema_rows.iter().map(|r| Schema { name: r.get(0), ..Default::default() }).collect();
    let schema_index: HashMap<String, usize> =
        schemas.iter().enumerate().map(|(i, s)| (s.name.clone(), i)).collect();

    // Relations. reltuples is -1 for never-analysed tables (PG14+); 0 means empty.
    let rels = q(
        client,
        &format!(
            "SELECT c.oid, n.nspname::text, c.relname::text, c.relkind::text, c.reltuples::float8, \
                    obj_description(c.oid, 'pg_class'), \
                    CASE WHEN c.relkind IN ('v','m') THEN pg_get_viewdef(c.oid, true) END \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relkind IN ('r','v','m','f','p') AND {SYSTEM_SCHEMAS} \
             ORDER BY n.nspname, c.relname"
        ),
    )
    .await?;
    let mut by_oid: HashMap<u32, (usize, usize)> = HashMap::new();
    for r in &rels {
        let oid: u32 = r.get(0);
        let schema: String = r.get(1);
        let Some(&si) = schema_index.get(&schema) else { continue };
        let kind = match r.get::<_, String>(3).as_str() {
            "v" => TableKind::View,
            "m" => TableKind::MaterializedView,
            "f" => TableKind::ForeignTable,
            _ => TableKind::Table,
        };
        let est: f64 = r.get(4);
        let tables = &mut schemas[si].tables;
        tables.push(Table {
            name: r.get(2),
            kind,
            columns: vec![],
            primary_key: vec![],
            foreign_keys: vec![],
            indexes: vec![],
            row_estimate: (est >= 0.0).then_some(est as u64),
            comment: r.get(5),
            view_definition: r.get(6),
            ..Default::default()
        });
        by_oid.insert(oid, (si, tables.len() - 1));
    }

    // `attgenerated` exists from PostgreSQL 12; older servers simply have no generated columns.
    let generated = match q(client, "SELECT current_setting('server_version_num')::int").await?.first() {
        Some(r) if r.get::<_, i32>(0) >= 120000 => "a.attgenerated::text",
        _ => "''::text",
    };
    let cols = q(
        client,
        &format!(
            "SELECT a.attrelid, a.attname::text, format_type(a.atttypid, a.atttypmod), \
                    format_type(CASE WHEN t.typtype = 'd' THEN t.typbasetype ELSE a.atttypid END, a.atttypmod), \
                    a.attnotnull, pg_get_expr(d.adbin, d.adrelid), a.attidentity::text, \
                    col_description(a.attrelid, a.attnum), {generated} \
             FROM pg_attribute a \
             JOIN pg_class c ON c.oid = a.attrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             JOIN pg_type t ON t.oid = a.atttypid \
             LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
             WHERE a.attnum > 0 AND NOT a.attisdropped AND c.relkind IN ('r','v','m','f','p') AND {SYSTEM_SCHEMAS} \
             ORDER BY a.attrelid, a.attnum"
        ),
    )
    .await?;
    for r in &cols {
        let Some(&(si, ti)) = by_oid.get(&r.get::<_, u32>(0)) else { continue };
        let data_type: String = r.get(2);
        let base_type: String = r.get(3);
        let identity: String = r.get(6);
        let default: Option<String> = match identity.as_str() {
            "a" => Some("GENERATED ALWAYS AS IDENTITY".into()),
            "d" => Some("GENERATED BY DEFAULT AS IDENTITY".into()),
            _ => r.get(5),
        };
        let generated_kind: String = r.get(8);
        let (default, generated) = if generated_kind == "s" {
            // For a generated column `pg_attrdef` holds the generation expression, not a default.
            (
                None,
                r.get::<_, Option<String>>(5).map(|expression| GeneratedColumn { expression, stored: true }),
            )
        } else {
            (default, None)
        };
        let identity = match identity.as_str() {
            "a" => Some(Identity::Always),
            "d" => Some(Identity::ByDefault),
            _ => None,
        };
        let geometry = parse_geometry_type(&base_type);
        let kind = if geometry.is_some() { TypeKind::Geometry } else { kind_of_formatted(&base_type) };
        schemas[si].tables[ti].columns.push(ColumnInfo {
            name: r.get(1),
            data_type,
            kind,
            nullable: !r.get::<_, bool>(4),
            default,
            comment: r.get(7),
            geometry,
            identity,
            generated,
        });
    }

    let cons = q(
        client,
        &format!(
            "SELECT con.conrelid, con.conname::text, con.contype::text, \
               (SELECT array_agg(a.attname::text ORDER BY k.ord) \
                  FROM unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord) \
                  JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum), \
               fn.nspname::text, fc.relname::text, \
               (SELECT array_agg(a.attname::text ORDER BY k.ord) \
                  FROM unnest(con.confkey) WITH ORDINALITY AS k(attnum, ord) \
                  JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.attnum), \
               con.confdeltype::text, con.confupdtype::text, \
               con.condeferrable, con.condeferred, \
               CASE WHEN con.contype = 'c' THEN pg_get_constraintdef(con.oid, true) END \
             FROM pg_constraint con \
             JOIN pg_class c ON c.oid = con.conrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             LEFT JOIN pg_class fc ON fc.oid = con.confrelid \
             LEFT JOIN pg_namespace fn ON fn.oid = fc.relnamespace \
             WHERE con.contype IN ('p','f','u','c') AND {SYSTEM_SCHEMAS} \
             ORDER BY con.conrelid, con.contype DESC, con.conname"
        ),
    )
    .await?;
    for r in &cons {
        let Some(&(si, ti)) = by_oid.get(&r.get::<_, u32>(0)) else { continue };
        let table = &mut schemas[si].tables[ti];
        let columns: Vec<String> = r.get::<_, Option<Vec<String>>>(3).unwrap_or_default();
        let contype: String = r.get(2);
        if contype == "p" {
            table.primary_key = columns;
        } else if contype == "u" {
            table.unique_constraints.push(UniqueConstraint { name: r.get(1), columns });
        } else if contype == "c" {
            let def: String = r.get::<_, Option<String>>(11).unwrap_or_default();
            table.checks.push(CheckConstraint { name: r.get(1), expression: strip_check(&def) });
        } else {
            table.foreign_keys.push(ForeignKey {
                name: r.get(1),
                columns,
                referenced_schema: r.get(4),
                referenced_table: r.get::<_, Option<String>>(5).unwrap_or_default(),
                referenced_columns: r.get::<_, Option<Vec<String>>>(6).unwrap_or_default(),
                on_delete: action(&r.get::<_, String>(7)),
                on_update: action(&r.get::<_, String>(8)),
                deferrable: r.get(9),
                initially_deferred: r.get(10),
            });
        }
    }

    let idx = q(
        client,
        &format!(
            "SELECT i.indrelid, ic.relname::text, i.indisunique, i.indisprimary, am.amname::text, \
                    pg_get_indexdef(i.indexrelid), \
               (SELECT array_agg(CASE WHEN k.attnum > 0 \
                          THEN (SELECT a.attname::text FROM pg_attribute a WHERE a.attrelid = i.indrelid AND a.attnum = k.attnum) \
                          ELSE pg_get_indexdef(i.indexrelid, k.ord::int, true) END ORDER BY k.ord) \
                  FROM unnest(i.indkey::int2[]) WITH ORDINALITY AS k(attnum, ord) \
                 WHERE k.ord <= i.indnkeyatts), \
                    pg_get_expr(i.indpred, i.indrelid), i.indexprs IS NOT NULL \
             FROM pg_index i \
             JOIN pg_class ic ON ic.oid = i.indexrelid \
             JOIN pg_class c ON c.oid = i.indrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             JOIN pg_am am ON am.oid = ic.relam \
             WHERE {SYSTEM_SCHEMAS} \
             ORDER BY i.indrelid, ic.relname"
        ),
    )
    .await?;
    for r in &idx {
        let Some(&(si, ti)) = by_oid.get(&r.get::<_, u32>(0)) else { continue };
        schemas[si].tables[ti].indexes.push(Index {
            name: r.get(1),
            unique: r.get(2),
            primary: r.get(3),
            method: Some(r.get(4)),
            definition: Some(r.get(5)),
            columns: r.get::<_, Option<Vec<String>>>(6).unwrap_or_default(),
            predicate: r.get(7),
            has_expressions: r.get(8),
        });
    }

    // Extension-owned routines (PostGIS alone ships thousands) are hidden: they are
    // not something the user wrote and would drown the explorer.
    let routines = q(
        client,
        &format!(
            "SELECT n.nspname::text, p.proname::text, p.prokind::text, pg_get_function_arguments(p.oid), \
                    pg_get_function_result(p.oid) \
             FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
             WHERE {SYSTEM_SCHEMAS} \
               AND NOT EXISTS (SELECT 1 FROM pg_depend d WHERE d.classid = 'pg_proc'::regclass \
                               AND d.objid = p.oid AND d.deptype = 'e') \
             ORDER BY n.nspname, p.proname, p.oid"
        ),
    )
    .await?;
    for r in &routines {
        let Some(&si) = schema_index.get(&r.get::<_, String>(0)) else { continue };
        let kind = match r.get::<_, String>(2).as_str() {
            "p" => RoutineKind::Procedure,
            "a" => RoutineKind::Aggregate,
            _ => RoutineKind::Function,
        };
        schemas[si].routines.push(Routine {
            name: r.get(1),
            kind,
            signature: format!("({})", r.get::<_, String>(3)),
            return_type: r.get(4),
        });
    }

    let seqs = q(
        client,
        &format!(
            "SELECT n.nspname::text, c.relname::text FROM pg_class c \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relkind = 'S' AND {SYSTEM_SCHEMAS} ORDER BY 1, 2"
        ),
    )
    .await?;
    for r in &seqs {
        if let Some(&si) = schema_index.get(&r.get::<_, String>(0)) {
            schemas[si].sequences.push(r.get(1));
        }
    }

    // `current_schemas(false)` lists only schemas that exist, in search order.
    let default_schema =
        q(client, "SELECT (current_schemas(false))[1]::text").await?.first().and_then(|r| r.get(0));
    Ok(Catalog { schemas, default_schema })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_typmod_is_parsed() {
        let g = parse_geometry_type("geometry(Point,4326)").unwrap();
        assert_eq!((g.geometry_type.as_str(), g.srid, g.geography), ("POINT", Some(4326), false));
        let g = parse_geometry_type("geography(MultiPolygon,4326)").unwrap();
        assert!(g.geography);
        let g = parse_geometry_type("public.geometry").unwrap();
        assert_eq!((g.geometry_type.as_str(), g.srid), ("GEOMETRY", None));
        assert!(parse_geometry_type("integer").is_none());
    }

    #[test]
    fn check_definitions_lose_their_wrapper() {
        assert_eq!(strip_check("CHECK ((price > (0)::numeric))"), "(price > (0)::numeric)");
        assert_eq!(strip_check("CHECK ((a > 0)) NOT VALID"), "(a > 0) NOT VALID");
        assert_eq!(strip_check("CHECK (status = ')')"), "status = ')'");
    }

    #[test]
    fn formatted_types_map_to_kinds() {
        assert_eq!(kind_of_formatted("character varying(40)"), TypeKind::Text);
        assert_eq!(kind_of_formatted("timestamp with time zone"), TypeKind::Timestamp);
        assert_eq!(kind_of_formatted("integer[]"), TypeKind::Array);
        assert_eq!(kind_of_formatted("numeric(10,2)"), TypeKind::Decimal);
        assert_eq!(kind_of_formatted("double precision"), TypeKind::Float);
        assert_eq!(kind_of_formatted("public.citext"), TypeKind::Text);
    }
}
