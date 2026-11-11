//! Theta's own SQL dump: structure from the DDL engine, data as batched `INSERT`s, streamed to a file.
//!
//! It exists so a dump needs no `pg_dump` or `mysqldump` on the machine, and so one tool works for every
//! product that has a verified DDL generator (`Feature::Dump`). It is *not* a complete backup: it reads
//! tables, views and (PostgreSQL / DuckDB) the sequences that columns use, and nothing else. [`not_captured`]
//! says what that leaves out, and the dialog shows it before the file is written.
//!
//! The work is split in two so the interesting part is testable without a database. [`plan`] is pure: it
//! orders the tables so every foreign key's target comes first, decides whether foreign keys go inline or
//! after the data, and emits every statement except the row data. [`dump`] walks that plan, writing
//! statements as they come and streaming each table's rows through [`export_table`], so memory stays at one
//! page of rows however large the database is.

use crate::export::{ExportJob, ExportOutcome, ExportProgress, export_table};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use theta_core::{Catalog, Connection, DatabaseKind, Family, Schema, Table, TableKind};
use theta_sql::ExportFormat;
use theta_sql::ddl::{self, TableDraft};
use theta_sql::keywords::{qualified_name, quote_if_needed};

#[derive(Debug, Clone)]
pub struct DumpOptions {
    pub kind: DatabaseKind,
    /// `(schema, name)` of the tables to dump; views are chosen the same way.
    pub tables: Vec<(String, String)>,
    pub views: Vec<(String, String)>,
    pub structure: bool,
    pub data: bool,
    /// Write `schema.table`; off writes bare names so the dump can be restored into another schema or database
    /// (set the target with `search_path` / `USE` first). Only valid for one schema.
    pub qualify: bool,
    pub rows_per_insert: usize,
    pub page_rows: usize,
}

impl DumpOptions {
    pub fn new(kind: DatabaseKind) -> Self {
        DumpOptions {
            kind,
            tables: Vec::new(),
            views: Vec::new(),
            structure: true,
            data: true,
            qualify: true,
            rows_per_insert: 500,
            page_rows: 2000,
        }
    }
}

/// Where a foreign key goes. SQLite and DuckDB cannot add one to an existing table, so for them the tables
/// are sorted and the keys are inline; for the others every table is created bare, the data loaded, and the
/// keys added at the end, which also copes with cycles.
fn keys_after_data(kind: DatabaseKind) -> bool {
    matches!(kind.family(), Family::Postgres | Family::MySql)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// One statement, written followed by `;`. Lines starting with `--` are comments and get no `;`.
    Sql(String),
    Data(TableData),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableData {
    pub schema: Option<String>,
    pub table: String,
    pub key: Vec<String>,
    /// Columns that can be inserted (generated ones cannot).
    pub columns: Vec<String>,
    pub overriding_system_value: bool,
    pub estimate: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// Things the user should know about this particular dump (a foreign key left out, a cycle).
    pub notes: Vec<String>,
}

/// What no dump from this module contains, for this product. Shown before writing and repeated in the file's
/// header, so a dump is never mistaken for a full backup.
pub fn not_captured(kind: DatabaseKind) -> Vec<&'static str> {
    let mut v = vec![
        "users, roles and every GRANT / REVOKE (privileges)",
        "the data of views (a view is recreated from its definition)",
    ];
    match kind.family() {
        Family::Postgres => v.extend([
            "functions, procedures and triggers",
            "extensions (CREATE EXTENSION), custom types (enums, domains, composites) and operators",
            "ownership: every object is owned by the user who restores the dump",
            "materialized views, partitioning, row-level-security policies and tablespaces",
            "sequences that no column uses, and a sequence's options other than its current value",
            "comments on objects other than tables and columns",
        ]),
        Family::MySql => v.extend([
            "stored routines, triggers and events",
            "views' DEFINER and SQL SECURITY, and the definitions' qualification by database name (a view body is copied as the server prints it)",
            "partitioning, the storage engine / character set options and FULLTEXT or SPATIAL index types",
            "executable comments (/*!40101 ... */) are not in a Theta dump, and are skipped when restoring one made by mysqldump",
        ]),
        Family::Sqlite => v.extend([
            "triggers and virtual tables (FTS, R*Tree)",
            "PRAGMA settings such as user_version and the journal mode",
        ]),
        Family::DuckDb => v.extend([
            "macros, custom types, secrets, extensions and attached databases",
            "sequence positions (DuckDB has no setval), so a restored table's next generated id restarts at the sequence's start",
        ]),
        _ => {}
    }
    v
}

