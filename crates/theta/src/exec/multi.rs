//! Multi-source run: what happened on each connection, and how that becomes the tabs the user sees.
//!
//! Pure on purpose: the workspace runs the statements, this module turns the per-connection outcomes into
//! the summary table (one row per connection: statements, rows, time, status) and the tab titles, so what the
//! user reads is tested without a window or a database.

use std::sync::Arc;
use std::time::Duration;
use theta_core::{Column, ResultSet, TypeKind, Value};

/// Why a connection was not asked at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skipped {
    NotConnected,
    Busy,
    /// A statement the connection's read-only setting refuses; the string is the reason shown to the user.
    ReadOnly(String),
}

#[derive(Debug, Clone)]
pub struct ConnOutcome {
    pub name: String,
    pub product: String,
    pub skipped: Option<Skipped>,
    /// Statements that finished (successfully) before the end or the first error.
    pub ran: usize,
    pub total: usize,
    /// Rows of the last statement that returned rows, or the rows affected by the statements when none did.
    pub rows: Option<u64>,
    pub elapsed: Duration,
    pub error: Option<String>,
    /// The last result set of the script, shown in this connection's tab.
    pub result: Option<Arc<ResultSet>>,
    pub truncated: bool,
}

impl ConnOutcome {
    pub fn skipped(name: &str, product: &str, total: usize, why: Skipped) -> Self {
        Self {
            name: name.into(),
            product: product.into(),
            skipped: Some(why),
            ran: 0,
            total,
            rows: None,
            elapsed: Duration::ZERO,
            error: None,
            result: None,
            truncated: false,
        }
    }

    pub fn failed(&self) -> bool {
        self.error.is_some() || self.skipped.is_some()
    }
}

/// The status column and the tab suffix: one short phrase, the same everywhere.
pub fn status(o: &ConnOutcome) -> String {
    match (&o.skipped, &o.error) {
        (Some(Skipped::NotConnected), _) => "not connected".into(),
        (Some(Skipped::Busy), _) => "busy: a query is already running".into(),
        (Some(Skipped::ReadOnly(why)), _) => format!("refused: {why}"),
        (None, Some(e)) => format!("error: {}", e.lines().next().unwrap_or("").trim()),
        (None, None) => "ok".into(),
    }
}

/// `prod · 12 rows`, `staging · error`, `dev · refused`.
pub fn tab_title(o: &ConnOutcome) -> String {
    let tail = match (&o.skipped, &o.error, o.rows) {
        (Some(Skipped::ReadOnly(_)), _, _) => "refused".to_string(),
        (Some(Skipped::NotConnected), _, _) => "not connected".to_string(),
        (Some(Skipped::Busy), _, _) => "busy".to_string(),
        (None, Some(_), _) => "error".to_string(),
        (None, None, Some(n)) => format!("{n} {}", if n == 1 { "row" } else { "rows" }),
        (None, None, None) => "ok".to_string(),
    };
    format!("{} · {tail}", o.name)
}

fn text_col(name: &str) -> Column {
    Column { name: name.into(), type_name: "text".into(), kind: TypeKind::Text, origin: None }
}

fn int_col(name: &str) -> Column {
    Column { name: name.into(), type_name: "int8".into(), kind: TypeKind::Integer, origin: None }
}

/// The summary table, one row per connection in the order they were chosen.
pub fn summary(outcomes: &[ConnOutcome]) -> ResultSet {
    let columns = vec![
        text_col("Connection"),
        text_col("Product"),
        text_col("Statements"),
        int_col("Rows"),
        int_col("Time (ms)"),
        text_col("Status"),
    ];
    let rows = outcomes
        .iter()
        .map(|o| {
            vec![
                Value::Text(o.name.clone()),
                Value::Text(o.product.clone()),
                Value::Text(format!("{} of {}", o.ran, o.total)),
                o.rows.map_or(Value::Null, |n| Value::Int(n as i64)),
                if o.skipped.is_some() { Value::Null } else { Value::Int(o.elapsed.as_millis() as i64) },
                Value::Text(status(o)),
            ]
        })
        .collect();
    ResultSet { columns, rows, truncated: false }
}

