//! Per-console state: the editor view, results of the last run and the
//! providers feeding completion and diagnostics.

use crate::exec::geo::GeoData;
use gpui_kit::{Entity, Subscription, Task};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use theta_core::{Catalog, DatabaseKind, ResultSet};
use theta_editor_view::{CompletionProvider, EditorView};
use theta_grid::{GridMeta, ResultGrid};
use theta_plan_view::PlanInput;
use theta_sql::{CompletionList, CompletionRequest};

/// The connection's catalog, swapped when it refreshes; read by the providers
/// (completion on the UI thread, diagnostics on a background thread).
pub type CatalogSlot = Arc<RwLock<Option<Arc<Catalog>>>>;

pub struct ResultItem {
    pub title: String,
    pub statement: String,
    pub result: Arc<ResultSet>,
    pub meta: GridMeta,
    pub geo: Option<Arc<GeoData>>,
}

/// What the Plan tab shows for a console. Kept per console so an EXPLAIN that finishes while
/// another tab is active is stored with the console that asked for it, not shown in the other one.
#[derive(Clone, Default)]
pub enum PlanState {
    #[default]
    Empty,
    Loading,
    Ready(PlanInput),
    Failed(String),
}

pub struct Console {
    pub key: String,
    pub editor: Entity<EditorView>,
    /// This console's results grid (shown by the Results panel while the console is active).
    pub grid: Entity<ResultGrid>,
    pub plan: PlanState,
    /// The statement last explained, for "rerun" and "open suggestion".
    pub last_explain: Option<super::run::ExplainTarget>,
    /// Bumped by every EXPLAIN so a slow, superseded one cannot overwrite a newer plan.
    pub explain_gen: u64,
    pub file: Option<PathBuf>,
    pub results: Vec<ResultItem>,
    pub active_result: usize,
    /// One-line outcome of the last statement when no rows came back.
    pub summary: Option<(String, bool)>,
    pub catalog: CatalogSlot,
    pub save_task: Option<Task<()>>,
    /// Parameter values last used in this console, by placeholder label, to pre-fill the dialog.
    pub param_values: std::collections::HashMap<String, theta_sql::params::ParamValue>,
    pub _subs: Vec<Subscription>,
}

pub struct SqlCompletion {
    pub kind: DatabaseKind,
    pub catalog: CatalogSlot,
}

impl CompletionProvider for SqlCompletion {
    fn complete(&self, text: &str, byte_offset: usize) -> CompletionList {
        let catalog = self.catalog.read().ok().and_then(|c| c.clone());
        theta_sql::complete(&CompletionRequest {
            sql: text,
            offset: byte_offset,
            kind: self.kind,
            catalog: catalog.as_deref(),
        })
    }
}

/// The first word of a statement in upper case, for outcome lines ("UPDATE — 3 rows affected").
pub fn verb(sql: &str) -> String {
    let mut rest = sql.trim_start();
    loop {
        if let Some(r) = rest.strip_prefix("--") {
            rest = r.split_once('\n').map(|(_, t)| t).unwrap_or("").trim_start();
        } else if let Some(r) = rest.strip_prefix("/*") {
            rest = r.split_once("*/").map(|(_, t)| t).unwrap_or("").trim_start();
        } else {
            break;
        }
    }
    let w: String = rest.chars().take_while(|c| c.is_alphabetic()).collect();
    if w.is_empty() { "Statement".into() } else { w.to_uppercase() }
}

/// Sub-tab title for result number `number` of a statement. The origin table is named only when
/// the statement is a plain query and every column comes from that one table; a join, an
/// expression column or `SHOW TABLES` gets the neutral "Result N".
pub fn result_title(number: usize, sql: &str, columns: &[theta_core::Column]) -> String {
    let queryish = matches!(verb(sql).as_str(), "SELECT" | "WITH" | "TABLE" | "VALUES");
    let table = columns.first().and_then(|c| c.origin.as_ref()).map(|o| o.table.as_str());
    let shared = queryish
        && table.is_some_and(|t| columns.iter().all(|c| c.origin.as_ref().is_some_and(|o| o.table == t)));
    match table {
        Some(t) if shared => format!("Result {number} · {t}"),
        _ => format!("Result {number}"),
    }
}

pub fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", fmt_int(n), if n == 1 { one } else { many })
}

pub fn fmt_int(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn fmt_ms(d: std::time::Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms >= 10_000.0 {
        format!("{:.1} s", ms / 1000.0)
    } else if ms >= 100.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{ms:.1} ms").replace(".0 ms", " ms")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbs_skip_comments() {
        assert_eq!(verb("-- hi\n  update t set a=1"), "UPDATE");
        assert_eq!(verb("/* x */ select 1"), "SELECT");
        assert_eq!(verb("(select 1)"), "Statement");
    }

    fn col(name: &str, table: Option<&str>) -> theta_core::Column {
        theta_core::Column {
            name: name.into(),
            type_name: "text".into(),
            kind: theta_core::TypeKind::Text,
            origin: table.map(|t| theta_core::ColumnOrigin {
                schema: None,
                table: t.into(),
                column: name.into(),
            }),
        }
    }

    #[test]
    fn result_titles_name_a_table_only_when_all_columns_share_it() {
        let one = [col("id", Some("orders")), col("total", Some("orders"))];
        assert_eq!(result_title(1, "select * from orders", &one), "Result 1 · orders");
        let joined = [col("id", Some("orders")), col("name", Some("customers"))];
        assert_eq!(result_title(2, "select * from orders join customers", &joined), "Result 2");
        let computed = [col("id", Some("orders")), col("n", None)];
        assert_eq!(result_title(3, "select id, 1 n from orders", &computed), "Result 3");
        assert_eq!(result_title(1, "show tables", &[col("Tables_in_x", Some("TABLES"))]), "Result 1");
        assert_eq!(result_title(1, "select 1", &[]), "Result 1");
    }

    #[test]
    fn numbers() {
        assert_eq!(fmt_int(1234567), "1,234,567");
        assert_eq!(plural(1, "row", "rows"), "1 row");
        assert_eq!(plural(1000, "row", "rows"), "1,000 rows");
        assert_eq!(fmt_ms(std::time::Duration::from_millis(42)), "42 ms");
        assert_eq!(fmt_ms(std::time::Duration::from_millis(12_300)), "12.3 s");
    }
}