fn header(opts: &DumpOptions) -> Vec<String> {
    let mut v = vec![
        format!("-- Theta SQL dump for {}", opts.kind.label()),
        "-- Complete for: the tables and views chosen, with their data as batched INSERTs.".into(),
        "-- Not captured:".into(),
    ];
    v.extend(not_captured(opts.kind).into_iter().map(|l| format!("--   {l}")));
    v
}

fn session_prologue(kind: DatabaseKind) -> (Vec<&'static str>, Vec<&'static str>) {
    match kind.family() {
        Family::MySql => {
            (vec!["SET NAMES utf8mb4", "SET FOREIGN_KEY_CHECKS = 0"], vec!["SET FOREIGN_KEY_CHECKS = 1"])
        }
        Family::Sqlite => (vec!["PRAGMA foreign_keys = OFF"], vec!["PRAGMA foreign_keys = ON"]),
        Family::Postgres => (vec!["SET client_encoding = 'UTF8'"], vec![]),
        _ => (vec![], vec![]),
    }
}

/// The sequence a `nextval('...')` default names, spelled as in the default.
fn nextval_target(default: &str) -> Option<String> {
    let at = default.to_ascii_lowercase().find("nextval('")? + "nextval('".len();
    let rest = &default[at..];
    let mut name = String::new();
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            if chars.peek() == Some(&'\'') {
                name.push('\'');
                chars.next();
                continue;
            }
            break;
        }
        name.push(c);
    }
    Some(name).filter(|n| !n.is_empty())
}

/// Order `tables` so a table follows the ones it references; self references are ignored. Tables caught in a
/// cycle are appended in their original order and returned separately.
fn order_by_dependency(tables: &[(&Schema, &Table)]) -> (Vec<usize>, Vec<usize>) {
    let index: HashMap<(String, String), usize> = tables
        .iter()
        .enumerate()
        .map(|(i, (s, t))| ((s.name.to_lowercase(), t.name.to_lowercase()), i))
        .collect();
    let deps: Vec<HashSet<usize>> = tables
        .iter()
        .enumerate()
        .map(|(i, (s, t))| {
            t.foreign_keys
                .iter()
                .filter_map(|f| {
                    let schema = f.referenced_schema.as_deref().unwrap_or(&s.name).to_lowercase();
                    index.get(&(schema, f.referenced_table.to_lowercase())).copied()
                })
                .filter(|d| *d != i)
                .collect()
        })
        .collect();
    let (mut done, mut order) = (HashSet::new(), Vec::new());
    loop {
        let ready: Vec<usize> = (0..tables.len())
            .filter(|i| !done.contains(i) && deps[*i].iter().all(|d| done.contains(d)))
            .collect();
        if ready.is_empty() {
            break;
        }
        for i in ready {
            done.insert(i);
            order.push(i);
        }
    }
    let cyclic: Vec<usize> = (0..tables.len()).filter(|i| !done.contains(i)).collect();
    order.extend(&cyclic);
    (order, cyclic)
}

