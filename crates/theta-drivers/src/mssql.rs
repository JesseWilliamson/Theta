//! SQL Server over TDS, on top of `tiberius` (rustls, no native TLS library).
//!
//! This driver has been compiled and its parsers unit-tested, but it has **not been run against a SQL
//! Server** (none can be installed on the development machine); it is exercised by the live test run in CI.
//!
//! Design notes:
//! * One TDS session per connection behind an async mutex, so statements run one at a time and keep their
//!   session state (`USE`, `SET`, temp tables, an open transaction).
//! * `tiberius` has no attention (query cancel) packet. `cancel` therefore opens a second session and runs
//!   `KILL <spid>`, which ends the running statement by ending its session; the statement answers
//!   `Cancelled` and the next one reconnects transparently (session state such as temp tables and an open
//!   transaction is gone, which the user guide says).
//! * `tiberius` reaches `todo!()` on a user-defined type column, and `geometry`/`geography` are CLR types.
//!   Such a statement is retried once with the spatial columns converted on the server
//!   (`.STAsBinary()` plus `.STSrid`, joined into EWKB here); see [`spatial_retry`].
//! * The row limit uses `SET ROWCOUNT` around plain `SELECT`s so the server stops producing rows.

use crate::plan_mssql;
use crate::util::{clock_text, date_text, is_ddl, leading_keyword, reports_rows_affected};
use async_trait::async_trait;
use futures_util::{FutureExt, TryStreamExt};
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant};
use theta_core::*;
use tiberius::{AuthMethod, Client, ColumnData, Config, EncryptionLevel, QueryItem};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

type Tds = Client<Compat<TcpStream>>;

pub struct SqlServerDriver;

#[async_trait]
impl Driver for SqlServerDriver {
    fn kind(&self) -> DatabaseKind {
        DatabaseKind::SqlServer
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>> {
        Ok(Box::new(SqlServerConnection::connect(config, None).await?))
    }
}

/// Where an SSH tunnel's local end listens.
pub struct LocalEndpoint {
    pub port: u16,
}

pub struct SqlServerConnection {
    config: ConnectionConfig,
    via: Option<u16>,
    session: Mutex<Option<Tds>>,
    spid: AtomicI32,
    running: AtomicBool,
    cancel_requested: AtomicBool,
    in_txn_cache: AtomicBool,
    info: ServerInfo,
}

fn tds_config(config: &ConnectionConfig) -> Config {
    let mut c = Config::new();
    c.host(if config.host.is_empty() { "localhost" } else { config.host.as_str() });
    c.port(if config.port == 0 { 1433 } else { config.port });
    if !config.database.is_empty() {
        c.database(&config.database);
    }
    c.application_name("Theta");
    c.authentication(AuthMethod::sql_server(
        if config.user.is_empty() { "sa" } else { config.user.as_str() },
        config.password.clone().unwrap_or_default(),
    ));
    match config.ssl_mode {
        // "Encrypt off": only the login is encrypted (the server may force more).
        SslMode::Disable => c.encryption(EncryptionLevel::Off),
        // "Encrypt on, trust server certificate": the usual setting for a server with its own certificate.
        SslMode::Prefer | SslMode::Require => {
            c.encryption(EncryptionLevel::Required);
            c.trust_cert();
        }
        SslMode::VerifyFull => c.encryption(EncryptionLevel::Required),
    }
    c
}

async fn open(config: &ConnectionConfig, via: Option<u16>) -> Result<Tds> {
    let cfg = tds_config(config);
    // Through a tunnel the TCP end is local while the configured host name stays for the TLS name check.
    let addr = match via {
        Some(p) => format!("127.0.0.1:{p}"),
        None => cfg.get_addr(),
    };
    let tcp = tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(&addr))
        .await
        .map_err(|_| Error::Connect(format!("timed out connecting to {addr}")))?
        .map_err(|e| Error::Connect(format!("{addr}: {e}")))?;
    let _ = tcp.set_nodelay(true);
    Client::connect(cfg, tcp.compat_write()).await.map_err(connect_error)
}

fn connect_error(e: tiberius::error::Error) -> Error {
    match &e {
        // 18456: login failed.
        tiberius::error::Error::Server(s) if s.code() == 18456 => Error::Auth(s.message().to_string()),
        tiberius::error::Error::Server(s) => Error::Connect(s.message().to_string()),
        other => Error::Connect(other.to_string()),
    }
}

fn query_error(e: tiberius::error::Error, sql: &str) -> Error {
    match &e {
        tiberius::error::Error::Server(s) => {
            // `line` is the line of the batch; the offset of its first character is the closest position
            // SQL Server gives (it has no column).
            let offset = (s.line() as usize).checked_sub(1).and_then(|n| {
                let mut at = 0;
                for (i, l) in sql.split_inclusive('\n').enumerate() {
                    if i == n {
                        return Some(at);
                    }
                    at += l.len();
                }
                None
            });
            Error::Query {
                message: s.message().to_string(),
                code: Some(s.code().to_string()),
                detail: Some(s.procedure().to_string()).filter(|p| !p.is_empty()).map(|p| format!("in {p}")),
                hint: None,
                position: offset.map(|offset| ErrorPosition { offset }),
            }
        }
        tiberius::error::Error::Io { .. } => Error::Closed,
        other => Error::Other(other.to_string()),
    }
}

// ------------------------------------------------------------------------------------------ values

/// Numeric text with its scale kept (`2.50` stays `2.50`).
fn decimal_text(value: i128, scale: u8) -> String {
    let neg = value < 0;
    let digits = value.unsigned_abs().to_string();
    let scale = scale as usize;
    let body = if scale == 0 {
        digits
    } else {
        let padded = format!("{digits:0>width$}", width = scale + 1);
        format!("{}.{}", &padded[..padded.len() - scale], &padded[padded.len() - scale..])
    };
    if neg { format!("-{body}") } else { body }
}

