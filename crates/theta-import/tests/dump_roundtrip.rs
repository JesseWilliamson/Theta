//! Dump a database with Theta's own dump, restore the file with the script runner into a scratch schema or
//! database, and compare what came back: columns, keys, indexes, row counts and a checksum of every row.
//!
//! PostgreSQL and MariaDB run against the local test servers (see AGENTS.md) and skip, printing why, when
//! they do not answer. The seeded `quarry_demo` data is only read; everything written goes to a schema or
//! database created here and dropped at the end.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use theta_core::*;
use theta_import::dump::{DumpOptions, DumpOutcome, dump_to_path};
use theta_import::script::{ScriptOptions, ScriptOutcome, run_script};

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

fn reachable(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(300),
    )
    .is_ok()
}

async fn pg() -> Option<Box<dyn Connection>> {
    if !reachable(5432) {
        eprintln!("skipped: no PostgreSQL on 127.0.0.1:5432");
        return None;
    }
    let mut c = ConnectionConfig::new("t", "pg", DatabaseKind::Postgres);
    (c.host, c.user, c.password, c.database) =
        ("127.0.0.1".into(), "postgres".into(), Some("postgres".into()), "quarry_demo".into());
    Some(theta_drivers::connect(&c).await.expect("postgres"))
}

async fn mysql() -> Option<Box<dyn Connection>> {
    if !reachable(3306) {
        eprintln!("skipped: no MariaDB on 127.0.0.1:3306");
        return None;
    }
    let mut c = ConnectionConfig::new("t", "my", DatabaseKind::MySql);
    (c.host, c.user, c.password, c.database) =
        ("127.0.0.1".into(), "quarry".into(), Some("quarry".into()), "quarry_demo".into());
    Some(theta_drivers::connect(&c).await.expect("mysql"))
}

async fn sqlite(path: &std::path::Path) -> Box<dyn Connection> {
    let mut c = ConnectionConfig::new("t", "sqlite", DatabaseKind::Sqlite);
    c.path = path.to_string_lossy().into_owned();
    theta_drivers::connect(&c).await.expect("sqlite")
}

