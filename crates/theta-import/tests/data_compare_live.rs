//! Data compare against real engines: PostgreSQL and PostgreSQL, PostgreSQL and MariaDB, SQLite and SQLite.
//!
//! Each test seeds a *source* and a *target* schema (database, file) with known inserted, deleted and changed
//! rows, NULLs, a composite key and text keys whose order differs between products, compares them, checks every
//! count, applies the generated sync script to the target and compares again: zero differences. The seeded
//! `quarry_demo` tables are never touched; every schema, database and file is unique and removed afterwards.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use theta_core::*;
use theta_import::{CompareJob, CompareOutcome, CompareReport, KeyOrder, compare_tables};
use theta_sql::data_diff::{DiffKind, Mapping, map_columns, sync_script};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}_{}", std::process::id(), COUNTER.fetch_add(1, Ordering::SeqCst))
}

fn reachable(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(300),
    )
    .is_ok()
}

async fn run(conn: &dyn Connection, sql: &str) -> Result<Vec<Vec<Value>>, Error> {
    conn.execute(sql, &ExecOptions { row_limit: None })
        .await
        .map(|r| r.result.map(|r| r.rows).unwrap_or_default())
}

async fn q(conn: &dyn Connection, sql: &str) {
    run(conn, sql).await.unwrap_or_else(|e| panic!("`{sql}` failed: {e}"));
}

async fn open(c: &ConnectionConfig) -> Arc<dyn Connection> {
    Arc::from(theta_drivers::connect(c).await.expect("connect"))
}

#[derive(Clone)]
struct Db {
    conn: Arc<dyn Connection>,
    kind: DatabaseKind,
    schema: String,
}

impl Db {
    fn p(&self) -> String {
        format!("{}.", self.schema)
    }

    async fn table(&self, name: &str) -> Table {
        let cat = self.conn.introspect().await.expect("introspect");
        cat.find_table(Some(&self.schema), name)
            .unwrap_or_else(|| panic!("{}.{name} missing", self.schema))
            .1
            .clone()
    }
}

fn types(kind: DatabaseKind) -> [&'static str; 6] {
    // id, name, score, active, note, created
    match kind {
        DatabaseKind::Postgres => {
            ["integer", "varchar(40)", "numeric(10,2)", "boolean", "text", "timestamp(6)"]
        }
        DatabaseKind::Sqlite => ["INTEGER", "TEXT", "NUMERIC", "INTEGER", "TEXT", "TEXT"],
        _ => ["int", "varchar(40)", "decimal(10,2)", "tinyint(1)", "text", "datetime(3)"],
    }
}

fn boolean(kind: DatabaseKind, b: bool) -> &'static str {
    match (kind, b) {
        (DatabaseKind::Postgres, true) => "true",
        (DatabaseKind::Postgres, false) => "false",
        (_, true) => "1",
        (_, false) => "0",
    }
}