/// Ticks of `10^-scale` seconds since midnight to `HH:MM:SS[.fff]` with exactly `scale` digits (SQL Server
/// shows trailing zeros of a `time(n)`).
fn time_text(increments: u64, scale: u8) -> String {
    let unit = 10u64.pow(scale as u32);
    let (secs, frac) = (increments / unit, increments % unit);
    let base = clock_text(secs as i128 * 1_000_000);
    let base = base.split('.').next().unwrap_or(&base).to_string();
    if scale == 0 { base } else { format!("{base}.{frac:0>width$}", width = scale as usize) }
}

/// Days since 0001-01-01 (SQL Server's `date`) to text.
fn mssql_date(days: i64) -> String {
    date_text(days - 719_162)
}

fn convert(d: ColumnData<'_>) -> Value {
    match d {
        ColumnData::U8(v) => v.map_or(Value::Null, |v| Value::Int(v as i64)),
        ColumnData::I16(v) => v.map_or(Value::Null, |v| Value::Int(v as i64)),
        ColumnData::I32(v) => v.map_or(Value::Null, |v| Value::Int(v as i64)),
        ColumnData::I64(v) => v.map_or(Value::Null, Value::Int),
        ColumnData::F32(v) => {
            v.map_or(Value::Null, |v| Value::Float(v.to_string().parse().unwrap_or(v as f64)))
        }
        ColumnData::F64(v) => v.map_or(Value::Null, Value::Float),
        ColumnData::Bit(v) => v.map_or(Value::Null, Value::Bool),
        ColumnData::String(v) => v.map_or(Value::Null, |s| Value::Text(s.into_owned())),
        ColumnData::Guid(v) => v.map_or(Value::Null, |g| Value::Uuid(g.to_string().to_ascii_uppercase())),
        ColumnData::Binary(v) => v.map_or(Value::Null, |b| Value::Bytes(b.into_owned())),
        ColumnData::Numeric(v) => {
            v.map_or(Value::Null, |n| Value::Decimal(decimal_text(n.value(), n.scale())))
        }
        ColumnData::Xml(v) => v.map_or(Value::Null, |x| Value::Text(x.into_owned().into_string())),
        ColumnData::DateTime(v) => v.map_or(Value::Null, |d| {
            // Days since 1900-01-01 and 1/300-second ticks.
            let ms = (d.seconds_fragments() as u64 * 1000 + 150) / 300;
            Value::Timestamp(format!("{} {}", date_text(d.days() as i64 - 25_567), time_text(ms, 3)))
        }),
        ColumnData::SmallDateTime(v) => v.map_or(Value::Null, |d| {
            // Minutes since midnight.
            Value::Timestamp(format!(
                "{} {:02}:{:02}:00",
                date_text(d.days() as i64 - 25_567),
                d.seconds_fragments() / 60,
                d.seconds_fragments() % 60
            ))
        }),
        ColumnData::Time(v) => v.map_or(Value::Null, |t| Value::Time(time_text(t.increments(), t.scale()))),
        ColumnData::Date(v) => v.map_or(Value::Null, |d| Value::Date(mssql_date(d.days() as i64))),
        ColumnData::DateTime2(v) => v.map_or(Value::Null, |d| {
            Value::Timestamp(format!(
                "{} {}",
                mssql_date(d.date().days() as i64),
                time_text(d.time().increments(), d.time().scale())
            ))
        }),
        ColumnData::DateTimeOffset(v) => v.map_or(Value::Null, |o| {
            let dt = o.datetime2();
            let (increments, scale) = (dt.time().increments(), dt.time().scale());
            // The stored value is UTC; the offset says how to show it.
            let unit = 10u64.pow(scale as u32);
            let local = increments as i64 + o.offset() as i64 * 60 * unit as i64;
            let day = unit as i64 * 86_400;
            let days = dt.date().days() as i64 + local.div_euclid(day);
            let rem = local.rem_euclid(day) as u64;
            let off = o.offset();
            Value::TimestampTz(format!(
                "{} {} {}{:02}:{:02}",
                mssql_date(days),
                time_text(rem, scale),
                if off < 0 { '-' } else { '+' },
                off.abs() / 60,
                off.abs() % 60
            ))
        }),
    }
}

fn type_kind(t: tiberius::ColumnType) -> TypeKind {
    use tiberius::ColumnType as T;
    match t {
        T::Bit | T::Bitn => TypeKind::Bool,
        T::Int1 | T::Int2 | T::Int4 | T::Int8 | T::Intn => TypeKind::Integer,
        T::Float4 | T::Float8 | T::Floatn => TypeKind::Float,
        T::Money | T::Money4 | T::Decimaln | T::Numericn => TypeKind::Decimal,
        T::Guid => TypeKind::Uuid,
        T::Datetime4 | T::Datetime | T::Datetimen | T::Datetime2 | T::DatetimeOffsetn => TypeKind::Timestamp,
        T::Daten => TypeKind::Date,
        T::Timen => TypeKind::Time,
        T::BigVarBin | T::BigBinary | T::Image => TypeKind::Bytes,
        T::BigVarChar | T::BigChar | T::NVarchar | T::NChar | T::Text | T::NText => TypeKind::Text,
        T::Xml => TypeKind::Text,
        _ => TypeKind::Other,
    }
}

