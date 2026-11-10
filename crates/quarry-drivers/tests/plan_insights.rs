//! Do the `quarry_sql::analyze_plan` rules fire on plans produced by the real
//! servers? The drivers' parsers and the analysis rules were written separately,
//! so these tests pin the contract between them (key names in `PlanNode::extra`,
//! node type spellings) against live PostgreSQL, MariaDB and SQLite.

mod common;

use common::*;
use quarry_core::*;
use quarry_sql::{Insight, Severity, analyze_plan, sql_literal};

async fn insights(conn: &dyn Connection, sql: &str, analyze: bool) -> (Plan, Vec<Insight>) {
    let plan = conn.explain(sql, analyze).await.unwrap_or_else(|e| panic!("explain `{sql}`: {e:?}"));
    let (_, found) = analyze_plan(&plan, None, conn.kind());
    if std::env::var_os("QUARRY_DUMP_PLANS").is_some() {
        eprintln!("== {sql}\n{}\n-- insights:", plan.raw);
        for i in &found {
            eprintln!("  [{:?}] {} | {} | {:?}", i.severity, i.title, i.node_path.len(), i.suggestion);
        }
    }
    (plan, found)
}

fn find<'a>(found: &'a [Insight], title_prefix: &str) -> &'a Insight {
    found.iter().find(|i| i.title.starts_with(title_prefix)).unwrap_or_else(|| {
        panic!(
            "no insight starting with {title_prefix:?}; got {:#?}",
            found.iter().map(|i| &i.title).collect::<Vec<_>>()
        )
    })
}

// ---------------------------------------------------------------- PostgreSQL

/// A 200k-row table without indexes plus a small one, in the fixture's schema.
async fn pg_tables(c: &dyn Connection) {
    run(c, "SET max_parallel_workers_per_gather = 0").await;
    run(
        c,
        "CREATE TABLE big AS SELECT g AS id, g % 5000 AS k, md5(g::text) AS pad FROM generate_series(1, 200000) g",
    )
    .await;
    run(c, "ALTER TABLE big SET (autovacuum_enabled = false)").await;
    run(c, "ANALYZE big").await;
}

#[tokio::test]
async fn pg_seq_scan_suggests_index() {
    let fx = PgFx::new().await;
    let c = fx.conn.as_ref();
    pg_tables(c).await;
    let (plan, found) = insights(c, "SELECT * FROM big WHERE id = 777", true).await;
    assert!(plan.analyzed);
    let i = find(&found, "Sequential scan reads 200,000 rows of big");
    assert!(matches!(i.severity, Severity::Warning | Severity::Critical));
    assert_eq!(i.suggestion.as_deref(), Some("CREATE INDEX idx_big_id ON big (id);"));
    assert!(i.node_path.is_empty(), "scan is the root: {:?}", i.node_path);
    fx.done().await;
}

#[tokio::test]
async fn pg_stale_statistics_misestimate() {
    let fx = PgFx::new().await;
    let c = fx.conn.as_ref();
    pg_tables(c).await;
    // No statistics exist for an expression, so the planner falls back to a flat
    // 0.5% guess while every row actually matches.
    let (_, found) = insights(c, "SELECT * FROM big WHERE (id % 1) = 0", true).await;
    let i = find(&found, "Row estimate is off by");
    assert_eq!(i.suggestion.as_deref(), Some("ANALYZE big;"));
    assert!(i.detail.contains("under-estimated"), "{}", i.detail);
    fx.done().await;
}

#[tokio::test]
async fn pg_sort_spills_to_disk() {
    let fx = PgFx::new().await;
    let c = fx.conn.as_ref();
    pg_tables(c).await;
    run(c, "SET work_mem = '64kB'").await;
    let (_, found) = insights(c, "SELECT * FROM big ORDER BY pad", true).await;
    let i = find(&found, "Sort spilled to disk");
    let s = i.suggestion.as_deref().expect("work_mem suggestion");
    assert!(s.starts_with("SET work_mem = '") && s.ends_with("MB';"), "{s}");
    fx.done().await;
}

