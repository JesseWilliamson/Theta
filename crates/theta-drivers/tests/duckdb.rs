//! DuckDB specifics that the generic run in `kinds_live.rs` does not cover: nested and exact values,
//! files read through plain SQL, row counts, transactions, and the guarantee that nothing is downloaded.
#![cfg(feature = "duckdb")]

mod common;

use common::*;
use theta_core::*;

async fn memory() -> Box<dyn Connection> {
    let mut c = ConnectionConfig::new("t-duck", "duck", DatabaseKind::DuckDb);
    c.path = ":memory:".into();
    theta_drivers::connect(&c).await.expect("open DuckDB")
}

#[tokio::test]
async fn values_round_trip() {
    let c = memory().await;
    let out = run(
        c.as_ref(),
        "SELECT [1,2,3] AS l, {'a': 1, 'b': 'x'} AS s, MAP {'k': 1} AS m, \
                170141183460469231731687303715884105727::HUGEINT AS h, 12345.6789::DECIMAL(18,4) AS d, \
                INTERVAL '1 year 2 months 3 days 4 hours' AS iv, TIMESTAMPTZ '2024-02-29 13:14:15+02' AS tz, \
                TIME '13:14:15.5' AS t, TIMESTAMP '2024-02-29 13:14:15.123456' AS ts, DATE '0044-03-15' AS bc, \
                0.1::FLOAT AS f, 'ab'::BLOB AS b, [{'x': [1, NULL]}] AS nested, NULL::INTEGER AS n, \
                '123e4567-e89b-12d3-a456-426614174000'::UUID AS u, 255::UTINYINT AS ut, 18446744073709551615::UBIGINT AS ub",
    )
    .await;
    let rs = out.result.unwrap();
    let get = |n: &str| rs.rows[0][rs.columns.iter().position(|c| c.name == n).unwrap()].clone();
    assert_eq!(get("l"), Value::Json("[1,2,3]".into()));
    assert_eq!(get("s"), Value::Json("{\"a\":1,\"b\":\"x\"}".into()));
    assert_eq!(get("m"), Value::Json("{\"k\":1}".into()));
    assert_eq!(get("h"), Value::Decimal("170141183460469231731687303715884105727".into()));
    assert_eq!(get("d"), Value::Decimal("12345.6789".into()));
    assert_eq!(get("iv"), Value::Interval("1 year 2 months 3 days 04:00:00".into()));
    assert_eq!(get("tz"), Value::TimestampTz("2024-02-29 11:14:15+00".into()));
    assert_eq!(get("t"), Value::Time("13:14:15.5".into()));
    assert_eq!(get("ts"), Value::Timestamp("2024-02-29 13:14:15.123456".into()));
    assert_eq!(get("f"), Value::Float(0.1));
    assert_eq!(get("b"), Value::Bytes(b"ab".to_vec()));
    assert_eq!(get("nested"), Value::Json("[{\"x\":[1,null]}]".into()));
    assert_eq!(get("n"), Value::Null);
    assert_eq!(get("u"), Value::Uuid("123e4567-e89b-12d3-a456-426614174000".into()));
    assert_eq!(get("ut"), Value::Int(255));
    assert_eq!(get("ub"), Value::UInt(u64::MAX));
    assert!(get("bc") != Value::Null);
    let types: Vec<&str> = rs.columns.iter().map(|c| c.type_name.as_str()).collect();
    assert!(
        types.contains(&"INTEGER[]")
            && types.contains(&"STRUCT(a INTEGER, b VARCHAR)")
            && types.contains(&"DECIMAL(18,4)"),
        "{types:?}"
    );
}

#[tokio::test]
async fn geometry_comes_through_as_wkb() {
    let c = memory().await;
    // GEOMETRY is a core type in the bundled DuckDB; the spatial extension's ST_ functions are not available.
    let v = scalar(c.as_ref(), "SELECT 'POINT(1 2)'::GEOMETRY").await;
    let Value::Geometry(wkb) = v else { panic!("expected geometry, got {v:?}") };
    assert_eq!(wkb.len(), 21);
    assert_eq!(wkb[0], 1, "little-endian WKB point");
}

#[tokio::test]
async fn files_are_read_through_plain_sql() {
    let c = memory().await;
    let dir = std::env::temp_dir().join(unique("duck-files"));
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("people.csv");
    std::fs::write(&csv, "id,name,score\n1,ada,10.5\n2,bob,20\n").unwrap();
    let parquet = dir.join("people.parquet");
    run(
        c.as_ref(),
        &format!("COPY (SELECT * FROM '{}') TO '{}' (FORMAT parquet)", csv.display(), parquet.display()),
    )
    .await;
    let rows =
        rows(c.as_ref(), &format!("SELECT id, name, score FROM '{}' ORDER BY id", parquet.display())).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][1], text("ada"));
    let csv_rows = rows_of(c.as_ref(), &format!("SELECT count(*) FROM read_csv('{}')", csv.display())).await;
    assert_eq!(csv_rows, Value::Int(2));
    let _ = std::fs::remove_dir_all(&dir);
}

async fn rows_of(c: &dyn Connection, sql: &str) -> Value {
    scalar(c, sql).await
}

#[tokio::test]
async fn row_counts_and_transactions() {
    let c = memory().await;
    run(c.as_ref(), "CREATE TABLE t (a INTEGER)").await;
    let ins = run(c.as_ref(), "INSERT INTO t SELECT * FROM range(5)").await;
    assert_eq!(ins.rows_affected, Some(5));
    assert!(ins.result.is_none());
    assert!(!c.in_transaction().await);
    run(c.as_ref(), "BEGIN").await;
    assert!(c.in_transaction().await);
    let upd = run(c.as_ref(), "UPDATE t SET a = a + 1 WHERE a < 2").await;
    assert_eq!(upd.rows_affected, Some(2));
    run(c.as_ref(), "ROLLBACK").await;
    assert!(!c.in_transaction().await);
    assert_eq!(scalar(c.as_ref(), "SELECT max(a) FROM t").await, Value::Int(4));
    // EXPLAIN ANALYZE of a write inside the user's transaction is refused rather than keeping its changes.
    run(c.as_ref(), "BEGIN").await;
    assert!(matches!(c.explain("DELETE FROM t", true).await, Err(Error::Unsupported(_))));
    run(c.as_ref(), "ROLLBACK").await;
    // outside one it runs and leaves nothing behind
    c.explain("DELETE FROM t", true).await.unwrap();
    assert_eq!(scalar(c.as_ref(), "SELECT count(*) FROM t").await, Value::Int(5));
}

#[tokio::test]
async fn errors_carry_class_and_position_and_nothing_is_downloaded() {
    let c = memory().await;
    match c.execute("SELEC 1", &all()).await {
        Err(Error::Query { code, position, .. }) => {
            assert_eq!(code.as_deref(), Some("Parser Error"));
            assert_eq!(position, Some(ErrorPosition { offset: 0 }));
        }
        other => panic!("expected a parser error, got {other:?}"),
    }
    // A remote path needs the httpfs extension. Auto-installation is off, so DuckDB must say the extension
    // is not available instead of fetching it.
    let started = std::time::Instant::now();
    let r = c.execute("SELECT * FROM 'https://example.invalid/x.parquet'", &all()).await;
    assert!(r.is_err());
    assert!(started.elapsed().as_secs() < 20, "should fail fast, not attempt a download");
    if let Err(Error::Query { message, .. }) = r {
        eprintln!("remote path error: {message}");
    }
}