/// The source holds what is wanted, the target what exists. See the module comment for the expected counts.
async fn seed(db: &Db, source: bool) {
    let (p, k) = (db.p(), db.kind);
    let t = types(k);
    q(db.conn.as_ref(), &format!("CREATE TABLE {p}people (id {} PRIMARY KEY, name {} NOT NULL, score {}, active {}, note {}, created {})", t[0], t[1], t[2], t[3], t[4], t[5])).await;
    // Microseconds where the product keeps them, milliseconds where it does not.
    let ts = if k == DatabaseKind::Postgres || k == DatabaseKind::Sqlite {
        "2024-03-05 12:00:00.123456"
    } else {
        "2024-03-05 12:00:00.123"
    };
    let ts_src = "2024-03-05 12:00:00.123456";
    let at = |src: bool| if src { ts_src } else { ts };
    let b = |x| boolean(k, x);
    let rows: Vec<String> = if source {
        vec![
            format!("(1, 'Ann', 12.50, {}, NULL, '{}')", b(true), at(true)),
            format!("(2, 'Bob', NULL, {}, 'hi', '{}')", b(false), at(true)),
            format!("(3, 'Cy', 7.00, {}, 'x', '{}')", b(true), at(true)),
            format!("(4, 'Di', 1.00, {}, NULL, '{}')", b(false), at(true)),
            format!("(5, 'Ed', 2.00, {}, 'e', '{}')", b(true), at(true)),
            format!("(6, 'Flo', 3.25, {}, 'f', '{}')", b(true), at(true)),
        ]
    } else {
        vec![
            format!("(1, 'Ann', 12.5, {}, NULL, '{}')", b(true), at(false)),
            format!("(2, 'Bob', NULL, {}, 'hi', '{}')", b(false), at(false)),
            format!("(3, 'Cy', 7.50, {}, 'x', '{}')", b(true), at(false)),
            format!("(4, 'Di', 1.00, {}, NULL, '{}')", b(true), at(false)),
            format!("(5, 'Ed', 2.00, {}, NULL, '{}')", b(true), at(false)),
            format!("(7, 'Gus', 9.00, {}, 'g', '{}')", b(false), at(false)),
        ]
    };
    q(
        db.conn.as_ref(),
        &format!("INSERT INTO {p}people (id, name, score, active, note, created) VALUES {}", rows.join(", ")),
    )
    .await;

    // A composite key.
    q(db.conn.as_ref(), &format!("CREATE TABLE {p}pairs (a {} NOT NULL, b varchar(10) NOT NULL, v varchar(10), PRIMARY KEY (a, b))", t[0])).await;
    let pairs = if source {
        "(1,'x','a'), (1,'y','b'), (2,'x','c')"
    } else {
        "(1,'y','B'), (2,'x','c'), (2,'z','d')"
    };
    q(db.conn.as_ref(), &format!("INSERT INTO {p}pairs (a, b, v) VALUES {pairs}")).await;

    // Text keys: a case-insensitive server sorts these a, apple, B, c, D, Zed; bytes sort B, D, Zed, a, apple, c.
    q(db.conn.as_ref(), &format!("CREATE TABLE {p}words (k varchar(20) PRIMARY KEY, v varchar(10))")).await;
    let words = if source {
        "('a','1'), ('B','2'), ('c','3'), ('D','4'), ('apple','5'), ('Zed','6')"
    } else {
        "('a','1'), ('B','x'), ('c','3'), ('Q','9'), ('apple','5')"
    };
    q(db.conn.as_ref(), &format!("INSERT INTO {p}words (k, v) VALUES {words}")).await;
    // No usable key.
    q(db.conn.as_ref(), &format!("CREATE TABLE {p}keyless (x {}, y {})", t[0], t[0])).await;
}

fn job(
    src: &Db,
    dst: &Db,
    mapping: Mapping,
    table: &str,
    page: usize,
    cap: usize,
    fallback: bool,
) -> CompareJob {
    CompareJob {
        mapping,
        left_kind: src.kind,
        left_schema: Some(src.schema.clone()),
        left_table: table.to_string(),
        right_kind: dst.kind,
        right_schema: Some(dst.schema.clone()),
        right_table: table.to_string(),
        page_rows: page,
        cap,
        allow_fallback: fallback,
    }
}

async fn compare(
    src: &Db,
    dst: &Db,
    table: &str,
    page: usize,
    cap: usize,
    fallback: bool,
) -> (Mapping, CompareReport) {
    let (l, r) = (src.table(table).await, dst.table(table).await);
    let mapping = map_columns(&l, &r, &[]).unwrap_or_else(|e| panic!("{table}: {e}"));
    let rep = compare_tables(
        src.conn.as_ref(),
        dst.conn.as_ref(),
        &job(src, dst, mapping.clone(), table, page, cap, fallback),
        &AtomicBool::new(false),
        &|_| {},
    )
    .await;
    (mapping, rep)
}

fn counts(r: &CompareReport) -> (u64, u64, u64, u64) {
    (r.counts.only_left, r.counts.only_right, r.counts.different, r.counts.same)
}

/// Apply the sync script (deletes included) to the target and compare once more.
async fn sync_and_recompare(src: &Db, dst: &Db, table: &str, mapping: &Mapping, rep: &CompareReport) {
    let script = sync_script(mapping, &rep.rows, dst.kind, &dst.schema, table, true);
    assert!(!script.statements.is_empty());
    for s in &script.statements {
        if let Err(e) = run(dst.conn.as_ref(), &s.sql).await {
            panic!("{table}: `{}` failed: {e}", s.sql);
        }
    }
    let (_, again) = compare(src, dst, table, 2, 100, true).await;
    assert_eq!(again.outcome, CompareOutcome::Completed, "{table}");
    assert_eq!(
        again.counts.differences(),
        0,
        "{table}: after the sync script nothing differs: {:?}",
        again.counts
    );
    assert_eq!(again.counts.same, rep.counts.same + rep.counts.only_left + rep.counts.different, "{table}");
}

