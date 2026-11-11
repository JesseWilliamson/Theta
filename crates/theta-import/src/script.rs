//! Run a large `.sql` file statement by statement without loading it into memory.
//!
//! [`StatementStream`] is the pure part: feed it text in chunks, get complete statements back. It re-uses
//! `theta_sql::split_statements` on a buffer that holds only the statement being read plus whatever has not
//! been split yet, and it keeps the *last* statement of the buffer back until more text arrives or the input
//! ends, because the splitter cannot tell "complete" from "cut off by the chunk boundary" at the end of its
//! input (an unterminated string or dollar-quote swallows the rest). Splitting is retried only when the
//! buffer has doubled, so a single 500 MB `INSERT` costs a linear, not a quadratic, number of re-scans.
//!
//! [`run_script`] drives a connection with it: progress after every statement, stop on the first error or
//! collect them all, optionally inside one transaction, cancel between statements.

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use theta_core::{Connection, DatabaseKind, Error, ExecOptions, Family, Feature};
use theta_sql::split_statements;

/// One statement and where it started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub sql: String,
    /// Byte offset of the statement in the input.
    pub offset: u64,
    /// 1-based line of its first character.
    pub line: u64,
}

pub struct StatementStream {
    kind: DatabaseKind,
    buf: String,
    /// Byte offset in the input of `buf[0]`.
    offset: u64,
    /// 1-based line number of `buf[0]`.
    line: u64,
    /// Do not try to split before the buffer is this long.
    next_try: usize,
}

const MIN_TRY: usize = 64 * 1024;

impl StatementStream {
    pub fn new(kind: DatabaseKind) -> Self {
        StatementStream { kind, buf: String::new(), offset: 0, line: 1, next_try: MIN_TRY }
    }

    /// Append `chunk`; return the statements that are certainly complete.
    pub fn push(&mut self, chunk: &str) -> Vec<Piece> {
        self.buf.push_str(chunk);
        if self.buf.len() < self.next_try {
            return Vec::new();
        }
        let spans = split_statements(&self.buf, self.kind);
        if spans.len() < 2 {
            self.next_try = self.buf.len().saturating_mul(2);
            return Vec::new();
        }
        let keep_from = spans.last().map(|s| s.range.start).unwrap_or(0);
        let pieces = self.pieces(&spans[..spans.len() - 1]);
        self.drain(keep_from);
        self.next_try = (self.buf.len() * 2).max(MIN_TRY);
        pieces
    }

    /// The input has ended: everything left is complete.
    pub fn finish(&mut self) -> Vec<Piece> {
        let spans = split_statements(&self.buf, self.kind);
        let pieces = self.pieces(&spans);
        self.drain(self.buf.len());
        pieces
    }

    fn pieces(&self, spans: &[theta_sql::StatementSpan]) -> Vec<Piece> {
        spans
            .iter()
            .map(|s| Piece {
                sql: self.buf[s.range.clone()].to_string(),
                offset: self.offset + s.range.start as u64,
                line: self.line + self.buf[..s.range.start].matches('\n').count() as u64,
            })
            .collect()
    }

    fn drain(&mut self, upto: usize) {
        self.line += self.buf[..upto].matches('\n').count() as u64;
        self.offset += upto as u64;
        self.buf.drain(..upto);
    }
}

/// Decode UTF-8 from a byte stream in chunks: an incomplete character at the end is carried to the next call,
/// and invalid bytes become U+FFFD (a dump in another encoding still runs; its text may be mangled, which
/// the dialog says).
pub struct Utf8Chunks {
    carry: Vec<u8>,
}

impl Default for Utf8Chunks {
    fn default() -> Self {
        Self::new()
    }
}

impl Utf8Chunks {
    pub fn new() -> Self {
        Utf8Chunks { carry: Vec::new() }
    }

