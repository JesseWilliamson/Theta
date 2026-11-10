use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseKind {
    Postgres,
    MySql,
    Sqlite,
}

impl DatabaseKind {
    pub const ALL: &'static [DatabaseKind] = &[DatabaseKind::Postgres, DatabaseKind::MySql, DatabaseKind::Sqlite];

    pub fn label(self) -> &'static str {
        match self {
            DatabaseKind::Postgres => "PostgreSQL",
            DatabaseKind::MySql => "MySQL / MariaDB",
            DatabaseKind::Sqlite => "SQLite",
        }
    }

    pub fn default_port(self) -> Option<u16> {
        match self {
            DatabaseKind::Postgres => Some(5432),
            DatabaseKind::MySql => Some(3306),
            DatabaseKind::Sqlite => None,
        }
    }

    /// File-based databases have a path instead of host/port/user.
    pub fn is_file_based(self) -> bool {
        matches!(self, DatabaseKind::Sqlite)
    }

    /// How this dialect quotes an identifier that needs quoting.
    pub fn quote_ident(self, ident: &str) -> String {
        match self {
            DatabaseKind::MySql => format!("`{}`", ident.replace('`', "``")),
            _ => format!("\"{}\"", ident.replace('"', "\"\"")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SslMode {
    Disable,
    /// Try TLS, fall back to plaintext.
    #[default]
    Prefer,
    /// TLS required, certificate not verified.
    Require,
    /// TLS required and the certificate chain + hostname verified.
    VerifyFull,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum SshAuth {
    Password { password: String },
    /// Private key file (OpenSSH or PEM), optionally encrypted.
    KeyFile { path: String, passphrase: Option<String> },
    /// Use the running ssh-agent (`SSH_AUTH_SOCK`).
    Agent,
}

/// Reach the database through an SSH jump host: the driver layer opens a local
/// port that forwards to `ConnectionConfig::host:port` *as seen from the SSH
/// server*, then connects the database client to that local port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: SshAuth,
}

/// Everything needed to open a connection. Secrets (`password`, SSH password /
/// passphrase) live in this struct in memory only: the app strips them before
/// writing connections to disk and stores them in the OS keychain instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionConfig {
    /// Stable unique id (UUID text). Never shown to the user.
    pub id: String,
    /// User-facing name, e.g. "prod (read-only)".
    pub name: String,
    pub kind: DatabaseKind,
    /// Network databases.
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// Database name; empty means the server default.
    #[serde(default)]
    pub database: String,
    /// File-based databases: path to the file (`:memory:` allowed for SQLite).
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub ssl_mode: SslMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<SshConfig>,
    /// When set, the app refuses to run statements that write.
    #[serde(default)]
    pub read_only: bool,
    /// Optional accent colour (`#rrggbb`) to mark e.g. production connections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

impl ConnectionConfig {
    pub fn new(id: impl Into<String>, name: impl Into<String>, kind: DatabaseKind) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            kind,
            host: if kind.is_file_based() { String::new() } else { "localhost".into() },
            port: kind.default_port().unwrap_or(0),
            user: String::new(),
            password: None,
            database: String::new(),
            path: String::new(),
            ssl_mode: SslMode::default(),
            ssh: None,
            read_only: false,
            color: None,
        }
    }
}
