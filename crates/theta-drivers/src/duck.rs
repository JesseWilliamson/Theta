//! DuckDB driver on top of the `duckdb` crate (bundled, so no system library is needed).
//!
//! Design notes:
//! * DuckDB is embedded and its connection is neither `Sync` nor async, so every call runs on a blocking
//!   thread (`spawn_blocking`) with the connection behind a `std` mutex. That mutex is also what makes
//!   statements run one at a time, in call order.
//! * `cancel` must not wait for that mutex (the running statement holds it), so it goes through the
//!   `InterruptHandle` taken when the connection was opened; DuckDB's interrupt is thread-safe.
//! * Extensions are never downloaded: the build links the `parquet` and `json` extensions statically and
//!   the app does not issue `INSTALL`. `GEOMETRY` values are mapped when the core type delivers them; the
//!   spatial extension's functions are not available, which the user guide says.
//! * Nested values (LIST, STRUCT, MAP, UNION) become JSON text; exact numbers (DECIMAL, HUGEINT) stay text.

use crate::plan_duck;
use crate::util::{clock_text, date_text, is_ddl, reports_rows_affected};
use async_trait::async_trait;
use duckdb::core::{LogicalTypeHandle, LogicalTypeId};
use duckdb::types::{TimeUnit, Value as DuckValue};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use theta_core::*;

pub struct DuckDriver;

#[async_trait]
impl Driver for DuckDriver {
    fn kind(&self) -> DatabaseKind {
        DatabaseKind::DuckDb
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>> {
        Ok(Box::new(DuckConnection::open(config).await?))
    }
}

pub struct DuckConnection {
    /// `None` after `close`.
    conn: Arc<Mutex<Option<duckdb::Connection>>>,
    interrupt: Arc<duckdb::InterruptHandle>,
    running: Arc<AtomicBool>,
    cancel_requested: Arc<AtomicBool>,
    /// Whether the user's own `BEGIN` is open. DuckDB's C API has no transaction-status call (the crate's
    /// `is_autocommit` is hard-wired to true), so it is tracked from the statements that run: `BEGIN` /
    /// `START` opens, `COMMIT` / `END` / `ROLLBACK` / `ABORT` closes, and a failed statement changes nothing
    /// (DuckDB keeps an aborted transaction open until it is rolled back).
    in_txn: Arc<AtomicBool>,
    /// Display name of the database file (or `:memory:`), for the status bar.
    location: String,
}

fn query_error(e: duckdb::Error, sql: &str) -> Error {
    let message = e.to_string();
    // DuckDB prefixes its messages with the error class (`Parser Error:`, `Binder Error:`, ...) and the
    // class doubles as the nearest thing to a code.
    let class = message.split_once(" Error:").map(|(c, _)| format!("{c} Error"));
    // Parser errors say `LINE 1: select ...` and point with a caret; the caret column is the offset.
    let position = message.lines().find_map(|l| l.trim_start().strip_prefix("LINE ")).and_then(|_| {
        let caret = message.lines().find(|l| l.trim_start().starts_with('^'))?;
        let col = caret.find('^')?;
        let line_text = message.lines().find(|l| l.trim_start().starts_with("LINE "))?;
        let prefix = line_text.find(": ").map(|p| p + 2)?;
        let in_line = col.checked_sub(prefix)?;
        // `LINE n:` counts lines of the statement; only the first line is mapped (the common case).
        sql.lines().next().filter(|l| in_line <= l.len()).map(|_| ErrorPosition { offset: in_line })
    });
    Error::Query { message, code: class, detail: None, hint: None, position }
}

impl DuckConnection {
    async fn open(config: &ConnectionConfig) -> Result<Self> {
        let path = config.path.trim().to_string();
        let memory = path.is_empty() || path == ":memory:";
        let location = if memory { ":memory:".to_string() } else { path.clone() };
        let conn = tokio::task::spawn_blocking(move || -> Result<duckdb::Connection> {
            // DuckDB can fetch an extension on its own the first time a query needs one (`httpfs` for an
            // `https://` path, say). Theta never downloads code on the user's behalf, so both automatic
            // paths are switched off; the bundled `parquet` and `json` are linked in and unaffected.
            let cfg = duckdb::Config::default()
                .with("autoinstall_known_extensions", "false")
                .and_then(|c| c.with("autoload_known_extensions", "false"))
                .map_err(|e| Error::Connect(e.to_string()))?;
            let c = if memory {
                duckdb::Connection::open_in_memory_with_flags(cfg)
            } else {
                if !Path::new(&path).exists()
                    && let Some(dir) = Path::new(&path).parent().filter(|d| !d.as_os_str().is_empty())
                    && !dir.exists()
                {
                    return Err(Error::Connect(format!("the folder {} does not exist", dir.display())));
                }
                duckdb::Connection::open_with_flags(&path, cfg)
            };
            c.map_err(|e| Error::Connect(e.to_string()))
        })
        .await
        .map_err(Error::other)??;
        let interrupt = conn.interrupt_handle();
        Ok(Self {
            conn: Arc::new(Mutex::new(Some(conn))),
            interrupt,
            running: Arc::new(AtomicBool::new(false)),
            cancel_requested: Arc::new(AtomicBool::new(false)),
            in_txn: Arc::new(AtomicBool::new(false)),
            location,
        })
    }

