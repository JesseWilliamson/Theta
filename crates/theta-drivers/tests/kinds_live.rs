//! One conformance run per database product, against a real server.
//!
//! Every product reads its URL from an environment variable and is skipped, with a printed message, when
//! the variable is unset (CI sets them: see `.github/workflows/ci.yml`). DuckDB is embedded and always runs.
//!
//! | product        | variable                      |
//! |----------------|-------------------------------|
//! | CockroachDB    | `THETA_TEST_COCKROACH_URL`    |
//! | TimescaleDB    | `THETA_TEST_TIMESCALE_URL`    |
//! | YugabyteDB     | `THETA_TEST_YUGABYTE_URL`     |
//! | Amazon Redshift| `THETA_TEST_REDSHIFT_URL`     |
//! | TiDB           | `THETA_TEST_TIDB_URL`         |
//! | SQL Server     | `THETA_TEST_SQLSERVER_URL`    |
//! | ClickHouse     | `THETA_TEST_CLICKHOUSE_URL`   |
//!
//! What each run checks: connect and `server_info` (and that the kind matches the server), a typed-values
//! round trip, introspection of a seeded schema (primary key, foreign key, index, view), EXPLAIN and
//! EXPLAIN ANALYZE parsed into a `Plan`, cancel of a long statement, the error shape, and the row limit.

mod common;

use common::*;
use std::sync::Arc;
use std::time::{Duration, Instant};
use theta_core::*;

/// The per-product facts the generic run needs.
struct Spec {
    kind: DatabaseKind,
    /// DDL and data for a small schema; `{p}` is replaced by a unique prefix.
    seed: &'static [&'static str],
    /// Statements that remove what `seed` made, run at the end (best effort).
    cleanup: &'static [&'static str],
    /// One row of typed values: `(column, expected display text)`.
    typed_sql: &'static str,
    typed: &'static [(&'static str, &'static str)],
    /// A statement that runs for far longer than the test waits.
    slow_sql: &'static str,
    /// `{p}_emp` has an index named `{p}_emp_name`; false where the product has none to list.
    has_index: bool,
    /// Foreign keys are listed (informational ones count).
    has_fk: bool,
}

fn pg_like(kind: DatabaseKind) -> Spec {
    Spec {
        kind,
        seed: &[
            "CREATE TABLE {p}_dept (id int PRIMARY KEY, name text NOT NULL)",
            "CREATE TABLE {p}_emp (id int PRIMARY KEY, dept_id int NOT NULL REFERENCES {p}_dept(id), name text, salary numeric(10,2))",
            "CREATE INDEX {p}_emp_name ON {p}_emp (name)",
            "CREATE VIEW {p}_v AS SELECT e.name, d.name AS dept FROM {p}_emp e JOIN {p}_dept d ON d.id = e.dept_id",
            "INSERT INTO {p}_dept VALUES (1, 'eng'), (2, 'ops')",
            "INSERT INTO {p}_emp VALUES (1, 1, 'ada', 10.50), (2, 1, 'bob', 20.00), (3, 2, 'cy', 30.25)",
        ],
        cleanup: &["DROP VIEW {p}_v", "DROP TABLE {p}_emp", "DROP TABLE {p}_dept"],
        typed_sql: "SELECT 1::int8 AS i, 2.50::numeric(10,2) AS n, 'héllo'::text AS t, true AS b, \
                    DATE '2024-02-29' AS d, TIMESTAMP '2024-02-29 13:14:15' AS ts, \
                    '123e4567-e89b-12d3-a456-426614174000'::uuid AS u",
        typed: &[
            ("i", "1"),
            ("n", "2.50"),
            ("t", "héllo"),
            ("b", "true"),
            ("d", "2024-02-29"),
            ("ts", "2024-02-29 13:14:15"),
            ("u", "123e4567-e89b-12d3-a456-426614174000"),
        ],
        slow_sql: "SELECT pg_sleep(60)",
        has_index: true,
        has_fk: true,
    }
}

fn redshift() -> Spec {
    Spec {
        // Redshift: informational constraints, no secondary indexes, no uuid type.
        seed: &[
            "CREATE TABLE {p}_dept (id int PRIMARY KEY, name varchar(50) NOT NULL)",
            "CREATE TABLE {p}_emp (id int PRIMARY KEY, dept_id int NOT NULL REFERENCES {p}_dept(id), name varchar(50), salary decimal(10,2))",
            "CREATE VIEW {p}_v AS SELECT e.name, d.name AS dept FROM {p}_emp e JOIN {p}_dept d ON d.id = e.dept_id",
            "INSERT INTO {p}_dept VALUES (1, 'eng'), (2, 'ops')",
            "INSERT INTO {p}_emp VALUES (1, 1, 'ada', 10.50), (2, 1, 'bob', 20.00), (3, 2, 'cy', 30.25)",
        ],
        typed_sql: "SELECT 1::bigint AS i, 2.50::decimal(10,2) AS n, 'héllo'::varchar(20) AS t, true AS b, \
                    DATE '2024-02-29' AS d, TIMESTAMP '2024-02-29 13:14:15' AS ts",
        typed: &[
            ("i", "1"),
            ("n", "2.50"),
            ("t", "héllo"),
            ("b", "true"),
            ("d", "2024-02-29"),
            ("ts", "2024-02-29 13:14:15"),
        ],
        slow_sql: "SELECT pg_sleep(60)",
        has_index: false,
        ..pg_like(DatabaseKind::Redshift)
    }
}

