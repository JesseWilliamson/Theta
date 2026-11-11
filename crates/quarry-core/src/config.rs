use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseKind {
    Postgres,
    MySql,
    Sqlite,
}

/// Everything the application needs to know about a kind of database that is not SQL
/// dialect behaviour: how it is presented, addressed and configured. The connection dialog,
/// URL parser and defaults are driven by this table, so **adding a database kind is one
/// entry here (plus the enum variant and a driver)**; no dialog layout changes.
#[derive(Debug)]
pub struct KindInfo {
    pub label: &'static str,
    /// Key of the glyph (`quarry_ui::DbIcon::from_key`); unknown keys get a generic one.
    pub icon: &'static str,
    pub default_port: Option<u16>,
    /// URL schemes (lower case, without `://`) that select this kind.
    pub url_schemes: &'static [&'static str],
    /// Extra words the kind is found by in the type-to-filter dropdown.
    pub keywords: &'static [&'static str],
    /// Form sections that apply.
    pub sections: Sections,
    pub host_placeholder: &'static str,
    pub user_placeholder: &'static str,
    pub database_placeholder: &'static str,
    pub path_placeholder: &'static str,
}

/// Which parts of the connection form a kind uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sections {
    /// Host, port, user, password and database name.
    pub network: bool,
    /// TLS mode.
    pub ssl: bool,
    /// SSH tunnel.
    pub ssh: bool,
    /// A database file (path chooser) instead of a server address.
    pub file: bool,
    /// "Create if missing" for a file that does not exist yet.
    pub create_file: bool,
}

const SERVER: Sections = Sections { network: true, ssl: true, ssh: true, file: false, create_file: false };
const FILE: Sections = Sections { network: false, ssl: false, ssh: false, file: true, create_file: true };

static POSTGRES: KindInfo = KindInfo {
    label: "PostgreSQL",
    icon: "postgres",
    default_port: Some(5432),
    url_schemes: &["postgres", "postgresql"],
    keywords: &["postgis", "pg"],
    sections: SERVER,
    host_placeholder: "localhost",
    user_placeholder: "postgres",
    database_placeholder: "database name",
    path_placeholder: "",
};
static MYSQL: KindInfo = KindInfo {
    label: "MySQL / MariaDB",
    icon: "mysql",
    default_port: Some(3306),
    url_schemes: &["mysql", "mariadb"],
    keywords: &["maria"],
    sections: SERVER,
    host_placeholder: "localhost",
    user_placeholder: "root",
    database_placeholder: "database name",
    path_placeholder: "",
};
static SQLITE: KindInfo = KindInfo {
    label: "SQLite",
    icon: "sqlite",
    default_port: None,
    url_schemes: &["sqlite", "sqlite3"],
    keywords: &["file", "embedded"],
    sections: FILE,
    host_placeholder: "",
    user_placeholder: "",
    database_placeholder: "",
    path_placeholder: "/path/to/database.sqlite",
};

impl DatabaseKind {
    pub const ALL: &'static [DatabaseKind] =
        &[DatabaseKind::Postgres, DatabaseKind::MySql, DatabaseKind::Sqlite];

    /// The descriptor of this kind (the one place that lists per-kind facts).
    pub fn info(self) -> &'static KindInfo {
        match self {
            DatabaseKind::Postgres => &POSTGRES,
            DatabaseKind::MySql => &MYSQL,
            DatabaseKind::Sqlite => &SQLITE,
        }
    }

    /// The kind a URL scheme (`postgres`, `mariadb`, ...) selects.
    pub fn from_url_scheme(scheme: &str) -> Option<DatabaseKind> {
        let s = scheme.to_ascii_lowercase();
        Self::ALL.iter().copied().find(|k| k.info().url_schemes.contains(&s.as_str()))
    }

    pub fn label(self) -> &'static str {
        self.info().label
    }

    pub fn default_port(self) -> Option<u16> {
        self.info().default_port
    }

    /// File-based databases have a path instead of host/port/user.
    pub fn is_file_based(self) -> bool {
        self.info().sections.file
    }

    /// How this dialect quotes an identifier that needs quoting.
    pub fn quote_ident(self, ident: &str) -> String {
        match self {
            DatabaseKind::MySql => format!("`{}`", ident.replace('`', "``")),
            _ => format!("\"{}\"", ident.replace('"', "\"\"")),
        }
    }
}

#[cfg(test)]
mod kind_tests {
    use super::*;

    #[test]
    fn every_kind_has_a_consistent_descriptor() {
        let mut schemes = std::collections::HashSet::new();
        for k in DatabaseKind::ALL {
            let i = k.info();
            assert!(!i.label.is_empty() && !i.url_schemes.is_empty(), "{k:?}");
            // A server kind has a port to default; a file kind never shows network fields.
            assert_eq!(i.sections.network, i.default_port.is_some(), "{k:?}");
            assert!(!(i.sections.file && i.sections.network), "{k:?}");
            assert!(!(i.sections.file && i.sections.ssh), "{k:?}: no tunnel for a local file");
            for s in i.url_schemes {
                assert!(schemes.insert(*s), "scheme {s} claimed twice");
                assert_eq!(DatabaseKind::from_url_scheme(&s.to_uppercase()), Some(*k));
            }
        }
        assert_eq!(DatabaseKind::from_url_scheme("mariadb"), Some(DatabaseKind::MySql));
        assert_eq!(DatabaseKind::from_url_scheme("oracle"), None);
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
    Password {
        password: String,
    },
    /// Private key file (OpenSSH or PEM), optionally encrypted.
    KeyFile {
        path: String,
        passphrase: Option<String>,
    },
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
