//! Foreign-key assistance behind the explorer and the "Related Tables" picker.

use quarry_core::{Catalog, DatabaseKind};
use quarry_sql::VirtualForeignKey;
use quarry_sql::keywords::{qualified_name, quote_if_needed};

#[derive(Debug, Clone, PartialEq)]
pub struct Related {
    pub schema: String,
    pub table: String,
    /// "orders.customer_id → customers.id", with the direction of the key.
    pub via: String,
    pub inferred: bool,
    pub confidence: Option<f32>,
    /// A ready-to-run JOIN query starting from the table the user chose.
    pub sql: String,
}

fn q(kind: DatabaseKind, default: Option<&str>, schema: &str, table: &str) -> String {
    let schema = (Some(schema) != default).then_some(schema);
    qualified_name(schema, table, kind)
}

/// `ALTER TABLE s.t ADD FOREIGN KEY (c) REFERENCES s2.t2 (c2)` for an inferred key.
pub fn alter_table_sql(kind: DatabaseKind, default: Option<&str>, v: &VirtualForeignKey) -> String {
    let col = quote_if_needed(&v.column, kind);
    let rcol = quote_if_needed(&v.referenced_column, kind);
    format!(
        "ALTER TABLE {}\n    ADD CONSTRAINT {} FOREIGN KEY ({col}) REFERENCES {} ({rcol});",
        q(kind, default, &v.schema, &v.table),
        quote_if_needed(&format!("{}_{}_fkey", v.table, v.column), kind),
        q(kind, default, &v.referenced_schema, &v.referenced_table),
    )
}

fn alias(table: &str, taken: &str) -> String {
    let a: String =
        table.chars().next().map(|c| c.to_ascii_lowercase().to_string()).unwrap_or_else(|| "t".into());
    if a == taken { format!("{a}2") } else { a }
}

/// `SELECT * FROM left l JOIN right r ON l.a = r.b` for column pairs.
pub fn join_sql(
    kind: DatabaseKind,
    default: Option<&str>,
    left: (&str, &str),
    right: (&str, &str),
    pairs: &[(String, String)],
) -> String {
    let la = alias(left.1, "");
    let ra = alias(right.1, &la);
    let on: Vec<String> = pairs
        .iter()
        .map(|(l, r)| format!("{la}.{} = {ra}.{}", quote_if_needed(l, kind), quote_if_needed(r, kind)))
        .collect();
    format!(
        "SELECT *\nFROM {} {la}\nJOIN {} {ra} ON {}",
        q(kind, default, left.0, left.1),
        q(kind, default, right.0, right.1),
        on.join(" AND ")
    )
}

/// Tables reachable from `schema.table` through declared and inferred keys, both directions.
pub fn related_tables(
    catalog: &Catalog,
    inferred: &[VirtualForeignKey],
    schema: &str,
    table: &str,
    kind: DatabaseKind,
) -> Vec<Related> {
    let default = catalog.default_schema.as_deref();
    let mut out: Vec<Related> = Vec::new();
    let Some((_, this)) = catalog.find_table(Some(schema), table) else { return out };
    // Outgoing declared keys.
    for fk in &this.foreign_keys {
        let rs = fk.referenced_schema.clone().unwrap_or_else(|| schema.to_string());
        let pairs: Vec<_> = fk.columns.iter().cloned().zip(fk.referenced_columns.iter().cloned()).collect();
        out.push(Related {
            via: format!(
                "{} → {}.{}",
                fk.columns.join(", "),
                fk.referenced_table,
                fk.referenced_columns.join(", ")
            ),
            sql: join_sql(kind, default, (schema, table), (&rs, &fk.referenced_table), &pairs),
            schema: rs,
            table: fk.referenced_table.clone(),
            inferred: false,
            confidence: None,
        });
    }
    // Incoming declared keys.
    for s in &catalog.schemas {
        for t in &s.tables {
            for fk in &t.foreign_keys {
                let rs = fk.referenced_schema.as_deref().unwrap_or(&s.name);
                if rs == schema && fk.referenced_table.eq_ignore_ascii_case(table) {
                    let pairs: Vec<_> =
                        fk.referenced_columns.iter().cloned().zip(fk.columns.iter().cloned()).collect();
                    out.push(Related {
                        via: format!(
                            "{}.{} → {}",
                            t.name,
                            fk.columns.join(", "),
                            fk.referenced_columns.join(", ")
                        ),
                        sql: join_sql(kind, default, (schema, table), (&s.name, &t.name), &pairs),
                        schema: s.name.clone(),
                        table: t.name.clone(),
                        inferred: false,
                        confidence: None,
                    });
                }
            }
        }
    }
    for v in inferred {
        if v.schema == schema && v.table == table {
            out.push(Related {
                via: format!("{} → {}.{} (suggested)", v.column, v.referenced_table, v.referenced_column),
                sql: join_sql(
                    kind,
                    default,
                    (schema, table),
                    (&v.referenced_schema, &v.referenced_table),
                    &[(v.column.clone(), v.referenced_column.clone())],
                ),
                schema: v.referenced_schema.clone(),
                table: v.referenced_table.clone(),
                inferred: true,
                confidence: Some(v.confidence),
            });
        } else if v.referenced_schema == schema && v.referenced_table == table {
            out.push(Related {
                via: format!("{}.{} → {} (suggested)", v.table, v.column, v.referenced_column),
                sql: join_sql(
                    kind,
                    default,
                    (schema, table),
                    (&v.schema, &v.table),
                    &[(v.referenced_column.clone(), v.column.clone())],
                ),
                schema: v.schema.clone(),
                table: v.table.clone(),
                inferred: true,
                confidence: Some(v.confidence),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use quarry_sql::fixtures;

    #[test]
    fn related_covers_both_directions() {
        let cat = fixtures::shop();
        let r = related_tables(&cat, &[], "public", "orders", DatabaseKind::Postgres);
        let names: Vec<&str> = r.iter().map(|r| r.table.as_str()).collect();
        assert!(names.contains(&"customers"), "{names:?}");
        assert!(names.contains(&"order_items"), "{names:?}");
        let c = r.iter().find(|r| r.table == "customers").unwrap();
        assert_eq!(c.sql, "SELECT *\nFROM orders o\nJOIN customers c ON o.customer_id = c.id");
    }

    #[test]
    fn alias_collision_is_resolved() {
        let sql = join_sql(
            DatabaseKind::Postgres,
            Some("public"),
            ("public", "orders"),
            ("public", "order_items"),
            &[("id".into(), "order_id".into())],
        );
        assert!(sql.contains("orders o") && sql.contains("order_items o2"), "{sql}");
    }

    #[test]
    fn alter_table_statement() {
        let v = VirtualForeignKey {
            schema: "public".into(),
            table: "orders".into(),
            column: "store_id".into(),
            referenced_schema: "public".into(),
            referenced_table: "stores".into(),
            referenced_column: "id".into(),
            confidence: 0.9,
            reason: "name".into(),
        };
        let s = alter_table_sql(DatabaseKind::Postgres, Some("public"), &v);
        assert_eq!(
            s,
            "ALTER TABLE orders\n    ADD CONSTRAINT orders_store_id_fkey FOREIGN KEY (store_id) REFERENCES stores (id);"
        );
    }
}
