//! Search Across Tables against real engines: PostgreSQL and MariaDB (the local test servers, see AGENTS.md;
//! a test prints why and passes when its server does not answer), SQLite, and DuckDB with
//! `--features duckdb`.
//!
//! Every test builds its own scratch schema, database or file with a unique name, searches only that, and
//! drops it again. The seeded `quarry_demo` tables are never touched. The sleeping statements that exercise
//! the timeout and the cancel are bounded (a few seconds at most).

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use theta_core::*;
use theta_import::search::{CancelToken, SearchOutcome, TableResult, run_search};
use theta_sql::search::{ColumnClass, Scope, SearchOptions, TableJob, filter_statement, plan};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}_{}", std::process::id(), COUNTER.fetch_add(1, Ordering::SeqCst))
}

async fn q(conn: &dyn Connection, sql: &str) -> Vec<Vec<Value>> {
    conn.execute(sql, &ExecOptions { row_limit: None })
        .await
        .unwrap_or_else(|e| panic!("`{sql}` failed: {e}"))
        .result
        .map(|r| r.rows)
        .unwrap_or_default()
}

fn reachable(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(300),
    )
    .is_ok()
}

fn pg_config() -> ConnectionConfig {
    let mut c = ConnectionConfig::new("t", "pg", DatabaseKind::Postgres);
    (c.host, c.user, c.password, c.database) =
        ("127.0.0.1".into(), "postgres".into(), Some("postgres".into()), "quarry_demo".into());
    c
}

fn mysql_config() -> ConnectionConfig {
    let mut c = ConnectionConfig::new("t", "my", DatabaseKind::MySql);
    (c.host, c.user, c.password, c.database) =
        ("127.0.0.1".into(), "quarry".into(), Some("quarry".into()), "quarry_demo".into());
    c
}

async fn open(c: &ConnectionConfig) -> Arc<dyn Connection> {
    Arc::from(theta_drivers::connect(c).await.expect("connect"))
}

/// The rows every engine gets. Chosen so that a LIKE-based search would give wrong answers: `a_b` and
/// `axb`, `100%` and `1000`, a quote and a backslash.
fn rows() -> Vec<Vec<Value>> {
    let t = |s: &str| Value::Text(s.into());
    vec![
        vec![Value::Int(1), t("Ann"), t("first"), Value::Int(12)],
        vec![Value::Int(2), t("ann"), t("a_b"), Value::Int(120)],
        vec![Value::Int(3), t("Joanna"), t("axb"), Value::Int(5)],
        vec![Value::Int(4), t("Bob"), t("100%"), Value::Int(7)],
        vec![Value::Int(5), t("O'Brien"), t("1000"), Value::Int(99)],
        vec![Value::Int(6), t("Carol"), t("C:\\temp"), Value::Int(12)],
        vec![Value::Int(7), Value::Null, Value::Null, Value::Null],
    ]
}

/// Create `people`, `blobs` and `empty_t` in `schema` (`""` for the connection's own database).
async fn seed(conn: &dyn Connection, kind: DatabaseKind, schema: &str) {
    let name = |t: &str| {
        if schema.is_empty() {
            kind.quote_ident(t)
        } else {
            format!("{}.{}", kind.quote_ident(schema), kind.quote_ident(t))
        }
    };
    let (text, blob) = match kind {
        DatabaseKind::Postgres => ("text", "bytea"),
        DatabaseKind::MySql => ("varchar(100)", "blob"),
        DatabaseKind::DuckDb => ("VARCHAR", "BLOB"),
        _ => ("TEXT", "BLOB"),
    };
    q(
        conn,
        &format!(
            "CREATE TABLE {} (id INTEGER PRIMARY KEY, name {text}, note {text}, qty INTEGER)",
            name("people")
        ),
    )
    .await;
    q(conn, &format!("CREATE TABLE {} (data {blob})", name("blobs"))).await;
    q(conn, &format!("CREATE TABLE {} (name {text})", name("empty_t"))).await;
    for r in rows() {
        let values = r.iter().map(|v| theta_sql::sql_literal(v, kind)).collect::<Vec<_>>().join(", ");
        q(conn, &format!("INSERT INTO {} VALUES ({values})", name("people"))).await;
    }
}

struct Run {
    results: Vec<TableResult>,
    jobs: Vec<TableJob>,
}

