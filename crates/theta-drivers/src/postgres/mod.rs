//! PostgreSQL driver on top of `tokio-postgres`.
//!
//! Design notes:
//! * One `Client` per `Connection`, guarded by an async mutex so statements run
//!   one at a time in call order. `cancel` bypasses the mutex: it only needs the
//!   cancel token and a (separate) short-lived socket.
//! * Result rows are streamed. For row-limited queries we bind a portal and
//!   fetch `limit + 1` rows so the server stops producing after that, instead of
//!   shipping (and us discarding) the rest of a huge table.
//! * `tokio-postgres` does not expose ReadyForQuery's transaction status, so
//!   `in_transaction` asks the server (see [`query_in_transaction`]).

mod introspect;
mod redshift;
mod tls;
mod types;

use crate::util::{char_pos_to_byte_offset, is_ddl, reports_rows_affected};
use crate::{plan_pg, plan_text};
use async_trait::async_trait;
use futures_util::future::BoxFuture;
use futures_util::{StreamExt, TryStreamExt, pin_mut};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use theta_core::*;
use tokio::sync::Mutex;
use tokio_postgres::error::SqlState;
use tokio_postgres::types::ToSql;
use tokio_postgres::{AsyncMessage, Client, Config, NoTls};
use tokio_postgres_rustls::MakeRustlsConnect;

/// One driver for every product that speaks the PostgreSQL wire protocol. The product decides the
/// few places that differ (see `DatabaseKind` checks below): catalogs, EXPLAIN, the transaction probe.
pub struct PostgresDriver {
    pub kind: DatabaseKind,
}

#[async_trait]
impl Driver for PostgresDriver {
    fn kind(&self) -> DatabaseKind {
        self.kind
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>> {
        Ok(Box::new(PgConnection::connect(config, None).await?))
    }
}

struct Notice {
    code: String,
    text: String,
}

type Notices = Arc<StdMutex<Vec<Notice>>>;
type CancelFn = Box<dyn Fn() -> BoxFuture<'static, Result<()>> + Send + Sync>;

struct TableMeta {
    schema: String,
    table: String,
    columns: HashMap<i16, String>,
}

struct Session {
    client: Client,
    /// Table OID -> name info; `None` records an OID that no longer exists.
    origins: HashMap<u32, Option<TableMeta>>,
}

pub struct PgConnection {
    kind: DatabaseKind,
    session: Mutex<Option<Session>>,
    cancel: CancelFn,
    notices: Notices,
    running: AtomicBool,
    cancel_requested: AtomicBool,
    /// Last known answer of `in_transaction`, served while a statement is running.
    in_txn_cache: AtomicBool,
    info: ServerInfo,
}

/// A short product and version for the status bar, from the `version()` banner.
fn describe_server(banner: &str, server_version: &str, timescale: Option<&str>) -> String {
    let word_after = |marker: &str| {
        banner.split_once(marker).and_then(|(_, rest)| rest.split_whitespace().next()).unwrap_or("")
    };
    match DatabaseKind::detect(banner) {
        Some(DatabaseKind::CockroachDb) => {
            // `CockroachDB CCL v24.1.0 (x86_64-pc-linux-gnu, ...)`
            let v = banner
                .split_whitespace()
                .find(|w| w.starts_with('v') && w[1..].starts_with(|c: char| c.is_ascii_digit()));
            format!("CockroachDB {}", v.unwrap_or(server_version))
        }
        Some(DatabaseKind::Redshift) => format!("Amazon Redshift (PostgreSQL {server_version})"),
        Some(DatabaseKind::YugabyteDb) => {
            // `PostgreSQL 11.2-YB-2.18.1.0-b0 on ...`
            let yb = word_after("-YB-").split('-').next().unwrap_or("");
            format!("YugabyteDB {yb} (PostgreSQL {})", server_version.split("-YB-").next().unwrap_or(""))
        }
        _ => {
            let pg = format!("PostgreSQL {}", server_version.split_whitespace().next().unwrap_or(""));
            match timescale {
                Some(t) => format!("TimescaleDB {t} on {pg}"),
                None => pg,
            }
        }
    }
}

fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(inner) = src {
        let t = inner.to_string();
        if !s.contains(&t) {
            s.push_str(": ");
            s.push_str(&t);
        }
        src = inner.source();
    }
    s
}

