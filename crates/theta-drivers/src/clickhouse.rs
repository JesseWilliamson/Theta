//! ClickHouse over its HTTP interface.
//!
//! Design notes:
//! * HTTP, not the native protocol: it needs no driver crate beyond the `ureq` client the application
//!   already ships (rustls), works through every proxy, and exposes everything this IDE needs. `ureq` is
//!   blocking, so each call runs on a blocking thread.
//! * Results are requested as `TabSeparatedWithNamesAndTypes` (`default_format`): the oldest format with
//!   column types, exact decimals (JSON would round them) and escapes that are trivial to undo. Values
//!   are parsed per ClickHouse type (`Nullable`, `LowCardinality`, `Array`, `Tuple`, `Map` included).
//! * A session id makes `SET` and `USE` persist across the HTTP requests of one connection; statements
//!   run one at a time, so the session is never "locked" by a second request.
//! * `cancel` sends `KILL QUERY WHERE query_id = ...` on a separate request; the cancelled statement
//!   answers with exception code 394 (`QUERY_WAS_CANCELLED`).
//! * The row limit is a server-side `max_result_rows` with `result_overflow_mode=break`, so the server stops
//!   instead of shipping what would be discarded.

use crate::plan_ch;
use async_trait::async_trait;
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use theta_core::*;

pub struct ClickHouseDriver;

#[async_trait]
impl Driver for ClickHouseDriver {
    fn kind(&self) -> DatabaseKind {
        DatabaseKind::ClickHouse
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>> {
        Ok(Box::new(ClickHouseConnection::connect(config, None).await?))
    }
}

/// Where the SSH tunnel's local end listens (the configured host is still what `Host:` says).
pub struct LocalEndpoint {
    pub port: u16,
}

#[derive(Clone)]
struct Endpoint {
    base: String,
    user: String,
    password: Option<String>,
    database: String,
    session: String,
    agent: ureq::Agent,
}

pub struct ClickHouseConnection {
    ep: Endpoint,
    /// The id of the statement in flight, for `KILL QUERY`.
    current: Arc<Mutex<Option<String>>>,
    cancel_requested: Arc<AtomicBool>,
    /// Serialises statements (one HTTP session, one query at a time).
    gate: tokio::sync::Mutex<()>,
    info: ServerInfo,
    closed: AtomicBool,
}

/// A failed request: the server's exception or a transport problem.
#[derive(Debug)]
enum Failure {
    Server { code: Option<u32>, message: String },
    Transport(String),
}

fn failure_to_error(f: Failure, sql: &str) -> Error {
    match f {
        Failure::Transport(m) => Error::Closed.or_other(m),
        Failure::Server { code, message } => {
            let position =
                server_position(&message).filter(|p| *p <= sql.len()).map(|offset| ErrorPosition { offset });
            Error::Query { message, code: code.map(|c| c.to_string()), detail: None, hint: None, position }
        }
    }
}

trait OrOther {
    fn or_other(self, message: String) -> Error;
}

impl OrOther for Error {
    /// A transport failure is a lost session when the socket broke, and a plain error otherwise.
    fn or_other(self, message: String) -> Error {
        let lower = message.to_ascii_lowercase();
        if lower.contains("connection reset")
            || lower.contains("broken pipe")
            || lower.contains("unexpected eof")
            || lower.contains("connection closed")
        {
            Error::Closed
        } else {
            Error::Other(message)
        }
    }
}

/// `Code: 62. DB::Exception: Syntax error: failed at position 1 (line 1, col 1): SELEC 1...`: the 1-based
/// position of the failure as a 0-based byte offset.
fn server_position(message: &str) -> Option<usize> {
    let rest = message.split("failed at position ").nth(1)?;
    let n: usize = rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
    n.checked_sub(1)
}

/// `Code: 62. DB::Exception: message (SYNTAX_ERROR) (version ...)`: the numeric code and the message
/// without the `Code: N. DB::Exception: ` lead.
fn parse_exception(body: &str) -> (Option<u32>, String) {
    let body = body.trim();
    let code = body
        .strip_prefix("Code: ")
        .and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|n| n.parse().ok());
    let message = match body.split_once("DB::Exception: ") {
        Some((_, m)) => m.to_string(),
        None => body.to_string(),
    };
    (code, message)
}

fn agent(config: &ConnectionConfig) -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig};
    let tls = match config.ssl_mode {
        // "Require" in the form means encrypted without checking the certificate (a self-signed server).
        SslMode::Require => TlsConfig::builder().disable_verification(true).build(),
        _ => TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build(),
    };
    ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_connect(Some(Duration::from_secs(10)))
        // No overall timeout: an analytical query may legitimately run for minutes.
        .http_status_as_error(false)
        .build()
        .into()
}

