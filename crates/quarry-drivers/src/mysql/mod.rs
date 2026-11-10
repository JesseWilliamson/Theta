//! MySQL / MariaDB driver on top of `mysql_async`.
//!
//! * Text protocol throughout (see `types.rs` for why).
//! * `cancel` opens a second short-lived connection and runs `KILL QUERY <id>`;
//!   it never touches the mutex that serialises statements.
//! * Rows stream out of the server; once the row limit is exceeded we kill the
//!   running query (so the server stops producing rows) and drain what is already
//!   in flight.

mod introspect;
mod types;

use crate::plan_mysql;
use crate::util::{has_multiple_statements, reports_rows_affected};
use async_trait::async_trait;
use mysql_async::consts::StatusFlags;
use mysql_async::prelude::*;
use mysql_async::{Conn, Opts, OptsBuilder, Row, SslOpts};
use quarry_core::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub struct MysqlDriver;

#[async_trait]
impl Driver for MysqlDriver {
    fn kind(&self) -> DatabaseKind {
        DatabaseKind::MySql
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>> {
        Ok(Box::new(MysqlConnection::connect(config, None).await?))
    }
}

pub struct MysqlConnection {
    conn: Mutex<Option<Conn>>,
    opts: Opts,
    conn_id: u32,
    is_mariadb: bool,
    running: AtomicBool,
    cancel_requested: AtomicBool,
    in_txn_cache: AtomicBool,
    info: ServerInfo,
}

/// Exposed for the SSH-tunnelled connect path.
pub struct LocalEndpoint {
    pub port: u16,
}

const ER_ACCESS_DENIED: u16 = 1045;
const ER_DBACCESS_DENIED: u16 = 1044;
const ER_QUERY_INTERRUPTED: u16 = 1317;
const ER_ACCOUNT_LOCKED: u16 = 3118;

fn map_connect_error(e: mysql_async::Error) -> Error {
    match &e {
        mysql_async::Error::Server(s)
            if matches!(s.code, ER_ACCESS_DENIED | ER_DBACCESS_DENIED | ER_ACCOUNT_LOCKED) =>
        {
            Error::Auth(s.message.clone())
        }
        mysql_async::Error::Server(s) => Error::Connect(s.message.clone()),
        other => Error::Connect(other.to_string()),
    }
}

/// Derive a 0-based byte offset from MySQL's "... near '<rest of statement>' at line N".
/// The quoted text is the tail of the statement starting at the offending token,
/// which is why searching for it from the start of line N finds the position.
pub(crate) fn syntax_error_offset(message: &str, sql: &str) -> Option<usize> {
    let start = message.find("near '")? + "near '".len();
    let end = message.rfind("' at line ")?;
    if end < start {
        return None;
    }
    let near = &message[start..end];
    let line: usize = message[end + "' at line ".len()..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
    let mut line_start = 0;
    for _ in 1..line {
        line_start += sql[line_start..].find('\n')? + 1;
    }
    if near.is_empty() {
        return Some(sql.len());
    }
    sql[line_start..].find(near).map(|i| line_start + i)
}

fn map_error(e: mysql_async::Error, sql: &str) -> Error {
    match e {
        mysql_async::Error::Server(s) => {
            let position = syntax_error_offset(&s.message, sql).map(|offset| ErrorPosition { offset });
            Error::Query {
                message: s.message.clone(),
                code: Some(s.code.to_string()),
                detail: Some(format!("SQLSTATE {}", s.state)),
                hint: None,
                position,
            }
        }
        mysql_async::Error::Io(_) | mysql_async::Error::Driver(_) => Error::Closed,
        other => Error::Other(other.to_string()),
    }
}

fn is_interrupted(e: &Error) -> bool {
    matches!(e, Error::Query { code: Some(c), .. } if *c == ER_QUERY_INTERRUPTED.to_string())
}

fn build_opts(config: &ConnectionConfig, via: Option<&LocalEndpoint>, ssl: SslChoice) -> Opts {
    let mut b = OptsBuilder::default()
        .ip_or_hostname(if via.is_some() { "127.0.0.1" } else if config.host.is_empty() { "localhost" } else { &config.host })
        .tcp_port(via.map(|v| v.port).unwrap_or(if config.port == 0 { 3306 } else { config.port }))
        .user(Some(config.user.clone()).filter(|u| !u.is_empty()))
        .pass(config.password.clone())
        .db_name(Some(config.database.clone()).filter(|d| !d.is_empty()))
        .prefer_socket(false)
        .stmt_cache_size(0);
    b = match ssl {
        SslChoice::Off => b.ssl_opts(None::<SslOpts>),
        SslChoice::NoVerify => b.ssl_opts(Some(
            SslOpts::default().with_danger_accept_invalid_certs(true).with_danger_skip_domain_validation(true),
        )),
        SslChoice::Verify => {
            let mut o = SslOpts::default();
            if via.is_some() && !config.host.is_empty() {
                // Dial the tunnel but validate the certificate against the real host name.
                o = o.with_danger_tls_hostname_override(Some(config.host.clone()));
            }
            b.ssl_opts(Some(o))
        }
    };
    b.into()
}

#[derive(Clone, Copy)]
enum SslChoice {
    Off,
    NoVerify,
    Verify,
}

impl MysqlConnection {
    pub async fn connect(config: &ConnectionConfig, via: Option<LocalEndpoint>) -> Result<Self> {
        let via = via.as_ref();
        let (opts, conn) = match config.ssl_mode {
            SslMode::Disable => {
                let o = build_opts(config, via, SslChoice::Off);
                (o.clone(), Conn::new(o).await.map_err(map_connect_error)?)
            }
            SslMode::Require => {
                let o = build_opts(config, via, SslChoice::NoVerify);
                (o.clone(), Conn::new(o).await.map_err(map_connect_error)?)
            }
            SslMode::VerifyFull => {
                let o = build_opts(config, via, SslChoice::Verify);
                (o.clone(), Conn::new(o).await.map_err(map_connect_error)?)
            }
            SslMode::Prefer => {
                // mysql_async has no "try TLS, else plaintext" mode of its own.
                let o = build_opts(config, via, SslChoice::NoVerify);
                match tokio::time::timeout(Duration::from_secs(10), Conn::new(o.clone())).await {
                    Ok(Ok(c)) => (o, c),
                    Ok(Err(e @ mysql_async::Error::Server(_))) => return Err(map_connect_error(e)),
                    _ => {
                        let o = build_opts(config, via, SslChoice::Off);
                        (o.clone(), Conn::new(o).await.map_err(map_connect_error)?)
                    }
                }
            }
        };
        let mut conn = conn;
        let conn_id = conn.id();
        let row: Option<(String, Option<String>, String)> = conn
            .query_first("SELECT VERSION(), DATABASE(), CURRENT_USER()")
            .await
            .map_err(map_connect_error)?;
        let (version, db, user) = row.unwrap_or_default();
        let is_mariadb = version.to_ascii_lowercase().contains("mariadb");
        let short = version.split('-').next().unwrap_or(&version).to_string();
        let info = ServerInfo {
            version: format!("{} {}", if is_mariadb { "MariaDB" } else { "MySQL" }, short),
            current_database: db,
            current_user: Some(user),
        };
        Ok(Self {
            conn: Mutex::new(Some(conn)),
            opts,
            conn_id,
            is_mariadb,
            running: AtomicBool::new(false),
            cancel_requested: AtomicBool::new(false),
            in_txn_cache: AtomicBool::new(false),
            info,
        })
    }

    async fn kill_query(&self) -> Result<()> {
        let mut killer = Conn::new(self.opts.clone()).await.map_err(map_connect_error)?;
        let r = killer.query_drop(format!("KILL QUERY {}", self.conn_id)).await;
        let _ = killer.disconnect().await;
        r.map_err(|e| Error::Other(e.to_string()))
    }

    async fn run_guarded<T>(&self, f: impl AsyncFnOnce(&mut Conn) -> Result<T>) -> Result<T> {
        let mut guard = self.conn.lock().await;
        let conn = guard.as_mut().ok_or(Error::Closed)?;
        self.cancel_requested.store(false, Ordering::SeqCst);
        self.running.store(true, Ordering::SeqCst);
        let r = f(conn).await;
        self.running.store(false, Ordering::SeqCst);
        let cancelled = self.cancel_requested.load(Ordering::SeqCst);
        match r {
            Err(e) if cancelled && is_interrupted(&e) => Err(Error::Cancelled),
            other => other,
        }
    }

    async fn execute_inner(&self, conn: &mut Conn, sql: &str, limit: Option<usize>) -> Result<StatementOutcome> {
        if has_multiple_statements(sql) && !matches!(crate::util::leading_keyword(sql).as_str(), "CREATE" | "ALTER") {
            return Err(Error::query("only one SQL statement can be executed at a time"));
        }
        let started = Instant::now();
        let m = |e| map_error(e, sql);
        let mut result = conn.query_iter(sql).await.map_err(m)?;
        let columns_meta = result.columns();
        let has_rows = columns_meta.as_ref().is_some_and(|c| !c.is_empty());
        let mut rows: Vec<Vec<Value>> = Vec::new();
        let mut truncated = false;
        let mut columns: Vec<Column> = Vec::new();
        if has_rows {
            let meta = columns_meta.unwrap();
            columns = meta
                .iter()
                .map(|c| {
                    let table = c.org_table_str();
                    let name = c.org_name_str();
                    Column {
                        name: c.name_str().into_owned(),
                        type_name: types::type_name(c),
                        kind: types::type_kind(c),
                        origin: (!table.is_empty() && !name.is_empty()).then(|| ColumnOrigin {
                            schema: Some(c.schema_str().into_owned()).filter(|s| !s.is_empty()),
                            table: table.into_owned(),
                            column: name.into_owned(),
                        }),
                    }
                })
                .collect();
            let mut needs_kill = false;
            loop {
                let next = result.next().await;
                let row: Row = match next {
                    Ok(Some(r)) => r,
                    Ok(None) => break,
                    Err(e) => return Err(m(e)),
                };
                if limit.is_some_and(|l| rows.len() >= l) {
                    truncated = true;
                    needs_kill = true;
                    break;
                }
                let cells = (0..meta.len())
                    .map(|i| {
                        let raw = row.as_ref(i);
                        match raw {
                            Some(mysql_async::Value::Bytes(b)) => types::cell(&meta[i], Some(b)),
                            Some(mysql_async::Value::NULL) | None => Value::Null,
                            Some(other) => Value::Unknown(format!("{other:?}")),
                        }
                    })
                    .collect();
                rows.push(cells);
            }
            if needs_kill {
                // Stop the server producing rows nobody will look at, then drain
                // what is already on the wire so the connection stays usable.
                let _ = self.kill_query().await;
            }
        }
        let affected = result.affected_rows();
        let warnings = result.warnings();
        // Drain everything left, including the error a KILL produces; that error
        // is expected when we truncated and meaningless otherwise to swallow.
        let drained = result.drop_result().await;
        if !truncated {
            drained.map_err(m)?;
        }
        let elapsed = started.elapsed();
        let mut notices = Vec::new();
        if warnings > 0 && !truncated {
            if let Ok(rows) = conn.query::<(String, u32, String), _>("SHOW WARNINGS").await {
                notices = rows.into_iter().map(|(level, code, msg)| format!("{level} ({code}): {msg}")).collect();
            }
        }
        Ok(StatementOutcome {
            result: has_rows.then_some(ResultSet { columns, rows, truncated }),
            rows_affected: (!has_rows && reports_rows_affected(sql)).then_some(affected),
            notices,
            elapsed,
        })
    }
}

async fn query_in_transaction(conn: &mut Conn) -> Option<bool> {
    // `DO 0` is the cheapest statement that yields an OK packet, whose status
    // flags carry SERVER_STATUS_IN_TRANS.
    conn.query_drop("DO 0").await.ok()?;
    conn.last_ok_packet().map(|ok| ok.status_flags().contains(StatusFlags::SERVER_STATUS_IN_TRANS))
}

#[async_trait]
impl Connection for MysqlConnection {
    fn kind(&self) -> DatabaseKind {
        DatabaseKind::MySql
    }

    async fn server_info(&self) -> Result<ServerInfo> {
        Ok(self.info.clone())
    }

    async fn execute(&self, sql: &str, options: &ExecOptions) -> Result<StatementOutcome> {
        let outcome = self.run_guarded(async |conn| self.execute_inner(conn, sql, options.row_limit).await).await?;
        // SLEEP() interrupted by KILL QUERY returns 1 instead of raising an error.
        // A row-returning statement that finishes while a cancel is in flight is
        // therefore reported as cancelled; its rows are of no interest to a user
        // who asked to stop it, and it cannot have had side effects.
        if outcome.result.is_some() && self.cancel_requested.swap(false, Ordering::SeqCst) {
            return Err(Error::Cancelled);
        }
        Ok(outcome)
    }

    async fn cancel(&self) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            self.cancel_requested.store(true, Ordering::SeqCst);
            self.kill_query().await?;
        }
        Ok(())
    }