fn find<'a>(run: &'a Run, table: &str) -> &'a TableResult {
    run.results.iter().find(|r| r.table == table).unwrap_or_else(|| panic!("no result for {table}"))
}

async fn search(pool: &[Arc<dyn Connection>], kind: DatabaseKind, schema: &str, opts: &SearchOptions) -> Run {
    let catalog = pool[0].introspect().await.expect("introspect");
    let p = plan(&catalog, kind, &Scope::Schema(schema.into()), opts).expect("plan");
    let results = Arc::new(Mutex::new(Vec::new()));
    let sink = results.clone();
    let report = run_search(
        pool.to_vec(),
        p.jobs.clone(),
        opts.row_limit,
        Duration::from_secs(20),
        CancelToken::new(),
        Arc::new(move |r| sink.lock().unwrap().push(r)),
    )
    .await;
    assert_eq!(report.tables, p.jobs.len());
    assert_eq!(report.not_run, 0);
    let results = results.lock().unwrap().clone();
    assert_eq!(results.len(), p.jobs.len(), "every table is reported exactly once");
    Run { results, jobs: p.jobs }
}

fn matched(r: &TableResult) -> (u64, Vec<(String, u64)>) {
    match &r.outcome {
        SearchOutcome::Matched(c) => (c.matched_rows, c.columns.clone()),
        other => panic!("expected a match, got {other:?}"),
    }
}

fn opts(text: &str) -> SearchOptions {
    SearchOptions::new(text)
}

/// The assertions every engine must pass.
async fn exercise(pool: &[Arc<dyn Connection>], kind: DatabaseKind, schema: &str) {
    let people = |run: &Run| find(run, "people").clone();

    // The binary-only table has nothing to search and is left out of the plan, not hidden.
    let catalog = pool[0].introspect().await.unwrap();
    let p = plan(&catalog, kind, &Scope::Schema(schema.into()), &opts("x")).unwrap();
    assert!(
        p.skipped.iter().any(|s| s.table == "blobs" && s.reason.contains("no column")),
        "{:?}",
        p.skipped
    );

    // Case-insensitive substring: Ann, ann, Joanna.
    let run = search(pool, kind, schema, &opts("ann")).await;
    assert_eq!(matched(&people(&run)), (3, vec![("name".into(), 3)]), "{kind:?} substring, any case");
    assert_eq!(find(&run, "empty_t").outcome, SearchOutcome::NoMatch { scanned: 0 });

    // The statement that opens the matching rows agrees with the count.
    let cols = vec!["name".to_string()];
    let sql = filter_statement(kind, schema, "people", &cols, &opts("ann"));
    assert_eq!(q(pool[0].as_ref(), &sql).await.len(), 3, "{kind:?}: {sql}");

    // Case-sensitive: ann, Joanna.
    let cs = SearchOptions { case_sensitive: true, ..opts("ann") };
    assert_eq!(matched(&people(&search(pool, kind, schema, &cs).await)).0, 2, "{kind:?} case-sensitive");

    // Whole value.
    let whole = SearchOptions { whole_value: true, ..opts("ann") };
    assert_eq!(matched(&people(&search(pool, kind, schema, &whole).await)).0, 2, "{kind:?} whole, any case");
    let whole_cs = SearchOptions { whole_value: true, case_sensitive: true, ..opts("Ann") };
    assert_eq!(matched(&people(&search(pool, kind, schema, &whole_cs).await)).0, 1, "{kind:?} whole, exact");

    // Characters that mean something to LIKE or to a string literal are plain text.
    for (needle, rows_expected, column) in
        [("a_b", 1, "note"), ("100%", 1, "note"), ("O'Brien", 1, "name"), ("C:\\temp", 1, "note")]
    {
        let run = search(pool, kind, schema, &opts(needle)).await;
        let (n, cols) = matched(&people(&run));
        assert_eq!((n, cols), (rows_expected, vec![(column.to_string(), 1)]), "{kind:?}: {needle:?}");
    }

    // Numbers are searched through their text form, and the column classes switch columns off.
    let (n, cols) = matched(&people(&search(pool, kind, schema, &opts("12")).await));
    assert_eq!((n, cols), (3, vec![("qty".to_string(), 3)]), "{kind:?}: 12 in qty (12, 120, 12)");
    let text_only = SearchOptions { classes: BTreeSet::from([ColumnClass::Text]), ..opts("12") };
    assert_eq!(
        people(&search(pool, kind, schema, &text_only).await).outcome,
        SearchOutcome::NoMatch { scanned: 7 },
        "{kind:?}: qty is not searched when only text is chosen"
    );

    // Nothing found says how many rows were looked at.
    assert_eq!(
        people(&search(pool, kind, schema, &opts("zzz")).await).outcome,
        SearchOutcome::NoMatch { scanned: 7 }
    );

    // The row limit bounds the scan and the result says the table was only partly read.
    let limited = SearchOptions { row_limit: 3, ..opts("o") };
    let r = people(&search(pool, kind, schema, &limited).await);
    assert!(r.partial, "{kind:?}");
    match r.outcome {
        SearchOutcome::Matched(c) => assert_eq!(c.scanned, 3),
        SearchOutcome::NoMatch { scanned } => assert_eq!(scanned, 3),
        other => panic!("{other:?}"),
    }

    // A statement the server rejects is reported as that table's failure, and the next table still runs.
    let bad = TableJob { sql: "SELECT nonsense FROM nowhere_at_all".into(), ..run.jobs[0].clone() };
    let good = run.jobs.iter().find(|j| j.table == "empty_t").unwrap().clone();
    let out = Arc::new(Mutex::new(Vec::new()));
    let sink = out.clone();
    let report = run_search(
        pool[..1].to_vec(),
        vec![bad, good],
        100,
        Duration::from_secs(20),
        CancelToken::new(),
        Arc::new(move |r| sink.lock().unwrap().push(r)),
    )
    .await;
    let out = out.lock().unwrap().clone();
    assert!(matches!(out[0].outcome, SearchOutcome::Failed(_)), "{:?}", out[0]);
    assert!(matches!(out[1].outcome, SearchOutcome::NoMatch { .. }), "{:?}", out[1]);
    assert_eq!((report.failed, report.tables), (1, 2));
}