impl Endpoint {
    /// Send `sql` as the request body. Returns the response body text.
    fn post(
        &self,
        sql: &str,
        params: &[(&str, String)],
        with_session: bool,
    ) -> std::result::Result<(String, HashMap<String, String>), Failure> {
        let mut req = self
            .agent
            .post(&self.base)
            .header("X-ClickHouse-User", &self.user)
            .header("X-ClickHouse-Database", &self.database)
            .header("User-Agent", "Theta");
        if let Some(p) = &self.password {
            req = req.header("X-ClickHouse-Key", p);
        }
        if with_session {
            req = req.query("session_id", &self.session).query("session_timeout", "3600");
        }
        for (k, v) in params {
            req = req.query(k, v);
        }
        let mut resp = req.send(sql.as_bytes()).map_err(|e| Failure::Transport(e.to_string()))?;
        let status = resp.status().as_u16();
        let mut headers = HashMap::new();
        for (k, v) in resp.headers() {
            if let Ok(v) = v.to_str() {
                headers.insert(k.as_str().to_ascii_lowercase(), v.to_string());
            }
        }
        let mut body = String::new();
        resp.body_mut()
            .as_reader()
            .read_to_string(&mut body)
            .map_err(|e| Failure::Transport(e.to_string()))?;
        // A failure that happens after the first block was sent arrives as an exception text at the end of
        // an HTTP 200 stream; look for it so a cancelled long query is reported as such.
        if status == 200 {
            if let Some(at) = body.rfind("Code: ")
                && body[at..].contains("DB::Exception")
                && body[at..].lines().count() <= 3
                && at > 0
                && body.as_bytes()[at - 1] == b'\n'
            {
                let (code, message) = parse_exception(&body[at..]);
                return Err(Failure::Server { code, message });
            }
            return Ok((body, headers));
        }
        let (code, message) = parse_exception(&body);
        Err(Failure::Server { code, message })
    }
}

impl ClickHouseConnection {
    pub async fn connect(config: &ConnectionConfig, via: Option<LocalEndpoint>) -> Result<Self> {
        let secure = config.ssl_mode != SslMode::Disable && config.ssl_mode != SslMode::Prefer;
        let host = if via.is_some() {
            "127.0.0.1"
        } else if config.host.is_empty() {
            "localhost"
        } else {
            config.host.as_str()
        };
        let default_port = if secure { 8443 } else { 8123 };
        let port = via.map(|v| v.port).unwrap_or(if config.port == 0 { default_port } else { config.port });
        let host = if host.contains(':') { format!("[{host}]") } else { host.to_string() };
        let ep = Endpoint {
            base: format!("{}://{host}:{port}/", if secure { "https" } else { "http" }),
            user: if config.user.is_empty() { "default".into() } else { config.user.clone() },
            password: config.password.clone(),
            database: if config.database.is_empty() { "default".into() } else { config.database.clone() },
            session: format!("theta-{}", uuid_like()),
            agent: agent(config),
        };
        let probe = ep.clone();
        let (version, db) = tokio::task::spawn_blocking(move || {
            probe.post(
                "SELECT version(), currentDatabase()",
                &[("default_format", "TabSeparated".into())],
                true,
            )
        })
        .await
        .map_err(Error::other)?
        .map_err(|f| match f {
            Failure::Server { code: Some(516 | 193), message } => Error::Auth(message),
            Failure::Server { message, .. } => Error::Connect(message),
            Failure::Transport(m) => Error::Connect(m),
        })
        .map(|(body, _)| {
            let line = body.lines().next().unwrap_or("").to_string();
            let mut p = line.split('\t');
            (p.next().unwrap_or("").to_string(), p.next().unwrap_or("").to_string())
        })?;
        let info = ServerInfo {
            version: format!("ClickHouse {version}"),
            current_database: Some(db),
            current_user: Some(ep.user.clone()),
            banner: format!("ClickHouse {version}"),
        };
        Ok(Self {
            ep,
            current: Arc::new(Mutex::new(None)),
            cancel_requested: Arc::new(AtomicBool::new(false)),
            gate: tokio::sync::Mutex::new(()),
            info,
            closed: AtomicBool::new(false),
        })
    }

    /// Run one statement and return its rows (empty for statements without a result set).
    async fn run(&self, sql: &str, limit: Option<usize>) -> Result<Raw> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }
        let _one_at_a_time = self.gate.lock().await;
        let id = uuid_like();
        *self.current.lock().unwrap() = Some(id.clone());
        self.cancel_requested.store(false, Ordering::SeqCst);
        let (ep, sql_owned) = (self.ep.clone(), sql.to_string());
        let mut params =
            vec![("query_id", id), ("default_format", "TabSeparatedWithNamesAndTypes".to_string())];
        if let Some(l) = limit {
            // One more than asked for, so "more rows exist" is known without a second query.
            params.push(("max_result_rows", (l + 1).to_string()));
            params.push(("result_overflow_mode", "break".into()));
        }
        let started = Instant::now();
        let r = tokio::task::spawn_blocking(move || ep.post(&sql_owned, &params, true))
            .await
            .map_err(Error::other)?;
        *self.current.lock().unwrap() = None;
        let cancelled = self.cancel_requested.swap(false, Ordering::SeqCst);
        match r {
            Ok((body, headers)) => Ok(Raw { body, headers, elapsed: started.elapsed() }),
            Err(Failure::Server { code: Some(394), .. }) => Err(Error::Cancelled),
            // A KILL that wins the race can also surface as a dropped stream.
            Err(Failure::Transport(_)) if cancelled => Err(Error::Cancelled),
            Err(f) => Err(failure_to_error(f, sql)),
        }
    }
}

