//! The import and export pipelines against real servers: PostgreSQL, MariaDB/MySQL and SQLite. Every test
//! works in its own schema/database (or an in-memory SQLite) and cleans up after itself.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use theta_core::*;
use theta_import::plan::{NewColumn, create_table_sql};
use theta_import::*;

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}_{}", std::process::id(), COUNTER.fetch_add(1, Ordering::SeqCst))
}

fn opts() -> ExecOptions {
    ExecOptions { row_limit: None }
}

async fn q(conn: &dyn Connection, sql: &str) -> Vec<Vec<Value>> {
    conn.execute(sql, &opts())
        .await
        .unwrap_or_else(|e| panic!("`{sql}` failed: {e}"))
        .result
        .map(|r| r.rows)
        .unwrap_or_default()
}

async fn count(conn: &dyn Connection, table: &str) -> i64 {
    match q(conn, &format!("SELECT count(*) FROM {table}")).await[0][0] {
        Value::Int(i) => i,
        ref v => panic!("count was {v:?}"),
    }
}

/// A connection plus the schema-qualified table prefix tests use and a way to tear it down.
struct Fx {
    conn: Box<dyn Connection>,
    kind: DatabaseKind,
    schema: Option<String>,
    cleanup: Vec<String>,
}

impl Fx {
    async fn new(kind: DatabaseKind) -> Fx {
        let (conn, schema, cleanup) = match kind {
            DatabaseKind::Postgres => {
                let mut c = ConnectionConfig::new("t", "pg", kind);
                c.host = "127.0.0.1".into();
                c.user = "postgres".into();
                c.password = Some("postgres".into());
                c.database = "quarry_demo".into();
                let conn = theta_drivers::connect(&c).await.expect("postgres");
                let s = unique("imp");
                q(conn.as_ref(), &format!("CREATE SCHEMA {s}")).await;
                (conn, Some(s.clone()), vec![format!("DROP SCHEMA {s} CASCADE")])
            }
            DatabaseKind::MySql => {
                let mut c = ConnectionConfig::new("t", "my", kind);
                c.host = "127.0.0.1".into();
                c.user = "quarry".into();
                c.password = Some("quarry".into());
                c.database = "quarry_demo".into();
                let conn = theta_drivers::connect(&c).await.expect("mysql");
                let s = unique("imp");
                q(conn.as_ref(), &format!("CREATE DATABASE {s}")).await;
                q(conn.as_ref(), &format!("USE {s}")).await;
                (conn, Some(s.clone()), vec![format!("DROP DATABASE {s}")])
            }
            DatabaseKind::Sqlite => {
                let mut c = ConnectionConfig::new("t", "sqlite", kind);
                c.path = ":memory:".into();
                (theta_drivers::connect(&c).await.expect("sqlite"), None, vec![])
            }
        };
        Fx { conn, kind, schema, cleanup }
    }

    fn qual(&self, table: &str) -> String {
        match (&self.schema, self.kind) {
            (Some(s), DatabaseKind::Postgres | DatabaseKind::MySql) => format!("{s}.{table}"),
            _ => table.to_string(),
        }
    }

    async fn done(self) {
        let _ = self.conn.execute("ROLLBACK", &opts()).await;
        for c in &self.cleanup {
            let _ = self.conn.execute(c, &opts()).await;
        }
        let _ = self.conn.close().await;
    }

    fn begin(&self) -> Vec<String> {
        TransactionConfig::default().begin_statements(self.kind, false)
    }

    fn job(&self, table: &str, columns: &[(&str, usize, &str)]) -> ImportJob {
        ImportJob {
            kind: self.kind,
            schema: self.schema.clone(),
            table: table.into(),
            create_sql: None,
            truncate_sql: None,
            columns: columns
                .iter()
                .map(|(n, s, ty)| ImportColumn {
                    source: *s,
                    name: (*n).into(),
                    target: Target::from_type_name(ty, self.kind, true),
                })
                .collect(),
            csv: CsvOptions::default(),
            has_header: true,
            convert: ConvertOptions::default(),
            on_error: ErrorMode::Abort,
            batch_rows: 100,
            max_statement_bytes: DEFAULT_STATEMENT_BYTES,
            txn: TxnPlan::Own { begin: self.begin() },
        }
    }

