//! # theta-drivers
//!
//! Implementations of [`theta_core::Driver`] / [`theta_core::Connection`] for
//! PostgreSQL, MySQL/MariaDB and SQLite, SSH tunnelling, and a demo-data seeder.
//!
//! Everything runs on Tokio. The entry point for the application is [`connect`].

pub mod demo;
mod mysql;
mod plan_mysql;
mod plan_pg;
mod plan_text;
mod postgres;
mod sqlite;
pub mod ssh;
mod util;

use async_trait::async_trait;
use theta_core::*;
use tokio::sync::Mutex;

/// Parse PostgreSQL `EXPLAIN (FORMAT JSON)` output. Public so the plan view's demo and
/// tests can load captured plans without a server.
pub fn parse_postgres_plan_json(raw: &str, analyzed: bool) -> Result<Plan> {
    plan_pg::parse_plan(raw, analyzed)
}

/// Parse MariaDB `EXPLAIN/ANALYZE FORMAT=JSON` or MySQL 8 `EXPLAIN FORMAT=JSON` output.
pub fn parse_mysql_plan_json(raw: &str, analyzed: bool) -> Result<Plan> {
    plan_mysql::parse_json_plan(raw, analyzed)
}

/// Parse SQLite `EXPLAIN QUERY PLAN` rows in the `id<TAB>parent<TAB>detail` form that
/// [`Connection::explain`] stores in `Plan::raw`. The result is an estimate-only
/// plan; SQLite has no per-node timings.
pub fn parse_sqlite_plan(raw: &str) -> Result<Plan> {
    let mut rows = Vec::new();
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        let mut p = line.splitn(3, '\t');
        let (Some(id), Some(parent), Some(detail)) = (p.next(), p.next(), p.next()) else {
            return Err(Error::other(format!("malformed EXPLAIN QUERY PLAN row: {line:?}")));
        };
        let num = |s: &str| s.parse::<i64>().map_err(|_| Error::other(format!("bad row id in {line:?}")));
        rows.push((num(id)?, num(parent)?, detail.to_string()));
    }
    if rows.is_empty() {
        return Err(Error::other("EXPLAIN QUERY PLAN returned no rows"));
    }
    Ok(Plan {
        root: sqlite::plan_from_eqp(&rows),
        analyzed: false,
        planning_time_ms: None,
        execution_time_ms: None,
        raw: raw.to_string(),
    })
}

/// The driver for a database kind.
pub fn driver_for(kind: DatabaseKind) -> Box<dyn Driver> {
    match kind {
        DatabaseKind::Postgres
        | DatabaseKind::CockroachDb
        | DatabaseKind::Redshift
        | DatabaseKind::TimescaleDb
        | DatabaseKind::YugabyteDb => Box::new(postgres::PostgresDriver { kind }),
        DatabaseKind::MySql | DatabaseKind::TiDb => Box::new(mysql::MysqlDriver { kind }),
        DatabaseKind::Sqlite => Box::new(sqlite::SqliteDriver),
        DatabaseKind::DuckDb | DatabaseKind::SqlServer | DatabaseKind::ClickHouse => {
            unreachable!("driver arrives with its own commit")
        }
    }
}

/// Connect, opening an SSH tunnel first when `config.ssh` is set. The returned
/// connection owns the tunnel and tears it down on close/drop.
pub async fn connect(config: &ConnectionConfig) -> Result<Box<dyn Connection>> {
    connect_with_notices(config).await.map(|(c, _)| c)
}

/// Like [`connect`], also returning things worth telling the user once (for example that an SSH
/// host key was added to known_hosts on first contact).
pub async fn connect_with_notices(config: &ConnectionConfig) -> Result<(Box<dyn Connection>, Vec<String>)> {
    let Some(ssh_cfg) = config.ssh.as_ref().filter(|_| !config.kind.is_file_based()) else {
        return driver_for(config.kind).connect(config).await.map(|c| (c, Vec::new()));
    };
    let port = if config.port == 0 { config.kind.default_port().unwrap_or(0) } else { config.port };
    let host = if config.host.is_empty() { "localhost" } else { config.host.as_str() };
    let tunnel = ssh::Tunnel::open(ssh_cfg, host, port).await?;
    let local = tunnel.local_port();
    let notices = tunnel.notices().to_vec();
    let inner: Result<Box<dyn Connection>> = match config.kind {
        k if k.family() == Family::Postgres => {
            postgres::PgConnection::connect(config, Some(postgres::LocalEndpoint { port: local }))
                .await
                .map(|c| Box::new(c) as Box<dyn Connection>)
        }
        k if k.family() == Family::MySql => {
            mysql::MysqlConnection::connect(config, Some(mysql::LocalEndpoint { port: local }))
                .await
                .map(|c| Box::new(c) as Box<dyn Connection>)
        }
        _ => unreachable!("file-based databases never tunnel"),
    };
    match inner {
        Ok(inner) => Ok((Box::new(Tunneled { inner, tunnel: Mutex::new(Some(tunnel)) }), notices)),
        Err(e) => {
            tunnel.close().await;
            Err(e)
        }
    }
}

/// A connection plus the SSH tunnel that carries it.
struct Tunneled {
    inner: Box<dyn Connection>,
    tunnel: Mutex<Option<ssh::Tunnel>>,
}

#[async_trait]
impl Connection for Tunneled {
    fn kind(&self) -> DatabaseKind {
        self.inner.kind()
    }
    async fn server_info(&self) -> Result<ServerInfo> {
        self.inner.server_info().await
    }
    async fn execute(&self, sql: &str, options: &ExecOptions) -> Result<StatementOutcome> {
        self.inner.execute(sql, options).await
    }
    async fn cancel(&self) -> Result<()> {
        self.inner.cancel().await
    }
    async fn introspect(&self) -> Result<Catalog> {
        self.inner.introspect().await
    }
    async fn explain(&self, sql: &str, analyze: bool) -> Result<Plan> {
        self.inner.explain(sql, analyze).await
    }
    async fn in_transaction(&self) -> bool {
        self.inner.in_transaction().await
    }
    async fn close(&self) -> Result<()> {
        let r = self.inner.close().await;
        if let Some(t) = self.tunnel.lock().await.take() {
            t.close().await;
        }
        r
    }
}

#[cfg(test)]
mod plan_parse_tests {
    use super::*;

    #[test]
    fn sqlite_rows_text_becomes_a_tree() {
        let p = parse_sqlite_plan("3\t0\tSCAN big\n9\t0\tUSE TEMP B-TREE FOR ORDER BY\n").unwrap();
        assert!(!p.analyzed);
        assert_eq!(p.root.node_type, "QUERY PLAN");
        assert_eq!(p.root.children[0].relation.as_deref(), Some("big"));
        assert!(parse_sqlite_plan("").is_err());
        assert!(parse_sqlite_plan("x\ty\tz").is_err());
    }

    #[test]
    fn postgres_and_mysql_json_wrappers_parse() {
        let pg = include_str!("../tests/fixtures/pg_plain.json");
        assert_eq!(parse_postgres_plan_json(pg, false).unwrap().root.node_type, "Sort");
        let my = include_str!("../tests/fixtures/mariadb_analyze.json");
        assert!(parse_mysql_plan_json(my, true).unwrap().analyzed);
    }
}
