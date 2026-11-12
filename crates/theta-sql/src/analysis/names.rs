//! Identifier matching rules per dialect, and the names the grammar defines.

use theta_core::Family;

/// An identifier as written in the script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident {
    pub text: String,
    pub span: std::ops::Range<usize>,
    pub quoted: bool,
}

/// Does the identifier `a` denote the stored name `stored`, by the dialect's own rule?
///
/// PostgreSQL folds unquoted names to lower case and compares quoted ones exactly; DuckDB, SQLite and
/// T-SQL (default collation) ignore case; MySQL ignores case for columns and aliases; ClickHouse is exact.
pub fn strict_eq(fam: Family, a: &Ident, stored: &str) -> bool {
    match fam {
        Family::Postgres => {
            if a.quoted {
                a.text == stored
            } else {
                a.text.to_lowercase() == stored
            }
        }
        Family::DuckDb | Family::Sqlite | Family::SqlServer | Family::MySql => {
            a.text.eq_ignore_ascii_case(stored)
        }
        Family::ClickHouse => a.text == stored,
    }
}

/// The lenient second tier: ignore case entirely. Used only when the strict rule finds nothing, so that a
/// name that differs only in case is never reported as unknown.
pub fn loose_eq(a: &str, stored: &str) -> bool {
    a.to_lowercase() == stored.to_lowercase()
}

/// Find `a` among `names`: the strict matches if there are any, else the case-insensitive ones.
pub fn find_all<'a, T>(
    fam: Family,
    a: &Ident,
    items: &'a [T],
    name_of: impl Fn(&'a T) -> &'a str,
) -> Vec<usize> {
    let strict: Vec<usize> =
        items.iter().enumerate().filter(|(_, t)| strict_eq(fam, a, name_of(t))).map(|(i, _)| i).collect();
    if !strict.is_empty() {
        return strict;
    }
    items.iter().enumerate().filter(|(_, t)| loose_eq(&a.text, name_of(t))).map(|(i, _)| i).collect()
}

/// Words that parse as a column reference but are values or functions without parentheses.
pub fn is_niladic(word: &str) -> bool {
    const NILADIC: &[&str] = &[
        "CURRENT_DATE",
        "CURRENT_TIME",
        "CURRENT_TIMESTAMP",
        "LOCALTIME",
        "LOCALTIMESTAMP",
        "CURRENT_USER",
        "SESSION_USER",
        "SYSTEM_USER",
        "USER",
        "CURRENT_SCHEMA",
        "CURRENT_CATALOG",
        "CURRENT_ROLE",
        "CURRENT_PATH",
        "SYSDATE",
        "UTC_TIMESTAMP",
        "UTC_DATE",
        "UTC_TIME",
        "DEFAULT",
        "NULL",
        "TRUE",
        "FALSE",
        "UNKNOWN",
        "NEW",
        "OLD",
        "MAXVALUE",
        "MINVALUE",
    ];
    NILADIC.iter().any(|w| w.eq_ignore_ascii_case(word))
}

/// Columns every row has without declaring them.
pub fn is_system_column(word: &str) -> bool {
    const SYSTEM: &[&str] =
        &["ctid", "xmin", "xmax", "cmin", "cmax", "tableoid", "oid", "rowid", "_rowid_", "rownum"];
    SYSTEM.iter().any(|w| w.eq_ignore_ascii_case(word))
}

/// Tables a server provides that the catalog does not list: never reported as unknown.
pub fn is_system_table(fam: Family, name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    match fam {
        Family::Postgres | Family::DuckDb => {
            l.starts_with("pg_")
                || l.starts_with("sqlite_")
                || l.starts_with("duckdb_")
                || l.starts_with("pragma_")
                || l == "information_schema"
        }
        Family::Sqlite => l.starts_with("sqlite_") || l.starts_with("pragma_"),
        Family::MySql => {
            // `dual`, and MariaDB's virtual sequence tables `seq_1_to_100`
            l == "dual" || (l.starts_with("seq_") && l.contains("_to_"))
        }
        Family::SqlServer => {
            l.starts_with('#')
                || l.starts_with('@')
                || l.starts_with("sys")
                || l == "inserted"
                || l == "deleted"
        }
        Family::ClickHouse => l == "system",
    }
}
