//! Turning staged grid edits into SQL, and the transaction wrapper around them.

use std::sync::Arc;
use theta_core::DatabaseKind;
use theta_core::{Connection, Error, ExecOptions, ResultSet, Value};
use theta_grid::{ChangeLocation, EditTarget, RowChange};
use theta_sql::keywords::quote_if_needed;
use theta_sql::{build_delete, build_insert, build_select_by_key, build_update};

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

/// Which staged change the statement number `statement` of [`statements`] came from, and how to say so
/// ("id = 7", "new row 2"). Mirrors the order and the skipping of `statements`.
pub fn locate(changes: &[RowChange], statement: usize) -> Option<(String, ChangeLocation)> {
    let mut n = 0;
    let mut new_rows = 0;
    for c in changes {
        let (label, at) = match c {
            RowChange::Update { row, key, set } => {
                if set.is_empty() {
                    continue;
                }
                (key_label(key), ChangeLocation::Row(*row))
            }
            RowChange::Delete { row, key } => (key_label(key), ChangeLocation::Row(*row)),
            RowChange::Insert { .. } => {
                new_rows += 1;
                (format!("new row {new_rows}"), ChangeLocation::NewRow(new_rows - 1))
            }
        };
        if n == statement {
            return Some((label, at));
        }
        n += 1;
    }
    None
}