/// Statements and data steps for `opts`, in restore order. Pure.
pub fn plan(catalog: &Catalog, opts: &DumpOptions) -> Plan {
    let kind = opts.kind;
    let mut out = Plan::default();
    let mut sql = |s: String| out.steps.push(Step::Sql(s));
    let (pre, post) = session_prologue(kind);

    let lookup = |sel: &(String, String)| {
        catalog.schema(&sel.0).and_then(|s| s.tables.iter().find(|t| t.name == sel.1).map(|t| (s, t)))
    };
    let tables: Vec<(&Schema, &Table)> =
        opts.tables.iter().filter_map(lookup).filter(|(_, t)| t.kind == TableKind::Table).collect();
    let views: Vec<(&Schema, &Table)> =
        opts.views.iter().filter_map(lookup).filter(|(_, t)| matches!(t.kind, TableKind::View)).collect();
    let mut notes = Vec::new();

    for l in header(opts) {
        sql(l);
    }
    for s in &pre {
        sql((*s).to_string());
    }
    let schema_of = |s: &Schema| -> Option<String> {
        (opts.qualify && kind.family() != Family::Sqlite && !s.name.is_empty()).then(|| s.name.clone())
    };
    let qualify_name = |s: &Schema, t: &str| qualified_name(schema_of(s).as_deref(), t, kind);

    if opts.structure && opts.qualify && kind.family() == Family::Postgres {
        let mut seen = HashSet::new();
        for (s, _) in tables.iter().chain(views.iter()) {
            if s.name != "public" && seen.insert(s.name.clone()) {
                sql(format!("CREATE SCHEMA IF NOT EXISTS {}", quote_if_needed(&s.name, kind)));
            }
        }
    }

    let (order, cyclic) = order_by_dependency(&tables);
    let after = keys_after_data(kind);
    if !cyclic.is_empty() && !after {
        notes.push(format!(
            "Tables {} reference each other in a cycle; {} cannot add a foreign key later, so the dump may not restore with foreign keys enforced.",
            cyclic.iter().map(|i| tables[*i].1.name.clone()).collect::<Vec<_>>().join(", "),
            kind.label()
        ));
    }
    let included: HashSet<(String, String)> =
        tables.iter().map(|(s, t)| (s.name.to_lowercase(), t.name.to_lowercase())).collect();

    let mut late: Vec<String> = Vec::new();
    let mut seq_fix: Vec<String> = Vec::new();
    let mut data_steps: Vec<Step> = Vec::new();
    let mut structure: Vec<String> = Vec::new();
    let mut sequences: Vec<String> = Vec::new();

    for &i in &order {
        let (s, t) = tables[i];
        let mut draft = TableDraft::from_table(&s.name, t);
        draft.schema = schema_of(s);
        // A key into a table that is not in the dump would fail the restore.
        let ids: Vec<u32> = draft
            .foreign_keys
            .iter()
            .filter(|f| {
                let rs = f.ref_schema.clone().unwrap_or_else(|| s.name.clone()).to_lowercase();
                !included.contains(&(rs, f.ref_table.to_lowercase()))
            })
            .map(|f| f.id)
            .collect();
        for f in draft.foreign_keys.iter().filter(|f| ids.contains(&f.id)) {
            notes.push(format!(
                "{}: the foreign key to {} is left out because that table is not in the dump.",
                t.name, f.ref_table
            ));
        }
        draft.foreign_keys.retain(|f| !ids.contains(&f.id));
        if !opts.qualify {
            for f in &mut draft.foreign_keys {
                f.ref_schema = None;
            }
        } else if kind.family() != Family::Sqlite {
            for f in &mut draft.foreign_keys {
                f.ref_schema.get_or_insert_with(|| s.name.clone());
            }
        }

        if opts.structure {
            // Sequences a column's default calls (PostgreSQL `serial`, DuckDB `nextval`).
            if matches!(kind.family(), Family::Postgres | Family::DuckDb) {
                for c in &t.columns {
                    if let Some(seq) = c.default.as_deref().and_then(nextval_target)
                        && !sequences.contains(&seq)
                    {
                        sequences.push(seq.clone());
                        structure.push(format!("CREATE SEQUENCE IF NOT EXISTS {seq}"));
                        if kind.family() == Family::Postgres {
                            late.push(format!(
                                "ALTER SEQUENCE {seq} OWNED BY {}.{}",
                                qualify_name(s, &t.name),
                                quote_if_needed(&c.name, kind)
                            ));
                        }
                    }
                }
            }
            let full = draft.clone();
            let mut bare = draft.clone();
            if after {
                bare.foreign_keys.clear();
            }
            structure.extend(ddl::create_table(&bare, kind).into_iter().map(|st| st.sql));
            if after && !full.foreign_keys.is_empty() {
                late.extend(ddl::diff(&bare, &full, kind).into_iter().map(|st| st.sql));
            }
        }
        if opts.data {
            let columns: Vec<String> =
                t.columns.iter().filter(|c| c.generated.is_none()).map(|c| c.name.clone()).collect();
            let overriding = kind.family() == Family::Postgres
                && t.columns.iter().any(|c| c.identity == Some(theta_core::Identity::Always));
            data_steps.push(Step::Data(TableData {
                schema: schema_of(s),
                table: t.name.clone(),
                key: t.primary_key.clone(),
                columns,
                overriding_system_value: overriding,
                estimate: t.row_estimate,
            }));
            // Put identity / serial counters after the rows that were just inserted.
            if kind.family() == Family::Postgres {
                let tbl = qualify_name(s, &t.name);
                for c in &t.columns {
                    let col = quote_if_needed(&c.name, kind);
                    let seq = c.default.as_deref().and_then(nextval_target);
                    if c.identity.is_some() || seq.is_some() {
                        let target = match &seq {
                            Some(q) => format!("'{}'", q.replace('\'', "''")),
                            None => format!(
                                "pg_get_serial_sequence('{}', '{}')",
                                tbl.replace('\'', "''"),
                                c.name.replace('\'', "''")
                            ),
                        };
                        seq_fix.push(format!(
                            "SELECT setval({target}, COALESCE((SELECT MAX({col}) FROM {tbl}), 1), (SELECT MAX({col}) IS NOT NULL FROM {tbl}))"
                        ));
                    }
                }
            }
        }
    }

    if opts.structure {
        for q in structure {
            sql(q);
        }
    }
    out.steps.extend(data_steps);
    let mut sql = |s: String| out.steps.push(Step::Sql(s));
    for q in seq_fix {
        sql(q);
    }
    for q in late {
        sql(q);
    }

    if opts.structure && !views.is_empty() {
        // A view may select from another; put the ones mentioning other chosen views' names last.
        let mut remaining: Vec<(&Schema, &Table)> = views.clone();
        let mut placed: Vec<&str> = Vec::new();
        while !remaining.is_empty() {
            let pick = remaining
                .iter()
                .position(|(_, v)| {
                    let def = v.view_definition.as_deref().unwrap_or("").to_lowercase();
                    remaining.iter().all(|(_, o)| {
                        o.name == v.name
                            || placed.contains(&o.name.as_str())
                            || !def.contains(&o.name.to_lowercase())
                    })
                })
                .unwrap_or(0);
            let (s, v) = remaining.remove(pick);
            placed.push(&v.name);
            match v.view_definition.as_deref().map(|d| d.trim().trim_end_matches(';').trim()) {
                Some(def) if !def.is_empty() => {
                    sql(format!("CREATE VIEW {} AS {def}", qualify_name(s, &v.name)));
                }
                _ => notes.push(format!(
                    "{}: the server did not give this view's definition, so it is not in the dump.",
                    v.name
                )),
            }
        }
    }
    for s in &post {
        sql((*s).to_string());
    }
    out.notes = notes;
    out
}

