//! The import pipeline: parse, convert, batch, insert.
//!
//! One function does the reading ([`RowReader`]) and is shared by **Validate** (no connection, nothing
//! written) and **Import**, so what validation promises is exactly what the import will convert.
//!
//! # Atomicity
//!
//! The import is one transaction: cancelled, aborted or failed means none of the rows are there afterwards,
//! which is what the verification in the milestone counts. Each INSERT batch additionally runs inside a
//! savepoint. When a batch is rejected the savepoint is rolled back and the batch is retried row by row, so
//! the error names the exact line (in `Abort` mode) or the bad rows are skipped and listed (in `Skip` mode).
//! A Postgres transaction cannot continue after a failed statement without that savepoint.

use crate::convert::convert;
use crate::job::*;
use crate::parser::{CsvReader, ParseError, Record};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use theta_core::Family;
use theta_core::{Connection, ExecOptions, Value};
use theta_sql::keywords::{qualified_name, quote_if_needed};
use theta_sql::sql_literal;

/// What the reader produced for one record.
pub enum Item {
    Row { line: u64, row: u64, values: Vec<Value> },
    Bad(RowIssue),
}

pub struct RowReader<R: Read> {
    reader: CsvReader<R>,
    rec: Record,
    columns: Vec<ImportColumn>,
    names: Vec<String>,
    opts: crate::convert::ConvertOptions,
    width: Option<usize>,
    data_row: u64,
    /// Header cells, when the file has a header row.
    pub header: Option<Vec<String>>,
}

impl<R: Read> RowReader<R> {
    /// Opens the reader and consumes the header row if the job says there is one.
    pub fn new(input: R, job: &ImportJob) -> Result<Self, ParseError> {
        let mut reader = CsvReader::new(input, job.csv.clone());
        let mut rec = Record::default();
        let mut header = None;
        let mut width = None;
        if job.has_header && reader.read_record(&mut rec)? {
            header = Some(rec.fields().iter().map(|f| f.text.trim().to_string()).collect::<Vec<_>>());
            width = Some(rec.len());
        }
        Ok(Self {
            reader,
            rec,
            names: job.columns.iter().map(|c| c.name.clone()).collect(),
            columns: job.columns.clone(),
            opts: job.convert.clone(),
            width,
            data_row: 0,
            header,
        })
    }

    pub fn bytes_consumed(&self) -> u64 {
        self.reader.bytes_consumed()
    }

    pub fn source_width(&self) -> usize {
        self.width.unwrap_or(0)
    }

    pub fn had_decode_errors(&self) -> bool {
        self.reader.had_decode_errors()
    }

    /// The next record converted, or the reason it cannot be. `Ok(None)` at the end of the file.
    pub fn next_item(&mut self) -> Result<Option<Item>, ParseError> {
        if !self.reader.read_record(&mut self.rec)? {
            return Ok(None);
        }
        self.data_row += 1;
        let (line, row) = (self.rec.line, self.data_row);
        let width = *self.width.get_or_insert(self.rec.len());
        if self.rec.len() != width {
            return Ok(Some(Item::Bad(RowIssue {
                line,
                row,
                column: None,
                reason: format!("found {} fields, expected {width}", self.rec.len()),
            })));
        }
        let mut values = Vec::with_capacity(self.columns.len());
        for (c, name) in self.columns.iter().zip(&self.names) {
            let Some(field) = self.rec.fields().get(c.source) else {
                return Ok(Some(Item::Bad(RowIssue {
                    line,
                    row,
                    column: Some(name.clone()),
                    reason: format!("the file has no column {}", c.source + 1),
                })));
            };
            match convert(field, &c.target, &self.opts) {
                Ok(v) => values.push(v),
                Err(e) => {
                    return Ok(Some(Item::Bad(RowIssue {
                        line,
                        row,
                        column: Some(name.clone()),
                        reason: e.reason,
                    })));
                }
            }
        }
        Ok(Some(Item::Row { line, row, values }))
    }
}

fn snapshot(rr_bytes: u64, total: Option<u64>, rep: &ImportReport, started: Instant) -> Progress {
    Progress {
        rows_read: rep.rows_read,
        rows_inserted: rep.rows_inserted,
        rows_skipped: rep.rows_skipped,
        bytes_read: rr_bytes,
        bytes_total: total,
        elapsed: started.elapsed(),
    }
}

