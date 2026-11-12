//! Whole-table export: page through a table and stream it to a file, so the size of the table is not
//! limited by the result row limit or by memory.
//!
//! Paging is keyset-based when the table has a primary key (`WHERE (k1, k2) > (…) ORDER BY k1, k2 LIMIT n`):
//! every page costs the same however far in it is, and rows inserted or deleted during the export cannot
//! shift later pages. Without a primary key the only portable option is `LIMIT … OFFSET …`, which is slower
//! on deep pages and can repeat or miss rows if the table changes meanwhile; the report says which was used.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use theta_core::{Connection, DatabaseKind, ExecOptions, Value};
use theta_sql::keywords::{qualified_name, quote_if_needed};
use theta_sql::{ExportEncoder, ExportFormat, sql_literal};

#[derive(Debug, Clone)]
pub struct ExportJob {
    pub kind: DatabaseKind,
    pub schema: Option<String>,
    pub table: String,
    /// Primary key columns in key order; empty when the table has none.
    pub key: Vec<String>,
    /// The columns to read, in order; `None` is `SELECT *`. A dump leaves out generated columns, which a
    /// restore cannot insert into.
    pub columns: Option<Vec<String>>,
    pub format: ExportFormat,
    pub page_rows: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paging {
    Keyset,
    Offset,
}

#[derive(Debug, Clone, Default)]
pub struct ExportProgress {
    pub rows: u64,
    pub bytes: u64,
    /// The planner's estimate for the table, to scale a progress bar; not exact.
    pub rows_estimate: Option<u64>,
    pub elapsed: Duration,
}

impl ExportProgress {
    pub fn fraction(&self) -> Option<f32> {
        self.rows_estimate.filter(|e| *e > 0).map(|e| (self.rows as f32 / e as f32).min(0.99))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportOutcome {
    Completed,
    Cancelled,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct ExportReport {
    pub outcome: ExportOutcome,
    pub rows: u64,
    pub bytes: u64,
    pub pages: u64,
    pub paging: Paging,
    pub elapsed: Duration,
}

/// The query for one page. `after` holds the key of the last row of the previous page.
pub fn page_query(job: &ExportJob, after: Option<&[Value]>, offset: u64) -> String {
    let table = qualified_name(job.schema.as_deref(), &job.table, job.kind);
    let n = job.page_rows.max(1);
    let select = match &job.columns {
        Some(c) if !c.is_empty() => {
            c.iter().map(|c| quote_if_needed(c, job.kind)).collect::<Vec<_>>().join(", ")
        }
        _ => "*".to_string(),
    };
    if job.key.is_empty() {
        return format!("SELECT {select} FROM {table} LIMIT {n} OFFSET {offset}");
    }
    let keys: Vec<String> = job.key.iter().map(|k| quote_if_needed(k, job.kind)).collect();
    let order = keys.join(", ");
    let filter = match after {
        None => String::new(),
        Some(vals) => {
            let lits: Vec<String> = vals.iter().map(|v| sql_literal(v, job.kind)).collect();
            if keys.len() == 1 {
                format!(" WHERE {} > {}", keys[0], lits[0])
            } else {
                format!(" WHERE ({}) > ({})", keys.join(", "), lits.join(", "))
            }
        }
    };
    format!("SELECT {select} FROM {table}{filter} ORDER BY {order} LIMIT {n}")
}

/// Stream the table into `out`.
pub async fn export_table<W: Write + Send>(
    conn: &dyn Connection,
    mut out: W,
    job: &ExportJob,
    rows_estimate: Option<u64>,
    cancel: &AtomicBool,
    progress: &(dyn Fn(&ExportProgress) + Send + Sync),
) -> ExportReport {
    let started = Instant::now();
    let paging = if job.key.is_empty() { Paging::Offset } else { Paging::Keyset };
    let mut rep = ExportReport {
        outcome: ExportOutcome::Completed,
        rows: 0,
        bytes: 0,
        pages: 0,
        paging,
        elapsed: Duration::ZERO,
    };
    let mut encoder: Option<ExportEncoder> = None;
    let mut key_idx: Vec<usize> = Vec::new();
    let mut after: Option<Vec<Value>> = None;
    let sql_target = job.schema.as_ref().map_or(job.table.clone(), |s| format!("{s}.{}", job.table));
    let mut format = job.format.clone();
    if let ExportFormat::SqlInserts { table } | ExportFormat::SqlInsertsMulti { table, .. } = &mut format
        && table.is_empty()
    {
        *table = sql_target;
    }
    macro_rules! write_out {
        ($s:expr) => {{
            let s: String = $s;
            if !s.is_empty() {
                if let Err(e) = out.write_all(s.as_bytes()) {
                    rep.outcome = ExportOutcome::Failed(format!("Writing the file failed. {}", theta_core::friendly::plain(&e.to_string())));
                    rep.elapsed = started.elapsed();
                    return rep;
                }
                rep.bytes += s.len() as u64;
            }
        }};
    }
    loop {
        if cancel.load(Ordering::Relaxed) {
            rep.outcome = ExportOutcome::Cancelled;
            break;
        }
        let sql = page_query(job, after.as_deref(), rep.rows);
        let outcome = match conn.execute(&sql, &ExecOptions { row_limit: None }).await {
            Ok(o) => o,
            Err(e) => {
                rep.outcome = ExportOutcome::Failed(e.to_string());
                break;
            }
        };
        let Some(rs) = outcome.result else {
            rep.outcome = ExportOutcome::Failed("the server returned no result set".into());
            break;
        };
        if encoder.is_none() {
            key_idx = job
                .key
                .iter()
                .filter_map(|k| rs.columns.iter().position(|c| c.name.eq_ignore_ascii_case(k)))
                .collect();
            if key_idx.len() != job.key.len() && !job.key.is_empty() {
                rep.outcome =
                    ExportOutcome::Failed("the primary key columns are missing from the result".into());
                break;
            }
            let Some(mut enc) = ExportEncoder::new(format.clone(), job.kind, rs.columns.clone()) else {
                rep.outcome = ExportOutcome::Failed("this format cannot be streamed".into());
                break;
            };
            let head = enc.begin();
            encoder = Some(enc);
            write_out!(head);
        }
        let n = rs.rows.len();
        if n > 0 {
            if let Some(enc) = encoder.as_mut() {
                let text = enc.rows(&rs.rows);
                write_out!(text);
            }
            if let Some(last) = rs.rows.last() {
                after = Some(key_idx.iter().map(|&i| last[i].clone()).collect());
            }
        }
        rep.rows += n as u64;
        rep.pages += 1;
        progress(&ExportProgress {
            rows: rep.rows,
            bytes: rep.bytes,
            rows_estimate,
            elapsed: started.elapsed(),
        });
        if n < job.page_rows.max(1) {
            break;
        }
    }
    if rep.outcome == ExportOutcome::Completed {
        if let Some(enc) = encoder.as_mut() {
            let tail = enc.finish();
            write_out!(tail);
        }
        if let Err(e) = out.flush() {
            rep.outcome = ExportOutcome::Failed(format!("Writing the file failed. {}", theta_core::friendly::plain(&e.to_string())));
        }
    }
    rep.elapsed = started.elapsed();
    progress(&ExportProgress { rows: rep.rows, bytes: rep.bytes, rows_estimate, elapsed: rep.elapsed });
    rep
}

/// Export to `path` through a sibling `.part` file that is renamed on success and deleted otherwise, so a
/// cancelled or failed export never leaves a truncated file under the real name (or replaces a good one).
pub async fn export_table_to_path(
    conn: &dyn Connection,
    path: &Path,
    job: &ExportJob,
    rows_estimate: Option<u64>,
    cancel: &AtomicBool,
    progress: &(dyn Fn(&ExportProgress) + Send + Sync),
) -> ExportReport {
    let part = part_path(path);
    let file = match std::fs::File::create(&part) {
        Ok(f) => f,
        Err(e) => {
            return ExportReport {
                outcome: ExportOutcome::Failed(theta_core::friendly::io_message(&e, path, theta_core::friendly::IoOp::Write)),
                rows: 0,
                bytes: 0,
                pages: 0,
                paging: if job.key.is_empty() { Paging::Offset } else { Paging::Keyset },
                elapsed: Duration::ZERO,
            };
        }
    };
    let writer = std::io::BufWriter::with_capacity(256 * 1024, file);
    let mut rep = export_table(conn, writer, job, rows_estimate, cancel, progress).await;
    if rep.outcome == ExportOutcome::Completed {
        if let Err(e) = std::fs::rename(&part, path) {
            rep.outcome = ExportOutcome::Failed(format!("The finished file could not be moved to {}. {}", path.display(), theta_core::friendly::plain(&e.to_string())));
            let _ = std::fs::remove_file(&part);
        }
    } else {
        let _ = std::fs::remove_file(&part);
    }
    rep
}

fn part_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".part");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(key: &[&str]) -> ExportJob {
        ExportJob {
            kind: DatabaseKind::Postgres,
            schema: Some("public".into()),
            table: "order".into(),
            key: key.iter().map(|s| s.to_string()).collect(),
            columns: None,
            format: ExportFormat::Csv,
            page_rows: 500,
        }
    }

    #[test]
    fn keyset_queries() {
        assert_eq!(
            page_query(&job(&["id"]), None, 0),
            "SELECT * FROM public.\"order\" ORDER BY id LIMIT 500"
        );
        assert_eq!(
            page_query(&job(&["id"]), Some(&[Value::Int(7)]), 0),
            "SELECT * FROM public.\"order\" WHERE id > 7 ORDER BY id LIMIT 500"
        );
        assert_eq!(
            page_query(&job(&["a", "b"]), Some(&[Value::Int(1), Value::Text("x'y".into())]), 0),
            "SELECT * FROM public.\"order\" WHERE (a, b) > (1, 'x''y') ORDER BY a, b LIMIT 500"
        );
    }

    #[test]
    fn offset_queries_without_a_key() {
        assert_eq!(page_query(&job(&[]), None, 1500), "SELECT * FROM public.\"order\" LIMIT 500 OFFSET 1500");
    }

    #[test]
    fn part_files_sit_beside_the_target() {
        assert_eq!(part_path(Path::new("/tmp/x/out.csv")), PathBuf::from("/tmp/x/out.csv.part"));
    }
}
