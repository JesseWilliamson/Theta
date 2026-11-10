//! # quarry-drivers
//!
//! Implementations of [`quarry_core::Driver`] / [`quarry_core::Connection`] for
//! PostgreSQL, MySQL/MariaDB and SQLite, SSH tunnelling, and a demo-data seeder.
//!
//! Everything runs on Tokio. The entry point for the application is [`connect`].

pub mod demo;
mod mysql;
mod plan_mysql;
mod plan_pg;
mod postgres;
mod sqlite;
pub mod ssh;
mod util;

use async_trait::async_trait;
use quarry_core::*;
use tokio::sync::Mutex;

/// The driver for a database kind.
pub fn driver_for(kind: DatabaseKind) -> Box<dyn Driver> {
    match kind {
        DatabaseKind::Postgres => Box::new(postgres::PostgresDriver),
        DatabaseKind::MySql => Box::new(mysql::MysqlDriver),
        DatabaseKind::Sqlite => Box::new(sqlite::SqliteDriver),
    }
}

/// Connect, opening an SSH tunnel first when `config.ssh` is set. The returned
/// connection owns the tunnel and tears it down on close/drop.
pub async fn connect(config: &ConnectionConfig) -> Result<Box<dyn Connection>> {
    let Some(ssh_cfg) = config.ssh.as_ref().filter(|_| !config.kind.is_file_based()) else {
        return driver_for(config.kind).connect(config).await;
    };
    let port = if config.port == 0 { config.kind.default_port().unwrap_or(0) } else { config.port };
    let host = if config.host.is_empty() { "localhost" } else { config.host.as_str() };
    let tunnel = ssh::Tunnel::open(ssh_cfg, host, port).await?;
    let local = tunnel.local_port();
    let inner: Result<Box<dyn Connection>> = match config.kind {
        DatabaseKind::Postgres => {
            postgres::PgConnection::connect(config, Some(postgres::LocalEndpoint { port: local }))
                .await
                .map(|c| Box::new(c) as Box<dyn Connection>)
        }
        DatabaseKind::MySql => {
            mysql::MysqlConnection::connect(config, Some(mysql::LocalEndpoint { port: local }))
                .await
                .map(|c| Box::new(c) as Box<dyn Connection>)
        }
        DatabaseKind::Sqlite => unreachable!("file-based databases never tunnel"),
    };
    match inner {
        Ok(inner) => Ok(Box::new(Tunneled { inner, tunnel: Mutex::new(Some(tunnel)) })),
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