/// Parse and convert the whole input without touching a database. `rows_inserted` of the report is the
/// number of rows that *would* be inserted. Every problem is listed (up to [`MAX_ISSUES`]), whatever the
/// job's error mode: the point of validating is to see them all.
pub fn validate<R: Read>(
    input: R,
    job: &ImportJob,
    total_bytes: Option<u64>,
    cancel: &AtomicBool,
    progress: &dyn Fn(&Progress),
) -> ImportReport {
    let started = Instant::now();
    let mut rep = ImportReport::new();
    let mut rr = match RowReader::new(input, job) {
        Ok(r) => r,
        Err(e) => {
            rep.outcome = Outcome::Failed(e.to_string());
            return rep;
        }
    };
    loop {
        if rep.rows_read.is_multiple_of(4096) {
            if cancel.load(Ordering::Relaxed) {
                rep.outcome = Outcome::Cancelled;
                break;
            }
            progress(&snapshot(rr.bytes_consumed(), total_bytes, &rep, started));
        }
        match rr.next_item() {
            Ok(Some(Item::Row { .. })) => {
                rep.rows_read += 1;
                rep.rows_inserted += 1;
            }
            Ok(Some(Item::Bad(issue))) => {
                rep.rows_read += 1;
                rep.rows_skipped += 1;
                rep.push_issue(issue);
            }
            Ok(None) => break,
            Err(e) => {
                rep.outcome = Outcome::Failed(e.to_string());
                break;
            }
        }
    }
    rep.source_width = rr.source_width();
    rep.encoding_warning = rr.had_decode_errors();
    rep.elapsed = started.elapsed();
    progress(&snapshot(rr.bytes_consumed(), total_bytes, &rep, started));
    rep
}

/// Text of an INSERT: the single place rows are rendered, so statement size is known as it is built.
pub struct InsertBuilder {
    prefix: String,
    kind: theta_core::DatabaseKind,
    tuples: Vec<String>,
    bytes: usize,
}

impl InsertBuilder {
    pub fn new(job: &ImportJob) -> Self {
        let cols: Vec<String> = job.columns.iter().map(|c| quote_if_needed(&c.name, job.kind)).collect();
        let prefix = format!(
            "INSERT INTO {} ({}) VALUES ",
            qualified_name(job.schema.as_deref(), &job.table, job.kind),
            cols.join(", ")
        );
        Self { bytes: prefix.len(), prefix, kind: job.kind, tuples: Vec::new() }
    }

    pub fn tuple(&self, row: &[Value]) -> String {
        let lits: Vec<String> = row.iter().map(|v| sql_literal(v, self.kind)).collect();
        format!("({})", lits.join(", "))
    }

    pub fn len(&self) -> usize {
        self.tuples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tuples.is_empty()
    }

    /// Whether adding `tuple` keeps the statement within `budget` (an empty builder always accepts).
    pub fn fits(&self, tuple: &str, budget: usize) -> bool {
        self.tuples.is_empty() || self.bytes + tuple.len() + 2 <= budget
    }

    pub fn push(&mut self, tuple: String) {
        self.bytes += tuple.len() + 2;
        self.tuples.push(tuple);
    }

    pub fn statement(&self) -> String {
        format!("{}{}", self.prefix, self.tuples.join(",\n"))
    }

    pub fn statement_for(&self, tuple: &str) -> String {
        format!("{}{}", self.prefix, tuple)
    }

    pub fn clear(&mut self) {
        self.tuples.clear();
        self.bytes = self.prefix.len();
    }
}

const SAVEPOINT_ALL: &str = "theta_import";
const SAVEPOINT_BATCH: &str = "theta_import_batch";

async fn exec(conn: &dyn Connection, sql: &str, rep: &mut ImportReport) -> Result<(), String> {
    rep.statements += 1;
    conn.execute(sql, &ExecOptions { row_limit: Some(0) }).await.map(|_| ()).map_err(|e| e.to_string())
}

/// Undo everything the import did. Errors are ignored on purpose: this runs while reporting another
/// failure, and the connection may be gone.
async fn rollback_all(conn: &dyn Connection, job: &ImportJob, began: bool, rep: &mut ImportReport) {
    match &job.txn {
        TxnPlan::Own { .. } => {
            let _ = exec(conn, "ROLLBACK", rep).await;
        }
        TxnPlan::Session { .. } => {
            if began {
                let _ = exec(conn, "ROLLBACK", rep).await;
            } else {
                let _ = exec(conn, &format!("ROLLBACK TO SAVEPOINT {SAVEPOINT_ALL}"), rep).await;
                let _ = exec(conn, &format!("RELEASE SAVEPOINT {SAVEPOINT_ALL}"), rep).await;
            }
        }
    }
    if rep.created_table && job.kind.family() == Family::MySql {
        // CREATE TABLE committed implicitly on MySQL, so the rollback cannot remove it.
        let name = qualified_name(job.schema.as_deref(), &job.table, job.kind);
        let _ = exec(conn, &format!("DROP TABLE {name}"), rep).await;
        rep.created_table = false;
    } else {
        rep.created_table = false;
    }
    rep.rows_inserted = 0;
    rep.committed = false;
}

