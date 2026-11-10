mod common;

use common::*;
use quarry_core::*;
use std::time::{Duration, Instant};

async fn mem() -> Box<dyn Connection> {
    quarry_drivers::connect(&sqlite_config(":memory:")).await.unwrap()
}

#[tokio::test]
async fn dynamic_types_and_declared_kinds() {
    let c = mem().await;
    run(
        c.as_ref(),
        "CREATE TABLE t (i INTEGER, r REAL, s TEXT, b BLOB, n NUMERIC, v VARCHAR(10), flag BOOLEAN, j JSON, d DATE, ts DATETIME, x)",
    )
    .await;
    run(
        c.as_ref(),
        "INSERT INTO t VALUES (1, 1.5, 'héllo', x'dead', '12.50', 'abc', 1, '{\"a\":1}', '2024-02-29', '2024-02-29 10:00:00', 'dyn')",
    )
    .await;
    run(c.as_ref(), "INSERT INTO t (i) VALUES (NULL)").await;
    let rs = run(c.as_ref(), "SELECT * FROM t ORDER BY i DESC").await.result.unwrap();
    let kinds: Vec<TypeKind> = rs.columns.iter().map(|c| c.kind).collect();
    use TypeKind::*;
    assert_eq!(kinds, vec![Integer, Float, Text, Bytes, Decimal, Text, Bool, Json, Date, Timestamp, Text]);
    assert_eq!(rs.columns[5].type_name, "VARCHAR(10)");
    let r = &rs.rows[0];
    assert_eq!(r[0], Value::Int(1));
    assert_eq!(r[1], Value::Float(1.5));
    assert_eq!(r[2], text("héllo"));
    assert_eq!(r[3], Value::Bytes(vec![0xde, 0xad]));
    // NUMERIC affinity stores '12.50' as the REAL 12.5; the cell reports what is stored.
    assert_eq!(r[4], Value::Float(12.5));
    assert_eq!(r[6], Value::Bool(true));
    assert_eq!(r[7], Value::Json("{\"a\":1}".into()));
    assert_eq!(r[8], Value::Date("2024-02-29".into()));
    assert_eq!(r[9], Value::Timestamp("2024-02-29 10:00:00".into()));
    assert_eq!(r[10], text("dyn"));
    assert!(rs.rows[1].iter().all(Value::is_null));
    // Column x has no declared type, so its kind came from the value. Expression columns likewise: infer from the value.
    let rs = run(c.as_ref(), "SELECT 1 AS a, 2.5 AS b, 'x' AS c, x'00' AS d, NULL AS e").await.result.unwrap();
    let kinds: Vec<TypeKind> = rs.columns.iter().map(|c| c.kind).collect();
    assert_eq!(kinds, vec![Integer, Float, Text, Bytes, Other]);
    // Dynamic typing: an INTEGER column can hold text.
    run(c.as_ref(), "CREATE TABLE dyn (i INTEGER)").await;
    run(c.as_ref(), "INSERT INTO dyn VALUES ('not a number')").await;
    assert_eq!(scalar(c.as_ref(), "SELECT i FROM dyn").await, text("not a number"));
}