    fn create(&self, table: &str, cols: &[(&str, &str)]) -> String {
        let cols: Vec<NewColumn> = cols
            .iter()
            .map(|(n, t)| NewColumn { name: (*n).into(), sql_type: (*t).into(), nullable: true })
            .collect();
        create_table_sql(self.kind, self.schema.as_deref(), table, &cols)
    }
}

const KINDS: [DatabaseKind; 3] = [DatabaseKind::Postgres, DatabaseKind::MySql, DatabaseKind::Sqlite];

fn text_type(k: DatabaseKind) -> &'static str {
    if k == DatabaseKind::MySql { "VARCHAR(100)" } else { "text" }
}

fn int_type(k: DatabaseKind) -> &'static str {
    if k == DatabaseKind::MySql { "INT" } else { "integer" }
}

fn csv_of(rows: usize) -> String {
    let mut s = String::from("id,name,note\n");
    for i in 0..rows {
        // Unicode, quotes, embedded newlines and NULLs (empty unquoted) in a regular pattern.
        let note = match i % 4 {
            0 => String::new(),
            1 => "\"multi\nline, with \"\"quotes\"\"\"".to_string(),
            2 => "plain".to_string(),
            _ => "日本語 é".to_string(),
        };
        s.push_str(&format!("{i},name {i},{note}\n"));
    }
    s
}

async fn run(fx: &Fx, data: &str, job: &ImportJob) -> ImportReport {
    run_import(fx.conn.as_ref(), data.as_bytes(), job, None, &AtomicBool::new(false), &|_| {}).await
}

#[tokio::test]
async fn imports_into_a_new_table_everywhere() {
    for kind in KINDS {
        let fx = Fx::new(kind).await;
        let mut job =
            fx.job("fresh", &[("id", 0, int_type(kind)), ("name", 1, text_type(kind)), ("note", 2, "text")]);
        job.create_sql =
            Some(fx.create("fresh", &[("id", int_type(kind)), ("name", text_type(kind)), ("note", "text")]));
        let data = csv_of(1000);
        let rep = run(&fx, &data, &job).await;
        assert_eq!(rep.outcome, Outcome::Completed, "{kind:?}");
        assert_eq!((rep.rows_read, rep.rows_inserted, rep.rows_skipped), (1000, 1000, 0));
        assert!(rep.created_table && rep.committed);
        let t = fx.qual("fresh");
        assert_eq!(count(fx.conn.as_ref(), &t).await, 1000);
        // NULL vs content, newline and unicode survive.
        assert_eq!(
            count(fx.conn.as_ref(), &format!("(SELECT 1 FROM {t} WHERE note IS NULL) x")).await,
            250,
            "{kind:?}"
        );
        let rows = q(fx.conn.as_ref(), &format!("SELECT note FROM {t} WHERE id = 1")).await;
        assert_eq!(rows[0][0], Value::Text("multi\nline, with \"quotes\"".into()), "{kind:?}");
        let rows = q(fx.conn.as_ref(), &format!("SELECT note FROM {t} WHERE id = 3")).await;
        assert_eq!(rows[0][0], Value::Text("日本語 é".into()));
        fx.done().await;
    }
}

#[tokio::test]
async fn abort_mode_rolls_everything_back_including_a_new_table() {
    for kind in KINDS {
        let fx = Fx::new(kind).await;
        let mut job = fx.job("t", &[("id", 0, int_type(kind)), ("name", 1, text_type(kind))]);
        job.create_sql = Some(fx.create("t", &[("id", int_type(kind)), ("name", text_type(kind))]));
        let mut data = String::from("id,name\n");
        for i in 0..350 {
            data.push_str(&format!("{i},n{i}\n"));
        }
        data.push_str("oops,bad\n99,tail\n");
        let rep = run(&fx, &data, &job).await;
        assert!(
            matches!(&rep.outcome, Outcome::Failed(m) if m.contains("line 352") && m.contains("not an integer")),
            "{kind:?} {:?}",
            rep.outcome
        );
        assert!(!rep.created_table);
        // The table must not exist any more: nothing of the failed import is left.
        let exists = fx.conn.execute(&format!("SELECT count(*) FROM {}", fx.qual("t")), &opts()).await;
        assert!(exists.is_err(), "{kind:?}: table survived a failed import");
        fx.done().await;
    }
}

