//! ClickHouse over HTTP against a real server (`THETA_TEST_CLICKHOUSE_URL`, e.g.
//! `clickhouse://default@127.0.0.1:8123/default`); skipped with a message when the variable is unset.
//!
//! The tests are written to hold on every server version: a type or function the server does not know is
//! reported as skipped (printed), not failed, so they run both on the 2018 server that Ubuntu packages and
//! on the current release CI uses. Which one ran is printed with the version.

mod common;

use common::*;
use std::sync::Arc;
use std::time::{Duration, Instant};
use theta_core::*;

async fn connect() -> Option<Arc<dyn Connection>> {
    let c = env_config("THETA_TEST_CLICKHOUSE_URL", DatabaseKind::ClickHouse)?;
    let conn = theta_drivers::connect(&c).await.expect("connect to ClickHouse");
    Some(Arc::from(conn))
}

fn major(info: &ServerInfo) -> u32 {
    info.version.trim_start_matches("ClickHouse ").split('.').next().and_then(|m| m.parse().ok()).unwrap_or(0)
}

#[tokio::test]
async fn connect_info_and_error_shape() {
    let Some(c) = connect().await else { return };
    let info = c.server_info().await.unwrap();
    eprintln!("ClickHouse server: {} (db {:?})", info.version, info.current_database);
    assert!(info.version.starts_with("ClickHouse "));
    assert_eq!(DatabaseKind::ClickHouse.mismatch(&info.banner), None);
    match c.execute("SELEC 1", &all()).await {
        Err(Error::Query { code, position, message, .. }) => {
            assert_eq!(code.as_deref(), Some("62"), "{message}");
            assert_eq!(position, Some(ErrorPosition { offset: 0 }));
        }
        other => panic!("expected a syntax error, got {other:?}"),
    }
    // The session survives an error.
    assert_eq!(scalar(c.as_ref(), "SELECT 1").await, Value::Int(1));
    // A wrong password is an authentication error.
    let mut bad = env_config("THETA_TEST_CLICKHOUSE_URL", DatabaseKind::ClickHouse).unwrap();
    bad.user = "nobody_here".into();
    bad.password = Some("wrong".into());
    assert!(matches!(theta_drivers::connect(&bad).await, Err(Error::Auth(_) | Error::Connect(_))));
}

#[tokio::test]
async fn typed_values_round_trip() {
    let Some(c) = connect().await else { return };
    // (expression, expected display text, expected type word)
    let cases: &[(&str, &str, &str)] = &[
        ("toInt8(-5)", "-5", "Int8"),
        ("toUInt64(18446744073709551615)", "18446744073709551615", "UInt64"),
        ("toFloat64(2.5)", "2.5", "Float64"),
        ("toDecimal64(2.50, 2)", "2.50", "Decimal"),
        ("'héllo\\tworld'", "héllo\tworld", "String"),
        ("toFixedString('ab', 4)", "ab\0\0", "FixedString(4)"),
        ("toDate('2024-02-29')", "2024-02-29", "Date"),
        ("toDateTime('2024-02-29 13:14:15', 'UTC')", "2024-02-29 13:14:15", "DateTime"),
        ("toUUID('123e4567-e89b-12d3-a456-426614174000')", "123e4567-e89b-12d3-a456-426614174000", "UUID"),
        ("CAST(NULL AS Nullable(Int32))", "", "Nullable(Int32)"),
        ("CAST(7 AS Nullable(Int32))", "7", "Nullable(Int32)"),
        ("[1, 2, 3]", "{1, 2, 3}", "Array(UInt8)"),
        ("['a', 'b,c']", "{a, b,c}", "Array(String)"),
        ("(1, 'x')", "[1,\"x\"]", "Tuple(UInt8, String)"),
        (
            "toDateTime64('2024-02-29 13:14:15.123', 3, 'UTC')",
            "2024-02-29 13:14:15.123",
            "DateTime64(3, 'UTC')",
        ),
        ("toIPv4('10.0.0.1')", "10.0.0.1", "IPv4"),
        ("toIPv6('::1')", "::1", "IPv6"),
        ("map('a', 1, 'b', 2)", "{\"a\":1,\"b\":2}", "Map(String, UInt8)"),
        ("CAST('x' AS LowCardinality(String))", "x", "LowCardinality(String)"),
    ];
    let mut skipped = Vec::new();
    for (expr, want, ty) in cases {
        match c.execute(&format!("SELECT {expr} AS v"), &all()).await {
            Ok(out) => {
                let rs = out.result.expect("result");
                let got = rs.rows[0][0].display();
                assert_eq!(got, *want, "{expr} -> {:?}", rs.rows[0][0]);
                assert!(
                    rs.columns[0].type_name.starts_with(ty.split('(').next().unwrap()),
                    "{expr}: {}",
                    rs.columns[0].type_name
                );
            }
            Err(Error::Query { message, .. }) => skipped.push(format!("{expr}: {message}")),
            Err(e) => panic!("{expr}: {e:?}"),
        }
    }
    eprintln!("typed values skipped on this server version ({}):\n  {}", skipped.len(), skipped.join("\n  "));
    // The basics must exist on every version.
    assert!(skipped.len() < cases.len() / 2, "too many unsupported: {skipped:?}");
}