struct Raw {
    body: String,
    headers: HashMap<String, String>,
    elapsed: Duration,
}

fn uuid_like() -> String {
    use std::sync::atomic::AtomicU64;
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{:x}-{:x}-{:x}", std::process::id(), nanos, N.fetch_add(1, Ordering::SeqCst))
}

// ------------------------------------------------------------------------------------------ values

/// The TSV escapes of ClickHouse: `\t \n \\ \' \0 \b \f \r` and `\N` for NULL.
fn unescape(field: &str) -> String {
    if !field.contains('\\') {
        return field.to_string();
    }
    let mut out = String::with_capacity(field.len());
    let mut it = field.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some(o) => out.push(o),
            None => out.push('\\'),
        }
    }
    out
}

/// `Nullable(LowCardinality(String))` to `String`: the wrappers that do not change what a value is.
fn strip_wrappers(t: &str) -> &str {
    let mut t = t.trim();
    loop {
        let inner = ["Nullable(", "LowCardinality(", "SimpleAggregateFunction("]
            .iter()
            .find_map(|p| t.strip_prefix(p).and_then(|r| r.strip_suffix(')')));
        match inner {
            Some(i) => {
                t = i.rsplit_once(", ").filter(|_| t.starts_with("SimpleAggregate")).map_or(i, |(_, ty)| ty)
            }
            None => return t,
        }
    }
}

pub(crate) fn type_kind(t: &str) -> TypeKind {
    let base = strip_wrappers(t);
    let head = base.split('(').next().unwrap_or("");
    match head {
        "Bool" => TypeKind::Bool,
        "UInt8" | "UInt16" | "UInt32" | "UInt64" | "UInt128" | "UInt256" | "Int8" | "Int16" | "Int32"
        | "Int64" | "Int128" | "Int256" => TypeKind::Integer,
        "Float32" | "Float64" => TypeKind::Float,
        "Decimal" | "Decimal32" | "Decimal64" | "Decimal128" | "Decimal256" => TypeKind::Decimal,
        "String" | "FixedString" | "Enum8" | "Enum16" | "IPv4" | "IPv6" => TypeKind::Text,
        "UUID" => TypeKind::Uuid,
        "Date" | "Date32" => TypeKind::Date,
        "DateTime" | "DateTime64" => TypeKind::Timestamp,
        "Time" | "Time64" => TypeKind::Time,
        "Array" => TypeKind::Array,
        "Tuple" | "Map" | "Nested" | "JSON" | "Object" | "Variant" => TypeKind::Json,
        _ => TypeKind::Other,
    }
}

/// Split the top level of a `(a, b)`-style argument list: commas inside quotes and nested brackets stay.
fn split_top(s: &str) -> Vec<&str> {
    let (mut depth, mut in_str, mut esc, mut start) = (0i32, false, false, 0);
    let mut out = Vec::new();
    for (i, c) in s.char_indices() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '\'' {
                in_str = false;
            }
            continue;
        }
        match c {
            '\'' => in_str = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    if !s[start..].trim().is_empty() || !out.is_empty() {
        out.push(s[start..].trim());
    }
    out
}

/// The arguments of `Name(a, b)`, or none.
fn type_args(t: &str) -> Vec<&str> {
    t.find('(').and_then(|i| t.strip_suffix(')').map(|r| split_top(&r[i + 1..]))).unwrap_or_default()
}

/// A quoted ClickHouse literal (`'it\'s'`) to its text; anything else unchanged.
fn unquote(s: &str) -> String {
    let s = s.trim();
    match s.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
        Some(inner) => unescape(inner),
        None => s.to_string(),
    }
}

fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
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

