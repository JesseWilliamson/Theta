//! From what the dialog collected to an [`ImportJob`], and the checks that make the dialog say "no" before
//! anything runs. Pure, so every rule has a unit test and the dialog stays a thin view.

use crate::convert::{ConvertOptions, Target};
use crate::infer::ColumnType;
use crate::job::*;
use crate::parser::CsvOptions;
use crate::plan::{NewColumn, create_table_sql};
use theta_core::Family;
use theta_core::{ColumnInfo, DatabaseKind, TypeKind};
use theta_sql::keywords::qualified_name;

/// One column of a new table, aligned with the file's columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewColumnSpec {
    pub name: String,
    pub sql_type: String,
    pub nullable: bool,
    pub skip: bool,
}

#[derive(Debug, Clone)]
pub enum Destination {
    Existing {
        schema: Option<String>,
        table: String,
        columns: Vec<ColumnInfo>,
        /// For each file column, the index into `columns` it feeds (`None`: skipped).
        mapping: Vec<Option<usize>>,
    },
    New {
        schema: Option<String>,
        table: String,
        /// One entry per file column.
        columns: Vec<NewColumnSpec>,
    },
}

#[derive(Debug, Clone)]
pub struct ImportSetup {
    pub kind: DatabaseKind,
    pub csv: CsvOptions,
    pub has_header: bool,
    pub convert: ConvertOptions,
    pub on_error: ErrorMode,
    pub batch_rows: usize,
    pub max_statement_bytes: usize,
    pub truncate_first: bool,
    pub txn: TxnPlan,
    pub dest: Destination,
}

/// The statement that empties a table inside the import's transaction. Postgres `TRUNCATE` is
/// transactional; MySQL `TRUNCATE` is DDL and would commit implicitly, and SQLite has none, so those use
/// `DELETE FROM`, which a rollback can undo.
pub fn truncate_sql(kind: DatabaseKind, schema: Option<&str>, table: &str) -> String {
    let name = qualified_name(schema, table, kind);
    match kind.family() {
        Family::Postgres => format!("TRUNCATE TABLE {name}"),
        _ => format!("DELETE FROM {name}"),
    }
}

/// Why this setup cannot run, one message per problem. Empty means it can.
pub fn check(setup: &ImportSetup) -> Vec<String> {
    let mut problems = Vec::new();
    match &setup.dest {
        Destination::Existing { table, columns, mapping, .. } => {
            if table.trim().is_empty() {
                problems.push("Choose the table to import into.".to_string());
            }
            let mut used: Vec<usize> = Vec::new();
            for d in mapping.iter().flatten() {
                if used.contains(d) {
                    let n = columns.get(*d).map_or("?", |c| c.name.as_str());
                    problems.push(format!("Two file columns are mapped to {n}."));
                }
                used.push(*d);
            }
            if used.is_empty() {
                problems.push("No file column is mapped to a table column.".to_string());
            }
            for (i, c) in columns.iter().enumerate() {
                if !c.nullable && c.default.is_none() && !used.contains(&i) && !is_generated(c) {
                    problems.push(format!(
                        "{} is NOT NULL without a default and no file column feeds it.",
                        c.name
                    ));
                }
            }
        }
        Destination::New { table, columns, .. } => {
            if table.trim().is_empty() {
                problems.push("Enter a name for the new table.".to_string());
            }
            let kept: Vec<&NewColumnSpec> = columns.iter().filter(|c| !c.skip).collect();
            if kept.is_empty() {
                problems.push("Every column is skipped.".to_string());
            }
            for (i, c) in kept.iter().enumerate() {
                if c.name.trim().is_empty() {
                    problems.push("A column has no name.".to_string());
                }
                if c.sql_type.trim().is_empty() {
                    problems.push(format!("Column {} has no type.", c.name));
                }
                if kept[..i].iter().any(|o| o.name.eq_ignore_ascii_case(&c.name)) {
                    problems.push(format!("Column name {} appears twice.", c.name));
                }
            }
        }
    }
    if setup.batch_rows == 0 {
        problems.push("The batch size must be at least 1.".to_string());
    }
    problems
}

/// Identity and serial columns fill themselves in; their defaults are not always visible in the catalog.
fn is_generated(c: &ColumnInfo) -> bool {
    let t = c.data_type.to_ascii_lowercase();
    t.contains("serial")
        || c.default.as_deref().is_some_and(|d| d.contains("nextval") || d.contains("auto_increment"))
}

