use crate::txn::TransactionConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseKind {
    Postgres,
    MySql,
    Sqlite,
    /// PostgreSQL wire protocol, its own SQL layer and catalogs (`crdb_internal`).
    CockroachDb,
    /// PostgreSQL 8.0 wire protocol with a very different catalog and DDL surface.
    Redshift,
    /// PostgreSQL with the TimescaleDB extension.
    TimescaleDb,
    /// PostgreSQL 11 compatible distributed SQL (YSQL).
    YugabyteDb,
    /// MySQL wire protocol, distributed storage engine.
    TiDb,
    DuckDb,
    SqlServer,
    ClickHouse,
}

/// The SQL dialect and wire family a kind belongs to. Lexing, formatting, completion and most
/// generated SQL depend on this, not on the product: CockroachDB, Redshift, TimescaleDB and
/// YugabyteDB are `Postgres`, TiDB is `MySql`. Code that must tell two products of one family apart
/// (a DDL restriction, a catalog difference) matches on [`DatabaseKind`] itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    Postgres,
    MySql,
    Sqlite,
    DuckDb,
    SqlServer,
    ClickHouse,
}

impl Family {
    /// Dialects that lex and parse like PostgreSQL (dollar quoting, `::` casts, `ILIKE`, `RETURNING`,
    /// `\x` bytea...). DuckDB copied PostgreSQL's grammar on purpose.
    pub fn pg_like(self) -> bool {
        matches!(self, Family::Postgres | Family::DuckDb)
    }
}

/// A capability a kind may lack. [`DatabaseKind::unsupported`] answers with the reason shown to the
/// user, so a menu entry or button can be disabled with an explanation instead of sending SQL that
/// the server would reject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    /// `Connection::explain` without ANALYZE.
    Explain,
    ExplainAnalyze,
    /// Aborting a running statement.
    Cancel,
    /// Modify Table on an existing table.
    ModifyTable,
    /// New Table in the table editor.
    CreateTable,
    /// CSV import into a table.
    Import,
    /// Editing result rows in the grid.
    EditRows,
    /// BEGIN / COMMIT across statements.
    Transactions,
    /// The Sessions tab: list server sessions, cancel a query, terminate a session.
    Sessions,
    /// The Roles tab: list roles / users and their privileges.
    Roles,
    /// Changing roles: create, alter, grant, revoke, drop.
    RoleEdit,
    /// Export Database: Theta's own SQL dump.
    Dump,
    /// Search Across Tables: one generated query per table that looks for a text in every column.
    TableSearch,
}

/// Why each feature is missing, `None` when it works. One row per kind keeps the answer in one place.
#[derive(Debug, Clone, Copy, Default)]
pub struct Features {
    pub explain: Option<&'static str>,
    pub explain_analyze: Option<&'static str>,
    pub cancel: Option<&'static str>,
    pub modify_table: Option<&'static str>,
    pub create_table: Option<&'static str>,
    pub import: Option<&'static str>,
    pub edit_rows: Option<&'static str>,
    pub transactions: Option<&'static str>,
    pub sessions: Option<&'static str>,
    pub roles: Option<&'static str>,
    pub role_edit: Option<&'static str>,
    pub dump: Option<&'static str>,
    pub table_search: Option<&'static str>,
}