/// A ClickHouse text literal of type `t` as JSON text (for Tuple and Map columns, which have no `Value`
/// of their own). Numbers stay numbers, everything else becomes a string.
fn json_of(t: &str, lit: &str) -> String {
    let t = strip_wrappers(t);
    let lit = lit.trim();
    if lit == "NULL" || lit == "\\N" {
        return "null".into();
    }
    match t.split('(').next().unwrap_or("") {
        "Array" => {
            let inner = type_args(t).first().copied().unwrap_or("String");
            let body = lit.strip_prefix('[').and_then(|r| r.strip_suffix(']')).unwrap_or(lit);
            format!("[{}]", split_top(body).iter().map(|e| json_of(inner, e)).collect::<Vec<_>>().join(","))
        }
        "Tuple" => {
            let args = type_args(t);
            let body = lit.strip_prefix('(').and_then(|r| r.strip_suffix(')')).unwrap_or(lit);
            let items = split_top(body);
            // `Tuple(a Int32, b String)` names its elements; the type of each is after the name.
            let named = args.iter().all(|a| {
                a.split_once(' ').is_some_and(|(n, _)| n.chars().all(|c| c.is_alphanumeric() || c == '_'))
            });
            if named && !args.is_empty() {
                let fields: Vec<String> = args
                    .iter()
                    .zip(&items)
                    .map(|(a, v)| {
                        let (n, ty) = a.split_once(' ').unwrap_or((a, "String"));
                        format!("{}:{}", json_str(n), json_of(ty, v))
                    })
                    .collect();
                format!("{{{}}}", fields.join(","))
            } else {
                let elems: Vec<String> = args.iter().zip(&items).map(|(ty, v)| json_of(ty, v)).collect();
                format!("[{}]", elems.join(","))
            }
        }
        "Map" => {
            let args = type_args(t);
            let (kt, vt) =
                (args.first().copied().unwrap_or("String"), args.get(1).copied().unwrap_or("String"));
            let body = lit.strip_prefix('{').and_then(|r| r.strip_suffix('}')).unwrap_or(lit);
            let pairs: Vec<(String, String)> = split_top(body)
                .iter()
                .filter_map(|e| {
                    let mut it = split_top_colon(e);
                    let k = it.next()?;
                    let v = it.next()?;
                    Some((json_of(kt, k), json_of(vt, v)))
                })
                .collect();
            if kind_is_text(kt) {
                format!("{{{}}}", pairs.iter().map(|(k, v)| format!("{k}:{v}")).collect::<Vec<_>>().join(","))
            } else {
                format!(
                    "[{}]",
                    pairs
                        .iter()
                        .map(|(k, v)| format!("{{\"key\":{k},\"value\":{v}}}"))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }
        }
        _ => match type_kind(t) {
            TypeKind::Integer | TypeKind::Float if lit.parse::<f64>().is_ok_and(f64::is_finite) => {
                lit.to_string()
            }
            TypeKind::Decimal if lit.parse::<f64>().is_ok() => lit.to_string(),
            TypeKind::Bool => lit.to_string(),
            _ => json_str(&unquote(lit)),
        },
    }
}

fn kind_is_text(t: &str) -> bool {
    matches!(type_kind(t), TypeKind::Text | TypeKind::Uuid | TypeKind::Date | TypeKind::Timestamp)
}

/// `'k': 1` into its two halves at the first top-level colon.
fn split_top_colon(e: &str) -> impl Iterator<Item = &str> {
    let (mut in_str, mut esc, mut depth) = (false, false, 0i32);
    let mut at = None;
    for (i, c) in e.char_indices() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '\'' {
                in_str = false;
            }
            continue;
        }
        match c {
            '\'' => in_str = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ':' if depth == 0 => {
                at = Some(i);
                break;
            }
            _ => {}
        }
    }
    let (a, b) = match at {
        Some(i) => (e[..i].trim(), e[i + 1..].trim()),
        None => (e.trim(), ""),
    };
    [a, b].into_iter().filter(|s| !s.is_empty())
}

/// A decimal's text with the fraction digits its type declares.
///
/// Servers since 21.9 print `2.5` for a `Decimal(18, 2)` holding 2.50 unless
/// `output_format_decimal_trailing_zeros` is set, and older servers reject that setting as unknown.
/// The scale is part of the value (it is what the column stores and what an editor should show), so
/// it is restored here from the type instead of depending on a per-version setting.
fn pad_to_scale(base: &str, text: String) -> String {
    let scale = base
        .rsplit_once(['(', ','])
        .and_then(|(_, s)| s.trim().trim_end_matches(')').trim().parse::<usize>().ok())
        .unwrap_or(0);
    let have = text.split_once('.').map_or(0, |(_, f)| f.len());
    if scale <= have || !text.bytes().all(|b| b.is_ascii_digit() || b == b'-' || b == b'.') {
        return text;
    }
    let mut out = text;
    if have == 0 {
        out.push('.');
    }
    out.extend(std::iter::repeat_n('0', scale - have));
    out
}