fn type_label(t: tiberius::ColumnType) -> String {
    use tiberius::ColumnType as T;
    match t {
        T::Bit | T::Bitn => "bit",
        T::Int1 => "tinyint",
        T::Int2 => "smallint",
        T::Int4 => "int",
        T::Int8 => "bigint",
        T::Intn => "int",
        T::Float4 => "real",
        T::Float8 | T::Floatn => "float",
        T::Money | T::Money4 => "money",
        T::Decimaln | T::Numericn => "decimal",
        T::Guid => "uniqueidentifier",
        T::Datetime4 => "smalldatetime",
        T::Datetime | T::Datetimen => "datetime",
        T::Datetime2 => "datetime2",
        T::DatetimeOffsetn => "datetimeoffset",
        T::Daten => "date",
        T::Timen => "time",
        T::BigVarBin => "varbinary",
        T::BigBinary => "binary",
        T::Image => "image",
        T::BigVarChar => "varchar",
        T::BigChar => "char",
        T::NVarchar => "nvarchar",
        T::NChar => "nchar",
        T::Text => "text",
        T::NText => "ntext",
        T::Xml => "xml",
        _ => "sql_variant",
    }
    .to_string()
}

// ----------------------------------------------------------------------------------------- spatial

/// Insert an SRID into plain WKB, giving EWKB: set the 0x20000000 flag in the geometry type word and
/// put the 4-byte SRID right after it, in the byte order the WKB declares.
pub(crate) fn wkb_with_srid(wkb: &[u8], srid: i32) -> Vec<u8> {
    if wkb.len() < 5 || srid == 0 {
        return wkb.to_vec();
    }
    let little = wkb[0] == 1;
    let ty = if little {
        u32::from_le_bytes(wkb[1..5].try_into().unwrap())
    } else {
        u32::from_be_bytes(wkb[1..5].try_into().unwrap())
    };
    if ty & 0x2000_0000 != 0 {
        return wkb.to_vec();
    }
    let (ty, srid) = (ty | 0x2000_0000, srid as u32);
    let mut out = Vec::with_capacity(wkb.len() + 4);
    out.push(wkb[0]);
    if little {
        out.extend_from_slice(&ty.to_le_bytes());
        out.extend_from_slice(&srid.to_le_bytes());
    } else {
        out.extend_from_slice(&ty.to_be_bytes());
        out.extend_from_slice(&srid.to_be_bytes());
    }
    out.extend_from_slice(&wkb[5..]);
    out
}

/// The statement with its spatial columns converted on the server, plus the plan for reading the result:
/// for each output column `Some(srid_column)` when it is spatial.
struct Rewritten {
    sql: String,
    /// Output column names of the original statement, in order.
    names: Vec<String>,
    /// For each original column: the spatial type name when it is one.
    spatial: Vec<Option<String>>,
}

fn quote(name: &str) -> String {
    format!("[{}]", name.replace(']', "]]"))
}

/// Build the rewritten statement from `sp_describe_first_result_set` rows `(name, system_type_name)`.
fn rewrite_spatial(sql: &str, columns: &[(String, String)]) -> Option<Rewritten> {
    let is_spatial = |t: &str| matches!(t.trim().to_ascii_lowercase().as_str(), "geometry" | "geography");
    if !columns.iter().any(|(_, t)| is_spatial(t)) {
        return None;
    }
    let mut select = Vec::new();
    let mut spatial = Vec::new();
    for (i, (name, ty)) in columns.iter().enumerate() {
        let q = quote(name);
        if is_spatial(ty) {
            select.push(format!("{q}.STAsBinary() AS {q}"));
            select.push(format!("{q}.STSrid AS [theta_srid_{i}]"));
            spatial.push(Some(ty.trim().to_string()));
        } else {
            select.push(q);
            spatial.push(None);
        }
    }
    Some(Rewritten {
        sql: format!("SELECT {} FROM ({}) AS [theta_q]", select.join(", "), sql.trim().trim_end_matches(';')),
        names: columns.iter().map(|(n, _)| n.clone()).collect(),
        spatial,
    })
}

// ---------------------------------------------------------------------------------------- execution

struct Fetched {
    columns: Vec<Column>,
    rows: Vec<Vec<Value>>,
    truncated: bool,
}

/// Read the first result set of `sql` (any further ones are counted, not shown).
async fn fetch(client: &mut Tds, sql: &str, limit: Option<usize>) -> Result<(Option<Fetched>, usize)> {
    let mut stream = client.simple_query(sql).await.map_err(|e| query_error(e, sql))?;
    let mut sets = 0usize;
    let mut cur: Option<Fetched> = None;
    let mut first: Option<Fetched> = None;
    while let Some(item) = stream.try_next().await.map_err(|e| query_error(e, sql))? {
        match item {
            QueryItem::Metadata(m) => {
                if let Some(done) = cur.take() {
                    first.get_or_insert(done);
                }
                sets += 1;
                cur = Some(Fetched {
                    columns: m
                        .columns()
                        .iter()
                        .map(|c| Column {
                            name: c.name().to_string(),
                            type_name: type_label(c.column_type()),
                            kind: type_kind(c.column_type()),
                            origin: None,
                        })
                        .collect(),
                    rows: Vec::new(),
                    truncated: false,
                });
            }
            QueryItem::Row(r) => {
                // Only the first result set with columns is kept.
                if first.is_some() {
                    continue;
                }
                if let Some(c) = cur.as_mut() {
                    if limit.is_some_and(|l| c.rows.len() >= l) {
                        c.truncated = true;
                        continue;
                    }
                    c.rows.push(r.into_iter().map(convert).collect());
                }
            }
        }
    }
    let result = first.or(cur).filter(|f| !f.columns.is_empty());
    Ok((result, sets))
}