#[tokio::test]
async fn skip_mode_imports_the_good_rows_and_lists_the_bad_ones() {
    for kind in KINDS {
        let fx = Fx::new(kind).await;
        q(
            fx.conn.as_ref(),
            &format!(
                "CREATE TABLE {} (id {} PRIMARY KEY, name {})",
                fx.qual("t"),
                int_type(kind),
                text_type(kind)
            ),
        )
        .await;
        let mut job = fx.job("t", &[("id", 0, int_type(kind)), ("name", 1, text_type(kind))]);
        job.on_error = ErrorMode::Skip;
        job.batch_rows = 4;
        // line 4: unconvertible, line 6: duplicate key (rejected by the server), line 8: ragged.
        let data = "id,name\n1,a\n2,b\nx,c\n4,d\n2,dup\n6,f\n7\n8,h\n";
        let rep = run(&fx, data, &job).await;
        assert_eq!(rep.outcome, Outcome::Completed, "{kind:?}");
        assert_eq!((rep.rows_read, rep.rows_inserted, rep.rows_skipped), (8, 5, 3), "{kind:?}");
        let lines: Vec<u64> = rep.issues.iter().map(|i| i.line).collect();
        assert_eq!(lines, vec![4, 6, 8], "{kind:?}");
        assert!(
            rep.issues.iter().any(|i| i.line == 6 && i.reason.contains("rejected by the server")),
            "{kind:?} {:?}",
            rep.issues
        );
        assert_eq!(count(fx.conn.as_ref(), &fx.qual("t")).await, 5, "{kind:?}");
        fx.done().await;
    }
}

#[tokio::test]
async fn cancel_midway_rolls_back_to_the_row_count_before() {
    for kind in KINDS {
        let fx = Fx::new(kind).await;
        q(
            fx.conn.as_ref(),
            &format!(
                "CREATE TABLE {} (id {}, name {}, note text)",
                fx.qual("t"),
                int_type(kind),
                text_type(kind)
            ),
        )
        .await;
        q(fx.conn.as_ref(), &format!("INSERT INTO {} VALUES (-1, 'keep', NULL)", fx.qual("t"))).await;
        let mut job =
            fx.job("t", &[("id", 0, int_type(kind)), ("name", 1, text_type(kind)), ("note", 2, "text")]);
        job.batch_rows = 50;
        let cancel = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(0u64));
        let (c2, s2) = (cancel.clone(), seen.clone());
        let data = csv_of(2000);
        let rep = run_import(fx.conn.as_ref(), data.as_bytes(), &job, None, &cancel, &move |p| {
            *s2.lock().unwrap() = p.rows_inserted;
            if p.rows_inserted >= 500 {
                c2.store(true, Ordering::Relaxed);
            }
        })
        .await;
        assert_eq!(rep.outcome, Outcome::Cancelled, "{kind:?}");
        assert!(*seen.lock().unwrap() >= 500, "some rows really were inserted before the cancel");
        assert_eq!(
            count(fx.conn.as_ref(), &fx.qual("t")).await,
            1,
            "{kind:?}: rollback must restore the old content"
        );
        fx.done().await;
    }
}

#[tokio::test]
async fn truncate_first_is_part_of_the_transaction() {
    for kind in KINDS {
        let fx = Fx::new(kind).await;
        q(
            fx.conn.as_ref(),
            &format!("CREATE TABLE {} (id {}, name {})", fx.qual("t"), int_type(kind), text_type(kind)),
        )
        .await;
        q(fx.conn.as_ref(), &format!("INSERT INTO {} VALUES (1, 'old'), (2, 'old')", fx.qual("t"))).await;
        let mut job = fx.job("t", &[("id", 0, int_type(kind)), ("name", 1, text_type(kind))]);
        job.truncate_sql = Some(match kind {
            DatabaseKind::Postgres => format!("TRUNCATE TABLE {}", fx.qual("t")),
            _ => format!("DELETE FROM {}", fx.qual("t")),
        });
        // A failing import leaves the old rows alone.
        let rep = run(&fx, "id,name\n1,a\nx,b\n", &job).await;
        assert!(matches!(rep.outcome, Outcome::Failed(_)));
        assert_eq!(count(fx.conn.as_ref(), &fx.qual("t")).await, 2, "{kind:?}");
        // A good one replaces them.
        let rep = run(&fx, "id,name\n5,new\n", &job).await;
        assert_eq!(rep.outcome, Outcome::Completed);
        assert_eq!(count(fx.conn.as_ref(), &fx.qual("t")).await, 1, "{kind:?}");
        fx.done().await;
    }
}