fn connect_error(e: tokio_postgres::Error) -> Error {
    if let Some(db) = e.as_db_error() {
        let code = db.code();
        return if *code == SqlState::INVALID_PASSWORD
            || *code == SqlState::INVALID_AUTHORIZATION_SPECIFICATION
        {
            Error::Auth(db.message().to_string())
        } else {
            Error::Connect(db.message().to_string())
        };
    }
    let chain = error_chain(&e);
    // The server asked for a password and the connection has none: tokio-postgres reports that as a
    // configuration error rather than as a server error.
    if chain.contains("password missing") {
        return Error::Auth("the server asked for a password".into());
    }
    Error::Connect(chain)
}

/// `skip` is the number of bytes of `sent` that precede the user's SQL (the
/// `EXPLAIN (...)` prefix), so positions stay relative to what the user typed.
fn query_error(e: tokio_postgres::Error, sent: &str, skip: usize) -> Error {
    let Some(db) = e.as_db_error() else {
        if e.is_closed() {
            return Error::Closed;
        }
        return Error::Other(error_chain(&e));
    };
    let position = match db.position() {
        Some(tokio_postgres::error::ErrorPosition::Original(p)) => char_pos_to_byte_offset(sent, *p as usize)
            .checked_sub(skip)
            .map(|offset| ErrorPosition { offset }),
        _ => None,
    };
    Error::Query {
        message: db.message().to_string(),
        code: Some(db.code().code().to_string()),
        detail: db.detail().map(str::to_string),
        hint: db.hint().map(str::to_string),
        position,
    }
}

fn is_cancel_error(e: &Error) -> bool {
    matches!(e, Error::Query { code: Some(c), .. } if c == SqlState::QUERY_CANCELED.code())
}

/// Exposed for the SSH-tunnelled connect path: the TCP endpoint to dial while
/// keeping the configured host name for TLS verification.
pub struct LocalEndpoint {
    pub port: u16,
}

