//! Amazon Redshift catalog introspection.
//!
//! Redshift is PostgreSQL 8.0.2 with a different catalog surface: no `pg_index` worth reading (it has no
//! indexes), no `pg_get_function_arguments`, no identity or generated columns in `pg_attribute`, array
//! functions missing, and constraints that are informational only. The stock introspection would fail on
//! it, so this module reads only what the Redshift documentation lists: `information_schema`,
//! `SVV_TABLE_INFO` (row counts, distribution style, sort key) and `pg_constraint` with
//! `pg_get_constraintdef`.
//!
//! This code has not been run against a cluster (there is no Redshift to test against); the parsers are
//! unit-tested and the queries use only documented views.

use super::query_error;
use crate::util::kind_from_type_name;
use std::collections::HashMap;
use theta_core::*;
use tokio_postgres::Client;

const HIDDEN: &str = "('pg_catalog', 'information_schema', 'pg_internal', 'catalog_history')";

async fn q(client: &Client, sql: &str) -> Result<Vec<tokio_postgres::Row>> {
    client.query(sql, &[]).await.map_err(|e| query_error(e, sql, 0))
}

/// A constraint as `pg_get_constraintdef` prints it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConstraintDef {
    Primary(Vec<String>),
    Unique(Vec<String>),
    Foreign { columns: Vec<String>, table: String, schema: Option<String>, referenced: Vec<String> },
}

fn idents(list: &str) -> Vec<String> {
    list.split(',').map(|c| c.trim().trim_matches('"').to_string()).filter(|c| !c.is_empty()).collect()
}

/// `PRIMARY KEY (a, b)`, `UNIQUE (a)`, `FOREIGN KEY (a) REFERENCES s.t(b)`.
pub(crate) fn parse_constraint_def(def: &str) -> Option<ConstraintDef> {
    let d = def.trim();
    let inside = |from: &str| -> Option<String> {
        let start = from.find('(')? + 1;
        let end = from[start..].find(')')? + start;
        Some(from[start..end].to_string())
    };
    if let Some(rest) = d.strip_prefix("PRIMARY KEY") {
        return Some(ConstraintDef::Primary(idents(&inside(rest)?)));
    }
    if let Some(rest) = d.strip_prefix("UNIQUE") {
        return Some(ConstraintDef::Unique(idents(&inside(rest)?)));
    }
    if let Some(rest) = d.strip_prefix("FOREIGN KEY") {
        let columns = idents(&inside(rest)?);
        let (_, refs) = rest.split_once("REFERENCES")?;
        let refs = refs.trim();
        let paren = refs.find('(')?;
        let name = refs[..paren].trim();
        let referenced = idents(&inside(&refs[paren..])?);
        let (schema, table) = match name.rsplit_once('.') {
            Some((s, t)) => (Some(s.trim_matches('"').to_string()), t.trim_matches('"').to_string()),
            None => (None, name.trim_matches('"').to_string()),
        };
        return Some(ConstraintDef::Foreign { columns, table, schema, referenced });
    }
    None
}

/// The type as written in DDL, from the `information_schema.columns` fields.
pub(crate) fn format_type(
    data_type: &str,
    char_len: Option<i32>,
    precision: Option<i32>,
    scale: Option<i32>,
) -> String {
    match (data_type, char_len, precision, scale) {
        ("character varying", Some(n), ..) => format!("character varying({n})"),
        ("character", Some(n), ..) => format!("character({n})"),
        ("numeric", _, Some(p), Some(s)) => format!("numeric({p},{s})"),
        _ => data_type.to_string(),
    }
}