    pub fn decode(&mut self, bytes: &[u8]) -> String {
        self.carry.extend_from_slice(bytes);
        let mut out = String::new();
        let mut rest: &[u8] = &self.carry;
        loop {
            match std::str::from_utf8(rest) {
                Ok(s) => {
                    out.push_str(s);
                    rest = &[];
                    break;
                }
                Err(e) => {
                    let (good, bad) = rest.split_at(e.valid_up_to());
                    out.push_str(std::str::from_utf8(good).unwrap_or(""));
                    match e.error_len() {
                        // Cut off at the end of the chunk: wait for the rest of the character.
                        None => {
                            rest = bad;
                            break;
                        }
                        Some(n) => {
                            out.push('\u{FFFD}');
                            rest = &bad[n..];
                        }
                    }
                }
            }
        }
        let keep = rest.to_vec();
        self.carry = keep;
        out
    }

    /// At end of input a dangling partial character is invalid.
    pub fn finish(&mut self) -> String {
        if self.carry.is_empty() {
            String::new()
        } else {
            std::mem::take(&mut self.carry).iter().map(|_| '\u{FFFD}').take(1).collect()
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScriptOptions {
    pub kind: DatabaseKind,
    /// Stop at the first failing statement; otherwise keep going and collect the errors.
    pub stop_on_error: bool,
    /// Wrap the run in BEGIN … COMMIT and roll back if it does not finish cleanly. Implies `stop_on_error`.
    pub single_transaction: bool,
    /// How many errors to keep in the report (the count continues past it).
    pub max_errors: usize,
}

impl ScriptOptions {
    pub fn new(kind: DatabaseKind) -> Self {
        ScriptOptions { kind, stop_on_error: true, single_transaction: false, max_errors: 200 }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ScriptProgress {
    /// Statements finished (successfully or not).
    pub statements: u64,
    pub errors: u64,
    pub bytes_read: u64,
    pub total_bytes: Option<u64>,
    /// The statement now running, shortened to one line.
    pub current: String,
    pub elapsed: Duration,
}

impl ScriptProgress {
    pub fn fraction(&self) -> Option<f32> {
        self.total_bytes.filter(|t| *t > 0).map(|t| (self.bytes_read as f32 / t as f32).min(1.0))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScriptError {
    /// 1-based position among the statements.
    pub statement: u64,
    pub line: u64,
    pub offset: u64,
    /// The first line of the statement.
    pub head: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptOutcome {
    Completed,
    /// Stopped by an error (`stop_on_error`).
    Stopped,
    Cancelled,
    /// The input could not be read or the transaction could not start.
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct ScriptReport {
    pub outcome: ScriptOutcome,
    pub statements: u64,
    pub errors: Vec<ScriptError>,
    pub error_count: u64,
    pub rows_affected: u64,
    pub bytes_read: u64,
    /// A single-transaction run that did not finish was rolled back.
    pub rolled_back: bool,
    pub elapsed: Duration,
}

pub fn begin_sql(kind: DatabaseKind) -> &'static str {
    match kind.family() {
        Family::MySql => "START TRANSACTION",
        Family::SqlServer => "BEGIN TRANSACTION",
        _ => "BEGIN",
    }
}

fn first_line(sql: &str, max: usize) -> String {
    let line = sql.trim_start().lines().next().unwrap_or("");
    if line.chars().count() > max {
        format!("{}…", line.chars().take(max).collect::<String>())
    } else {
        line.to_string()
    }
}

/// What `pg_dump`'s default format uses for table data. It is not SQL the server accepts from a client
/// library, so say so instead of letting every data line fail.
pub fn copy_from_stdin(sql: &str) -> bool {
    let up = sql.trim_start().to_ascii_uppercase();
    up.starts_with("COPY ") && up.lines().next().is_some_and(|l| l.contains("FROM STDIN"))
}

/// Run `reader` as a script. See the module comment.
pub async fn run_script<R: Read + Send>(
    conn: &dyn Connection,
    mut reader: R,
    total_bytes: Option<u64>,
    opts: &ScriptOptions,
    cancel: &AtomicBool,
    progress: &(dyn Fn(&ScriptProgress) + Send + Sync),
) -> ScriptReport {
    let started = Instant::now();
    let mut rep = ScriptReport {
        outcome: ScriptOutcome::Completed,
        statements: 0,
        errors: Vec::new(),
        error_count: 0,
        rows_affected: 0,
        bytes_read: 0,
        rolled_back: false,
        elapsed: Duration::ZERO,
    };
    let stop_on_error = opts.stop_on_error || opts.single_transaction;
    let exec = ExecOptions { row_limit: Some(1) };
    if opts.single_transaction {
        if let Some(why) = opts.kind.unsupported(Feature::Transactions) {
            rep.outcome = ScriptOutcome::Failed(why.to_string());
            return rep;
        }
        if let Err(e) = conn.execute(begin_sql(opts.kind), &exec).await {
            rep.outcome = ScriptOutcome::Failed(format!("could not start the transaction: {e}"));
            return rep;
        }
    }

    let mut stream = StatementStream::new(opts.kind);
    let mut decoder = Utf8Chunks::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut first = true;
    let mut eof = false;
    let mut pending: std::collections::VecDeque<Piece> = std::collections::VecDeque::new();
    'outer: loop {
        if pending.is_empty() {
            if eof {
                break;
            }
            match reader.read(&mut buf) {
                Ok(0) => {
                    eof = true;
                    let tail = decoder.finish();
                    pending.extend(stream.push(&tail));
                    pending.extend(stream.finish());
                }
                Ok(n) => {
                    rep.bytes_read += n as u64;
                    let mut text = decoder.decode(&buf[..n]);
                    if first {
                        first = false;
                        if let Some(rest) = text.strip_prefix('\u{FEFF}') {
                            text = rest.to_string();
                        }
                    }
                    pending.extend(stream.push(&text));
                }
                Err(e) => {
                    rep.outcome = ScriptOutcome::Failed(format!("reading the file failed: {e}"));
                    break;
                }
            }
            continue;
        }
        let Some(piece) = pending.pop_front() else { continue };
        if cancel.load(Ordering::Relaxed) {
            rep.outcome = ScriptOutcome::Cancelled;
            break;
        }
        progress(&ScriptProgress {
            statements: rep.statements,
            errors: rep.error_count,
            bytes_read: rep.bytes_read,
            total_bytes,
            current: first_line(&piece.sql, 100),
            elapsed: started.elapsed(),
        });
        let number = rep.statements + 1;
        let result = if copy_from_stdin(&piece.sql) {
            rep.outcome = ScriptOutcome::Failed(format!(
                "line {}: COPY … FROM stdin carries its data inside the file, which Theta cannot send. Re-create the dump with `pg_dump --inserts` (or `--column-inserts`).",
                piece.line
            ));
            break 'outer;
        } else {
            conn.execute(&piece.sql, &exec).await
        };
        rep.statements = number;
        match result {
            Ok(o) => rep.rows_affected += o.rows_affected.unwrap_or(0),
            Err(Error::Cancelled) => {
                rep.statements -= 1;
                rep.outcome = ScriptOutcome::Cancelled;
                break;
            }
            Err(e) => {
                rep.error_count += 1;
                if rep.errors.len() < opts.max_errors {
                    rep.errors.push(ScriptError {
                        statement: number,
                        line: piece.line,
                        offset: piece.offset,
                        head: first_line(&piece.sql, 160),
                        message: e.to_string(),
                    });
                }
                if stop_on_error {
                    rep.outcome = ScriptOutcome::Stopped;
                    break;
                }
                if e.is_connection_lost() {
                    rep.outcome = ScriptOutcome::Failed(format!("the connection was lost: {e}"));
                    break;
                }
            }
        }
    }
    if opts.single_transaction {
        if rep.outcome == ScriptOutcome::Completed && rep.error_count == 0 {
            if let Err(e) = conn.execute("COMMIT", &exec).await {
                rep.outcome = ScriptOutcome::Failed(format!("COMMIT failed: {e}"));
                let _ = conn.execute("ROLLBACK", &exec).await;
                rep.rolled_back = true;
            }
        } else {
            rep.rolled_back = conn.execute("ROLLBACK", &exec).await.is_ok();
        }
    }
    rep.elapsed = started.elapsed();
    progress(&ScriptProgress {
        statements: rep.statements,
        errors: rep.error_count,
        bytes_read: rep.bytes_read,
        total_bytes,
        current: String::new(),
        elapsed: rep.elapsed,
    });
    rep
}

#[cfg(test)]
mod tests {
    use super::*;
    use DatabaseKind::*;

    fn all(kind: DatabaseKind, text: &str, chunk: usize) -> Vec<Piece> {
        let mut s = StatementStream::new(kind);
        let mut out = Vec::new();
        let mut dec = Utf8Chunks::new();
        for c in text.as_bytes().chunks(chunk) {
            out.extend(s.push(&dec.decode(c)));
        }
        out.extend(s.finish());
        out
    }

    const SCRIPT: &str = "-- header\nCREATE TABLE t (a int, s text);\nINSERT INTO t VALUES (1, 'a;b');\nINSERT INTO t VALUES (2, 'it''s -- not a comment');\n/* block; comment */\nSELECT $$x;y$$;\nUPDATE t SET s = 'multi\nline; text' WHERE a = 1\n";

    #[test]
    fn chunked_input_splits_exactly_like_one_shot() {
        let expect: Vec<String> =
            split_statements(SCRIPT, Postgres).iter().map(|s| SCRIPT[s.range.clone()].to_string()).collect();
        assert_eq!(expect.len(), 5);
        // A chunk of 1 byte puts a boundary inside every string, comment and dollar quote.
        for chunk in [1, 2, 3, 7, 16, 1000] {
            let got: Vec<String> = all(Postgres, SCRIPT, chunk).into_iter().map(|p| p.sql).collect();
            assert_eq!(got, expect, "chunk size {chunk}");
        }
    }

    #[test]
    fn small_buffers_are_not_split_early_but_large_ones_are() {
        let mut s = StatementStream::new(Sqlite);
        assert!(s.push("SELECT 1; SELECT 2; SEL").is_empty(), "below the threshold nothing is released");
        let big = format!("{}SELECT 3;", "SELECT 1;\n".repeat(20_000));
        let first = s.push(&big);
        assert!(first.len() > 19_000, "a large buffer releases everything but its last statement");
        assert_eq!(s.finish().len(), 1);
    }

    #[test]
    fn offsets_and_lines_point_at_the_statement() {
        let text = "SELECT 1;\n\nSELECT 2;\nSELECT\n3;\n";
        let p = all(Sqlite, text, 4);
        assert_eq!((p[0].line, p[0].offset), (1, 0));
        assert_eq!((p[1].line, p[1].offset), (3, 11));
        assert_eq!((p[2].line, p[2].offset), (4, 21));
    }

    #[test]
    fn a_huge_single_statement_is_one_piece() {
        let body = "x".repeat(2_000_000);
        let text = format!("INSERT INTO t VALUES ('{body}');");
        let p = all(Postgres, &text, 200_000);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].sql.len(), text.len() - 1);
    }

    #[test]
    fn utf8_split_across_chunks_survives() {
        let text = "SELECT 'héllo wörld ✓';";
        let p = all(Postgres, text, 1);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].sql, "SELECT 'héllo wörld ✓'");
        let mut d = Utf8Chunks::new();
        assert_eq!(d.decode(&[0x61, 0xFF, 0x62]), "a\u{FFFD}b");
    }

    #[test]
    fn t_sql_go_and_mysql_backticks() {
        let p = all(SqlServer, "SELECT 1\nGO\nSELECT 2\nGO\n", 5);
        assert_eq!(p.len(), 2, "{p:?}");
        let p = all(MySql, "CREATE TABLE `a;b` (x int);\nINSERT INTO `a;b` VALUES (1);", 3);
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn copy_from_stdin_is_recognised() {
        assert!(copy_from_stdin("COPY public.t (a, b) FROM stdin"));
        assert!(!copy_from_stdin("COPY t TO '/tmp/x'"));
        assert!(!copy_from_stdin("SELECT 'copy from stdin'"));
    }

    #[test]
    fn begin_statement_per_product() {
        assert_eq!(begin_sql(MySql), "START TRANSACTION");
        assert_eq!(begin_sql(Postgres), "BEGIN");
        assert_eq!(begin_sql(SqlServer), "BEGIN TRANSACTION");
    }
}