#[tokio::test]
async fn pg_hash_join_batches() {
    let fx = PgFx::new().await;
    let c = fx.conn.as_ref();
    pg_tables(c).await;
    run(c, "SET work_mem = '64kB'").await;
    run(c, "SET enable_mergejoin = off").await;
    run(c, "SET enable_nestloop = off").await;
    let (_, found) =
        insights(c, "SELECT count(*) FROM big a JOIN big b ON a.pad = b.pad WHERE a.id < 150000", true).await;
    let i = find(&found, "Hash split into");
    assert!(i.suggestion.as_deref().is_some_and(|s| s.starts_with("SET work_mem")));
    fx.done().await;
}

#[tokio::test]
async fn pg_nested_loop_rescans_unindexed_table() {
    let fx = PgFx::new().await;
    let c = fx.conn.as_ref();
    run(c, "SET max_parallel_workers_per_gather = 0").await;
    run(c, "CREATE TABLE inner_t AS SELECT g AS id, g % 100 AS k FROM generate_series(1, 5000) g").await;
    run(c, "CREATE TABLE outer_t AS SELECT g AS id, g AS k FROM generate_series(1, 300) g").await;
    run(c, "ANALYZE inner_t").await;
    run(c, "ANALYZE outer_t").await;
    for s in ["enable_hashjoin", "enable_mergejoin", "enable_material"] {
        run(c, &format!("SET {s} = off")).await;
    }
    let (_, found) = insights(c, "SELECT * FROM outer_t o JOIN inner_t i ON i.k = o.k", true).await;
    let i = find(&found, "Scan of inner_t repeated 300 times");
    assert_eq!(i.suggestion.as_deref(), Some("CREATE INDEX idx_inner_t_k ON inner_t (k);"));
    fx.done().await;
}

#[tokio::test]
async fn pg_cartesian_product() {
    let fx = PgFx::new().await;
    let c = fx.conn.as_ref();
    run(c, "CREATE TABLE a AS SELECT g AS id FROM generate_series(1, 100) g").await;
    run(c, "CREATE TABLE b AS SELECT g AS id FROM generate_series(1, 100) g").await;
    run(c, "ANALYZE a").await;
    run(c, "ANALYZE b").await;
    // Both the estimated and the analysed plan must be flagged.
    for analyze in [false, true] {
        let (_, found) = insights(c, "SELECT * FROM a, b", analyze).await;
        let i = find(&found, "Possible cartesian product");
        assert_eq!(i.severity, Severity::Critical);
        assert!(i.suggestion.as_deref().is_some_and(|s| s.contains("ON")));
    }
    // A real join condition must not be flagged.
    let (_, found) = insights(c, "SELECT * FROM a JOIN b ON a.id = b.id", true).await;
    assert!(!found.iter().any(|i| i.title.contains("cartesian")), "{found:#?}");
    fx.done().await;
}

// ------------------------------------------------------------------- MariaDB

async fn my_tables(c: &dyn Connection) {
    // The default recursion cap (1000) would truncate the row generator.
    run(c, "SET SESSION max_recursive_iterations = 1000000").await;
    run(
        c,
        "CREATE TABLE big (id int NOT NULL, k int NOT NULL, grp int NOT NULL, pad varchar(40) NOT NULL) ENGINE=InnoDB",
    )
    .await;
    run(
        c,
        "INSERT INTO big WITH RECURSIVE s(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM s WHERE n < 200000) \
         SELECT n, n % 5000, n % 50, md5(n) FROM s",
    )
    .await;
    run(c, "ANALYZE TABLE big").await;
}

#[tokio::test]
async fn mariadb_full_scan_filesort_and_temporary() {
    let fx = MyFx::new().await;
    let c = fx.conn.as_ref();
    my_tables(c).await;

    let (_, found) = insights(c, "SELECT * FROM big WHERE k = 123", true).await;
    let i = find(&found, "Full table scan (type ALL) on big");
    assert_eq!(i.suggestion.as_deref(), Some("CREATE INDEX idx_big_k ON big (k);"));
    // The estimate-only plan carries the same finding.
    let (_, found) = insights(c, "SELECT * FROM big WHERE k = 123", false).await;
    find(&found, "Full table scan (type ALL) on big");

    let (_, found) = insights(c, "SELECT * FROM big ORDER BY pad LIMIT 20000", true).await;
    find(&found, "Using filesort");

    let (_, found) = insights(c, "SELECT k, count(*) FROM big GROUP BY k", true).await;
    find(&found, "Using temporary table");
    fx.done().await;
}