    async fn introspect(&self) -> Result<Catalog> {
        let is_mariadb = self.is_mariadb;
        self.run_guarded(async |conn| introspect::introspect(conn, is_mariadb).await).await
    }

    async fn explain(&self, sql: &str, analyze: bool) -> Result<Plan> {
        let is_mariadb = self.is_mariadb;
        if has_multiple_statements(sql) {
            return Err(Error::query("only one SQL statement can be explained at a time"));
        }
        self.run_guarded(async |conn| {
            let m = |e| map_error(e, sql);
            let (stmt, text_plan) = match (analyze, is_mariadb) {
                (false, _) => (format!("EXPLAIN FORMAT=JSON {sql}"), false),
                (true, true) => (format!("ANALYZE FORMAT=JSON {sql}"), false),
                (true, false) => (format!("EXPLAIN ANALYZE {sql}"), true),
            };
            // ANALYZE executes the statement, so make it leave nothing behind.
            let in_txn = if analyze { query_in_transaction(conn).await.unwrap_or(false) } else { false };
            let (begin, end) = if !analyze {
                (None, None)
            } else if in_txn {
                (Some("SAVEPOINT quarry_explain"), Some("ROLLBACK TO SAVEPOINT quarry_explain"))
            } else {
                (Some("START TRANSACTION"), Some("ROLLBACK"))
            };
            if let Some(b) = begin {
                conn.query_drop(b).await.map_err(m)?;
            }
            let rows: std::result::Result<Vec<Row>, _> = conn.query(&stmt).await;
            if let Some(e) = end {
                let _ = conn.query_drop(e).await;
            }
            let rows = rows.map_err(|e| {
                // Errors name the original statement, not our `EXPLAIN` prefix.
                map_error(e, sql)
            })?;
            let raw = rows
                .first()
                .and_then(|r| r.get::<String, _>(0))
                .ok_or_else(|| Error::other("EXPLAIN returned no rows"))?;
            if text_plan {
                // EXPLAIN ANALYZE emits one text row per tree (a single column).
                let all: Vec<String> = rows.iter().filter_map(|r| r.get::<String, _>(0)).collect();
                plan_mysql::parse_text_plan(&all.join("\n"))
            } else {
                plan_mysql::parse_json_plan(&raw, analyze)
            }
        })
        .await
    }

    async fn in_transaction(&self) -> bool {
        match self.conn.try_lock() {
            Ok(mut guard) => match guard.as_mut() {
                Some(c) => {
                    let v = query_in_transaction(c).await.unwrap_or(false);
                    self.in_txn_cache.store(v, Ordering::SeqCst);
                    v
                }
                None => false,
            },
            Err(_) => self.in_txn_cache.load(Ordering::SeqCst),
        }
    }

    async fn close(&self) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            let _ = self.cancel().await;
        }
        if let Some(c) = self.conn.lock().await.take() {
            let _ = c.disconnect().await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syntax_error_position_is_found_from_near_text() {
        let sql = "SELECT 1\nFROM t WHERE ??? = 1";
        let msg = "You have an error in your SQL syntax; check the manual that corresponds to your MariaDB server version for the right syntax to use near '??? = 1' at line 2";
        assert_eq!(syntax_error_offset(msg, sql), Some(sql.find("???").unwrap()));
        let eof = "You have an error in your SQL syntax; ... near '' at line 1";
        assert_eq!(syntax_error_offset(eof, "SELECT FROM"), Some(11));
        assert_eq!(syntax_error_offset("Unknown column 'x' in 'field list'", sql), None);
    }
}
