//! Running a planned table search: bounded concurrency, a timeout per table, cancellation, and one
//! result per table delivered as soon as it is known.
//!
//! **How concurrency is bounded.** A [`Connection`] is one server session and runs one statement at a
//! time, so the only way to ask about several tables at once is to have several sessions. The caller
//! hands over a pool (`Vec<Arc<dyn Connection>>`); the number of sessions *is* the concurrency limit.
//! One worker task per session pulls the next table off a shared queue, so a slow table occupies one
//! worker while the others carry on, and no more than `pool.len()` statements are ever in flight. A pool
//! of one (an in-memory SQLite or DuckDB database cannot be opened a second time) simply runs the tables
//! one after another.
//!
//! **Time and cancellation.** Each statement races a timer and the cancel token. When either wins, the
//! session is asked to cancel the running statement (`Connection::cancel`, which the drivers turn into a
//! server-side cancel or an interrupt), the aborted `execute` is awaited for a few seconds so the session
//! is idle again, and the table is reported as timed out or cancelled. Nothing is dropped silently:
//! every table of the plan ends up in exactly one of the outcomes below, or is counted as `not_run` when
//! the search was cancelled before it started.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use theta_core::{Connection, Error, ExecOptions};
use theta_sql::search::{Counts, TableJob, parse_counts};
use tokio::sync::Notify;

/// A cancel switch that can be shared, flipped from the UI thread and awaited by workers.
#[derive(Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    async fn cancelled(&self) {
        loop {
            // Register for the wake-up before looking at the flag, so a cancel between the two is not lost.
            let wake = self.notify.notified();
            if self.is_cancelled() {
                return;
            }
            wake.await;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchOutcome {
    /// At least one row matched.
    Matched(Counts),
    /// Looked at `scanned` rows, found nothing.
    NoMatch {
        scanned: u64,
    },
    TimedOut,
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableResult {
    pub schema: String,
    pub table: String,
    pub outcome: SearchOutcome,
    /// The table has at least as many rows as the limit, so rows beyond it were not looked at.
    pub partial: bool,
    pub columns_cut: usize,
    pub elapsed: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchReport {
    pub tables: usize,
    pub matched_tables: usize,
    pub matched_rows: u64,
    pub timed_out: usize,
    pub failed: usize,
    /// Tables that were never started because the search was cancelled first.
    pub not_run: usize,
    pub cancelled: bool,
    pub elapsed: Duration,
}

/// How long an aborted statement gets to hand its session back.
const ABORT_GRACE: Duration = Duration::from_secs(5);

async fn ask(
    conn: &Arc<dyn Connection>,
    job: &TableJob,
    timeout: Duration,
    cancel: &CancelToken,
) -> SearchOutcome {
    let opts = ExecOptions { row_limit: Some(2) };
    let mut exec = conn.execute(&job.sql, &opts);
    let stopped = tokio::select! {
        r = &mut exec => Ok(r),
        _ = tokio::time::sleep(timeout) => Err(SearchOutcome::TimedOut),
        _ = cancel.cancelled() => Err(SearchOutcome::Cancelled),
    };
    let answer = match stopped {
        Ok(r) => r,
        Err(why) => {
            // Ask the server to stop, then wait (bounded) for the aborted statement to return so the
            // session can take the next table.
            let _ = conn.cancel().await;
            let _ = tokio::time::timeout(ABORT_GRACE, &mut exec).await;
            return why;
        }
    };
    match answer {
        Ok(outcome) => {
            let Some(row) = outcome.result.as_ref().and_then(|r| r.rows.first()) else {
                return SearchOutcome::Failed("the server returned no row".into());
            };
            match parse_counts(job, row) {
                Ok(c) if c.matched_rows == 0 => SearchOutcome::NoMatch { scanned: c.scanned },
                Ok(c) => SearchOutcome::Matched(c),
                Err(e) => SearchOutcome::Failed(e),
            }
        }
        Err(Error::Cancelled) if cancel.is_cancelled() => SearchOutcome::Cancelled,
        Err(e) => SearchOutcome::Failed(e.to_string()),
    }
}

/// Run `jobs` over the sessions in `pool`; `on_result` is called once per table, from the worker that
/// finished it, as soon as it is known. `row_limit` is only used to flag a table as partially scanned.
pub async fn run_search(
    pool: Vec<Arc<dyn Connection>>,
    jobs: Vec<TableJob>,
    row_limit: usize,
    timeout: Duration,
    cancel: CancelToken,
    on_result: Arc<dyn Fn(TableResult) + Send + Sync>,
) -> SearchReport {
    let started = Instant::now();
    let total = jobs.len();
    let queue = Arc::new(Mutex::new(jobs.into_iter().collect::<VecDeque<_>>()));
    let report = Arc::new(Mutex::new(SearchReport { tables: total, ..Default::default() }));
    let mut workers = Vec::new();
    for conn in pool {
        let (queue, report, cancel, on_result) =
            (queue.clone(), report.clone(), cancel.clone(), on_result.clone());
        workers.push(tokio::spawn(async move {
            loop {
                if cancel.is_cancelled() {
                    return;
                }
                let Some(job) = queue.lock().unwrap().pop_front() else { return };
                let t0 = Instant::now();
                let outcome = ask(&conn, &job, timeout, &cancel).await;
                let partial = match &outcome {
                    SearchOutcome::Matched(c) => c.scanned >= row_limit as u64,
                    SearchOutcome::NoMatch { scanned } => *scanned >= row_limit as u64,
                    _ => false,
                };
                {
                    let mut r = report.lock().unwrap();
                    match &outcome {
                        SearchOutcome::Matched(c) => {
                            r.matched_tables += 1;
                            r.matched_rows += c.matched_rows;
                        }
                        SearchOutcome::TimedOut => r.timed_out += 1,
                        SearchOutcome::Failed(_) => r.failed += 1,
                        _ => {}
                    }
                }
                on_result(TableResult {
                    schema: job.schema,
                    table: job.table,
                    outcome,
                    partial,
                    columns_cut: job.columns_cut,
                    elapsed: t0.elapsed(),
                });
            }
        }));
    }
    for w in workers {
        // A panicking worker would otherwise vanish; its tables are counted as not run below.
        let _ = w.await;
    }
    let mut r = report.lock().unwrap().clone();
    r.not_run = queue.lock().unwrap().len();
    r.cancelled = cancel.is_cancelled();
    r.elapsed = started.elapsed();
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn a_cancelled_token_wakes_a_waiter_even_when_cancel_comes_first() {
        let t = CancelToken::new();
        t.cancel();
        tokio::time::timeout(Duration::from_secs(1), t.cancelled()).await.expect("already cancelled");
        let t2 = CancelToken::new();
        let waiter = {
            let t2 = t2.clone();
            tokio::spawn(async move { t2.cancelled().await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        t2.cancel();
        tokio::time::timeout(Duration::from_secs(1), waiter).await.expect("woken").unwrap();
    }

    #[tokio::test]
    async fn no_pool_means_nothing_runs_and_everything_is_counted() {
        let job = TableJob {
            schema: "s".into(),
            table: "t".into(),
            columns: vec!["a".into()],
            columns_cut: 0,
            sql: "select 1".into(),
            estimate: None,
        };
        let rep = run_search(
            Vec::new(),
            vec![job],
            10,
            Duration::from_secs(1),
            CancelToken::new(),
            Arc::new(|_| panic!("no table can have run")),
        )
        .await;
        assert_eq!((rep.tables, rep.not_run), (1, 1));
    }
}
