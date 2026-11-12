//! Copy Table against real engines: PostgreSQL and MariaDB (the local test servers, see AGENTS.md; a test
//! prints why and passes when its server does not answer), SQLite, and DuckDB with `--features duckdb`.
//!
//! Each test builds scratch schemas, databases or files with unique names, copies tables between products
//! and checks the target's contents, then removes everything it made. The seeded `quarry_demo` tables are
//! never read or written.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use theta_core::*;
use theta_import::copy::{Cleanup, CopyJob, CopyOutcome, CopyReport, copy_table};
use theta_sql::copy::{CopyPlan, plan_copy};

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
        std::time::Duration::from_millis(300),
    )
    .is_ok()
}

async fn open(c: &ConnectionConfig) -> Arc<dyn Connection> {
    Arc::from(theta_drivers::connect(c).await.expect("connect"))
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

fn file_config(kind: DatabaseKind, name: &str) -> (ConnectionConfig, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(unique("theta_copy"));
    std::fs::create_dir_all(&dir).unwrap();
    let mut c = ConnectionConfig::new("t", name, kind);
    c.path = dir.join(format!("{name}.db")).to_string_lossy().into_owned();
    (c, dir)
}

async fn table_of(conn: &dyn Connection, schema: &str, name: &str) -> Table {
    let cat = conn.introspect().await.expect("introspect");
    cat.find_table(Some(schema), name)
        .unwrap_or_else(|| panic!("{schema}.{name} not in the catalog"))
        .1
        .clone()
}

async fn exists(conn: &dyn Connection, schema: &str, name: &str) -> bool {
    conn.introspect().await.unwrap().find_table(Some(schema), name).is_some()
}

struct Copy {
    plan: CopyPlan,
    report: CopyReport,
}

#[allow(clippy::too_many_arguments)]
async fn copy(
    src: &dyn Connection,
    src_schema: &str,
    src_table: &str,
    dst: &dyn Connection,
    dst_schema: &str,
    dst_table: &str,
    with_data: bool,
    page: usize,
    cancel: &AtomicBool,
    cancel_after_rows: Option<u64>,
) -> Copy {
    let table = table_of(src, src_schema, src_table).await;
    let plan = plan_copy(src.kind(), &table, dst.kind(), dst_schema, dst_table).expect("plan");
    let job = CopyJob {
        src_kind: src.kind(),
        src_schema: Some(src_schema.to_string()),
        src_table: src_table.to_string(),
        key: table.primary_key.clone(),
        columns: plan.columns.clone(),
        dst_kind: dst.kind(),
        dst_schema: dst_schema.to_string(),
        dst_table: dst_table.to_string(),
        create_sql: plan.create.clone(),
        with_data,
        page_rows: page,
        rows_estimate: None,
    };
    let progress = move |p: &theta_import::CopyProgress| {
        if cancel_after_rows.is_some_and(|n| p.rows >= n) {
            cancel.store(true, Ordering::SeqCst);
        }
    };
    let report = copy_table(src, dst, &job, cancel, &progress).await;
    Copy { plan, report }
}

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

async fn seed_postgres(conn: &dyn Connection, schema: &str) {
    q(conn, &format!("CREATE SCHEMA {schema}")).await;
    q(conn, &format!(
        "CREATE TABLE {schema}.mixed (id integer PRIMARY KEY, name varchar(30) NOT NULL, price numeric(10,2), \
         active boolean, ref uuid, at timestamptz, doc jsonb, raw bytea, tags integer[], note text)"
    ))
    .await;
    q(conn, &format!(
        "INSERT INTO {schema}.mixed VALUES \
         (1, 'Ann', 12.50, true, '11111111-1111-1111-1111-111111111111', '2024-03-05 12:00:00+02', '{{\"a\": 1}}', '\\x00ff10', ARRAY[1,2,3], 'O''Brien \\ x'), \
         (2, 'Bob', NULL, false, NULL, NULL, NULL, NULL, NULL, NULL)"
    ))
    .await;
    q(conn, &format!("CREATE TABLE {schema}.bulk (id integer PRIMARY KEY, v text)")).await;
    q(conn, &format!("INSERT INTO {schema}.bulk SELECT g, 'row ' || g FROM generate_series(1, 2500) g"))
        .await;
}

fn text_of(v: &Value) -> String {
    v.display()
}

/// Row 1 and row 2 of the copied `mixed` table, read back as `(id, name, price, active, ref, at, doc, raw, tags, note)`.
async fn read_mixed(conn: &dyn Connection, kind: DatabaseKind, schema: &str, table: &str) -> Vec<Vec<Value>> {
    let name = if schema.is_empty() {
        kind.quote_ident(table)
    } else {
        format!("{}.{}", kind.quote_ident(schema), kind.quote_ident(table))
    };
    let cols = ["id", "name", "price", "active", "ref", "at", "doc", "raw", "tags", "note"]
        .map(|c| kind.quote_ident(c))
        .join(", ");
    q(conn, &format!("SELECT {cols} FROM {name} ORDER BY id")).await
}

fn check_row1_text(rows: &[Vec<Value>], what: &str) {
    let r = &rows[0];
    assert_eq!(r[1], Value::Text("Ann".into()), "{what}");
    assert!((text_of(&r[2]).parse::<f64>().unwrap() - 12.5).abs() < 1e-9, "{what}: price {:?}", r[2]);
    assert_eq!(text_of(&r[4]), "11111111-1111-1111-1111-111111111111", "{what}");
    assert!(text_of(&r[5]).starts_with("2024-03-05 10:00:00"), "{what}: instant kept, as UTC: {:?}", r[5]);
    assert!(text_of(&r[6]).contains("\"a\""), "{what}: {:?}", r[6]);
    assert_eq!(r[7], Value::Bytes(vec![0, 255, 16]), "{what}");
    let tags = text_of(&r[8]);
    assert!(tags.contains('1') && tags.contains('2') && tags.contains('3'), "{what}: {tags}");
    assert_eq!(r[9], Value::Text("O'Brien \\ x".into()), "{what}: quotes and backslashes survive");
    let b = &rows[1];
    assert_eq!(b[1], Value::Text("Bob".into()));
    for i in [2, 4, 5, 6, 7, 8, 9] {
        assert!(b[i].is_null(), "{what}: NULL stays NULL in column {i}: {:?}", b[i]);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn postgres_to_sqlite_and_mariadb_and_back() {
    if !reachable(5432) {
        eprintln!("skipped: no PostgreSQL on 127.0.0.1:5432");
        return;
    }
    let pg = open(&pg_config()).await;
    let schema = unique("theta_cp");
    seed_postgres(pg.as_ref(), &schema).await;

    // PostgreSQL -> SQLite
    let (lc, dir) = file_config(DatabaseKind::Sqlite, "target");
    let lite = open(&lc).await;
    let c = copy(
        pg.as_ref(),
        &schema,
        "mixed",
        lite.as_ref(),
        "main",
        "mixed_copy",
        true,
        1000,
        &no_cancel(),
        None,
    )
    .await;
    assert_eq!(c.report.outcome, CopyOutcome::Completed, "{:?}", c.report);
    assert_eq!(c.report.rows, 2);
    let cols: Vec<&str> = c.plan.substitutions.iter().map(|s| s.column.as_str()).collect();
    assert_eq!(
        cols,
        ["price", "active", "ref", "at", "doc", "tags"],
        "what SQLite cannot hold exactly is listed"
    );
    let rows = read_mixed(lite.as_ref(), DatabaseKind::Sqlite, "", "mixed_copy").await;
    check_row1_text(&rows, "sqlite");
    assert_eq!(rows[0][3], Value::Int(1), "true is 1");
    assert_eq!(rows[1][3], Value::Int(0));
    let t = table_of(lite.as_ref(), "main", "mixed_copy").await;
    assert_eq!(t.primary_key, ["id"], "the primary key is recreated");
    drop(lite);
    let _ = std::fs::remove_dir_all(dir);

    // PostgreSQL -> MariaDB (scratch database), then MariaDB -> PostgreSQL (a new schema)
    if reachable(3306) {
        let my_cfg = mysql_config();
        let my = open(&my_cfg).await;
        let db = unique("theta_cp");
        q(my.as_ref(), &format!("CREATE DATABASE {db}")).await;
        let c = copy(
            pg.as_ref(),
            &schema,
            "mixed",
            my.as_ref(),
            &db,
            "mixed_copy",
            true,
            1000,
            &no_cancel(),
            None,
        )
        .await;
        assert_eq!(c.report.outcome, CopyOutcome::Completed, "{:?}", c.report);
        let rows = read_mixed(my.as_ref(), DatabaseKind::MySql, &db, "mixed_copy").await;
        check_row1_text(&rows, "mariadb");
        assert_eq!(text_of(&rows[0][3]), "1", "a MySQL boolean reads back as 1");

        // And back: MariaDB's own types (tinyint(1), datetime, json, longblob) into PostgreSQL.
        let back_schema = unique("theta_cp");
        q(pg.as_ref(), &format!("CREATE SCHEMA {back_schema}")).await;
        let c = copy(
            my.as_ref(),
            &db,
            "mixed_copy",
            pg.as_ref(),
            &back_schema,
            "again",
            true,
            1000,
            &no_cancel(),
            None,
        )
        .await;
        assert_eq!(c.report.outcome, CopyOutcome::Completed, "{:?}", c.report);
        let n = q(
            pg.as_ref(),
            &format!("SELECT count(*) FROM {back_schema}.again WHERE active IS TRUE AND name = 'Ann'"),
        )
        .await;
        assert_eq!(n[0][0], Value::Int(1), "tinyint(1) became a boolean");
        let doc = q(pg.as_ref(), &format!("SELECT doc->>'a' FROM {back_schema}.again WHERE id = 1")).await;
        assert_eq!(doc[0][0], Value::Text("1".into()), "the JSON survived the round trip");
        q(pg.as_ref(), &format!("DROP SCHEMA {back_schema} CASCADE")).await;
        q(my.as_ref(), &format!("DROP DATABASE {db}")).await;
    } else {
        eprintln!("skipped the MariaDB half: no MariaDB on 127.0.0.1:3306");
    }

    // The same connection, another schema: types are copied verbatim, structure only works too.
    let other = unique("theta_cp");
    q(pg.as_ref(), &format!("CREATE SCHEMA {other}")).await;
    let c = copy(pg.as_ref(), &schema, "mixed", pg.as_ref(), &other, "mixed", true, 1000, &no_cancel(), None)
        .await;
    assert_eq!(c.report.outcome, CopyOutcome::Completed, "{:?}", c.report);
    assert!(c.plan.verbatim && c.plan.substitutions.is_empty());
    let a = read_mixed(pg.as_ref(), DatabaseKind::Postgres, &schema, "mixed").await;
    let b = read_mixed(pg.as_ref(), DatabaseKind::Postgres, &other, "mixed").await;
    assert_eq!(format!("{a:?}"), format!("{b:?}"), "PostgreSQL to PostgreSQL loses nothing");
    let c = copy(
        pg.as_ref(),
        &schema,
        "bulk",
        pg.as_ref(),
        &other,
        "bulk_structure",
        false,
        1000,
        &no_cancel(),
        None,
    )
    .await;
    assert_eq!(c.report.outcome, CopyOutcome::Completed);
    assert_eq!(
        q(pg.as_ref(), &format!("SELECT count(*) FROM {other}.bulk_structure")).await[0][0],
        Value::Int(0)
    );
    // Copying onto a name that exists fails, and leaves the existing table untouched.
    let c = copy(
        pg.as_ref(),
        &schema,
        "bulk",
        pg.as_ref(),
        &other,
        "bulk_structure",
        true,
        1000,
        &no_cancel(),
        None,
    )
    .await;
    assert!(matches!(c.report.outcome, CopyOutcome::Failed(_)), "{:?}", c.report);
    assert_eq!(c.report.cleanup, Cleanup::RolledBack);
    assert!(
        exists(pg.as_ref(), &other, "bulk_structure").await,
        "the table that was already there is still there"
    );
    q(pg.as_ref(), &format!("DROP SCHEMA {other} CASCADE")).await;
    q(pg.as_ref(), &format!("DROP SCHEMA {schema} CASCADE")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_copy_leaves_no_table_behind() {
    if !reachable(5432) {
        eprintln!("skipped: no PostgreSQL on 127.0.0.1:5432");
        return;
    }
    let pg = open(&pg_config()).await;
    let schema = unique("theta_cp");
    seed_postgres(pg.as_ref(), &schema).await;

    // Target PostgreSQL: the transaction is rolled back.
    let other = unique("theta_cp");
    q(pg.as_ref(), &format!("CREATE SCHEMA {other}")).await;
    let cancel = no_cancel();
    let dst = open(&pg_config()).await;
    let c =
        copy(pg.as_ref(), &schema, "bulk", dst.as_ref(), &other, "bulk", true, 500, &cancel, Some(500)).await;
    assert_eq!(c.report.outcome, CopyOutcome::Cancelled, "{:?}", c.report);
    assert_eq!(c.report.cleanup, Cleanup::RolledBack);
    assert!(c.report.rows >= 500 && c.report.rows < 2500, "stopped part-way: {}", c.report.rows);
    assert!(!exists(pg.as_ref(), &other, "bulk").await, "nothing is left");
    q(pg.as_ref(), &format!("DROP SCHEMA {other} CASCADE")).await;

    // Target SQLite: also a rollback.
    let (lc, dir) = file_config(DatabaseKind::Sqlite, "cancel");
    let lite = open(&lc).await;
    let cancel = no_cancel();
    let c = copy(pg.as_ref(), &schema, "bulk", lite.as_ref(), "main", "bulk", true, 500, &cancel, Some(500))
        .await;
    assert_eq!(c.report.outcome, CopyOutcome::Cancelled);
    assert_eq!(c.report.cleanup, Cleanup::RolledBack);
    assert!(!exists(lite.as_ref(), "main", "bulk").await);
    drop(lite);
    let _ = std::fs::remove_dir_all(dir);

    // Target MariaDB: no transactional DDL, so the half-made table is dropped.
    if reachable(3306) {
        let my = open(&mysql_config()).await;
        let db = unique("theta_cp");
        q(my.as_ref(), &format!("CREATE DATABASE {db}")).await;
        let cancel = no_cancel();
        let c =
            copy(pg.as_ref(), &schema, "bulk", my.as_ref(), &db, "bulk", true, 500, &cancel, Some(500)).await;
        assert_eq!(c.report.outcome, CopyOutcome::Cancelled);
        assert_eq!(c.report.cleanup, Cleanup::Dropped);
        assert!(!exists(my.as_ref(), &db, "bulk").await, "the half-made table was dropped");
        // A completed copy of the same table is all there.
        let c =
            copy(pg.as_ref(), &schema, "bulk", my.as_ref(), &db, "bulk", true, 500, &no_cancel(), None).await;
        assert_eq!(c.report.outcome, CopyOutcome::Completed, "{:?}", c.report);
        assert_eq!(q(my.as_ref(), &format!("SELECT count(*) FROM {db}.bulk")).await[0][0], Value::Int(2500));
        q(my.as_ref(), &format!("DROP DATABASE {db}")).await;
    }
    q(pg.as_ref(), &format!("DROP SCHEMA {schema} CASCADE")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_row_the_target_rejects_fails_the_copy_and_leaves_the_target_clean() {
    // SQLite lets an INTEGER column hold text; PostgreSQL will not. The failure comes on the 3rd row.
    let (lc, dir) = file_config(DatabaseKind::Sqlite, "source");
    let lite = open(&lc).await;
    q(lite.as_ref(), "CREATE TABLE odd (id INTEGER PRIMARY KEY, n INTEGER)").await;
    q(lite.as_ref(), "INSERT INTO odd VALUES (1, 10), (2, 20), (3, 'not a number')").await;

    let mut ok_targets = 0;
    if reachable(5432) {
        let pg = open(&pg_config()).await;
        let schema = unique("theta_cp");
        q(pg.as_ref(), &format!("CREATE SCHEMA {schema}")).await;
        let c =
            copy(lite.as_ref(), "main", "odd", pg.as_ref(), &schema, "odd", true, 1000, &no_cancel(), None)
                .await;
        let CopyOutcome::Failed(why) = &c.report.outcome else { panic!("{:?}", c.report) };
        assert!(why.contains("writing rows 1–3 failed"), "{why}");
        assert_eq!(c.report.cleanup, Cleanup::RolledBack);
        assert!(!exists(pg.as_ref(), &schema, "odd").await, "the table made for the copy is gone");
        q(pg.as_ref(), &format!("DROP SCHEMA {schema} CASCADE")).await;
        ok_targets += 1;
    }
    if reachable(3306) {
        let my = open(&mysql_config()).await;
        let db = unique("theta_cp");
        q(my.as_ref(), &format!("CREATE DATABASE {db}")).await;
        let c =
            copy(lite.as_ref(), "main", "odd", my.as_ref(), &db, "odd", true, 1000, &no_cancel(), None).await;
        assert!(matches!(c.report.outcome, CopyOutcome::Failed(_)), "{:?}", c.report);
        assert_eq!(c.report.cleanup, Cleanup::Dropped);
        assert!(!exists(my.as_ref(), &db, "odd").await);
        q(my.as_ref(), &format!("DROP DATABASE {db}")).await;
        ok_targets += 1;
    }
    eprintln!("failure cleanup checked on {ok_targets} server target(s)");
    drop(lite);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_table_cannot_be_made_over_an_existing_one_in_sqlite() {
    let (lc, dir) = file_config(DatabaseKind::Sqlite, "dup");
    let lite = open(&lc).await;
    q(lite.as_ref(), "CREATE TABLE a (id INTEGER PRIMARY KEY, v TEXT)").await;
    q(lite.as_ref(), "INSERT INTO a VALUES (1, 'x')").await;
    let c = copy(lite.as_ref(), "main", "a", lite.as_ref(), "main", "a", true, 100, &no_cancel(), None).await;
    assert!(matches!(c.report.outcome, CopyOutcome::Failed(_)));
    assert_eq!(c.report.cleanup, Cleanup::RolledBack);
    assert_eq!(q(lite.as_ref(), "SELECT count(*) FROM a").await[0][0], Value::Int(1));
    let c = copy(lite.as_ref(), "main", "a", lite.as_ref(), "main", "b", true, 100, &no_cancel(), None).await;
    assert_eq!(c.report.outcome, CopyOutcome::Completed, "{:?}", c.report);
    assert_eq!(q(lite.as_ref(), "SELECT v FROM b").await[0][0], Value::Text("x".into()));
    drop(lite);
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(feature = "duckdb")]
#[tokio::test(flavor = "multi_thread")]
async fn postgres_to_duckdb() {
    if !reachable(5432) {
        eprintln!("skipped: no PostgreSQL on 127.0.0.1:5432");
        return;
    }
    let pg = open(&pg_config()).await;
    let schema = unique("theta_cp");
    seed_postgres(pg.as_ref(), &schema).await;
    let (dc, dir) = file_config(DatabaseKind::DuckDb, "target");
    let duck = open(&dc).await;
    let c = copy(
        pg.as_ref(),
        &schema,
        "mixed",
        duck.as_ref(),
        "main",
        "mixed_copy",
        true,
        1000,
        &no_cancel(),
        None,
    )
    .await;
    assert_eq!(c.report.outcome, CopyOutcome::Completed, "{:?}", c.report);
    let rows = read_mixed(duck.as_ref(), DatabaseKind::DuckDb, "", "mixed_copy").await;
    assert_eq!(rows[0][1], Value::Text("Ann".into()));
    assert!((text_of(&rows[0][2]).parse::<f64>().unwrap() - 12.5).abs() < 1e-9);
    assert_eq!(text_of(&rows[0][4]), "11111111-1111-1111-1111-111111111111");
    assert_eq!(rows[0][9], Value::Text("O'Brien \\ x".into()));
    assert_eq!(rows[0][7], Value::Bytes(vec![0, 255, 16]));
    assert!(rows[1][2].is_null() && rows[1][8].is_null());
    let cancel = no_cancel();
    let c = copy(pg.as_ref(), &schema, "bulk", duck.as_ref(), "main", "bulk", true, 500, &cancel, Some(500))
        .await;
    assert_eq!(c.report.outcome, CopyOutcome::Cancelled);
    assert_eq!(c.report.cleanup, Cleanup::RolledBack);
    assert!(!exists(duck.as_ref(), "main", "bulk").await);
    drop(duck);
    let _ = std::fs::remove_dir_all(dir);
    q(pg.as_ref(), &format!("DROP SCHEMA {schema} CASCADE")).await;
}