#[tokio::test]
async fn session_plan_leaves_the_transaction_open_and_rolls_back_to_a_savepoint() {
    for kind in KINDS {
        let fx = Fx::new(kind).await;
        q(
            fx.conn.as_ref(),
            &format!("CREATE TABLE {} (id {}, name {})", fx.qual("t"), int_type(kind), text_type(kind)),
        )
        .await;
        let mut job = fx.job("t", &[("id", 0, int_type(kind)), ("name", 1, text_type(kind))]);
        job.txn = TxnPlan::Session { open: false, begin: fx.begin() };
        let rep = run(&fx, "id,name\n1,a\n2,b\n", &job).await;
        assert_eq!(rep.outcome, Outcome::Completed);
        assert!(!rep.committed);
        assert!(fx.conn.in_transaction().await, "{kind:?}: manual mode keeps the transaction open");
        assert_eq!(count(fx.conn.as_ref(), &fx.qual("t")).await, 2);
        // A second import in the same open transaction that fails undoes only itself.
        job.txn = TxnPlan::Session { open: true, begin: fx.begin() };
        let rep = run(&fx, "id,name\n3,c\nbad,d\n", &job).await;
        assert!(matches!(rep.outcome, Outcome::Failed(_)));
        assert!(fx.conn.in_transaction().await);
        assert_eq!(count(fx.conn.as_ref(), &fx.qual("t")).await, 2, "{kind:?}");
        q(fx.conn.as_ref(), "ROLLBACK").await;
        assert_eq!(count(fx.conn.as_ref(), &fx.qual("t")).await, 0);
        fx.done().await;
    }
}