#[derive(Debug, Clone, Default)]
pub struct DumpProgress {
    pub table: Option<String>,
    pub tables_done: usize,
    pub tables_total: usize,
    pub rows: u64,
    pub bytes: u64,
    pub elapsed: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DumpOutcome {
    Completed,
    Cancelled,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct DumpReport {
    pub outcome: DumpOutcome,
    pub tables: usize,
    pub rows: u64,
    pub bytes: u64,
    pub notes: Vec<String>,
    pub elapsed: Duration,
}

/// Write the dump to `out`. Stops between statements and between pages when `cancel` is set.
pub async fn dump<W: Write + Send>(
    conn: &dyn Connection,
    catalog: &Catalog,
    opts: &DumpOptions,
    mut out: W,
    cancel: &AtomicBool,
    progress: &(dyn Fn(&DumpProgress) + Send + Sync),
) -> DumpReport {
    let started = Instant::now();
    let plan = plan(catalog, opts);
    let total = plan.steps.iter().filter(|s| matches!(s, Step::Data(_))).count();
    let mut rep = DumpReport {
        outcome: DumpOutcome::Completed,
        tables: 0,
        rows: 0,
        bytes: 0,
        notes: plan.notes.clone(),
        elapsed: Duration::ZERO,
    };
    let fail = |rep: &mut DumpReport, msg: String| {
        rep.outcome = DumpOutcome::Failed(msg);
    };
    for step in &plan.steps {
        if cancel.load(Ordering::Relaxed) {
            rep.outcome = DumpOutcome::Cancelled;
            break;
        }
        match step {
            Step::Sql(s) => {
                let text = if s.starts_with("--") { format!("{s}\n") } else { format!("{s};\n\n") };
                if let Err(e) = out.write_all(text.as_bytes()) {
                    fail(&mut rep, format!("writing the file failed: {e}"));
                    break;
                }
                rep.bytes += text.len() as u64;
            }
            Step::Data(d) => {
                let target = qualified_name(d.schema.as_deref(), &d.table, opts.kind);
                let job = ExportJob {
                    kind: opts.kind,
                    schema: d.schema.clone(),
                    table: d.table.clone(),
                    key: d.key.clone(),
                    columns: Some(d.columns.clone()),
                    format: ExportFormat::SqlInsertsMulti {
                        table: target,
                        per_statement: opts.rows_per_insert,
                        overriding_system_value: d.overriding_system_value,
                    },
                    page_rows: opts.page_rows,
                };
                let (base_rows, base_bytes, done) = (rep.rows, rep.bytes, rep.tables);
                let name = d.table.clone();
                let report = {
                    let cb = |p: &ExportProgress| {
                        progress(&DumpProgress {
                            table: Some(name.clone()),
                            tables_done: done,
                            tables_total: total,
                            rows: base_rows + p.rows,
                            bytes: base_bytes + p.bytes,
                            elapsed: started.elapsed(),
                        })
                    };
                    export_table(conn, &mut out, &job, d.estimate, cancel, &cb).await
                };
                rep.rows += report.rows;
                rep.bytes += report.bytes;
                rep.tables += 1;
                match report.outcome {
                    ExportOutcome::Completed => {
                        let sep = b"\n";
                        let _ = out.write_all(sep);
                        rep.bytes += 1;
                    }
                    ExportOutcome::Cancelled => {
                        rep.outcome = DumpOutcome::Cancelled;
                        break;
                    }
                    ExportOutcome::Failed(m) => {
                        fail(&mut rep, format!("{}: {m}", d.table));
                        break;
                    }
                }
            }
        }
    }
    if rep.outcome == DumpOutcome::Completed
        && let Err(e) = out.flush()
    {
        fail(&mut rep, format!("writing the file failed: {e}"));
    }
    rep.elapsed = started.elapsed();
    progress(&DumpProgress {
        table: None,
        tables_done: rep.tables,
        tables_total: total,
        rows: rep.rows,
        bytes: rep.bytes,
        elapsed: rep.elapsed,
    });
    rep
}

/// [`dump`] into `path` through a `.part` file that is renamed on success and deleted otherwise.
pub async fn dump_to_path(
    conn: &dyn Connection,
    catalog: &Catalog,
    opts: &DumpOptions,
    path: &Path,
    cancel: &AtomicBool,
    progress: &(dyn Fn(&DumpProgress) + Send + Sync),
) -> DumpReport {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".part");
    let part = path.with_file_name(name);
    let file = match std::fs::File::create(&part) {
        Ok(f) => f,
        Err(e) => {
            return DumpReport {
                outcome: DumpOutcome::Failed(format!("cannot create {}: {e}", part.display())),
                tables: 0,
                rows: 0,
                bytes: 0,
                notes: Vec::new(),
                elapsed: Duration::ZERO,
            };
        }
    };
    let writer = std::io::BufWriter::with_capacity(256 * 1024, file);
    let mut rep = dump(conn, catalog, opts, writer, cancel, progress).await;
    if rep.outcome == DumpOutcome::Completed {
        if let Err(e) = std::fs::rename(&part, path) {
            rep.outcome = DumpOutcome::Failed(format!("cannot move the finished file into place: {e}"));
            let _ = std::fs::remove_file(&part);
        }
    } else {
        let _ = std::fs::remove_file(&part);
    }
    rep
}

#[cfg(test)]
mod tests {
    use super::*;
    use theta_core::{ColumnInfo, ForeignKey, Identity, TypeKind};

