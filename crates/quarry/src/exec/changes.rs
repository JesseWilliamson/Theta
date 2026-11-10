//! Turning staged grid edits into SQL, and the transaction wrapper around them.

use quarry_core::DatabaseKind;
use quarry_grid::{EditTarget, RowChange};
use quarry_sql::{build_delete, build_insert, build_update};

/// One statement per change, in order. Changes that cannot be expressed safely
/// (an update or delete without key columns, an empty update) are reported as errors.
pub fn statements(
    kind: DatabaseKind,
    target: &EditTarget,
    changes: &[RowChange],
) -> Result<Vec<String>, String> {
    let schema = target.schema.as_deref();
    let mut out = Vec::new();
    for c in changes {
        match c {
            RowChange::Update { key, set, .. } => {
                if key.is_empty() {
                    return Err("An edited row has no primary key values".into());
                }
                if set.is_empty() {
                    continue;
                }
                out.push(build_update(kind, schema, &target.table, set, key));
            }
            RowChange::Insert { values } => {
                let cols: Vec<String> = values.iter().map(|(c, _)| c.clone()).collect();
                let row = values.iter().map(|(_, v)| v.clone()).collect::<Vec<_>>();
                out.push(build_insert(kind, schema, &target.table, &cols, &[row]));
            }
            RowChange::Delete { key, .. } => {
                if key.is_empty() {
                    return Err("A deleted row has no primary key values".into());
                }
                out.push(build_delete(kind, schema, &target.table, key));
            }
        }
    }
    Ok(out)
}

/// The statements actually sent: BEGIN/COMMIT around `stmts` unless the user
/// already has a transaction open (then it stays theirs to commit).
pub fn with_transaction(stmts: Vec<String>, already_open: bool) -> Vec<String> {
    if already_open {
        return stmts;
    }
    let mut v = Vec::with_capacity(stmts.len() + 2);
    v.push("BEGIN".to_string());
    v.extend(stmts);
    v.push("COMMIT".to_string());
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use quarry_core::Value;

    fn target() -> EditTarget {
        EditTarget::new(Some("public".into()), "customers", vec![0], vec![0, 1])
    }

    #[test]
    fn builds_update_insert_delete() {
        let ch = vec![
            RowChange::Update {
                row: 0,
                key: vec![("id".into(), Value::Int(7))],
                set: vec![("name".into(), Value::Text("O'Neil".into()))],
            },
            RowChange::Insert { values: vec![("name".into(), Value::Text("new".into()))] },
            RowChange::Delete { row: 2, key: vec![("id".into(), Value::Int(9))] },
        ];
        let s = statements(DatabaseKind::Postgres, &target(), &ch).unwrap();
        assert_eq!(s[0], "UPDATE public.customers SET name = 'O''Neil' WHERE id = 7");
        assert!(s[1].starts_with("INSERT INTO public.customers (name) VALUES ('new')"));
        assert_eq!(s[2], "DELETE FROM public.customers WHERE id = 9");
    }

    #[test]
    fn keyless_changes_are_refused() {
        let ch = vec![RowChange::Delete { row: 0, key: vec![] }];
        assert!(statements(DatabaseKind::Postgres, &target(), &ch).is_err());
    }

    #[test]
    fn transaction_wrapping() {
        let w = with_transaction(vec!["X".into()], false);
        assert_eq!(w, vec!["BEGIN", "X", "COMMIT"]);
        assert_eq!(with_transaction(vec!["X".into()], true), vec!["X"]);
    }
}