/// One TSV field of ClickHouse type `t` as a `Value`.
fn parse_value(t: &str, field: &str) -> Value {
    if field == "\\N" {
        return Value::Null;
    }
    let text = unescape(field);
    let base = strip_wrappers(t);
    match base.split('(').next().unwrap_or("") {
        "Bool" => Value::Bool(text == "true" || text == "1"),
        "UInt64" | "UInt128" | "UInt256" | "Int128" | "Int256" => match text.parse::<u64>() {
            Ok(u) if base == "UInt64" => Value::UInt(u),
            _ => match text.parse::<i64>() {
                Ok(i) if !base.starts_with("UInt") && !matches!(base, "Int128" | "Int256") => Value::Int(i),
                // 128 and 256-bit integers do not fit `Int`/`UInt`; their text is exact.
                _ => Value::Decimal(text),
            },
        },
        "UInt8" | "UInt16" | "UInt32" | "Int8" | "Int16" | "Int32" | "Int64" => {
            text.parse::<i64>().map(Value::Int).unwrap_or(Value::Unknown(text))
        }
        "Float32" | "Float64" => match text.as_str() {
            "nan" => Value::Float(f64::NAN),
            "inf" => Value::Float(f64::INFINITY),
            "-inf" => Value::Float(f64::NEG_INFINITY),
            _ => text.parse().map(Value::Float).unwrap_or(Value::Unknown(text)),
        },
        "Decimal" | "Decimal32" | "Decimal64" | "Decimal128" | "Decimal256" => {
            Value::Decimal(pad_to_scale(base, text))
        }
        "String" | "FixedString" | "Enum8" | "Enum16" => Value::Text(text),
        "IPv4" | "IPv6" => Value::Text(text),
        "UUID" => Value::Uuid(text),
        "Date" | "Date32" => Value::Date(text),
        "DateTime" | "DateTime64" => Value::Timestamp(text),
        "Array" => {
            let inner = type_args(base).first().copied().unwrap_or("String");
            let body = text.strip_prefix('[').and_then(|r| r.strip_suffix(']')).unwrap_or(&text);
            Value::Array(split_top(body).iter().map(|e| parse_literal(inner, e)).collect())
        }
        "Tuple" | "Map" | "Nested" => Value::Json(json_of(base, &text)),
        _ => Value::Unknown(text),
    }
}

/// An element inside an array literal: quoted strings are unquoted, `NULL` is null, the rest parse as
/// the element type.
fn parse_literal(t: &str, lit: &str) -> Value {
    let lit = lit.trim();
    if lit == "NULL" {
        return Value::Null;
    }
    let base = strip_wrappers(t);
    match type_kind(base) {
        TypeKind::Array | TypeKind::Json => parse_value(base, lit),
        _ => parse_value(base, &escape_for_parse(&unquote(lit))),
    }
}

/// `parse_value` expects TSV-escaped text; an already unquoted literal must not be unescaped twice.
fn escape_for_parse(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n")
}

/// The header and body of a `TabSeparatedWithNamesAndTypes` response.
fn parse_tsv(body: &str, limit: Option<usize>) -> Option<ResultSet> {
    // The body ends with the newline of its last row; without stripping it a one-column result would
    // gain an empty row.
    let mut lines = body.strip_suffix('\n').unwrap_or(body).split('\n');
    let names = lines.next()?;
    let types = lines.next()?;
    let names: Vec<String> = names.split('\t').map(unescape).collect();
    let types: Vec<String> = types.split('\t').map(unescape).collect();
    if names.len() != types.len() {
        return None;
    }
    let mut rows = Vec::new();
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != names.len() {
            return None;
        }
        rows.push(fields.iter().zip(&types).map(|(f, t)| parse_value(t, f)).collect::<Vec<_>>());
    }
    let truncated = limit.is_some_and(|l| rows.len() > l);
    if let Some(l) = limit {
        rows.truncate(l);
    }
    let columns = names
        .into_iter()
        .zip(types)
        .map(|(name, ty)| Column { kind: type_kind(&ty), name, type_name: ty, origin: None })
        .collect();
    Some(ResultSet { columns, rows, truncated })
}

/// Whether the statement already says how to format its output (`... FORMAT JSON`), in which case the
/// text is returned as it came.
fn has_format_clause(sql: &str) -> bool {
    let t = sql.trim().trim_end_matches(';').to_ascii_uppercase();
    t.rsplit_once(" FORMAT ").is_some_and(|(_, f)| f.split_whitespace().count() == 1)
}

// ------------------------------------------------------------------------------------ the contract

#[async_trait]
impl Connection for ClickHouseConnection {
    fn kind(&self) -> DatabaseKind {
        DatabaseKind::ClickHouse
    }

    async fn server_info(&self) -> Result<ServerInfo> {
        Ok(self.info.clone())
    }

    async fn execute(&self, sql: &str, options: &ExecOptions) -> Result<StatementOutcome> {
        let raw = self.run(sql, options.row_limit).await?;
        let written = raw
            .headers
            .get("x-clickhouse-summary")
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .and_then(|v| {
                v.get("written_rows")
                    .and_then(|w| w.as_str().and_then(|s| s.parse::<u64>().ok()).or(w.as_u64()))
            })
            .filter(|n| *n > 0);
        let result = if raw.body.is_empty() {
            None
        } else if has_format_clause(sql) {
            // The user asked for a specific format: show it verbatim in one column.
            Some(ResultSet {
                columns: vec![Column {
                    name: "output".into(),
                    type_name: "String".into(),
                    kind: TypeKind::Text,
                    origin: None,
                }],
                rows: raw.body.lines().map(|l| vec![Value::Text(l.to_string())]).collect(),
                truncated: false,
            })
        } else {
            Some(
                parse_tsv(&raw.body, options.row_limit)
                    .ok_or_else(|| Error::other("could not read the server's result"))?,
            )
        };
        Ok(StatementOutcome { result, rows_affected: written, notices: Vec::new(), elapsed: raw.elapsed })
    }