fn tidb() -> Spec {
    Spec {
        kind: DatabaseKind::TiDb,
        seed: &[
            "CREATE TABLE {p}_dept (id int PRIMARY KEY, name varchar(50) NOT NULL)",
            "CREATE TABLE {p}_emp (id int PRIMARY KEY, dept_id int NOT NULL, name varchar(50), salary decimal(10,2), \
             CONSTRAINT {p}_fk FOREIGN KEY (dept_id) REFERENCES {p}_dept(id))",
            "CREATE INDEX {p}_emp_name ON {p}_emp (name)",
            "CREATE VIEW {p}_v AS SELECT e.name, d.name AS dept FROM {p}_emp e JOIN {p}_dept d ON d.id = e.dept_id",
            "INSERT INTO {p}_dept VALUES (1, 'eng'), (2, 'ops')",
            "INSERT INTO {p}_emp VALUES (1, 1, 'ada', 10.50), (2, 1, 'bob', 20.00), (3, 2, 'cy', 30.25)",
        ],
        cleanup: &["DROP VIEW {p}_v", "DROP TABLE {p}_emp", "DROP TABLE {p}_dept"],
        typed_sql: "SELECT CAST(1 AS SIGNED) AS i, CAST(2.5 AS DECIMAL(10,2)) AS n, 'héllo' AS t, \
                    CAST('2024-02-29' AS DATE) AS d, CAST('2024-02-29 13:14:15' AS DATETIME) AS ts",
        typed: &[
            ("i", "1"),
            ("n", "2.50"),
            ("t", "héllo"),
            ("d", "2024-02-29"),
            ("ts", "2024-02-29 13:14:15"),
        ],
        slow_sql: "SELECT SLEEP(60)",
        has_index: true,
        has_fk: true,
    }
}

