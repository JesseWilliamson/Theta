//! Shared helpers for the integration tests. The servers are shared with other
//! processes, so every test works in its own uniquely named schema/database.
#![allow(dead_code)]

use quarry_core::*;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

pub fn unique(prefix: &str) -> String {
    format!("{prefix}_{}_{}", std::process::id(), COUNTER.fetch_add(1, Ordering::SeqCst))
}

pub fn pg_config() -> ConnectionConfig {
    let mut c = ConnectionConfig::new("t-pg", "pg", DatabaseKind::Postgres);
    c.host = "127.0.0.1".into();
    c.user = "postgres".into();
    c.password = Some("postgres".into());
    c.database = "quarry_demo".into();
    c
}

pub fn mysql_config() -> ConnectionConfig {
    let mut c = ConnectionConfig::new("t-my", "my", DatabaseKind::MySql);
    c.host = "127.0.0.1".into();
    c.user = "quarry".into();
    c.password = Some("quarry".into());
    c.database = "quarry_demo".into();
    c
}

pub fn sqlite_config(path: &str) -> ConnectionConfig {
    let mut c = ConnectionConfig::new("t-sqlite", "sqlite", DatabaseKind::Sqlite);
    c.path = path.into();
    c
}

pub fn all() -> ExecOptions {
    ExecOptions { row_limit: None }
}

pub async fn run(conn: &dyn Connection, sql: &str) -> StatementOutcome {
    conn.execute(sql, &all()).await.unwrap_or_else(|e| panic!("`{sql}` failed: {e:?}"))
}

pub async fn rows(conn: &dyn Connection, sql: &str) -> Vec<Vec<Value>> {
    run(conn, sql).await.result.unwrap_or_else(|| panic!("`{sql}` returned no result set")).rows
}

pub async fn one(conn: &dyn Connection, sql: &str) -> Vec<Value> {
    rows(conn, sql).await.into_iter().next().expect("one row")
}

pub async fn scalar(conn: &dyn Connection, sql: &str) -> Value {
    one(conn, sql).await.remove(0)
}

pub fn text(s: &str) -> Value {
    Value::Text(s.to_string())
}

/// A PostgreSQL connection whose `search_path` points at a fresh schema (PostGIS
/// lives in `public`, so it stays on the path).
pub struct PgFx {
    pub conn: Box<dyn Connection>,
    pub schema: String,
}

impl PgFx {
    pub async fn new() -> PgFx {
        let conn = quarry_drivers::connect(&pg_config()).await.expect("connect to PostgreSQL");
        let schema = unique("qt");
        run(conn.as_ref(), &format!("CREATE SCHEMA {schema}")).await;
        run(conn.as_ref(), &format!("SET search_path TO {schema}, public")).await;
        PgFx { conn, schema }
    }

    pub async fn done(self) {
        let _ = self.conn.execute("ROLLBACK", &all()).await;
        let _ = self.conn.execute(&format!("DROP SCHEMA {} CASCADE", self.schema), &all()).await;
        let _ = self.conn.close().await;
    }
}

/// A MariaDB connection `USE`-ing a fresh database.
pub struct MyFx {
    pub conn: Box<dyn Connection>,
    pub db: String,
}

impl MyFx {
    pub async fn new() -> MyFx {
        let conn = quarry_drivers::connect(&mysql_config()).await.expect("connect to MariaDB");
        let db = unique("qt");
        run(conn.as_ref(), &format!("CREATE DATABASE {db}")).await;
        run(conn.as_ref(), &format!("USE {db}")).await;
        MyFx { conn, db }
    }

    pub async fn done(self) {
        let _ = self.conn.execute("ROLLBACK", &all()).await;
        let _ = self.conn.execute(&format!("DROP DATABASE {}", self.db), &all()).await;
        let _ = self.conn.close().await;
    }
}