    async fn cancel(&self) -> Result<()> {
        let id = self.current.lock().unwrap().clone();
        let Some(id) = id else { return Ok(()) };
        self.cancel_requested.store(true, Ordering::SeqCst);
        let ep = self.ep.clone();
        // A different request without the session: the session is busy with the statement being killed.
        let r = tokio::task::spawn_blocking(move || {
            ep.post(
                &format!("KILL QUERY WHERE query_id = '{id}' SYNC"),
                &[("default_format", "TabSeparated".into())],
                false,
            )
        })
        .await
        .map_err(Error::other)?;
        r.map(|_| ()).map_err(|f| failure_to_error(f, ""))
    }

    async fn introspect(&self) -> Result<Catalog> {
        introspect(self).await
    }

    async fn explain(&self, sql: &str, analyze: bool) -> Result<Plan> {
        if analyze {
            return Err(Error::Unsupported("ClickHouse has no EXPLAIN ANALYZE".into()));
        }
        // `json = 1` gives the tree with typed fields (ClickHouse 21.x and later); older servers have no
        // EXPLAIN at all and say so with a syntax error.
        let stmt =
            format!("EXPLAIN json = 1, description = 1, indexes = 1 {}", sql.trim().trim_end_matches(';'));
        let raw = match self.run(&stmt, None).await {
            Ok(r) => r,
            Err(Error::Query { code: Some(c), message, .. }) if c == "62" || c == "115" => {
                return Err(Error::Unsupported(format!(
                    "this ClickHouse server cannot EXPLAIN this way: {message}"
                )));
            }
            Err(e) => return Err(e),
        };
        let rs =
            parse_tsv(&raw.body, None).ok_or_else(|| Error::other("could not read the EXPLAIN result"))?;
        let text =
            rs.rows.iter().filter_map(|r| r.first().map(|v| v.display())).collect::<Vec<_>>().join("\n");
        plan_ch::parse_json(&text)
    }

    async fn in_transaction(&self) -> bool {
        // Over HTTP every statement is its own unit; there is nothing to be "inside" of.
        false
    }

