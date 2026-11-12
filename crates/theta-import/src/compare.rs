//! Data compare, the runner: read two tables page by page in key order and merge them with
//! `theta_sql::data_diff::Comparer`.
//!
//! Memory is bounded by two pages plus the differing rows kept for display (`CompareJob::cap`); the counts are
//! exact however many rows differ. Paging is keyset, like the export's (`WHERE (k1, k2) > (…) ORDER BY k1, k2`),
//! so every page costs the same and a row that appears or goes during the comparison cannot shift the later
//! pages.
//!
//! # Two products, two orders
//!
//! The first pass asks each server for `ORDER BY key` in its own collation, which is fast and uses the index.
//! The comparer checks every key against the one before it *on the client*, in one order for everything. If a
//! stream is not in that order (PostgreSQL's `en_US` or MariaDB's case-insensitive collation puts `a` before
//! `B`; bytes put `B` first) the pass stops, and the second pass asks again with a binary collation
//! (`COLLATE "C"`, `CAST(… AS BINARY)`, `COLLATE BINARY`), whose order is the client's. A stream that is still
//! out of order after that, a duplicate key, or a NULL key is a failure with the sentence the comparer wrote.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use theta_core::{Connection, DatabaseKind, ExecOptions, Family, Value};
use theta_sql::data_diff::{Comparer, Counts, DiffRow, KeyClass, MergeError, Need, Rules};
use theta_sql::generate::sql_literal;
use theta_sql::keywords::{qualified_name, quote_if_needed};

pub use theta_sql::data_diff::Mapping;

