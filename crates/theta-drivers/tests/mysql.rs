mod common;

use common::*;
use std::time::{Duration, Instant};
use theta_core::*;

#[tokio::test]
async fn type_round_trips() {
    let fx = MyFx::new().await;
    let c = fx.conn.as_ref();
    run(
        c,
        "CREATE TABLE t (
           ti tinyint, si smallint, mi mediumint, i int, bi bigint,
           uti tinyint unsigned, ubi bigint unsigned,
           d decimal(10,2), f float, db double,
           dt date, tm time, dtm datetime(6), ts timestamp NULL, yr year,
           c char(5), vc varchar(20), tx text, tt tinytext, bl blob, vb varbinary(8), bn binary(4),
           j json, b1 bit(1), b8 bit(8), en enum('small','large'), st set('a','b','c'),
           g point NOT NULL, bo boolean, nul int NULL)",
    )
    .await;
    run(
        c,
        "INSERT INTO t VALUES (-1, -300, -70000, -2000000000, -9000000000000000000,
           255, 18446744073709551615,
           12345.60, 1.5, 2.25,
           '2024-02-29', '-12:34:56', '2024-02-29 13:14:15.123456', '2024-02-29 13:14:15', 2024,
           'ab', 'héllo', 'some text', 'tiny', 0xDEADBEEF, 0x0102, 'ab',
           '{\"a\": [1,   2]}', b'1', b'00000101', 'large', 'a,c',
           ST_GeomFromText('POINT(1 2)', 4326), true, NULL)",
    )
    .await;
    run(c, "INSERT INTO t (g) VALUES (ST_GeomFromText('POINT(0 0)'))").await;
    let rs = run(c, "SELECT * FROM t ORDER BY i DESC").await.result.unwrap();
    let col = |n: &str| rs.columns.iter().position(|c| c.name == n).unwrap();
    let v = |n: &str| rs.rows[1][col(n)].clone(); // ORDER BY i DESC: NULL row first? see below
    // NULLs sort first ascending / last descending in MariaDB: row 0 is the full row.
    let full = |n: &str| rs.rows[0][col(n)].clone();
    let _ = v;
    assert_eq!(full("ti"), Value::Int(-1));
    assert_eq!(full("si"), Value::Int(-300));
    assert_eq!(full("mi"), Value::Int(-70000));
    assert_eq!(full("i"), Value::Int(-2_000_000_000));
    assert_eq!(full("bi"), Value::Int(-9_000_000_000_000_000_000));
    assert_eq!(full("uti"), Value::UInt(255));
    assert_eq!(full("ubi"), Value::UInt(u64::MAX));
    assert_eq!(full("d"), Value::Decimal("12345.60".into()));
    assert_eq!(full("f"), Value::Float(1.5));
    assert_eq!(full("db"), Value::Float(2.25));
    assert_eq!(full("dt"), Value::Date("2024-02-29".into()));
    assert_eq!(full("tm"), Value::Time("-12:34:56".into()));
    assert_eq!(full("dtm"), Value::Timestamp("2024-02-29 13:14:15.123456".into()));
    assert_eq!(full("ts"), Value::Timestamp("2024-02-29 13:14:15".into()));
    assert_eq!(full("yr"), Value::Int(2024));
    assert_eq!(full("c"), text("ab"));
    assert_eq!(full("vc"), text("héllo"));
    assert_eq!(full("tx"), text("some text"));
    assert_eq!(full("tt"), text("tiny"));
    assert_eq!(full("bl"), Value::Bytes(vec![0xde, 0xad, 0xbe, 0xef]));
    assert_eq!(full("vb"), Value::Bytes(vec![1, 2]));
    assert_eq!(full("bn"), Value::Bytes(vec![b'a', b'b', 0, 0]));
    // MariaDB's JSON is LONGTEXT with a CHECK constraint: the wire type is text.
    assert_eq!(full("j"), text("{\"a\": [1,   2]}"));
    assert_eq!(full("b1"), Value::Bool(true));
    assert_eq!(full("b8"), Value::UInt(5));
    assert_eq!(full("en"), text("large"));
    assert_eq!(full("st"), text("a,c"));
    assert_eq!(full("bo"), Value::Int(1));
    assert_eq!(full("nul"), Value::Null);
    let Value::Geometry(g) = full("g") else { panic!("{:?}", full("g")) };
    // EWKB: little endian, POINT | SRID flag, SRID 4326, x, y
    assert_eq!(&g[..9], &[1, 1, 0, 0, 0x20, 0xE6, 0x10, 0, 0]);
    assert_eq!(&g[9..17], &1f64.to_le_bytes());
    assert_eq!(&g[17..25], &2f64.to_le_bytes());
    // The other row: all NULL except the geometry (SRID 0 yields plain WKB).
    let other = &rs.rows[1];
    assert!(
        other
            .iter()
            .enumerate()
            .all(|(i, v)| i == col("g") || i == col("bo") && false || v.is_null() || i == col("bo"))
    );
    let Value::Geometry(g0) = &other[col("g")] else { panic!() };
    assert_eq!(&g0[..5], &[1, 1, 0, 0, 0]);

    use TypeKind::*;
    let k = |n: &str| rs.columns[col(n)].kind;
    assert_eq!(
        [
            k("ti"),
            k("d"),
            k("f"),
            k("dt"),
            k("tm"),
            k("dtm"),
            k("vc"),
            k("bl"),
            k("b1"),
            k("b8"),
            k("en"),
            k("g")
        ],
        [Integer, Decimal, Float, Date, Time, Timestamp, Text, Bytes, Bool, Integer, Text, Geometry]
    );
    let n = |name: &str| rs.columns[col(name)].type_name.clone();
    assert_eq!(n("ubi"), "bigint unsigned");
    assert_eq!(n("tx"), "text");
    assert_eq!(n("bl"), "blob");
    assert_eq!(n("vb"), "varbinary");
    assert_eq!(n("en"), "enum");
    assert_eq!(n("g"), "geometry");
    fx.done().await;
}