#[tokio::test]
async fn introspection_shows_engine_and_sorting_key() {
    let Some(c) = connect().await else { return };
    let t = unique("thc");
    let create = format!(
        "CREATE TABLE {t} (id UInt64, name String, ts DateTime, tag Nullable(String)) ENGINE = MergeTree ORDER BY id"
    );
    if let Err(e) = c.execute(&create, &all()).await {
        eprintln!("SKIPPED MergeTree ORDER BY on this version: {e:?}");
        return;
    }
    let view = format!("{t}_v");
    run(c.as_ref(), &format!("CREATE VIEW {view} AS SELECT id, name FROM {t}")).await;
    run(c.as_ref(), &format!("INSERT INTO {t} (id, name, ts) VALUES (1, 'a', '2024-01-01 00:00:00'), (2, 'b', '2024-01-02 00:00:00'), (3, 'c', '2024-01-03 00:00:00')")).await;
    let cat = c.introspect().await.expect("introspect");
    let table = cat.schemas.iter().flat_map(|s| &s.tables).find(|x| x.name == t).expect("table listed");
    let option = |n: &str| table.options.iter().find(|o| o.name == n).map(|o| o.value.clone());
    assert_eq!(option("ENGINE").as_deref(), Some("MergeTree"));
    assert_eq!(option("ORDER BY").as_deref(), Some("id"));
    // A ClickHouse primary key is not unique, so it is never offered as the row identity.
    assert!(table.primary_key.is_empty());
    assert_eq!(table.columns.len(), 4);
    assert!(table.columns.iter().find(|x| x.name == "tag").unwrap().nullable);
    assert_eq!(table.columns[0].data_type, "UInt64");
    let v = cat.schemas.iter().flat_map(|s| &s.tables).find(|x| x.name == view).expect("view listed");
    assert_eq!(v.kind, TableKind::View);
    // Rows come back typed and the grid layer is told they cannot be edited.
    let rs = run(c.as_ref(), &format!("SELECT id, name FROM {t} ORDER BY id")).await.result.unwrap();
    assert_eq!(rs.rows.len(), 3);
    assert!(DatabaseKind::ClickHouse.unsupported(Feature::EditRows).is_some());
    run(c.as_ref(), &format!("DROP TABLE {view}")).await;
    run(c.as_ref(), &format!("DROP TABLE {t}")).await;
}

#[tokio::test]
async fn row_limit_cancel_and_explain() {
    let Some(c) = connect().await else { return };
    let info = c.server_info().await.unwrap();
    // row limit: more rows exist than asked for
    let out =
        c.execute("SELECT number FROM numbers(100000)", &ExecOptions { row_limit: Some(10) }).await.unwrap();
    let rs = out.result.unwrap();
    assert_eq!((rs.rows.len(), rs.truncated), (10, true));
    let out = c.execute("SELECT number FROM numbers(5)", &ExecOptions { row_limit: Some(10) }).await.unwrap();
    assert!(!out.result.unwrap().truncated);

    // cancel a statement that would run for a long time
    let c2 = c.clone();
    let running = tokio::spawn(async move {
        c2.execute("SELECT sum(cityHash64(number)) FROM numbers(100000000000)", &all()).await
    });
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let started = Instant::now();
    c.cancel().await.expect("cancel");
    let r = tokio::time::timeout(Duration::from_secs(30), running)
        .await
        .expect("cancel did not stop the query")
        .unwrap();
    assert!(matches!(r, Err(Error::Cancelled)), "expected Cancelled, got {r:?}");
    assert!(started.elapsed() < Duration::from_secs(20));
    assert_eq!(scalar(c.as_ref(), "SELECT 1").await, Value::Int(1), "usable after a cancel");

    // EXPLAIN: JSON plans exist from 21.x; ANALYZE never
    assert!(matches!(c.explain("SELECT 1", true).await, Err(Error::Unsupported(_))));
    match c.explain("SELECT number FROM numbers(10) WHERE number > 3", false).await {
        Ok(plan) => {
            assert!(!plan.analyzed && !plan.root.node_type.is_empty(), "{}", plan.raw);
            eprintln!("EXPLAIN root: {}", plan.root.node_type);
        }
        Err(e) => {
            assert!(major(&info) < 22, "EXPLAIN should work on {}: {e:?}", info.version);
            eprintln!("EXPLAIN not available on {}: {e}", info.version);
        }
    }
}

#[tokio::test]
async fn session_settings_persist() {
    let Some(c) = connect().await else { return };
    // A `SET` must still apply to the next statement: the connection is one HTTP session.
    run(c.as_ref(), "SET max_threads = 3").await;
    let v = scalar(c.as_ref(), "SELECT value FROM system.settings WHERE name = 'max_threads'").await;
    assert_eq!(v.display(), "3");
}