/// Everything the application needs to know about a kind of database that is not SQL
/// dialect behaviour: how it is presented, addressed and configured. The connection dialog,
/// URL parser and defaults are driven by this table, so **adding a database kind is one
/// entry here (plus the enum variant and a driver)**; no dialog layout changes.
#[derive(Debug)]
pub struct KindInfo {
    pub label: &'static str,
    /// Key of the glyph (`theta_ui::DbIcon::from_key`); unknown keys get a generic one.
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
    pub family: Family,
    pub features: Features,
    /// Labels of the connection dialog's TLS dropdown, in [`SslMode`] order (disable, prefer, require,
    /// verify-full); `None` hides the entry. Products spell this differently (SQL Server "Encrypt",
    /// ClickHouse "HTTPS").
    pub ssl_labels: [Option<&'static str>; 4],
    /// Port to suggest when the user switches the dialog to this kind with TLS on, when it differs
    /// (ClickHouse HTTPS is 8443).
    pub tls_port: Option<u16>,
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

const PG_SSL: [Option<&str>; 4] =
    [Some("Disable"), Some("Prefer"), Some("Require"), Some("Verify certificate and host")];
const NO_FEATURES: Features = Features {
    explain: None,
    explain_analyze: None,
    cancel: None,
    modify_table: None,
    create_table: None,
    import: None,
    edit_rows: None,
    transactions: None,
    sessions: None,
    roles: None,
    role_edit: None,
    dump: None,
    table_search: None,
};

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
    family: Family::Postgres,
    features: NO_FEATURES,
    ssl_labels: PG_SSL,
    tls_port: None,
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
    family: Family::MySql,
    features: NO_FEATURES,
    ssl_labels: PG_SSL,
    tls_port: None,
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
    family: Family::Sqlite,
    features: Features {
        sessions: Some(
            "an embedded database runs inside Theta's own process and has no server sessions to list",
        ),
        roles: Some(
            "an embedded database has no users or roles; access is whatever the file's permissions allow",
        ),
        role_edit: Some(
            "an embedded database has no users or roles; access is whatever the file's permissions allow",
        ),
        ..NO_FEATURES
    },
    ssl_labels: [None; 4],
    tls_port: None,
};

static COCKROACH: KindInfo = KindInfo {
    label: "CockroachDB",
    icon: "cockroachdb",
    default_port: Some(26257),
    url_schemes: &["cockroachdb", "cockroach", "crdb"],
    keywords: &["cockroach", "crdb", "distributed", "postgres wire"],
    sections: SERVER,
    host_placeholder: "localhost",
    user_placeholder: "root",
    database_placeholder: "defaultdb",
    path_placeholder: "",
    family: Family::Postgres,
    features: Features {
        modify_table: Some(
            "CockroachDB runs schema changes as online jobs and refuses some of them (such as ALTER COLUMN TYPE) \
             inside a transaction, which is how the table editor applies a script; not supported yet",
        ),
        create_table: Some(
            "the generated CREATE TABLE (identity columns, types) is not verified against CockroachDB yet",
        ),
        sessions: Some(
            "CockroachDB has its own SHOW SESSIONS / CANCEL SESSION statements and no pg_cancel_backend; not supported yet",
        ),
        roles: Some(
            "CockroachDB's roles live in system tables with different columns than PostgreSQL's; not supported yet",
        ),
        role_edit: Some(
            "CockroachDB's roles live in system tables with different columns than PostgreSQL's; not supported yet",
        ),
        dump: Some(
            "the generated CREATE TABLE is not verified against CockroachDB yet; use Run SQL Script to restore a dump made elsewhere",
        ),
        ..NO_FEATURES
    },
    ssl_labels: PG_SSL,
    tls_port: None,
};
static REDSHIFT: KindInfo = KindInfo {
    label: "Amazon Redshift",
    icon: "redshift",
    default_port: Some(5439),
    url_schemes: &["redshift"],
    keywords: &["aws", "amazon", "warehouse"],
    sections: SERVER,
    host_placeholder: "cluster.xxxx.region.redshift.amazonaws.com",
    user_placeholder: "awsuser",
    database_placeholder: "dev",
    path_placeholder: "",
    family: Family::Postgres,
    features: Features {
        explain_analyze: Some(
            "Redshift has no EXPLAIN ANALYZE; run the query and read SVL_QUERY_REPORT instead",
        ),
        modify_table: Some(
            "Redshift cannot alter a column's type (except widening a VARCHAR), nullability or default, \
             so a table is changed with console statements",
        ),
        create_table: Some(
            "Redshift needs its own column attributes (IDENTITY, DISTKEY, SORTKEY, ENCODE); write CREATE TABLE in a console",
        ),
        import: Some("Redshift has no savepoints and loads data with COPY from S3; use COPY in a console"),
        edit_rows: Some(
            "primary keys are informational on Redshift (not enforced), so a row has no reliable identity",
        ),
        sessions: Some(
            "Redshift sessions are in STV_SESSIONS and its roles in PG_USER / SVV_*, not in the PostgreSQL catalogs this tab reads; not supported yet",
        ),
        roles: Some(
            "Redshift sessions are in STV_SESSIONS and its roles in PG_USER / SVV_*, not in the PostgreSQL catalogs this tab reads; not supported yet",
        ),
        role_edit: Some(
            "Redshift sessions are in STV_SESSIONS and its roles in PG_USER / SVV_*, not in the PostgreSQL catalogs this tab reads; not supported yet",
        ),
        dump: Some(
            "Redshift needs its own column attributes (DISTKEY, SORTKEY, ENCODE) that the generated CREATE TABLE does not carry; use UNLOAD or pg_dump",
        ),
        ..NO_FEATURES
    },
    ssl_labels: PG_SSL,
    tls_port: None,
};
static TIMESCALE: KindInfo = KindInfo {
    label: "TimescaleDB",
    icon: "timescaledb",
    default_port: Some(5432),
    url_schemes: &["timescaledb", "timescale", "tsdb"],
    keywords: &["timescale", "time series", "hypertable", "postgis"],
    sections: SERVER,
    host_placeholder: "localhost",
    user_placeholder: "postgres",
    database_placeholder: "database name",
    path_placeholder: "",
    family: Family::Postgres,
    features: NO_FEATURES,
    ssl_labels: PG_SSL,
    tls_port: None,
};
static YUGABYTE: KindInfo = KindInfo {
    label: "YugabyteDB",
    icon: "yugabytedb",
    default_port: Some(5433),
    url_schemes: &["yugabytedb", "yugabyte", "ysql"],
    keywords: &["yugabyte", "ysql", "distributed"],
    sections: SERVER,
    host_placeholder: "localhost",
    user_placeholder: "yugabyte",
    database_placeholder: "yugabyte",
    path_placeholder: "",
    family: Family::Postgres,
    features: Features {
        modify_table: Some(
            "YugabyteDB (YSQL) does not support every ALTER TABLE a PostgreSQL table editor emits (ALTER COLUMN TYPE \
             rewrites in particular); not supported yet",
        ),
        create_table: Some("the generated CREATE TABLE is not verified against YugabyteDB yet"),
        sessions: Some(
            "YugabyteDB is not verified against the PostgreSQL catalogs this tab reads (its sessions are spread over several nodes); not supported yet",
        ),
        roles: Some(
            "YugabyteDB is not verified against the PostgreSQL catalogs this tab reads (its sessions are spread over several nodes); not supported yet",
        ),
        role_edit: Some(
            "YugabyteDB is not verified against the PostgreSQL catalogs this tab reads (its sessions are spread over several nodes); not supported yet",
        ),
        dump: Some("the generated CREATE TABLE is not verified against YugabyteDB yet; use ysql_dump"),
        ..NO_FEATURES
    },
    ssl_labels: PG_SSL,
    tls_port: None,
};
static TIDB: KindInfo = KindInfo {
    label: "TiDB",
    icon: "tidb",
    default_port: Some(4000),
    url_schemes: &["tidb"],
    keywords: &["pingcap", "mysql wire", "distributed"],
    sections: SERVER,
    host_placeholder: "localhost",
    user_placeholder: "root",
    database_placeholder: "database name",
    path_placeholder: "",
    family: Family::MySql,
    features: Features {
        sessions: Some(
            "TiDB uses KILL TIDB with cluster-wide connection ids and has no pg-style lock views; not supported yet",
        ),
        roles: Some(
            "TiDB's mysql.user differs from MySQL's and roles are managed differently; not supported yet",
        ),
        role_edit: Some(
            "TiDB's mysql.user differs from MySQL's and roles are managed differently; not supported yet",
        ),
        dump: Some("the generated CREATE TABLE is not verified against TiDB yet; use mysqldump or Dumpling"),
        ..NO_FEATURES
    },
    ssl_labels: PG_SSL,
    tls_port: None,
};
static DUCKDB: KindInfo = KindInfo {
    label: "DuckDB",
    icon: "duckdb",
    default_port: None,
    url_schemes: &["duckdb"],
    keywords: &["file", "embedded", "parquet", "analytics", "olap"],
    sections: FILE,
    host_placeholder: "",
    user_placeholder: "",
    database_placeholder: "",
    path_placeholder: "/path/to/database.duckdb  (or :memory:)",
    family: Family::DuckDb,
    features: Features {
        import: Some(
            "DuckDB has no savepoints, which the import's per-batch recovery needs; use COPY or read_csv() in a console",
        ),
        sessions: Some(
            "an embedded database runs inside Theta's own process and has no server sessions to list",
        ),
        roles: Some(
            "an embedded database has no users or roles; access is whatever the file's permissions allow",
        ),
        role_edit: Some(
            "an embedded database has no users or roles; access is whatever the file's permissions allow",
        ),
        ..NO_FEATURES
    },
    ssl_labels: [None; 4],
    tls_port: None,
};
static SQLSERVER: KindInfo = KindInfo {
    label: "SQL Server",
    icon: "sqlserver",
    default_port: Some(1433),
    url_schemes: &["sqlserver", "mssql"],
    keywords: &["microsoft", "mssql", "tsql", "t-sql", "azure"],
    sections: SERVER,
    host_placeholder: "localhost",
    user_placeholder: "sa",
    database_placeholder: "master",
    path_placeholder: "",
    family: Family::SqlServer,
    features: Features {
        modify_table: Some("the table editor does not generate T-SQL ALTER TABLE yet"),
        create_table: Some("the table editor does not generate T-SQL CREATE TABLE yet"),
        import: Some("the CSV import uses savepoints, which T-SQL spells differently; not supported yet"),
        role_edit: Some(
            "Theta lists SQL Server logins, users and role membership (read-only); changing them needs T-SQL that is not generated yet",
        ),
        dump: Some(
            "the DDL generator does not emit T-SQL (CREATE TABLE with IDENTITY, bracket quoting) yet; Run SQL Script still restores a script made elsewhere",
        ),
        table_search: Some(
            "the search queries (CAST to NVARCHAR, CHARINDEX, TOP) are not written for T-SQL and no SQL Server was available to test them",
        ),
        ..NO_FEATURES
    },
    ssl_labels: [
        Some("Encrypt off"),
        None,
        Some("Encrypt on, trust server certificate"),
        Some("Encrypt on, verify certificate"),
    ],
    tls_port: None,
};
static CLICKHOUSE: KindInfo = KindInfo {
    label: "ClickHouse",
    icon: "clickhouse",
    default_port: Some(8123),
    url_schemes: &["clickhouse", "ch", "clickhouses"],
    keywords: &["olap", "columnar", "analytics", "http"],
    sections: SERVER,
    host_placeholder: "localhost",
    user_placeholder: "default",
    database_placeholder: "default",
    path_placeholder: "",
    family: Family::ClickHouse,
    features: Features {
        explain_analyze: Some("ClickHouse has no EXPLAIN ANALYZE; run the query and read system.query_log"),
        modify_table: Some("ClickHouse ALTERs are mutations with engine-specific rules; not supported yet"),
        create_table: Some(
            "a ClickHouse table needs an ENGINE and ORDER BY; write CREATE TABLE in a console",
        ),
        import: Some("ClickHouse has no transactions or savepoints; use INSERT ... FORMAT CSV in a console"),
        edit_rows: Some(
            "ClickHouse tables have no row identity (a primary key is not unique), so rows cannot be edited here",
        ),
        transactions: Some("ClickHouse over HTTP has no transactions"),
        roles: Some(
            "ClickHouse access control is SQL-driven (CREATE USER ... / system.users) and its 18.x servers predate it; not supported yet",
        ),
        role_edit: Some("ClickHouse access control is not supported yet"),
        dump: Some(
            "a ClickHouse table needs an ENGINE and ORDER BY, which the generated CREATE TABLE does not carry; use clickhouse-client",
        ),
        table_search: Some(
            "the search queries (toString, positionCaseInsensitive) are not written for ClickHouse and the 18.x server available for tests rejects them",
        ),
        ..NO_FEATURES
    },
    ssl_labels: [Some("HTTP"), None, None, Some("HTTPS")],
    tls_port: Some(8443),
};

impl DatabaseKind {
    pub const ALL: &'static [DatabaseKind] = &[
        DatabaseKind::Postgres,
        DatabaseKind::MySql,
        DatabaseKind::Sqlite,
        DatabaseKind::CockroachDb,
        DatabaseKind::Redshift,
        DatabaseKind::TimescaleDb,
        DatabaseKind::YugabyteDb,
        DatabaseKind::TiDb,
        DatabaseKind::DuckDb,
        DatabaseKind::SqlServer,
        DatabaseKind::ClickHouse,
    ];