/// Run an import. See the module documentation for the transaction behaviour.
pub async fn run_import<R: Read + Send>(
    conn: &dyn Connection,
    input: R,
    job: &ImportJob,
    total_bytes: Option<u64>,
    cancel: &AtomicBool,
    progress: &(dyn Fn(&Progress) + Send + Sync),
) -> ImportReport {
    let started = Instant::now();
    let mut rep = ImportReport::new();
    let mut rr = match RowReader::new(input, job) {
        Ok(r) => r,
        Err(e) => {
            rep.outcome = Outcome::Failed(e.to_string());
            return rep;
        }
    };
    let mysql_ddl = job.create_sql.is_some() && job.kind.family() == Family::MySql;

    macro_rules! fail {
        ($began:expr, $msg:expr) => {{
            rep.outcome = Outcome::Failed($msg);
            rollback_all(conn, job, $began, &mut rep).await;
            rep.elapsed = started.elapsed();
            return rep;
        }};
    }

    // MySQL commits implicitly around DDL, so the table is created before the transaction starts.
    if mysql_ddl {
        if let Some(sql) = &job.create_sql
            && let Err(e) = exec(conn, sql, &mut rep).await
        {
            rep.outcome = Outcome::Failed(format!("creating the table failed: {e}"));
            return rep;
        }
        rep.created_table = true;
    }
    let mut began = false;
    let begin = |v: &Vec<String>| v.clone();
    match &job.txn {
        TxnPlan::Own { begin: b } => {
            for s in begin(b) {
                if let Err(e) = exec(conn, &s, &mut rep).await {
                    rep.outcome = Outcome::Failed(format!("could not start a transaction: {e}"));
                    if rep.created_table {
                        rollback_all(conn, job, false, &mut rep).await;
                    }
                    return rep;
                }
            }
        }
        TxnPlan::Session { open, begin: b } => {
            if !*open {
                for s in begin(b) {
                    if let Err(e) = exec(conn, &s, &mut rep).await {
                        rep.outcome = Outcome::Failed(format!("could not start a transaction: {e}"));
                        if rep.created_table {
                            rollback_all(conn, job, false, &mut rep).await;
                        }
                        return rep;
                    }
                }
                began = true;
            }
            if let Err(e) = exec(conn, &format!("SAVEPOINT {SAVEPOINT_ALL}"), &mut rep).await {
                fail!(began, format!("could not create a savepoint: {e}"));
            }
        }
    }
    if !mysql_ddl && let Some(sql) = &job.create_sql {
        if let Err(e) = exec(conn, sql, &mut rep).await {
            fail!(began, format!("creating the table failed: {e}"));
        }
        rep.created_table = true;
    }
    if let Some(sql) = &job.truncate_sql
        && let Err(e) = exec(conn, sql, &mut rep).await
    {
        fail!(began, format!("emptying the table failed: {e}"));
    }

    let mut builder = InsertBuilder::new(job);
    // Source position of the rows in the current batch, for error messages.
    let mut batch_rows: Vec<(u64, u64, String)> = Vec::new();
    let mut at_end = false;
    let budget = job.max_statement_bytes.max(1024);
    let batch_limit = job.batch_rows.max(1);
    // A tuple read but not yet placed because the batch was full.
    let mut carry: Option<(u64, u64, String)> = None;
    while !at_end || carry.is_some() || !builder.is_empty() {
        if cancel.load(Ordering::Relaxed) {
            rep.outcome = Outcome::Cancelled;
            rollback_all(conn, job, began, &mut rep).await;
            rep.elapsed = started.elapsed();
            return rep;
        }
        // Fill one batch.
        if let Some((l, r, t)) = carry.take() {
            builder.push(t.clone());
            batch_rows.push((l, r, t));
        }
        while !at_end && builder.len() < batch_limit {
            match rr.next_item() {
                Ok(Some(Item::Row { line, row, values })) => {
                    rep.rows_read += 1;
                    let t = builder.tuple(&values);
                    if !builder.fits(&t, budget) {
                        carry = Some((line, row, t));
                        break;
                    }
                    builder.push(t.clone());
                    batch_rows.push((line, row, t));
                }
                Ok(Some(Item::Bad(issue))) => {
                    rep.rows_read += 1;
                    if job.on_error == ErrorMode::Abort {
                        let msg = issue.to_string();
                        rep.push_issue(issue);
                        fail!(began, msg);
                    }
                    rep.rows_skipped += 1;
                    rep.push_issue(issue);
                }
                Ok(None) => at_end = true,
                Err(e) => fail!(began, e.to_string()),
            }
        }
        if builder.is_empty() {
            continue;
        }
        let n = builder.len() as u64;
        if let Err(e) = exec(conn, &format!("SAVEPOINT {SAVEPOINT_BATCH}"), &mut rep).await {
            fail!(began, format!("could not create a savepoint: {e}"));
        }
        match exec(conn, &builder.statement(), &mut rep).await {
            Ok(()) => {
                if let Err(e) = exec(conn, &format!("RELEASE SAVEPOINT {SAVEPOINT_BATCH}"), &mut rep).await {
                    fail!(began, e);
                }
                rep.rows_inserted += n;
            }
            Err(batch_err) => {
                // Roll the batch back and find the rows the server objects to.
                for s in [
                    format!("ROLLBACK TO SAVEPOINT {SAVEPOINT_BATCH}"),
                    format!("RELEASE SAVEPOINT {SAVEPOINT_BATCH}"),
                ] {
                    if let Err(e) = exec(conn, &s, &mut rep).await {
                        fail!(began, format!("{batch_err} (and the rollback failed: {e})"));
                    }
                }
                for (line, row, tuple) in &batch_rows {
                    if let Err(e) = exec(conn, &format!("SAVEPOINT {SAVEPOINT_BATCH}"), &mut rep).await {
                        fail!(began, e);
                    }
                    match exec(conn, &builder.statement_for(tuple), &mut rep).await {
                        Ok(()) => {
                            if let Err(e) =
                                exec(conn, &format!("RELEASE SAVEPOINT {SAVEPOINT_BATCH}"), &mut rep).await
                            {
                                fail!(began, e);
                            }
                            rep.rows_inserted += 1;
                        }
                        Err(e) => {
                            for s in [
                                format!("ROLLBACK TO SAVEPOINT {SAVEPOINT_BATCH}"),
                                format!("RELEASE SAVEPOINT {SAVEPOINT_BATCH}"),
                            ] {
                                if let Err(e2) = exec(conn, &s, &mut rep).await {
                                    fail!(began, format!("{e} (and the rollback failed: {e2})"));
                                }
                            }
                            let issue = RowIssue {
                                line: *line,
                                row: *row,
                                column: None,
                                reason: format!("rejected by the server: {e}"),
                            };
                            if job.on_error == ErrorMode::Abort {
                                let msg = issue.to_string();
                                rep.push_issue(issue);
                                fail!(began, msg);
                            }
                            rep.rows_skipped += 1;
                            rep.push_issue(issue);
                        }
                    }
                }
            }
        }
        builder.clear();
        batch_rows.clear();
        progress(&snapshot(rr.bytes_consumed(), total_bytes, &rep, started));
    }

    // Commit (or hand the open transaction back).
    match &job.txn {
        TxnPlan::Own { .. } => {
            if let Err(e) = exec(conn, "COMMIT", &mut rep).await {
                fail!(began, format!("commit failed: {e}"));
            }
            rep.committed = true;
        }
        TxnPlan::Session { .. } => {
            if let Err(e) = exec(conn, &format!("RELEASE SAVEPOINT {SAVEPOINT_ALL}"), &mut rep).await {
                fail!(began, e);
            }
        }
    }
    rep.source_width = rr.source_width();
    rep.encoding_warning = rr.had_decode_errors();
    rep.elapsed = started.elapsed();
    progress(&snapshot(rr.bytes_consumed(), total_bytes, &rep, started));
    rep
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::{ConvertOptions, Target};
    use crate::parser::CsvOptions;
    use theta_core::{ColumnInfo, DatabaseKind, TypeKind};

    fn col(name: &str, source: usize, kind: TypeKind, ty: &str) -> ImportColumn {
        ImportColumn {
            source,
            name: name.into(),
            target: Target::from_column(
                &ColumnInfo {
                    name: name.into(),
                    data_type: ty.into(),
                    kind,
                    nullable: true,
                    default: None,
                    comment: None,
                    geometry: None,
                    ..Default::default()
                },
                DatabaseKind::Postgres,
            ),
        }
    }

    pub(crate) fn job() -> ImportJob {
        ImportJob {
            kind: DatabaseKind::Postgres,
            schema: Some("public".into()),
            table: "t".into(),
            create_sql: None,
            truncate_sql: None,
            columns: vec![col("id", 0, TypeKind::Integer, "integer"), col("name", 1, TypeKind::Text, "text")],
            csv: CsvOptions::default(),
            has_header: true,
            convert: ConvertOptions::default(),
            on_error: ErrorMode::Abort,
            batch_rows: 2,
            max_statement_bytes: DEFAULT_STATEMENT_BYTES,
            txn: TxnPlan::Own { begin: vec!["BEGIN".into()] },
        }
    }

    #[test]
    fn reader_converts_and_reports_bad_rows_with_lines() {
        let data = "id,name\n1,Ann\n2\nx,Bob\n4,\"multi\nline\"\n5,Eve\n";
        let mut rr = RowReader::new(data.as_bytes(), &job()).unwrap();
        let mut out = Vec::new();
        while let Some(i) = rr.next_item().unwrap() {
            out.push(match i {
                Item::Row { line, row, values } => format!("ok {line} {row} {values:?}"),
                Item::Bad(b) => format!("bad {b}"),
            });
        }
        assert_eq!(out.len(), 5);
        assert!(out[0].starts_with("ok 2 1 [Int(1), Text(\"Ann\")]"));
        assert_eq!(out[1], "bad line 3 (row 2): found 1 fields, expected 2");
        assert!(out[2].starts_with("bad line 4 (row 3), column id: \"x\" is not an integer"));
        assert!(out[3].contains("Text(\"multi\\nline\")"));
        assert!(out[3].starts_with("ok 5 4"), "{}", out[3]);
        assert!(out[4].starts_with("ok 7 5"), "{}", out[4]);
    }

    #[test]
    fn validate_lists_every_problem_and_counts_valid_rows() {
        let data = "id,name\n1,a\nx,b\n3,c\ny,d\n";
        let rep = validate(data.as_bytes(), &job(), None, &AtomicBool::new(false), &|_| {});
        assert_eq!(rep.outcome, Outcome::Completed);
        assert_eq!((rep.rows_read, rep.rows_inserted, rep.rows_skipped, rep.issue_count), (4, 2, 2, 2));
        assert_eq!(rep.issues[1].line, 5);
        assert_eq!(rep.source_width, 2);
    }

    #[test]
    fn validate_reports_a_truncated_quote_as_failure() {
        let rep = validate("id,name\n1,\"oops".as_bytes(), &job(), None, &AtomicBool::new(false), &|_| {});
        assert!(matches!(rep.outcome, Outcome::Failed(ref m) if m.contains("never closed")));
    }

    #[test]
    fn validate_can_be_cancelled() {
        let rep = validate("id,name\n1,a\n".as_bytes(), &job(), None, &AtomicBool::new(true), &|_| {});
        assert_eq!(rep.outcome, Outcome::Cancelled);
    }

    #[test]
    fn statement_budget_respects_mysql_packets() {
        assert_eq!(statement_budget(DatabaseKind::Postgres, None), DEFAULT_STATEMENT_BYTES);
        assert_eq!(statement_budget(DatabaseKind::MySql, Some(16 * 1024 * 1024)), DEFAULT_STATEMENT_BYTES);
        assert_eq!(statement_budget(DatabaseKind::MySql, Some(65_536)), 32_768);
        assert_eq!(statement_budget(DatabaseKind::MySql, Some(10)), 4096);
    }

    #[test]
    fn builder_matches_build_insert_and_respects_budget() {
        let j = job();
        let mut b = InsertBuilder::new(&j);
        let rows = vec![vec![Value::Int(1), Value::Text("O'Neil".into())], vec![Value::Int(2), Value::Null]];
        for r in &rows {
            b.push(b.tuple(r));
        }
        let want = theta_sql::build_insert(
            DatabaseKind::Postgres,
            Some("public"),
            "t",
            &["id".to_string(), "name".to_string()],
            &rows,
        );
        // Same text apart from the row separator layout.
        assert_eq!(b.statement().replace(",\n", ", "), want.replace(",\n       ", ", "));
        let big = "x".repeat(200);
        let t = b.tuple(&[Value::Int(3), Value::Text(big)]);
        assert!(!b.fits(&t, 150));
        let mut empty = InsertBuilder::new(&j);
        assert!(empty.fits(&t, 10), "a single oversized row must still be sendable");
        empty.push(t);
    }
}