async fn scenario(src: Db, dst: Db) {
    let cross = src.kind != dst.kind;
    seed(&src, true).await;
    seed(&dst, false).await;

    // people: one inserted, one deleted, three changed (a number, a boolean, a NULL against a value), two same
    // (a different scale and, between products, a different timestamp precision are not differences; NULL
    // against NULL is not one).
    let (m, rep) = compare(&src, &dst, "people", 2, 10_000, true).await;
    assert_eq!(rep.outcome, CompareOutcome::Completed);
    assert_eq!(counts(&rep), (1, 1, 3, 2), "people ({:?} -> {:?}): {:?}", src.kind, dst.kind, rep.rows);
    assert_eq!((rep.left_rows, rep.right_rows), (6, 6));
    assert!(!rep.capped);
    let changed: Vec<(i64, Vec<&str>)> = rep
        .rows
        .iter()
        .filter(|r| r.kind == DiffKind::Different)
        .map(|r| {
            let id = if let Value::Int(i) = r.key[0] { i } else { panic!("{:?}", r.key) };
            (id, r.changed.iter().map(|&i| m.columns[i].left.name.as_str()).collect())
        })
        .collect();
    assert_eq!(
        changed,
        [(3, vec!["score"]), (4, vec!["active"]), (5, vec!["note"])],
        "{:?} -> {:?}",
        src.kind,
        dst.kind
    );
    assert!(m.left_only.is_empty() && m.right_only.is_empty());
    // The page size does not matter, and a cap keeps the first rows but not the counts.
    let (_, one) = compare(&src, &dst, "people", 1, 10_000, true).await;
    assert_eq!(counts(&one), counts(&rep));
    let (_, capped) = compare(&src, &dst, "people", 3, 2, true).await;
    assert_eq!(counts(&capped), counts(&rep), "counts stay exact when the rows kept are capped");
    assert_eq!(capped.rows.len(), 2);
    assert!(capped.capped);
    sync_and_recompare(&src, &dst, "people", &m, &rep).await;

    // A composite key.
    let (m, rep) = compare(&src, &dst, "pairs", 1, 100, true).await;
    assert_eq!(counts(&rep), (1, 1, 1, 1), "pairs: {:?}", rep.rows);
    sync_and_recompare(&src, &dst, "pairs", &m, &rep).await;

    // Text keys in an order that differs between products.
    let (m, rep) = compare(&src, &dst, "words", 2, 100, true).await;
    assert_eq!(rep.outcome, CompareOutcome::Completed, "{:?}", rep.outcome);
    assert_eq!(
        counts(&rep),
        (2, 1, 1, 3),
        "words ({:?} -> {:?}, order {:?}): {:?}",
        src.kind,
        dst.kind,
        rep.order,
        rep.rows
    );
    let mariadb = src.kind == DatabaseKind::MySql || dst.kind == DatabaseKind::MySql;
    if mariadb {
        // MariaDB's default collation is case-insensitive: its order is not bytes, so the first pass is
        // detected and the second pass, in binary collation, is the answer.
        assert_eq!(rep.order, KeyOrder::Binary, "the native order of MariaDB is not the client's");
        let (_, strict) = compare(&src, &dst, "words", 2, 100, false).await;
        match strict.outcome {
            CompareOutcome::Failed(why) => assert!(why.contains("sort this key differently"), "{why}"),
            other => panic!("without the fallback the unsorted stream must be refused, got {other:?}"),
        }
    }
    if src.kind == DatabaseKind::Sqlite {
        assert_eq!(rep.order, KeyOrder::Native, "SQLite already sorts by bytes");
    }
    sync_and_recompare(&src, &dst, "words", &m, &rep).await;

    // No key: refused before anything is read.
    let (l, r) = (src.table("keyless").await, dst.table("keyless").await);
    let e = map_columns(&l, &r, &[]).unwrap_err();
    assert!(e.contains("no primary key"), "{e}");
    // A key chosen by the user works, and the key must be unique: `x` repeats nowhere here.
    q(src.conn.as_ref(), &format!("INSERT INTO {}keyless VALUES (1, 1), (2, 5)", src.p())).await;
    q(dst.conn.as_ref(), &format!("INSERT INTO {}keyless VALUES (1, 1), (2, 6)", dst.p())).await;
    let (l, r) = (src.table("keyless").await, dst.table("keyless").await);
    let m = map_columns(&l, &r, &["x".to_string()]).unwrap();
    let rep = compare_tables(
        src.conn.as_ref(),
        dst.conn.as_ref(),
        &job(&src, &dst, m, "keyless", 10, 10, true),
        &AtomicBool::new(false),
        &|_| {},
    )
    .await;
    assert_eq!(counts(&rep), (0, 0, 1, 1), "{:?}", rep.rows);
    // A repeated key is refused with a sentence.
    q(src.conn.as_ref(), &format!("INSERT INTO {}keyless VALUES (2, 7)", src.p())).await;
    let (l, r) = (src.table("keyless").await, dst.table("keyless").await);
    let m = map_columns(&l, &r, &["x".to_string()]).unwrap();
    let rep = compare_tables(
        src.conn.as_ref(),
        dst.conn.as_ref(),
        &job(&src, &dst, m, "keyless", 10, 10, true),
        &AtomicBool::new(false),
        &|_| {},
    )
    .await;
    match rep.outcome {
        CompareOutcome::Failed(why) => assert!(why.contains("more than once"), "{why}"),
        other => panic!("a duplicate key must fail, got {other:?}"),
    }

    // Cancel: the flag set from the progress callback stops it after the page that was running.
    let cancel = AtomicBool::new(false);
    let (l, r) = (src.table("people").await, dst.table("people").await);
    let m = map_columns(&l, &r, &[]).unwrap();
    let rep = compare_tables(
        src.conn.as_ref(),
        dst.conn.as_ref(),
        &job(&src, &dst, m, "people", 1, 10, true),
        &cancel,
        &|p| {
            if p.left_rows >= 2 {
                cancel.store(true, Ordering::SeqCst);
            }
        },
    )
    .await;
    assert_eq!(rep.outcome, CompareOutcome::Cancelled);
    assert!(rep.left_rows < 6, "it stopped early: {} rows read", rep.left_rows);
    let _ = cross;
}

