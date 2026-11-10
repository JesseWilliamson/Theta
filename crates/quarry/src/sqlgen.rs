//! SQL text generated from catalog metadata (explorer context menu: "Generate
//! SELECT / INSERT / UPDATE / DELETE", "Show DDL"). Pure functions, no UI.

use quarry_core::{DatabaseKind, Table, TableKind};

const RESERVED: &[&str] = &[
    "all",
    "and",
    "any",
    "as",
    "asc",
    "between",
    "by",
    "case",
    "check",
    "column",
    "constraint",
    "create",
    "cross",
    "default",
    "delete",
    "desc",
    "distinct",
    "drop",
    "else",
    "end",
    "except",
    "exists",
    "false",
    "for",
    "foreign",
    "from",
    "full",
    "group",
    "having",
    "in",
    "index",
    "inner",
    "insert",
    "intersect",
    "into",
    "is",
    "join",
    "key",
    "left",
    "like",
    "limit",
    "not",
    "null",
    "offset",
    "on",
    "or",
    "order",
    "outer",
    "primary",
    "references",
    "right",
    "select",
    "set",
    "table",
    "then",
    "to",
    "true",
    "union",
    "unique",
    "update",
    "user",
    "using",
    "values",
    "when",
    "where",
    "with",
];

/// Quote an identifier only when it must be quoted (mixed case, spaces,
/// reserved words), so generated SQL reads like hand-written SQL.
pub fn ident(kind: DatabaseKind, name: &str) -> String {
    let simple = !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if simple && !RESERVED.contains(&name) { name.to_string() } else { kind.quote_ident(name) }
}

/// `schema.table`, omitting the schema when it is the connection's default.
pub fn qualified(kind: DatabaseKind, schema: &str, table: &str, default_schema: Option<&str>) -> String {
    if schema.is_empty() || default_schema.is_some_and(|d| d == schema) && kind == DatabaseKind::Sqlite {
        ident(kind, table)
    } else {
        format!("{}.{}", ident(kind, schema), ident(kind, table))
    }
}

fn column_list(kind: DatabaseKind, t: &Table, indent: &str) -> String {
    t.columns.iter().map(|c| format!("{indent}{}", ident(kind, &c.name))).collect::<Vec<_>>().join(",\n")
}

pub fn select(kind: DatabaseKind, schema: &str, t: &Table, default_schema: Option<&str>) -> String {
    let name = qualified(kind, schema, &t.name, default_schema);
    if t.columns.is_empty() {
        return format!("SELECT *\nFROM {name}\nLIMIT 100;\n");
    }
    format!("SELECT\n{}\nFROM {name}\nLIMIT 100;\n", column_list(kind, t, "    "))
}

pub fn insert(kind: DatabaseKind, schema: &str, t: &Table, default_schema: Option<&str>) -> String {
    let name = qualified(kind, schema, &t.name, default_schema);
    let cols = t.columns.iter().map(|c| ident(kind, &c.name)).collect::<Vec<_>>().join(", ");
    let n = t.columns.len();
    let values = t
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let comma = if i + 1 < n { "," } else { "" };
            format!("    NULL{comma} -- {} ({})", c.name, c.data_type)
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("INSERT INTO {name} ({cols})\nVALUES (\n{values}\n);\n")
}

fn where_clause(kind: DatabaseKind, t: &Table) -> String {
    if t.primary_key.is_empty() {
        return "WHERE /* condition */ FALSE".into();
    }
    // `= NULL` is never true, so an unedited template cannot touch any row.
    let conds: Vec<String> = t.primary_key.iter().map(|c| format!("{} = NULL", ident(kind, c))).collect();
    format!("WHERE {}", conds.join("\n  AND "))
}

pub fn update(kind: DatabaseKind, schema: &str, t: &Table, default_schema: Option<&str>) -> String {
    let name = qualified(kind, schema, &t.name, default_schema);
    let sets: Vec<&_> = t.columns.iter().filter(|c| !t.primary_key.contains(&c.name)).collect();
    let sets = if sets.is_empty() { t.columns.iter().collect() } else { sets };
    let body =
        sets.iter().map(|c| format!("    {} = NULL", ident(kind, &c.name))).collect::<Vec<_>>().join(",\n");
    format!("UPDATE {name}\nSET\n{body}\n{};\n", where_clause(kind, t))
}