/// A checksum of every row, in key order. Two copies of a table that hash equal hold the same values: the
/// hash is taken over `Value`'s debug form, which keeps the type (`Int(1)` is not `Text("1")`).
async fn checksum(conn: &dyn Connection, kind: DatabaseKind, schema: Option<&str>, t: &Table) -> (u64, u64) {
    use std::hash::{DefaultHasher, Hasher};
    let name = theta_sql::keywords::qualified_name(schema, &t.name, kind);
    let order = if t.primary_key.is_empty() {
        (1..=t.columns.len()).map(|i| i.to_string()).collect::<Vec<_>>().join(", ")
    } else {
        t.primary_key
            .iter()
            .map(|c| theta_sql::keywords::quote_if_needed(c, kind))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let rows = q(conn, &format!("SELECT * FROM {name} ORDER BY {order}")).await;
    let mut h = DefaultHasher::new();
    for r in &rows {
        h.write(format!("{r:?}").as_bytes());
    }
    (rows.len() as u64, h.finish())
}

fn describe(t: &Table) -> String {
    let mut s = String::new();
    for c in &t.columns {
        s.push_str(&format!(
            "col {} {} null={} default={:?} identity={:?}\n",
            c.name,
            c.data_type.to_lowercase(),
            c.nullable,
            c.default.as_deref().map(|d| d.replace("::", "")),
            c.identity
        ));
    }
    s.push_str(&format!("pk {:?}\n", t.primary_key));
    let mut fks: Vec<String> = t
        .foreign_keys
        .iter()
        .map(|f| {
            format!(
                "fk {:?} -> {} {:?} del={:?} upd={:?}",
                f.columns, f.referenced_table, f.referenced_columns, f.on_delete, f.on_update
            )
        })
        .collect();
    fks.sort();
    s.push_str(&fks.join("\n"));
    let mut idx: Vec<String> = t
        .indexes
        .iter()
        .filter(|i| !i.primary)
        .map(|i| format!("idx {:?} unique={}", i.columns, i.unique))
        .collect();
    idx.sort();
    s.push_str(&format!("\n{}\n", idx.join("\n")));
    let mut checks: Vec<String> = t.checks.iter().map(|c| c.expression.clone()).collect();
    checks.sort();
    s.push_str(&format!("checks {checks:?}\n"));
    s
}

struct Compared {
    tables: usize,
    rows: u64,
}

async fn compare(
    src: &dyn Connection,
    dst: &dyn Connection,
    kind: DatabaseKind,
    src_schema: Option<&str>,
    dst_schema: Option<&str>,
    names: &[String],
) -> Compared {
    let (sc, dc) = (src.introspect().await.unwrap(), dst.introspect().await.unwrap());
    let (mut tables, mut rows) = (0, 0);
    for n in names {
        let find = |c: &Catalog, s: Option<&str>| -> Table {
            let schema = match s {
                Some(s) => c.schema(s),
                None => c.schemas.first(),
            }
            .unwrap_or_else(|| panic!("schema {s:?} missing"));
            schema
                .tables
                .iter()
                .find(|t| t.name == *n)
                .unwrap_or_else(|| panic!("table {n} missing in {s:?}"))
                .clone()
        };
        let (a, b) = (find(&sc, src_schema), find(&dc, dst_schema));
        assert_eq!(describe(&a), describe(&b), "structure of {n} differs");
        let (ca, cb) = (checksum(src, kind, src_schema, &a).await, checksum(dst, kind, dst_schema, &b).await);
        assert_eq!(ca, cb, "rows of {n} differ (count, checksum)");
        assert!(ca.0 > 0 || n == "empty", "{n} should have rows to make the comparison meaningful");
        tables += 1;
        rows += ca.0;
    }
    Compared { tables, rows }
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(unique("theta-dump-test"));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

async fn dump_file(
    conn: &dyn Connection,
    o: &DumpOptions,
    path: &std::path::Path,
) -> theta_import::DumpReport {
    let catalog = conn.introspect().await.unwrap();
    let rep = dump_to_path(conn, &catalog, o, path, &AtomicBool::new(false), &|_| {}).await;
    assert_eq!(rep.outcome, DumpOutcome::Completed, "{:?}", rep.outcome);
    rep
}

async fn restore(conn: &dyn Connection, kind: DatabaseKind, path: &std::path::Path, txn: bool) {
    let file = std::fs::File::open(path).unwrap();
    let len = file.metadata().unwrap().len();
    let mut o = ScriptOptions::new(kind);
    o.single_transaction = txn;
    let rep = run_script(conn, file, Some(len), &o, &AtomicBool::new(false), &|_| {}).await;
    assert_eq!(rep.outcome, ScriptOutcome::Completed, "errors: {:#?}", rep.errors);
    assert!(rep.errors.is_empty());
    assert!(rep.statements > 5);
}

const DEMO: [&str; 7] =
    ["customers", "delivery_zones", "order_items", "orders", "products", "routes", "stores"];

#[tokio::test(flavor = "multi_thread")]
async fn postgres_demo_schema_round_trips() {
    let Some(conn) = pg().await else { return };
    let kind = DatabaseKind::Postgres;
    let scratch = unique("rt");
    let mut o = DumpOptions::new(kind);
    o.tables = DEMO.iter().map(|t| ("public".to_string(), t.to_string())).collect();
    o.views = vec![("public".into(), "customer_order_summary".into())];
    o.qualify = false;
    let file = tmp("pg.sql");
    let rep = dump_file(conn.as_ref(), &o, &file).await;
    assert!(rep.rows > 0 && rep.tables == 7, "{rep:?}");

    // Restore into a scratch schema on a second session; `public` stays on the path for PostGIS's types.
    let target = pg().await.unwrap();
    q(target.as_ref(), &format!("CREATE SCHEMA {scratch}")).await;
    q(target.as_ref(), &format!("SET search_path = {scratch}, public")).await;
    restore(target.as_ref(), kind, &file, true).await;
    let c = compare(
        conn.as_ref(),
        target.as_ref(),
        kind,
        Some("public"),
        Some(&scratch),
        &DEMO.map(String::from),
    )
    .await;
    // The view came back and answers the same.
    let a = q(conn.as_ref(), "SELECT * FROM public.customer_order_summary ORDER BY 1").await;
    let b = q(target.as_ref(), &format!("SELECT * FROM {scratch}.customer_order_summary ORDER BY 1")).await;
    assert_eq!(a, b);
    assert!(!a.is_empty());
    // The identity counter continues after the restored rows instead of colliding with them.
    let max = q(target.as_ref(), &format!("SELECT max(id) FROM {scratch}.customers")).await;
    q(target.as_ref(), &format!("SET search_path = {scratch}, public")).await;
    q(target.as_ref(), "INSERT INTO customers (first_name, last_name, email, created_at) VALUES ('a', 'b', 'roundtrip@example.test', now())").await;
    let next = q(target.as_ref(), &format!("SELECT max(id) FROM {scratch}.customers")).await;
    assert_ne!(max, next);
    q(target.as_ref(), &format!("DROP SCHEMA {scratch} CASCADE")).await;
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
    eprintln!("postgres round trip: {} tables, {} rows compared", c.tables, c.rows);
}

#[tokio::test(flavor = "multi_thread")]
async fn mariadb_demo_database_round_trips() {
    let Some(conn) = mysql().await else { return };
    let kind = DatabaseKind::MySql;
    let scratch = unique("rt");
    let mut o = DumpOptions::new(kind);
    o.tables = DEMO.iter().map(|t| ("quarry_demo".to_string(), t.to_string())).collect();
    o.qualify = false;
    let file = tmp("my.sql");
    dump_file(conn.as_ref(), &o, &file).await;

    let target = mysql().await.unwrap();
    q(target.as_ref(), &format!("CREATE DATABASE {scratch}")).await;
    q(target.as_ref(), &format!("USE {scratch}")).await;
    // MariaDB commits DDL implicitly, so a single transaction would only cover the INSERTs; run plainly.
    restore(target.as_ref(), kind, &file, false).await;
    let c = compare(
        conn.as_ref(),
        target.as_ref(),
        kind,
        Some("quarry_demo"),
        Some(&scratch),
        &DEMO.map(String::from),
    )
    .await;
    q(target.as_ref(), &format!("DROP DATABASE {scratch}")).await;
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
    eprintln!("mariadb round trip: {} tables, {} rows compared", c.tables, c.rows);
}

const SQLITE_SOURCE: &[&str] = &[
    "CREATE TABLE stores (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, opened DATE)",
    "CREATE TABLE products (id INTEGER PRIMARY KEY AUTOINCREMENT, sku TEXT NOT NULL, price REAL DEFAULT 9.5, photo BLOB, note TEXT)",
    "CREATE INDEX products_sku ON products (sku)",
    "CREATE TABLE stock (store_id INTEGER NOT NULL REFERENCES stores(id) ON DELETE CASCADE, product_id INTEGER NOT NULL REFERENCES products(id), qty INTEGER NOT NULL DEFAULT 0 CHECK (qty >= 0), PRIMARY KEY (store_id, product_id))",
    "CREATE VIEW low_stock AS SELECT store_id, product_id FROM stock WHERE qty < 5",
    "INSERT INTO stores VALUES (1, 'Main St', '2020-01-02'), (2, 'O''Brien''s; \"quoted\"', NULL)",
    "INSERT INTO products (sku, price, photo, note) VALUES ('a-1', 1.25, x'00ff10', 'line1\nline2 -- not a comment'), ('b-2', NULL, NULL, 'ünïcode ✓ 漢字'), ('c-3', 3, x'', '')",
    "INSERT INTO stock VALUES (1, 1, 10), (1, 2, 3), (2, 3, 0)",
];

#[tokio::test(flavor = "multi_thread")]
async fn sqlite_round_trips_including_awkward_values() {
    let kind = DatabaseKind::Sqlite;
    let (src_path, dst_path) = (tmp("src.db"), tmp("dst.db"));
    let src = sqlite(&src_path).await;
    for s in SQLITE_SOURCE {
        q(src.as_ref(), s).await;
    }
    let mut o = DumpOptions::new(kind);
    o.tables = ["stores", "products", "stock"].map(|t| ("main".to_string(), t.to_string())).to_vec();
    o.views = vec![("main".into(), "low_stock".into())];
    // Small batches so several INSERT statements per table are exercised.
    o.rows_per_insert = 2;
    o.page_rows = 2;
    let file = tmp("sqlite.sql");
    let rep = dump_file(src.as_ref(), &o, &file).await;
    assert_eq!(rep.rows, 8);

    let dst = sqlite(&dst_path).await;
    restore(dst.as_ref(), kind, &file, true).await;
    let c = compare(
        src.as_ref(),
        dst.as_ref(),
        kind,
        None,
        None,
        &["stores", "products", "stock"].map(String::from),
    )
    .await;
    let a = q(src.as_ref(), "SELECT * FROM low_stock ORDER BY 1, 2").await;
    assert_eq!(a, q(dst.as_ref(), "SELECT * FROM low_stock ORDER BY 1, 2").await);
    assert_eq!(c.rows, 8);
    for p in [&src_path, &dst_path, &file] {
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}

/// An error in the middle of a script: stop-on-error stops there, continue collects, and a single
/// transaction leaves nothing behind.
#[tokio::test(flavor = "multi_thread")]
async fn script_error_modes_on_sqlite() {
    let kind = DatabaseKind::Sqlite;
    let script = "CREATE TABLE t (a INTEGER PRIMARY KEY);\nINSERT INTO t VALUES (1);\nINSERT INTO nope VALUES (1);\nINSERT INTO t VALUES (2);\nINSERT INTO t VALUES (1);\nINSERT INTO t VALUES (3);\n";
    let run = |o: ScriptOptions| async move {
        let c = sqlite(std::path::Path::new(":memory:")).await;
        let rep = run_script(c.as_ref(), script.as_bytes(), None, &o, &AtomicBool::new(false), &|_| {}).await;
        let n = q(c.as_ref(), "SELECT count(*) FROM sqlite_master WHERE name = 't'").await;
        let rows = if n[0][0] == Value::Int(1) {
            q(c.as_ref(), "SELECT count(*) FROM t").await[0][0].clone()
        } else {
            Value::Null
        };
        (rep, rows)
    };
    let (rep, rows) = run(ScriptOptions::new(kind)).await;
    assert_eq!(rep.outcome, ScriptOutcome::Stopped);
    assert_eq!((rep.statements, rep.error_count), (3, 1));
    assert_eq!(rep.errors[0].line, 3);
    assert!(
        rep.errors[0].head.contains("nope") && rep.errors[0].message.to_lowercase().contains("no such table")
    );
    assert_eq!(rows, Value::Int(1));

    let mut o = ScriptOptions::new(kind);
    o.stop_on_error = false;
    let (rep, rows) = run(o).await;
    assert_eq!(rep.outcome, ScriptOutcome::Completed);
    assert_eq!((rep.statements, rep.error_count), (6, 2));
    assert_eq!(rows, Value::Int(3), "1, 2 and 3 are in; the duplicate 1 and the missing table were skipped");

    let mut o = ScriptOptions::new(kind);
    o.single_transaction = true;
    let (rep, rows) = run(o).await;
    assert!(rep.rolled_back && rep.outcome == ScriptOutcome::Stopped);
    assert_eq!(rows, Value::Null, "the rollback took the CREATE TABLE with it (SQLite DDL is transactional)");
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_stops_between_statements() {
    let c = sqlite(std::path::Path::new(":memory:")).await;
    let cancel = AtomicBool::new(false);
    let script = "CREATE TABLE t (a);\n".to_string() + &"INSERT INTO t VALUES (1);\n".repeat(10_000);
    let seen = std::sync::atomic::AtomicU64::new(0);
    let rep = run_script(
        c.as_ref(),
        script.as_bytes(),
        None,
        &ScriptOptions::new(DatabaseKind::Sqlite),
        &cancel,
        &|p| {
            if p.statements >= 50 {
                cancel.store(true, Ordering::SeqCst);
            }
            seen.store(p.statements, Ordering::SeqCst);
        },
    )
    .await;
    assert_eq!(rep.outcome, ScriptOutcome::Cancelled);
    assert!(rep.statements >= 50 && rep.statements < 10_000, "{}", rep.statements);
}

/// A dump that is cancelled leaves no file, not even a partial one.
#[tokio::test(flavor = "multi_thread")]
async fn cancelled_dump_leaves_no_file() {
    let path = tmp("c.db");
    let src = sqlite(&path).await;
    for s in &SQLITE_SOURCE[..3] {
        q(src.as_ref(), s).await;
    }
    let cat = src.introspect().await.unwrap();
    let mut o = DumpOptions::new(DatabaseKind::Sqlite);
    o.tables = vec![("main".into(), "stores".into())];
    let out = tmp("c.sql");
    let rep = dump_to_path(src.as_ref(), &cat, &o, &out, &AtomicBool::new(true), &|_| {}).await;
    assert_eq!(rep.outcome, DumpOutcome::Cancelled);
    assert!(!out.exists() && !out.with_file_name("c.sql.part").exists());
}

#[cfg(feature = "duckdb")]
const DUCKDB_SOURCE: &[&str] = &[
    "CREATE TABLE stores (id INTEGER PRIMARY KEY, name VARCHAR NOT NULL UNIQUE, opened DATE)",
    "CREATE TABLE products (id INTEGER PRIMARY KEY, sku VARCHAR NOT NULL, price DOUBLE DEFAULT 9.5, photo BLOB, note VARCHAR)",
    "CREATE INDEX products_sku ON products (sku)",
    "CREATE TABLE stock (store_id INTEGER NOT NULL REFERENCES stores(id), product_id INTEGER NOT NULL REFERENCES products(id), qty INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (store_id, product_id))",
    "CREATE VIEW low_stock AS SELECT store_id, product_id FROM stock WHERE qty < 5",
    "INSERT INTO stores VALUES (1, 'Main St', DATE '2020-01-02'), (2, 'O''Brien''s; \"quoted\"', NULL)",
    "INSERT INTO products VALUES (1, 'a-1', 1.25, '\\x00\\xff\\x10'::BLOB, 'line1\nline2 -- not a comment'), (2, 'b-2', NULL, NULL, 'ünïcode ✓ 漢字'), (3, 'c-3', 3, ''::BLOB, '')",
    "INSERT INTO stock VALUES (1, 1, 10), (1, 2, 3), (2, 3, 0)",
];

#[cfg(feature = "duckdb")]
#[tokio::test(flavor = "multi_thread")]
async fn duckdb_round_trips() {
    let kind = DatabaseKind::DuckDb;
    let open = |p: &std::path::Path| {
        let mut c = ConnectionConfig::new("t", "duck", kind);
        c.path = p.to_string_lossy().into_owned();
        async move { theta_drivers::connect(&c).await.expect("duckdb") }
    };
    let (src_path, dst_path) = (tmp("src.duckdb"), tmp("dst.duckdb"));
    let src = open(&src_path).await;
    for s in DUCKDB_SOURCE {
        q(src.as_ref(), s).await;
    }
    let mut o = DumpOptions::new(kind);
    o.tables = ["stores", "products", "stock"].map(|t| ("main".to_string(), t.to_string())).to_vec();
    o.views = vec![("main".into(), "low_stock".into())];
    o.qualify = false;
    o.rows_per_insert = 2;
    let file = tmp("duck.sql");
    dump_file(src.as_ref(), &o, &file).await;
    let dst = open(&dst_path).await;
    restore(dst.as_ref(), kind, &file, false).await;
    let c = compare(
        src.as_ref(),
        dst.as_ref(),
        kind,
        Some("main"),
        Some("main"),
        &["stores", "products", "stock"].map(String::from),
    )
    .await;
    assert_eq!(c.rows, 8);
    for p in [&src_path, &dst_path, &file] {
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
