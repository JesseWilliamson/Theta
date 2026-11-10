use crate::{Catalog, ConnectionConfig, DatabaseKind, ExecOptions, Plan, Result, ServerInfo, StatementOutcome};
use async_trait::async_trait;

/// A live session with one database.
///
/// Implementations must be usable from any thread and from multiple tasks:
/// the app calls `cancel` from the UI while `execute` is still awaiting.
/// Statements on one `Connection` run **one at a time, in call order** (they
/// share a single server session, so transactions and session settings behave
/// the way a user expects from a console).
#[async_trait]
pub trait Connection: Send + Sync {
    fn kind(&self) -> DatabaseKind;

    async fn server_info(&self) -> Result<ServerInfo>;

    /// Run exactly one SQL statement. Callers split scripts into statements
    /// first (see `quarry_sql::split_statements`).
    async fn execute(&self, sql: &str, options: &ExecOptions) -> Result<StatementOutcome>;

    /// Ask the server to abort whatever `execute` is currently running on this
    /// connection. Returns `Ok` even if nothing was running. The interrupted
    /// `execute` call resolves to `Err(Error::Cancelled)`.
    async fn cancel(&self) -> Result<()>;

    /// Read the full structure: schemas, tables, columns, keys, indexes, routines.
    async fn introspect(&self) -> Result<Catalog>;

    /// Produce the query plan. With `analyze = true` the statement is really
    /// executed to collect timings; drivers must make that side-effect free
    /// for writes (run inside a transaction that is rolled back).
    async fn explain(&self, sql: &str, analyze: bool) -> Result<Plan>;

    /// Whether the session currently has an open transaction.
    async fn in_transaction(&self) -> bool;

    /// Close the session and any SSH tunnel carrying it.
    async fn close(&self) -> Result<()>;
}

/// Opens connections for one [`DatabaseKind`].
#[async_trait]
pub trait Driver: Send + Sync {
    fn kind(&self) -> DatabaseKind;
    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>>;
}