#[tokio::test]
async fn mariadb_geometry_literal_round_trip() {
    let fx = MyFx::new().await;
    let c = fx.conn.as_ref();
    run(c, "CREATE TABLE src (id int, g geometry)").await;
    run(c, "CREATE TABLE dst (id int, g geometry)").await;
    run(
        c,
        "INSERT INTO src VALUES (1, ST_GeomFromText('POINT(1 2)', 4326)), \
         (2, ST_GeomFromText('POLYGON((0 0,4 0,4 4,0 0))', 3857)), (3, ST_GeomFromText('POINT(5 6)'))",
    )
    .await;
    let rs = run(c, "SELECT id, g FROM src ORDER BY id").await.result.unwrap();
    for row in &rs.rows {
        let lit = sql_literal(&row[1], DatabaseKind::MySql);
        run(
            c,
            &format!(
                "INSERT INTO dst VALUES ({}, {lit})",
                match &row[0] {
                    Value::Int(i) => *i,
                    other => panic!("{other:?}"),
                }
            ),
        )
        .await;
    }
    // Same bytes in, same bytes out: geometry, SRID and all.
    let copy = run(c, "SELECT id, g FROM dst ORDER BY id").await.result.unwrap();
    assert_eq!(rs.rows, copy.rows);
    let meta = rows(c, "SELECT ST_SRID(g), ST_AsText(g) FROM dst ORDER BY id").await;
    assert_eq!(meta[0], vec![Value::Int(4326), text("POINT(1 2)")]);
    assert_eq!(meta[1][0], Value::Int(3857));
    assert_eq!(meta[2], vec![Value::Int(0), text("POINT(5 6)")]);
    fx.done().await;
}

// -------------------------------------------------------------------- SQLite

async fn sqlite_tables(c: &dyn Connection) {
    run(c, "CREATE TABLE big (id INTEGER PRIMARY KEY, k INTEGER, pad TEXT)").await;
    run(
        c,
        "WITH RECURSIVE s(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM s WHERE n < 200000) \
         INSERT INTO big SELECT n, n % 5000, hex(randomblob(8)) FROM s",
    )
    .await;
}

#[tokio::test]
async fn sqlite_scan_and_temp_btree() {
    let dir = std::env::temp_dir().join(unique("qplan"));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plan.db");
    let c = quarry_drivers::connect(&sqlite_config(path.to_str().unwrap())).await.unwrap();
    sqlite_tables(c.as_ref()).await;

    let (_, found) = insights(c.as_ref(), "SELECT * FROM big WHERE k = 5", true).await;
    let i = find(&found, "SCAN big reads the whole table");
    // EXPLAIN QUERY PLAN does not report the WHERE clause, so the rule cannot know
    // which column to index and offers advice in prose rather than a CREATE INDEX.
    assert!(i.suggestion.is_none(), "{:?}", i.suggestion);
    assert!(i.detail.contains("SEARCH using an index"), "{}", i.detail);

    let (_, found) = insights(c.as_ref(), "SELECT * FROM big ORDER BY pad", true).await;
    find(&found, "Temporary B-tree built for ORDER BY");
    let (_, found) = insights(c.as_ref(), "SELECT pad, count(*) FROM big GROUP BY pad", false).await;
    find(&found, "Temporary B-tree built for GROUP BY");

    // An indexed lookup is quiet.
    let (_, found) = insights(c.as_ref(), "SELECT * FROM big WHERE id = 5", true).await;
    assert!(found.is_empty(), "{found:#?}");

    let _ = c.close().await;
    let _ = std::fs::remove_dir_all(&dir);
}