#[derive(Debug, Clone)]
pub struct CompareJob {
    pub mapping: Mapping,
    pub left_kind: DatabaseKind,
    pub left_schema: Option<String>,
    pub left_table: String,
    pub right_kind: DatabaseKind,
    pub right_schema: Option<String>,
    pub right_table: String,
    pub page_rows: usize,
    /// Differing rows kept for display; the counts stay exact.
    pub cap: usize,
    /// Ask again with a binary collation when a stream is not in the client's order. Off, that is a failure
    /// (used by the tests that prove the check).
    pub allow_fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompareOutcome {
    Completed,
    Cancelled,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOrder {
    /// Each server's own `ORDER BY`.
    Native,
    /// A binary collation, after the native order turned out not to be the client's.
    Binary,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CompareProgress {
    pub left_rows: u64,
    pub right_rows: u64,
    pub differences: u64,
}

#[derive(Debug, Clone)]
pub struct CompareReport {
    pub outcome: CompareOutcome,
    pub counts: Counts,
    pub rows: Vec<DiffRow>,
    pub capped: bool,
    pub order: KeyOrder,
    pub left_rows: u64,
    pub right_rows: u64,
    pub pages: u64,
    pub elapsed: Duration,
}

/// The expression a key column is ordered and compared by.
fn key_expr(kind: DatabaseKind, column: &str, class: KeyClass, binary: bool) -> Result<String, String> {
    let c = quote_if_needed(column, kind);
    if !(binary && class == KeyClass::Text) {
        return Ok(c);
    }
    match kind.family() {
        Family::Postgres => Ok(format!("{c} COLLATE \"C\"")),
        Family::MySql => Ok(format!("CAST({c} AS BINARY)")),
        Family::Sqlite => Ok(format!("{c} COLLATE BINARY")),
        _ => Err(format!("{} has no binary collation this comparison can use for a text key", kind.label())),
    }
}

/// One page of `table`: the `select` columns of up to `n` rows after the key `after`, in key order.
#[allow(clippy::too_many_arguments)]
pub fn page_sql(
    kind: DatabaseKind,
    schema: Option<&str>,
    table: &str,
    select: &[String],
    key: &[String],
    classes: &[KeyClass],
    after: Option<&[Value]>,
    n: usize,
    binary: bool,
) -> Result<String, String> {
    let cols = select.iter().map(|c| quote_if_needed(c, kind)).collect::<Vec<_>>().join(", ");
    let keys: Vec<String> =
        key.iter().zip(classes).map(|(k, c)| key_expr(kind, k, *c, binary)).collect::<Result<_, _>>()?;
    let filter = match after {
        None => String::new(),
        Some(vals) => {
            let lits: Vec<String> = vals.iter().map(|v| sql_literal(v, kind)).collect();
            if keys.len() == 1 {
                format!(" WHERE {} > {}", keys[0], lits[0])
            } else {
                format!(" WHERE ({}) > ({})", keys.join(", "), lits.join(", "))
            }
        }
    };
    Ok(format!(
        "SELECT {cols} FROM {}{filter} ORDER BY {} LIMIT {}",
        qualified_name(schema, table, kind),
        keys.join(", "),
        n.max(1)
    ))
}

struct Stream<'a> {
    conn: &'a dyn Connection,
    kind: DatabaseKind,
    schema: Option<String>,
    table: String,
    select: Vec<String>,
    key: Vec<String>,
    key_at: Vec<usize>,
    after: Option<Vec<Value>>,
    done: bool,
    rows: u64,
}

impl Stream<'_> {
    async fn next_page(
        &mut self,
        classes: &[KeyClass],
        n: usize,
        binary: bool,
    ) -> Result<Vec<Vec<Value>>, String> {
        let sql = page_sql(
            self.kind,
            self.schema.as_deref(),
            &self.table,
            &self.select,
            &self.key,
            classes,
            self.after.as_deref(),
            n,
            binary,
        )?;
        let rows = self
            .conn
            .execute(&sql, &ExecOptions { row_limit: None })
            .await
            .map_err(|e| format!("Reading {} failed: {e}", self.table))?
            .result
            .map(|r| r.rows)
            .unwrap_or_default();
        if rows.len() < n.max(1) {
            self.done = true;
        }
        if let Some(last) = rows.last() {
            self.after = Some(self.key_at.iter().map(|&i| last[i].clone()).collect());
        }
        self.rows += rows.len() as u64;
        Ok(rows)
    }
}

pub async fn compare_tables(
    left: &dyn Connection,
    right: &dyn Connection,
    job: &CompareJob,
    cancel: &AtomicBool,
    progress: &(dyn Fn(&CompareProgress) + Send + Sync),
) -> CompareReport {
    let started = Instant::now();
    let m = &job.mapping;
    let mut report = CompareReport {
        outcome: CompareOutcome::Completed,
        counts: Counts::default(),
        rows: Vec::new(),
        capped: false,
        order: KeyOrder::Native,
        left_rows: 0,
        right_rows: 0,
        pages: 0,
        elapsed: Duration::ZERO,
    };
    // Rows are compared leniently only between different products.
    let rules = Rules { cross_product: job.left_kind != job.right_kind };
    let passes: &[bool] = if job.allow_fallback { &[false, true] } else { &[false] };
    for &binary in passes {
        let mut comparer = Comparer::new(m.clone(), rules, job.cap);
        let mut l = Stream {
            conn: left,
            kind: job.left_kind,
            schema: job.left_schema.clone(),
            table: job.left_table.clone(),
            select: m.left_names(),
            key: m.key_left(),
            key_at: m.key.clone(),
            after: None,
            done: false,
            rows: 0,
        };
        let mut r = Stream {
            conn: right,
            kind: job.right_kind,
            schema: job.right_schema.clone(),
            table: job.right_table.clone(),
            select: m.right_names(),
            key: m.key_right(),
            key_at: m.key.clone(),
            after: None,
            done: false,
            rows: 0,
        };
        report.order = if binary { KeyOrder::Binary } else { KeyOrder::Native };
        let (mut lbuf, mut rbuf): (VecDeque<Vec<Value>>, VecDeque<Vec<Value>>) =
            (VecDeque::new(), VecDeque::new());
        report.pages = 0;
        let outcome = loop {
            if cancel.load(Ordering::SeqCst) {
                break Some(CompareOutcome::Cancelled);
            }
            match comparer.feed(&mut lbuf, l.done, &mut rbuf, r.done) {
                Ok(Need::Done) => break Some(CompareOutcome::Completed),
                Ok(need) => {
                    let (stream, buf) =
                        if need == Need::Left { (&mut l, &mut lbuf) } else { (&mut r, &mut rbuf) };
                    match stream.next_page(&m.classes, job.page_rows, binary).await {
                        Ok(rows) => buf.extend(rows),
                        Err(e) => break Some(CompareOutcome::Failed(e)),
                    }
                    report.pages += 1;
                    progress(&CompareProgress {
                        left_rows: l.rows,
                        right_rows: r.rows,
                        differences: comparer.counts.differences(),
                    });
                }
                Err(e @ MergeError::Unsorted { .. }) if !binary && job.allow_fallback => {
                    // The native order is not the client's: start again in binary order.
                    let _ = e;
                    break None;
                }
                Err(e) => {
                    let mut msg = e.describe(&job.left_table, &job.right_table);
                    if matches!(e, MergeError::Unsorted { .. }) {
                        msg.push_str(if binary {
                            " Even a binary collation did not put them in order, so this key cannot be compared."
                        } else {
                            " (Falling back to a binary collation was switched off.)"
                        });
                    }
                    break Some(CompareOutcome::Failed(msg));
                }
            }
        };
        if let Some(outcome) = outcome {
            report.outcome = outcome;
            report.counts = comparer.counts;
            report.capped = comparer.capped;
            report.rows = std::mem::take(&mut comparer.rows);
            report.left_rows = l.rows;
            report.right_rows = r.rows;
            break;
        }
    }
    report.elapsed = started.elapsed();
    report
}