#[tokio::test]
async fn row_limit_streams_and_stops_the_server() {
    let fx = MyFx::new().await;
    let c = fx.conn.as_ref();
    let limited = |n| ExecOptions { row_limit: Some(n) };
    let rs = c.execute("SELECT seq FROM seq_1_to_5000", &limited(100)).await.unwrap().result.unwrap();
    assert_eq!((rs.rows.len(), rs.truncated), (100, true));
    assert_eq!(rs.rows[99][0], Value::UInt(100));
    let rs = c.execute("SELECT seq FROM seq_1_to_5000", &limited(5000)).await.unwrap().result.unwrap();
    assert_eq!((rs.rows.len(), rs.truncated), (5000, false));
    let rs = c.execute("SELECT seq FROM seq_1_to_5000", &all()).await.unwrap().result.unwrap();
    assert_eq!(rs.rows.len(), 5000);
    let rs = c.execute("SELECT 1 FROM dual WHERE 0", &limited(5)).await.unwrap().result.unwrap();
    assert_eq!((rs.rows.len(), rs.truncated, rs.columns.len()), (0, false, 1));
    // Killing the running query keeps the connection usable and fast.
    let started = Instant::now();
    let rs = c.execute("SELECT seq FROM seq_1_to_2000000000", &limited(10)).await.unwrap().result.unwrap();
    assert!(rs.truncated);
    assert!(started.elapsed() < Duration::from_secs(5), "took {:?}", started.elapsed());
    assert_eq!(scalar(c, "SELECT 5").await, Value::Int(5));
    // Rows come from a transaction the user opened: it must survive truncation.
    run(c, "START TRANSACTION").await;
    let _ = c.execute("SELECT seq FROM seq_1_to_100000", &limited(5)).await.unwrap();
    assert!(c.in_transaction().await);
    run(c, "ROLLBACK").await;
    fx.done().await;
}