#[tokio::test]
async fn limits_rows_affected_and_errors() {
    let c = mem().await;
    let rs = c
        .execute("WITH RECURSIVE g(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM g WHERE x < 5000) SELECT x FROM g", &ExecOptions { row_limit: Some(100) })
        .await
        .unwrap()
        .result
        .unwrap();
    assert_eq!((rs.rows.len(), rs.truncated), (100, true));
    let rs = c.execute("SELECT 1 UNION ALL SELECT 2", &ExecOptions { row_limit: Some(2) }).await.unwrap().result.unwrap();
    assert_eq!((rs.rows.len(), rs.truncated), (2, false));

    assert_eq!(run(c.as_ref(), "CREATE TABLE t (id INTEGER PRIMARY KEY, v)").await.rows_affected, None);
    assert_eq!(run(c.as_ref(), "INSERT INTO t (v) VALUES (1),(2),(3)").await.rows_affected, Some(3));
    assert_eq!(run(c.as_ref(), "UPDATE t SET v = v + 1 WHERE v > 1").await.rows_affected, Some(2));
    let ret = run(c.as_ref(), "INSERT INTO t (v) VALUES (9) RETURNING id").await;
    assert_eq!((ret.rows_affected, ret.result.unwrap().rows.len()), (Some(1), 1));
    assert_eq!(run(c.as_ref(), "DELETE FROM t").await.rows_affected, Some(4));
    assert_eq!(run(c.as_ref(), "SELECT * FROM t").await.rows_affected, None);

    let sql = "SELECT 'é', FROM t";
    let Error::Query { message, position, .. } = c.execute(sql, &all()).await.unwrap_err() else { panic!() };
    assert!(message.contains("syntax error"), "{message}");
    assert_eq!(position.unwrap().offset, sql.find("FROM").unwrap());
    let Error::Query { code, .. } = c.execute("INSERT INTO nope VALUES (1)", &all()).await.unwrap_err() else { panic!() };
    assert_eq!(code.as_deref(), Some("Unknown"));
    assert!(matches!(c.execute("SELECT 1; SELECT 2", &all()).await, Err(Error::Query { .. })));
    // Constraint violations surface the SQLite message.
    run(c.as_ref(), "CREATE TABLE u (a UNIQUE)").await;
    run(c.as_ref(), "INSERT INTO u VALUES (1)").await;
    let Error::Query { message, code, .. } = c.execute("INSERT INTO u VALUES (1)", &all()).await.unwrap_err() else { panic!() };
    assert!(message.contains("UNIQUE"), "{message}");
    assert_eq!(code.as_deref(), Some("ConstraintViolation"));
}