    fn col(name: &str, ty: &str, kind: TypeKind) -> ColumnInfo {
        ColumnInfo { name: name.into(), data_type: ty.into(), kind, nullable: false, ..Default::default() }
    }

    fn table(name: &str, cols: Vec<ColumnInfo>, pk: &[&str], fks: Vec<ForeignKey>) -> Table {
        Table {
            name: name.into(),
            kind: TableKind::Table,
            columns: cols,
            primary_key: pk.iter().map(|s| s.to_string()).collect(),
            foreign_keys: fks,
            ..Default::default()
        }
    }

    fn fk(name: &str, cols: &[&str], to: &str) -> ForeignKey {
        ForeignKey {
            name: name.into(),
            columns: cols.iter().map(|s| s.to_string()).collect(),
            referenced_table: to.into(),
            referenced_columns: vec!["id".into()],
            ..Default::default()
        }
    }

    fn catalog(tables: Vec<Table>) -> Catalog {
        Catalog {
            schemas: vec![Schema { name: "public".into(), tables, ..Default::default() }],
            default_schema: Some("public".into()),
        }
    }

    fn shop() -> Catalog {
        let int = || col("id", "integer", TypeKind::Integer);
        // Deliberately listed child-first.
        catalog(vec![
            table(
                "order_items",
                vec![int(), col("order_id", "integer", TypeKind::Integer)],
                &["id"],
                vec![fk("oi_o", &["order_id"], "orders")],
            ),
            table(
                "orders",
                vec![int(), col("customer_id", "integer", TypeKind::Integer)],
                &["id"],
                vec![fk("o_c", &["customer_id"], "customers")],
            ),
            table(
                "customers",
                vec![ColumnInfo { identity: Some(Identity::ByDefault), ..int() }],
                &["id"],
                vec![],
            ),
        ])
    }