#[tokio::test]
async fn rows_affected_warnings_and_errors() {
    let fx = MyFx::new().await;
    let c = fx.conn.as_ref();
    let ddl = run(c, "CREATE TABLE t (id int AUTO_INCREMENT PRIMARY KEY, v int, UNIQUE KEY uq (v))").await;
    assert_eq!((ddl.result.is_none(), ddl.rows_affected), (true, None));
    assert_eq!(run(c, "INSERT INTO t (v) VALUES (1),(2),(3)").await.rows_affected, Some(3));
    assert_eq!(run(c, "UPDATE t SET v = v + 10 WHERE v >= 2").await.rows_affected, Some(2));
    assert_eq!(run(c, "DELETE FROM t").await.rows_affected, Some(3));
    assert_eq!(run(c, "SELECT * FROM t").await.rows_affected, None);

    let out = run(c, "SELECT CAST('abc' AS SIGNED) AS x").await;
    assert_eq!(out.notices.len(), 1, "{:?}", out.notices);
    assert!(out.notices[0].starts_with("Warning (1292)"), "{}", out.notices[0]);
    assert!(run(c, "SELECT 1").await.notices.is_empty());

    let sql = "SELECT 1\nFROM t WHERE ??? = 1";
    let Error::Query { code, position, detail, .. } = c.execute(sql, &all()).await.unwrap_err() else {
        panic!()
    };
    assert_eq!(code.as_deref(), Some("1064"));
    assert_eq!(detail.as_deref(), Some("SQLSTATE 42000"));
    assert_eq!(position.unwrap().offset, sql.find("???").unwrap());
    run(c, "INSERT INTO t (v) VALUES (7)").await;
    let Error::Query { code, .. } = c.execute("INSERT INTO t (v) VALUES (7)", &all()).await.unwrap_err()
    else {
        panic!()
    };
    assert_eq!(code.as_deref(), Some("1062"));
    let Error::Query { code, .. } = c.execute("SELECT * FROM nope", &all()).await.unwrap_err() else {
        panic!()
    };
    assert_eq!(code.as_deref(), Some("1146"));
    // Exactly one statement.
    assert!(matches!(c.execute("SELECT 1; SELECT 2", &all()).await, Err(Error::Query { .. })));
    assert!(matches!(c.execute("SELECT 1; DROP TABLE t", &all()).await, Err(Error::Query { .. })));
    assert_eq!(scalar(c, "SELECT count(*) FROM t").await, Value::Int(1));
    // A trailing semicolon is fine, and a semicolon inside a literal is not a second statement.
    assert_eq!(scalar(c, "SELECT ';' ;").await, text(";"));
    fx.done().await;
}

#[tokio::test]
async fn connect_failures_map_to_variants() {
    let mut bad = mysql_config();
    bad.password = Some("definitely-wrong".into());
    let e = theta_drivers::connect(&bad).await.err().expect("must fail");
    assert!(matches!(e, Error::Auth(_)), "{e:?}");
    let mut closed = mysql_config();
    closed.port = 1;
    let e = theta_drivers::connect(&closed).await.err().expect("must fail");
    assert!(matches!(e, Error::Connect(_)), "{e:?}");
    let mut nodb = mysql_config();
    nodb.database = "no_such_database_xyz".into();
    let e = theta_drivers::connect(&nodb).await.err().expect("must fail");
    assert!(matches!(e, Error::Connect(_)), "{e:?}");
}

#[tokio::test]
async fn ssl_modes() {
    for mode in [SslMode::Disable, SslMode::Prefer] {
        let mut cfg = mysql_config();
        cfg.ssl_mode = mode;
        let conn = theta_drivers::connect(&cfg).await.unwrap_or_else(|e| panic!("{mode:?}: {e:?}"));
        assert_eq!(scalar(conn.as_ref(), "SELECT 1").await, Value::Int(1));
        conn.close().await.unwrap();
    }
    // Require succeeds iff the server offers TLS.
    let conn = theta_drivers::connect(&mysql_config()).await.unwrap();
    let have_ssl = scalar(conn.as_ref(), "SELECT @@have_ssl").await == text("YES");
    let mut cfg = mysql_config();
    cfg.ssl_mode = SslMode::Require;
    let r = theta_drivers::connect(&cfg).await;
    assert_eq!(r.is_ok(), have_ssl, "{:?}", r.err());
}