    async fn close(&self) -> Result<()> {
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

// ------------------------------------------------------------------------------------ introspection

/// A query whose result is read by column name, so a column a version does not have is simply absent.
async fn named(conn: &ClickHouseConnection, sql: &str) -> Result<Vec<HashMap<String, Value>>> {
    let raw = conn.run(sql, None).await?;
    let rs =
        parse_tsv(&raw.body, None).unwrap_or(ResultSet { columns: vec![], rows: vec![], truncated: false });
    Ok(rs
        .rows
        .into_iter()
        .map(|r| rs.columns.iter().map(|c| c.name.clone()).zip(r).collect::<HashMap<_, _>>())
        .collect())
}

fn s(row: &HashMap<String, Value>, k: &str) -> String {
    row.get(k).map(|v| v.display()).unwrap_or_default()
}

fn opt(row: &HashMap<String, Value>, k: &str) -> Option<String> {
    Some(s(row, k)).filter(|v| !v.is_empty())
}

async fn introspect(conn: &ClickHouseConnection) -> Result<Catalog> {
    const SYSTEM: &str = "('system', 'INFORMATION_SCHEMA', 'information_schema')";
    let dbs =
        named(conn, &format!("SELECT name FROM system.databases WHERE name NOT IN {SYSTEM} ORDER BY name"))
            .await?;
    let mut schemas: Vec<Schema> =
        dbs.iter().map(|d| Schema { name: s(d, "name"), ..Default::default() }).collect();
    let sidx: HashMap<String, usize> = schemas.iter().enumerate().map(|(i, x)| (x.name.clone(), i)).collect();
    let mut tidx: HashMap<(usize, String), usize> = HashMap::new();

    // `SELECT *` so the columns a server version lacks (comment, total_rows, sorting_key on old servers)
    // are absent instead of an error.
    for t in named(conn, &format!("SELECT * FROM system.tables WHERE database NOT IN {SYSTEM} AND NOT is_temporary ORDER BY database, name")).await? {
        let Some(&si) = sidx.get(&s(&t, "database")) else { continue };
        let engine = s(&t, "engine");
        let is_view = engine.ends_with("View");
        let mut options = vec![TableOption { name: "ENGINE".into(), value: engine.clone() }];
        for (col, label) in [
            ("sorting_key", "ORDER BY"),
            ("primary_key", "PRIMARY KEY (not unique)"),
            ("partition_key", "PARTITION BY"),
            ("sampling_key", "SAMPLE BY"),
        ] {
            if let Some(v) = opt(&t, col) {
                options.push(TableOption { name: label.into(), value: v });
            }
        }
        let name = s(&t, "name");
        schemas[si].tables.push(Table {
            name: name.clone(),
            kind: if engine == "MaterializedView" { TableKind::MaterializedView } else if is_view { TableKind::View } else { TableKind::Table },
            row_estimate: t.get("total_rows").and_then(|v| match v {
                Value::UInt(u) => Some(*u),
                Value::Int(i) => u64::try_from(*i).ok(),
                _ => None,
            }),
            comment: opt(&t, "comment"),
            view_definition: is_view.then(|| s(&t, "create_table_query")).filter(|d| !d.is_empty()),
            options,
            ..Default::default()
        });
        tidx.insert((si, name), schemas[si].tables.len() - 1);
    }

    // `position` orders the columns of a table; servers from before it existed list them in table order
    // anyway, so the query is retried without it (an "unknown identifier" error, code 47).
    let columns_sql =
        format!("SELECT * FROM system.columns WHERE database NOT IN {SYSTEM} ORDER BY database, table");
    let columns = match named(conn, &format!("{columns_sql}, position")).await {
        Err(Error::Query { code: Some(code), .. }) if code == "47" => named(conn, &columns_sql).await?,
        other => other?,
    };
    for c in columns {
        let Some(&si) = sidx.get(&s(&c, "database")) else { continue };
        let Some(&ti) = tidx.get(&(si, s(&c, "table"))) else { continue };
        let data_type = s(&c, "type");
        let default_kind = s(&c, "default_kind");
        let expr = opt(&c, "default_expression");
        let (default, generated) = match (default_kind.as_str(), expr) {
            ("MATERIALIZED", Some(e)) => (None, Some(GeneratedColumn { expression: e, stored: true })),
            ("ALIAS", Some(e)) => (None, Some(GeneratedColumn { expression: e, stored: false })),
            (_, e) => (e, None),
        };
        let mut comment = opt(&c, "comment");
        if matches!(c.get("is_in_sorting_key"), Some(Value::Int(1) | Value::Bool(true))) {
            let note = "in the sorting key";
            comment = Some(comment.map_or(note.to_string(), |x| format!("{x} ({note})")));
        }
        schemas[si].tables[ti].columns.push(ColumnInfo {
            name: s(&c, "name"),
            kind: type_kind(&data_type),
            nullable: data_type.starts_with("Nullable(") || data_type.starts_with("LowCardinality(Nullable("),
            data_type,
            default,
            comment,
            geometry: None,
            identity: None,
            generated,
        });
    }

    // Data-skipping indexes; the table is newer than the HTTP interface, so older servers have none.
    if let Ok(rows) =
        named(conn, &format!("SELECT * FROM system.data_skipping_indices WHERE database NOT IN {SYSTEM}"))
            .await
    {
        for r in rows {
            let Some(&si) = sidx.get(&s(&r, "database")) else { continue };
            let Some(&ti) = tidx.get(&(si, s(&r, "table"))) else { continue };
            let expr = opt(&r, "expr").or_else(|| opt(&r, "expression")).unwrap_or_default();
            schemas[si].tables[ti].indexes.push(Index {
                name: s(&r, "name"),
                columns: vec![],
                method: opt(&r, "type"),
                definition: Some(format!(
                    "INDEX {} {expr} TYPE {} GRANULARITY {}",
                    s(&r, "name"),
                    s(&r, "type"),
                    s(&r, "granularity")
                )),
                has_expressions: true,
                ..Default::default()
            });
        }
    }
    let default_schema =
        named(conn, "SELECT currentDatabase() AS db").await.ok().and_then(|r| r.first().map(|x| s(x, "db")));
    Ok(Catalog { schemas, default_schema })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tsv_escapes() {
        assert_eq!(unescape("a\\tb\\nc\\\\d\\'e"), "a\tb\nc\\d'e");
        assert_eq!(unescape("plain"), "plain");
    }

    #[test]
    fn exceptions_and_positions() {
        let body = "Code: 62. DB::Exception: Syntax error: failed at position 1 (line 1, col 1): SELEC 1. Expected one of: Query. (SYNTAX_ERROR) (version 24.8.1)\n";
        let (code, msg) = parse_exception(body);
        assert_eq!(code, Some(62));
        assert!(msg.starts_with("Syntax error: failed at position 1"));
        assert_eq!(server_position(&msg), Some(0));
        let (code, _) =
            parse_exception("Code: 394. DB::Exception: Query was cancelled. (QUERY_WAS_CANCELLED)");
        assert_eq!(code, Some(394));
    }

    #[test]
    fn scalar_types() {
        assert_eq!(parse_value("Int32", "-5"), Value::Int(-5));
        assert_eq!(parse_value("UInt64", "18446744073709551615"), Value::UInt(u64::MAX));
        assert_eq!(
            parse_value("Int128", "170141183460469231731687303715884105727"),
            Value::Decimal("170141183460469231731687303715884105727".into())
        );
        assert_eq!(parse_value("Decimal(10, 2)", "2.50"), Value::Decimal("2.50".into()));
        // Newer servers drop trailing zeros; the declared scale puts them back.
        assert_eq!(parse_value("Decimal(18, 2)", "2.5"), Value::Decimal("2.50".into()));
        assert_eq!(parse_value("Decimal(18, 3)", "-7"), Value::Decimal("-7.000".into()));
        assert_eq!(parse_value("Nullable(Decimal(9, 0))", "42"), Value::Decimal("42".into()));
        assert_eq!(parse_value("Decimal(38, 4)", "1.23456"), Value::Decimal("1.23456".into()));
        assert_eq!(parse_value("Float64", "inf"), Value::Float(f64::INFINITY));
        assert_eq!(parse_value("String", "h\\tx"), Value::Text("h\tx".into()));
        assert_eq!(
            parse_value("UUID", "123e4567-e89b-12d3-a456-426614174000"),
            Value::Uuid("123e4567-e89b-12d3-a456-426614174000".into())
        );
        assert_eq!(
            parse_value("DateTime64(3)", "2024-02-29 13:14:15.123"),
            Value::Timestamp("2024-02-29 13:14:15.123".into())
        );
        assert_eq!(parse_value("IPv4", "10.0.0.1"), Value::Text("10.0.0.1".into()));
        assert_eq!(parse_value("Bool", "true"), Value::Bool(true));
    }

    #[test]
    fn wrappers_are_transparent() {
        assert_eq!(parse_value("Nullable(Int32)", "\\N"), Value::Null);
        assert_eq!(parse_value("Nullable(Int32)", "7"), Value::Int(7));
        assert_eq!(parse_value("LowCardinality(String)", "abc"), Value::Text("abc".into()));
        assert_eq!(parse_value("LowCardinality(Nullable(String))", "\\N"), Value::Null);
        assert_eq!(type_kind("LowCardinality(Nullable(String))"), TypeKind::Text);
        assert_eq!(type_kind("Nullable(DateTime64(3, 'UTC'))"), TypeKind::Timestamp);
    }

    #[test]
    fn arrays_tuples_maps() {
        assert_eq!(
            parse_value("Array(Int32)", "[1,2,3]"),
            Value::Array(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
        );
        assert_eq!(
            parse_value("Array(String)", "['a','b,c','it\\'s']"),
            Value::Array(vec![
                Value::Text("a".into()),
                Value::Text("b,c".into()),
                Value::Text("it's".into())
            ])
        );
        assert_eq!(
            parse_value("Array(Nullable(Int32))", "[1,NULL]"),
            Value::Array(vec![Value::Int(1), Value::Null])
        );
        assert_eq!(parse_value("Array(Int32)", "[]"), Value::Array(vec![]));
        assert_eq!(parse_value("Tuple(Int32, String)", "(1,'x')"), Value::Json("[1,\"x\"]".into()));
        assert_eq!(
            parse_value("Tuple(a Int32, b String)", "(1,'x')"),
            Value::Json("{\"a\":1,\"b\":\"x\"}".into())
        );
        assert_eq!(
            parse_value("Map(String, Int32)", "{'a':1,'b':2}"),
            Value::Json("{\"a\":1,\"b\":2}".into())
        );
        assert_eq!(
            parse_value("Map(Int32, String)", "{1:'a'}"),
            Value::Json("[{\"key\":1,\"value\":\"a\"}]".into())
        );
        assert_eq!(
            parse_value("Array(Array(Int32))", "[[1,2],[3]]"),
            Value::Array(vec![
                Value::Array(vec![Value::Int(1), Value::Int(2)]),
                Value::Array(vec![Value::Int(3)]),
            ])
        );
    }

    #[test]
    fn result_tables() {
        let body = "a\tb\nInt32\tNullable(String)\n1\tx\n2\t\\N\n";
        let rs = parse_tsv(body, None).unwrap();
        assert_eq!(rs.columns[1].type_name, "Nullable(String)");
        assert_eq!(
            rs.rows,
            vec![vec![Value::Int(1), Value::Text("x".into())], vec![Value::Int(2), Value::Null]]
        );
        // one more row than the limit means "truncated"
        let rs = parse_tsv("a\nInt32\n1\n2\n3\n", Some(2)).unwrap();
        assert_eq!((rs.rows.len(), rs.truncated), (2, true));
        // a result of one empty string is a row, not nothing
        assert_eq!(parse_tsv("s\nString\n\n", None).unwrap().rows.len(), 1);
    }

    #[test]
    fn explicit_format_is_left_alone() {
        assert!(has_format_clause("SELECT 1 FORMAT JSON"));
        assert!(has_format_clause("select 1 format CSV;"));
        assert!(!has_format_clause("SELECT format FROM t"));
        assert!(!has_format_clause("SELECT 1"));
    }
}