pub async fn introspect(client: &Client) -> Result<Catalog> {
    let schema_rows = q(
        client,
        &format!(
            "SELECT nspname::text FROM pg_namespace \
             WHERE nspname NOT IN {HIDDEN} AND nspname NOT LIKE 'pg\\_%' ORDER BY 1"
        ),
    )
    .await?;
    let mut schemas: Vec<Schema> =
        schema_rows.iter().map(|r| Schema { name: r.get(0), ..Default::default() }).collect();
    let schema_index: HashMap<String, usize> =
        schemas.iter().enumerate().map(|(i, s)| (s.name.clone(), i)).collect();
    let mut by_name: HashMap<(String, String), (usize, usize)> = HashMap::new();

    let tables = q(
        client,
        &format!(
            "SELECT table_schema::text, table_name::text, table_type::text FROM information_schema.tables \
             WHERE table_schema NOT IN {HIDDEN} AND table_schema NOT LIKE 'pg\\_%' ORDER BY 1, 2"
        ),
    )
    .await?;
    for r in &tables {
        let schema: String = r.get(0);
        let Some(&si) = schema_index.get(&schema) else { continue };
        let name: String = r.get(1);
        let kind = if r.get::<_, String>(2) == "VIEW" { TableKind::View } else { TableKind::Table };
        schemas[si].tables.push(Table { name: name.clone(), kind, ..Default::default() });
        by_name.insert((schema, name), (si, schemas[si].tables.len() - 1));
    }

    // Row estimate, distribution style and sort key; the view lists only tables, and only those the
    // user may see, so a failure here is a permissions matter for the whole catalog and is reported.
    let info = q(
        client,
        "SELECT \"schema\"::text, \"table\"::text, tbl_rows::float8, diststyle::text, sortkey1::text \
         FROM svv_table_info",
    )
    .await?;
    for r in &info {
        let Some(&(si, ti)) = by_name.get(&(r.get::<_, String>(0), r.get::<_, String>(1))) else { continue };
        let t = &mut schemas[si].tables[ti];
        t.row_estimate = r.get::<_, Option<f64>>(2).map(|n| n as u64);
        if let Some(d) = r.get::<_, Option<String>>(3) {
            t.options.push(TableOption { name: "DISTSTYLE".into(), value: d });
        }
        if let Some(k) = r.get::<_, Option<String>>(4).filter(|k| !k.is_empty() && k != "AUTO") {
            t.options.push(TableOption { name: "SORTKEY".into(), value: k });
        }
    }

    let cols = q(
        client,
        &format!(
            "SELECT table_schema::text, table_name::text, column_name::text, data_type::text, \
                    character_maximum_length::int4, numeric_precision::int4, numeric_scale::int4, \
                    is_nullable::text, column_default::text \
             FROM information_schema.columns \
             WHERE table_schema NOT IN {HIDDEN} AND table_schema NOT LIKE 'pg\\_%' \
             ORDER BY table_schema, table_name, ordinal_position"
        ),
    )
    .await?;
    for r in &cols {
        let Some(&(si, ti)) = by_name.get(&(r.get::<_, String>(0), r.get::<_, String>(1))) else { continue };
        let data_type = format_type(&r.get::<_, String>(3), r.get(4), r.get(5), r.get(6));
        let default: Option<String> = r.get(8);
        // `"identity"(seed, step, 'start,step'::text)` is how Redshift stores an IDENTITY column's default.
        let identity =
            default.as_deref().is_some_and(|d| d.contains("identity\"(")).then_some(Identity::ByDefault);
        let geometry = super::introspect::parse_geometry_type(&data_type);
        schemas[si].tables[ti].columns.push(ColumnInfo {
            name: r.get(2),
            kind: if geometry.is_some() { TypeKind::Geometry } else { kind_from_type_name(&data_type) },
            data_type,
            nullable: r.get::<_, String>(7) == "YES",
            default: if identity.is_some() { None } else { default },
            identity,
            geometry,
            ..Default::default()
        });
    }

    let cons = q(
        client,
        &format!(
            "SELECT n.nspname::text, c.relname::text, con.conname::text, pg_get_constraintdef(con.oid) \
             FROM pg_constraint con JOIN pg_class c ON c.oid = con.conrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE con.contype IN ('p', 'u', 'f') AND n.nspname NOT IN {HIDDEN} AND n.nspname NOT LIKE 'pg\\_%' \
             ORDER BY 1, 2, 3"
        ),
    )
    .await?;
    for r in &cons {
        let Some(&(si, ti)) = by_name.get(&(r.get::<_, String>(0), r.get::<_, String>(1))) else { continue };
        let table = &mut schemas[si].tables[ti];
        let name: String = r.get(2);
        match parse_constraint_def(&r.get::<_, String>(3)) {
            Some(ConstraintDef::Primary(columns)) => table.primary_key = columns,
            Some(ConstraintDef::Unique(columns)) => {
                table.unique_constraints.push(UniqueConstraint { name, columns })
            }
            Some(ConstraintDef::Foreign { columns, table: ref_table, schema, referenced }) => {
                table.foreign_keys.push(ForeignKey {
                    name,
                    columns,
                    referenced_schema: schema,
                    referenced_table: ref_table,
                    referenced_columns: referenced,
                    ..Default::default()
                })
            }
            None => {}
        }
    }
    // Primary and foreign keys are informational on Redshift; say so where the user looks.
    for s in &mut schemas {
        for t in &mut s.tables {
            if !t.primary_key.is_empty() || !t.foreign_keys.is_empty() {
                t.options.push(TableOption {
                    name: "constraints".into(),
                    value: "informational (not enforced)".into(),
                });
            }
        }
    }

    // Redshift has no indexes and no `pg_get_function_arguments`; routines are not listed.
    let default_schema =
        q(client, "SELECT current_schema()::text").await?.first().and_then(|r| r.get::<_, Option<String>>(0));
    Ok(Catalog { schemas, default_schema })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constraint_definitions() {
        assert_eq!(
            parse_constraint_def("PRIMARY KEY (id, \"Region\")"),
            Some(ConstraintDef::Primary(vec!["id".into(), "Region".into()]))
        );
        assert_eq!(parse_constraint_def("UNIQUE (email)"), Some(ConstraintDef::Unique(vec!["email".into()])));
        assert_eq!(
            parse_constraint_def("FOREIGN KEY (venueid) REFERENCES public.venue(venueid)"),
            Some(ConstraintDef::Foreign {
                columns: vec!["venueid".into()],
                table: "venue".into(),
                schema: Some("public".into()),
                referenced: vec!["venueid".into()]
            })
        );
        assert_eq!(parse_constraint_def("CHECK (a > 0)"), None);
    }

    #[test]
    fn types_carry_their_parameters() {
        assert_eq!(format_type("character varying", Some(255), None, None), "character varying(255)");
        assert_eq!(format_type("numeric", None, Some(12), Some(2)), "numeric(12,2)");
        assert_eq!(format_type("integer", None, Some(32), Some(0)), "integer");
    }
}
