//! Copy a table, structure and optionally data, to another schema or another connection.
//!
//! The read side is the export's keyset pager (`export::page_query`), so a copy of a large table costs
//! the same per page however deep it is and is not thrown off by rows that change meanwhile. The write side
//! is `theta_sql::copy::insert_statements`: one multi-row `INSERT` per page, every value a literal written
//! for the target product.
//!
//! **Leaving the target clean.** Where the target can roll back DDL (PostgreSQL, SQLite, DuckDB) the whole
//! copy, `CREATE TABLE` included, is one transaction: a failure or a cancel rolls it back and nothing
//! exists afterwards. MySQL and MariaDB commit a `CREATE TABLE` at once, so there the half-made table is
//! dropped instead; if even that fails the report says so, with the name, so it is never left unmentioned.
//! The report states which of the two happened (`Cleanup`).

use crate::export::{ExportJob, page_query};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use theta_core::{Connection, DatabaseKind, ExecOptions, Family, Value};
use theta_sql::ExportFormat;
use theta_sql::copy::{CopyColumn, insert_statements};

#[derive(Debug, Clone)]
pub struct CopyJob {
    pub src_kind: DatabaseKind,
    pub src_schema: Option<String>,
    pub src_table: String,
    /// Primary key of the source in key order; empty means offset paging.
    pub key: Vec<String>,
    pub columns: Vec<CopyColumn>,
    pub dst_kind: DatabaseKind,
    pub dst_schema: String,
    pub dst_table: String,
    pub create_sql: String,
    pub with_data: bool,
    pub page_rows: usize,
    pub rows_estimate: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct CopyProgress {
    pub rows: u64,
    pub rows_estimate: Option<u64>,
    pub elapsed: Duration,
}

impl CopyProgress {
    pub fn fraction(&self) -> Option<f32> {
        self.rows_estimate.filter(|e| *e > 0).map(|e| (self.rows as f32 / e as f32).min(0.99))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyOutcome {
    Completed,
    Cancelled,
    Failed(String),
}

/// What was done about the target table when the copy did not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cleanup {
    /// Nothing had been made, or the copy completed.
    NotNeeded,
    /// The transaction was rolled back: the target table does not exist.
    RolledBack,
    /// The half-made table was dropped.
    Dropped,
    /// It could not be dropped; the table is still there and the string says why.
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct CopyReport {
    pub outcome: CopyOutcome,
    pub rows: u64,
    pub elapsed: Duration,
    pub cleanup: Cleanup,
}

/// Whether the target can undo a `CREATE TABLE` with `ROLLBACK`.
pub fn transactional_ddl(kind: DatabaseKind) -> bool {
    !matches!(kind.family(), Family::MySql | Family::ClickHouse)
}

fn begin_sql(kind: DatabaseKind) -> &'static str {
    if kind.family() == Family::DuckDb { "BEGIN TRANSACTION" } else { "BEGIN" }
}

pub async fn copy_table(
    src: &dyn Connection,
    dst: &dyn Connection,
    job: &CopyJob,
    cancel: &AtomicBool,
    progress: &(dyn Fn(&CopyProgress) + Send + Sync),
) -> CopyReport {
    let started = Instant::now();
    let opts = ExecOptions { row_limit: None };
    let transactional = transactional_ddl(job.dst_kind);
    let mut rows = 0u64;
    let report = |outcome, rows, cleanup| CopyReport { outcome, rows, elapsed: started.elapsed(), cleanup };

    if transactional {
        if dst.in_transaction().await {
            return report(
                CopyOutcome::Failed(
                    "the target session is already inside a transaction; commit or roll it back first".into(),
                ),
                0,
                Cleanup::NotNeeded,
            );
        }
        if let Err(e) = dst.execute(begin_sql(job.dst_kind), &opts).await {
            return report(
                CopyOutcome::Failed(format!("could not start a transaction: {e}")),
                0,
                Cleanup::NotNeeded,
            );
        }
    }
    let mut created = false;
    let outcome = 'run: {
        if let Err(e) = dst.execute(&job.create_sql, &opts).await {
            break 'run CopyOutcome::Failed(format!("creating the table failed: {e}"));
        }
        created = true;
        if job.with_data {
            let read = ExportJob {
                kind: job.src_kind,
                schema: job.src_schema.clone(),
                table: job.src_table.clone(),
                key: job.key.clone(),
                columns: Some(job.columns.iter().map(|c| c.name.clone()).collect()),
                format: ExportFormat::Csv,
                page_rows: job.page_rows.max(1),
            };
            let mut after: Option<Vec<Value>> = None;
            let mut key_idx: Vec<usize> = Vec::new();
            loop {
                if cancel.load(Ordering::Relaxed) {
                    break 'run CopyOutcome::Cancelled;
                }
                let sql = page_query(&read, after.as_deref(), rows);
                let page = match src.execute(&sql, &opts).await {
                    Ok(o) => o,
                    Err(e) => break 'run CopyOutcome::Failed(format!("reading the source failed: {e}")),
                };
                let Some(rs) = page.result else {
                    break 'run CopyOutcome::Failed("the source returned no result set".into());
                };
                if key_idx.is_empty() && !job.key.is_empty() {
                    key_idx = job
                        .key
                        .iter()
                        .filter_map(|k| rs.columns.iter().position(|c| c.name.eq_ignore_ascii_case(k)))
                        .collect();
                    if key_idx.len() != job.key.len() {
                        break 'run CopyOutcome::Failed(
                            "the primary key columns are missing from the result".into(),
                        );
                    }
                }
                let n = rs.rows.len();
                if n > 0 {
                    if rs.rows.iter().any(|r| r.len() != job.columns.len()) {
                        break 'run CopyOutcome::Failed("the source returned rows of the wrong width".into());
                    }
                    for stmt in insert_statements(
                        job.dst_kind,
                        &job.dst_schema,
                        &job.dst_table,
                        &job.columns,
                        &rs.rows,
                        job.page_rows,
                    ) {
                        if let Err(e) = dst.execute(&stmt, &opts).await {
                            break 'run CopyOutcome::Failed(format!(
                                "writing rows {}–{} failed: {e}",
                                rows + 1,
                                rows + n as u64
                            ));
                        }
                    }
                    if let Some(last) = rs.rows.last() {
                        after = Some(key_idx.iter().map(|&i| last[i].clone()).collect());
                    }
                }
                rows += n as u64;
                progress(&CopyProgress {
                    rows,
                    rows_estimate: job.rows_estimate,
                    elapsed: started.elapsed(),
                });
                if n < job.page_rows.max(1) {
                    break;
                }
            }
        }
        if transactional && let Err(e) = dst.execute("COMMIT", &opts).await {
            break 'run CopyOutcome::Failed(format!("committing failed: {e}"));
        }
        CopyOutcome::Completed
    };

    let cleanup = if outcome == CopyOutcome::Completed {
        Cleanup::NotNeeded
    } else if transactional {
        match dst.execute("ROLLBACK", &opts).await {
            Ok(_) => Cleanup::RolledBack,
            Err(e) => Cleanup::Failed(format!("the rollback failed: {e}")),
        }
    } else if created {
        let target = if job.dst_schema.is_empty() {
            job.dst_kind.quote_ident(&job.dst_table)
        } else {
            format!(
                "{}.{}",
                job.dst_kind.quote_ident(&job.dst_schema),
                job.dst_kind.quote_ident(&job.dst_table)
            )
        };
        match dst.execute(&format!("DROP TABLE {target}"), &opts).await {
            Ok(_) => Cleanup::Dropped,
            Err(e) => Cleanup::Failed(format!("the half-made table {target} could not be dropped: {e}")),
        }
    } else {
        Cleanup::NotNeeded
    };
    progress(&CopyProgress { rows, rows_estimate: job.rows_estimate, elapsed: started.elapsed() });
    report(outcome, rows, cleanup)
}