    /// The descriptor of this kind (the one place that lists per-kind facts).
    pub fn info(self) -> &'static KindInfo {
        match self {
            DatabaseKind::Postgres => &POSTGRES,
            DatabaseKind::MySql => &MYSQL,
            DatabaseKind::Sqlite => &SQLITE,
            DatabaseKind::CockroachDb => &COCKROACH,
            DatabaseKind::Redshift => &REDSHIFT,
            DatabaseKind::TimescaleDb => &TIMESCALE,
            DatabaseKind::YugabyteDb => &YUGABYTE,
            DatabaseKind::TiDb => &TIDB,
            DatabaseKind::DuckDb => &DUCKDB,
            DatabaseKind::SqlServer => &SQLSERVER,
            DatabaseKind::ClickHouse => &CLICKHOUSE,
        }
    }

    /// The SQL dialect family (see [`Family`]).
    pub fn family(self) -> Family {
        self.info().family
    }

    /// Why `feature` is unavailable for this kind, or `None` when it works.
    pub fn unsupported(self, feature: Feature) -> Option<&'static str> {
        let f = &self.info().features;
        match feature {
            Feature::Explain => f.explain,
            Feature::ExplainAnalyze => f.explain_analyze,
            Feature::Cancel => f.cancel,
            Feature::ModifyTable => f.modify_table,
            Feature::CreateTable => f.create_table,
            Feature::Import => f.import,
            Feature::EditRows => f.edit_rows,
            Feature::Transactions => f.transactions,
            Feature::Sessions => f.sessions,
            Feature::Roles => f.roles,
            Feature::RoleEdit => f.role_edit,
            Feature::Dump => f.dump,
            Feature::TableSearch => f.table_search,
        }
    }

    /// Work out which product answered, from `version()` / the server banner. `None` when nothing
    /// recognisable is in the text. TimescaleDB is an extension of a stock PostgreSQL, so it is
    /// never detected here (the Postgres driver checks `pg_extension` and appends `TimescaleDB` to
    /// the banner it reports).
    pub fn detect(banner: &str) -> Option<DatabaseKind> {
        let b = banner.to_ascii_lowercase();
        let has = |w: &str| b.contains(w);
        Some(if has("cockroachdb") {
            DatabaseKind::CockroachDb
        } else if has("redshift") {
            DatabaseKind::Redshift
        } else if has("-yb-") || has("yugabyte") {
            DatabaseKind::YugabyteDb
        } else if has("timescaledb") {
            DatabaseKind::TimescaleDb
        } else if has("tidb") {
            DatabaseKind::TiDb
        } else if has("mariadb") || has("mysql") {
            DatabaseKind::MySql
        } else if has("clickhouse") {
            DatabaseKind::ClickHouse
        } else if has("microsoft sql server") {
            DatabaseKind::SqlServer
        } else if has("duckdb") {
            DatabaseKind::DuckDb
        } else if has("postgresql") {
            DatabaseKind::Postgres
        } else {
            return None;
        })
    }

    /// A sentence for the user when the server is not the product the connection says, `None` when
    /// they agree (or the banner says nothing). A plain `Postgres` connection to a TimescaleDB server
    /// is fine and silent (it is PostgreSQL); the other way round is not.
    pub fn mismatch(self, banner: &str) -> Option<String> {
        let found = Self::detect(banner)?;
        if found == self || (self == DatabaseKind::Postgres && found == DatabaseKind::TimescaleDb) {
            return None;
        }
        Some(format!(
            "This connection is set up as {}, but the server identifies as {}. Edit the connection and pick {} so \
             Theta uses the right catalogs and SQL.",
            self.label(),
            found.label(),
            found.label()
        ))
    }

    /// The kind a URL scheme (`postgres`, `mariadb`, ...) selects.
    pub fn from_url_scheme(scheme: &str) -> Option<DatabaseKind> {
        let s = scheme.to_ascii_lowercase();
        Self::ALL.iter().copied().find(|k| k.info().url_schemes.contains(&s.as_str()))
    }

    /// The TLS choice a new connection of this kind starts with: plain HTTP for ClickHouse (8123), an
    /// encrypted session that trusts the server's own certificate for SQL Server (its images ship a
    /// self-signed one), "prefer" elsewhere.
    pub fn default_ssl_mode(self) -> SslMode {
        match self {
            DatabaseKind::ClickHouse => SslMode::Disable,
            DatabaseKind::SqlServer => SslMode::Require,
            _ => SslMode::Prefer,
        }
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
        match self.family() {
            Family::MySql | Family::ClickHouse => format!("`{}`", ident.replace('`', "``")),
            Family::SqlServer => format!("[{}]", ident.replace(']', "]]")),
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
        assert_eq!(DatabaseKind::from_url_scheme("cockroachdb"), Some(DatabaseKind::CockroachDb));
        assert_eq!(DatabaseKind::from_url_scheme("redshift"), Some(DatabaseKind::Redshift));
    }

    #[test]
    fn banners_identify_the_product() {
        use DatabaseKind::*;
        // Banners as the products document them (version() output shapes).
        let cases = [
            ("CockroachDB CCL v24.1.0 (x86_64-pc-linux-gnu, built 2024/05/15, go1.22.2)", CockroachDb),
            (
                "PostgreSQL 8.0.2 on i686-pc-linux-gnu, compiled by GCC gcc (GCC) 3.4.2, Redshift 1.0.77467",
                Redshift,
            ),
            (
                "PostgreSQL 11.2-YB-2.18.1.0-b0 on x86_64-pc-linux-gnu, compiled by clang version 15.0.3",
                YugabyteDb,
            ),
            ("PostgreSQL 16.4 (Debian 16.4-1) on x86_64-pc-linux-gnu", Postgres),
            ("8.0.11-TiDB-v7.5.0", TiDb),
            ("10.11.6-MariaDB-0ubuntu0.23.10.2", MySql),
            ("8.0.36", MySql),
            ("Microsoft SQL Server 2022 (RTM-CU12) (KB5033663) - 16.0.4115.5 (X64)", SqlServer),
            ("ClickHouse 24.3.1", ClickHouse),
        ];
        for (banner, want) in cases {
            if banner == "8.0.36" {
                assert_eq!(DatabaseKind::detect(banner), None);
                continue;
            }
            assert_eq!(DatabaseKind::detect(banner), Some(want), "{banner}");
        }
    }

    #[test]
    fn mismatch_message_names_both_products() {
        let m = DatabaseKind::Postgres.mismatch("CockroachDB CCL v24.1.0").unwrap();
        assert!(m.contains("PostgreSQL") && m.contains("CockroachDB"));
        assert!(DatabaseKind::CockroachDb.mismatch("CockroachDB CCL v24.1.0").is_none());
        // A plain PostgreSQL connection to TimescaleDB is still PostgreSQL.
        assert!(DatabaseKind::Postgres.mismatch("PostgreSQL 16.4, TimescaleDB 2.15").is_none());
        assert!(DatabaseKind::TimescaleDb.mismatch("PostgreSQL 16.4").is_some());
        assert!(DatabaseKind::Postgres.mismatch("something else").is_none());
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
    /// Auto-commit or manual transactions, and the isolation level (see [`crate::txn`]).
    #[serde(default, skip_serializing_if = "TransactionConfig::is_default")]
    pub transaction: TransactionConfig,
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
            ssl_mode: kind.default_ssl_mode(),
            ssh: None,
            read_only: false,
            color: None,
            transaction: TransactionConfig::default(),
        }
    }
}