/// A job that takes `secs` seconds on the server and answers with the two counts a table search expects.
fn sleeper(kind: DatabaseKind, secs: f64) -> TableJob {
    let sql = match kind {
        DatabaseKind::Postgres => {
            format!("SELECT 1 AS scanned, 0 AS matched FROM (SELECT pg_sleep({secs})) AS s")
        }
        _ => format!("SELECT 1 AS scanned, 0 AS matched FROM (SELECT SLEEP({secs})) AS s"),
    };
    TableJob {
        schema: "s".into(),
        table: unique("sleep"),
        columns: vec![],
        columns_cut: 0,
        sql,
        estimate: None,
    }
}

/// Timeout, cancel and bounded concurrency, on an engine that can sleep inside a statement.
async fn exercise_time(config: &ConnectionConfig, kind: DatabaseKind) {
    // Three sessions, six 0.4 s statements: at most three at a time means at least two rounds, and
    // running at all in parallel means well under six rounds.
    let pool = vec![open(config).await, open(config).await, open(config).await];
    let results = Arc::new(Mutex::new(Vec::new()));
    let sink = results.clone();
    let t0 = Instant::now();
    let report = run_search(
        pool.clone(),
        (0..6).map(|_| sleeper(kind, 0.4)).collect(),
        10,
        Duration::from_secs(20),
        CancelToken::new(),
        Arc::new(move |r| sink.lock().unwrap().push(r)),
    )
    .await;
    let took = t0.elapsed();
    assert_eq!(results.lock().unwrap().len(), 6);
    assert_eq!(report.not_run, 0);
    assert!(took >= Duration::from_millis(780), "{kind:?}: six statements on three sessions took {took:?}");
    assert!(took < Duration::from_millis(2300), "{kind:?}: they did not overlap ({took:?})");

    // Timeout: reported as timed out long before the statement would have ended, and the session is fine after.
    let results = Arc::new(Mutex::new(Vec::new()));
    let sink = results.clone();
    let t0 = Instant::now();
    let report = run_search(
        pool[..1].to_vec(),
        vec![sleeper(kind, 8.0), sleeper(kind, 0.0)],
        10,
        Duration::from_millis(400),
        CancelToken::new(),
        Arc::new(move |r| sink.lock().unwrap().push(r)),
    )
    .await;
    assert!(t0.elapsed() < Duration::from_secs(5), "{kind:?}: the timeout did not stop the statement");
    let got = results.lock().unwrap().clone();
    assert_eq!(got[0].outcome, SearchOutcome::TimedOut, "{kind:?}");
    assert!(
        matches!(got[1].outcome, SearchOutcome::NoMatch { .. }),
        "{kind:?}: the session works after a timeout: {:?}",
        got[1]
    );
    assert_eq!(report.timed_out, 1);

    // Cancel while a statement runs.
    let cancel = CancelToken::new();
    let results = Arc::new(Mutex::new(Vec::new()));
    let sink = results.clone();
    let c2 = cancel.clone();
    let canceller = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        c2.cancel();
    });
    let t0 = Instant::now();
    let report = run_search(
        pool[..1].to_vec(),
        vec![sleeper(kind, 8.0), sleeper(kind, 8.0)],
        10,
        Duration::from_secs(20),
        cancel,
        Arc::new(move |r| sink.lock().unwrap().push(r)),
    )
    .await;
    canceller.await.unwrap();
    assert!(t0.elapsed() < Duration::from_secs(5), "{kind:?}: cancel did not stop the statement");
    assert!(report.cancelled);
    assert_eq!(results.lock().unwrap()[0].outcome, SearchOutcome::Cancelled, "{kind:?}");
    assert_eq!(report.not_run, 1, "the second table never started");
}