#[tokio::test]
async fn statements_stay_under_the_size_budget() {
    let fx = Fx::new(DatabaseKind::Sqlite).await;
    q(fx.conn.as_ref(), "CREATE TABLE t (id integer, name text)").await;
    let mut job = fx.job("t", &[("id", 0, "integer"), ("name", 1, "text")]);
    job.batch_rows = 100_000;
    job.max_statement_bytes = 2048;
    let mut data = String::from("id,name\n");
    for i in 0..500 {
        data.push_str(&format!("{i},{}\n", "x".repeat(50)));
    }
    let rep = run(&fx, &data, &job).await;
    assert_eq!(rep.outcome, Outcome::Completed);
    assert_eq!(count(fx.conn.as_ref(), "t").await, 500);
    // 500 rows of ~60 bytes cannot fit in fewer than ~14 statements of 2 KB.
    assert!(rep.statements > 14, "{}", rep.statements);
    fx.done().await;
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(unique("theta-import-test"));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

async fn make_source(fx: &Fx, name: &str, rows: usize, composite: bool) {
    let k = fx.kind;
    let (txt, int, num, ts, flag) = match k {
        DatabaseKind::MySql => ("VARCHAR(200)", "INT", "DECIMAL(12,2)", "DATETIME", "TINYINT(1)"),
        DatabaseKind::Postgres => ("text", "integer", "numeric(12,2)", "timestamp", "boolean"),
        DatabaseKind::Sqlite => ("text", "integer", "numeric", "timestamp", "integer"),
    };
    let pk = if composite { "PRIMARY KEY (a, id)" } else { "PRIMARY KEY (id)" };
    q(
        fx.conn.as_ref(),
        &format!(
            "CREATE TABLE {} (a {int} NOT NULL, id {int} NOT NULL, name {txt}, note {txt}, amount {num}, ts {ts}, flag {flag}, {pk})",
            fx.qual(name)
        ),
    )
    .await;
    let mut tuples = Vec::new();
    for i in 0..rows {
        let note = match i % 5 {
            0 => "NULL".to_string(),
            1 => "''".to_string(),
            2 => "'multi\nline \"quoted\", comma'".to_string(),
            3 => "'日本語 é'".to_string(),
            _ => "'plain'".to_string(),
        };
        let amount = if i % 7 == 0 { "NULL".to_string() } else { format!("{}.{:02}", i, i % 100) };
        let ts =
            if i % 3 == 0 { "NULL".to_string() } else { format!("'2024-01-{:02} 10:20:30'", 1 + i % 28) };
        let flag = if i % 2 == 0 { "1" } else { "0" };
        let flag = if k == DatabaseKind::Postgres { if i % 2 == 0 { "TRUE" } else { "FALSE" } } else { flag };
        tuples.push(format!("({}, {}, 'name {i}', {note}, {amount}, {ts}, {flag})", i % 3, i));
    }
    for chunk in tuples.chunks(200) {
        q(fx.conn.as_ref(), &format!("INSERT INTO {} VALUES {}", fx.qual(name), chunk.join(", "))).await;
    }
}

#[tokio::test]
async fn export_then_import_round_trips_exactly() {
    for kind in KINDS {
        for composite in [false, true] {
            let fx = Fx::new(kind).await;
            make_source(&fx, "src", 777, composite).await;
            let path = tmp("src.csv");
            let ejob = ExportJob {
                kind,
                schema: fx.schema.clone(),
                table: "src".into(),
                key: if composite { vec!["a".into(), "id".into()] } else { vec!["id".into()] },
                format: theta_sql::ExportFormat::Csv,
                // Not a divisor of 777, so the last page is short; small, so keyset paging is exercised.
                page_rows: 50,
            };
            let rep = export_table_to_path(
                fx.conn.as_ref(),
                &path,
                &ejob,
                Some(777),
                &AtomicBool::new(false),
                &|_| {},
            )
            .await;
            assert_eq!(rep.outcome, ExportOutcome::Completed, "{kind:?}");
            assert_eq!((rep.rows, rep.pages, rep.paging), (777, 16, Paging::Keyset));
            assert!(!path.with_extension("csv.part").exists());

            // Re-import into a table of the same shape, mapping by name.
            let (txt, int, num, ts, flag) = match kind {
                DatabaseKind::MySql => ("VARCHAR(200)", "INT", "DECIMAL(12,2)", "DATETIME", "TINYINT(1)"),
                DatabaseKind::Postgres => ("text", "integer", "numeric(12,2)", "timestamp", "boolean"),
                DatabaseKind::Sqlite => ("text", "integer", "numeric", "timestamp", "integer"),
            };
            q(fx.conn.as_ref(), &format!("CREATE TABLE {} (a {int}, id {int}, name {txt}, note {txt}, amount {num}, ts {ts}, flag {flag})", fx.qual("dst"))).await;
            let job = fx.job(
                "dst",
                &[
                    ("a", 0, int),
                    ("id", 1, int),
                    ("name", 2, txt),
                    ("note", 3, txt),
                    ("amount", 4, num),
                    ("ts", 5, ts),
                    ("flag", 6, flag),
                ],
            );
            let file = std::fs::File::open(&path).unwrap();
            let rep = run_import(fx.conn.as_ref(), file, &job, None, &AtomicBool::new(false), &|_| {}).await;
            assert_eq!(rep.outcome, Outcome::Completed, "{kind:?} {:?}", rep.issues);
            assert_eq!(rep.rows_inserted, 777);
            let (s, d) = (fx.qual("src"), fx.qual("dst"));
            for (x, y) in [(&s, &d), (&d, &s)] {
                let diff = count(fx.conn.as_ref(), &format!("(SELECT a,id,name,note,amount,ts,flag FROM {x} EXCEPT SELECT a,id,name,note,amount,ts,flag FROM {y}) z")).await;
                assert_eq!(diff, 0, "{kind:?} composite={composite}: {x} vs {y}");
            }
            assert_eq!(count(fx.conn.as_ref(), &d).await, 777);
            // NULL and '' stayed distinct.
            assert_eq!(
                count(fx.conn.as_ref(), &format!("(SELECT 1 FROM {d} WHERE note = '') z")).await,
                156,
                "{kind:?}"
            );
            std::fs::remove_dir_all(path.parent().unwrap()).ok();
            fx.done().await;
        }
    }
}

#[tokio::test]
async fn export_formats_and_offset_paging_without_a_key() {
    let fx = Fx::new(DatabaseKind::Sqlite).await;
    q(fx.conn.as_ref(), "CREATE TABLE nokey (x integer, y text)").await;
    for i in 0..25 {
        q(fx.conn.as_ref(), &format!("INSERT INTO nokey VALUES ({i}, 'v{i}')")).await;
    }
    for (fmt, name) in [
        (theta_sql::ExportFormat::Json, "o.json"),
        (theta_sql::ExportFormat::Tsv, "o.tsv"),
        (theta_sql::ExportFormat::SqlInserts { table: String::new() }, "o.sql"),
    ] {
        let path = tmp(name);
        let job = ExportJob {
            kind: DatabaseKind::Sqlite,
            schema: None,
            table: "nokey".into(),
            key: vec![],
            format: fmt.clone(),
            page_rows: 10,
        };
        let rep =
            export_table_to_path(fx.conn.as_ref(), &path, &job, None, &AtomicBool::new(false), &|_| {}).await;
        assert_eq!((rep.rows, rep.paging, &rep.outcome), (25, Paging::Offset, &ExportOutcome::Completed));
        let text = std::fs::read_to_string(&path).unwrap();
        match fmt {
            theta_sql::ExportFormat::Json => {
                let v: serde_json::Value = serde_json::from_str(&text).unwrap();
                assert_eq!(v.as_array().unwrap().len(), 25);
            }
            theta_sql::ExportFormat::Tsv => assert_eq!(text.lines().count(), 26),
            _ => {
                assert_eq!(text.lines().count(), 25);
                assert!(text.starts_with("INSERT INTO nokey (x, y) VALUES (0, 'v0');"));
            }
        }
    }
    fx.done().await;
}

#[tokio::test]
async fn export_cancel_leaves_no_file() {
    let fx = Fx::new(DatabaseKind::Sqlite).await;
    make_source(&fx, "src", 300, false).await;
    let path = tmp("c.csv");
    let job = ExportJob {
        kind: DatabaseKind::Sqlite,
        schema: None,
        table: "src".into(),
        key: vec!["id".into()],
        format: theta_sql::ExportFormat::Csv,
        page_rows: 20,
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    let rep = export_table_to_path(fx.conn.as_ref(), &path, &job, None, &cancel, &move |p| {
        if p.rows >= 60 {
            c2.store(true, Ordering::Relaxed);
        }
    })
    .await;
    assert_eq!(rep.outcome, ExportOutcome::Cancelled);
    assert!(!path.exists());
    assert!(!path.with_file_name("c.csv.part").exists());
    fx.done().await;
}

/// `cargo test -p theta-import --release -- --ignored --nocapture throughput` prints rows per second.
#[tokio::test]
#[ignore = "generates a 1M-row file; run explicitly"]
async fn throughput() {
    let path = tmp("million.csv");
    {
        use std::io::Write;
        let mut w = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        writeln!(w, "id,name,amount,created,note").unwrap();
        for i in 0..1_000_000u32 {
            writeln!(
                w,
                "{i},name {i},{}.{:02},2024-01-{:02} 10:20:30,\"note, with \"\"quotes\"\" {i}\"",
                i % 10000,
                i % 100,
                1 + i % 28
            )
            .unwrap();
        }
    }
    let size = std::fs::metadata(&path).unwrap().len();
    println!("file: {:.1} MB", size as f64 / 1e6);
    // Parse only (validate): no database.
    let job = {
        let fx_kind = DatabaseKind::Postgres;
        ImportJob {
            kind: fx_kind,
            schema: None,
            table: "t".into(),
            create_sql: None,
            truncate_sql: None,
            columns: ["integer", "text", "numeric", "timestamp", "text"]
                .iter()
                .enumerate()
                .map(|(i, t)| ImportColumn {
                    source: i,
                    name: format!("c{i}"),
                    target: Target::from_type_name(t, fx_kind, true),
                })
                .collect(),
            csv: CsvOptions::default(),
            has_header: true,
            convert: ConvertOptions::default(),
            on_error: ErrorMode::Abort,
            batch_rows: 1000,
            max_statement_bytes: DEFAULT_STATEMENT_BYTES,
            txn: TxnPlan::Own { begin: vec!["BEGIN".into()] },
        }
    };
    let t = std::time::Instant::now();
    let rep =
        validate(std::fs::File::open(&path).unwrap(), &job, Some(size), &AtomicBool::new(false), &|_| {});
    let s = t.elapsed().as_secs_f64();
    println!(
        "parse+convert only: {} rows in {s:.2}s = {:.0} rows/s",
        rep.rows_read,
        rep.rows_read as f64 / s
    );
    // Raw parse, no conversion.
    let t = std::time::Instant::now();
    let mut r = CsvReader::new(std::fs::File::open(&path).unwrap(), CsvOptions::default());
    let mut rec = Record::default();
    let mut n = 0u64;
    while r.read_record(&mut rec).unwrap() {
        n += 1;
    }
    let s = t.elapsed().as_secs_f64();
    println!(
        "raw parse: {n} records in {s:.2}s = {:.0} rows/s ({:.0} MB/s)",
        n as f64 / s,
        size as f64 / 1e6 / s
    );
    // End to end into each database.
    for kind in KINDS {
        let fx = Fx::new(kind).await;
        let (txt, int, num, ts) = match kind {
            DatabaseKind::MySql => ("VARCHAR(200)", "INT", "DECIMAL(12,2)", "DATETIME"),
            DatabaseKind::Postgres => ("text", "integer", "numeric(12,2)", "timestamp"),
            DatabaseKind::Sqlite => ("text", "integer", "numeric", "timestamp"),
        };
        let mut j = fx.job(
            "big",
            &[("id", 0, int), ("name", 1, txt), ("amount", 2, num), ("created", 3, ts), ("note", 4, txt)],
        );
        j.create_sql = Some(
            fx.create("big", &[("id", int), ("name", txt), ("amount", num), ("created", ts), ("note", txt)]),
        );
        let t = std::time::Instant::now();
        let rep = run_import(
            fx.conn.as_ref(),
            std::fs::File::open(&path).unwrap(),
            &j,
            Some(size),
            &AtomicBool::new(false),
            &|_| {},
        )
        .await;
        let s = t.elapsed().as_secs_f64();
        println!(
            "{kind:?}: {:?} {} rows in {s:.2}s = {:.0} rows/s ({} statements)",
            rep.outcome,
            rep.rows_inserted,
            rep.rows_inserted as f64 / s,
            rep.statements
        );
        fx.done().await;
    }
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

#[tokio::test]
async fn inferred_types_create_a_table_every_database_accepts() {
    use theta_import::scan::{SAMPLE_ROWS, scan};
    use theta_import::setup::{Destination, ImportSetup, NewColumnSpec, build_job};
    let data = "id,Full Name,born,score,active,seen,big\n\
                1,Ann,2000-01-02,1.50,true,2024-01-02 03:04:05,3000000000\n\
                2,\"Bo\nb\",2001-02-03,,false,2024-01-03T03:04:05,4\n\
                3,,2002-03-04,20.25,t,,5\n";
    for kind in KINDS {
        let fx = Fx::new(kind).await;
        let csv = CsvOptions::default();
        let conv = ConvertOptions::default();
        let sc = scan(data.as_bytes(), &csv, true, &conv, SAMPLE_ROWS).unwrap();
        let names = sc.suggested_column_names();
        let columns: Vec<NewColumnSpec> = (0..sc.width)
            .map(|i| NewColumnSpec {
                name: names[i].clone(),
                sql_type: sc.types[i].sql_type(kind),
                nullable: true,
                skip: false,
            })
            .collect();
        let setup = ImportSetup {
            kind,
            csv,
            has_header: true,
            convert: conv,
            on_error: ErrorMode::Abort,
            batch_rows: 100,
            max_statement_bytes: DEFAULT_STATEMENT_BYTES,
            truncate_first: false,
            txn: TxnPlan::Own { begin: fx.begin() },
            dest: Destination::New { schema: fx.schema.clone(), table: "inferred".into(), columns },
        };
        let job = build_job(&setup).unwrap();
        let rep = run(&fx, data, &job).await;
        assert_eq!(rep.outcome, Outcome::Completed, "{kind:?}: {:?} {:?}", rep.outcome, job.create_sql);
        let t = fx.qual("inferred");
        assert_eq!(count(fx.conn.as_ref(), &t).await, 3);
        let rows = q(fx.conn.as_ref(), &format!("SELECT full_name, active, big FROM {t} ORDER BY id")).await;
        assert_eq!(rows[1][0], Value::Text("Bo\nb".into()), "{kind:?}");
        assert_eq!(rows[2][0], Value::Null, "{kind:?}");
        assert_eq!(rows[0][2], Value::Int(3_000_000_000), "{kind:?}");
        // Booleans arrive as booleans (or 0/1 where the database has no boolean type).
        let truthy = |v: &Value| matches!(v, Value::Bool(true) | Value::Int(1));
        assert!(truthy(&rows[0][1]) && truthy(&rows[2][1]) && !truthy(&rows[1][1]), "{kind:?}: {:?}", rows);
        fx.done().await;
    }
}