    fn opts(kind: DatabaseKind, c: &Catalog) -> DumpOptions {
        let mut o = DumpOptions::new(kind);
        o.tables = c.schemas[0].tables.iter().map(|t| ("public".to_string(), t.name.clone())).collect();
        o
    }

    fn sql_of(p: &Plan) -> Vec<String> {
        p.steps.iter().filter_map(|s| if let Step::Sql(s) = s { Some(s.clone()) } else { None }).collect()
    }

    fn data_tables(p: &Plan) -> Vec<String> {
        p.steps
            .iter()
            .filter_map(|s| if let Step::Data(d) = s { Some(d.table.clone()) } else { None })
            .collect()
    }

    #[test]
    fn tables_come_after_the_tables_they_reference() {
        let c = shop();
        let p = plan(&c, &opts(DatabaseKind::Sqlite, &c));
        assert_eq!(data_tables(&p), ["customers", "orders", "order_items"]);
        let creates: Vec<String> = sql_of(&p).into_iter().filter(|s| s.starts_with("CREATE TABLE")).collect();
        assert!(creates[0].contains("customers") && creates[2].contains("order_items"), "{creates:?}");
        // SQLite cannot add a key later, so it is inline.
        assert!(creates[1].contains("FOREIGN KEY") || creates[1].contains("REFERENCES"), "{}", creates[1]);
    }