    /// Run `f` on a blocking thread with the connection, tracking `running` for `cancel`.
    async fn with_conn<T: Send + 'static>(
        &self,
        f: impl FnOnce(&duckdb::Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let conn = self.conn.clone();
        let running = self.running.clone();
        let cancel_requested = self.cancel_requested.clone();
        cancel_requested.store(false, Ordering::SeqCst);
        let r = tokio::task::spawn_blocking(move || {
            let guard = conn.lock().unwrap_or_else(|p| p.into_inner());
            let c = guard.as_ref().ok_or(Error::Closed)?;
            running.store(true, Ordering::SeqCst);
            let r = f(c);
            running.store(false, Ordering::SeqCst);
            r
        })
        .await
        .map_err(Error::other)?;
        match r {
            Err(Error::Query { ref message, .. })
                if message.contains("INTERRUPT") && self.cancel_requested.swap(false, Ordering::SeqCst) =>
            {
                Err(Error::Cancelled)
            }
            other => other,
        }
    }
}

// ------------------------------------------------------------------------------------------ values

/// DuckDB's own spelling of a column type, built from its logical type.
pub(crate) fn type_name(t: &LogicalTypeHandle) -> String {
    use LogicalTypeId as L;
    let base = match t.id() {
        L::Boolean => "BOOLEAN",
        L::Tinyint => "TINYINT",
        L::Smallint => "SMALLINT",
        L::Integer => "INTEGER",
        L::Bigint => "BIGINT",
        L::UTinyint => "UTINYINT",
        L::USmallint => "USMALLINT",
        L::UInteger => "UINTEGER",
        L::UBigint => "UBIGINT",
        L::Hugeint => "HUGEINT",
        L::UHugeint => "UHUGEINT",
        L::Float => "FLOAT",
        L::Double => "DOUBLE",
        L::Timestamp => "TIMESTAMP",
        L::TimestampS => "TIMESTAMP_S",
        L::TimestampMs => "TIMESTAMP_MS",
        L::TimestampNs => "TIMESTAMP_NS",
        L::TimestampTZ => "TIMESTAMP WITH TIME ZONE",
        L::Date => "DATE",
        L::Time => "TIME",
        L::TimeTZ => "TIME WITH TIME ZONE",
        L::TimeNs => "TIME_NS",
        L::Interval => "INTERVAL",
        L::Varchar => "VARCHAR",
        L::Blob => "BLOB",
        L::Bit => "BIT",
        L::Uuid => "UUID",
        L::Enum => "ENUM",
        L::Geometry => "GEOMETRY",
        L::Bignum => "BIGNUM",
        L::Variant => "VARIANT",
        L::SqlNull => "NULL",
        L::Decimal => return format!("DECIMAL({},{})", t.decimal_width(), t.decimal_scale()),
        L::List => return format!("{}[]", type_name(&t.child(0))),
        L::Array => return format!("{}[]", type_name(&t.child(0))),
        L::Map => return format!("MAP({}, {})", type_name(&t.child(0)), type_name(&t.child(1))),
        L::Struct => {
            let fields: Vec<String> = (0..t.num_children())
                .map(|i| format!("{} {}", t.child_name(i), type_name(&t.child(i))))
                .collect();
            return format!("STRUCT({})", fields.join(", "));
        }
        L::Union => {
            let fields: Vec<String> = (0..t.num_children())
                .map(|i| format!("{} {}", t.child_name(i), type_name(&t.child(i))))
                .collect();
            return format!("UNION({})", fields.join(", "));
        }
        _ => "UNKNOWN",
    };
    base.to_string()
}

fn type_kind(id: LogicalTypeId) -> TypeKind {
    use LogicalTypeId as L;
    match id {
        L::Boolean => TypeKind::Bool,
        L::Tinyint
        | L::Smallint
        | L::Integer
        | L::Bigint
        | L::UTinyint
        | L::USmallint
        | L::UInteger
        | L::UBigint
        | L::Hugeint
        | L::UHugeint
        | L::Bignum => TypeKind::Integer,
        L::Float | L::Double => TypeKind::Float,
        L::Decimal => TypeKind::Decimal,
        L::Varchar | L::Enum => TypeKind::Text,
        L::Blob | L::Bit => TypeKind::Bytes,
        L::Uuid => TypeKind::Uuid,
        L::Date => TypeKind::Date,
        L::Time | L::TimeTZ | L::TimeNs => TypeKind::Time,
        L::Timestamp | L::TimestampS | L::TimestampMs | L::TimestampNs | L::TimestampTZ => {
            TypeKind::Timestamp
        }
        L::Interval => TypeKind::Interval,
        L::List | L::Array | L::Struct | L::Map | L::Union | L::Variant => TypeKind::Json,
        L::Geometry => TypeKind::Geometry,
        _ => TypeKind::Other,
    }
}

fn micros(unit: TimeUnit, v: i64) -> i128 {
    match unit {
        TimeUnit::Second => v as i128 * 1_000_000,
        TimeUnit::Millisecond => v as i128 * 1_000,
        TimeUnit::Microsecond => v as i128,
        TimeUnit::Nanosecond => v as i128 / 1_000,
    }
}

fn timestamp_text(unit: TimeUnit, v: i64) -> String {
    // DuckDB's infinities are the extreme i64 values.
    if v == i64::MAX {
        return "infinity".into();
    }
    if v == i64::MIN {
        return "-infinity".into();
    }
    let us = micros(unit, v);
    let days = us.div_euclid(86_400_000_000) as i64;
    format!("{} {}", date_text(days), clock_text(us.rem_euclid(86_400_000_000)))
}

fn interval_text(months: i32, days: i32, nanos: i64) -> String {
    let mut parts = Vec::new();
    let (years, months) = (months / 12, months % 12);
    let plural = |n: i32, w: &str| format!("{n} {w}{}", if n == 1 || n == -1 { "" } else { "s" });
    if years != 0 {
        parts.push(plural(years, "year"));
    }
    if months != 0 {
        parts.push(plural(months, "month"));
    }
    if days != 0 {
        parts.push(plural(days, "day"));
    }
    if nanos != 0 || parts.is_empty() {
        let sign = if nanos < 0 { "-" } else { "" };
        parts.push(format!("{sign}{}", clock_text((nanos.unsigned_abs() / 1_000) as i128)));
    }
    parts.join(" ")
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A nested value as JSON text. Scalars that JSON has no literal for (dates, intervals, blobs, huge
/// numbers) are strings; maps become objects when every key is a string and a list of `{key, value}`
/// pairs otherwise, so no key is lost.
fn json_of(v: &DuckValue) -> String {
    match v {
        DuckValue::Null => "null".into(),
        DuckValue::Boolean(b) => b.to_string(),
        DuckValue::TinyInt(_)
        | DuckValue::SmallInt(_)
        | DuckValue::Int(_)
        | DuckValue::BigInt(_)
        | DuckValue::UTinyInt(_)
        | DuckValue::USmallInt(_)
        | DuckValue::UInt(_)
        | DuckValue::UBigInt(_) => scalar(v).display(),
        DuckValue::Float(_) | DuckValue::Double(_) => {
            let t = scalar(v).display();
            if t.parse::<f64>().is_ok_and(f64::is_finite) { t } else { json_str(&t) }
        }
        DuckValue::Decimal(_) | DuckValue::HugeInt(_) | DuckValue::UHugeInt(_) => scalar(v).display(),
        DuckValue::Text(s) | DuckValue::Enum(s) => json_str(s),
        DuckValue::List(items) | DuckValue::Array(items) => {
            format!("[{}]", items.iter().map(json_of).collect::<Vec<_>>().join(","))
        }
        DuckValue::Struct(fields) => {
            format!(
                "{{{}}}",
                fields
                    .iter()
                    .map(|(k, v)| format!("{}:{}", json_str(k), json_of(v)))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        DuckValue::Map(entries) => {
            if entries.iter().all(|(k, _)| matches!(k, DuckValue::Text(_) | DuckValue::Enum(_))) {
                format!(
                    "{{{}}}",
                    entries
                        .iter()
                        .map(|(k, v)| format!("{}:{}", json_of(k), json_of(v)))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            } else {
                format!(
                    "[{}]",
                    entries
                        .iter()
                        .map(|(k, v)| format!("{{\"key\":{},\"value\":{}}}", json_of(k), json_of(v)))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }
        }
        DuckValue::Union(inner) => json_of(inner),
        other => json_str(&scalar(other).display()),
    }
}

/// A non-nested value, with no column type to go on (so a UUID is text and a TIMESTAMPTZ has no offset).
fn scalar(v: &DuckValue) -> Value {
    match v {
        DuckValue::Null => Value::Null,
        DuckValue::Boolean(b) => Value::Bool(*b),
        DuckValue::TinyInt(i) => Value::Int(*i as i64),
        DuckValue::SmallInt(i) => Value::Int(*i as i64),
        DuckValue::Int(i) => Value::Int(*i as i64),
        DuckValue::BigInt(i) => Value::Int(*i),
        DuckValue::UTinyInt(i) => Value::Int(*i as i64),
        DuckValue::USmallInt(i) => Value::Int(*i as i64),
        DuckValue::UInt(i) => Value::Int(*i as i64),
        DuckValue::UBigInt(i) => Value::UInt(*i),
        // 128-bit integers do not fit `Int`/`UInt`; their exact decimal text does.
        DuckValue::HugeInt(i) => Value::Decimal(i.to_string()),
        DuckValue::UHugeInt(i) => Value::Decimal(i.to_string()),
        // f32 widened to f64 shows float noise (0.1 as 0.10000000149); the shortest f32 text is what the
        // user stored.
        DuckValue::Float(f) => Value::Float(f.to_string().parse().unwrap_or(*f as f64)),
        DuckValue::Double(f) => Value::Float(*f),
        DuckValue::Decimal(d) => Value::Decimal(d.to_string()),
        DuckValue::Timestamp(unit, v) => Value::Timestamp(timestamp_text(*unit, *v)),
        DuckValue::Text(s) | DuckValue::Enum(s) => Value::Text(s.clone()),
        DuckValue::Blob(b) => Value::Bytes(b.clone()),
        DuckValue::Geometry(b) => Value::Geometry(b.clone()),
        DuckValue::Date32(d) => Value::Date(date_text(*d as i64)),
        DuckValue::Time64(unit, v) => Value::Time(clock_text(micros(*unit, *v))),
        DuckValue::Interval { months, days, nanos } => Value::Interval(interval_text(*months, *days, *nanos)),
        DuckValue::List(_) | DuckValue::Array(_) | DuckValue::Struct(_) | DuckValue::Map(_) => {
            Value::Json(json_of(v))
        }
        DuckValue::Union(inner) => scalar(inner),
        // The enum is non-exhaustive: a type a newer DuckDB adds shows as its debug text, not a panic.
        other => Value::Unknown(format!("{other:?}")),
    }
}

/// A result cell, using the column's logical type for what the value alone cannot say.
fn convert(v: DuckValue, id: LogicalTypeId) -> Value {
    match (&v, id) {
        (DuckValue::Null, _) => Value::Null,
        (DuckValue::Timestamp(unit, t), LogicalTypeId::TimestampTZ) => {
            // TIMESTAMPTZ values are UTC instants; the offset is stated so the text is unambiguous.
            Value::TimestampTz(format!("{}+00", timestamp_text(*unit, *t)))
        }
        (DuckValue::Text(s), LogicalTypeId::Uuid) => Value::Uuid(s.clone()),
        _ => scalar(&v),
    }
}

// ---------------------------------------------------------------------------------------- execution

struct Meta {
    name: String,
    type_name: String,
    id: LogicalTypeId,
}

fn run_statement(conn: &duckdb::Connection, sql: &str, limit: Option<usize>) -> Result<StatementOutcome> {
    let started = Instant::now();
    let mut stmt = conn.prepare(sql).map_err(|e| query_error(e, sql))?;
    let mut rows = stmt.query([]).map_err(|e| query_error(e, sql))?;
    let metas: Vec<Meta> = match rows.as_ref() {
        Some(s) => (0..s.column_count())
            .map(|i| {
                let t = s.column_logical_type(i);
                Meta {
                    name: s.column_name(i).map(|n| n.to_string()).unwrap_or_default(),
                    type_name: type_name(&t),
                    id: t.id(),
                }
            })
            .collect(),
        None => Vec::new(),
    };
    let dml = reports_rows_affected(sql);
    let mut out: Vec<Vec<Value>> = Vec::new();
    let mut truncated = false;
    while let Some(row) = rows.next().map_err(|e| query_error(e, sql))? {
        if !dml && limit.is_some_and(|l| out.len() >= l) {
            truncated = true;
            break;
        }
        let mut cells = Vec::with_capacity(metas.len());
        for (i, m) in metas.iter().enumerate() {
            let v: DuckValue = row.get_ref(i).map(|r| r.to_owned()).map_err(|e| query_error(e, sql))?;
            cells.push(convert(v, m.id));
        }
        out.push(cells);
    }
    drop(rows);
    let elapsed = started.elapsed();
    // DML reports its count as a one-row `Count` result; DDL answers nothing worth a grid either.
    if dml || is_ddl(sql) || metas.is_empty() {
        let n = if dml { out.first().and_then(|r| r.first()).and_then(count_of) } else { None };
        return Ok(StatementOutcome { result: None, rows_affected: n, notices: Vec::new(), elapsed });
    }
    let columns = metas
        .into_iter()
        .map(|m| Column { kind: type_kind(m.id), name: m.name, type_name: m.type_name, origin: None })
        .collect();
    Ok(StatementOutcome {
        result: Some(ResultSet { columns, rows: out, truncated }),
        rows_affected: None,
        notices: Vec::new(),
        elapsed,
    })
}

fn count_of(v: &Value) -> Option<u64> {
    match v {
        Value::Int(i) => u64::try_from(*i).ok(),
        Value::UInt(u) => Some(*u),
        _ => None,
    }
}

fn scalar_text(conn: &duckdb::Connection, sql: &str) -> Option<String> {
    conn.query_row(sql, [], |r| r.get::<_, Option<String>>(0)).ok().flatten()
}

#[async_trait]
impl Connection for DuckConnection {
    fn kind(&self) -> DatabaseKind {
        DatabaseKind::DuckDb
    }

    async fn server_info(&self) -> Result<ServerInfo> {
        let location = self.location.clone();
        self.with_conn(move |c| {
            let version = scalar_text(c, "SELECT version()").unwrap_or_default();
            let db = scalar_text(c, "SELECT current_database()");
            Ok(ServerInfo {
                version: format!("DuckDB {}", version.trim_start_matches('v')),
                current_database: Some(match db {
                    Some(d) if location == ":memory:" => format!("{d} (in memory)"),
                    _ => location,
                }),
                current_user: None,
                banner: format!("DuckDB {version}"),
            })
        })
        .await
    }

    async fn execute(&self, sql: &str, options: &ExecOptions) -> Result<StatementOutcome> {
        let (sql, limit) = (sql.to_string(), options.row_limit);
        let flag = self.in_txn.clone();
        self.with_conn(move |c| {
            let out = run_statement(c, &sql, limit)?;
            match crate::util::leading_keyword(&sql).as_str() {
                "BEGIN" | "START" => flag.store(true, Ordering::SeqCst),
                "COMMIT" | "END" | "ROLLBACK" | "ABORT" => flag.store(false, Ordering::SeqCst),
                _ => {}
            }
            Ok(out)
        })
        .await
    }

    async fn cancel(&self) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            self.cancel_requested.store(true, Ordering::SeqCst);
            self.interrupt.interrupt();
        }
        Ok(())
    }

    async fn introspect(&self) -> Result<Catalog> {
        self.with_conn(introspect).await
    }

    async fn explain(&self, sql: &str, analyze: bool) -> Result<Plan> {
        let sql = sql.to_string();
        let in_txn = self.in_txn.load(Ordering::SeqCst);
        self.with_conn(move |c| {
            // JSON keeps the tree; the text form is a box drawing. ANALYZE runs the statement, so a write
            // is wrapped in a transaction that is rolled back (a savepoint is not available in DuckDB, so
            // inside the user's own transaction the statement is explained, not analysed, and the caller
            // is told).
            if analyze && in_txn && !matches!(crate::util::leading_keyword(&sql).as_str(), "SELECT" | "WITH") {
                return Err(Error::Unsupported(
                    "EXPLAIN ANALYZE of a write inside an open transaction would keep its changes; commit or roll back first".into(),
                ));
            }
            let prefix = if analyze { "EXPLAIN (ANALYZE, FORMAT JSON) " } else { "EXPLAIN (FORMAT JSON) " };
            let sent = format!("{prefix}{sql}");
            let begin = analyze && !in_txn;
            if begin {
                c.execute_batch("BEGIN").map_err(|e| query_error(e, "BEGIN"))?;
            }
            let rows: std::result::Result<Vec<(String, String)>, duckdb::Error> = (|| {
                let mut st = c.prepare(&sent)?;
                let it = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
                it.collect()
            })();
            if begin {
                let _ = c.execute_batch("ROLLBACK");
            }
            let rows = rows.map_err(|e| query_error(e, &sql))?;
            let raw = rows.into_iter().map(|(_, v)| v).collect::<Vec<_>>().join("\n");
            plan_duck::parse(&raw, analyze)
        })
        .await
    }

    async fn in_transaction(&self) -> bool {
        self.in_txn.load(Ordering::SeqCst)
    }

    async fn close(&self) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            self.interrupt.interrupt();
        }
        self.in_txn.store(false, Ordering::SeqCst);
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            // Dropping the connection checkpoints and releases the file lock.
            conn.lock().unwrap_or_else(|p| p.into_inner()).take();
        })
        .await
        .map_err(Error::other)?;
        Ok(())
    }
}

// ------------------------------------------------------------------------------------ introspection

fn list_of_text(v: DuckValue) -> Vec<String> {
    match v {
        DuckValue::List(items) | DuckValue::Array(items) => items
            .into_iter()
            .filter_map(|i| match i {
                DuckValue::Text(s) => Some(s),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn introspect(c: &duckdb::Connection) -> Result<Catalog> {
    let q = |sql: &str| -> Result<Vec<Vec<DuckValue>>> {
        let mut st = c.prepare(sql).map_err(|e| query_error(e, sql))?;
        let mut rows = st.query([]).map_err(|e| query_error(e, sql))?;
        let n = rows.as_ref().map_or(0, |s| s.column_count());
        let mut out = Vec::new();
        while let Some(r) = rows.next().map_err(|e| query_error(e, sql))? {
            out.push((0..n).map(|i| r.get_ref(i).map(|v| v.to_owned()).unwrap_or(DuckValue::Null)).collect());
        }
        Ok(out)
    };
    let text = |v: &DuckValue| match v {
        DuckValue::Text(s) | DuckValue::Enum(s) => s.clone(),
        DuckValue::Null => String::new(),
        other => scalar(other).display(),
    };
    let opt = |v: &DuckValue| match v {
        DuckValue::Null => None,
        other => Some(text(other)).filter(|s| !s.is_empty()),
    };
    let flag = |v: &DuckValue| matches!(v, DuckValue::Boolean(true));
    let int = |v: &DuckValue| match scalar(v) {
        Value::Int(i) => Some(i),
        Value::UInt(u) => i64::try_from(u).ok(),
        _ => None,
    };

    let db = "database_name = current_database()";
    let mut schemas: Vec<Schema> = q(&format!(
        "SELECT schema_name FROM duckdb_schemas() WHERE {db} \
         AND schema_name NOT IN ('information_schema', 'pg_catalog') ORDER BY schema_name"
    ))?
    .iter()
    .map(|r| Schema { name: text(&r[0]), ..Default::default() })
    .collect();
    let sidx: HashMap<String, usize> = schemas.iter().enumerate().map(|(i, s)| (s.name.clone(), i)).collect();
    let mut tidx: HashMap<(usize, String), usize> = HashMap::new();

    for r in q(&format!(
        "SELECT schema_name, table_name, comment, estimated_size FROM duckdb_tables() \
         WHERE {db} AND NOT internal AND NOT temporary ORDER BY schema_name, table_name"
    ))? {
        let Some(&si) = sidx.get(&text(&r[0])) else { continue };
        let name = text(&r[1]);
        schemas[si].tables.push(Table {
            name: name.clone(),
            comment: opt(&r[2]),
            row_estimate: int(&r[3]).and_then(|n| u64::try_from(n).ok()),
            ..Default::default()
        });
        tidx.insert((si, name), schemas[si].tables.len() - 1);
    }
    for r in q(&format!(
        "SELECT schema_name, view_name, sql, comment FROM duckdb_views() \
         WHERE {db} AND NOT internal AND NOT temporary ORDER BY schema_name, view_name"
    ))? {
        let Some(&si) = sidx.get(&text(&r[0])) else { continue };
        let name = text(&r[1]);
        schemas[si].tables.push(Table {
            name: name.clone(),
            kind: TableKind::View,
            view_definition: opt(&r[2]),
            comment: opt(&r[3]),
            ..Default::default()
        });
        tidx.insert((si, name), schemas[si].tables.len() - 1);
    }

    for r in q(&format!(
        "SELECT schema_name, table_name, column_name, data_type, is_nullable, column_default, comment \
         FROM duckdb_columns() WHERE {db} AND NOT internal ORDER BY schema_name, table_name, column_index"
    ))? {
        let Some(&si) = sidx.get(&text(&r[0])) else { continue };
        let Some(&ti) = tidx.get(&(si, text(&r[1]))) else { continue };
        let data_type = text(&r[3]);
        let default = opt(&r[5]);
        let geometry = (data_type.eq_ignore_ascii_case("GEOMETRY")
            || data_type.to_ascii_uppercase().starts_with("GEOMETRY("))
        .then(|| GeometryColumn { geometry_type: "GEOMETRY".into(), srid: None, geography: false });
        schemas[si].tables[ti].columns.push(ColumnInfo {
            name: text(&r[2]),
            kind: if geometry.is_some() {
                TypeKind::Geometry
            } else {
                crate::util::kind_from_type_name(&duck_base_type(&data_type))
            },
            data_type,
            nullable: flag(&r[4]),
            // A `nextval('seq')` default is DuckDB's idiom for an auto-increment key.
            identity: default.as_deref().filter(|d| d.starts_with("nextval(")).map(|_| Identity::ByDefault),
            default,
            comment: opt(&r[6]),
            geometry,
            generated: None,
        });
    }

    // Constraints. The column list differs between DuckDB versions in what it names; the stable fields are
    // the type and the column names, plus `constraint_text` for CHECK.
    for r in q(&format!(
        "SELECT schema_name, table_name, constraint_type, constraint_column_names, constraint_text, \
                COALESCE(constraint_name, ''), referenced_table, referenced_column_names FROM duckdb_constraints() WHERE {db} ORDER BY schema_name, table_name, constraint_index"
    ))? {
        let Some(&si) = sidx.get(&text(&r[0])) else { continue };
        let Some(&ti) = tidx.get(&(si, text(&r[1]))) else { continue };
        let t = &mut schemas[si].tables[ti];
        let cols = list_of_text(r[3].clone());
        let name = text(&r[5]);
        match text(&r[2]).as_str() {
            "PRIMARY KEY" => t.primary_key = cols,
            "UNIQUE" => t.unique_constraints.push(UniqueConstraint {
                name: if name.is_empty() { format!("{}_{}_key", t.name, cols.join("_")) } else { name },
                columns: cols,
            }),
            // `referenced_table` and its columns come with the constraint; DuckDB lists a foreign key's
            // rules (ON DELETE ...) nowhere a client can read, and an unspecified rule is NO ACTION.
            "FOREIGN KEY" => t.foreign_keys.push(ForeignKey {
                name: if name.is_empty() { format!("{}_{}_fkey", t.name, cols.join("_")) } else { name },
                columns: cols,
                referenced_schema: Some(text(&r[0])),
                referenced_table: text(&r[6]),
                referenced_columns: list_of_text(r[7].clone()),
                ..Default::default()
            }),
            "CHECK" => t.checks.push(CheckConstraint {
                name,
                expression: text(&r[4]).trim().trim_start_matches("CHECK").trim().to_string(),
            }),
            _ => {}
        }
    }
    for r in q(&format!(
        "SELECT schema_name, table_name, index_name, is_unique, is_primary, expressions::VARCHAR, sql \
         FROM duckdb_indexes() WHERE {db} ORDER BY schema_name, table_name, index_name"
    ))? {
        let Some(&si) = sidx.get(&text(&r[0])) else { continue };
        let Some(&ti) = tidx.get(&(si, text(&r[1]))) else { continue };
        let exprs = text(&r[5]);
        let columns: Vec<String> = exprs
            .trim_matches(|c| c == '[' || c == ']')
            .split(',')
            .map(|c| c.trim().trim_matches('\'').trim_matches('"').to_string())
            .filter(|c| !c.is_empty())
            .collect();
        schemas[si].tables[ti].indexes.push(Index {
            name: text(&r[2]),
            unique: flag(&r[3]),
            primary: flag(&r[4]),
            method: Some("art".into()),
            definition: opt(&r[6]),
            columns,
            ..Default::default()
        });
    }
    // The primary key also shows as an index entry for the explorer.
    for s in &mut schemas {
        for t in &mut s.tables {
            if !t.primary_key.is_empty() && !t.indexes.iter().any(|i| i.primary) {
                t.indexes.insert(
                    0,
                    Index {
                        name: format!("{}_pkey", t.name),
                        columns: t.primary_key.clone(),
                        unique: true,
                        primary: true,
                        method: Some("art".into()),
                        ..Default::default()
                    },
                );
            }
        }
    }

    for r in q(&format!(
        "SELECT schema_name, function_name, function_type, parameters::VARCHAR, return_type \
         FROM duckdb_functions() WHERE {db} AND NOT internal AND function_type IN ('macro', 'table_macro') \
         ORDER BY schema_name, function_name"
    ))? {
        let Some(&si) = sidx.get(&text(&r[0])) else { continue };
        schemas[si].routines.push(Routine {
            name: text(&r[1]),
            kind: RoutineKind::Function,
            signature: format!("({})", text(&r[3]).trim_matches(|c| c == '[' || c == ']')),
            return_type: opt(&r[4]),
        });
    }
    for r in
        q(&format!("SELECT schema_name, sequence_name FROM duckdb_sequences() WHERE {db} ORDER BY 1, 2"))?
    {
        if let Some(&si) = sidx.get(&text(&r[0])) {
            schemas[si].sequences.push(text(&r[1]));
        }
    }
    let default_schema = scalar_text(c, "SELECT current_schema()");
    Ok(Catalog { schemas, default_schema })
}

/// `DECIMAL(10,2)` / `INTEGER[]` / `STRUCT(a INTEGER)` to the lowercase base word `kind_from_type_name` knows.
fn duck_base_type(t: &str) -> String {
    let u = t.trim().to_ascii_uppercase();
    if u.ends_with(']') {
        return format!("{}[]", u.split('[').next().unwrap_or("").to_ascii_lowercase());
    }
    let base = u.split(['(', ' ']).next().unwrap_or("");
    match base {
        "STRUCT" | "MAP" | "UNION" => "json".into(),
        "UTINYINT" | "USMALLINT" | "UINTEGER" | "UBIGINT" | "HUGEINT" | "UHUGEINT" | "BIGNUM" => {
            "bigint".into()
        }
        "TIMESTAMP_S" | "TIMESTAMP_MS" | "TIMESTAMP_NS" => "timestamp".into(),
        "BIT" | "BITSTRING" => "blob".into(),
        _ => base.to_ascii_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(date_text(0), "1970-01-01");
        assert_eq!(date_text(19_782), "2024-02-29");
        assert_eq!(date_text(-1), "1969-12-31");
    }

    #[test]
    fn timestamps_and_clocks() {
        assert_eq!(
            timestamp_text(TimeUnit::Microsecond, 1_709_212_455_123_456),
            "2024-02-29 13:14:15.123456"
        );
        assert_eq!(timestamp_text(TimeUnit::Second, 0), "1970-01-01 00:00:00");
        assert_eq!(timestamp_text(TimeUnit::Microsecond, i64::MAX), "infinity");
        assert_eq!(clock_text(13 * 3_600_000_000 + 500_000), "13:00:00.5");
    }

    #[test]
    fn intervals() {
        assert_eq!(
            interval_text(14, 3, 4 * 3_600_000_000_000 + 5 * 60_000_000_000 + 6_000_000_000),
            "1 year 2 months 3 days 04:05:06"
        );
        assert_eq!(interval_text(0, 0, 0), "00:00:00");
        assert_eq!(interval_text(1, 1, 0), "1 month 1 day");
    }

    #[test]
    fn json_escapes_and_maps() {
        assert_eq!(json_str("a\"b\n"), "\"a\\\"b\\n\"");
        let m = DuckValue::Map(vec![(DuckValue::Text("k".into()), DuckValue::Int(1))].into());
        assert_eq!(json_of(&m), "{\"k\":1}");
        let m = DuckValue::Map(vec![(DuckValue::Int(1), DuckValue::Text("v".into()))].into());
        assert_eq!(json_of(&m), "[{\"key\":1,\"value\":\"v\"}]");
        let l = DuckValue::List(vec![DuckValue::Int(1), DuckValue::Null, DuckValue::Text("x".into())]);
        assert_eq!(json_of(&l), "[1,null,\"x\"]");
    }

    #[test]
    fn type_words_for_kinds() {
        assert_eq!(duck_base_type("DECIMAL(10,2)"), "decimal");
        assert_eq!(duck_base_type("INTEGER[]"), "integer[]");
        assert_eq!(duck_base_type("STRUCT(a INTEGER)"), "json");
        assert_eq!(duck_base_type("TIMESTAMP WITH TIME ZONE"), "timestamp");
    }
}