impl PgConnection {
    pub async fn connect(config: &ConnectionConfig, via: Option<LocalEndpoint>) -> Result<Self> {
        let mut cfg = Config::new();
        if config.host.starts_with('/') {
            // A leading '/' means a Unix-socket directory (libpq convention).
            #[cfg(unix)]
            cfg.host_path(&config.host);
            #[cfg(not(unix))]
            return Err(Error::Connect("Unix-socket hosts are not available on this platform".into()));
        } else {
            cfg.host(if config.host.is_empty() { "localhost" } else { &config.host });
        }
        // `Config::port` appends, and hosts/hostaddrs/ports must pair up one to one.
        let default_port = config.kind.default_port().unwrap_or(5432);
        cfg.port(via.as_ref().map(|v| v.port).unwrap_or(if config.port == 0 {
            default_port
        } else {
            config.port
        }));
        if via.is_some() {
            // Dial the tunnel's local end but keep `host` for TLS name checks.
            cfg.hostaddr(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        }
        if !config.user.is_empty() {
            cfg.user(&config.user);
        } else {
            // The conventional superuser of each product (root for CockroachDB, yugabyte, awsuser...).
            cfg.user(config.kind.info().user_placeholder);
        }
        if let Some(p) = &config.password {
            cfg.password(p);
        }
        if !config.database.is_empty() {
            cfg.dbname(&config.database);
        }
        cfg.application_name("Theta");
        cfg.connect_timeout(Duration::from_secs(10));
        cfg.keepalives(true);
        cfg.keepalives_idle(Duration::from_secs(60));
        let notices: Notices = Arc::default();
        let (client, version, cancel) = match config.ssl_mode {
            SslMode::Disable => {
                cfg.ssl_mode(tokio_postgres::config::SslMode::Disable);
                Self::establish(&cfg, NoTls, notices.clone()).await?
            }
            mode => {
                cfg.ssl_mode(match mode {
                    SslMode::Prefer => tokio_postgres::config::SslMode::Prefer,
                    _ => tokio_postgres::config::SslMode::Require,
                });
                let make = MakeRustlsConnect::new(tls::client_config(mode)?);
                Self::establish(&cfg, make, notices.clone()).await?
            }
        };
        let row = client
            .query_one("SELECT current_database()::text, current_user::text", &[])
            .await
            .map_err(connect_error)?;
        // `version()` names the product (`server_version` is a compatibility number on CockroachDB and
        // Redshift). TimescaleDB is an extension, so it is looked up; the probe is skipped on products
        // without `pg_extension` semantics because a failed statement would abort an open transaction.
        let banner: String = client
            .query_one("SELECT version()::text", &[])
            .await
            .map(|r| r.get(0))
            .unwrap_or_else(|_| version.clone());
        let timescale: Option<String> =
            if matches!(config.kind, DatabaseKind::Postgres | DatabaseKind::TimescaleDb) {
                client
                    .query_opt("SELECT extversion::text FROM pg_extension WHERE extname = 'timescaledb'", &[])
                    .await
                    .ok()
                    .flatten()
                    .map(|r| r.get(0))
            } else {
                None
            };
        let mut full_banner = banner.clone();
        if let Some(v) = &timescale {
            full_banner.push_str(&format!(", TimescaleDB {v}"));
        }
        let info = ServerInfo {
            version: describe_server(&banner, &version, timescale.as_deref()),
            current_database: row.get(0),
            current_user: row.get(1),
            banner: full_banner,
        };
        Ok(Self {
            kind: config.kind,
            session: Mutex::new(Some(Session { client, origins: HashMap::new() })),
            cancel,
            notices,
            running: AtomicBool::new(false),
            cancel_requested: AtomicBool::new(false),
            in_txn_cache: AtomicBool::new(false),
            info,
        })
    }

    async fn establish<T>(cfg: &Config, tls: T, notices: Notices) -> Result<(Client, String, CancelFn)>
    where
        T: tokio_postgres::tls::MakeTlsConnect<tokio_postgres::Socket> + Clone + Send + Sync + 'static,
        T::Stream: Send + 'static,
        T::TlsConnect: Send,
        <T::TlsConnect as tokio_postgres::tls::TlsConnect<tokio_postgres::Socket>>::Future: Send,
    {
        let (client, mut connection) = cfg.connect(tls.clone()).await.map_err(connect_error)?;
        let version = connection.parameter("server_version").unwrap_or_default().to_string();
        // The connection future must be polled for the client to make progress; we
        // also use it to capture NOTICE/WARNING messages, which are delivered here
        // and nowhere else.
        tokio::spawn(async move {
            let mut messages = futures_util::stream::poll_fn(move |cx| connection.poll_message(cx));
            while let Some(m) = messages.next().await {
                match m {
                    Ok(AsyncMessage::Notice(n)) => {
                        let mut text = format!("{}: {}", n.severity(), n.message());
                        if let Some(d) = n.detail() {
                            text.push_str(&format!("\nDETAIL: {d}"));
                        }
                        if let Some(h) = n.hint() {
                            text.push_str(&format!("\nHINT: {h}"));
                        }
                        notices.lock().unwrap().push(Notice { code: n.code().code().to_string(), text });
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        });
        let token = client.cancel_token();
        let cancel: CancelFn = Box::new(move || {
            let token = token.clone();
            let tls = tls.clone();
            Box::pin(async move { token.cancel_query(tls).await.map_err(|e| Error::Other(error_chain(&e))) })
        });
        Ok((client, version, cancel))
    }

    fn drain_notices(&self) -> Vec<String> {
        self.notices.lock().unwrap().drain(..).map(|n| n.text).collect()
    }

    /// Bookkeeping around one unit of work so `cancel` knows something is running.
    async fn run_guarded<T>(&self, f: impl AsyncFnOnce(&mut Session) -> Result<T>) -> Result<T> {
        let mut guard = self.session.lock().await;
        let sess = guard.as_mut().ok_or(Error::Closed)?;
        self.cancel_requested.store(false, Ordering::SeqCst);
        self.running.store(true, Ordering::SeqCst);
        let r = f(sess).await;
        self.running.store(false, Ordering::SeqCst);
        match r {
            Err(e) if is_cancel_error(&e) && self.cancel_requested.swap(false, Ordering::SeqCst) => {
                Err(Error::Cancelled)
            }
            other => other,
        }
    }

    async fn execute_inner(
        &self,
        sess: &mut Session,
        sql: &str,
        limit: Option<usize>,
    ) -> Result<StatementOutcome> {
        self.notices.lock().unwrap().clear();
        let started = Instant::now();
        let m = |e| query_error(e, sql, 0);
        let stmt = sess.client.prepare(sql).await.map_err(m)?;
        let dml = reports_rows_affected(sql);
        // Renames and drops change what a table OID's columns are called.
        if is_ddl(sql) {
            sess.origins.clear();
        }
        if stmt.columns().is_empty() {
            let n = sess.client.execute(&stmt, &[]).await.map_err(m)?;
            return Ok(StatementOutcome {
                result: None,
                rows_affected: dml.then_some(n),
                notices: self.drain_notices(),
                elapsed: started.elapsed(),
            });
        }
        let ncols = stmt.columns().len();
        let decode_row = |row: &tokio_postgres::Row| -> Vec<Value> {
            (0..ncols).map(|i| row.try_get::<_, types::Cell>(i).map(|c| c.0).unwrap_or(Value::Null)).collect()
        };
        let mut rows: Vec<Vec<Value>> = Vec::new();
        let mut truncated = false;
        let mut affected = None;
        match limit {
            Some(l) if !dml => {
                let tx = sess.client.transaction().await.map_err(m)?;
                // BEGIN inside a transaction only warns (25001); that tells us the
                // user's transaction is open and must not be committed by us.
                let nested = {
                    let mut n = self.notices.lock().unwrap();
                    let before = n.len();
                    n.retain(|x| x.code != SqlState::ACTIVE_SQL_TRANSACTION.code());
                    n.len() != before
                };
                let fetched: Result<(), tokio_postgres::Error> = async {
                    let portal = tx.bind(&stmt, &[]).await?;
                    let stream = tx
                        .query_portal_raw(&portal, (l.saturating_add(1)).min(i32::MAX as usize) as i32)
                        .await?;
                    pin_mut!(stream);
                    while let Some(row) = stream.try_next().await? {
                        if rows.len() >= l {
                            truncated = true;
                            break;
                        }
                        rows.push(decode_row(&row));
                    }
                    Ok(())
                }
                .await;
                if nested {
                    // Dropping a `Transaction` would roll back the user's transaction.
                    std::mem::forget(tx);
                } else if fetched.is_ok() {
                    tx.commit().await.map_err(m)?;
                } else {
                    let _ = tx.rollback().await;
                }
                fetched.map_err(m)?;
            }
            _ => {
                let no_params: [&(dyn ToSql + Sync); 0] = [];
                let stream = sess.client.query_raw(&stmt, no_params).await.map_err(m)?;
                pin_mut!(stream);
                while let Some(row) = stream.try_next().await.map_err(m)? {
                    if limit.is_none_or(|l| rows.len() < l) {
                        rows.push(decode_row(&row));
                    } else {
                        truncated = true;
                    }
                }
                if dml {
                    affected = stream.rows_affected();
                }
            }
        }
        let elapsed = started.elapsed();
        let notices = self.drain_notices();
        let mut columns: Vec<Column> = stmt
            .columns()
            .iter()
            .map(|c| Column {
                name: c.name().to_string(),
                type_name: types::display_type_name(c.type_(), c.type_modifier()),
                kind: types::type_kind(c.type_()),
                origin: None,
            })
            .collect();
        let wanted: Vec<(u32, i16)> =
            stmt.columns().iter().map(|c| (c.table_oid().unwrap_or(0), c.column_id().unwrap_or(0))).collect();
        // Redshift is PostgreSQL 8.0 inside: the `oid[]` parameter below is not available there, and a
        // failed statement would abort the user's open transaction. Its grid is read-only anyway.
        if self.kind != DatabaseKind::Redshift {
            resolve_origins(sess, &wanted, &mut columns).await;
        }
        Ok(StatementOutcome {
            result: Some(ResultSet { columns, rows, truncated }),
            rows_affected: affected,
            notices,
            elapsed,
        })
    }
}

/// Fill `Column::origin` from `pg_attribute`, caching per table OID. Failures are
/// swallowed: an editable grid is a nicety and must not fail a successful query.
async fn resolve_origins(sess: &mut Session, wanted: &[(u32, i16)], columns: &mut [Column]) {
    let missing: Vec<u32> = {
        let mut v: Vec<u32> =
            wanted.iter().map(|(o, _)| *o).filter(|o| *o != 0 && !sess.origins.contains_key(o)).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    if !missing.is_empty() {
        let sql = "SELECT c.oid, n.nspname::text, c.relname::text, a.attnum, a.attname::text \
                   FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                   JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped \
                   WHERE c.oid = ANY($1::oid[])";
        let Ok(rows) = sess.client.query(sql, &[&missing]).await else { return };
        let mut found: HashMap<u32, TableMeta> = HashMap::new();
        for r in rows {
            let oid: u32 = r.get(0);
            let meta = found.entry(oid).or_insert_with(|| TableMeta {
                schema: r.get(1),
                table: r.get(2),
                columns: HashMap::new(),
            });
            meta.columns.insert(r.get(3), r.get(4));
        }
        for oid in missing {
            sess.origins.insert(oid, found.remove(&oid));
        }
    }
    for (col, (oid, attnum)) in columns.iter_mut().zip(wanted) {
        if let Some(Some(meta)) = sess.origins.get(oid)
            && let Some(name) = meta.columns.get(attnum)
        {
            col.origin = Some(ColumnOrigin {
                schema: Some(meta.schema.clone()),
                table: meta.table.clone(),
                column: name.clone(),
            });
        }
    }
}

/// Whether the session is inside a transaction block (including an aborted one).
///
/// `now()` is frozen at transaction start while `statement_timestamp()` advances
/// with every statement; in autocommit mode the implicit transaction starts with
/// the statement, so they are equal. The simple-query protocol is used because
/// that is the path where the server sets the statement timestamp from the
/// incoming message, independent of any earlier pipelined work.
async fn query_in_transaction(client: &Client, kind: DatabaseKind) -> bool {
    // Redshift documents SYSDATE as the transaction's start and GETDATE() as the statement's; that is
    // the same trick (not exercised against a real cluster).
    if kind == DatabaseKind::CockroachDb {
        // CockroachDB's now() and statement_timestamp() differ even outside a transaction (the first
        // live run took a SAVEPOINT in autocommit because of it); it has a session variable for this.
        return match client.simple_query("SHOW transaction_status").await {
            Ok(msgs) => msgs.iter().any(
                |m| matches!(m, tokio_postgres::SimpleQueryMessage::Row(r) if r.get(0).is_some_and(|s| s != "NoTxn")),
            ),
            Err(e) => e.code() == Some(&SqlState::IN_FAILED_SQL_TRANSACTION),
        };
    }
    let probe = if kind == DatabaseKind::Redshift {
        "SELECT sysdate = getdate()"
    } else {
        "SELECT now() = statement_timestamp()"
    };
    match client.simple_query(probe).await {
        Ok(msgs) => msgs
            .iter()
            .any(|m| matches!(m, tokio_postgres::SimpleQueryMessage::Row(r) if r.get(0) == Some("f"))),
        // "current transaction is aborted" is itself proof of an open transaction.
        Err(e) => e.code() == Some(&SqlState::IN_FAILED_SQL_TRANSACTION),
    }
}

#[async_trait]
impl Connection for PgConnection {
    fn kind(&self) -> DatabaseKind {
        self.kind
    }

    async fn server_info(&self) -> Result<ServerInfo> {
        Ok(self.info.clone())
    }

    async fn execute(&self, sql: &str, options: &ExecOptions) -> Result<StatementOutcome> {
        self.run_guarded(async |sess| self.execute_inner(sess, sql, options.row_limit).await).await
    }

    async fn cancel(&self) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            self.cancel_requested.store(true, Ordering::SeqCst);
            (self.cancel)().await?;
        }
        Ok(())
    }

    async fn introspect(&self) -> Result<Catalog> {
        self.run_guarded(async |sess| introspect::introspect(&sess.client, self.kind).await).await
    }

    async fn explain(&self, sql: &str, analyze: bool) -> Result<Plan> {
        self.run_guarded(async |sess| {
            let client = &sess.client;
            // What each product can do (their documentation, not a guess about Postgres):
            // CockroachDB has no FORMAT JSON for EXPLAIN (a text tree, one row per line) and no BUFFERS;
            // Redshift has plain `EXPLAIN` only; YugabyteDB takes ANALYZE and JSON but its BUFFERS output
            // is not tracked, so it is left out.
            let (prefix, text_plan) = match (self.kind, analyze) {
                (DatabaseKind::Redshift, true) => {
                    return Err(Error::Unsupported("Redshift has no EXPLAIN ANALYZE".into()));
                }
                (DatabaseKind::Redshift, false) => ("EXPLAIN ", true),
                (DatabaseKind::CockroachDb, true) => ("EXPLAIN ANALYZE ", true),
                (DatabaseKind::CockroachDb, false) => ("EXPLAIN ", true),
                (DatabaseKind::YugabyteDb, true) => ("EXPLAIN (ANALYZE, FORMAT JSON) ", false),
                (_, true) => ("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) ", false),
                (_, false) => ("EXPLAIN (FORMAT JSON) ", false),
            };
            let sent = format!("{prefix}{sql}");
            let m = |e| query_error(e, &sent, prefix.len());
            // ANALYZE really runs the statement. Wrap it so writes vanish, and
            // use a savepoint when the user already has a transaction open so we
            // neither commit nor roll back their work.
            let (begin, end) = if !analyze {
                (None, None)
            } else if query_in_transaction(client, self.kind).await {
                (
                    Some("SAVEPOINT theta_explain"),
                    Some("ROLLBACK TO SAVEPOINT theta_explain; RELEASE SAVEPOINT theta_explain"),
                )
            } else {
                (Some("BEGIN"), Some("ROLLBACK"))
            };
            if let Some(b) = begin {
                client.batch_execute(b).await.map_err(|e| query_error(e, b, 0))?;
            }
            let result = client.query(&sent, &[]).await;
            if let Some(e) = end {
                let _ = client.batch_execute(e).await;
            }
            let rows = result.map_err(m)?;
            let cell = |r: &tokio_postgres::Row| r.try_get::<_, types::Cell>(0).ok().map(|c| c.0.display());
            if text_plan {
                let raw = rows.iter().filter_map(cell).collect::<Vec<_>>().join("\n");
                if raw.trim().is_empty() {
                    return Err(Error::other("EXPLAIN returned no rows"));
                }
                return if self.kind == DatabaseKind::Redshift {
                    plan_text::parse_redshift(&raw)
                } else {
                    plan_text::parse_cockroach(&raw, analyze)
                };
            }
            let raw = rows.first().and_then(cell).ok_or_else(|| Error::other("EXPLAIN returned no rows"))?;
            plan_pg::parse_plan(&raw, analyze)
        })
        .await
    }

    async fn in_transaction(&self) -> bool {
        match self.session.try_lock() {
            Ok(guard) => match guard.as_ref() {
                Some(s) => {
                    let v = query_in_transaction(&s.client, self.kind).await;
                    self.in_txn_cache.store(v, Ordering::SeqCst);
                    v
                }
                None => false,
            },
            Err(_) => self.in_txn_cache.load(Ordering::SeqCst),
        }
    }

    async fn close(&self) -> Result<()> {
        // A running statement holds the lock; stop it rather than wait it out.
        if self.running.load(Ordering::SeqCst) {
            let _ = self.cancel().await;
        }
        // Dropping the client sends Terminate and ends the connection task.
        self.session.lock().await.take();
        Ok(())
    }
}