async fn guarded(body: tokio::task::JoinHandle<()>, cleanup: impl std::future::Future<Output = ()>) {
    let r = body.await;
    cleanup.await;
    if let Err(e) = r {
        std::panic::resume_unwind(e.into_panic());
    }
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

#[tokio::test(flavor = "multi_thread")]
async fn postgres_to_postgres() {
    if !reachable(5432) {
        eprintln!("skipped: no PostgreSQL on 127.0.0.1:5432");
        return;
    }
    let conn = open(&pg_config()).await;
    let (a, b) = (unique("cmpd_a"), unique("cmpd_b"));
    q(conn.as_ref(), &format!("CREATE SCHEMA {a}")).await;
    q(conn.as_ref(), &format!("CREATE SCHEMA {b}")).await;
    let mk = |s: &String| Db { conn: conn.clone(), kind: DatabaseKind::Postgres, schema: s.clone() };
    let body = tokio::spawn(scenario(mk(&a), mk(&b)));
    let c2 = conn.clone();
    guarded(body, async move {
        for s in [a, b] {
            let _ = run(c2.as_ref(), &format!("DROP SCHEMA IF EXISTS {s} CASCADE")).await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn postgres_and_mariadb_both_ways() {
    if !reachable(5432) || !reachable(3306) {
        eprintln!("skipped: PostgreSQL and MariaDB are both needed (127.0.0.1:5432 and :3306)");
        return;
    }
    let pg = open(&pg_config()).await;
    let my = open(&mysql_config()).await;
    for flip in [false, true] {
        let (schema, database) = (unique("cmpd_pg"), unique("cmpd_my"));
        if let Err(e) = run(my.as_ref(), &format!("CREATE DATABASE {database}")).await {
            eprintln!("skipped: this MariaDB user may not create databases ({e})");
            return;
        }
        q(pg.as_ref(), &format!("CREATE SCHEMA {schema}")).await;
        let p = Db { conn: pg.clone(), kind: DatabaseKind::Postgres, schema: schema.clone() };
        let m = Db { conn: my.clone(), kind: DatabaseKind::MySql, schema: database.clone() };
        let (src, dst) = if flip { (m, p) } else { (p, m) };
        let body = tokio::spawn(scenario(src, dst));
        let (pg2, my2) = (pg.clone(), my.clone());
        guarded(body, async move {
            let _ = run(pg2.as_ref(), &format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await;
            let _ = run(my2.as_ref(), &format!("DROP DATABASE IF EXISTS {database}")).await;
        })
        .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn sqlite_to_sqlite() {
    let dir = std::env::temp_dir().join(unique("theta_cmpd"));
    std::fs::create_dir_all(&dir).unwrap();
    let mut dbs = Vec::new();
    for n in ["src", "dst"] {
        let mut c = ConnectionConfig::new("t", n, DatabaseKind::Sqlite);
        c.path = dir.join(format!("{n}.db")).to_string_lossy().into_owned();
        dbs.push(Db { conn: open(&c).await, kind: DatabaseKind::Sqlite, schema: "main".into() });
    }
    let body = tokio::spawn(scenario(dbs[0].clone(), dbs[1].clone()));
    let d2 = dir.clone();
    guarded(body, async move {
        drop(dbs);
        let _ = std::fs::remove_dir_all(d2);
    })
    .await;
}