#[tokio::test(flavor = "multi_thread")]
async fn postgres() {
    if !reachable(5432) {
        eprintln!("skipped: no PostgreSQL on 127.0.0.1:5432");
        return;
    }
    let config = pg_config();
    let conn = open(&config).await;
    let schema = unique("theta_srch");
    q(conn.as_ref(), &format!("CREATE SCHEMA {schema}")).await;
    seed(conn.as_ref(), DatabaseKind::Postgres, &schema).await;
    // Three sessions: the same code path the application uses for a connection with a pool.
    let pool = vec![conn.clone(), open(&config).await, open(&config).await];
    exercise(&pool, DatabaseKind::Postgres, &schema).await;
    q(conn.as_ref(), &format!("DROP SCHEMA {schema} CASCADE")).await;
    exercise_time(&config, DatabaseKind::Postgres).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mariadb() {
    if !reachable(3306) {
        eprintln!("skipped: no MariaDB on 127.0.0.1:3306");
        return;
    }
    let config = mysql_config();
    let conn = open(&config).await;
    let database = unique("theta_srch");
    q(conn.as_ref(), &format!("CREATE DATABASE {database}")).await;
    seed(conn.as_ref(), DatabaseKind::MySql, &database).await;
    let pool = vec![conn.clone(), open(&config).await];
    exercise(&pool, DatabaseKind::MySql, &database).await;
    q(conn.as_ref(), &format!("DROP DATABASE {database}")).await;
    exercise_time(&config, DatabaseKind::MySql).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sqlite() {
    let dir = std::env::temp_dir().join(unique("theta_srch_sqlite"));
    std::fs::create_dir_all(&dir).unwrap();
    let mut c = ConnectionConfig::new("t", "sqlite", DatabaseKind::Sqlite);
    c.path = dir.join("s.db").to_string_lossy().into_owned();
    let conn = open(&c).await;
    seed(conn.as_ref(), DatabaseKind::Sqlite, "").await;
    exercise(std::slice::from_ref(&conn), DatabaseKind::Sqlite, "main").await;
    // A second session of a file database sees the same tables, so a pool of two works.
    exercise(&[conn, open(&c).await], DatabaseKind::Sqlite, "main").await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "duckdb")]
#[tokio::test(flavor = "multi_thread")]
async fn duckdb() {
    let dir = std::env::temp_dir().join(unique("theta_srch_duck"));
    std::fs::create_dir_all(&dir).unwrap();
    let mut c = ConnectionConfig::new("t", "duck", DatabaseKind::DuckDb);
    c.path = dir.join("s.duckdb").to_string_lossy().into_owned();
    let conn = open(&c).await;
    seed(conn.as_ref(), DatabaseKind::DuckDb, "").await;
    // One session, as the application uses for an in-memory database.
    exercise(&[conn], DatabaseKind::DuckDb, "main").await;
    let _ = std::fs::remove_dir_all(&dir);
}
