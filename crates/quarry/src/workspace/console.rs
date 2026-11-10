//! Per-console state: the editor view, results of the last run and the
//! providers feeding completion and diagnostics.

use crate::exec::geo::GeoData;
use gpui_kit::{Entity, Subscription, Task};
use quarry_core::{Catalog, DatabaseKind, ResultSet};
use quarry_editor_view::{CompletionProvider, EditorView};
use quarry_grid::GridMeta;
use quarry_sql::{CompletionList, CompletionRequest};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

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

pub struct Console {
    pub key: String,
    pub editor: Entity<EditorView>,
    pub file: Option<PathBuf>,
    pub results: Vec<ResultItem>,
    pub active_result: usize,
    /// One-line outcome of the last statement when no rows came back.
    pub summary: Option<(String, bool)>,
    pub catalog: CatalogSlot,
    pub save_task: Option<Task<()>>,
    pub _subs: Vec<Subscription>,
}

pub struct SqlCompletion {
    pub kind: DatabaseKind,
    pub catalog: CatalogSlot,
}

impl CompletionProvider for SqlCompletion {
    fn complete(&self, text: &str, byte_offset: usize) -> CompletionList {
        let catalog = self.catalog.read().ok().and_then(|c| c.clone());
        quarry_sql::complete(&CompletionRequest {
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

    #[test]
    fn numbers() {
        assert_eq!(fmt_int(1234567), "1,234,567");
        assert_eq!(plural(1, "row", "rows"), "1 row");
        assert_eq!(plural(1000, "row", "rows"), "1,000 rows");
        assert_eq!(fmt_ms(std::time::Duration::from_millis(42)), "42 ms");
        assert_eq!(fmt_ms(std::time::Duration::from_millis(12_300)), "12.3 s");
    }
}