/// The retry for a statement whose result has `geometry` / `geography` columns.
async fn spatial_retry(client: &mut Tds, sql: &str, limit: Option<usize>) -> Result<Option<Fetched>> {
    let describe = format!("EXEC sp_describe_first_result_set @tsql = N'{}'", sql.replace('\'', "''"));
    let mut stream = client.simple_query(describe.as_str()).await.map_err(|e| query_error(e, sql))?;
    let mut name_ix = None;
    let mut type_ix = None;
    let mut cols: Vec<(String, String)> = Vec::new();
    while let Some(item) = stream.try_next().await.map_err(|e| query_error(e, sql))? {
        match item {
            QueryItem::Metadata(m) => {
                name_ix = m.columns().iter().position(|c| c.name() == "name");
                type_ix = m.columns().iter().position(|c| c.name() == "system_type_name");
            }
            QueryItem::Row(r) => {
                let cells: Vec<ColumnData<'_>> = r.into_iter().collect();
                let text = |ix: Option<usize>| match ix.and_then(|i| cells.get(i)) {
                    Some(ColumnData::String(Some(s))) => s.to_string(),
                    _ => String::new(),
                };
                cols.push((text(name_ix), text(type_ix)));
            }
        }
    }
    drop(stream);
    let Some(rw) = rewrite_spatial(sql, &cols) else { return Ok(None) };
    let (fetched, _) = fetch(client, &rw.sql, limit).await?;
    let Some(f) = fetched else { return Ok(None) };
    // The converted statement returns `[col, srid]` pairs for spatial columns; fold each pair into one
    // EWKB value and name the column as the original did.
    let mut columns = Vec::new();
    let mut plan: Vec<(usize, Option<usize>)> = Vec::new();
    let mut at = 0;
    for (i, name) in rw.names.iter().enumerate() {
        match &rw.spatial[i] {
            Some(ty) => {
                columns.push(Column {
                    name: name.clone(),
                    type_name: ty.to_ascii_lowercase(),
                    kind: TypeKind::Geometry,
                    origin: None,
                });
                plan.push((at, Some(at + 1)));
                at += 2;
            }
            None => {
                columns.push(f.columns.get(at).cloned().unwrap_or(Column {
                    name: name.clone(),
                    type_name: String::new(),
                    kind: TypeKind::Other,
                    origin: None,
                }));
                plan.push((at, None));
                at += 1;
            }
        }
    }
    let rows = f
        .rows
        .into_iter()
        .map(|row| {
            plan.iter()
                .map(|&(v, srid)| match (row.get(v), srid.and_then(|s| row.get(s))) {
                    (Some(Value::Bytes(wkb)), Some(Value::Int(srid))) => {
                        Value::Geometry(wkb_with_srid(wkb, *srid as i32))
                    }
                    (Some(Value::Bytes(wkb)), _) => Value::Geometry(wkb.clone()),
                    (Some(other), _) => other.clone(),
                    _ => Value::Null,
                })
                .collect()
        })
        .collect();
    Ok(Some(Fetched { columns, rows, truncated: f.truncated }))
}

/// Whether a panic payload is `tiberius`'s `todo!("User-defined types not supported")`.
fn is_udt_panic(payload: &(dyn std::any::Any + Send)) -> bool {
    let text = payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned());
    text.is_some_and(|t| t.contains("User-defined types"))
}

impl SqlServerConnection {
    pub async fn connect(config: &ConnectionConfig, via: Option<LocalEndpoint>) -> Result<Self> {
        let via = via.map(|v| v.port);
        let mut client = open(config, via).await?;
        let (banner, db, user, spid) = {
            let (fetched, _) = fetch(
                &mut client,
                "SELECT @@VERSION, DB_NAME(), SUSER_SNAME(), CAST(@@SPID AS int), \
                        CAST(SERVERPROPERTY('ProductVersion') AS nvarchar(50)), CAST(SERVERPROPERTY('Edition') AS nvarchar(100))",
                None,
            )
            .await?;
            let row = fetched.and_then(|f| f.rows.into_iter().next()).unwrap_or_default();
            let t = |i: usize| row.get(i).map(|v| v.display()).unwrap_or_default();
            let spid = row
                .get(3)
                .and_then(|v| if let Value::Int(i) = v { Some(*i as i32) } else { None })
                .unwrap_or(0);
            (t(0), t(1), t(2), (spid, t(4), t(5)))
        };
        let (spid, product_version, edition) = spid;
        // `Microsoft SQL Server 2022 (RTM-CU14) (KB5038325) - 16.0.4135.4 (X64) ...`
        let year = banner
            .split_whitespace()
            .find(|w| w.len() == 4 && w.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or("");
        let info = ServerInfo {
            version: format!("SQL Server {year} {product_version} ({edition})").replace("  ", " "),
            current_database: Some(db),
            current_user: Some(user),
            banner: banner.lines().next().unwrap_or("").to_string(),
        };
        Ok(Self {
            config: config.clone(),
            via,
            session: Mutex::new(Some(client)),
            spid: AtomicI32::new(spid),
            running: AtomicBool::new(false),
            cancel_requested: AtomicBool::new(false),
            in_txn_cache: AtomicBool::new(false),
            info,
        })
    }

    /// The session, reconnecting first when a cancel or a lost socket ended the previous one.
    async fn with_session<T>(&self, f: impl AsyncFnOnce(&mut Tds) -> Result<T>) -> Result<T> {
        let mut guard = self.session.lock().await;
        if guard.is_none() {
            let c = open(&self.config, self.via).await?;
            *guard = Some(c);
            self.refresh_spid(guard.as_mut().unwrap()).await;
        }
        self.cancel_requested.store(false, Ordering::SeqCst);
        self.running.store(true, Ordering::SeqCst);
        let r = f(guard.as_mut().unwrap()).await;
        self.running.store(false, Ordering::SeqCst);
        let cancelled = self.cancel_requested.swap(false, Ordering::SeqCst);
        match r {
            // The cancel killed the session: the statement ended with a transport error. Drop the
            // session so the next statement opens a new one.
            Err(Error::Closed | Error::Other(_)) if cancelled => {
                *guard = None;
                Err(Error::Cancelled)
            }
            Err(Error::Closed) => {
                *guard = None;
                Err(Error::Closed)
            }
            other => other,
        }
    }

    async fn refresh_spid(&self, c: &mut Tds) {
        if let Ok((Some(f), _)) = fetch(c, "SELECT CAST(@@SPID AS int)", None).await
            && let Some(Value::Int(i)) = f.rows.first().and_then(|r| r.first())
        {
            self.spid.store(*i as i32, Ordering::SeqCst);
        }
    }

    async fn run(&self, client: &mut Tds, sql: &str, limit: Option<usize>) -> Result<StatementOutcome> {
        let started = Instant::now();
        let kw = leading_keyword(sql);
        let dml = reports_rows_affected(sql) && !sql.to_ascii_uppercase().contains("OUTPUT");
        if is_ddl(sql) && !dml {
            // DDL goes as a plain batch. `execute` wraps the text in an `sp_executesql` call, and
            // CREATE VIEW / PROCEDURE / FUNCTION / TRIGGER must be the first statement of their batch:
            // SQL Server 2022 answered the wrapped form with "Incorrect syntax near the keyword 'VIEW'".
            client
                .simple_query(sql)
                .await
                .map_err(|e| query_error(e, sql))?
                .into_results()
                .await
                .map_err(|e| query_error(e, sql))?;
            return Ok(StatementOutcome {
                result: None,
                rows_affected: None,
                notices: Vec::new(),
                elapsed: started.elapsed(),
            });
        }
        if dml {
            // `execute` reports the row count of DML.
            let r = client.execute(sql, &[]).await.map_err(|e| query_error(e, sql))?;
            let n = dml.then(|| r.total());
            return Ok(StatementOutcome {
                result: None,
                rows_affected: n,
                notices: Vec::new(),
                elapsed: started.elapsed(),
            });
        }
        // A plain SELECT is capped on the server; SET ROWCOUNT is reset whatever happens.
        let capped = limit.filter(|_| {
            matches!(kw.as_str(), "SELECT" | "WITH") && !sql.to_ascii_uppercase().contains(" INTO ")
        });
        if let Some(l) = capped {
            client
                .simple_query(format!("SET ROWCOUNT {}", l + 1).as_str())
                .await
                .map_err(|e| query_error(e, sql))?
                .into_results()
                .await
                .map_err(|e| query_error(e, sql))?;
        }
        let attempt = AssertUnwindSafe(fetch(client, sql, limit)).catch_unwind().await;
        if capped.is_some() {
            let _ = async { client.simple_query("SET ROWCOUNT 0").await?.into_results().await }.await;
        }
        let (fetched, sets) = match attempt {
            Ok(r) => r?,
            Err(payload) if is_udt_panic(payload.as_ref()) => {
                // The panic left the response half read; this session is unusable. A new one reruns the
                // statement with the spatial columns converted.
                return Err(Error::Closed);
            }
            Err(payload) => std::panic::resume_unwind(payload),
        };
        let mut notices = Vec::new();
        if sets > 1 {
            notices.push(format!("The statement returned {sets} result sets; only the first is shown."));
        }
        let result = fetched.map(|f| ResultSet { columns: f.columns, rows: f.rows, truncated: f.truncated });
        Ok(StatementOutcome { result, rows_affected: None, notices, elapsed: started.elapsed() })
    }
}

#[async_trait]
impl Connection for SqlServerConnection {
    fn kind(&self) -> DatabaseKind {
        DatabaseKind::SqlServer
    }

