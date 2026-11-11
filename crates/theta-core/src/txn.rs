//! Transaction mode and isolation level: what a connection is configured to do and the statements that
//! start a transaction accordingly. Pure; the application issues the statements through
//! [`Connection::execute`](crate::Connection::execute), so no driver needs to know about modes.
//!
//! In **auto-commit** mode (the default, and what a console has always done) every statement commits by
//! itself unless the user typed `BEGIN`. In **manual** mode the application starts a transaction lazily,
//! before the first statement that needs one, with the configured isolation level, and leaves it open
//! until the user commits or rolls back.

use crate::DatabaseKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    /// Whatever the server's default is (no clause is sent).
    #[default]
    ServerDefault,
    ReadUncommitted,
    ReadCommitted,
    RepeatableRead,
    Serializable,
}

impl Isolation {
    pub fn label(self) -> &'static str {
        match self {
            Isolation::ServerDefault => "Server default",
            Isolation::ReadUncommitted => "Read Uncommitted",
            Isolation::ReadCommitted => "Read Committed",
            Isolation::RepeatableRead => "Repeatable Read",
            Isolation::Serializable => "Serializable",
        }
    }

    fn sql(self) -> Option<&'static str> {
        match self {
            Isolation::ServerDefault => None,
            Isolation::ReadUncommitted => Some("READ UNCOMMITTED"),
            Isolation::ReadCommitted => Some("READ COMMITTED"),
            Isolation::RepeatableRead => Some("REPEATABLE READ"),
            Isolation::Serializable => Some("SERIALIZABLE"),
        }
    }
}

/// SQLite's `BEGIN` modes: when the write lock is taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SqliteBegin {
    #[default]
    Deferred,
    Immediate,
    Exclusive,
}

impl SqliteBegin {
    pub const ALL: [SqliteBegin; 3] = [SqliteBegin::Deferred, SqliteBegin::Immediate, SqliteBegin::Exclusive];

    pub fn label(self) -> &'static str {
        match self {
            SqliteBegin::Deferred => "Deferred",
            SqliteBegin::Immediate => "Immediate",
            SqliteBegin::Exclusive => "Exclusive",
        }
    }
}

/// The per-connection setting saved in `connections.json` (the session can change it afterwards).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TransactionConfig {
    /// `true`: the application opens and keeps transactions; `false`: auto-commit.
    pub manual: bool,
    pub isolation: Isolation,
    pub sqlite_begin: SqliteBegin,
}

impl TransactionConfig {
    pub fn is_default(&self) -> bool {
        *self == TransactionConfig::default()
    }

    /// The statements that start a transaction in manual mode, in the order to execute them. `read_only`
    /// asks for a read-only transaction where the database has one (Postgres and MySQL; SQLite does not).
    pub fn begin_statements(&self, kind: DatabaseKind, read_only: bool) -> Vec<String> {
        match kind {
            DatabaseKind::Postgres => {
                let mut s = String::from("BEGIN");
                if let Some(level) = self.isolation.sql() {
                    s.push_str(&format!(" ISOLATION LEVEL {level}"));
                }
                if read_only {
                    s.push_str(" READ ONLY");
                }
                vec![s]
            }
            DatabaseKind::MySql => {
                let mut out = Vec::new();
                // `SET TRANSACTION` applies to the next transaction only, which is the one started below.
                if let Some(level) = self.isolation.sql() {
                    out.push(format!("SET TRANSACTION ISOLATION LEVEL {level}"));
                }
                out.push(if read_only {
                    "START TRANSACTION READ ONLY".into()
                } else {
                    "START TRANSACTION".into()
                });
                out
            }
            DatabaseKind::Sqlite => {
                vec![format!("BEGIN {}", self.sqlite_begin.label().to_ascii_uppercase())]
            }
        }
    }

    /// One line for the title-bar chip: `Manual · Repeatable Read`, `Auto-commit`, `Manual · Immediate`.
    pub fn summary(&self, kind: DatabaseKind) -> String {
        if !self.manual {
            return "Auto-commit".into();
        }
        match kind {
            DatabaseKind::Sqlite => format!("Manual · {}", self.sqlite_begin.label()),
            _ if self.isolation == Isolation::ServerDefault => "Manual".into(),
            _ => format!("Manual · {}", self.isolation.label()),
        }
    }
}

/// The isolation levels a database lets you choose (the first is always the server default).
pub fn isolation_levels(kind: DatabaseKind) -> &'static [Isolation] {
    use Isolation::*;
    match kind {
        // Postgres accepts READ UNCOMMITTED but runs it as READ COMMITTED, so offering it would mislead.
        DatabaseKind::Postgres => &[ServerDefault, ReadCommitted, RepeatableRead, Serializable],
        DatabaseKind::MySql => &[ServerDefault, ReadUncommitted, ReadCommitted, RepeatableRead, Serializable],
        DatabaseKind::Sqlite => &[],
    }
}

/// Which statements start a transaction in manual mode. Session settings, `SHOW`-style statements and
/// transaction control do not: the first because they are not transactional, the second because the user
/// is already controlling it.
pub fn starts_transaction(class: StatementKind) -> bool {
    matches!(class, StatementKind::Query | StatementKind::Write | StatementKind::Ddl)
}