    #[test]
    fn postgres_adds_keys_after_the_data() {
        let c = shop();
        let p = plan(&c, &opts(DatabaseKind::Postgres, &c));
        let positions = |pred: &dyn Fn(&Step) -> bool| p.steps.iter().position(pred).unwrap();
        let last_data = p.steps.iter().rposition(|s| matches!(s, Step::Data(_))).unwrap();
        let first_alter = positions(&|s| matches!(s, Step::Sql(q) if q.starts_with("ALTER TABLE")));
        let last_create =
            p.steps.iter().rposition(|s| matches!(s, Step::Sql(q) if q.starts_with("CREATE TABLE"))).unwrap();
        assert!(last_create < positions(&|s| matches!(s, Step::Data(_))));
        assert!(last_data < first_alter);
        let creates: Vec<String> = sql_of(&p).into_iter().filter(|s| s.starts_with("CREATE TABLE")).collect();
        assert!(creates.iter().all(|s| !s.contains("REFERENCES")), "{creates:?}");
        assert_eq!(sql_of(&p).iter().filter(|s| s.contains("FOREIGN KEY")).count(), 2);
    }

    #[test]
    fn identity_counters_are_restored_after_the_data() {
        let c = shop();
        let p = plan(&c, &opts(DatabaseKind::Postgres, &c));
        let all = sql_of(&p);
        let fix = all.iter().find(|s| s.starts_with("SELECT setval")).expect("a setval");
        assert!(fix.contains("pg_get_serial_sequence('public.customers', 'id')"), "{fix}");
    }

    #[test]
    fn a_key_into_an_unselected_table_is_left_out_with_a_note() {
        let c = shop();
        let mut o = opts(DatabaseKind::Postgres, &c);
        o.tables.retain(|t| t.1 != "customers");
        let p = plan(&c, &o);
        assert!(p.notes.iter().any(|n| n.contains("orders") && n.contains("customers")), "{:?}", p.notes);
        assert_eq!(sql_of(&p).iter().filter(|s| s.contains("FOREIGN KEY")).count(), 1);
    }

    #[test]
    fn cycles_are_reported_where_keys_must_be_inline() {
        let int = || col("id", "integer", TypeKind::Integer);
        let c = catalog(vec![
            table(
                "a",
                vec![int(), col("b_id", "integer", TypeKind::Integer)],
                &["id"],
                vec![fk("a_b", &["b_id"], "b")],
            ),
            table(
                "b",
                vec![int(), col("a_id", "integer", TypeKind::Integer)],
                &["id"],
                vec![fk("b_a", &["a_id"], "a")],
            ),
        ]);
        let p = plan(&c, &opts(DatabaseKind::Sqlite, &c));
        assert!(p.notes.iter().any(|n| n.contains("cycle")));
        assert_eq!(data_tables(&p).len(), 2, "both tables are still dumped");
        assert!(
            plan(&c, &opts(DatabaseKind::Postgres, &c)).notes.is_empty(),
            "PostgreSQL adds the keys last"
        );
    }

    #[test]
    fn structure_and_data_can_be_chosen_separately() {
        let c = shop();
        let mut o = opts(DatabaseKind::Postgres, &c);
        o.data = false;
        let p = plan(&c, &o);
        assert!(data_tables(&p).is_empty() && sql_of(&p).iter().any(|s| s.starts_with("CREATE TABLE")));
        o.data = true;
        o.structure = false;
        let p = plan(&c, &o);
        assert_eq!(data_tables(&p).len(), 3);
        assert!(!sql_of(&p).iter().any(|s| s.starts_with("CREATE TABLE") || s.starts_with("ALTER TABLE")));
    }