    async fn server_info(&self) -> Result<ServerInfo> {
        Ok(self.info.clone())
    }

    async fn execute(&self, sql: &str, options: &ExecOptions) -> Result<StatementOutcome> {
        let first = self.with_session(async |c| self.run(c, sql, options.row_limit).await).await;
        match first {
            // The UDT panic ended the session: open another and rerun with spatial columns converted.
            Err(Error::Closed) if self.session.lock().await.is_none() => {
                let started = Instant::now();
                let retried =
                    self.with_session(async |c| spatial_retry(c, sql, options.row_limit).await).await?;
                match retried {
                    Some(f) => Ok(StatementOutcome {
                        result: Some(ResultSet { columns: f.columns, rows: f.rows, truncated: f.truncated }),
                        rows_affected: None,
                        notices: Vec::new(),
                        elapsed: started.elapsed(),
                    }),
                    None => Err(Error::Query {
                        message: "This result has a column type Theta cannot read (a CLR user-defined type). Select it as text or binary, e.g. col.ToString() or CAST(col AS varbinary(max)).".into(),
                        code: None,
                        detail: None,
                        hint: None,
                        position: None,
                    }),
                }
            }
            other => other,
        }
    }

    async fn cancel(&self) -> Result<()> {
        if !self.running.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.cancel_requested.store(true, Ordering::SeqCst);
        let spid = self.spid.load(Ordering::SeqCst);
        // `KILL` ends the running statement by ending its session; a second session does it.
        let mut killer = open(&self.config, self.via).await?;
        let r = run_batch(&mut killer, &format!("KILL {spid}")).await;
        let _ = killer.close().await;
        r
    }

    async fn introspect(&self) -> Result<Catalog> {
        self.with_session(async |c| introspect(c).await).await
    }

    async fn explain(&self, sql: &str, analyze: bool) -> Result<Plan> {
        self.with_session(async |c| {
            // Estimated: SHOWPLAN_XML returns the plan instead of running the statement. Actual:
            // STATISTICS XML runs it and sends the plan as an extra result set after the rows. Both
            // are session settings, switched back off whatever the outcome; SHOWPLAN_XML has to be the
            // only statement of its batch.
            let in_txn = self.txn_open(c).await;
            let (setting, wrap_begin, wrap_end) = if analyze {
                let (b, e) = if in_txn {
                    ("SAVE TRANSACTION theta_explain", "ROLLBACK TRANSACTION theta_explain")
                } else {
                    ("BEGIN TRANSACTION", "ROLLBACK TRANSACTION")
                };
                ("SET STATISTICS XML ON", Some(b), Some(e))
            } else {
                ("SET SHOWPLAN_XML ON", None, None)
            };
            let off = if analyze { "SET STATISTICS XML OFF" } else { "SET SHOWPLAN_XML OFF" };
            run_batch(c, setting).await?;
            if let Some(b) = wrap_begin {
                run_batch(c, b).await?;
            }
            let result = xml_results(c, sql).await;
            if let Some(e) = wrap_end {
                let _ = run_batch(c, e).await;
            }
            let _ = run_batch(c, off).await;
            let xmls = result?;
            // The estimated plan is the only result; the actual one is the last result set of the batch.
            let xml = xmls.last().ok_or_else(|| Error::other("the server returned no plan"))?;
            plan_mssql::parse(xml, analyze)
        })
        .await
    }