#[tokio::test]
async fn cancel_returns_promptly() {
    let fx = MyFx::new().await;
    let db = fx.db.clone();
    let conn: std::sync::Arc<dyn Connection> = fx.conn.into();
    // MariaDB stops a recursive CTE after 1000 iterations by default.
    run(conn.as_ref(), "SET SESSION max_recursive_iterations = 4294967295").await;
    for sql in [
        "SELECT SLEEP(30)",
        "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r) SELECT count(*) FROM r",
        "SELECT SLEEP(30) FROM seq_1_to_3",
    ] {
        let c2 = conn.clone();
        let started = Instant::now();
        let task = tokio::spawn(async move { c2.execute(sql, &ExecOptions::default()).await });
        tokio::time::sleep(Duration::from_millis(400)).await;
        conn.cancel().await.unwrap();
        let r =
            tokio::time::timeout(Duration::from_secs(5), task).await.expect("cancel took too long").unwrap();
        assert!(matches!(r, Err(Error::Cancelled)), "{sql}: {r:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(scalar(conn.as_ref(), "SELECT 7").await, Value::Int(7));
    }
    conn.cancel().await.unwrap();
    assert_eq!(scalar(conn.as_ref(), "SELECT 8").await, Value::Int(8));
    let _ = conn.execute(&format!("DROP DATABASE {db}"), &all()).await;
}

#[tokio::test]
async fn in_transaction_tracks_server_state() {
    let fx = MyFx::new().await;
    let c = fx.conn.as_ref();
    assert!(!c.in_transaction().await);
    run(c, "CREATE TABLE t (a int) ENGINE=InnoDB").await;
    run(c, "START TRANSACTION").await;
    assert!(c.in_transaction().await);
    run(c, "INSERT INTO t VALUES (1)").await;
    assert!(c.in_transaction().await);
    run(c, "ROLLBACK").await;
    assert!(!c.in_transaction().await);
    run(c, "BEGIN").await;
    assert!(c.in_transaction().await);
    run(c, "COMMIT").await;
    assert!(!c.in_transaction().await);
    run(c, "SET autocommit = 0").await;
    run(c, "INSERT INTO t VALUES (2)").await;
    assert!(c.in_transaction().await);
    run(c, "COMMIT").await;
    fx.done().await;
}

#[tokio::test]
async fn statements_run_in_call_order() {
    let fx = MyFx::new().await;
    let c = fx.conn.as_ref();
    let opts = all();
    let (a, b, d) = tokio::join!(
        c.execute("SELECT SLEEP(0.2), @x := 'one'", &opts),
        c.execute("SELECT @x := 'two'", &opts),
        c.execute("SELECT @x", &opts),
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(d.unwrap().result.unwrap().rows[0][0], text("two"));
    fx.done().await;
}

#[tokio::test]
async fn result_columns_know_their_origin() {
    let fx = MyFx::new().await;
    let c = fx.conn.as_ref();
    run(c, "CREATE TABLE people (id int PRIMARY KEY, name varchar(20))").await;
    run(c, "CREATE TABLE pets (id int PRIMARY KEY, owner int, name varchar(20))").await;
    run(c, "INSERT INTO people VALUES (1,'ann')").await;
    run(c, "INSERT INTO pets VALUES (1,1,'rex')").await;
    let cols = run(c, "SELECT p.id AS pid, p.name AS owner_name, t.name, p.id + 1 AS computed, 'x' AS lit FROM people p JOIN pets t ON t.owner = p.id")
        .await
        .result
        .unwrap()
        .columns;
    let o = |table: &str, col: &str| {
        Some(ColumnOrigin { schema: Some(fx.db.clone()), table: table.into(), column: col.into() })
    };
    assert_eq!(cols[0].origin, o("people", "id"));
    assert_eq!(cols[1].origin, o("people", "name"));
    assert_eq!(cols[2].origin, o("pets", "name"));
    assert_eq!(cols[3].origin, None);
    assert_eq!(cols[4].origin, None);
    fx.done().await;
}

#[tokio::test]
async fn introspection_reads_the_whole_catalog() {
    let fx = MyFx::new().await;
    let c = fx.conn.as_ref();
    for sql in [
        "CREATE TABLE parent (a int, b int, label varchar(20) NOT NULL DEFAULT 'x' COMMENT 'A label', PRIMARY KEY (a, b)) COMMENT='The parent table'",
        "CREATE TABLE child (id int AUTO_INCREMENT PRIMARY KEY, pa int, pb int, note varchar(40), amount decimal(10,2), doc json, flag bit(1),
           CONSTRAINT fk_child_parent FOREIGN KEY (pa, pb) REFERENCES parent (a, b) ON DELETE CASCADE ON UPDATE SET NULL)",
        "CREATE UNIQUE INDEX child_note_idx ON child (note)",
        "CREATE INDEX child_multi_idx ON child (amount, note)",
        "CREATE VIEW child_view AS SELECT id, note FROM child",
        "CREATE TABLE places (id int, g polygon NOT NULL, any_g geometry, SPATIAL INDEX (g))",
        "CREATE FUNCTION add_one(x int) RETURNS int DETERMINISTIC RETURN x + 1",
        "CREATE PROCEDURE do_nothing(IN a int, OUT b text) BEGIN SELECT 1; END",
        "CREATE SEQUENCE extra_seq",
        "INSERT INTO parent VALUES (1, 1, 'a')",
        "INSERT INTO child (pa, pb) VALUES (1, 1)",
        "ANALYZE TABLE child",
    ] {
        run(c, sql).await;
    }
    let cat = c.introspect().await.unwrap();
    assert_eq!(cat.default_schema.as_deref(), Some(fx.db.as_str()));
    for sys in ["mysql", "information_schema", "performance_schema", "sys"] {
        assert!(cat.schema(sys).is_none(), "{sys} must be hidden");
    }
    let s = cat.schema(&fx.db).unwrap();
    let t = |n: &str| s.tables.iter().find(|t| t.name == n).unwrap_or_else(|| panic!("table {n}"));
    let parent = t("parent");
    assert_eq!(parent.primary_key, vec!["a", "b"]);
    assert_eq!(parent.comment.as_deref(), Some("The parent table"));
    let label = parent.columns.iter().find(|c| c.name == "label").unwrap();
    assert_eq!(
        (label.data_type.as_str(), label.nullable, label.kind),
        ("varchar(20)", false, TypeKind::Text)
    );
    assert_eq!(label.default.as_deref(), Some("'x'"));
    assert_eq!(label.comment.as_deref(), Some("A label"));
    let child = t("child");
    assert_eq!(child.primary_key, vec!["id"]);
    assert_eq!(child.columns[0].default.as_deref(), Some("AUTO_INCREMENT"));
    assert_eq!(child.columns.iter().find(|c| c.name == "amount").unwrap().kind, TypeKind::Decimal);
    assert_eq!(
        child.columns.iter().find(|c| c.name == "doc").unwrap().kind,
        TypeKind::Json,
        "MariaDB JSON alias detected via its CHECK"
    );
    assert_eq!(child.columns.iter().find(|c| c.name == "flag").unwrap().kind, TypeKind::Integer);
    let fk = &child.foreign_keys[0];
    assert_eq!(fk.name, "fk_child_parent");
    assert_eq!(
        (fk.columns.clone(), fk.referenced_columns.clone()),
        (vec!["pa".into(), "pb".into()], vec!["a".into(), "b".into()])
    );
    assert_eq!(
        (fk.referenced_table.as_str(), fk.referenced_schema.as_deref()),
        ("parent", Some(fx.db.as_str()))
    );
    assert_eq!((fk.on_delete.as_deref(), fk.on_update.as_deref()), (Some("CASCADE"), Some("SET NULL")));
    let pk = child.indexes.iter().find(|i| i.primary).unwrap();
    assert_eq!((pk.name.as_str(), pk.columns.clone(), pk.unique), ("PRIMARY", vec!["id".to_string()], true));
    let uq = child.indexes.iter().find(|i| i.name == "child_note_idx").unwrap();
    assert!(uq.unique && !uq.primary);
    assert_eq!(uq.method.as_deref(), Some("btree"));
    assert!(uq.definition.as_deref().unwrap().starts_with("CREATE UNIQUE INDEX `child_note_idx`"));
    let multi = child.indexes.iter().find(|i| i.name == "child_multi_idx").unwrap();
    assert_eq!(multi.columns, vec!["amount", "note"]);
    assert_eq!(t("child_view").kind, TableKind::View);
    assert_eq!(t("child_view").columns.len(), 2);
    assert_eq!(t("child_view").comment, None);
    assert!(s.sequences.contains(&"extra_seq".to_string()));
    assert!(!s.tables.iter().any(|t| t.name == "extra_seq"));
    let add = s.routines.iter().find(|r| r.name == "add_one").unwrap();
    assert_eq!(
        (add.kind, add.signature.as_str(), add.return_type.as_deref()),
        (RoutineKind::Function, "(IN x int(11))", Some("int(11)"))
    );
    let proc_ = s.routines.iter().find(|r| r.name == "do_nothing").unwrap();
    assert_eq!(
        (proc_.kind, proc_.signature.as_str(), proc_.return_type.clone()),
        (RoutineKind::Procedure, "(IN a int(11), OUT b text)", None)
    );
    let sp = t("places");
    let g = sp.columns.iter().find(|c| c.name == "g").unwrap();
    assert_eq!(g.kind, TypeKind::Geometry);
    assert_eq!(g.geometry.as_ref().unwrap().geometry_type, "POLYGON");
    let any = sp.columns.iter().find(|c| c.name == "any_g").unwrap();
    assert_eq!(any.geometry.as_ref().unwrap().geometry_type, "GEOMETRY");
    fx.done().await;
}

#[tokio::test]
async fn explain_and_analyze() {
    let fx = MyFx::new().await;
    let c = fx.conn.as_ref();
    run(c, "CREATE TABLE t (id int PRIMARY KEY, v int, KEY tv (v)) ENGINE=InnoDB").await;
    run(c, "INSERT INTO t SELECT seq, seq % 10 FROM seq_1_to_1000").await;
    run(c, "ANALYZE TABLE t").await;

    let plan = c.explain("SELECT * FROM t WHERE v = 3 ORDER BY id", false).await.unwrap();
    assert!(!plan.analyzed && plan.raw.contains("query_block"));
    let mut tables = vec![];
    plan.root.walk(&mut |n, _| {
        if let Some(r) = &n.relation {
            tables.push((r.clone(), n.node_type.clone(), n.index.clone(), n.plan_rows));
        }
    });
    assert_eq!(tables.len(), 1, "{tables:?}");
    assert_eq!(tables[0].0, "t");
    assert!(tables[0].3.unwrap() > 0.0);

    let plan = c.explain("SELECT * FROM t WHERE v < 5", true).await.unwrap();
    assert!(plan.analyzed);
    assert!(plan.execution_time_ms.unwrap() >= 0.0);
    let mut analysed_rows = None;
    plan.root.walk(&mut |n, _| {
        if n.relation.as_deref() == Some("t") {
            analysed_rows = n.actual_rows;
        }
    });
    assert_eq!(analysed_rows, Some(500.0));

    // ANALYZE executes the statement; it must leave no trace.
    c.explain("DELETE FROM t WHERE v = 1", true).await.unwrap();
    c.explain("UPDATE t SET v = 99", true).await.unwrap();
    assert_eq!(scalar(c, "SELECT count(*) FROM t").await, Value::Int(1000));
    assert_eq!(scalar(c, "SELECT count(*) FROM t WHERE v = 99").await, Value::Int(0));
    assert!(!c.in_transaction().await);
    // ...and must not disturb a user's transaction.
    run(c, "START TRANSACTION").await;
    run(c, "INSERT INTO t VALUES (5000, 1)").await;
    c.explain("DELETE FROM t WHERE id = 5000", true).await.unwrap();
    assert!(c.in_transaction().await);
    assert_eq!(scalar(c, "SELECT count(*) FROM t WHERE id = 5000").await, Value::Int(1));
    run(c, "ROLLBACK").await;
    assert_eq!(scalar(c, "SELECT count(*) FROM t WHERE id = 5000").await, Value::Int(0));
    assert!(c.explain("SELECT * FROM nope", false).await.is_err());
    fx.done().await;
}

#[tokio::test]
async fn demo_seed_builds_a_consistent_dataset() {
    let fx = MyFx::new().await;
    let c = fx.conn.as_ref();
    theta_drivers::demo::seed(c).await.unwrap();
    theta_drivers::demo::seed(c).await.unwrap();
    assert_eq!(scalar(c, "SELECT count(*) FROM customers").await, Value::Int(200));
    assert_eq!(scalar(c, "SELECT count(*) FROM products").await, Value::Int(40));
    assert_eq!(scalar(c, "SELECT count(*) FROM orders").await, Value::Int(600));
    assert_eq!(scalar(c, "SELECT count(*) FROM stores").await, Value::Int(25));
    assert_eq!(
        scalar(c, "SELECT count(*) FROM orders o WHERE o.total <> order_total(o.id)").await,
        Value::Int(0)
    );
    assert_eq!(scalar(c, "SELECT count(*) FROM stores WHERE ST_SRID(location) = 4326").await, Value::Int(25));
    assert_eq!(
        scalar(c, "SELECT count(*) FROM stores s JOIN delivery_zones z ON z.store_id = s.id WHERE ST_Contains(z.area, s.location)").await,
        Value::Int(25)
    );
    let rs = run(c, "SELECT location FROM stores ORDER BY id LIMIT 1").await.result.unwrap();
    assert!(matches!(rs.rows[0][0], Value::Geometry(_)));
    let cat = c.introspect().await.unwrap();
    let s = cat.schema(&fx.db).unwrap();
    let orders = s.tables.iter().find(|t| t.name == "orders").unwrap();
    assert_eq!(orders.comment.as_deref(), Some("Customer orders"));
    assert_eq!(orders.foreign_keys.len(), 1);
    let items = s.tables.iter().find(|t| t.name == "order_items").unwrap();
    assert_eq!(items.primary_key, vec!["order_id", "line_no"]);
    assert!(s.tables.iter().any(|t| t.name == "customer_order_summary" && t.kind == TableKind::View));
    assert!(s.routines.iter().any(|r| r.name == "order_total"));
    fx.done().await;
}

#[tokio::test]
async fn close_then_use_reports_closed() {
    let fx = MyFx::new().await;
    let db = fx.db.clone();
    let c = fx.conn;
    let _ = c.execute(&format!("DROP DATABASE {db}"), &all()).await;
    c.close().await.unwrap();
    assert!(matches!(c.execute("SELECT 1", &all()).await, Err(Error::Closed)));
}

/// `KILL CONNECTION_ID()` ends the session under the statement; the first failure must already
/// classify as a lost connection so the host can reconnect and re-run.
#[tokio::test]
async fn killing_the_own_connection_is_reported_as_connection_lost() {
    let conn = theta_drivers::connect(&mysql_config()).await.expect("connect to MariaDB");
    let err = conn.execute("KILL CONNECTION_ID()", &all()).await;
    // Some servers acknowledge the KILL of the own connection and drop the socket afterwards.
    if let Err(e) = &err {
        assert!(e.is_connection_lost(), "first failure must be a session loss: {e:?}");
    }
    let next = conn.execute("select 1", &all()).await.expect_err("the session is gone");
    assert!(next.is_connection_lost(), "{next:?}");
    let fresh = theta_drivers::connect(&mysql_config()).await.expect("reconnect");
    let syntax = fresh.execute("selec 1", &all()).await.expect_err("syntax error");
    assert!(!syntax.is_connection_lost(), "{syntax:?}");
    assert_eq!(scalar(fresh.as_ref(), "select 1").await, Value::Int(1));
    let _ = fresh.close().await;
}

#[tokio::test]
async fn a_connection_killed_by_another_session_is_reported_as_connection_lost() {
    let victim = theta_drivers::connect(&mysql_config()).await.expect("connect");
    let admin = theta_drivers::connect(&mysql_config()).await.expect("connect");
    let id = match scalar(victim.as_ref(), "select connection_id()").await {
        Value::Int(i) => i.to_string(),
        Value::UInt(u) => u.to_string(),
        other => panic!("connection id is an integer, got {other:?}"),
    };
    run(admin.as_ref(), &format!("KILL CONNECTION {id}")).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let err = victim.execute("select 1", &all()).await.expect_err("killed");
    assert!(err.is_connection_lost(), "{err:?}");
    let _ = admin.close().await;
}

/// MySQL/MariaDB (`@@tx_isolation` is the variable MariaDB 10 has): `SET TRANSACTION ISOLATION LEVEL` + `START TRANSACTION`, and the implicit commit of DDL is
/// visible through `in_transaction`.
#[tokio::test]
async fn manual_transactions_start_with_the_configured_isolation() {
    let c = theta_drivers::connect(&mysql_config()).await.expect("connect to MySQL");
    let c = c.as_ref();
    // `SET TRANSACTION` is not visible in `@@tx_isolation`; InnoDB reports the level of the running
    // transaction once it has touched a table.
    let probe = unique("txn_iso");
    run(c, &format!("CREATE TABLE {probe} (a int) ENGINE=InnoDB")).await;
    for (level, expected) in [
        (Isolation::ReadUncommitted, "READ UNCOMMITTED"),
        (Isolation::ReadCommitted, "READ COMMITTED"),
        (Isolation::RepeatableRead, "REPEATABLE READ"),
        (Isolation::Serializable, "SERIALIZABLE"),
    ] {
        let cfg = TransactionConfig { manual: true, isolation: level, ..Default::default() };
        for sql in cfg.begin_statements(DatabaseKind::MySql, false) {
            run(c, &sql).await;
        }
        assert!(c.in_transaction().await, "{level:?}");
        run(c, &format!("SELECT COUNT(*) FROM {probe}")).await;
        // information_schema.innodb_trx is served from a cache that refreshes every 100 ms.
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(
            scalar(
                c,
                "SELECT trx_isolation_level FROM information_schema.innodb_trx WHERE trx_mysql_thread_id = CONNECTION_ID()"
            )
            .await,
            text(expected)
        );
        run(c, "ROLLBACK").await;
        assert!(!c.in_transaction().await);
    }
    // The level applied to that one transaction only: the session default is back afterwards.
    assert_eq!(scalar(c, "SELECT @@tx_isolation").await, text("REPEATABLE-READ"));
    run(c, &format!("DROP TABLE {probe}")).await;

    // DDL commits implicitly: the app must be able to see the transaction is gone.
    let table = unique("txn_ddl");
    let cfg = TransactionConfig { manual: true, ..Default::default() };
    for sql in cfg.begin_statements(DatabaseKind::MySql, false) {
        run(c, &sql).await;
    }
    assert!(c.in_transaction().await);
    run(c, &format!("CREATE TABLE {table} (a int)")).await;
    assert!(!c.in_transaction().await, "CREATE TABLE ended the transaction");
    run(c, &format!("DROP TABLE {table}")).await;

    // Read-only transactions refuse writes.
    let t2 = unique("txn_ro");
    run(c, &format!("CREATE TABLE {t2} (a int)")).await;
    for sql in cfg.begin_statements(DatabaseKind::MySql, true) {
        run(c, &sql).await;
    }
    let err = c.execute(&format!("INSERT INTO {t2} VALUES (1)"), &all()).await.unwrap_err();
    assert!(err.to_string().to_lowercase().contains("read only"), "{err}");
    run(c, "ROLLBACK").await;
    run(c, &format!("DROP TABLE {t2}")).await;
}

/// The four everyday connect failures read as plain sentences, with the original text kept as details.
#[tokio::test]
async fn connect_failures_are_worded_for_people() {
    use theta_core::friendly::connect_error;
    let base = mysql_config();
    let say = |e: &Error, c: &ConnectionConfig| connect_error(e, Some(c));

    let mut wrong_port = base.clone();
    wrong_port.port = 5999;
    let e = theta_drivers::connect(&wrong_port).await.err().expect("must fail");
    let x = say(&e, &wrong_port);
    assert_eq!(x.message, "Nothing is listening on 127.0.0.1:5999. Is the server running, and is the port right?");
    assert!(x.details.as_deref().is_some_and(|d| d.contains("os error 111")), "{x:?}");

    let mut unknown_host = base.clone();
    unknown_host.host = "no-such-host.invalid".into();
    let e = theta_drivers::connect(&unknown_host).await.err().expect("must fail");
    assert_eq!(
        say(&e, &unknown_host).message,
        "The host no-such-host.invalid could not be found. Check the name or your VPN."
    );

    let mut wrong_password = base.clone();
    wrong_password.password = Some("definitely-wrong".into());
    let e = theta_drivers::connect(&wrong_password).await.err().expect("must fail");
    assert_eq!(say(&e, &wrong_password).message, "The server rejected the password for user quarry.");

    let mut no_db = base.clone();
    no_db.database = "nodb_x".into();
    let e = theta_drivers::connect(&no_db).await.err().expect("must fail");
    let m = say(&e, &no_db).message;
    assert!(m.starts_with("There is no database named nodb_x on 127.0.0.1:3306."), "{m}");
}
