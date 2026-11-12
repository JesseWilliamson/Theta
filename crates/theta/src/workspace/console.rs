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
    /// When the rows were fetched (or last changed by the user): a rollback makes stale what was produced
    /// after the transaction's first write.
    pub produced: std::time::Instant,
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
    pub templates: std::sync::Arc<crate::templates::TemplateStore>,
}

impl CompletionProvider for SqlCompletion {
    fn complete(&self, text: &str, byte_offset: usize) -> CompletionList {
        let catalog = self.catalog.read().ok().and_then(|c| c.clone());
        let mut list = theta_sql::complete(&CompletionRequest {
            sql: text,
            offset: byte_offset,
            kind: self.kind,
            catalog: catalog.as_deref(),
        });
        // The user's templates: offered wherever a word is being typed, and a template with the same
        // abbreviation as a built-in replaces it.
        let user: Vec<_> = self.templates.user().into_iter().filter(|t| t.applies_to(self.kind)).collect();
        if !user.is_empty() {
            let prefix = text.get(list.replace_range.clone()).unwrap_or("").to_lowercase();
            list.items.retain(|i| {
                i.kind != theta_sql::CompletionKind::Snippet
                    || !user.iter().any(|t| t.abbreviation.eq_ignore_ascii_case(&i.label))
            });
            if !prefix.is_empty() {
                for t in user.iter().filter(|t| t.abbreviation.to_lowercase().starts_with(&prefix)) {
                    list.items.push(theta_sql::CompletionItem {
                        label: t.abbreviation.clone(),
                        kind: theta_sql::CompletionKind::Snippet,
                        detail: (!t.description.is_empty()).then(|| t.description.clone()),
                        insert_text: t.body.clone(),
                        score: 140,
                    });
                }
                list.items.sort_by_key(|i| std::cmp::Reverse(i.score));
                list.items.truncate(theta_sql::complete::MAX_ITEMS);
            }
        }
        list
    }

    fn snippet_for(&self, word: &str) -> Option<String> {
        let all = theta_sql::templates::merged(self.kind, &self.templates.user());
        theta_sql::templates::expand(&all, word).map(|t| t.body.clone())
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
    theta_core::text::plural(n, one, many)
}

pub fn fmt_int(n: u64) -> String {
    theta_core::text::count(n)
}

/// The one duration format (see `theta_core::text::duration`).
pub fn fmt_ms(d: std::time::Duration) -> String {
    theta_core::text::duration(d)
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
        assert_eq!(fmt_ms(std::time::Duration::from_millis(12_300)), "12.30 s");
    }
}

/// The closing line of a run that stopped before its last statement, or `None` when nothing was skipped.
/// `index` is the 0-based statement that failed or was cancelled.
pub fn stopped_summary(index: usize, total: usize) -> Option<String> {
    let skipped = total.checked_sub(index + 1).filter(|n| *n > 0)?;
    Some(format!(
        "Stopped at statement {} of {total}: {} not run",
        index + 1,
        if skipped == 1 { "1 statement was".to_string() } else { format!("{skipped} statements were") }
    ))
}

/// The status-bar text for a failed run: the first line of the error, kept short.
pub fn failure_headline(error: &str) -> String {
    let first = error.lines().next().unwrap_or(error).trim();
    let mut chars = first.chars();
    let head: String = chars.by_ref().take(90).collect();
    if chars.next().is_some() { format!("{head}…") } else { head }
}

#[cfg(test)]
mod stopped_tests {
    use super::*;

    #[test]
    fn summary_counts_what_was_skipped() {
        assert_eq!(
            stopped_summary(2, 7).as_deref(),
            Some("Stopped at statement 3 of 7: 4 statements were not run")
        );
        assert_eq!(
            stopped_summary(5, 7).as_deref(),
            Some("Stopped at statement 6 of 7: 1 statement was not run")
        );
        assert_eq!(stopped_summary(6, 7), None, "the last statement leaves nothing behind");
        assert_eq!(stopped_summary(0, 1), None);
    }

    #[test]
    fn headline_is_one_short_line() {
        assert_eq!(failure_headline("boom\nCode: 1"), "boom");
        let long = "x".repeat(200);
        assert_eq!(failure_headline(&long).chars().count(), 91);
    }
}