fn key_label(key: &[(String, Value)]) -> String {
    key.iter()
        .map(|(c, v)| match v {
            Value::Null => format!("{c} is NULL"),
            Value::Text(t) | Value::Uuid(t) => format!("{c} = '{t}'"),
            other => format!("{c} = {}", other.display()),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// How the app learns what the database stored for an inserted row (generated keys, defaults,
/// computed columns), so the grid can show the row as it is rather than as it was typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadBack {
    /// `INSERT ... RETURNING`: PostgreSQL and its relatives, DuckDB, SQLite 3.35+, MariaDB 10.5+.
    Returning,
    /// MySQL has no `RETURNING`: `LAST_INSERT_ID()` names an auto-generated key, and the row is read
    /// again by key (the key the user typed, when there is one).
    LastInsertId,
    /// Nothing reliable (SQL Server, ClickHouse, Redshift, old engines): reload the result instead.
    Reload,
}

/// `major.minor` of the first version-looking token in `text` (`SQLite 3.46.0` is `(3, 46)`).
fn version_of(text: &str) -> Option<(u32, u32)> {
    let tok = text.split_whitespace().find(|t| t.starts_with(|c: char| c.is_ascii_digit()))?;
    let mut it = tok.split(['.', '-']).map(|p| p.parse::<u32>().ok());
    Some((it.next()??, it.next().flatten().unwrap_or(0)))
}

/// The read-back to use for `kind`; `server_version` is `ServerInfo::version`.
pub fn read_back(kind: DatabaseKind, server_version: &str) -> ReadBack {
    use theta_core::Family;
    let at_least = |min: (u32, u32)| version_of(server_version).is_some_and(|v| v >= min);
    match kind {
        DatabaseKind::Redshift => ReadBack::Reload,
        _ => match kind.family() {
            Family::Postgres | Family::DuckDb => ReadBack::Returning,
            Family::Sqlite if at_least((3, 35)) => ReadBack::Returning,
            Family::MySql if server_version.starts_with("MariaDB") && at_least((10, 5)) => {
                ReadBack::Returning
            }
            Family::MySql => ReadBack::LastInsertId,
            _ => ReadBack::Reload,
        },
    }
}

/// `insert_sql` asking for `columns` back.
pub fn with_returning(kind: DatabaseKind, insert_sql: &str, columns: &[String]) -> String {
    let cols: Vec<String> = columns.iter().map(|c| quote_if_needed(c, kind)).collect();
    format!("{insert_sql} RETURNING {}", cols.join(", "))
}

/// The key to read an inserted row back by (MySQL): the key columns the user filled in, else the
/// generated id for a single-column key. `None` when neither is known.
pub fn insert_key(
    key_columns: &[String],
    values: &[(String, Value)],
    last_insert_id: Option<i64>,
) -> Option<Vec<(String, Value)>> {
    if key_columns.is_empty() {
        return None;
    }
    let typed: Vec<(String, Value)> = key_columns
        .iter()
        .filter_map(|k| values.iter().find(|(c, _)| c == k).map(|(c, v)| (c.clone(), v.clone())))
        .collect();
    if typed.len() == key_columns.len() && typed.iter().all(|(_, v)| !v.is_null()) {
        return Some(typed);
    }
    match (key_columns, last_insert_id) {
        ([k], Some(id)) if id > 0 => Some(vec![(k.clone(), Value::Int(id))]),
        _ => None,
    }
}

/// The statement that reads an inserted row back by `key`.
pub fn read_back_select(
    kind: DatabaseKind,
    target: &EditTarget,
    columns: &[String],
    key: &[(String, Value)],
) -> String {
    build_select_by_key(kind, target.schema.as_deref(), &target.table, columns, key)
}

/// What is stored for one inserted row: a value per result column, `None` where the read-back
/// does not cover the column (an expression of the query, say).
pub type StoredRow = Vec<Option<Value>>;

/// Everything needed to read the inserted rows of one submit back from the database.
#[derive(Debug, Clone)]
pub struct InsertBack {
    mode: ReadBack,
    kind: DatabaseKind,
    target: EditTarget,
    /// (index into the result's columns, the table column it comes from).
    columns: Vec<(usize, String)>,
    ncols: usize,
    key_names: Vec<String>,
    /// The values the user typed, per insert in submit order.
    inserts: Vec<Vec<(String, Value)>>,
}

impl InsertBack {
    /// `None` when the submit inserts nothing.
    pub fn new(
        kind: DatabaseKind,
        server_version: &str,
        target: &EditTarget,
        rs: &ResultSet,
        changes: &[RowChange],
    ) -> Option<InsertBack> {
        let inserts: Vec<Vec<(String, Value)>> = changes
            .iter()
            .filter_map(|c| match c {
                RowChange::Insert { values } => Some(values.clone()),
                _ => None,
            })
            .collect();
        if inserts.is_empty() {
            return None;
        }
        let name_of = |c: usize| rs.columns.get(c).and_then(|c| c.origin.as_ref()).map(|o| o.column.clone());
        let columns = (0..rs.columns.len()).filter_map(|c| name_of(c).map(|n| (c, n))).collect();
        let key_names = target.key_columns.iter().filter_map(|&c| name_of(c)).collect();
        Some(InsertBack {
            mode: read_back(kind, server_version),
            kind,
            target: target.clone(),
            columns,
            ncols: rs.columns.len(),
            key_names,
            inserts,
        })
    }

    pub fn insert_count(&self) -> usize {
        self.inserts.len()
    }

    /// Whether the database is asked for the stored row at all (else the caller reloads the result).
    pub fn mode(&self) -> ReadBack {
        self.mode
    }

    fn spread(&self, row: Vec<Value>) -> StoredRow {
        let mut out = vec![None; self.ncols];
        for ((c, _), v) in self.columns.iter().zip(row) {
            out[*c] = Some(v);
        }
        out
    }

    /// Run insert number `which` (`sql` is its plain `INSERT`) and return what was stored, `None` when
    /// it could not be determined. A failing read-back is not an error: the row is in.
    pub async fn run(
        &self,
        c: &Arc<dyn Connection>,
        sql: &str,
        which: usize,
        opts: &ExecOptions,
    ) -> Result<Option<StoredRow>, Error> {
        let names: Vec<String> = self.columns.iter().map(|(_, n)| n.clone()).collect();
        if names.is_empty() {
            c.execute(sql, opts).await?;
            return Ok(None);
        }
        let first_row =
            |out: theta_core::StatementOutcome| out.result.and_then(|r| r.rows.into_iter().next());
        match self.mode {
            ReadBack::Returning => {
                let out = c.execute(&with_returning(self.kind, sql, &names), opts).await?;
                Ok(first_row(out).map(|r| self.spread(r)))
            }
            ReadBack::LastInsertId => {
                c.execute(sql, opts).await?;
                let typed = &self.inserts[which];
                let mut key = insert_key(&self.key_names, typed, None);
                if key.is_none() {
                    let id = c
                        .execute("SELECT LAST_INSERT_ID()", opts)
                        .await
                        .ok()
                        .and_then(first_row)
                        .and_then(|r| r.into_iter().next())
                        .and_then(|v| match v {
                            Value::Int(i) => Some(i),
                            Value::UInt(u) => i64::try_from(u).ok(),
                            Value::Decimal(s) => s.parse().ok(),
                            _ => None,
                        });
                    key = insert_key(&self.key_names, typed, id);
                }
                let Some(key) = key else { return Ok(None) };
                let q = read_back_select(self.kind, &self.target, &names, &key);
                Ok(c.execute(&q, opts).await.ok().and_then(first_row).map(|r| self.spread(r)))
            }
            ReadBack::Reload => {
                c.execute(sql, opts).await?;
                Ok(None)
            }
        }
    }
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
    use theta_core::Value;

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
    fn a_failed_statement_is_traced_to_its_change() {
        let ch = vec![
            RowChange::Update { row: 4, key: vec![("id".into(), Value::Int(7))], set: vec![] },
            RowChange::Update {
                row: 5,
                key: vec![("id".into(), Value::Int(9))],
                set: vec![("n".into(), Value::Int(1))],
            },
            RowChange::Delete { row: 1, key: vec![("code".into(), Value::Text("A".into()))] },
            RowChange::Insert { values: vec![] },
            RowChange::Insert { values: vec![] },
        ];
        assert_eq!(locate(&ch, 0), Some(("id = 9".into(), ChangeLocation::Row(5))));
        assert_eq!(locate(&ch, 1), Some(("code = 'A'".into(), ChangeLocation::Row(1))));
        assert_eq!(locate(&ch, 3), Some(("new row 2".into(), ChangeLocation::NewRow(1))));
        assert_eq!(locate(&ch, 4), None);
    }

    #[test]
    fn read_back_by_product() {
        use DatabaseKind::*;
        assert_eq!(read_back(Postgres, "PostgreSQL 16.4"), ReadBack::Returning);
        assert_eq!(read_back(CockroachDb, ""), ReadBack::Returning);
        assert_eq!(read_back(Sqlite, "SQLite 3.46.0"), ReadBack::Returning);
        assert_eq!(read_back(Sqlite, "SQLite 3.34.1"), ReadBack::Reload);
        assert_eq!(read_back(MySql, "MariaDB 10.11.6"), ReadBack::Returning);
        assert_eq!(read_back(MySql, "MariaDB 10.4.30"), ReadBack::LastInsertId);
        assert_eq!(read_back(MySql, "MySQL 8.0.36"), ReadBack::LastInsertId);
        assert_eq!(read_back(SqlServer, "SQL Server 2022"), ReadBack::Reload);
        assert_eq!(read_back(Redshift, "PostgreSQL 8.0.2"), ReadBack::Reload);
    }

    #[test]
    fn returning_clause_and_keys() {
        let sql = with_returning(
            DatabaseKind::Postgres,
            "INSERT INTO t (a) VALUES (1)",
            &["id".into(), "Mixed".into()],
        );
        assert_eq!(sql, "INSERT INTO t (a) VALUES (1) RETURNING id, \"Mixed\"");
        let typed = vec![("id".to_string(), Value::Int(5))];
        assert_eq!(insert_key(&["id".into()], &typed, None), Some(typed.clone()));
        assert_eq!(insert_key(&["id".into()], &[], Some(9)), Some(vec![("id".to_string(), Value::Int(9))]));
        assert_eq!(insert_key(&["id".into()], &[], Some(0)), None);
        assert_eq!(
            insert_key(&["a".into(), "b".into()], &[], Some(9)),
            None,
            "composite keys are not generated"
        );
        assert_eq!(insert_key(&[], &typed, Some(9)), None);
    }

    #[test]
    fn transaction_wrapping() {
        let w = with_transaction(vec!["X".into()], false);
        assert_eq!(w, vec!["BEGIN", "X", "COMMIT"]);
        assert_eq!(with_transaction(vec!["X".into()], true), vec!["X"]);
    }
}