/// The classes of `theta_sql::StatementClass` this module cares about (`theta-core` cannot depend on
/// `theta-sql`; the application maps one to the other).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementKind {
    Query,
    Write,
    Ddl,
    Transaction,
    Other,
}

/// What to tell the user when a transaction that was open is gone after a statement that did not end it.
/// MySQL (and MariaDB) commit implicitly before and after most DDL; the other databases do not.
pub fn implicit_commit_note(
    kind: DatabaseKind,
    was_open: bool,
    now_open: bool,
    class: StatementKind,
) -> Option<&'static str> {
    (was_open && !now_open && class == StatementKind::Ddl && kind == DatabaseKind::MySql).then_some(
        "MySQL ended the open transaction by itself: a DDL statement commits implicitly, so the changes made before it are committed and cannot be rolled back.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use DatabaseKind::*;

    fn cfg(isolation: Isolation) -> TransactionConfig {
        TransactionConfig { manual: true, isolation, sqlite_begin: SqliteBegin::Deferred }
    }

    #[test]
    fn postgres_begin_carries_level_and_read_only() {
        assert_eq!(cfg(Isolation::ServerDefault).begin_statements(Postgres, false), ["BEGIN"]);
        assert_eq!(
            cfg(Isolation::RepeatableRead).begin_statements(Postgres, false),
            ["BEGIN ISOLATION LEVEL REPEATABLE READ"]
        );
        assert_eq!(
            cfg(Isolation::Serializable).begin_statements(Postgres, true),
            ["BEGIN ISOLATION LEVEL SERIALIZABLE READ ONLY"]
        );
        assert_eq!(cfg(Isolation::ServerDefault).begin_statements(Postgres, true), ["BEGIN READ ONLY"]);
    }

    #[test]
    fn mysql_sets_the_level_for_the_next_transaction_then_starts_it() {
        assert_eq!(cfg(Isolation::ServerDefault).begin_statements(MySql, false), ["START TRANSACTION"]);
        assert_eq!(
            cfg(Isolation::ReadCommitted).begin_statements(MySql, false),
            ["SET TRANSACTION ISOLATION LEVEL READ COMMITTED", "START TRANSACTION"]
        );
        assert_eq!(
            cfg(Isolation::ReadUncommitted).begin_statements(MySql, true),
            ["SET TRANSACTION ISOLATION LEVEL READ UNCOMMITTED", "START TRANSACTION READ ONLY"]
        );
    }

    #[test]
    fn sqlite_uses_its_begin_modes_and_ignores_isolation() {
        let mut c = cfg(Isolation::Serializable);
        assert_eq!(c.begin_statements(Sqlite, false), ["BEGIN DEFERRED"]);
        c.sqlite_begin = SqliteBegin::Immediate;
        assert_eq!(c.begin_statements(Sqlite, true), ["BEGIN IMMEDIATE"]);
        c.sqlite_begin = SqliteBegin::Exclusive;
        assert_eq!(c.begin_statements(Sqlite, false), ["BEGIN EXCLUSIVE"]);
    }

    #[test]
    fn levels_per_kind() {
        assert!(!isolation_levels(Postgres).contains(&Isolation::ReadUncommitted));
        assert!(isolation_levels(MySql).contains(&Isolation::ReadUncommitted));
        assert!(isolation_levels(Sqlite).is_empty());
        assert_eq!(isolation_levels(Postgres)[0], Isolation::ServerDefault);
    }

    #[test]
    fn summaries() {
        assert_eq!(TransactionConfig::default().summary(Postgres), "Auto-commit");
        assert_eq!(cfg(Isolation::ServerDefault).summary(Postgres), "Manual");
        assert_eq!(cfg(Isolation::RepeatableRead).summary(MySql), "Manual · Repeatable Read");
        let mut c = cfg(Isolation::RepeatableRead);
        c.sqlite_begin = SqliteBegin::Immediate;
        assert_eq!(c.summary(Sqlite), "Manual · Immediate");
    }

    #[test]
    fn which_statements_begin_a_transaction() {
        assert!(starts_transaction(StatementKind::Query));
        assert!(starts_transaction(StatementKind::Write));
        assert!(starts_transaction(StatementKind::Ddl));
        assert!(!starts_transaction(StatementKind::Transaction));
        assert!(!starts_transaction(StatementKind::Other));
    }

    #[test]
    fn only_mysql_ddl_ends_a_transaction_silently() {
        let n = |k, was, now, c| implicit_commit_note(k, was, now, c).is_some();
        assert!(n(MySql, true, false, StatementKind::Ddl));
        assert!(!n(MySql, true, true, StatementKind::Ddl));
        assert!(!n(MySql, false, false, StatementKind::Ddl));
        assert!(!n(Postgres, true, false, StatementKind::Ddl));
        assert!(!n(MySql, true, false, StatementKind::Write));
    }

    #[test]
    fn config_round_trips_and_old_files_read_as_auto_commit() {
        let c: TransactionConfig = serde_json::from_str("{}").unwrap();
        assert!(c.is_default());
        let c = cfg(Isolation::RepeatableRead);
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<TransactionConfig>(&json).unwrap(), c);
        assert!(json.contains("repeatable_read"));
    }
}
