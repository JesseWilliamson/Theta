//! Parse `postgres://user:pass@host:5432/db?sslmode=require`-style URLs into
//! connection fields (used by `--connect` and the dialog's "paste URL" field).

use theta_core::Family;
use theta_core::{ConnectionConfig, DatabaseKind, SslMode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUrl {
    pub kind: DatabaseKind,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub database: String,
    pub path: String,
    pub ssl_mode: Option<SslMode>,
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_ssl(value: &str) -> Option<SslMode> {
    match value.to_ascii_lowercase().as_str() {
        "disable" | "disabled" | "false" => Some(SslMode::Disable),
        "prefer" | "preferred" | "allow" => Some(SslMode::Prefer),
        "require" | "required" | "true" => Some(SslMode::Require),
        "verify-full" | "verify_identity" | "verify-ca" | "verify_ca" => Some(SslMode::VerifyFull),
        _ => None,
    }
}

pub fn parse_connection_url(input: &str) -> Result<ParsedUrl, String> {
    let input = input.trim();
    let (scheme, rest) = input
        .split_once("://")
        .or_else(|| input.strip_prefix("sqlite:").map(|r| ("sqlite", r)))
        .or_else(|| input.strip_prefix("duckdb:").map(|r| ("duckdb", r)))
        .ok_or_else(|| "expected a URL such as postgres://user:password@host:5432/database".to_string())?;
    let kind = DatabaseKind::from_url_scheme(scheme).ok_or_else(|| {
        let known: Vec<String> =
            DatabaseKind::ALL.iter().map(|k| format!("{}://", k.info().url_schemes[0])).collect();
        format!("unsupported scheme \"{scheme}\" (use {})", known.join(", "))
    })?;

    if kind.is_file_based() {
        // `sqlite:///abs/path`, `sqlite://relative.db`, `sqlite::memory:`, `sqlite:path`; DuckDB the same.
        let path = rest.split('?').next().unwrap_or("");
        let path = percent_decode(path);
        if path.is_empty() {
            return Err(format!("the {} URL has no file path ({scheme}:///path/to/file)", kind.label()));
        }
        return Ok(ParsedUrl {
            kind,
            host: String::new(),
            port: 0,
            user: String::new(),
            password: None,
            database: String::new(),
            path,
            ssl_mode: None,
        });
    }

    let url = url::Url::parse(input).map_err(|e| format!("invalid URL: {e}"))?;
    let host = url.host_str().unwrap_or("").to_string();
    if host.is_empty() {
        return Err("the URL has no host".into());
    }
    let query = |names: &[&str]| {
        url.query_pairs()
            .find(|(k, _)| names.iter().any(|n| k.eq_ignore_ascii_case(n)))
            .map(|(_, v)| v.into_owned())
    };
    let mut ssl_mode = query(&["sslmode", "ssl-mode", "ssl_mode"]).and_then(|v| parse_ssl(&v));
    let mut port = url.port().or(kind.default_port()).unwrap_or(0);
    let mut database = percent_decode(url.path().trim_start_matches('/'));
    match kind.family() {
        Family::SqlServer => {
            // ADO/JDBC spellings: `encrypt=true|false`, `trustServerCertificate=true`, `database=name`.
            if let Some(e) = query(&["encrypt"]) {
                let on = matches!(e.to_ascii_lowercase().as_str(), "true" | "yes" | "strict" | "mandatory");
                let trust = query(&["trustServerCertificate"])
                    .is_some_and(|t| matches!(t.to_ascii_lowercase().as_str(), "true" | "yes"));
                ssl_mode = Some(match (on, trust) {
                    (false, _) => SslMode::Disable,
                    (true, true) => SslMode::Require,
                    (true, false) => SslMode::VerifyFull,
                });
            }
            if database.is_empty() {
                database = query(&["database", "databaseName", "initial catalog"]).unwrap_or_default();
            }
        }
        Family::ClickHouse => {
            // `clickhouses://` and `?secure=true` mean HTTPS; the HTTPS port differs.
            let secure = scheme.eq_ignore_ascii_case("clickhouses")
                || query(&["secure", "ssl"])
                    .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes"));
            if secure && ssl_mode.is_none() {
                ssl_mode = Some(SslMode::VerifyFull);
            }
            if url.port().is_none() && ssl_mode.is_some_and(|m| m != SslMode::Disable) {
                port = kind.info().tls_port.unwrap_or(port);
            }
            if database.is_empty() {
                database = query(&["database"]).unwrap_or_default();
            }
        }
        _ => {}
    }
    Ok(ParsedUrl {
        kind,
        host,
        port,
        user: percent_decode(url.username()),
        password: url.password().map(percent_decode),
        database,
        path: String::new(),
        ssl_mode,
    })
}

impl ParsedUrl {
    /// A suggested display name: database @ host, or the file name.
    pub fn suggested_name(&self) -> String {
        match self.kind.family() {
            Family::Sqlite | Family::DuckDb => std::path::Path::new(&self.path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.path.clone()),
            _ if !self.database.is_empty() => format!("{} @ {}", self.database, self.host),
            _ => self.host.clone(),
        }
    }

    pub fn apply_to(&self, config: &mut ConnectionConfig) {
        config.kind = self.kind;
        config.host = self.host.clone();
        config.port = self.port;
        config.user = self.user.clone();
        config.password = self.password.clone();
        config.database = self.database.clone();
        config.path = self.path.clone();
        if let Some(ssl) = self.ssl_mode {
            config.ssl_mode = ssl;
        }
    }

    pub fn into_config(self, id: impl Into<String>, name: Option<String>) -> ConnectionConfig {
        let name = name.unwrap_or_else(|| self.suggested_name());
        let mut c = ConnectionConfig::new(id, name, self.kind);
        self.apply_to(&mut c);
        c
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn new_kinds_parse() {
        let c = parse_connection_url("cockroachdb://root@h:26257/defaultdb?sslmode=verify-full").unwrap();
        assert_eq!(
            (c.kind, c.port, c.ssl_mode),
            (DatabaseKind::CockroachDb, 26257, Some(SslMode::VerifyFull))
        );
        let r =
            parse_connection_url("redshift://awsuser:pw@c.x.eu-west-1.redshift.amazonaws.com/dev").unwrap();
        assert_eq!((r.kind, r.port, r.database.as_str()), (DatabaseKind::Redshift, 5439, "dev"));
        assert_eq!(parse_connection_url("timescaledb://h/db").unwrap().kind, DatabaseKind::TimescaleDb);
        assert_eq!(parse_connection_url("yugabytedb://h/db").unwrap().port, 5433);
        assert_eq!(parse_connection_url("tidb://root@h/test").unwrap().port, 4000);
        let d = parse_connection_url("duckdb:///tmp/a.duckdb").unwrap();
        assert_eq!((d.kind, d.path.as_str()), (DatabaseKind::DuckDb, "/tmp/a.duckdb"));
        assert_eq!(parse_connection_url("duckdb::memory:").unwrap().path, ":memory:");
        let m =
            parse_connection_url("sqlserver://sa:pw@h:1433/master?encrypt=true&trustServerCertificate=true")
                .unwrap();
        assert_eq!(
            (m.kind, m.database.as_str(), m.ssl_mode),
            (DatabaseKind::SqlServer, "master", Some(SslMode::Require))
        );
        let m = parse_connection_url("mssql://sa@h?encrypt=false&database=app").unwrap();
        assert_eq!((m.database.as_str(), m.ssl_mode), ("app", Some(SslMode::Disable)));
        let c = parse_connection_url("clickhouses://default@h/db").unwrap();
        assert_eq!((c.kind, c.port, c.ssl_mode), (DatabaseKind::ClickHouse, 8443, Some(SslMode::VerifyFull)));
        assert_eq!(parse_connection_url("clickhouse://h").unwrap().port, 8123);
    }

    use super::*;

    #[test]
    fn postgres_full_url() {
        let p = parse_connection_url("postgres://postgres:postgres@127.0.0.1:5432/quarry_demo").unwrap();
        assert_eq!(p.kind, DatabaseKind::Postgres);
        assert_eq!(p.host, "127.0.0.1");
        assert_eq!(p.port, 5432);
        assert_eq!(p.user, "postgres");
        assert_eq!(p.password.as_deref(), Some("postgres"));
        assert_eq!(p.database, "quarry_demo");
        assert_eq!(p.suggested_name(), "quarry_demo @ 127.0.0.1");
    }

    #[test]
    fn default_ports_and_schemes() {
        assert_eq!(parse_connection_url("postgresql://h/db").unwrap().port, 5432);
        let m = parse_connection_url("mysql://quarry:quarry@127.0.0.1/quarry_demo").unwrap();
        assert_eq!((m.kind, m.port), (DatabaseKind::MySql, 3306));
        assert_eq!(parse_connection_url("mariadb://h").unwrap().kind, DatabaseKind::MySql);
    }

    #[test]
    fn credentials_are_percent_decoded() {
        let p = parse_connection_url("postgres://us%40er:p%3Aw%2Fd@h:6543/d%20b").unwrap();
        assert_eq!(p.user, "us@er");
        assert_eq!(p.password.as_deref(), Some("p:w/d"));
        assert_eq!(p.database, "d b");
        assert_eq!(p.port, 6543);
    }

    #[test]
    fn sslmode_query_parameter() {
        let p = parse_connection_url("postgres://h/db?sslmode=require").unwrap();
        assert_eq!(p.ssl_mode, Some(SslMode::Require));
        let p = parse_connection_url("mysql://h/db?ssl-mode=DISABLED").unwrap();
        assert_eq!(p.ssl_mode, Some(SslMode::Disable));
        assert_eq!(parse_connection_url("postgres://h/db").unwrap().ssl_mode, None);
    }

    #[test]
    fn sqlite_paths() {
        let p = parse_connection_url("sqlite:///tmp/demo.sqlite").unwrap();
        assert_eq!(p.kind, DatabaseKind::Sqlite);
        assert_eq!(p.path, "/tmp/demo.sqlite");
        assert_eq!(p.suggested_name(), "demo.sqlite");
        assert_eq!(parse_connection_url("sqlite://rel.db").unwrap().path, "rel.db");
        assert_eq!(parse_connection_url("sqlite::memory:").unwrap().path, ":memory:");
    }

    #[test]
    fn errors_are_explained() {
        assert!(parse_connection_url("not a url").unwrap_err().contains("expected a URL"));
        assert!(parse_connection_url("oracle://h/db").unwrap_err().contains("unsupported scheme"));
        assert!(parse_connection_url("postgres:///db").unwrap_err().contains("no host"));
        assert!(parse_connection_url("sqlite://").unwrap_err().contains("no file path"));
    }

    #[test]
    fn into_config_fills_every_field() {
        let c = parse_connection_url("postgres://u:p@h:1/d").unwrap().into_config("id1", None);
        assert_eq!((c.id.as_str(), c.name.as_str()), ("id1", "d @ h"));
        assert_eq!((c.host.as_str(), c.port, c.user.as_str()), ("h", 1, "u"));
        assert_eq!(c.password.as_deref(), Some("p"));
    }
}