    #[test]
    fn generated_columns_are_not_inserted_and_always_identity_overrides() {
        let mut computed = col("total", "numeric", TypeKind::Decimal);
        computed.generated = Some(theta_core::GeneratedColumn { expression: "a*2".into(), stored: true });
        let id = ColumnInfo { identity: Some(Identity::Always), ..col("id", "integer", TypeKind::Integer) };
        let c = catalog(vec![table("t", vec![id, computed], &["id"], vec![])]);
        let p = plan(&c, &opts(DatabaseKind::Postgres, &c));
        let d = p.steps.iter().find_map(|s| if let Step::Data(d) = s { Some(d) } else { None }).unwrap();
        assert_eq!(d.columns, ["id"]);
        assert!(d.overriding_system_value);
    }

    #[test]
    fn unqualified_dumps_omit_schema_names() {
        let c = shop();
        let mut o = opts(DatabaseKind::Postgres, &c);
        o.qualify = false;
        let p = plan(&c, &o);
        assert!(
            sql_of(&p).iter().all(|s| s.starts_with("--") || !s.contains("\"public\".")),
            "{:?}",
            sql_of(&p)
        );
    }

    #[test]
    fn views_follow_the_tables_they_read() {
        let mut c = shop();
        let mut v1 = table("v_top", vec![], &[], vec![]);
        v1.kind = TableKind::View;
        v1.view_definition = Some(" SELECT * FROM v_base;".into());
        let mut v0 = table("v_base", vec![], &[], vec![]);
        v0.kind = TableKind::View;
        v0.view_definition = Some("SELECT id FROM customers".into());
        c.schemas[0].tables.extend([v1, v0]);
        let mut o = opts(DatabaseKind::Postgres, &c);
        o.views = vec![("public".into(), "v_top".into()), ("public".into(), "v_base".into())];
        o.tables.retain(|t| !t.1.starts_with("v_"));
        let all = sql_of(&plan(&c, &o));
        let base = all
            .iter()
            .position(|s| s.contains("VIEW \"public\".\"v_base\"") || s.contains("VIEW public.v_base"))
            .unwrap();
        let top = all.iter().position(|s| s.contains("v_top")).unwrap();
        assert!(base < top, "{all:?}");
        assert!(
            all[top].ends_with("SELECT * FROM v_base"),
            "the trailing ; and space are trimmed: {}",
            all[top]
        );
    }

    #[test]
    fn serial_defaults_bring_their_sequence() {
        let mut id = col("id", "integer", TypeKind::Integer);
        id.default = Some("nextval('things_id_seq'::regclass)".into());
        let c = catalog(vec![table("things", vec![id], &["id"], vec![])]);
        let all = sql_of(&plan(&c, &opts(DatabaseKind::Postgres, &c)));
        assert!(all.contains(&"CREATE SEQUENCE IF NOT EXISTS things_id_seq".to_string()), "{all:?}");
        assert!(all.iter().any(|s| s.starts_with("ALTER SEQUENCE things_id_seq OWNED BY")));
        assert!(all.iter().any(|s| s.starts_with("SELECT setval('things_id_seq'")));
    }

    #[test]
    fn header_lists_what_is_not_captured() {
        let c = shop();
        for k in [DatabaseKind::Postgres, DatabaseKind::MySql, DatabaseKind::Sqlite, DatabaseKind::DuckDb] {
            let p = plan(&c, &opts(k, &c));
            let text = sql_of(&p).join("\n");
            assert!(text.contains("Not captured"), "{k:?}");
            assert!(text.contains("GRANT"), "{k:?}");
        }
        assert!(not_captured(DatabaseKind::Postgres).iter().any(|l| l.contains("triggers")));
        assert!(not_captured(DatabaseKind::MySql).iter().any(|l| l.contains("stored routines")));
    }
}