#[tokio::test]
async fn files_persist_and_connect_errors_map() {
    let dir = std::env::temp_dir().join(unique("quarry-sqlite"));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("db.sqlite");
    let cfg = sqlite_config(path.to_str().unwrap());
    let c = quarry_drivers::connect(&cfg).await.unwrap();
    run(c.as_ref(), "CREATE TABLE t (a)").await;
    run(c.as_ref(), "INSERT INTO t VALUES (42)").await;
    let info = c.server_info().await.unwrap();
    assert!(info.version.starts_with("SQLite 3."));
    assert!(info.current_database.unwrap().ends_with("db.sqlite"));
    c.close().await.unwrap();
    assert!(matches!(c.execute("SELECT 1", &all()).await, Err(Error::Closed)));
    let c = quarry_drivers::connect(&cfg).await.unwrap();
    assert_eq!(scalar(c.as_ref(), "SELECT a FROM t").await, Value::Int(42));

    let garbage = dir.join("garbage.sqlite");
    std::fs::write(&garbage, b"this is not a database file at all, just text. ".repeat(50)).unwrap();
    let e = quarry_drivers::connect(&sqlite_config(garbage.to_str().unwrap())).await.err().unwrap();
    assert!(matches!(e, Error::Connect(_)), "{e:?}");
    let e = quarry_drivers::connect(&sqlite_config(dir.join("no/such/dir/x.db").to_str().unwrap())).await.err().unwrap();
    assert!(matches!(e, Error::Connect(_)), "{e:?}");
    let e = quarry_drivers::connect(&sqlite_config("")).await.err().unwrap();
    assert!(matches!(e, Error::Connect(_)));
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn cancel_interrupts_a_long_statement() {
    let c: std::sync::Arc<dyn Connection> = mem().await.into();
    let c2 = c.clone();
    let started = Instant::now();
    let task = tokio::spawn(async move {
        c2.execute("WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r) SELECT count(*) FROM r", &all()).await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.cancel().await.unwrap();
    let r = tokio::time::timeout(Duration::from_secs(5), task).await.expect("cancel too slow").unwrap();
    assert!(matches!(r, Err(Error::Cancelled)), "{r:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(scalar(c.as_ref(), "SELECT 3").await, Value::Int(3));
    c.cancel().await.unwrap();
    assert_eq!(scalar(c.as_ref(), "SELECT 4").await, Value::Int(4));
}

#[tokio::test]
async fn in_transaction_follows_autocommit() {
    let c = mem().await;
    assert!(!c.in_transaction().await);
    run(c.as_ref(), "BEGIN").await;
    assert!(c.in_transaction().await);
    run(c.as_ref(), "CREATE TABLE t (a)").await;
    run(c.as_ref(), "ROLLBACK").await;
    assert!(!c.in_transaction().await);
    assert!(c.execute("SELECT * FROM t", &all()).await.is_err());
}

#[tokio::test]
async fn column_origins() {
    let c = mem().await;
    run(c.as_ref(), "CREATE TABLE people (id INTEGER PRIMARY KEY, name TEXT)").await;
    run(c.as_ref(), "INSERT INTO people VALUES (1, 'ann')").await;
    let cols = run(c.as_ref(), "SELECT p.id AS pid, name AS n, id + 1, 'x' FROM people p").await.result.unwrap().columns;
    let o = |t: &str, col: &str| Some(ColumnOrigin { schema: Some("main".into()), table: t.into(), column: col.into() });
    assert_eq!(cols[0].origin, o("people", "id"));
    assert_eq!(cols[1].origin, o("people", "name"));
    assert_eq!(cols[2].origin, None);
    assert_eq!(cols[3].origin, None);
}

#[tokio::test]
async fn introspection() {
    let c = mem().await;
    for sql in [
        "CREATE TABLE parent (a INTEGER, b INTEGER, label TEXT NOT NULL DEFAULT 'x', PRIMARY KEY (a, b))",
        "CREATE TABLE child (id INTEGER PRIMARY KEY AUTOINCREMENT, pa INTEGER, pb INTEGER, note VARCHAR(40), \
         FOREIGN KEY (pa, pb) REFERENCES parent (a, b) ON DELETE CASCADE ON UPDATE SET NULL)",
        "CREATE UNIQUE INDEX child_note ON child (note)",
        "CREATE INDEX child_expr ON child (lower(note))",
        "CREATE VIEW child_v AS SELECT id, note FROM child",
        "INSERT INTO parent VALUES (1,1,'a')",
        "INSERT INTO child (pa, pb) VALUES (1,1)",
        "ANALYZE",
    ] {
        run(c.as_ref(), sql).await;
    }
    let cat = c.introspect().await.unwrap();
    assert_eq!(cat.default_schema.as_deref(), Some("main"));
    assert_eq!(cat.schemas.len(), 1);
    let s = &cat.schemas[0];
    assert_eq!(s.name, "main");
    let t = |n: &str| s.tables.iter().find(|t| t.name == n).unwrap();
    let parent = t("parent");
    assert_eq!(parent.primary_key, vec!["a", "b"]);
    let label = &parent.columns[2];
    assert_eq!((label.nullable, label.default.as_deref(), label.kind), (false, Some("'x'"), TypeKind::Text));
    assert_eq!(parent.row_estimate, Some(1));
    let child = t("child");
    assert_eq!(child.primary_key, vec!["id"]);
    let fk = &child.foreign_keys[0];
    assert_eq!((fk.columns.clone(), fk.referenced_columns.clone()), (vec!["pa".into(), "pb".into()], vec!["a".into(), "b".into()]));
    assert_eq!((fk.referenced_table.as_str(), fk.on_delete.as_deref(), fk.on_update.as_deref()), ("parent", Some("CASCADE"), Some("SET NULL")));
    let uq = child.indexes.iter().find(|i| i.name == "child_note").unwrap();
    assert!(uq.unique && !uq.primary);
    assert!(uq.definition.as_deref().unwrap().starts_with("CREATE UNIQUE INDEX child_note"));
    assert_eq!(uq.columns, vec!["note"]);
    let ex = child.indexes.iter().find(|i| i.name == "child_expr").unwrap();
    assert_eq!(ex.columns, vec!["<expression>"]);
    assert_eq!(t("child_v").kind, TableKind::View);
    assert_eq!(t("child_v").columns.len(), 2);
    assert_eq!(s.sequences, vec!["child"]);
    assert!(!s.tables.iter().any(|t| t.name.starts_with("sqlite_")));
}

#[tokio::test]
async fn introspection_includes_attached_databases() {
    let c = mem().await;
    run(c.as_ref(), "ATTACH DATABASE ':memory:' AS other").await;
    run(c.as_ref(), "CREATE TABLE other.o (a INTEGER)").await;
    run(c.as_ref(), "CREATE TABLE m (a INTEGER)").await;
    let cat = c.introspect().await.unwrap();
    let names: Vec<&str> = cat.schemas.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["main", "other"]);
    assert_eq!(cat.schema("other").unwrap().tables[0].name, "o");
}

#[tokio::test]
async fn explain_and_analyze() {
    let c = mem().await;
    run(c.as_ref(), "CREATE TABLE t (id INTEGER PRIMARY KEY, v INTEGER)").await;
    run(c.as_ref(), "CREATE INDEX tv ON t (v)").await;
    run(c.as_ref(), "WITH RECURSIVE g(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM g WHERE x < 500) INSERT INTO t SELECT x, x % 10 FROM g").await;
    let plan = c.explain("SELECT * FROM t WHERE v = 3", false).await.unwrap();
    assert!(!plan.analyzed && plan.raw.contains("t"));
    assert_eq!(plan.root.node_type, "SEARCH");
    assert_eq!((plan.root.relation.as_deref(), plan.root.index.as_deref()), (Some("t"), Some("tv")));
    let plan = c.explain("SELECT * FROM t a JOIN t b ON b.id = a.v ORDER BY a.id DESC", false).await.unwrap();
    let mut n = 0;
    plan.root.walk(&mut |_, _| n += 1);
    assert!(n >= 2);
    let plan = c.explain("SELECT * FROM t WHERE v < 5", true).await.unwrap();
    assert!(plan.analyzed && plan.execution_time_ms.unwrap() >= 0.0);
    assert_eq!(plan.root.actual_rows, Some(250.0));
    // Writes are rolled back.
    c.explain("DELETE FROM t", true).await.unwrap();
    c.explain("INSERT INTO t VALUES (9999, 1)", true).await.unwrap();
    assert_eq!(scalar(c.as_ref(), "SELECT count(*) FROM t").await, Value::Int(500));
    assert!(!c.in_transaction().await);
    // Inside a user transaction the savepoint keeps the user's work.
    run(c.as_ref(), "BEGIN").await;
    run(c.as_ref(), "INSERT INTO t VALUES (7000, 1)").await;
    c.explain("INSERT INTO t VALUES (7001, 1)", true).await.unwrap();
    assert!(c.in_transaction().await);
    assert_eq!(scalar(c.as_ref(), "SELECT count(*) FROM t WHERE id >= 7000").await, Value::Int(1));
    run(c.as_ref(), "ROLLBACK").await;
    assert!(c.explain("SELECT * FROM nope", false).await.is_err());
}

#[tokio::test]
async fn demo_seed() {
    let c = mem().await;
    quarry_drivers::demo::seed(c.as_ref()).await.unwrap();
    quarry_drivers::demo::seed(c.as_ref()).await.unwrap();
    assert_eq!(scalar(c.as_ref(), "SELECT count(*) FROM customers").await, Value::Int(200));
    assert_eq!(scalar(c.as_ref(), "SELECT count(*) FROM products").await, Value::Int(40));
    assert_eq!(scalar(c.as_ref(), "SELECT count(*) FROM orders").await, Value::Int(600));
    assert_eq!(scalar(c.as_ref(), "SELECT count(*) FROM stores").await, Value::Int(25));
    assert_eq!(scalar(c.as_ref(), "SELECT count(*) FROM stores WHERE location_wkt LIKE 'POINT(%'").await, Value::Int(25));
    assert_eq!(
        scalar(c.as_ref(), "SELECT count(*) FROM orders o WHERE ABS(o.total - (SELECT SUM(quantity * unit_price) FROM order_items i WHERE i.order_id = o.id)) > 0.001").await,
        Value::Int(0)
    );
    assert!(rows(c.as_ref(), "PRAGMA foreign_key_check").await.is_empty());
    let cat = c.introspect().await.unwrap();
    let items = cat.schemas[0].tables.iter().find(|t| t.name == "order_items").unwrap();
    assert_eq!(items.primary_key, vec!["order_id", "line_no"]);
    assert_eq!(items.foreign_keys.len(), 2);
}
