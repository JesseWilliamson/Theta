//! Deciding whether a result can be edited in place, and how.

use quarry_core::{Catalog, ResultSet};
use quarry_grid::{EditTarget, GridMeta};
use std::collections::HashSet;

/// What the grid should be told about editability for `result`.
pub fn grid_meta(result: &ResultSet, catalog: Option<&Catalog>, read_only_connection: bool) -> GridMeta {
    let mut meta = GridMeta::new();
    match analyze(result, catalog) {
        Ok(target) => {
            meta.reference_columns = reference_columns(result, catalog, &target);
            if read_only_connection {
                meta.read_only_reason = Some("The connection is read-only".into());
            } else {
                meta.editable = Some(target);
            }
        }
        Err(reason) => meta.read_only_reason = Some(reason),
    }
    meta
}

/// `Ok(target)` when all columns come from one table whose whole primary key is present.
pub fn analyze(result: &ResultSet, catalog: Option<&Catalog>) -> Result<EditTarget, String> {
    let Some(catalog) = catalog else { return Err("Schema is not loaded yet".into()) };
    if result.columns.is_empty() {
        return Err("No columns".into());
    }
    let mut origin: Option<(Option<&str>, &str)> = None;
    for c in &result.columns {
        let Some(o) = &c.origin else {
            return Err(format!("Column “{}” is computed, so the rows cannot be edited", c.name));
        };
        let key = (o.schema.as_deref(), o.table.as_str());
        match origin {
            None => origin = Some(key),
            Some(prev) if prev == key => {}
            Some(_) => return Err("The result joins several tables".into()),
        }
    }
    let (schema, table_name) = origin.expect("non-empty columns");
    let Some((s, table)) = catalog.find_table(schema, table_name) else {
        return Err(format!("Table “{table_name}” is not in the loaded schema"));
    };
    if !matches!(table.kind, quarry_core::TableKind::Table) {
        return Err("Only tables can be edited, not views".into());
    }
    if table.primary_key.is_empty() {
        return Err(format!("“{}” has no primary key", table.name));
    }
    let origin_col = |i: usize| result.columns[i].origin.as_ref().map(|o| o.column.as_str()).unwrap_or("");
    let mut key_columns = Vec::new();
    for pk in &table.primary_key {
        match (0..result.columns.len()).find(|&i| origin_col(i).eq_ignore_ascii_case(pk)) {
            Some(i) => key_columns.push(i),
            None => return Err(format!("The primary key column “{pk}” is not in the result")),
        }
    }
    let keys: HashSet<usize> = key_columns.iter().copied().collect();
    let editable_columns: Vec<usize> = (0..result.columns.len())
        .filter(|i| !keys.contains(i) || true)
        .filter(|&i| table.columns.iter().any(|c| c.name.eq_ignore_ascii_case(origin_col(i))))
        .collect();
    Ok(EditTarget::new(Some(s.name.clone()), table.name.clone(), key_columns, editable_columns))
}

/// Result column indexes that are (single-column) foreign keys.
pub fn reference_columns(result: &ResultSet, catalog: Option<&Catalog>, target: &EditTarget) -> Vec<usize> {
    let Some(catalog) = catalog else { return vec![] };
    let Some((_, table)) = catalog.find_table(target.schema.as_deref(), &target.table) else { return vec![] };
    (0..result.columns.len())
        .filter(|&i| {
            let name = result.columns[i].origin.as_ref().map(|o| o.column.as_str()).unwrap_or("");
            table
                .foreign_keys
                .iter()
                .any(|fk| fk.columns.len() == 1 && fk.columns[0].eq_ignore_ascii_case(name))
        })
        .collect()
}

/// The foreign key behind result column `column`: (schema, table, referenced column).
pub fn reference_target(
    result: &ResultSet,
    catalog: &Catalog,
    target_table: (&Option<String>, &str),
    column: usize,
) -> Option<(Option<String>, String, String)> {
    let (_, table) = catalog.find_table(target_table.0.as_deref(), target_table.1)?;
    let name = result.columns.get(column)?.origin.as_ref()?.column.as_str();
    let fk = table
        .foreign_keys
        .iter()
        .find(|fk| fk.columns.len() == 1 && fk.columns[0].eq_ignore_ascii_case(name))?;
    Some((fk.referenced_schema.clone(), fk.referenced_table.clone(), fk.referenced_columns.first()?.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use quarry_core::{Column, ColumnOrigin, TypeKind};
    use quarry_sql::fixtures;

    fn col(name: &str, table: &str, c: &str) -> Column {
        Column {
            name: name.into(),
            type_name: "int".into(),
            kind: TypeKind::Integer,
            origin: Some(ColumnOrigin {
                schema: Some("public".into()),
                table: table.into(),
                column: c.into(),
            }),
        }
    }
    fn rs(cols: Vec<Column>) -> ResultSet {
        ResultSet { columns: cols, rows: vec![], truncated: false }
    }

    #[test]
    fn editable_when_pk_present() {
        let cat = fixtures::shop();
        let t =
            analyze(&rs(vec![col("id", "customers", "id"), col("name", "customers", "name")]), Some(&cat))
                .unwrap();
        assert_eq!(t.table, "customers");
        assert_eq!(t.key_columns, vec![0]);
    }

    #[test]
    fn refuses_without_pk_column() {
        let cat = fixtures::shop();
        let e = analyze(&rs(vec![col("name", "customers", "name")]), Some(&cat)).unwrap_err();
        assert!(e.contains("primary key"), "{e}");
    }

    #[test]
    fn refuses_joins_and_computed() {
        let cat = fixtures::shop();
        let e = analyze(&rs(vec![col("id", "customers", "id"), col("id", "orders", "id")]), Some(&cat))
            .unwrap_err();
        assert!(e.contains("several tables"));
        let mut c = col("x", "customers", "id");
        c.origin = None;
        assert!(analyze(&rs(vec![c]), Some(&cat)).unwrap_err().contains("computed"));
    }

    #[test]
    fn read_only_connection_keeps_reason() {
        let cat = fixtures::shop();
        let m = grid_meta(&rs(vec![col("id", "customers", "id")]), Some(&cat), true);
        assert!(m.editable.is_none() && m.read_only_reason.is_some());
    }

    #[test]
    fn fk_columns_are_references() {
        let cat = fixtures::shop();
        let r = rs(vec![col("id", "orders", "id"), col("customer_id", "orders", "customer_id")]);
        let m = grid_meta(&r, Some(&cat), false);
        assert_eq!(m.reference_columns, vec![1]);
    }
}