    async fn in_transaction(&self) -> bool {
        match self.session.try_lock() {
            Ok(mut g) => match g.as_mut() {
                Some(c) => {
                    let v = self.txn_open(c).await;
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
        if let Some(c) = self.session.lock().await.take() {
            let _ = c.close().await;
        }
        Ok(())
    }
}

impl SqlServerConnection {
    async fn txn_open(&self, c: &mut Tds) -> bool {
        matches!(
            fetch(c, "SELECT @@TRANCOUNT", None).await,
            Ok((Some(f), _)) if matches!(f.rows.first().and_then(|r| r.first()), Some(Value::Int(n)) if *n > 0)
        )
    }
}

async fn run_batch(c: &mut Tds, sql: &str) -> Result<()> {
    c.simple_query(sql)
        .await
        .map_err(|e| query_error(e, sql))?
        .into_results()
        .await
        .map_err(|e| query_error(e, sql))?;
    Ok(())
}

/// Every single-column result set of `sql` whose first cell is an XML document (a showplan).
async fn xml_results(c: &mut Tds, sql: &str) -> Result<Vec<String>> {
    let mut stream = c.simple_query(sql).await.map_err(|e| query_error(e, sql))?;
    let mut out = Vec::new();
    while let Some(item) = stream.try_next().await.map_err(|e| query_error(e, sql))? {
        if let QueryItem::Row(r) = item {
            for cell in r {
                match cell {
                    ColumnData::Xml(Some(x)) => out.push(x.into_owned().into_string()),
                    ColumnData::String(Some(s)) if s.trim_start().starts_with("<ShowPlanXML") => {
                        out.push(s.into_owned())
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------------------------ introspection

async fn table(c: &mut Tds, sql: &str) -> Result<Vec<Vec<Value>>> {
    Ok(fetch(c, sql, None).await?.0.map(|f| f.rows).unwrap_or_default())
}

fn t(row: &[Value], i: usize) -> String {
    row.get(i).map(|v| v.display()).unwrap_or_default()
}

fn o(row: &[Value], i: usize) -> Option<String> {
    row.get(i).filter(|v| !v.is_null()).map(|v| v.display()).filter(|s| !s.is_empty())
}

fn b(row: &[Value], i: usize) -> bool {
    matches!(row.get(i), Some(Value::Bool(true)) | Some(Value::Int(1)))
}

fn action(a: String) -> Option<String> {
    match a.as_str() {
        "CASCADE" => Some("CASCADE".into()),
        "SET_NULL" => Some("SET NULL".into()),
        "SET_DEFAULT" => Some("SET DEFAULT".into()),
        _ => None,
    }
}

async fn introspect(c: &mut Tds) -> Result<Catalog> {
    let mut schemas: Vec<Schema> = table(
        c,
        "SELECT name FROM sys.schemas WHERE name NOT IN ('sys', 'INFORMATION_SCHEMA', 'guest') \
         AND schema_id NOT BETWEEN 16384 AND 16399 ORDER BY name",
    )
    .await?
    .iter()
    .map(|r| Schema { name: t(r, 0), ..Default::default() })
    .collect();
    let sidx: HashMap<String, usize> = schemas.iter().enumerate().map(|(i, s)| (s.name.clone(), i)).collect();
    let mut by_id: HashMap<i64, (usize, usize)> = HashMap::new();
    let id = |r: &[Value], i: usize| match r.get(i) {
        Some(Value::Int(n)) => *n,
        _ => 0,
    };

    // Tables and views, row counts from `sys.partitions` (needs no special permission), view text and
    // the MS_Description extended property as the comment.
    for r in table(
        c,
        "SELECT o.object_id, s.name, o.name, o.type, \
                (SELECT SUM(CAST(p.rows AS bigint)) FROM sys.partitions p WHERE p.object_id = o.object_id AND p.index_id IN (0, 1)), \
                CAST(ep.value AS nvarchar(4000)), OBJECT_DEFINITION(o.object_id) \
         FROM sys.objects o JOIN sys.schemas s ON s.schema_id = o.schema_id \
         LEFT JOIN sys.extended_properties ep ON ep.major_id = o.object_id AND ep.minor_id = 0 AND ep.class = 1 AND ep.name = 'MS_Description' \
         WHERE o.type IN ('U', 'V') AND o.is_ms_shipped = 0 ORDER BY s.name, o.name",
    )
    .await?
    {
        let Some(&si) = sidx.get(&t(&r, 1)) else { continue };
        let view = t(&r, 3).trim() == "V";
        schemas[si].tables.push(Table {
            name: t(&r, 2),
            kind: if view { TableKind::View } else { TableKind::Table },
            row_estimate: match r.get(4) {
                Some(Value::Int(n)) => u64::try_from(*n).ok(),
                _ => None,
            },
            comment: o(&r, 5),
            view_definition: if view { o(&r, 6) } else { None },
            ..Default::default()
        });
        by_id.insert(id(&r, 0), (si, schemas[si].tables.len() - 1));
    }

    for r in table(
        c,
        "SELECT c.object_id, c.name, TYPE_NAME(c.user_type_id), TYPE_NAME(c.system_type_id), c.max_length, c.precision, c.scale, \
                c.is_nullable, c.is_identity, OBJECT_DEFINITION(c.default_object_id), cc.definition, cc.is_persisted, \
                CAST(ep.value AS nvarchar(4000)) \
         FROM sys.columns c \
         JOIN sys.objects o ON o.object_id = c.object_id AND o.type IN ('U', 'V') AND o.is_ms_shipped = 0 \
         LEFT JOIN sys.computed_columns cc ON cc.object_id = c.object_id AND cc.column_id = c.column_id \
         LEFT JOIN sys.extended_properties ep ON ep.major_id = c.object_id AND ep.minor_id = c.column_id AND ep.class = 1 AND ep.name = 'MS_Description' \
         ORDER BY c.object_id, c.column_id",
    )
    .await?
    {
        let Some(&(si, ti)) = by_id.get(&id(&r, 0)) else { continue };
        let (user_type, base) = (t(&r, 2), t(&r, 3));
        let len = id(&r, 4);
        let data_type = match base.as_str() {
            "varchar" | "char" | "varbinary" | "binary" => {
                format!("{user_type}({})", if len == -1 { "max".to_string() } else { len.to_string() })
            }
            "nvarchar" | "nchar" => format!("{user_type}({})", if len == -1 { "max".to_string() } else { (len / 2).to_string() }),
            "decimal" | "numeric" => format!("{user_type}({},{})", id(&r, 5), id(&r, 6)),
            "datetime2" | "datetimeoffset" | "time" => format!("{user_type}({})", id(&r, 6)),
            _ => user_type,
        };
        let geometry = matches!(base.as_str(), "geometry" | "geography").then(|| GeometryColumn {
            geometry_type: "GEOMETRY".into(),
            srid: None,
            geography: base == "geography",
        });
        // `((0))` / `(getdate())`: SQL Server wraps stored defaults in parentheses.
        let default = o(&r, 9).map(|d| {
            let mut d = d.as_str();
            while d.starts_with('(') && d.ends_with(')') && d.len() > 2 {
                d = &d[1..d.len() - 1];
            }
            d.to_string()
        });
        schemas[si].tables[ti].columns.push(ColumnInfo {
            name: t(&r, 1),
            kind: if geometry.is_some() { TypeKind::Geometry } else { crate::util::kind_from_type_name(&base) },
            data_type,
            nullable: b(&r, 7),
            default,
            comment: o(&r, 12),
            geometry,
            identity: b(&r, 8).then_some(Identity::ByDefault),
            generated: o(&r, 10).map(|expression| GeneratedColumn { expression, stored: b(&r, 11) }),
        });
    }

    // Keys and indexes in one pass: key columns in key order, included columns left out.
    for r in table(
        c,
        "SELECT i.object_id, i.name, i.type_desc, i.is_unique, i.is_primary_key, i.is_unique_constraint, i.filter_definition, \
                (SELECT STRING_AGG(CAST(col.name AS nvarchar(max)), ',') WITHIN GROUP (ORDER BY ic.key_ordinal) \
                   FROM sys.index_columns ic JOIN sys.columns col ON col.object_id = ic.object_id AND col.column_id = ic.column_id \
                  WHERE ic.object_id = i.object_id AND ic.index_id = i.index_id AND ic.is_included_column = 0 AND ic.key_ordinal > 0) \
         FROM sys.indexes i JOIN sys.objects o ON o.object_id = i.object_id AND o.type IN ('U', 'V') AND o.is_ms_shipped = 0 \
         WHERE i.index_id > 0 AND i.name IS NOT NULL ORDER BY i.object_id, i.name",
    )
    .await?
    {
        let Some(&(si, ti)) = by_id.get(&id(&r, 0)) else { continue };
        let cols: Vec<String> = t(&r, 7).split(',').filter(|s| !s.is_empty()).map(str::to_string).collect();
        let table = &mut schemas[si].tables[ti];
        if b(&r, 4) {
            table.primary_key = cols.clone();
        } else if b(&r, 5) {
            table.unique_constraints.push(UniqueConstraint { name: t(&r, 1), columns: cols.clone() });
        }
        table.indexes.push(Index {
            name: t(&r, 1),
            unique: b(&r, 3),
            primary: b(&r, 4),
            method: o(&r, 2),
            definition: None,
            columns: cols,
            predicate: o(&r, 6),
            has_expressions: false,
        });
    }

    for r in table(
        c,
        "SELECT fk.parent_object_id, fk.name, ps.name, pt.name, rs.name, rt.name, \
                (SELECT STRING_AGG(CAST(pc.name AS nvarchar(max)), ',') WITHIN GROUP (ORDER BY fkc.constraint_column_id) \
                   FROM sys.foreign_key_columns fkc JOIN sys.columns pc ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id \
                  WHERE fkc.constraint_object_id = fk.object_id), \
                (SELECT STRING_AGG(CAST(rc.name AS nvarchar(max)), ',') WITHIN GROUP (ORDER BY fkc.constraint_column_id) \
                   FROM sys.foreign_key_columns fkc JOIN sys.columns rc ON rc.object_id = fkc.referenced_object_id AND rc.column_id = fkc.referenced_column_id \
                  WHERE fkc.constraint_object_id = fk.object_id), \
                fk.delete_referential_action_desc, fk.update_referential_action_desc \
         FROM sys.foreign_keys fk \
         JOIN sys.tables pt ON pt.object_id = fk.parent_object_id JOIN sys.schemas ps ON ps.schema_id = pt.schema_id \
         JOIN sys.tables rt ON rt.object_id = fk.referenced_object_id JOIN sys.schemas rs ON rs.schema_id = rt.schema_id \
         ORDER BY fk.parent_object_id, fk.name",
    )
    .await?
    {
        let Some(&(si, ti)) = by_id.get(&id(&r, 0)) else { continue };
        let split = |i: usize| t(&r, i).split(',').filter(|s| !s.is_empty()).map(str::to_string).collect::<Vec<_>>();
        schemas[si].tables[ti].foreign_keys.push(ForeignKey {
            name: t(&r, 1),
            columns: split(6),
            referenced_schema: Some(t(&r, 4)),
            referenced_table: t(&r, 5),
            referenced_columns: split(7),
            on_delete: action(t(&r, 8)),
            on_update: action(t(&r, 9)),
            ..Default::default()
        });
    }

    for r in table(c, "SELECT parent_object_id, name, definition FROM sys.check_constraints ORDER BY parent_object_id, name").await? {
        let Some(&(si, ti)) = by_id.get(&id(&r, 0)) else { continue };
        let def = t(&r, 2);
        // The stored text is parenthesised: `([price]>(0))`.
        let inner = def.strip_prefix('(').and_then(|d| d.strip_suffix(')')).unwrap_or(&def).to_string();
        schemas[si].tables[ti].checks.push(CheckConstraint { name: t(&r, 1), expression: inner });
    }

    for r in table(
        c,
        "SELECT s.name, o.name, o.type, \
                (SELECT STRING_AGG(CONCAT(p.name, ' ', TYPE_NAME(p.user_type_id)), ', ') WITHIN GROUP (ORDER BY p.parameter_id) \
                   FROM sys.parameters p WHERE p.object_id = o.object_id AND p.parameter_id > 0), \
                (SELECT TYPE_NAME(p.user_type_id) FROM sys.parameters p WHERE p.object_id = o.object_id AND p.parameter_id = 0) \
         FROM sys.objects o JOIN sys.schemas s ON s.schema_id = o.schema_id \
         WHERE o.type IN ('P', 'FN', 'IF', 'TF', 'AF', 'PC', 'FS', 'FT') AND o.is_ms_shipped = 0 ORDER BY s.name, o.name",
    )
    .await?
    {
        let Some(&si) = sidx.get(&t(&r, 0)) else { continue };
        let ty = t(&r, 2);
        let ty = ty.trim();
        schemas[si].routines.push(Routine {
            name: t(&r, 1),
            kind: match ty {
                "P" | "PC" => RoutineKind::Procedure,
                "AF" => RoutineKind::Aggregate,
                _ => RoutineKind::Function,
            },
            signature: format!("({})", t(&r, 3)),
            return_type: o(&r, 4),
        });
    }
    for r in table(c, "SELECT s.name, q.name FROM sys.sequences q JOIN sys.schemas s ON s.schema_id = q.schema_id ORDER BY 1, 2").await? {
        if let Some(&si) = sidx.get(&t(&r, 0)) {
            schemas[si].sequences.push(t(&r, 1));
        }
    }
    let default_schema = table(c, "SELECT SCHEMA_NAME()").await?.first().and_then(|r| o(r, 0));
    Ok(Catalog { schemas, default_schema })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_keep_their_scale() {
        assert_eq!(decimal_text(250, 2), "2.50");
        assert_eq!(decimal_text(-5, 3), "-0.005");
        assert_eq!(decimal_text(42, 0), "42");
        assert_eq!(decimal_text(0, 2), "0.00");
    }

    #[test]
    fn temporal_text() {
        assert_eq!(mssql_date(719_162), "1970-01-01");
        assert_eq!(mssql_date(0), "0001-01-01");
        assert_eq!(time_text(13 * 3600 * 10_000_000 + 5_000_000, 7), "13:00:00.5000000");
        assert_eq!(time_text(61, 0), "00:01:01");
        // 1900-01-01 is 25567 days before the epoch
        assert_eq!(date_text(-25_567), "1900-01-01");
    }

    #[test]
    fn ewkb_from_wkb_and_srid() {
        // little-endian POINT(1 2)
        let wkb = [1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xf0, 0x3f, 0, 0, 0, 0, 0, 0, 0, 0x40];
        let e = wkb_with_srid(&wkb, 4326);
        assert_eq!(&e[..9], &[1, 1, 0, 0, 0x20, 0xe6, 0x10, 0, 0]);
        assert_eq!(&e[9..], &wkb[5..]);
        // already EWKB, or no SRID: unchanged
        assert_eq!(wkb_with_srid(&e, 4326), e);
        assert_eq!(wkb_with_srid(&wkb, 0), wkb.to_vec());
        // big-endian
        let be = [0, 0, 0, 0, 1, 0x3f, 0xf0, 0, 0, 0, 0, 0, 0, 0x40, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(&wkb_with_srid(&be, 4326)[..9], &[0, 0x20, 0, 0, 1, 0, 0, 0x10, 0xe6]);
    }

    #[test]
    fn spatial_columns_are_rewritten_with_their_srid() {
        let cols = vec![("id".to_string(), "int".to_string()), ("shape".to_string(), "geometry".to_string())];
        let rw = rewrite_spatial("SELECT id, shape FROM dbo.t;", &cols).unwrap();
        assert_eq!(
            rw.sql,
            "SELECT [id], [shape].STAsBinary() AS [shape], [shape].STSrid AS [theta_srid_1] FROM (SELECT id, shape FROM dbo.t) AS [theta_q]"
        );
        assert_eq!(rw.spatial, vec![None, Some("geometry".to_string())]);
        assert!(rewrite_spatial("SELECT 1", &[("a".into(), "int".into())]).is_none());
        assert_eq!(quote("a]b"), "[a]]b]");
    }

    #[test]
    fn udt_panics_are_recognised() {
        let p: Box<dyn std::any::Any + Send> =
            Box::new("not yet implemented: User-defined types not supported");
        assert!(is_udt_panic(p.as_ref()));
        let q: Box<dyn std::any::Any + Send> = Box::new("index out of bounds");
        assert!(!is_udt_panic(q.as_ref()));
    }

    #[test]
    fn actions() {
        assert_eq!(action("SET_NULL".into()).as_deref(), Some("SET NULL"));
        assert_eq!(action("NO_ACTION".into()), None);
    }
}