pub fn delete(kind: DatabaseKind, schema: &str, t: &Table, default_schema: Option<&str>) -> String {
    let name = qualified(kind, schema, &t.name, default_schema);
    format!("DELETE FROM {name}\n{};\n", where_clause(kind, t))
}

/// Best-effort DDL reconstructed from metadata (the catalog carries no stored
/// definition text).
pub fn ddl(kind: DatabaseKind, schema: &str, t: &Table, default_schema: Option<&str>) -> String {
    let name = qualified(kind, schema, &t.name, default_schema);
    if matches!(t.kind, TableKind::View | TableKind::MaterializedView) {
        let what = if t.kind == TableKind::View { "view" } else { "materialized view" };
        let cols = t
            .columns
            .iter()
            .map(|c| format!("--   {} {}", c.name, c.data_type))
            .collect::<Vec<_>>()
            .join("\n");
        return format!(
            "-- The definition of {what} {name} is not available from the catalog.\n-- Columns:\n{cols}\n\nSELECT * FROM {name};\n"
        );
    }
    let mut lines: Vec<String> = t
        .columns
        .iter()
        .map(|c| {
            let mut l = format!("    {} {}", ident(kind, &c.name), c.data_type);
            if !c.nullable {
                l.push_str(" NOT NULL");
            }
            if let Some(d) = &c.default {
                l.push_str(&format!(" DEFAULT {d}"));
            }
            l
        })
        .collect();
    if !t.primary_key.is_empty() {
        let cols = t.primary_key.iter().map(|c| ident(kind, c)).collect::<Vec<_>>().join(", ");
        lines.push(format!("    PRIMARY KEY ({cols})"));
    }
    for fk in &t.foreign_keys {
        let cols = fk.columns.iter().map(|c| ident(kind, c)).collect::<Vec<_>>().join(", ");
        let target = match &fk.referenced_schema {
            Some(s) => qualified(kind, s, &fk.referenced_table, default_schema),
            None => ident(kind, &fk.referenced_table),
        };
        let rcols = fk.referenced_columns.iter().map(|c| ident(kind, c)).collect::<Vec<_>>().join(", ");
        let mut l = format!(
            "    CONSTRAINT {} FOREIGN KEY ({cols}) REFERENCES {target} ({rcols})",
            ident(kind, &fk.name)
        );
        if let Some(a) = &fk.on_delete
            && a != "NO ACTION"
        {
            l.push_str(&format!(" ON DELETE {a}"));
        }
        if let Some(a) = &fk.on_update
            && a != "NO ACTION"
        {
            l.push_str(&format!(" ON UPDATE {a}"));
        }
        lines.push(l);
    }
    let mut out = format!("CREATE TABLE {name} (\n{}\n);\n", lines.join(",\n"));
    for i in t.indexes.iter().filter(|i| !i.primary) {
        match &i.definition {
            Some(def) => out.push_str(&format!("\n{};", def.trim_end_matches(';'))),
            None => {
                let cols = i.columns.iter().map(|c| ident(kind, c)).collect::<Vec<_>>().join(", ");
                out.push_str(&format!(
                    "\nCREATE {}INDEX {} ON {name} ({cols});",
                    if i.unique { "UNIQUE " } else { "" },
                    ident(kind, &i.name)
                ));
            }
        }
    }
    if out.ends_with(';') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use quarry_core::{ColumnInfo, ForeignKey, Index, TypeKind};

    fn col(name: &str, ty: &str, nullable: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            data_type: ty.into(),
            kind: TypeKind::Other,
            nullable,
            default: None,
            comment: None,
            geometry: None,
        }
    }

    fn orders() -> Table {
        Table {
            name: "orders".into(),
            kind: TableKind::Table,
            columns: vec![
                col("id", "integer", false),
                col("customer_id", "integer", true),
                col("Total Price", "numeric(10,2)", true),
            ],
            primary_key: vec!["id".into()],
            foreign_keys: vec![ForeignKey {
                name: "orders_customer_fk".into(),
                columns: vec!["customer_id".into()],
                referenced_schema: Some("public".into()),
                referenced_table: "customers".into(),
                referenced_columns: vec!["id".into()],
                on_delete: Some("CASCADE".into()),
                on_update: Some("NO ACTION".into()),
            }],
            indexes: vec![
                Index {
                    name: "orders_pkey".into(),
                    columns: vec!["id".into()],
                    unique: true,
                    primary: true,
                    method: None,
                    definition: None,
                },
                Index {
                    name: "orders_customer_idx".into(),
                    columns: vec!["customer_id".into()],
                    unique: false,
                    primary: false,
                    method: None,
                    definition: None,
                },
            ],
            row_estimate: None,
            comment: None,
        }
    }

    #[test]
    fn identifiers_are_quoted_only_when_needed() {
        assert_eq!(ident(DatabaseKind::Postgres, "orders"), "orders");
        assert_eq!(ident(DatabaseKind::Postgres, "Orders"), "\"Orders\"");
        assert_eq!(ident(DatabaseKind::Postgres, "order"), "\"order\"");
        assert_eq!(ident(DatabaseKind::MySql, "total price"), "`total price`");
        assert_eq!(ident(DatabaseKind::Postgres, "1abc"), "\"1abc\"");
    }

    #[test]
    fn select_lists_columns_and_limits() {
        let sql = select(DatabaseKind::Postgres, "public", &orders(), None);
        assert_eq!(
            sql,
            "SELECT\n    id,\n    customer_id,\n    \"Total Price\"\nFROM public.orders\nLIMIT 100;\n"
        );
    }

    #[test]
    fn insert_has_one_placeholder_per_column() {
        let sql = insert(DatabaseKind::Postgres, "public", &orders(), None);
        assert!(sql.starts_with("INSERT INTO public.orders (id, customer_id, \"Total Price\")\nVALUES (\n"));
        assert!(sql.contains("    NULL, -- id (integer)\n"));
        assert!(sql.contains("    NULL -- Total Price (numeric(10,2))\n);"));
    }

    #[test]
    fn update_and_delete_are_safe_by_default() {
        let u = update(DatabaseKind::Postgres, "public", &orders(), None);
        assert_eq!(
            u,
            "UPDATE public.orders\nSET\n    customer_id = NULL,\n    \"Total Price\" = NULL\nWHERE id = NULL;\n"
        );
        let d = delete(DatabaseKind::Postgres, "public", &orders(), None);
        assert_eq!(d, "DELETE FROM public.orders\nWHERE id = NULL;\n");
        let mut no_pk = orders();
        no_pk.primary_key.clear();
        assert!(delete(DatabaseKind::Postgres, "public", &no_pk, None).contains("FALSE"));
    }

    #[test]
    fn ddl_contains_constraints_and_indexes() {
        let sql = ddl(DatabaseKind::Postgres, "public", &orders(), None);
        assert!(sql.starts_with("CREATE TABLE public.orders (\n    id integer NOT NULL,"));
        assert!(sql.contains("    PRIMARY KEY (id),"));
        assert!(sql.contains("CONSTRAINT orders_customer_fk FOREIGN KEY (customer_id) REFERENCES public.customers (id) ON DELETE CASCADE\n);"));
        assert!(!sql.contains("ON UPDATE"), "NO ACTION is the default and omitted");
        assert!(sql.contains("CREATE INDEX orders_customer_idx ON public.orders (customer_id);"));
        assert!(!sql.contains("orders_pkey"));
    }

    #[test]
    fn sqlite_default_schema_is_omitted() {
        assert_eq!(qualified(DatabaseKind::Sqlite, "main", "t", Some("main")), "t");
        assert_eq!(qualified(DatabaseKind::Postgres, "public", "t", Some("public")), "public.t");
    }
}
