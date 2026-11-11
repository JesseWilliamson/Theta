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