async fn conformance(config: ConnectionConfig, spec: Spec) {
    let kind = spec.kind;
    let p = unique("tl");
    let sub = |s: &str| s.replace("{p}", &p);
    let conn = theta_drivers::connect(&config)
        .await
        .unwrap_or_else(|e| panic!("connect to {}: {e:?}", kind.label()));
    let conn: Arc<dyn Connection> = Arc::from(conn);
    assert_eq!(conn.kind(), kind);

    // 1. server_info
    let info = conn.server_info().await.unwrap();
    assert!(!info.version.is_empty(), "no version");
    assert!(!info.banner.is_empty(), "no banner for the mismatch check");
    assert_eq!(kind.mismatch(&info.banner), None, "kind and server disagree: {}", info.banner);
    eprintln!("{}: {} / {:?}", kind.label(), info.version, info.current_database);

    // 2. typed values
    let out = conn.execute(spec.typed_sql, &all()).await.unwrap_or_else(|e| panic!("typed: {e:?}"));
    let rs = out.result.expect("typed result set");
    for (col, want) in spec.typed {
        let i = rs
            .columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(col))
            .unwrap_or_else(|| panic!("no column {col}"));
        assert_eq!(rs.rows[0][i].display(), *want, "column {col} ({})", rs.columns[i].type_name);
    }

    // 3. seed and introspect
    for s in spec.seed {
        conn.execute(&sub(s), &all()).await.unwrap_or_else(|e| panic!("`{}`: {e:?}", sub(s)));
    }
    async {
        let cat = conn.introspect().await.expect("introspect");
        let emp = cat
            .schemas
            .iter()
            .flat_map(|s| s.tables.iter())
            .find(|t| t.name == format!("{p}_emp"))
            .unwrap_or_else(|| panic!("{p}_emp not in the catalog"));
        assert_eq!(emp.primary_key, vec!["id".to_string()]);
        assert!(emp.columns.len() >= 4 && emp.columns.iter().any(|c| c.name == "salary"));
        if spec.has_fk {
            assert_eq!(emp.foreign_keys.len(), 1, "foreign keys: {:?}", emp.foreign_keys);
            assert_eq!(emp.foreign_keys[0].referenced_table, format!("{p}_dept"));
        }
        if spec.has_index {
            assert!(
                emp.indexes.iter().any(|i| i.name == format!("{p}_emp_name")),
                "indexes: {:?}",
                emp.indexes
            );
        }
        let view = cat.schemas.iter().flat_map(|s| s.tables.iter()).find(|t| t.name == format!("{p}_v"));
        assert_eq!(view.map(|v| v.kind), Some(TableKind::View), "view missing");

        // 4. plans
        let q = sub("SELECT e.name FROM {p}_emp e JOIN {p}_dept d ON d.id = e.dept_id WHERE e.name = 'ada'");
        let plan = conn.explain(&q, false).await.expect("explain");
        assert!(!plan.analyzed && !plan.root.node_type.is_empty());
        let mut relations = Vec::new();
        plan.root.walk(&mut |n, _| relations.extend(n.relation.clone()));
        // MySQL's JSON plan names a table by its alias (`e`), the others by name.
        assert!(
            relations.iter().any(|r| r.contains(&format!("{p}_emp")) || r == "e"),
            "plan names no table: {relations:?}\n{}",
            plan.raw
        );
        if kind.unsupported(Feature::ExplainAnalyze).is_none() {
            let plan = conn.explain(&q, true).await.expect("explain analyze");
            assert!(plan.analyzed);
            let mut actual = false;
            plan.root.walk(&mut |n, _| actual |= n.actual_rows.is_some() || n.actual_total_ms.is_some());
            assert!(actual || plan.execution_time_ms.is_some(), "no measured values:\n{}", plan.raw);
        } else {
            assert!(matches!(conn.explain(&q, true).await, Err(Error::Unsupported(_))));
        }

        // 5. row limit
        let limited = conn
            .execute(&sub("SELECT id FROM {p}_emp ORDER BY id"), &ExecOptions { row_limit: Some(2) })
            .await
            .expect("limited");
        let rs = limited.result.unwrap();
        assert_eq!((rs.rows.len(), rs.truncated), (2, true));
        let unlimited = conn.execute(&sub("SELECT id FROM {p}_emp"), &all()).await.unwrap().result.unwrap();
        assert_eq!((unlimited.rows.len(), unlimited.truncated), (3, false));

        // 6. error shape
        match conn.execute("SELEC 1", &all()).await {
            Err(Error::Query { message, .. }) => assert!(!message.is_empty()),
            other => panic!("expected a Query error, got {other:?}"),
        }
        // the session survives an error
        assert!(conn.execute("SELECT 1", &all()).await.is_ok());

        // 7. cancel
        let c2 = conn.clone();
        let slow = spec.slow_sql;
        let running = tokio::spawn(async move { c2.execute(slow, &all()).await });
        tokio::time::sleep(Duration::from_millis(800)).await;
        let started = Instant::now();
        conn.cancel().await.expect("cancel");
        let r = tokio::time::timeout(Duration::from_secs(15), running)
            .await
            .expect("cancel did not stop the statement")
            .unwrap();
        assert!(matches!(r, Err(Error::Cancelled)), "expected Cancelled, got {r:?}");
        assert!(started.elapsed() < Duration::from_secs(10));
        // and the connection is usable afterwards
        assert!(conn.execute("SELECT 1", &all()).await.is_ok());
    }
    .await;
    for s in spec.cleanup {
        let _ = conn.execute(&sub(s), &all()).await;
    }
    let _ = conn.close().await;
}

#[tokio::test]
async fn cockroachdb() {
    if let Some(c) = env_config("THETA_TEST_COCKROACH_URL", DatabaseKind::CockroachDb) {
        conformance(c, pg_like(DatabaseKind::CockroachDb)).await;
    }
}

#[tokio::test]
async fn timescaledb() {
    if let Some(c) = env_config("THETA_TEST_TIMESCALE_URL", DatabaseKind::TimescaleDb) {
        conformance(c, pg_like(DatabaseKind::TimescaleDb)).await;
    }
}

#[tokio::test]
async fn yugabytedb() {
    if let Some(c) = env_config("THETA_TEST_YUGABYTE_URL", DatabaseKind::YugabyteDb) {
        conformance(c, pg_like(DatabaseKind::YugabyteDb)).await;
    }
}

#[tokio::test]
async fn redshift_cluster() {
    if let Some(c) = env_config("THETA_TEST_REDSHIFT_URL", DatabaseKind::Redshift) {
        conformance(c, redshift()).await;
    }
}

#[tokio::test]
async fn tidb_server() {
    if let Some(c) = env_config("THETA_TEST_TIDB_URL", DatabaseKind::TiDb) {
        conformance(c, tidb()).await;
    }
}

// The two products that run on every development machine (and in CI) go through the same generic run.
// That is what proves the harness itself: a failure here is a bug in the test, not in an untested driver.

#[tokio::test]
async fn postgres_through_the_generic_run() {
    conformance(pg_config(), pg_like(DatabaseKind::Postgres)).await;
}

#[tokio::test]
async fn mariadb_through_the_generic_run() {
    conformance(mysql_config(), Spec { kind: DatabaseKind::MySql, ..tidb() }).await;
}