/// One line for the status bar and Messages: `3 connections: 2 ok, 1 failed`.
pub fn headline(outcomes: &[ConnOutcome]) -> String {
    let failed = outcomes.iter().filter(|o| o.failed()).count();
    let n = outcomes.len();
    let noun = if n == 1 { "connection" } else { "connections" };
    if failed == 0 {
        format!("{n} {noun}: all ok")
    } else {
        format!("{n} {noun}: {} ok, {failed} failed", n - failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(name: &str, rows: u64) -> ConnOutcome {
        ConnOutcome {
            name: name.into(),
            product: "PostgreSQL".into(),
            skipped: None,
            ran: 1,
            total: 1,
            rows: Some(rows),
            elapsed: Duration::from_millis(12),
            error: None,
            result: None,
            truncated: false,
        }
    }

    #[test]
    fn titles_say_what_each_tab_holds() {
        assert_eq!(tab_title(&ok("prod", 12)), "prod · 12 rows");
        assert_eq!(tab_title(&ok("prod", 1)), "prod · 1 row");
        let mut e = ok("staging", 0);
        e.error = Some("relation \"t\" does not exist".into());
        assert_eq!(tab_title(&e), "staging · error");
        assert_eq!(
            tab_title(&ConnOutcome::skipped("dev", "SQLite", 1, Skipped::ReadOnly("read-only".into()))),
            "dev · refused"
        );
        assert_eq!(
            tab_title(&ConnOutcome::skipped("x", "SQLite", 1, Skipped::NotConnected)),
            "x · not connected"
        );
        let mut none = ok("ddl", 0);
        none.rows = None;
        assert_eq!(tab_title(&none), "ddl · ok");
    }

    #[test]
    fn the_summary_has_a_row_per_connection_in_order_and_names_every_failure() {
        let mut bad = ok("staging", 0);
        bad.error = Some("no such table: t\nLINE 1".into());
        bad.ran = 0;
        let outcomes = [
            ok("prod", 12),
            bad,
            ConnOutcome::skipped(
                "replica",
                "MySQL",
                1,
                Skipped::ReadOnly("this connection is read-only; INSERT would change data".into()),
            ),
            ConnOutcome::skipped("down", "SQLite", 1, Skipped::NotConnected),
        ];
        let s = summary(&outcomes);
        assert_eq!(
            s.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["Connection", "Product", "Statements", "Rows", "Time (ms)", "Status"]
        );
        let cell = |r: usize, c: usize| s.rows[r][c].display();
        assert_eq!(
            (cell(0, 0), cell(0, 2), cell(0, 3), cell(0, 4), cell(0, 5)),
            ("prod".into(), "1 of 1".into(), "12".into(), "12".into(), "ok".into())
        );
        assert_eq!(cell(1, 5), "error: no such table: t", "only the first line of an error");
        assert_eq!(cell(1, 2), "0 of 1");
        assert_eq!(cell(2, 5), "refused: this connection is read-only; INSERT would change data");
        assert!(s.rows[2][4].is_null(), "a connection that was not asked has no time");
        assert_eq!(cell(3, 5), "not connected");
        assert_eq!(s.rows.len(), 4);
    }

    #[test]
    fn the_headline_counts_failures_and_skips_together() {
        assert_eq!(headline(&[ok("a", 1), ok("b", 2)]), "2 connections: all ok");
        assert_eq!(headline(&[ok("a", 1)]), "1 connection: all ok");
        let outcomes = [ok("a", 1), ConnOutcome::skipped("b", "x", 1, Skipped::Busy)];
        assert_eq!(headline(&outcomes), "2 connections: 1 ok, 1 failed");
    }
}
