//! The description of one import and the report it produces. Plain data: the dialog builds an
//! [`ImportJob`], the pipeline in [`crate::run`] executes it.

use crate::convert::{ConvertOptions, Target};
use crate::parser::CsvOptions;
use std::time::Duration;
use theta_core::DatabaseKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ErrorMode {
    /// The first bad row stops the import and everything is rolled back (the default).
    #[default]
    Abort,
    /// Bad rows (unconvertible, or rejected by the server) are skipped and listed; the rest is imported.
    Skip,
}

/// How the import relates to transactions. Either way the data is written atomically: it is all there when
/// the import reports success, and none of it is when it is cancelled or aborts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxnPlan {
    /// Auto-commit connection: the import starts its own transaction and commits it.
    Own { begin: Vec<String> },
    /// Manual-commit connection: run inside the session's transaction (started with `begin` when none is
    /// open) and leave it open for the user to commit or roll back. Atomicity comes from a savepoint.
    Session { open: bool, begin: Vec<String> },
}

/// One column that receives data.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportColumn {
    /// Index of the field in each record.
    pub source: usize,
    /// Destination column name.
    pub name: String,
    pub target: Target,
}

#[derive(Debug, Clone)]
pub struct ImportJob {
    pub kind: DatabaseKind,
    pub schema: Option<String>,
    pub table: String,
    /// `CREATE TABLE` to run first (import as new table).
    pub create_sql: Option<String>,
    /// Statement that empties the table first, when requested.
    pub truncate_sql: Option<String>,
    pub columns: Vec<ImportColumn>,
    pub csv: CsvOptions,
    pub has_header: bool,
    pub convert: ConvertOptions,
    pub on_error: ErrorMode,
    pub batch_rows: usize,
    /// Upper bound for one INSERT statement's text.
    pub max_statement_bytes: usize,
    pub txn: TxnPlan,
}

pub const DEFAULT_BATCH_ROWS: usize = 1000;
pub const DEFAULT_STATEMENT_BYTES: usize = 1024 * 1024;

/// Statement size budget: never more than 1 MB, and for MySQL no more than half of `max_allowed_packet`
/// (the statement is one packet, and leaving headroom avoids "packet too large" on the unlucky batch).
pub fn statement_budget(kind: DatabaseKind, max_allowed_packet: Option<u64>) -> usize {
    match (kind, max_allowed_packet) {
        (DatabaseKind::MySql, Some(p)) => ((p / 2) as usize).clamp(4096, DEFAULT_STATEMENT_BYTES),
        _ => DEFAULT_STATEMENT_BYTES,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowIssue {
    /// 1-based physical line where the record starts.
    pub line: u64,
    /// 1-based data row (header excluded).
    pub row: u64,
    pub column: Option<String>,
    pub reason: String,
}

impl std::fmt::Display for RowIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {} (row {})", self.line, self.row)?;
        if let Some(c) = &self.column {
            write!(f, ", column {c}")?;
        }
        write!(f, ": {}", self.reason)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub rows_read: u64,
    pub rows_inserted: u64,
    pub rows_skipped: u64,
    pub bytes_read: u64,
    /// Size of the file, when known, for a percentage.
    pub bytes_total: Option<u64>,
    pub elapsed: Duration,
}

impl Progress {
    pub fn rows_per_second(&self) -> f64 {
        let s = self.elapsed.as_secs_f64();
        if s > 0.0 { self.rows_read as f64 / s } else { 0.0 }
    }

    pub fn fraction(&self) -> Option<f32> {
        self.bytes_total.filter(|t| *t > 0).map(|t| (self.bytes_read as f32 / t as f32).min(1.0))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    /// Stopped by the user; nothing was written.
    Cancelled,
    /// Stopped by an error; nothing was written (abort mode) or the work was rolled back.
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct ImportReport {
    pub outcome: Outcome,
    pub rows_read: u64,
    pub rows_inserted: u64,
    pub rows_skipped: u64,
    /// The first issues (capped), in file order.
    pub issues: Vec<RowIssue>,
    /// All issues, including those past the cap.
    pub issue_count: u64,
    pub elapsed: Duration,
    pub statements: u64,
    /// The table was created by this import (and is gone again when the outcome is not `Completed`).
    pub created_table: bool,
    /// The data is committed (false in a manual-commit session, where the transaction stays open).
    pub committed: bool,
    pub encoding_warning: bool,
    /// Detected number of fields in the header / first record.
    pub source_width: usize,
}

pub const MAX_ISSUES: usize = 1000;

impl ImportReport {
    pub(crate) fn new() -> Self {
        Self {
            outcome: Outcome::Completed,
            rows_read: 0,
            rows_inserted: 0,
            rows_skipped: 0,
            issues: Vec::new(),
            issue_count: 0,
            elapsed: Duration::ZERO,
            statements: 0,
            created_table: false,
            committed: false,
            encoding_warning: false,
            source_width: 0,
        }
    }

    pub(crate) fn push_issue(&mut self, issue: RowIssue) {
        self.issue_count += 1;
        if self.issues.len() < MAX_ISSUES {
            self.issues.push(issue);
        }
    }
}