/// Build the job. Fails with the problems [`check`] reports.
pub fn build_job(setup: &ImportSetup) -> Result<ImportJob, Vec<String>> {
    let problems = check(setup);
    if !problems.is_empty() {
        return Err(problems);
    }
    let kind = setup.kind;
    let (schema, table, create_sql, columns) = match &setup.dest {
        Destination::Existing { schema, table, columns: cols, mapping } => {
            let columns = mapping
                .iter()
                .enumerate()
                .filter_map(|(src, d)| {
                    let c = &cols[(*d)?];
                    Some(ImportColumn {
                        source: src,
                        name: c.name.clone(),
                        target: Target::from_column(c, kind),
                    })
                })
                .collect();
            (schema.clone(), table.clone(), None, columns)
        }
        Destination::New { schema, table, columns: specs } => {
            let columns: Vec<ImportColumn> = specs
                .iter()
                .enumerate()
                .filter(|(_, c)| !c.skip)
                .map(|(src, c)| ImportColumn {
                    source: src,
                    name: c.name.trim().to_string(),
                    target: Target::from_type_name(&c.sql_type, kind, c.nullable),
                })
                .collect();
            let new: Vec<NewColumn> = specs
                .iter()
                .filter(|c| !c.skip)
                .map(|c| NewColumn {
                    name: c.name.trim().to_string(),
                    sql_type: c.sql_type.trim().to_string(),
                    nullable: c.nullable,
                })
                .collect();
            let create = create_table_sql(kind, schema.as_deref(), table.trim(), &new);
            (schema.clone(), table.trim().to_string(), Some(create), columns)
        }
    };
    let truncate =
        (setup.truncate_first && create_sql.is_none()).then(|| truncate_sql(kind, schema.as_deref(), &table));
    Ok(ImportJob {
        kind,
        schema,
        table,
        create_sql,
        truncate_sql: truncate,
        columns,
        csv: setup.csv.clone(),
        has_header: setup.has_header,
        convert: setup.convert.clone(),
        on_error: setup.on_error,
        batch_rows: setup.batch_rows,
        max_statement_bytes: setup.max_statement_bytes,
        txn: setup.txn.clone(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Compat {
    Ok,
    /// Probably fine, but worth a glance (e.g. text going into a number column).
    Check(String),
    /// Will fail for every non-empty value.
    Mismatch(String),
}

/// Compare what the sample looks like with what the target column accepts.
pub fn compatibility(inferred: &ColumnType, target: &Target) -> Compat {
    use TypeKind::*;
    let src = inferred.label();
    match (inferred, target.kind) {
        (_, Text | Other | Array | Geometry | Json | Interval | Bytes | Uuid) => Compat::Ok,
        (ColumnType::Integer | ColumnType::BigInt, Integer | Decimal | Float | Bool) => Compat::Ok,
        (ColumnType::Numeric { .. }, Decimal | Float) => Compat::Ok,
        (ColumnType::Numeric { .. }, Integer) => Compat::Check("decimals will be rejected".to_string()),
        (ColumnType::Float, Float | Decimal) => Compat::Ok,
        (ColumnType::Boolean, Bool) => Compat::Ok,
        (ColumnType::Date, Date | Timestamp) => Compat::Ok,
        (ColumnType::Timestamp { .. }, Timestamp) => Compat::Ok,
        (ColumnType::Timestamp { .. }, Date | Time) => Compat::Mismatch(format!("{src} values do not fit")),
        (ColumnType::Text { max_len: 0 }, _) => Compat::Ok,
        (ColumnType::Text { .. }, _) => Compat::Check("the sample looks like text".to_string()),
        (_, Date | Time | Timestamp | Bool | Integer | Decimal | Float) => {
            Compat::Mismatch(format!("{src} values will not convert"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, ty: &str, kind: TypeKind, nullable: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            data_type: ty.into(),
            kind,
            nullable,
            default: None,
            comment: None,
            geometry: None,
            ..Default::default()
        }
    }

    fn setup(dest: Destination) -> ImportSetup {
        ImportSetup {
            kind: DatabaseKind::Postgres,
            csv: CsvOptions::default(),
            has_header: true,
            convert: ConvertOptions::default(),
            on_error: ErrorMode::Abort,
            batch_rows: 1000,
            max_statement_bytes: DEFAULT_STATEMENT_BYTES,
            truncate_first: false,
            txn: TxnPlan::Own { begin: vec!["BEGIN".into()] },
            dest,
        }
    }

    fn existing(mapping: Vec<Option<usize>>) -> Destination {
        Destination::Existing {
            schema: Some("public".into()),
            table: "people".into(),
            columns: vec![
                info("id", "integer", TypeKind::Integer, false),
                info("name", "text", TypeKind::Text, true),
                info("born", "date", TypeKind::Date, true),
            ],
            mapping,
        }
    }

    #[test]
    fn existing_mapping_builds_columns_in_file_order() {
        let job = build_job(&setup(existing(vec![Some(1), None, Some(0)]))).unwrap();
        let cols: Vec<(&str, usize)> = job.columns.iter().map(|c| (c.name.as_str(), c.source)).collect();
        assert_eq!(cols, vec![("name", 0), ("id", 2)]);
        assert!(job.create_sql.is_none() && job.truncate_sql.is_none());
    }

    #[test]
    fn problems_are_reported_before_running() {
        let p = check(&setup(existing(vec![Some(1), Some(1)])));
        assert!(p.iter().any(|m| m.contains("Two file columns")));
        assert!(p.iter().any(|m| m.contains("id is NOT NULL")));
        let p = check(&setup(existing(vec![None, None])));
        assert!(p.iter().any(|m| m.contains("No file column is mapped")));
        let mut s = setup(existing(vec![Some(0)]));
        s.batch_rows = 0;
        assert!(check(&s).iter().any(|m| m.contains("batch size")));
    }

    #[test]
    fn serial_columns_need_no_input() {
        let mut dest = existing(vec![Some(1)]);
        if let Destination::Existing { columns, .. } = &mut dest {
            columns[0].default = Some("nextval('people_id_seq'::regclass)".into());
        }
        assert!(check(&setup(dest)).is_empty());
    }

    #[test]
    fn new_table_creates_and_validates_names() {
        let dest = Destination::New {
            schema: None,
            table: "fresh".into(),
            columns: vec![
                NewColumnSpec { name: "id".into(), sql_type: "integer".into(), nullable: false, skip: false },
                NewColumnSpec { name: "junk".into(), sql_type: "text".into(), nullable: true, skip: true },
                NewColumnSpec { name: "name".into(), sql_type: "text".into(), nullable: true, skip: false },
            ],
        };
        let job = build_job(&setup(dest.clone())).unwrap();
        assert_eq!(
            job.create_sql.as_deref(),
            Some("CREATE TABLE fresh (\n    id integer NOT NULL,\n    name text\n)")
        );
        assert_eq!(job.columns.iter().map(|c| c.source).collect::<Vec<_>>(), vec![0, 2]);
        // Truncate does not apply to a table that does not exist yet.
        let mut s = setup(dest);
        s.truncate_first = true;
        assert!(build_job(&s).unwrap().truncate_sql.is_none());

        let dup = Destination::New {
            schema: None,
            table: "".into(),
            columns: vec![
                NewColumnSpec { name: "a".into(), sql_type: "text".into(), nullable: true, skip: false },
                NewColumnSpec { name: "A".into(), sql_type: "".into(), nullable: true, skip: false },
            ],
        };
        let p = check(&setup(dup));
        assert_eq!(p.len(), 3, "{p:?}");
    }

    #[test]
    fn truncate_per_dialect() {
        assert_eq!(truncate_sql(DatabaseKind::Postgres, Some("s"), "t"), "TRUNCATE TABLE s.t");
        assert_eq!(truncate_sql(DatabaseKind::MySql, Some("s"), "t"), "DELETE FROM s.t");
        assert_eq!(truncate_sql(DatabaseKind::Sqlite, None, "t"), "DELETE FROM t");
        let mut s = setup(existing(vec![Some(0)]));
        s.truncate_first = true;
        assert_eq!(build_job(&s).unwrap().truncate_sql.as_deref(), Some("TRUNCATE TABLE public.people"));
    }

    #[test]
    fn compatibility_flags_mismatches() {
        let int = Target::from_column(&info("n", "integer", TypeKind::Integer, true), DatabaseKind::Postgres);
        let date = Target::from_column(&info("d", "date", TypeKind::Date, true), DatabaseKind::Postgres);
        let text = Target::from_column(&info("t", "text", TypeKind::Text, true), DatabaseKind::Postgres);
        assert_eq!(compatibility(&ColumnType::Integer, &int), Compat::Ok);
        assert_eq!(compatibility(&ColumnType::Integer, &text), Compat::Ok);
        assert!(matches!(
            compatibility(&ColumnType::Numeric { precision: 4, scale: 2 }, &int),
            Compat::Check(_)
        ));
        assert!(matches!(compatibility(&ColumnType::Boolean, &date), Compat::Mismatch(_)));
        assert!(matches!(compatibility(&ColumnType::Text { max_len: 5 }, &int), Compat::Check(_)));
        assert_eq!(compatibility(&ColumnType::Date, &date), Compat::Ok);
    }
}
